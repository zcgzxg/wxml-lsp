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
    /// A requested node index is outside the node table.
    #[error("AST node index is outside the node table")]
    NodeIndexOutOfRange,
    /// The node table entry does not carry the expected node kind.
    #[error("AST node {index} has kind {observed}, expected {expected}")]
    NodeKindMismatch {
        /// Node table index.
        index: u32,
        /// Expected node kind from the handle.
        expected: u32,
        /// Observed node kind.
        observed: u32,
    },
    /// A `next`/`parent` link is out of range or cyclic.
    #[error("AST node link is outside the node table or cyclic")]
    NodeLinkInvalid,
    /// A string-pool identifier is valid WTF-8 but not valid Rust UTF-8.
    ///
    /// The pinned encoder stores JavaScript strings as WTF-8, so a lone
    /// surrogate is well-formed wire data that cannot become a Rust `String`.
    #[error("AST identifier text is not valid UTF-8")]
    NameNotUtf8,
}

/// Node kind of an Identifier in the frozen v9 schema.
pub const IDENTIFIER_KIND: u32 = 79;

/// Node kind of a `NodeList` in the frozen v9 schema.
pub const NODE_LIST_KIND: u32 = u32::MAX;

/// Node kind of a `SourceFile` in the frozen v9 schema.
pub const SOURCE_FILE_KIND: u32 = 308;

/// Node kind of a `StringLiteral`.
pub const STRING_LITERAL_KIND: u32 = 10;

/// Node kind of a `NumericLiteral`.
pub const NUMERIC_LITERAL_KIND: u32 = 8;

/// Node kind of an `ArrayBindingPattern`.
pub const ARRAY_BINDING_PATTERN_KIND: u32 = 207;

/// Node kind of an `ObjectBindingPattern`.
pub const OBJECT_BINDING_PATTERN_KIND: u32 = 208;

// The following node kinds are the pinned 7.1.0-beta AST v9 encoder values.
// They are *not* TypeScript's public `SyntaxKind` numbers: the native encoder
// uses its own enum. Each value was verified against a decoded `getSourceFile`
// payload from the pinned artifact. Keep them frozen with the schema.

/// `FunctionDeclaration`.
pub const FUNCTION_DECLARATION_KIND: u32 = 264;
/// `ClassDeclaration`.
pub const CLASS_DECLARATION_KIND: u32 = 265;
/// `InterfaceDeclaration`.
pub const INTERFACE_DECLARATION_KIND: u32 = 266;
/// `TypeAliasDeclaration`.
pub const TYPE_ALIAS_DECLARATION_KIND: u32 = 267;
/// `EnumDeclaration`.
pub const ENUM_DECLARATION_KIND: u32 = 268;
/// `ModuleDeclaration` (`namespace` and `module`).
pub const MODULE_DECLARATION_KIND: u32 = 269;
/// `ModuleBlock`.
pub const MODULE_BLOCK_KIND: u32 = 270;
/// `VariableStatement`.
pub const VARIABLE_STATEMENT_KIND: u32 = 245;
/// `VariableDeclarationList`.
pub const VARIABLE_DECLARATION_LIST_KIND: u32 = 263;
/// `VariableDeclaration`.
pub const VARIABLE_DECLARATION_KIND: u32 = 262;
/// `ImportEqualsDeclaration`.
pub const IMPORT_EQUALS_DECLARATION_KIND: u32 = 273;
/// `ImportDeclaration`.
pub const IMPORT_DECLARATION_KIND: u32 = 274;
/// `ExportAssignment`.
pub const EXPORT_ASSIGNMENT_KIND: u32 = 279;
/// `ExportDeclaration`.
pub const EXPORT_DECLARATION_KIND: u32 = 280;
/// `NamespaceExportDeclaration`.
pub const NAMESPACE_EXPORT_DECLARATION_KIND: u32 = 272;
/// `PropertyDeclaration`.
pub const PROPERTY_DECLARATION_KIND: u32 = 174;
/// `MethodSignature`.
pub const METHOD_SIGNATURE_KIND: u32 = 175;
/// `MethodDeclaration`.
pub const METHOD_DECLARATION_KIND: u32 = 176;
/// `ClassStaticBlockDeclaration`.
pub const CLASS_STATIC_BLOCK_KIND: u32 = 177;
/// `Constructor`.
pub const CONSTRUCTOR_KIND: u32 = 178;
/// `GetAccessor`.
pub const GET_ACCESSOR_KIND: u32 = 179;
/// `SetAccessor`.
pub const SET_ACCESSOR_KIND: u32 = 180;
/// `CallSignature`.
pub const CALL_SIGNATURE_KIND: u32 = 181;
/// `ConstructSignature`.
pub const CONSTRUCT_SIGNATURE_KIND: u32 = 182;
/// `IndexSignature`.
pub const INDEX_SIGNATURE_KIND: u32 = 183;
/// `PropertySignature`.
pub const PROPERTY_SIGNATURE_KIND: u32 = 173;
/// `EnumMember`.
pub const ENUM_MEMBER_KIND: u32 = 307;

/// One decoded node-table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstNode {
    /// Node-table index.
    pub index: u32,
    /// Node kind.
    pub kind: u32,
    /// Full start, including leading trivia, in UTF-16 code units.
    pub pos: u32,
    /// Exclusive token end in UTF-16 code units.
    pub end: u32,
    /// Node index of the next sibling, or `0`.
    pub next: u32,
    /// Node index of the parent, or `0`.
    pub parent: u32,
    /// Encoded children mask, string-pool index or extended-data offset.
    pub data: u32,
    /// Node flags.
    pub flags: u32,
}

/// One decoded identifier name with its exact source token range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstName {
    /// String-pool text decoded as UTF-8.
    pub text: String,
    /// Exact token range; leading trivia in the identifier node is trimmed.
    pub range: AstRange,
}

/// The declared name slot of one declaration node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstNameSlot {
    /// The named-child property is absent.
    Missing,
    /// The name slot is a decodable Identifier.
    Identifier(AstName),
    /// The name slot is present but is not an Identifier (string, numeric,
    /// computed name, binding pattern, ...). The contained value is the actual
    /// child node kind.
    NotIdentifier {
        /// Observed name-slot node kind.
        kind: u32,
    },
}

/// A validated v9 AST tree reader.
///
/// The reader borrows one decoded `getSourceFile` payload and validates every
/// node record it touches. It never follows `next`/`parent` links without a
/// bound, and it never allocates from untrusted counts beyond the already
/// validated string/node table caps in [`AstHeader::parse`].
#[derive(Debug, Clone, Copy)]
pub struct AstTree<'a> {
    payload: &'a [u8],
    header: AstHeader,
    source_utf16_len: u32,
}

impl<'a> AstTree<'a> {
    /// Validates the header and creates a tree reader for `payload`.
    ///
    /// `source_utf16_len` is the UTF-16 length of the *same* source snapshot
    /// that produced the payload; node ranges are checked against it.
    ///
    /// # Errors
    ///
    /// Returns [`AstError`] for a malformed header. Node records are validated
    /// lazily by [`Self::node`].
    pub fn parse(payload: &'a [u8], source_utf16_len: u32) -> Result<Self, AstError> {
        let header = AstHeader::parse(payload)?;
        Ok(Self {
            payload,
            header,
            source_utf16_len,
        })
    }

    /// Returns the validated header.
    #[must_use]
    pub const fn header(&self) -> AstHeader {
        self.header
    }

    /// Returns one validated node record.
    ///
    /// # Errors
    ///
    /// Returns [`AstError`] for a nil/out-of-range index, a malformed record,
    /// an inverted/out-of-source range, or an out-of-table link.
    pub fn node(&self, index: u32) -> Result<AstNode, AstError> {
        if index == 0 || index >= self.header.nnodes {
            return Err(AstError::NodeIndexOutOfRange);
        }
        let base = node_base(&self.header, index)?;
        let kind = read_u32(self.payload, base).ok_or(AstError::SectionOrder)?;
        let pos = read_u32(self.payload, base + 4).ok_or(AstError::SectionOrder)?;
        let end = read_u32(self.payload, base + 8).ok_or(AstError::SectionOrder)?;
        let next = read_u32(self.payload, base + 12).ok_or(AstError::SectionOrder)?;
        let parent = read_u32(self.payload, base + 16).ok_or(AstError::SectionOrder)?;
        let data = read_u32(self.payload, base + 20).ok_or(AstError::SectionOrder)?;
        let flags = read_u32(self.payload, base + 24).ok_or(AstError::SectionOrder)?;
        if pos > end {
            return Err(AstError::NodeRangeInvalid);
        }
        if end > self.source_utf16_len {
            return Err(AstError::NodeRangeOutOfSource);
        }
        if next > self.header.nnodes || (parent != 0 && parent >= self.header.nnodes) {
            return Err(AstError::NodeLinkInvalid);
        }
        Ok(AstNode {
            index,
            kind,
            pos,
            end,
            next,
            parent,
            data,
            flags,
        })
    }

    /// Returns the exclusive end of one node's subtree.
    ///
    /// # Errors
    ///
    /// Returns [`AstError`] for an out-of-range index or malformed links.
    pub fn subtree_end(&self, index: u32) -> Result<u32, AstError> {
        if index == 0 || index >= self.header.nnodes {
            return Err(AstError::NodeIndexOutOfRange);
        }
        subtree_end(self.payload, &self.header, index)
    }

    /// Returns the direct children of one node in source order.
    ///
    /// `NodeList` nodes appear as ordinary children; use [`Self::list_items`] to
    /// expand one. The walk follows each child's subtree end, so it is linear
    /// in the number of direct children and the depth of the last child.
    ///
    /// # Errors
    ///
    /// Returns [`AstError`] for malformed links or records.
    pub fn direct_children(&self, node: AstNode) -> Result<Vec<AstNode>, AstError> {
        let end = self.subtree_end(node.index)?;
        let mut children = Vec::new();
        let mut current = node.index.checked_add(1).ok_or(AstError::NodeLinkInvalid)?;
        while current < end {
            let child = self.node(current)?;
            if child.parent != node.index {
                return Err(AstError::NodeLinkInvalid);
            }
            children.push(child);
            let next = self.subtree_end(current)?;
            if next <= current || next > end {
                return Err(AstError::NodeLinkInvalid);
            }
            current = next;
        }
        if current != end {
            return Err(AstError::NodeLinkInvalid);
        }
        Ok(children)
    }

    /// Expands one `NodeList` into its direct items.
    ///
    /// # Errors
    ///
    /// Returns [`AstError::NodeKindMismatch`] for a non-`NodeList` node and
    /// propagates link/record failures.
    pub fn list_items(&self, list: AstNode) -> Result<Vec<AstNode>, AstError> {
        if list.kind != NODE_LIST_KIND {
            return Err(AstError::NodeKindMismatch {
                index: list.index,
                expected: NODE_LIST_KIND,
                observed: list.kind,
            });
        }
        self.direct_children(list)
    }

    /// Returns the child encoded at one children-mask property bit.
    ///
    /// The low byte of a children-data word is the mask of present named
    /// properties, in the same order as the encoded direct children. This is
    /// how a container's member list is selected without guessing from node
    /// kinds.
    ///
    /// Provenance: the mask comes from the pinned 7.1.0-beta generated encoder
    /// (`getChildrenPropertyMask`), and the direct-child order matches that
    /// generated property order. Callers must pass the bit for the specific
    /// declaration kind; see `native::document_symbol::name_slot_bit` for the
    /// documented table.
    ///
    /// # Errors
    ///
    /// Returns [`AstError::InvalidNodeData`] when the node does not carry
    /// children data, or [`AstError::NodeLinkInvalid`] when the mask and the
    /// direct children disagree.
    pub fn child_property(&self, node: AstNode, bit: u8) -> Result<Option<AstNode>, AstError> {
        if bit >= 8 {
            return Err(AstError::NodeLinkInvalid);
        }
        if node.data & 0xC000_0000 != 0 {
            return Err(AstError::InvalidNodeData);
        }
        let mask = node.data & 0xFF;
        if mask & (1u32 << bit) == 0 {
            return Ok(None);
        }
        let children = self.direct_children(node)?;
        let mut remaining = children.into_iter();
        for current_bit in 0..8u8 {
            if mask & (1u32 << current_bit) == 0 {
                continue;
            }
            let Some(child) = remaining.next() else {
                return Err(AstError::NodeLinkInvalid);
            };
            if current_bit == bit {
                return Ok(Some(child));
            }
        }
        if remaining.next().is_some() {
            return Err(AstError::NodeLinkInvalid);
        }
        Ok(None)
    }

    /// Returns the named-child property at one children-mask bit as a name
    /// slot.
    ///
    /// This is the only declaration-name accessor. It never falls back to the
    /// first Identifier anywhere in the subtree: a destructuring
    /// `VariableDeclaration` has an `ObjectBindingPattern`/`ArrayBindingPattern`
    /// in its name slot, and a class property with a string name has a
    /// `StringLiteral` there. Both must be reported as gaps, while the
    /// initializer `Identifier` (`config`/`fallback`) must never be published
    /// as the symbol name.
    ///
    /// # Errors
    ///
    /// Returns structural [`AstError`] values for malformed links/records. A
    /// candidate-local Identifier decode failure is returned as a
    /// non-structural [`AstError`] so the caller can skip only that
    /// declaration.
    pub fn name_slot(&self, node: AstNode, bit: u8) -> Result<AstNameSlot, AstError> {
        match self.child_property(node, bit)? {
            None => Ok(AstNameSlot::Missing),
            Some(child) if child.kind == IDENTIFIER_KIND => {
                self.identifier_name(child).map(AstNameSlot::Identifier)
            }
            Some(child) => Ok(AstNameSlot::NotIdentifier { kind: child.kind }),
        }
    }

    /// Returns the decoded string-pool bytes addressed by a node data word.
    ///
    /// # Errors
    ///
    /// Propagates string-index/range failures from the frozen schema.
    pub fn string_data(&self, data: u32) -> Result<&'a [u8], AstError> {
        string_bytes(self.payload, &self.header, data)
    }

    /// Finds the root `SourceFile` node.
    ///
    /// # Errors
    ///
    /// Returns [`AstError::NodeKindMismatch`] when no `SourceFile` exists and
    /// propagates node-record failures while searching.
    pub fn source_file(&self) -> Result<AstNode, AstError> {
        let first = self.node(1)?;
        if first.kind == SOURCE_FILE_KIND {
            return Ok(first);
        }
        for index in 2..self.header.nnodes {
            let node = self.node(index)?;
            if node.kind == SOURCE_FILE_KIND {
                return Ok(node);
            }
        }
        Err(AstError::NodeKindMismatch {
            index: 1,
            expected: SOURCE_FILE_KIND,
            observed: first.kind,
        })
    }

    fn identifier_name(&self, node: AstNode) -> Result<AstName, AstError> {
        let bytes = self.string_data(node.data)?;
        let text = std::str::from_utf8(bytes).map_err(|_| AstError::NameNotUtf8)?;
        let token_units = u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX);
        if token_units == 0 || node.end < token_units {
            return Err(AstError::NodeRangeInvalid);
        }
        let start = node.end - token_units;
        if start < node.pos {
            return Err(AstError::NodeRangeInvalid);
        }
        Ok(AstName {
            text: text.to_owned(),
            range: AstRange::new(start, node.end),
        })
    }
}

/// Half-open UTF-16 code-unit range decoded from an AST node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstRange {
    /// Inclusive token start after trimming leading trivia.
    pub start: u32,
    /// Exclusive token end.
    pub end: u32,
}

impl AstRange {
    /// Creates a range without validating it against any text.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

impl AstError {
    /// Returns whether the failure is payload-wide corruption rather than a
    /// candidate-local node/range problem.
    ///
    /// Structural failures must abort the query as
    /// [`crate::native::NativeError::Protocol`]; missing or mismatched
    /// candidate nodes and out-of-source ranges may degrade one candidate to
    /// `Partial`.
    #[must_use]
    pub const fn is_structural(self) -> bool {
        !matches!(
            self,
            Self::NodeIndexOutOfRange
                | Self::NodeKindMismatch { .. }
                | Self::NodeRangeInvalid
                | Self::NodeRangeOutOfSource
                | Self::NameNotUtf8
        )
    }
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
        if kind != IDENTIFIER_KIND {
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

/// Finds the name token of a declaration node by node-table index.
///
/// Declaration handles returned by `getReferencedSymbolsForNode` address a
/// declaration node (for example a `PropertySignature` or an
/// `InterfaceDeclaration`), not an Identifier. This function walks the flat
/// pre-order node table from the declaration into its subtree, bounded by the
/// nearest `next` link, and prefers an Identifier whose `parent` is the
/// declaration itself (the name slot). Decorator, heritage and initializer
/// identifiers can share the same text, so they only win when no direct name
/// slot matches.
///
/// Identifier `pos` includes leading trivia in the frozen encoder, while `end`
/// is the exact token end. The token start is therefore derived as
/// `end - utf16_len(name)` and must stay inside the node's `pos..end` range.
/// The caller must still compare the source text in `start..end` with `name`;
/// this function only proves the AST side.
///
/// # Errors
///
/// Returns [`AstError`] for a malformed header, an out-of-range node index, a
/// node-kind mismatch, malformed node links, or an invalid node range or string
/// reference. A declaration without a matching name Identifier is `Ok(None)`.
pub fn find_declaration_name(
    payload: &[u8],
    node_index: u32,
    node_kind: u32,
    name: &str,
    source_utf16_len: u32,
) -> Result<Option<AstRange>, AstError> {
    if node_index == 0 {
        // Index zero is the v9 nil sentinel, never a declaration handle.
        return Err(AstError::NodeIndexOutOfRange);
    }
    let header = AstHeader::parse(payload)?;
    let base = node_base(&header, node_index)?;
    let observed = read_u32(payload, base).ok_or(AstError::SectionOrder)?;
    if observed != node_kind {
        return Err(AstError::NodeKindMismatch {
            index: node_index,
            expected: node_kind,
            observed,
        });
    }
    if observed == IDENTIFIER_KIND {
        return identifier_range(payload, &header, base, name, source_utf16_len);
    }

    let end = subtree_end(payload, &header, node_index)?;
    let mut fallback = None;
    let mut index = node_index.saturating_add(1);
    while index < end {
        let base = node_base(&header, index)?;
        let kind = read_u32(payload, base).ok_or(AstError::SectionOrder)?;
        if kind == IDENTIFIER_KIND {
            if let Some(range) = identifier_range(payload, &header, base, name, source_utf16_len)? {
                // A direct child is the declaration's name slot. Decorators,
                // heritage clauses and initializers can contain identifiers
                // with the same text; they must never win over the name slot.
                let parent = read_u32(payload, base + 16).ok_or(AstError::SectionOrder)?;
                if parent == node_index {
                    return Ok(Some(range));
                }
                if fallback.is_none() {
                    fallback = Some(range);
                }
            }
        }
        index = index.saturating_add(1);
    }
    Ok(fallback)
}

/// Returns the full `pos..end` span of one node by node-table index.
///
/// Unlike [`find_declaration_name`], this span includes leading trivia. It is
/// used only to bound a caller-supplied Identifier position when the token text
/// cannot be verified (for example an escaped identifier).
///
/// # Errors
///
/// Returns [`AstError`] for a malformed header, an out-of-range node index, a
/// node-kind mismatch, or a range outside the source text.
pub fn node_span(
    payload: &[u8],
    node_index: u32,
    node_kind: u32,
    source_utf16_len: u32,
) -> Result<AstRange, AstError> {
    if node_index == 0 {
        return Err(AstError::NodeIndexOutOfRange);
    }
    let header = AstHeader::parse(payload)?;
    let base = node_base(&header, node_index)?;
    let observed = read_u32(payload, base).ok_or(AstError::SectionOrder)?;
    if observed != node_kind {
        return Err(AstError::NodeKindMismatch {
            index: node_index,
            expected: node_kind,
            observed,
        });
    }
    let pos = read_u32(payload, base + 4).ok_or(AstError::SectionOrder)?;
    let end = read_u32(payload, base + 8).ok_or(AstError::SectionOrder)?;
    if pos > end {
        return Err(AstError::NodeRangeInvalid);
    }
    if end > source_utf16_len {
        return Err(AstError::NodeRangeOutOfSource);
    }
    Ok(AstRange::new(pos, end))
}

/// Decodes one Identifier node's exact token range and checks its pool name.
fn identifier_range(
    payload: &[u8],
    header: &AstHeader,
    base: usize,
    name: &str,
    source_utf16_len: u32,
) -> Result<Option<AstRange>, AstError> {
    let pos = read_u32(payload, base + 4).ok_or(AstError::SectionOrder)?;
    let end = read_u32(payload, base + 8).ok_or(AstError::SectionOrder)?;
    if pos > end {
        return Err(AstError::NodeRangeInvalid);
    }
    if end > source_utf16_len {
        return Err(AstError::NodeRangeOutOfSource);
    }
    let data = read_u32(payload, base + 20).ok_or(AstError::SectionOrder)?;
    if string_bytes(payload, header, data)? != name.as_bytes() {
        return Ok(None);
    }

    let token_units = u32::try_from(name.encode_utf16().count()).unwrap_or(u32::MAX);
    if token_units == 0 || end < token_units {
        return Ok(None);
    }
    let start = end - token_units;
    if start < pos {
        // The pool name does not correspond to a source token of the same
        // length, for example an escaped identifier. Let the caller treat the
        // candidate as undecodable instead of publishing a shifted range.
        return Ok(None);
    }
    Ok(Some(AstRange::new(start, end)))
}

/// Returns the exclusive subtree end for one node table index.
fn subtree_end(payload: &[u8], header: &AstHeader, index: u32) -> Result<u32, AstError> {
    let mut current = index;
    let mut steps = 0u32;
    loop {
        if steps > header.nnodes {
            return Err(AstError::NodeLinkInvalid);
        }
        steps = steps.saturating_add(1);
        let base = node_base(header, current)?;
        let next = read_u32(payload, base + 12).ok_or(AstError::SectionOrder)?;
        if next != 0 {
            if next <= current || next > header.nnodes {
                return Err(AstError::NodeLinkInvalid);
            }
            return Ok(next);
        }
        let parent = read_u32(payload, base + 16).ok_or(AstError::SectionOrder)?;
        if parent == 0 {
            return Ok(header.nnodes);
        }
        if parent >= header.nnodes || parent == current {
            return Err(AstError::NodeLinkInvalid);
        }
        current = parent;
    }
}

/// Reads the UTF-8/WTF-8 string-pool bytes addressed by a node `data` word.
fn string_bytes<'a>(
    payload: &'a [u8],
    header: &AstHeader,
    data: u32,
) -> Result<&'a [u8], AstError> {
    if data & 0xC000_0000 != 0x4000_0000 {
        return Err(AstError::InvalidNodeData);
    }
    let ordinal = (data & 0x00FF_FFFF) / 2;
    if ordinal >= header.nstrings {
        return Err(AstError::StringIndexOutOfRange);
    }
    let entry = header
        .string_offsets
        .checked_add(ordinal.checked_mul(8).ok_or(AstError::SectionOrder)?)
        .ok_or(AstError::SectionOrder)?;
    let entry = usize::try_from(entry).map_err(|_| AstError::SectionOrder)?;
    let start = read_u32(payload, entry).ok_or(AstError::SectionOrder)?;
    let finish = read_u32(payload, entry + 4).ok_or(AstError::SectionOrder)?;
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
    payload
        .get(text_start..text_end)
        .ok_or(AstError::StringRangeInvalid)
}

/// Resolves a node table index to its record offset.
fn node_base(header: &AstHeader, index: u32) -> Result<usize, AstError> {
    if index >= header.nnodes {
        return Err(AstError::NodeIndexOutOfRange);
    }
    let stride = u32::try_from(AST_NODE_STRIDE_BYTES).unwrap_or(u32::MAX);
    let base = header
        .nodes
        .checked_add(index.checked_mul(stride).ok_or(AstError::SectionOrder)?)
        .ok_or(AstError::SectionOrder)?;
    usize::try_from(base).map_err(|_| AstError::SectionOrder)
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

    /// Builds a minimal v9 payload with a string pool and a node table.
    fn tree_payload(strings: &[&[u8]], nodes: &[[u32; 7]]) -> Vec<u8> {
        let mut table = Vec::new();
        let mut pool = Vec::new();
        for string in strings {
            let start = u32::try_from(pool.len()).expect("pool start");
            pool.extend_from_slice(string);
            let end = u32::try_from(pool.len()).expect("pool end");
            table.extend_from_slice(&start.to_le_bytes());
            table.extend_from_slice(&end.to_le_bytes());
        }
        let string_data = 64 + u32::try_from(table.len()).expect("table length");
        let extended = string_data + u32::try_from(pool.len()).expect("pool length");
        let nodes_offset = extended;
        let mut payload = header(9, 64, [string_data, extended, extended, nodes_offset], 0);
        payload.extend_from_slice(&table);
        payload.extend_from_slice(&pool);
        for node in nodes {
            for word in node {
                payload.extend_from_slice(&word.to_le_bytes());
            }
        }
        payload
    }

    const fn string_word(ordinal: u32) -> u32 {
        0x4000_0000 | (ordinal * 2)
    }

    #[test]
    fn finds_a_declaration_name_by_trimming_leading_trivia() {
        let payload = tree_payload(
            &[b"User", b"name"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                // Declaration starts at 0 and includes leading trivia.
                [266, 0, 20, 7, 2, 0x13, 0],
                // Identifier pos includes trivia (4..10), token is 6..10.
                [79, 4, 10, 0, 3, string_word(0), 0],
                [173, 12, 28, 0, 3, 0xA, 0],
                [79, 12, 22, 0, 5, string_word(1), 0],
                // A later sibling declaration with the same name.
                [266, 30, 38, 0, 2, 0x13, 0],
                [79, 32, 40, 0, 7, string_word(0), 0],
            ],
        );

        assert_eq!(
            find_declaration_name(&payload, 3, 266, "User", 40).expect("declaration"),
            Some(AstRange::new(6, 10))
        );
        assert_eq!(
            find_declaration_name(&payload, 5, 173, "name", 40).expect("property"),
            Some(AstRange::new(18, 22))
        );
        assert_eq!(
            find_declaration_name(&payload, 4, 79, "User", 40).expect("identifier"),
            Some(AstRange::new(6, 10))
        );
    }

    #[test]
    fn prefers_the_direct_name_slot_over_a_decorator_identifier() {
        let payload = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 60, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 59, 0, 1, 1, 0],
                // Class declaration; the decorator comes first in pre-order.
                [300, 0, 50, 8, 2, 0, 0],
                [301, 0, 10, 5, 3, 0, 0],
                [79, 4, 8, 0, 4, string_word(0), 0],
                // The class name Identifier is a direct child of the class.
                [79, 39, 44, 0, 3, string_word(0), 0],
                [302, 45, 50, 0, 3, 0, 0],
                [266, 52, 58, 0, 2, 0x13, 0],
            ],
        );

        assert_eq!(
            find_declaration_name(&payload, 3, 300, "User", 60).expect("class name"),
            Some(AstRange::new(40, 44)),
            "the decorator identifier must not win over the declaration name slot"
        );
    }

    #[test]
    fn falls_back_to_a_nested_name_when_there_is_no_direct_slot() {
        let payload = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 20, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 19, 0, 1, 1, 0],
                [300, 0, 20, 6, 2, 0, 0],
                [301, 0, 15, 0, 3, 0, 0],
                [79, 4, 10, 0, 4, string_word(0), 0],
            ],
        );
        assert_eq!(
            find_declaration_name(&payload, 3, 300, "User", 20).expect("fallback"),
            Some(AstRange::new(6, 10))
        );
    }

    #[test]
    fn declaration_subtree_stops_at_the_next_sibling() {
        let payload = tree_payload(
            &[b"User", b"name"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                [266, 0, 20, 5, 2, 0x13, 0],
                [79, 0, 10, 0, 3, string_word(1), 0],
                [266, 30, 38, 0, 2, 0x13, 0],
                [79, 30, 38, 0, 5, string_word(0), 0],
            ],
        );

        assert_eq!(
            find_declaration_name(&payload, 3, 266, "User", 40).expect("bounded"),
            None,
            "the first declaration must not borrow the later sibling's identifier"
        );
    }

    #[test]
    fn last_child_is_bounded_by_its_parent_next_link() {
        let payload = tree_payload(
            &[b"User", b"other"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                // Last child of the inner list.
                [300, 0, 20, 0, 6, 0, 0],
                [79, 0, 10, 0, 3, string_word(1), 0],
                [302, 12, 20, 0, 3, 0, 0],
                // Inner list ends at its next sibling, node 8.
                [0xFFFF_FFFF, 0, 20, 8, 2, 1, 0],
                [303, 20, 28, 0, 6, 0, 0],
                // Later sibling declaration with the same name.
                [300, 30, 40, 0, 2, 0, 0],
                [79, 30, 40, 0, 8, string_word(0), 0],
            ],
        );

        assert_eq!(
            find_declaration_name(&payload, 3, 300, "User", 40).expect("bounded"),
            None,
            "a last child must stop at its parent's next link, not at the file end"
        );
    }

    #[test]
    fn rejects_invalid_next_links_and_multi_hop_parent_cycles() {
        let invalid_next = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                [300, 0, 20, 99, 2, 0, 0],
                [79, 0, 10, 0, 3, string_word(0), 0],
            ],
        );
        assert_eq!(
            find_declaration_name(&invalid_next, 3, 300, "User", 40).expect_err("next"),
            AstError::NodeLinkInvalid
        );

        let cycle = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                [300, 0, 20, 0, 4, 0, 0],
                [301, 0, 20, 0, 3, 0, 0],
            ],
        );
        assert_eq!(
            find_declaration_name(&cycle, 3, 300, "User", 40).expect_err("parent cycle"),
            AstError::NodeLinkInvalid
        );
    }

    #[test]
    fn rejects_a_token_start_before_the_identifier_node() {
        let payload = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 20, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 19, 0, 1, 1, 0],
                [300, 0, 20, 0, 2, 0, 0],
                // "User" is four units; end 8 would start at 4, before pos 5.
                [79, 5, 8, 0, 3, string_word(0), 0],
            ],
        );
        assert_eq!(
            find_declaration_name(&payload, 3, 300, "User", 20).expect("start"),
            None
        );
    }

    #[test]
    fn classifies_structural_payload_errors() {
        assert!(AstError::TooShort.is_structural());
        assert!(AstError::VersionMismatch { observed: 5 }.is_structural());
        assert!(AstError::NodeTableMisaligned.is_structural());
        assert!(AstError::StringRangeInvalid.is_structural());
        assert!(AstError::NodeLinkInvalid.is_structural());
        assert!(!AstError::NodeIndexOutOfRange.is_structural());
        assert!(
            !AstError::NodeKindMismatch {
                index: 3,
                expected: 300,
                observed: 79,
            }
            .is_structural()
        );
        assert!(!AstError::NodeRangeInvalid.is_structural());
        assert!(!AstError::NodeRangeOutOfSource.is_structural());
        assert!(!AstError::NameNotUtf8.is_structural());
    }

    #[test]
    fn rejects_invalid_declaration_indices_kinds_and_links() {
        let payload = tree_payload(
            &[b"User", b"name"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                [266, 0, 20, 7, 2, 0x13, 0],
                [79, 0, 10, 0, 3, string_word(0), 0],
                [266, 30, 38, 0, 2, 0x13, 0],
                [79, 30, 38, 0, 5, string_word(0), 0],
            ],
        );

        assert_eq!(
            find_declaration_name(&payload, 0, 0, "User", 40).expect_err("nil"),
            AstError::NodeIndexOutOfRange
        );
        assert_eq!(
            find_declaration_name(&payload, 99, 266, "User", 40).expect_err("index"),
            AstError::NodeIndexOutOfRange
        );
        assert!(matches!(
            find_declaration_name(&payload, 3, 173, "User", 40),
            Err(AstError::NodeKindMismatch { .. })
        ));

        let mut cyclic = payload;
        // Make the first declaration its own parent and drop its next link so
        // the subtree walk cannot terminate; the decoder must reject it.
        let nodes_offset = 64 + 2 * 8 + 8;
        let first = nodes_offset + 3 * 28;
        cyclic[first + 12..first + 16].copy_from_slice(&0u32.to_le_bytes());
        cyclic[first + 16..first + 20].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(
            find_declaration_name(&cyclic, 3, 266, "User", 40).expect_err("cycle"),
            AstError::NodeLinkInvalid
        );
    }

    #[test]
    fn rejects_declaration_ranges_outside_the_source() {
        let payload = tree_payload(
            &[b"User"],
            &[
                [0, 0, 0, 0, 0, 0, 0],
                [308, 0, 40, 0, 0, 0x8000_0000, 0],
                [0xFFFF_FFFF, 0, 39, 0, 1, 1, 0],
                [266, 0, 20, 0, 2, 0x13, 0],
                [79, 0, 10, 0, 3, string_word(0), 0],
            ],
        );
        assert_eq!(
            find_declaration_name(&payload, 3, 266, "User", 5).expect_err("source"),
            AstError::NodeRangeOutOfSource
        );
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

    #[test]
    fn ast_tree_validates_indices_ranges_links_and_source_boundaries() {
        let payload = tree_payload(&[b"x"], &[[0; 7], [308, 0, 10, 0, 0, 0x8000_0000, 0]]);
        let tree = AstTree::parse(&payload, 10).expect("tree");
        assert_eq!(tree.header().nnodes, 2);
        assert_eq!(tree.node(1).expect("source").kind, SOURCE_FILE_KIND);
        assert_eq!(
            tree.node(0).expect_err("nil"),
            AstError::NodeIndexOutOfRange
        );
        assert_eq!(
            tree.node(99).expect_err("range"),
            AstError::NodeIndexOutOfRange
        );
        assert_eq!(
            tree.subtree_end(0).expect_err("nil end"),
            AstError::NodeIndexOutOfRange
        );

        let inverted = tree_payload(&[b"x"], &[[0; 7], [308, 5, 4, 0, 0, 0x8000_0000, 0]]);
        assert_eq!(
            AstTree::parse(&inverted, 10)
                .expect("tree")
                .node(1)
                .expect_err("inverted"),
            AstError::NodeRangeInvalid
        );

        let out = tree_payload(&[b"x"], &[[0; 7], [308, 0, 11, 0, 0, 0x8000_0000, 0]]);
        assert_eq!(
            AstTree::parse(&out, 10)
                .expect("tree")
                .node(1)
                .expect_err("out of source"),
            AstError::NodeRangeOutOfSource
        );

        let bad_parent = tree_payload(&[b"x"], &[[0; 7], [308, 0, 10, 0, 2, 0x8000_0000, 0]]);
        assert_eq!(
            AstTree::parse(&bad_parent, 10)
                .expect("tree")
                .node(1)
                .expect_err("parent"),
            AstError::NodeLinkInvalid
        );
    }

    #[test]
    fn ast_tree_expands_lists_and_validates_children_masks() {
        let payload = tree_payload(
            &[b"f"],
            &[
                [0; 7],
                [308, 0, 10, 0, 0, 0x8000_0000, 0],
                [NODE_LIST_KIND, 0, 10, 0, 1, 1, 0],
                [FUNCTION_DECLARATION_KIND, 0, 10, 0, 2, 1 << 2, 0],
                [IDENTIFIER_KIND, 0, 1, 0, 3, string_word(0), 0],
            ],
        );
        let tree = AstTree::parse(&payload, 10).expect("tree");
        let source = tree.source_file().expect("source");
        let children = tree.direct_children(source).expect("children");
        assert_eq!(children.len(), 1);
        let list = children[0];
        assert_eq!(list.kind, NODE_LIST_KIND);
        let items = tree.list_items(list).expect("items");
        assert_eq!(items.len(), 1);
        let function = items[0];
        assert_eq!(function.kind, FUNCTION_DECLARATION_KIND);
        assert_eq!(
            tree.name_slot(function, 2).expect("name"),
            AstNameSlot::Identifier(AstName {
                text: "f".to_owned(),
                range: AstRange::new(0, 1),
            })
        );
        assert_eq!(
            tree.child_property(function, 2)
                .expect("bit")
                .expect("child")
                .kind,
            IDENTIFIER_KIND
        );
        assert!(tree.child_property(function, 3).expect("absent").is_none());
        assert_eq!(
            tree.child_property(function, 8).expect_err("bit"),
            AstError::NodeLinkInvalid
        );
        assert_eq!(
            tree.child_property(source, 0).expect_err("extended data"),
            AstError::InvalidNodeData
        );
        assert!(matches!(
            tree.list_items(function),
            Err(AstError::NodeKindMismatch { .. })
        ));
    }

    #[test]
    fn ast_tree_name_slots_do_not_fall_back_to_initializer_identifiers() {
        // Real masks from the pinned encoder:
        // VariableDeclaration data 0x09 = name(bit0) + initializer(bit3);
        // PropertyDeclaration data 0x12 = name(bit1) + initializer(bit4);
        // ClassDeclaration data 0x12 = name(bit1) + members(bit4).
        let destructuring = tree_payload(
            &[b"config"],
            &[
                [0; 7],
                [262, 0, 20, 0, 0, 0x09, 0],
                [208, 5, 10, 3, 1, 1, 0],
                [79, 13, 20, 0, 1, string_word(0), 0],
            ],
        );
        let tree = AstTree::parse(&destructuring, 20).expect("tree");
        assert_eq!(
            tree.name_slot(tree.node(1).expect("declaration"), 0)
                .expect("slot"),
            AstNameSlot::NotIdentifier { kind: 208 }
        );

        let string_property = tree_payload(
            &[b"fallback"],
            &[
                [0; 7],
                [174, 0, 20, 0, 0, 0x12, 0],
                [10, 1, 4, 3, 1, 0x8000_0000, 0],
                [79, 12, 20, 0, 1, string_word(0), 0],
            ],
        );
        let tree = AstTree::parse(&string_property, 20).expect("tree");
        let property = tree.node(1).expect("declaration");
        assert_eq!(
            tree.name_slot(property, 1).expect("name slot"),
            AstNameSlot::NotIdentifier { kind: 10 }
        );
        // The initializer bit still resolves to its Identifier; this proves a
        // first-Identifier fallback would have published `fallback`.
        assert!(matches!(
            tree.name_slot(property, 4).expect("initializer"),
            AstNameSlot::Identifier(AstName { text, .. }) if text == "fallback"
        ));

        let class = tree_payload(
            &[b"C"],
            &[
                [0; 7],
                [265, 0, 10, 0, 0, 0x12, 0],
                [79, 6, 7, 3, 1, string_word(0), 0],
                [NODE_LIST_KIND, 8, 10, 0, 1, 0, 0],
            ],
        );
        let tree = AstTree::parse(&class, 10).expect("tree");
        let class = tree.node(1).expect("class");
        assert!(matches!(
            tree.name_slot(class, 1).expect("name"),
            AstNameSlot::Identifier(AstName { text, .. }) if text == "C"
        ));
        assert_eq!(
            tree.name_slot(class, 4).expect("members"),
            AstNameSlot::NotIdentifier {
                kind: NODE_LIST_KIND
            }
        );
        assert_eq!(
            tree.name_slot(class, 2).expect("absent"),
            AstNameSlot::Missing
        );
    }

    #[test]
    fn ast_tree_name_reports_undecodable_tokens_and_missing_source_file() {
        let payload = tree_payload(
            &[b"x"],
            &[
                [0; 7],
                [308, 0, 1, 0, 0, 0x8000_0000, 0],
                [IDENTIFIER_KIND, 0, 1, 0, 1, 0, 0],
            ],
        );
        let tree = AstTree::parse(&payload, 1).expect("tree");
        assert_eq!(
            tree.identifier_name(tree.node(2).expect("identifier"))
                .expect_err("data"),
            AstError::InvalidNodeData
        );

        let invalid = [0xFFu8];
        let payload = tree_payload(
            &[&invalid],
            &[
                [0; 7],
                [308, 0, 1, 0, 0, 0x8000_0000, 0],
                [IDENTIFIER_KIND, 0, 1, 0, 1, 0x4000_0000, 0],
            ],
        );
        let tree = AstTree::parse(&payload, 1).expect("tree");
        assert_eq!(
            tree.identifier_name(tree.node(2).expect("identifier"))
                .expect_err("utf8"),
            AstError::NameNotUtf8
        );

        let payload = tree_payload(
            &[b"name"],
            &[
                [0; 7],
                [308, 0, 4, 0, 0, 0x8000_0000, 0],
                [IDENTIFIER_KIND, 0, 2, 0, 1, string_word(0), 0],
            ],
        );
        let tree = AstTree::parse(&payload, 4).expect("tree");
        assert_eq!(
            tree.identifier_name(tree.node(2).expect("identifier"))
                .expect_err("range"),
            AstError::NodeRangeInvalid
        );

        let payload = tree_payload(
            &[b"x"],
            &[[0; 7], [IDENTIFIER_KIND, 0, 1, 0, 0, string_word(0), 0]],
        );
        let tree = AstTree::parse(&payload, 1).expect("tree");
        assert!(matches!(
            tree.source_file(),
            Err(AstError::NodeKindMismatch { .. })
        ));
    }
}
