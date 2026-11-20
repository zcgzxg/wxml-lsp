//! Subresource Integrity (SRI) and SHA-256 verification for native assets.
//!
//! The delivery contract fixes asset identity with two independent digests:
//! a `sha512-<base64>` SRI for the packed tarball and a lowercase SHA-256 for
//! the archive or payload file. Both are checked before any extraction or
//! activation. Base64 handling is intentionally local and strict: standard
//! alphabet, mandatory padding, canonical trailing bits and no embedded
//! whitespace or URL-safe characters.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256, Sha512};

use crate::error::{DeliveryError, Result};
use crate::paths::validate_sha256;

/// Number of bytes in a SHA-512 digest.
pub const SHA512_BYTES: usize = 64;
/// Required algorithm prefix for asset integrity values.
pub const SHA512_SRI_PREFIX: &str = "sha512-";

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A parsed and validated `sha512-<base64>` Subresource Integrity value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sha512Integrity {
    digest: [u8; SHA512_BYTES],
}

impl Sha512Integrity {
    /// Wraps an already computed digest.
    #[must_use]
    pub const fn from_digest(digest: [u8; SHA512_BYTES]) -> Self {
        Self { digest }
    }

    /// Parses `sha512-<standard base64>` with canonical padding.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryError::Integrity`] for a missing or unsupported
    /// algorithm, malformed base64, a non-canonical encoding or a digest that
    /// is not exactly 64 bytes.
    pub fn parse(value: &str) -> Result<Self> {
        let encoded = value.strip_prefix(SHA512_SRI_PREFIX).ok_or_else(|| {
            DeliveryError::Integrity(
                "asset integrity must use the `sha512-` algorithm prefix".to_owned(),
            )
        })?;
        if encoded.is_empty() {
            return Err(DeliveryError::Integrity(
                "asset integrity has no base64 digest".to_owned(),
            ));
        }
        let bytes = decode_base64(encoded)?;
        let digest: [u8; SHA512_BYTES] = bytes.try_into().map_err(|_| {
            DeliveryError::Integrity(format!(
                "asset integrity digest must be {SHA512_BYTES} bytes"
            ))
        })?;
        Ok(Self { digest })
    }

    /// Returns the raw digest bytes.
    #[must_use]
    pub const fn digest(&self) -> &[u8; SHA512_BYTES] {
        &self.digest
    }

    /// Formats the digest in canonical SRI form.
    #[must_use]
    pub fn to_sri(&self) -> String {
        format!("{SHA512_SRI_PREFIX}{}", encode_base64(&self.digest))
    }
}

/// One verified asset with both of its fixed digests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedAsset {
    /// SHA-512 SRI computed from the asset bytes.
    pub sha512: Sha512Integrity,
    /// Lowercase SHA-256 computed from the asset bytes.
    pub sha256: String,
    /// Observed asset size.
    pub size: u64,
}

/// Computes the SHA-512 SRI for a byte slice.
#[must_use]
pub fn sha512_sri_bytes(bytes: &[u8]) -> Sha512Integrity {
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let digest: [u8; SHA512_BYTES] = hasher.finalize().into();
    Sha512Integrity::from_digest(digest)
}

/// Computes the SHA-512 SRI for a regular file.
///
/// # Errors
///
/// Returns an error when the path is not a regular file or cannot be read.
pub fn sha512_sri_file(path: &Path) -> Result<Sha512Integrity> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{}` must be a regular file",
            crate::paths::redact(path)
        )));
    }
    let mut file = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    let mut hasher = Sha512::new();
    hash_reader(&mut file, &mut hasher, path)?;
    let digest: [u8; SHA512_BYTES] = hasher.finalize().into();
    Ok(Sha512Integrity::from_digest(digest))
}

/// Verifies a byte slice against an SRI value.
///
/// # Errors
///
/// Returns [`DeliveryError::Integrity`] when the digest does not match.
pub fn verify_sri_bytes(bytes: &[u8], expected: &Sha512Integrity) -> Result<()> {
    let actual = sha512_sri_bytes(bytes);
    if actual != *expected {
        return Err(DeliveryError::Integrity(
            "asset SHA-512 SRI does not match".to_owned(),
        ));
    }
    Ok(())
}

/// Verifies a regular file against an SRI value.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be read or
/// [`DeliveryError::Integrity`] when the digest does not match.
pub fn verify_sri_file(path: &Path, expected: &Sha512Integrity) -> Result<()> {
    let actual = sha512_sri_file(path)?;
    if actual != *expected {
        return Err(DeliveryError::Integrity(
            "asset SHA-512 SRI does not match".to_owned(),
        ));
    }
    Ok(())
}

/// Verifies an asset against SRI, SHA-256 and its declared size in one pass.
///
/// # Errors
///
/// Returns an error for a non-regular file, an unreadable file, a size
/// mismatch, an invalid SHA-256 shape or either digest mismatch.
pub fn verify_asset_file(
    path: &Path,
    integrity: &str,
    sha256: &str,
    expected_size: u64,
) -> Result<VerifiedAsset> {
    let expected_sri = Sha512Integrity::parse(integrity)?;
    validate_sha256(sha256)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{}` must be a regular file",
            crate::paths::redact(path)
        )));
    }
    if metadata.len() != expected_size {
        return Err(DeliveryError::Integrity(format!(
            "asset size {} does not match the declared {expected_size} bytes",
            metadata.len()
        )));
    }
    let mut file = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    let mut sha512 = Sha512::new();
    let mut sha256_hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DeliveryError::io(path, error))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(read).map_err(|_| {
                DeliveryError::SizeLimit("asset size does not fit in u64".to_owned())
            })?)
            .ok_or_else(|| DeliveryError::SizeLimit("asset size overflows".to_owned()))?;
        sha512.update(&buffer[..read]);
        sha256_hasher.update(&buffer[..read]);
    }
    if size != expected_size {
        return Err(DeliveryError::Integrity(format!(
            "asset changed while hashing: {size} bytes instead of {expected_size}"
        )));
    }
    let actual_sri = Sha512Integrity::from_digest(sha512.finalize().into());
    if actual_sri != expected_sri {
        return Err(DeliveryError::Integrity(
            "asset SHA-512 SRI does not match".to_owned(),
        ));
    }
    let actual_sha256 = hex(&sha256_hasher.finalize());
    if actual_sha256 != sha256 {
        return Err(DeliveryError::Integrity(
            "asset SHA-256 does not match".to_owned(),
        ));
    }
    Ok(VerifiedAsset {
        sha512: actual_sri,
        sha256: actual_sha256,
        size,
    })
}

/// Verifies an in-memory asset against SRI, SHA-256 and size.
///
/// # Errors
///
/// See [`verify_asset_file`].
pub fn verify_asset_bytes(
    bytes: &[u8],
    integrity: &str,
    sha256: &str,
    expected_size: u64,
) -> Result<VerifiedAsset> {
    let expected_sri = Sha512Integrity::parse(integrity)?;
    validate_sha256(sha256)?;
    let size = u64::try_from(bytes.len())
        .map_err(|_| DeliveryError::SizeLimit("asset size does not fit in u64".to_owned()))?;
    if size != expected_size {
        return Err(DeliveryError::Integrity(format!(
            "asset size {size} does not match the declared {expected_size} bytes"
        )));
    }
    let actual_sri = sha512_sri_bytes(bytes);
    if actual_sri != expected_sri {
        return Err(DeliveryError::Integrity(
            "asset SHA-512 SRI does not match".to_owned(),
        ));
    }
    let actual_sha256 = crate::manifest::sha256_bytes(bytes);
    if actual_sha256 != sha256 {
        return Err(DeliveryError::Integrity(
            "asset SHA-256 does not match".to_owned(),
        ));
    }
    Ok(VerifiedAsset {
        sha512: actual_sri,
        sha256: actual_sha256,
        size,
    })
}

fn hash_reader<D: Digest>(reader: &mut impl Read, hasher: &mut D, path: &Path) -> Result<()> {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| DeliveryError::io(path, error))?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_char(byte: u8) -> Result<u8> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(DeliveryError::Integrity(
            "asset integrity contains a non-base64 character".to_owned(),
        )),
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(DeliveryError::Integrity(
            "asset integrity base64 must use canonical padding".to_owned(),
        ));
    }
    let chunks = bytes.len() / 4;
    let mut output = Vec::with_capacity(chunks * 3);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let last = index + 1 == chunks;
        let mut padding = 0_usize;
        let mut values = [0_u8; 4];
        for (position, byte) in chunk.iter().enumerate() {
            if *byte == b'=' {
                if !last || position < 2 {
                    return Err(DeliveryError::Integrity(
                        "asset integrity base64 padding is not canonical".to_owned(),
                    ));
                }
                padding += 1;
                values[position] = 0;
            } else {
                if padding > 0 {
                    return Err(DeliveryError::Integrity(
                        "asset integrity base64 padding is not canonical".to_owned(),
                    ));
                }
                values[position] = decode_char(*byte)?;
            }
        }
        output.push((values[0] << 2) | (values[1] >> 4));
        if padding < 2 {
            output.push((values[1] << 4) | (values[2] >> 2));
        }
        if padding == 0 {
            output.push((values[2] << 6) | values[3]);
        }
    }
    // Reject non-zero unused trailing bits: only the canonical encoding may
    // round-trip to the same text.
    let canonical = encode_base64(&output);
    if canonical != text {
        return Err(DeliveryError::Integrity(
            "asset integrity base64 is not canonical".to_owned(),
        ));
    }
    Ok(output)
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(BASE64_ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            BASE64_ALPHABET[usize::from((first & 0b0000_0011) << 4 | second >> 4)],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                BASE64_ALPHABET[usize::from((second & 0b0000_1111) << 2 | third >> 6)],
            ));
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(char::from(
                BASE64_ALPHABET[usize::from(third & 0b0011_1111)],
            ));
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA512: &str = "sha512-z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg==";

    #[test]
    fn parses_and_formats_the_known_empty_sha512_sri() {
        let integrity = Sha512Integrity::parse(EMPTY_SHA512).unwrap();
        assert_eq!(integrity.to_sri(), EMPTY_SHA512);
        assert_eq!(integrity.digest().len(), SHA512_BYTES);
        assert_eq!(sha512_sri_bytes(b""), integrity);
    }

    #[test]
    fn rejects_non_canonical_or_unsupported_sri_values() {
        for value in [
            "",
            "sha512-",
            "sha256-AAAA",
            "SHA512-AAAA",
            &EMPTY_SHA512.replace("sha512-", ""),
            &format!("{EMPTY_SHA512}=="),
            &format!("{EMPTY_SHA512} "),
            &EMPTY_SHA512.replace('+', "-"),
            &EMPTY_SHA512.replace('/', "_"),
            "sha512-AAA",
            "sha512-====",
            "sha512-AA==AAAA",
            // Non-zero unused bits in the last base64 character.
            &EMPTY_SHA512.replace("SfaPg==", "SfaPh=="),
        ] {
            assert!(
                Sha512Integrity::parse(value).is_err(),
                "should reject {value:?}"
            );
        }
    }

    #[test]
    fn verifies_bytes_and_files_against_both_digests() {
        let bytes = b"native engine asset";
        let sri = sha512_sri_bytes(bytes);
        let sha256 = crate::manifest::sha256_bytes(bytes);
        verify_asset_bytes(bytes, &sri.to_sri(), &sha256, 19).unwrap();

        let wrong = sha512_sri_bytes(b"another asset");
        assert!(verify_asset_bytes(bytes, &wrong.to_sri(), &sha256, 19).is_err());
        assert!(verify_asset_bytes(bytes, &sri.to_sri(), &"0".repeat(64), 19).is_err());
        assert!(verify_asset_bytes(bytes, &sri.to_sri(), &sha256, 18).is_err());
        assert!(verify_asset_bytes(bytes, &sri.to_sri(), "not-a-digest", 19).is_err());

        let base = std::env::var_os("WXML_TEST_TMPDIR").map_or_else(
            || std::path::PathBuf::from("/tmp/opencode"),
            std::path::PathBuf::from,
        );
        let path = base.join(format!("wxml-sri-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let verified = verify_asset_file(&path, &sri.to_sri(), &sha256, 19).unwrap();
        assert_eq!(verified.size, 19);
        verify_sri_file(&path, &sri).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn base64_round_trips_all_padding_lengths() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"abcd"] {
            let encoded = encode_base64(bytes);
            assert_eq!(decode_base64(&encoded).unwrap(), bytes);
        }
    }
}
