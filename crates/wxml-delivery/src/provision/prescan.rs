//! Bounded gzip/tar header pre-scan for npm tarballs.
//!
//! `tar-rs` reads GNU long-name and PAX extension headers into a `Vec` before
//! returning an entry. A crafted archive can therefore claim a multi-gigabyte
//! extension header and exhaust memory before the extraction loop runs. This
//! streaming pre-scan rejects oversized extension headers and conflicting PAX
//! sizes before the real extractor is allowed to see them.

use std::io::Read;

use crate::error::{DeliveryError, Result};
use crate::manifest::{MAX_EXTRACTED_BYTES, MAX_FILE_BYTES, MAX_FILES};

const MAX_EXTENSION_BYTES: u64 = 64 * 1024;
pub(super) const MAX_DECOMPRESSED_BYTES: u64 = MAX_EXTRACTED_BYTES + 64 * 1024 * 1024;
const TAR_BLOCK: usize = 512;
const TAR_BLOCK_BYTES: u64 = 512;

/// A `Read` adapter that fails once the decompressed budget is exhausted.
pub(super) struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> BoundedReader<R> {
    pub(super) const fn new(inner: R, remaining: u64) -> Self {
        Self { inner, remaining }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("decompressed size limit exceeded"));
        }
        let maximum = buffer
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..maximum])?;
        self.remaining -= u64::try_from(read).unwrap_or(0);
        Ok(read)
    }
}

/// Scans tar headers without materialising extension payloads.
pub(super) fn prescan_tar(mut reader: impl Read) -> Result<()> {
    let mut totals = PrescanTotals::default();
    let mut pending_pax_size = None;
    loop {
        let Some(header) = read_header(&mut reader)? else {
            return Ok(());
        };
        if header.iter().all(|byte| *byte == 0) {
            return Ok(());
        }
        let kind = header[156];
        let raw_size = parse_tar_size(&header[124..136])?;
        if prescan_extension(&mut reader, kind, raw_size, &mut pending_pax_size)? {
            continue;
        }
        prescan_entry(&mut reader, raw_size, &mut pending_pax_size, &mut totals)?;
    }
}

#[derive(Default)]
struct PrescanTotals {
    files: usize,
    bytes: u64,
}

fn prescan_extension(
    reader: &mut impl Read,
    kind: u8,
    raw_size: u64,
    pending_pax_size: &mut Option<u64>,
) -> Result<bool> {
    if kind == b'S' {
        return Err(DeliveryError::UnsafeArchive(
            "GNU sparse tar entries are not accepted".to_owned(),
        ));
    }
    if !matches!(kind, b'L' | b'K' | b'x' | b'g') {
        return Ok(false);
    }
    if raw_size > MAX_EXTENSION_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "tar extension header declares {raw_size} bytes"
        )));
    }
    if kind == b'x' {
        let content = read_exact_vec(reader, raw_size)?;
        *pending_pax_size = parse_pax_size(&content)?;
        skip_padding(reader, raw_size)?;
    } else {
        skip_padded(reader, raw_size)?;
    }
    Ok(true)
}

fn prescan_entry(
    reader: &mut impl Read,
    raw_size: u64,
    pending_pax_size: &mut Option<u64>,
    totals: &mut PrescanTotals,
) -> Result<()> {
    totals.files += 1;
    if totals.files > MAX_FILES.saturating_mul(2) {
        return Err(DeliveryError::SizeLimit(
            "archive contains too many entries".to_owned(),
        ));
    }
    let effective_size = match pending_pax_size.take() {
        Some(pax_size) if pax_size != raw_size => {
            return Err(DeliveryError::UnsafeArchive(
                "PAX size overrides the tar header size".to_owned(),
            ));
        }
        Some(pax_size) => pax_size,
        None => raw_size,
    };
    if effective_size > MAX_FILE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "entry declares {effective_size} bytes, above the per-file limit"
        )));
    }
    totals.bytes = totals
        .bytes
        .checked_add(effective_size)
        .ok_or_else(|| DeliveryError::SizeLimit("archive sizes overflow".to_owned()))?;
    if totals.bytes > MAX_EXTRACTED_BYTES {
        return Err(DeliveryError::SizeLimit(
            "archive exceeds the total extracted-size limit".to_owned(),
        ));
    }
    skip_padded(reader, effective_size)
}

fn read_header(reader: &mut impl Read) -> Result<Option<[u8; TAR_BLOCK]>> {
    let mut header = [0_u8; TAR_BLOCK];
    let mut read = 0;
    while read < TAR_BLOCK {
        match reader.read(&mut header[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => {
                return Err(DeliveryError::Integrity(
                    "tar header was truncated".to_owned(),
                ));
            }
            Ok(count) => read += count,
            Err(error) => return Err(DeliveryError::io_label("archive", error)),
        }
    }
    Ok(Some(header))
}

fn read_exact_vec(reader: &mut impl Read, size: u64) -> Result<Vec<u8>> {
    let size = usize::try_from(size)
        .map_err(|_| DeliveryError::SizeLimit("extension header is too large".to_owned()))?;
    let mut bytes = vec![0_u8; size];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| DeliveryError::io_label("archive", error))?;
    Ok(bytes)
}

fn skip_padding(reader: &mut impl Read, size: u64) -> Result<()> {
    let padding = (TAR_BLOCK_BYTES - size % TAR_BLOCK_BYTES) % TAR_BLOCK_BYTES;
    let mut buffer = [0_u8; TAR_BLOCK];
    let padding = usize::try_from(padding).unwrap_or(0);
    reader
        .read_exact(&mut buffer[..padding])
        .map_err(|error| DeliveryError::io_label("archive", error))
}

fn skip_padded(reader: &mut impl Read, size: u64) -> Result<()> {
    let padded = size
        .checked_add(TAR_BLOCK_BYTES - 1)
        .ok_or_else(|| DeliveryError::SizeLimit("tar size overflow".to_owned()))?
        / TAR_BLOCK_BYTES
        * TAR_BLOCK_BYTES;
    let mut remaining = padded;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let count = usize::try_from(remaining.min(8192)).unwrap_or(8192);
        reader
            .read_exact(&mut buffer[..count])
            .map_err(|error| DeliveryError::io_label("archive", error))?;
        remaining -= u64::try_from(count).unwrap_or(0);
    }
    Ok(())
}

fn parse_tar_size(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        return Err(DeliveryError::UnsafeArchive(
            "base-256 tar size fields are not accepted".to_owned(),
        ));
    }
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    let text = std::str::from_utf8(&field[..end])
        .map_err(|_| DeliveryError::UnsafeArchive("tar size field is not ASCII".to_owned()))?;
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err(DeliveryError::UnsafeArchive(
            "tar size field is not canonical octal".to_owned(),
        ));
    }
    u64::from_str_radix(text, 8)
        .map_err(|_| DeliveryError::UnsafeArchive("tar size field is malformed".to_owned()))
}

fn parse_pax_size(content: &[u8]) -> Result<Option<u64>> {
    let mut rest = content;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| DeliveryError::UnsafeArchive("PAX record has no length".to_owned()))?;
        let length_text = std::str::from_utf8(&rest[..space])
            .map_err(|_| DeliveryError::UnsafeArchive("PAX length is not ASCII".to_owned()))?;
        let length = length_text
            .parse::<usize>()
            .map_err(|_| DeliveryError::UnsafeArchive("PAX length is malformed".to_owned()))?;
        if length == 0 || length > rest.len() || length <= space + 1 {
            return Err(DeliveryError::UnsafeArchive(
                "PAX record length is inconsistent".to_owned(),
            ));
        }
        let record = &rest[..length];
        if record.last() != Some(&b'\n') {
            return Err(DeliveryError::UnsafeArchive(
                "PAX record has no newline".to_owned(),
            ));
        }
        let payload = &record[space + 1..record.len() - 1];
        if let Some(value) = payload.strip_prefix(b"size=") {
            let value = std::str::from_utf8(value)
                .map_err(|_| DeliveryError::UnsafeArchive("PAX size is not ASCII".to_owned()))?;
            let size = value.parse::<u64>().map_err(|_| {
                DeliveryError::UnsafeArchive("PAX size value is malformed".to_owned())
            })?;
            return Ok(Some(size));
        }
        rest = &rest[length..];
    }
    Ok(None)
}
