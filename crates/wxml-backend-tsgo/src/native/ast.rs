//! Frozen AST schema metadata, header validation and base64 decoding.
//!
//! `getSourceFile` returns a base64 JSON member whose decoded payload starts
//! with the 64-byte v9 header. This module validates the header and section
//! bounds before any node or string data is used. Full node-tree decoding,
//! binder data and structured msgpack remain later work; nothing here invents
//! a node tree.
//!
//! Header layout (little-endian, verified against the pinned encoder):
//!
//! | offset | bytes | field |
//! | ---: | ---: | --- |
//! | 0 | 4 | metadata word; version 9 in the high byte |
//! | 4 | 16 | xxh3-128 content hash |
//! | 20 | 4 | parse options |
//! | 24 | 4 | string offset table |
//! | 28 | 4 | string data pool |
//! | 32 | 4 | extended data |
//! | 36 | 4 | structured data |
//! | 40 | 4 | node table |
//! | 44 | 8 | source id |
//! | 52 | 8 | source lease |
//! | 60 | 4 | binder offset (`0` means none) |

use thiserror::Error;

use crate::config::{
    AST_HEADER_BYTES, AST_NODE_STRIDE_BYTES, AST_PROTOCOL_VERSION, MAX_AST_NODES, MAX_AST_STRINGS,
};

/// Frozen AST wire schema identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstSchema {
    /// Protocol version stored in the header metadata word.
    pub protocol_version: u32,
    /// Header size in bytes.
    pub header_bytes: usize,
    /// Node record size in bytes.
    pub node_stride_bytes: usize,
}

/// The v9/header-64 schema accepted by F0.
pub const AST_V9: AstSchema = AstSchema {
    protocol_version: AST_PROTOCOL_VERSION,
    header_bytes: AST_HEADER_BYTES,
    node_stride_bytes: AST_NODE_STRIDE_BYTES,
};

/// Decoded v9 header fields.
///
/// Every value is untrusted metadata. Section offsets are byte offsets from
/// the payload start, not from any enclosing buffer. `protocol_version` is the
/// high byte of the header metadata word (for v9, bytes `00 00 00 09`) rather
/// than a standalone leading field. `source_id` and `source_lease` are
/// informational only; Rust epoch binding, not this header, decides whether a
/// handle may be used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AstHeader {
    /// Total encoded payload size.
    pub bytes: u32,
    /// AST protocol version from the high byte of the metadata word.
    pub protocol_version: u32,
    /// Byte offset of the string offset table; F0 requires this to equal the
    /// 64-byte header size for v9.
    pub string_offsets: u32,
    /// Byte offset of the string data pool.
    pub string_data: u32,
    /// Byte offset of extended node data.
    pub extended: u32,
    /// Byte offset of structured node data.
    pub structured: u32,
    /// Byte offset of the node table.
    pub nodes: u32,
    /// Number of string-pool entries.
    pub nstrings: u32,
    /// Number of AST nodes.
    pub nnodes: u32,
    /// Engine source identifier metadata.
    pub source_id: u64,
    /// Engine source lease metadata.
    pub source_lease: u64,
    /// Binder data offset; zero means the field is absent.
    pub binder: u32,
}

/// Header or payload validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AstError {
    /// The payload is shorter than the 64-byte header.
    #[error("AST payload is shorter than the header")]
    TooShort,
    /// The metadata word does not carry protocol version 9.
    #[error("AST protocol version {observed} is not 9")]
    VersionMismatch {
        /// Version read from the metadata word.
        observed: u32,
    },
    /// `string_offsets` is not the frozen header size.
    #[error("AST string offset table starts at {observed}, expected 64")]
    StringOffsetMismatch {
        /// Observed offset.
        observed: u32,
    },
    /// Section offsets are not ordered or point outside the payload.
    #[error("AST sections are out of order or exceed the payload")]
    SectionOrder,
    /// The string offset table has an invalid length.
    #[error("AST string offset table length is not a multiple of 8")]
    StringTableMisaligned,
    /// The string pool exceeds the frozen entry budget.
    #[error("AST string pool has {count} entries, limit is {limit}")]
    TooManyStrings {
        /// Observed entry count.
        count: u32,
        /// Configured limit.
        limit: usize,
    },
    /// The node table has an invalid length.
    #[error("AST node table length is not a multiple of 28")]
    NodeTableMisaligned,
    /// The node table exceeds the frozen entry budget.
    #[error("AST node table has {count} nodes, limit is {limit}")]
    TooManyNodes {
        /// Observed node count.
        count: u32,
        /// Configured limit.
        limit: usize,
    },
    /// The binder offset points before the node table or past the payload.
    #[error("AST binder offset is outside the payload")]
    BinderOutOfRange,
    /// An identifier node has an invalid `pos`/`end` range.
    #[error("AST identifier node range is invalid")]
    NodeRangeInvalid,
    /// An identifier node range is outside the source text.
    #[error("AST identifier node range is outside the source text")]
    NodeRangeOutOfSource,
    /// An identifier node carries an unexpected `data` word.
    #[error("AST identifier node data is not a string index")]
    InvalidNodeData,
    /// A string table index is outside the declared pool.
    #[error("AST string index is outside the string table")]
    StringIndexOutOfRange,
    /// A string pool range is inverted or outside the payload.
    #[error("AST string pool range is invalid")]
    StringRangeInvalid,
    /// The payload is larger than `u32`.
    #[error("AST payload exceeds the u32 size field")]
    PayloadTooLarge,
}

impl AstHeader {
    /// Parses and validates the 64-byte header of one decoded payload.
    ///
    /// # Errors
    ///
    /// Returns [`AstError`] for a short payload, a non-v9 version, a wrong
    /// string-offset table start, unordered/overlapping sections, unaligned or
    /// over-budget string/node tables, or an out-of-range binder offset.
    pub fn parse(payload: &[u8]) -> Result<Self, AstError> {
        if payload.len() < AST_HEADER_BYTES {
            return Err(AstError::TooShort);
        }

        let metadata = read_u32(payload, 0).ok_or(AstError::TooShort)?;
        let version = metadata >> 24;
        if version != AST_PROTOCOL_VERSION {
            return Err(AstError::VersionMismatch { observed: version });
        }

        let parse_options = read_u32(payload, 20).ok_or(AstError::TooShort)?;
        let string_offsets = read_u32(payload, 24).ok_or(AstError::TooShort)?;
        let string_data = read_u32(payload, 28).ok_or(AstError::TooShort)?;
        let extended = read_u32(payload, 32).ok_or(AstError::TooShort)?;
        let structured = read_u32(payload, 36).ok_or(AstError::TooShort)?;
        let nodes = read_u32(payload, 40).ok_or(AstError::TooShort)?;
        let source_id = read_u64(payload, 44).ok_or(AstError::TooShort)?;
        let source_lease = read_u64(payload, 52).ok_or(AstError::TooShort)?;
        let binder = read_u32(payload, 60).ok_or(AstError::TooShort)?;

        let total = u32::try_from(payload.len()).map_err(|_| AstError::PayloadTooLarge)?;
        if string_offsets != u32::try_from(AST_HEADER_BYTES).unwrap_or(u32::MAX) {
            return Err(AstError::StringOffsetMismatch {
                observed: string_offsets,
            });
        }
        if !(string_offsets <= string_data
            && string_data <= extended
            && extended <= structured
            && structured <= nodes
            && nodes <= total)
        {
            return Err(AstError::SectionOrder);
        }

        let string_table_bytes = string_data - string_offsets;
        if string_table_bytes % 8 != 0 {
            return Err(AstError::StringTableMisaligned);
        }
        let nstrings = string_table_bytes / 8;
        if usize::try_from(nstrings).unwrap_or(usize::MAX) > MAX_AST_STRINGS {
            return Err(AstError::TooManyStrings {
                count: nstrings,
                limit: MAX_AST_STRINGS,
            });
        }

        let node_end = if binder == 0 { total } else { binder };
        if binder != 0 && (binder < nodes || binder > total) {
            return Err(AstError::BinderOutOfRange);
        }
        let node_bytes = node_end.saturating_sub(nodes);
        if node_bytes % u32::try_from(AST_NODE_STRIDE_BYTES).unwrap_or(u32::MAX) != 0 {
            return Err(AstError::NodeTableMisaligned);
        }
        let node_count = node_bytes / u32::try_from(AST_NODE_STRIDE_BYTES).unwrap_or(u32::MAX);
        if usize::try_from(node_count).unwrap_or(usize::MAX) > MAX_AST_NODES {
            return Err(AstError::TooManyNodes {
                count: node_count,
                limit: MAX_AST_NODES,
            });
        }

        let _ = parse_options;
        Ok(Self {
            bytes: total,
            protocol_version: version,
            string_offsets,
            string_data,
            extended,
            structured,
            nodes,
            nstrings,
            nnodes: node_count,
            source_id,
            source_lease,
            binder,
        })
    }
}

/// Finds the smallest Identifier node containing `offset` whose string-pool
/// text equals `name`.
///
/// The scan is flat and bounded by `nnodes`; it does not build a tree and does
/// not follow `next`/`parent`, so malformed links cannot loop. Node `pos`/`end`
/// are UTF-16 offsets and are validated against the source text length (not the
/// byte length of the AST payload). The string pool is compared as raw
/// UTF-8/WTF-8 bytes; no lossy replacement is introduced.
///
/// # Errors
///
/// Returns [`AstError`] when the header, a section read, a node range, a
/// string index or a string-pool range is invalid. Malformed identifier data
/// is a protocol error, never a silent skip.
#[allow(clippy::too_many_lines)]
pub fn find_identifier(
    payload: &[u8],
    offset: u32,
    name: &str,
    source_utf16_len: u32,
) -> Result<Option<(u32, u32)>, AstError> {
    let header = AstHeader::parse(payload)?;
    let string_table = header
        .string_offsets
        .checked_add(
            header
                .nstrings
                .checked_mul(8)
                .ok_or(AstError::SectionOrder)?,
        )
        .ok_or(AstError::SectionOrder)?;
    if string_table > header.string_data {
        return Err(AstError::SectionOrder);
    }

    let mut exact: Option<(u32, u32, u32)> = None;
    for index in 0..header.nnodes {
        let Some(base) = header.nodes.checked_add(
            index
                .checked_mul(u32::try_from(AST_NODE_STRIDE_BYTES).unwrap_or(u32::MAX))
                .ok_or(AstError::SectionOrder)?,
        ) else {
            return Err(AstError::SectionOrder);
        };
        let base = usize::try_from(base).map_err(|_| AstError::SectionOrder)?;
        let Some(kind) = read_u32(payload, base) else {
            return Err(AstError::SectionOrder);
        };
        if kind != 79 {
            continue;
        }
        let Some(pos) = read_u32(payload, base + 4) else {
            return Err(AstError::SectionOrder);
        };
        let Some(end) = read_u32(payload, base + 8) else {
            return Err(AstError::SectionOrder);
        };
        if pos > end {
            return Err(AstError::NodeRangeInvalid);
        }
        if end > source_utf16_len {
            return Err(AstError::NodeRangeOutOfSource);
        }
        let Some(data) = read_u32(payload, base + 20) else {
            return Err(AstError::SectionOrder);
        };
        if data & 0xC000_0000 != 0x4000_0000 {
            return Err(AstError::InvalidNodeData);
        }
        if !(pos <= offset && offset < end) {
            continue;
        }
        let width = end - pos;

        let string_index = data & 0x00FF_FFFF;
        let ordinal = string_index / 2;
        if ordinal >= header.nstrings {
            return Err(AstError::StringIndexOutOfRange);
        }
        let entry = header
            .string_offsets
            .checked_add(ordinal.checked_mul(8).ok_or(AstError::SectionOrder)?)
            .ok_or(AstError::SectionOrder)?;
        let entry = usize::try_from(entry).map_err(|_| AstError::SectionOrder)?;
        let Some(start) = read_u32(payload, entry) else {
            return Err(AstError::SectionOrder);
        };
        let Some(finish) = read_u32(payload, entry + 4) else {
            return Err(AstError::SectionOrder);
        };
        if finish < start {
            return Err(AstError::StringRangeInvalid);
        }
        let text_start = header
            .string_data
            .checked_add(start)
            .ok_or(AstError::StringRangeInvalid)?;
        let text_end = header
            .string_data
            .checked_add(finish)
            .ok_or(AstError::StringRangeInvalid)?;
        if text_end > header.extended {
            return Err(AstError::StringRangeInvalid);
        }
        let text_start = usize::try_from(text_start).map_err(|_| AstError::StringRangeInvalid)?;
        let text_end = usize::try_from(text_end).map_err(|_| AstError::StringRangeInvalid)?;
        let bytes = payload
            .get(text_start..text_end)
            .ok_or(AstError::StringRangeInvalid)?;
        if bytes == name.as_bytes() && exact.is_none_or(|(_, _, best)| width < best) {
            exact = Some((index, kind, width));
        }
    }

    Ok(exact.map(|(index, kind, _)| (index, kind)))
}

fn read_u32(payload: &[u8], offset: usize) -> Option<u32> {
    let bytes = payload.get(offset..offset.checked_add(4)?)?;
    let array: [u8; 4] = bytes.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

fn read_u64(payload: &[u8], offset: usize) -> Option<u64> {
    let bytes = payload.get(offset..offset.checked_add(8)?)?;
    let array: [u8; 8] = bytes.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

/// Strict standard base64 decoding for the `getSourceFile` payload.
///
/// Whitespace is ignored; padding is required when the input length is not a
/// multiple of four; any other character is rejected. Output is bounded by
/// `input.len() * 3 / 4`.
///
/// # Errors
///
/// Returns [`Base64Error`] for invalid characters, bad padding or impossible
/// lengths. The decoder never panics.
pub fn decode_base64(input: &str) -> Result<Vec<u8>, Base64Error> {
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    let mut padding = 0usize;
    let mut symbols = 0usize;

    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'=' {
            padding = padding.saturating_add(1);
            if padding > 2 {
                return Err(Base64Error::InvalidPadding);
            }
            continue;
        }
        if padding != 0 {
            return Err(Base64Error::InvalidPadding);
        }
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(Base64Error::InvalidCharacter),
        };
        symbols = symbols.saturating_add(1);
        accumulator = (accumulator << 6) | value;
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits -= 8;
            let shift = bits;
            let chunk = accumulator >> shift;
            let Ok(value) = u8::try_from(chunk & 0xff) else {
                return Err(Base64Error::InvalidLength);
            };
            output.push(value);
            accumulator &= (1u32 << shift).saturating_sub(1);
        }
    }

    if symbols % 4 == 1 {
        return Err(Base64Error::InvalidLength);
    }
    if symbols % 4 != 0 && padding == 0 {
        return Err(Base64Error::InvalidPadding);
    }
    if padding != 0 {
        let expected_padding = (4 - (symbols % 4)) % 4;
        if expected_padding != padding {
            return Err(Base64Error::InvalidPadding);
        }
    }

    Ok(output)
}

/// Base64 decoding failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Base64Error {
    /// The input contains a character outside the standard alphabet.
    #[error("base64 input contains an invalid character")]
    InvalidCharacter,
    /// Padding is malformed or appears before the end.
    #[error("base64 padding is malformed")]
    InvalidPadding,
    /// The encoded length cannot represent whole bytes.
    #[error("base64 length is invalid")]
    InvalidLength,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: u32, string_offsets: u32, sections: [u32; 4], binder: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[0..4].copy_from_slice(&(version << 24).to_le_bytes());
        bytes[24..28].copy_from_slice(&string_offsets.to_le_bytes());
        bytes[28..32].copy_from_slice(&sections[0].to_le_bytes());
        bytes[32..36].copy_from_slice(&sections[1].to_le_bytes());
        bytes[36..40].copy_from_slice(&sections[2].to_le_bytes());
        bytes[40..44].copy_from_slice(&sections[3].to_le_bytes());
        bytes[44..52].copy_from_slice(&7u64.to_le_bytes());
        bytes[52..60].copy_from_slice(&0u64.to_le_bytes());
        bytes[60..64].copy_from_slice(&binder.to_le_bytes());
        bytes
    }

    #[test]
    fn ast_v9_layout_is_frozen() {
        assert_eq!(AST_V9.protocol_version, 9);
        assert_eq!(AST_V9.header_bytes, 64);
        assert_eq!(AST_V9.node_stride_bytes, 28);
    }

    #[test]
    fn parses_a_valid_header_and_counts_tables() {
        // 64 header + 8 string table + 8 string bytes + 28 one node.
        let mut payload = header(9, 64, [72, 80, 80, 80], 108);
        payload.extend_from_slice(&[0u8; 8]);
        payload.extend_from_slice(&[0u8; 8]);
        payload.extend_from_slice(&[0u8; 28]);

        let parsed = AstHeader::parse(&payload).expect("valid header");
        assert_eq!(parsed.protocol_version, 9);
        assert_eq!(parsed.bytes, 108);
        assert_eq!(parsed.string_offsets, 64);
        assert_eq!(parsed.nstrings, 1);
        assert_eq!(parsed.nnodes, 1);
        assert_eq!(parsed.source_id, 7);
        assert_eq!(parsed.binder, 108);
    }

    #[test]
    fn rejects_wrong_version_short_payloads_and_section_order() {
        assert_eq!(
            AstHeader::parse(&[0u8; 63]).expect_err("short"),
            AstError::TooShort
        );

        let payload = header(8, 64, [72, 80, 80, 80], 0);
        assert!(matches!(
            AstHeader::parse(&payload),
            Err(AstError::VersionMismatch { observed: 8 })
        ));

        let payload = header(9, 68, [72, 80, 80, 80], 0);
        assert!(matches!(
            AstHeader::parse(&payload),
            Err(AstError::StringOffsetMismatch { observed: 68 })
        ));

        let payload = header(9, 64, [80, 72, 80, 80], 0);
        assert_eq!(
            AstHeader::parse(&payload).expect_err("order"),
            AstError::SectionOrder
        );

        let payload = header(9, 64, [72, 80, 80, 80], 107);
        let mut payload = payload;
        payload.extend_from_slice(&[0u8; 44]);
        assert_eq!(
            AstHeader::parse(&payload).expect_err("binder"),
            AstError::NodeTableMisaligned
        );
    }

    #[test]
    fn finds_an_identifier_by_name_and_position() {
        let mut payload = header(9, 64, [72, 73, 73, 73], 0);
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(b'u'); // string pool
        let node = payload.len();
        payload.extend_from_slice(&[0u8; 28]);
        // kind 79, pos 0, end 1, string index 0.
        payload[node..node + 4].copy_from_slice(&79u32.to_le_bytes());
        payload[node + 4..node + 8].copy_from_slice(&0u32.to_le_bytes());
        payload[node + 8..node + 12].copy_from_slice(&1u32.to_le_bytes());
        payload[node + 20..node + 24].copy_from_slice(&0x4000_0000u32.to_le_bytes());

        assert_eq!(
            find_identifier(&payload, 0, "u", 1).expect("identifier"),
            Some((0, 79))
        );
        assert_eq!(find_identifier(&payload, 1, "u", 1).expect("outside"), None);
        assert_eq!(
            find_identifier(&payload, 0, "v", 1).expect("name mismatch"),
            None
        );
    }

    #[test]
    fn finds_an_identifier_beyond_the_payload_byte_length() {
        // A 1 MiB comment makes the UTF-16 identifier offset much larger than
        // the small AST payload; the range check must use the source length,
        // not the payload byte length.
        let mut payload = header(9, 64, [72, 73, 73, 73], 0);
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(b'x');
        let node = payload.len();
        payload.extend_from_slice(&[0u8; 28]);
        payload[node..node + 4].copy_from_slice(&79u32.to_le_bytes());
        payload[node + 4..node + 8].copy_from_slice(&1_000_000u32.to_le_bytes());
        payload[node + 8..node + 12].copy_from_slice(&1_000_001u32.to_le_bytes());
        payload[node + 20..node + 24].copy_from_slice(&0x4000_0000u32.to_le_bytes());

        assert_eq!(
            find_identifier(&payload, 1_000_000, "x", 1_000_001).expect("identifier"),
            Some((0, 79))
        );
        assert_eq!(
            find_identifier(&payload, 1_000_000, "x", 1_000_000).expect_err("out of source"),
            AstError::NodeRangeOutOfSource
        );
    }

    #[test]
    fn rejects_malformed_identifier_data_and_string_ranges() {
        let mut payload = header(9, 64, [72, 73, 73, 73], 0);
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(b'x');
        let node = payload.len();
        payload.extend_from_slice(&[0u8; 28]);
        payload[node..node + 4].copy_from_slice(&79u32.to_le_bytes());
        payload[node + 4..node + 8].copy_from_slice(&0u32.to_le_bytes());
        payload[node + 8..node + 12].copy_from_slice(&1u32.to_le_bytes());
        payload[node + 20..node + 24].copy_from_slice(&0x4000_0000u32.to_le_bytes());

        let mut bad_data = payload.clone();
        bad_data[node + 20..node + 24].copy_from_slice(&0x0000_0000u32.to_le_bytes());
        assert_eq!(
            find_identifier(&bad_data, 0, "x", 1).expect_err("data"),
            AstError::InvalidNodeData
        );

        let mut bad_index = payload.clone();
        bad_index[node + 20..node + 24].copy_from_slice(&0x4000_0010u32.to_le_bytes());
        assert_eq!(
            find_identifier(&bad_index, 0, "x", 1).expect_err("index"),
            AstError::StringIndexOutOfRange
        );

        let mut bad_range = payload;
        bad_range[64..68].copy_from_slice(&2u32.to_le_bytes());
        bad_range[68..72].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            find_identifier(&bad_range, 0, "x", 1).expect_err("range"),
            AstError::StringRangeInvalid
        );
    }

    #[test]
    fn rejects_recorded_v5_header_shapes() {
        // A v5 payload with a 44-byte header is too short for v9.
        assert_eq!(
            AstHeader::parse(&[0u8; 44]).expect_err("v5 header"),
            AstError::TooShort
        );

        // A 64-byte payload that still carries version 5 in the metadata word.
        let payload = header(5, 44, [64, 64, 64, 64], 0);
        assert!(matches!(
            AstHeader::parse(&payload),
            Err(AstError::VersionMismatch { observed: 5 })
        ));
    }

    #[test]
    fn decodes_base64_and_rejects_malformed_input() {
        assert_eq!(decode_base64("").expect("empty"), Vec::<u8>::new());
        assert_eq!(decode_base64("aGVsbG8=").expect("hello"), b"hello");
        assert_eq!(decode_base64("aGVs\nbG8=").expect("whitespace"), b"hello");
        assert_eq!(decode_base64("AQID").expect("three bytes"), vec![1, 2, 3]);

        assert_eq!(
            decode_base64("a").expect_err("length"),
            Base64Error::InvalidLength
        );
        assert_eq!(
            decode_base64("aGVsbG8").expect_err("padding"),
            Base64Error::InvalidPadding
        );
        assert_eq!(
            decode_base64("aGVsbG8=x").expect_err("padding"),
            Base64Error::InvalidPadding
        );
        assert_eq!(
            decode_base64("****").expect_err("alphabet"),
            Base64Error::InvalidCharacter
        );
    }
}
