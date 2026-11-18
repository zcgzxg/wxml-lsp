//! Frozen AST schema metadata.
//!
//! Decoding, node-tree reconstruction and string-pool access are `P3-N` work.
//! This module only records the v9/header-64 layout so the future decoder and
//! its tests share the same constants.
//!
//! [`AstHeader`] is a descriptive scaffold. Exact word offsets are owned by
//! `P3-N` and must be verified against the pinned encoder before decoding any
//! payload; field names here are not a wire schema promise.

use crate::config::{AST_HEADER_BYTES, AST_NODE_STRIDE_BYTES, AST_PROTOCOL_VERSION};

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
    pub source_id: u32,
    /// Engine source lease metadata.
    pub source_lease: u32,
    /// Binder metadata marker.
    pub binder: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_v9_layout_is_frozen() {
        assert_eq!(AST_V9.protocol_version, 9);
        assert_eq!(AST_V9.header_bytes, 64);
        assert_eq!(AST_V9.node_stride_bytes, 28);
    }
}
