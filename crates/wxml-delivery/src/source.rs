//! Trusted release sources: HTTPS, loopback HTTP for tests, or a local directory.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use crate::error::{DeliveryError, Result};
use crate::index_v2::{IndexV2, ReleaseV2};
use crate::manifest::{
    Index, MAX_ARCHIVE_BYTES, MAX_INDEX_BYTES, Release, parse_index, sha256_file,
};
use crate::paths::{validate_archive_name, validate_sha256};

/// A validated release source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    /// HTTPS (or loopback HTTP) base URL.
    Remote(Url),
    /// Local release output directory.
    Local(PathBuf),
}

impl Source {
    /// Parses a remote source under the trusted-source policy.
    ///
    /// HTTPS is accepted for any host. Plain HTTP is accepted only for
    /// `localhost`, `127.0.0.1` and `[::1]`, which keeps the fixture-server
    /// tests off the network. Credentials, queries and fragments are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryError::UntrustedSource`] for unsupported schemes,
    /// non-loopback HTTP, credentials or malformed URLs.
    pub fn remote(raw: &str) -> Result<Self> {
        if raw.split(['/', '\\']).any(|segment| segment == "..") {
            return Err(DeliveryError::UntrustedSource(
                "source URL must not contain `..` segments".to_owned(),
            ));
        }
        let mut url = Url::parse(raw)
            .map_err(|error| DeliveryError::UntrustedSource(format!("malformed URL: {error}")))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(DeliveryError::UntrustedSource(
                "credentials in the source URL are not allowed".to_owned(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(DeliveryError::UntrustedSource(
                "query strings and fragments are not allowed in the source URL".to_owned(),
            ));
        }
        let scheme = url.scheme();
        if scheme != "https" {
            if scheme != "http" {
                return Err(DeliveryError::UntrustedSource(format!(
                    "scheme `{scheme}` is not allowed; use HTTPS"
                )));
            }
            if !is_loopback(url.host_str()) {
                return Err(DeliveryError::UntrustedSource(
                    "plain HTTP is allowed only for localhost test sources".to_owned(),
                ));
            }
        }
        if url.path().split('/').any(|segment| segment == "..") {
            return Err(DeliveryError::UntrustedSource(
                "source URL must not contain `..` segments".to_owned(),
            ));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(Self::Remote(url))
    }

    /// Canonicalizes a local release directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not an existing directory.
    pub fn local(path: &Path) -> Result<Self> {
        let canonical =
            std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
        if !canonical.is_dir() {
            return Err(DeliveryError::InvalidArgument(
                "local source is not a directory".to_owned(),
            ));
        }
        Ok(Self::Local(canonical))
    }

    /// Returns a short source description for logs.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Remote(url) => url.origin().ascii_serialization(),
            Self::Local(path) => crate::paths::redact(path),
        }
    }

    /// Reads and validates `index.json` from the source.
    ///
    /// # Errors
    ///
    /// Returns an error when the index is missing, too large, malformed or
    /// fails validation.
    pub fn read_index(&self) -> Result<Index> {
        let bytes = self.read_index_bytes()?;
        parse_index(&bytes)
    }

    /// Reads the raw, size-bounded `index.json` bytes from the source.
    ///
    /// Callers that support both schema versions inspect `schemaVersion` once
    /// and then use [`crate::manifest::parse_index`] or
    /// [`crate::index_v2::parse_index_v2`] on the same bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the index is missing, too large or unreadable.
    pub fn read_index_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Remote(base) => {
                let url = base
                    .join("index.json")
                    .map_err(|error| DeliveryError::UntrustedSource(error.to_string()))?;
                ensure_same_origin(base, &url)?;
                fetch_bytes(&url, MAX_INDEX_BYTES)
            }
            Self::Local(directory) => {
                crate::manifest::read_limited(&directory.join("index.json"), MAX_INDEX_BYTES)
            }
        }
    }

    /// Reads and validates a schema v2 native index from the source.
    ///
    /// # Errors
    ///
    /// Returns an error when the index is missing, too large, malformed or
    /// fails native validation.
    pub fn read_index_v2(&self) -> Result<IndexV2> {
        let bytes = self.read_index_bytes()?;
        crate::index_v2::parse_index_v2(&bytes)
    }

    /// Downloads or copies one release archive to `destination`.
    ///
    /// The caller must verify the archive hash and manifest afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error for an untrusted archive name, missing file, oversized
    /// archive, network failure or truncated transfer.
    pub fn fetch_archive(&self, release: &Release, destination: &Path) -> Result<()> {
        release.validate()?;
        validate_archive_name(&release.archive)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
        }
        match self {
            Self::Remote(base) => {
                let url = base
                    .join(&release.archive)
                    .map_err(|error| DeliveryError::UntrustedSource(error.to_string()))?;
                ensure_same_origin(base, &url)?;
                let client = http_client()?;
                let response = client.get(url).send().map_err(|error| {
                    DeliveryError::Network(format!("release download failed: {error}"))
                })?;
                if !response.status().is_success() {
                    return Err(DeliveryError::HttpStatus {
                        status: response.status().as_u16(),
                        resource: release.archive.clone(),
                    });
                }
                if let Some(length) = response.content_length() {
                    if length > MAX_ARCHIVE_BYTES {
                        return Err(DeliveryError::SizeLimit(
                            "release archive exceeds the size limit".to_owned(),
                        ));
                    }
                    if length != release.size {
                        return Err(DeliveryError::Integrity(format!(
                            "server declared {} bytes but the index says {}",
                            length, release.size
                        )));
                    }
                }
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|error| DeliveryError::io(destination, error))?;
                let written = std::io::copy(&mut response.take(MAX_ARCHIVE_BYTES + 1), &mut output)
                    .map_err(|error| {
                        let _ = std::fs::remove_file(destination);
                        DeliveryError::io(destination, error)
                    })?;
                if written > MAX_ARCHIVE_BYTES {
                    let _ = std::fs::remove_file(destination);
                    return Err(DeliveryError::SizeLimit(
                        "release archive exceeds the size limit".to_owned(),
                    ));
                }
                if written != release.size {
                    let _ = std::fs::remove_file(destination);
                    return Err(DeliveryError::Integrity(format!(
                        "downloaded {written} bytes but the index says {}",
                        release.size
                    )));
                }
            }
            Self::Local(directory) => {
                let archive = directory.join(&release.archive);
                let metadata = std::fs::symlink_metadata(&archive)
                    .map_err(|error| DeliveryError::io(&archive, error))?;
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err(DeliveryError::UnsafeArchive(
                        "local release archive is not a regular file".to_owned(),
                    ));
                }
                let canonical = std::fs::canonicalize(&archive)
                    .map_err(|error| DeliveryError::io(&archive, error))?;
                if !canonical.starts_with(directory) {
                    return Err(DeliveryError::UnsafeArchive(
                        "local release archive escapes its directory".to_owned(),
                    ));
                }
                if metadata.len() != release.size {
                    return Err(DeliveryError::Integrity(
                        "local release archive size does not match the index".to_owned(),
                    ));
                }
                std::fs::copy(&archive, destination)
                    .map_err(|error| DeliveryError::io(destination, error))?;
            }
        }
        let digest = sha256_file(destination)?;
        if digest != release.sha256 {
            let _ = std::fs::remove_file(destination);
            return Err(DeliveryError::Integrity(format!(
                "release archive SHA-256 mismatch for {}",
                release.version
            )));
        }
        validate_sha256(&release.sha256)?;
        Ok(())
    }

    /// Downloads or copies one schema v2 release archive to `destination` and
    /// verifies its size, SHA-256 and SHA-512 SRI before returning.
    ///
    /// # Errors
    ///
    /// Returns an error for an untrusted archive name, missing file, oversized
    /// archive, network failure, truncated transfer, size mismatch or either
    /// digest mismatch. A failed transfer never leaves a partial file behind.
    pub fn fetch_archive_v2(&self, release: &ReleaseV2, destination: &Path) -> Result<()> {
        release.validate()?;
        validate_archive_name(&release.archive)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
        }
        match self {
            Self::Remote(base) => {
                let url = base
                    .join(&release.archive)
                    .map_err(|error| DeliveryError::UntrustedSource(error.to_string()))?;
                ensure_same_origin(base, &url)?;
                let client = http_client()?;
                let response = client.get(url).send().map_err(|error| {
                    DeliveryError::Network(format!("release download failed: {error}"))
                })?;
                if !response.status().is_success() {
                    return Err(DeliveryError::HttpStatus {
                        status: response.status().as_u16(),
                        resource: release.archive.clone(),
                    });
                }
                if let Some(length) = response.content_length() {
                    if length > MAX_ARCHIVE_BYTES {
                        return Err(DeliveryError::SizeLimit(
                            "release archive exceeds the size limit".to_owned(),
                        ));
                    }
                    if length != release.size {
                        return Err(DeliveryError::Integrity(format!(
                            "server declared {} bytes but the index says {}",
                            length, release.size
                        )));
                    }
                }
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|error| DeliveryError::io(destination, error))?;
                let written = std::io::copy(&mut response.take(MAX_ARCHIVE_BYTES + 1), &mut output)
                    .map_err(|error| {
                        let _ = std::fs::remove_file(destination);
                        DeliveryError::io(destination, error)
                    })?;
                if written > MAX_ARCHIVE_BYTES {
                    let _ = std::fs::remove_file(destination);
                    return Err(DeliveryError::SizeLimit(
                        "release archive exceeds the size limit".to_owned(),
                    ));
                }
                if written != release.size {
                    let _ = std::fs::remove_file(destination);
                    return Err(DeliveryError::Integrity(format!(
                        "downloaded {written} bytes but the index says {}",
                        release.size
                    )));
                }
            }
            Self::Local(directory) => {
                let archive = directory.join(&release.archive);
                let metadata = std::fs::symlink_metadata(&archive)
                    .map_err(|error| DeliveryError::io(&archive, error))?;
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err(DeliveryError::UnsafeArchive(
                        "local release archive is not a regular file".to_owned(),
                    ));
                }
                let canonical = std::fs::canonicalize(&archive)
                    .map_err(|error| DeliveryError::io(&archive, error))?;
                if !canonical.starts_with(directory) {
                    return Err(DeliveryError::UnsafeArchive(
                        "local release archive escapes its directory".to_owned(),
                    ));
                }
                if metadata.len() != release.size {
                    return Err(DeliveryError::Integrity(
                        "local release archive size does not match the index".to_owned(),
                    ));
                }
                std::fs::copy(&archive, destination).map_err(|error| {
                    let _ = std::fs::remove_file(destination);
                    DeliveryError::io(destination, error)
                })?;
            }
        }
        if let Err(error) = release.verify_archive(destination) {
            let _ = std::fs::remove_file(destination);
            return Err(error);
        }
        Ok(())
    }
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .build()
        .map_err(|error| DeliveryError::Network(format!("HTTP client setup failed: {error}")))
}

fn fetch_bytes(url: &Url, maximum: u64) -> Result<Vec<u8>> {
    let client = http_client()?;
    let response = client
        .get(url.clone())
        .send()
        .map_err(|error| DeliveryError::Network(format!("request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(DeliveryError::HttpStatus {
            status: response.status().as_u16(),
            resource: url
                .path()
                .rsplit('/')
                .find(|part| !part.is_empty())
                .map_or_else(|| "index.json".to_owned(), ToOwned::to_owned),
        });
    }
    if let Some(length) = response.content_length() {
        if length > maximum {
            return Err(DeliveryError::SizeLimit(
                "response exceeds the size limit".to_owned(),
            ));
        }
    }
    let mut bytes = Vec::new();
    response
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DeliveryError::Network(format!("response read failed: {error}")))?;
    if bytes.len() as u64 > maximum {
        return Err(DeliveryError::SizeLimit(
            "response exceeds the size limit".to_owned(),
        ));
    }
    Ok(bytes)
}

fn ensure_same_origin(base: &Url, candidate: &Url) -> Result<()> {
    if base.scheme() != candidate.scheme()
        || base.host_str() != candidate.host_str()
        || base.port_or_known_default() != candidate.port_or_known_default()
        || !candidate.path().starts_with(base.path())
    {
        return Err(DeliveryError::UntrustedSource(
            "archive URL escapes the configured source origin".to_owned(),
        ));
    }
    Ok(())
}

fn is_loopback(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "::1" | "[::1]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_loopback_http_only() {
        Source::remote("https://example.invalid/releases/").unwrap();
        Source::remote("http://localhost:8123/releases").unwrap();
        Source::remote("http://127.0.0.1:8123").unwrap();
        Source::remote("http://[::1]:8123").unwrap();
        for source in [
            "http://example.com/releases",
            "http://127.0.0.2:8123",
            "ftp://example.com/releases",
            "file:///tmp/releases",
            "https://user:pass@example.com/releases",
            "https://example.com/releases?token=1",
            "https://example.com/a/../b",
        ] {
            assert!(Source::remote(source).is_err(), "{source}");
        }
    }

    #[test]
    fn normalizes_trailing_slash() {
        let Source::Remote(url) = Source::remote("https://example.invalid/releases").unwrap()
        else {
            panic!("expected remote source");
        };
        assert_eq!(url.as_str(), "https://example.invalid/releases/");
    }

    #[test]
    fn rejects_archive_names_that_escape_the_base() {
        let Source::Remote(base) = Source::remote("https://example.invalid/releases/").unwrap()
        else {
            panic!("expected remote source");
        };
        let candidate = Url::parse("https://example.invalid/other/x.tar.gz").unwrap();
        assert!(ensure_same_origin(&base, &candidate).is_err());
    }

    #[test]
    fn local_source_requires_a_directory() {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        assert!(Source::local(&path).is_ok());
        std::fs::write(path.join("file"), b"x").unwrap();
        assert!(Source::local(&path.join("file")).is_err());
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn remote_loopback_source_reads_index_and_archive() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let archive = b"archive".to_vec();
        let release = Release {
            version: "0.1.0".to_owned(),
            platform: "linux-x86_64".to_owned(),
            protocol_version: 2,
            node_minimum_version: "24.0.0".to_owned(),
            archive: "archive.tar.gz".to_owned(),
            sha256: crate::manifest::sha256_bytes(&archive),
            size: archive.len() as u64,
        };
        let index = serde_json::to_vec_pretty(&Index {
            schema_version: 1,
            package: "wxml-lsp".to_owned(),
            releases: vec![release.clone()],
        })
        .unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let text = String::from_utf8_lossy(&request);
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = if path.ends_with("index.json") {
                    index.clone()
                } else {
                    archive.clone()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let source = Source::remote(&format!("http://{address}/")).unwrap();
        let loaded = source.read_index().unwrap();
        assert_eq!(loaded.releases[0].version, "0.1.0");
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let destination = base.join(format!(
            "wxml-remote-source-{}-{}.tar.gz",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        source.fetch_archive(&release, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"archive");
        let _ = std::fs::remove_file(&destination);
        server.join().unwrap();
    }

    #[test]
    fn local_source_reads_index_and_copies_archive() {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-local-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("archive.tar.gz"), b"archive").unwrap();
        let release = Release {
            version: "0.1.0".to_owned(),
            platform: "linux-x86_64".to_owned(),
            protocol_version: 2,
            node_minimum_version: "24.0.0".to_owned(),
            archive: "archive.tar.gz".to_owned(),
            sha256: sha256_file(&path.join("archive.tar.gz")).unwrap(),
            size: 7,
        };
        let index = Index {
            schema_version: 1,
            package: "wxml-lsp".to_owned(),
            releases: vec![release.clone()],
        };
        std::fs::write(
            path.join("index.json"),
            serde_json::to_vec_pretty(&index).unwrap(),
        )
        .unwrap();
        let source = Source::local(&path).unwrap();
        let loaded = source.read_index().unwrap();
        assert_eq!(loaded.releases.len(), 1);
        let destination = path.join("copy.tar.gz");
        source.fetch_archive(&release, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"archive");
        std::fs::write(path.join("archive.tar.gz"), b"tampered").unwrap();
        assert!(
            source
                .fetch_archive(&release, &path.join("copy2.tar.gz"))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn bounded_reads_reject_oversized_local_indexes() {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        let index = path.join("index.json");
        std::fs::write(
            &index,
            vec![b' '; usize::try_from(MAX_INDEX_BYTES).unwrap() + 1],
        )
        .unwrap();
        assert!(crate::manifest::read_limited(&index, MAX_INDEX_BYTES).is_err());
        let _ = std::fs::remove_dir_all(&path);
    }
}
