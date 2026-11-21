//! Conservative native document-symbol extraction from the frozen v9 AST.
//!
//! The pinned native API has no `getNavigationBarItems`, `getOutliningSpans`
//! or document-symbol method. This module therefore walks the same-snapshot
//! `getSourceFile` AST directly and emits only names that are proven by a
//! direct Identifier child plus an exact source-token match.
//!
//! Covered declarations:
//!
//! - `FunctionDeclaration`, `ClassDeclaration`, `InterfaceDeclaration`,
//!   `TypeAliasDeclaration`, `EnumDeclaration`, `VariableDeclaration` and
//!   `ModuleDeclaration` at source-file and namespace level;
//! - named class/interface properties, methods and accessors;
//! - enum members.
//!
//! `Constructor`, call/construct/index signatures, class static blocks,
//! computed names and string/numeric property names have no Identifier name
//! slot in this schema. They are never guessed: each one becomes an explicit
//! [`DocumentSymbolGap`], which makes the query `Partial` in the presentation
//! layer. Import/export declarations and export assignments are also explicit
//! gaps; every unresolved top-level/namespace statement kind is an explicit
//! gap as well.
//!
//! The walk is bounded by [`wxml_types::MAX_DOCUMENT_SYMBOLS`],
//! [`wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH`] and
//! [`wxml_types::MAX_DOCUMENT_SYMBOL_GAPS`]. Truncation and every skipped
//! candidate are visible in [`DocumentSymbolsEvidence`]; the presentation layer
//! must not report `Supported` for this evidence when either is non-empty.

use crate::native::ast::{
    AstError, AstName, AstNameSlot, AstNode, AstRange, AstTree, CALL_SIGNATURE_KIND,
    CLASS_DECLARATION_KIND, CLASS_STATIC_BLOCK_KIND, CONSTRUCT_SIGNATURE_KIND, CONSTRUCTOR_KIND,
    ENUM_DECLARATION_KIND, ENUM_MEMBER_KIND, EXPORT_ASSIGNMENT_KIND, EXPORT_DECLARATION_KIND,
    FUNCTION_DECLARATION_KIND, GET_ACCESSOR_KIND, IMPORT_DECLARATION_KIND,
    IMPORT_EQUALS_DECLARATION_KIND, INDEX_SIGNATURE_KIND, INTERFACE_DECLARATION_KIND,
    METHOD_DECLARATION_KIND, METHOD_SIGNATURE_KIND, MODULE_BLOCK_KIND, MODULE_DECLARATION_KIND,
    NAMESPACE_EXPORT_DECLARATION_KIND, NODE_LIST_KIND, PROPERTY_DECLARATION_KIND,
    PROPERTY_SIGNATURE_KIND, SET_ACCESSOR_KIND, TYPE_ALIAS_DECLARATION_KIND,
    VARIABLE_DECLARATION_KIND, VARIABLE_DECLARATION_LIST_KIND, VARIABLE_STATEMENT_KIND,
};
use crate::native::strings::{Utf8Offset, Utf16Offset, utf16_slice, utf16_to_utf8};
use wxml_types::{MAX_DOCUMENT_SYMBOL_DEPTH, MAX_DOCUMENT_SYMBOL_GAPS, MAX_DOCUMENT_SYMBOLS};

/// Returns the pinned v9 children-mask bit for one declaration's name slot.
///
/// Provenance: these bits come from the pinned 7.1.0-beta generated encoder
/// (`getChildrenPropertyMask`), not from TypeScript's public `SyntaxKind`
/// numbering. Each value was also checked against decoded real payloads:
///
/// - `FunctionDeclaration` / `MethodDeclaration`: bit 2;
/// - `ClassDeclaration`, `InterfaceDeclaration`, `TypeAliasDeclaration`,
///   `EnumDeclaration`, `ModuleDeclaration`, `PropertyDeclaration`,
///   `PropertySignature`, `GetAccessor`, `SetAccessor`, `MethodSignature`:
///   bit 1;
/// - `VariableDeclaration` / `EnumMember`: bit 0;
/// - class/interface member list: bit 4;
/// - enum member list: bit 2;
/// - `ModuleDeclaration` body: bit 3;
/// - `ModuleBlock` statements: bit 0.
const fn name_slot_bit(kind: u32) -> Option<u8> {
    match kind {
        FUNCTION_DECLARATION_KIND | METHOD_DECLARATION_KIND => Some(2),
        CLASS_DECLARATION_KIND
        | INTERFACE_DECLARATION_KIND
        | TYPE_ALIAS_DECLARATION_KIND
        | ENUM_DECLARATION_KIND
        | MODULE_DECLARATION_KIND
        | PROPERTY_DECLARATION_KIND
        | PROPERTY_SIGNATURE_KIND
        | GET_ACCESSOR_KIND
        | SET_ACCESSOR_KIND
        | METHOD_SIGNATURE_KIND => Some(1),
        VARIABLE_DECLARATION_KIND | ENUM_MEMBER_KIND => Some(0),
        _ => None,
    }
}

/// One decoded document symbol in native coordinates.
///
/// Both ranges are exact: `range` is the AST declaration range (including its
/// leading trivia), and `selection` is the verified name token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbolEvidence {
    /// Symbol name decoded from the Identifier string pool and verified
    /// against the source text.
    pub name: String,
    /// Frozen v9 node kind.
    pub node_kind: u32,
    /// Declaration range start in UTF-16 code units.
    pub start_utf16: Utf16Offset,
    /// Declaration range end in UTF-16 code units.
    pub end_utf16: Utf16Offset,
    /// Declaration range start in UTF-8 bytes.
    pub start_utf8: Utf8Offset,
    /// Declaration range end in UTF-8 bytes.
    pub end_utf8: Utf8Offset,
    /// Name-token start in UTF-16 code units.
    pub selection_start_utf16: Utf16Offset,
    /// Name-token end in UTF-16 code units.
    pub selection_end_utf16: Utf16Offset,
    /// Name-token start in UTF-8 bytes.
    pub selection_start_utf8: Utf8Offset,
    /// Name-token end in UTF-8 bytes.
    pub selection_end_utf8: Utf8Offset,
    /// Nested members or namespace declarations.
    pub children: Vec<Self>,
}

/// One declaration candidate that could not be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbolGap {
    /// Frozen v9 node kind of the skipped candidate.
    pub node_kind: u32,
    /// Observed name-slot node kind when the gap is a non-Identifier name.
    pub name_kind: Option<u32>,
    /// Full-start UTF-16 offset of the skipped candidate.
    pub start_utf16: u32,
    /// Stable reason marker.
    pub reason: &'static str,
}

/// Raw document-symbol evidence for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbolsEvidence {
    /// Decoded symbols in deterministic source order.
    pub symbols: Vec<DocumentSymbolEvidence>,
    /// First [`wxml_types::MAX_DOCUMENT_SYMBOL_GAPS`] skipped candidates.
    pub gaps: Vec<DocumentSymbolGap>,
    /// Total skipped candidates, including gaps beyond the stored bound.
    pub skipped: usize,
    /// Whether [`wxml_types::MAX_DOCUMENT_SYMBOLS`] cut the tree short.
    pub truncated: bool,
}

/// Walks one decoded AST and returns conservative document symbols.
///
/// # Errors
///
/// Returns [`AstError`] only for structural payload/link failures. A
/// declaration-local malformed name, a non-Identifier name slot, an unsupported
/// member kind or the depth/size cap is recorded in the returned evidence.
pub fn document_symbols(
    ast: &AstTree<'_>,
    source: &str,
) -> Result<DocumentSymbolsEvidence, AstError> {
    let source_file = ast.source_file()?;
    let mut walker = Walker {
        ast,
        source,
        emitted: 0,
        skipped: 0,
        gaps: Vec::new(),
        truncated: false,
    };
    let root = walker.find_node_list(source_file)?;
    let symbols = match root {
        Some(list) => walker.walk_statements(list, 0)?,
        None => Vec::new(),
    };
    Ok(walker.finish(symbols))
}

struct Walker<'a, 'tree> {
    ast: &'a AstTree<'tree>,
    source: &'a str,
    emitted: usize,
    skipped: usize,
    gaps: Vec<DocumentSymbolGap>,
    truncated: bool,
}

impl Walker<'_, '_> {
    fn finish(self, mut symbols: Vec<DocumentSymbolEvidence>) -> DocumentSymbolsEvidence {
        normalize(&mut symbols);
        DocumentSymbolsEvidence {
            symbols,
            gaps: self.gaps,
            skipped: self.skipped,
            truncated: self.truncated,
        }
    }

    fn find_node_list(&self, container: AstNode) -> Result<Option<AstNode>, AstError> {
        for child in self.ast.direct_children(container)? {
            if child.kind == NODE_LIST_KIND {
                return Ok(Some(child));
            }
        }
        Ok(None)
    }

    fn walk_statements(
        &mut self,
        list: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        let mut symbols = Vec::new();
        for statement in self.ast.list_items(list)? {
            if self.truncated {
                break;
            }
            match statement.kind {
                FUNCTION_DECLARATION_KIND
                | CLASS_DECLARATION_KIND
                | INTERFACE_DECLARATION_KIND
                | TYPE_ALIAS_DECLARATION_KIND
                | ENUM_DECLARATION_KIND
                | MODULE_DECLARATION_KIND => {
                    if let Some(symbol) = self.decode_named(statement, depth)? {
                        symbols.push(symbol);
                    }
                }
                VARIABLE_STATEMENT_KIND => {
                    symbols.extend(self.decode_variable_statement(statement, depth)?);
                }
                IMPORT_DECLARATION_KIND
                | IMPORT_EQUALS_DECLARATION_KIND
                | EXPORT_DECLARATION_KIND
                | EXPORT_ASSIGNMENT_KIND
                | NAMESPACE_EXPORT_DECLARATION_KIND => {
                    self.record_gap(statement, "import/export declaration kind is not supported");
                }
                // A statement kind this walker does not resolve is an
                // explicit gap, not a silent empty result.
                _ => self.record_gap(statement, "unsupported statement kind"),
            }
        }
        Ok(symbols)
    }

    fn decode_variable_statement(
        &mut self,
        statement: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        let Some(list) = self.ast.child_property(statement, 1)? else {
            self.record_gap(statement, "variable statement has no declaration list");
            return Ok(Vec::new());
        };
        if list.kind != VARIABLE_DECLARATION_LIST_KIND {
            self.record_gap(
                list,
                "variable declaration list has an unexpected node kind",
            );
            return Ok(Vec::new());
        }
        let Some(declarations) = self.ast.child_property(list, 0)? else {
            self.record_gap(list, "variable declaration list has no declarations");
            return Ok(Vec::new());
        };
        let mut symbols = Vec::new();
        for declaration in self.ast.list_items(declarations)? {
            if self.truncated {
                break;
            }
            if declaration.kind != VARIABLE_DECLARATION_KIND {
                self.record_gap(declaration, "unsupported variable declaration kind");
                continue;
            }
            if let Some(symbol) = self.decode_named(declaration, depth)? {
                symbols.push(symbol);
            }
        }
        Ok(symbols)
    }

    fn decode_named(
        &mut self,
        node: AstNode,
        depth: usize,
    ) -> Result<Option<DocumentSymbolEvidence>, AstError> {
        if depth >= MAX_DOCUMENT_SYMBOL_DEPTH {
            self.record_gap(node, "maximum document symbol nesting depth exceeded");
            return Ok(None);
        }
        let Some(bit) = name_slot_bit(node.kind) else {
            self.record_gap(node, "unsupported declaration kind");
            return Ok(None);
        };
        let name = match self.ast.name_slot(node, bit) {
            Ok(AstNameSlot::Identifier(name)) => name,
            Ok(AstNameSlot::Missing) => {
                self.record_gap(node, "declaration has no name slot");
                return Ok(None);
            }
            Ok(AstNameSlot::NotIdentifier { kind }) => {
                self.record_name_gap(node, kind);
                return Ok(None);
            }
            Err(error) if error.is_structural() => return Err(error),
            Err(_) => {
                self.record_gap(node, "identifier name is not decodable UTF-8");
                return Ok(None);
            }
        };
        let Some(selection) = self.verified_name_range(&name) else {
            self.record_gap(node, "name token does not match the source text");
            return Ok(None);
        };
        let Some(ranges) = self.ranges(node, selection) else {
            self.record_gap(node, "declaration range is not valid UTF-8 text");
            return Ok(None);
        };
        if self.emitted >= MAX_DOCUMENT_SYMBOLS {
            self.truncated = true;
            return Ok(None);
        }
        self.emitted = self.emitted.saturating_add(1);
        let children = self.children(node, depth.saturating_add(1))?;
        Ok(Some(DocumentSymbolEvidence {
            name: name.text,
            node_kind: node.kind,
            start_utf16: ranges.start_utf16,
            end_utf16: ranges.end_utf16,
            start_utf8: ranges.start_utf8,
            end_utf8: ranges.end_utf8,
            selection_start_utf16: ranges.selection_start_utf16,
            selection_end_utf16: ranges.selection_end_utf16,
            selection_start_utf8: ranges.selection_start_utf8,
            selection_end_utf8: ranges.selection_end_utf8,
            children,
        }))
    }

    fn children(
        &mut self,
        node: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        match node.kind {
            CLASS_DECLARATION_KIND | INTERFACE_DECLARATION_KIND => self.walk_members(node, depth),
            ENUM_DECLARATION_KIND => self.walk_enum_members(node, depth),
            MODULE_DECLARATION_KIND => self.walk_module(node, depth),
            _ => Ok(Vec::new()),
        }
    }

    fn walk_members(
        &mut self,
        owner: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        // Real class/interface payloads use children bit 4 for `Members`
        // (data masks 0x12/0x13), including classes nested in modules.
        let Some(list) = self.ast.child_property(owner, 4)? else {
            self.record_gap(owner, "class/interface has no member list");
            return Ok(Vec::new());
        };
        let mut symbols = Vec::new();
        for member in self.ast.list_items(list)? {
            if self.truncated {
                break;
            }
            match member.kind {
                PROPERTY_DECLARATION_KIND
                | PROPERTY_SIGNATURE_KIND
                | GET_ACCESSOR_KIND
                | SET_ACCESSOR_KIND
                | METHOD_DECLARATION_KIND
                | METHOD_SIGNATURE_KIND => {
                    if let Some(symbol) = self.decode_named(member, depth)? {
                        symbols.push(symbol);
                    }
                }
                CONSTRUCTOR_KIND
                | CLASS_STATIC_BLOCK_KIND
                | CALL_SIGNATURE_KIND
                | CONSTRUCT_SIGNATURE_KIND
                | INDEX_SIGNATURE_KIND => {
                    self.record_gap(member, "member kind has no Identifier name slot");
                }
                _ => self.record_gap(member, "unsupported class/interface member kind"),
            }
        }
        Ok(symbols)
    }

    fn walk_enum_members(
        &mut self,
        owner: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        // Real enum payloads use children bit 2 for `Members` (mask 0x06/0x07).
        let Some(list) = self.ast.child_property(owner, 2)? else {
            self.record_gap(owner, "enum has no member list");
            return Ok(Vec::new());
        };
        let mut symbols = Vec::new();
        for member in self.ast.list_items(list)? {
            if self.truncated {
                break;
            }
            if member.kind != ENUM_MEMBER_KIND {
                self.record_gap(member, "unsupported enum member kind");
                continue;
            }
            if let Some(symbol) = self.decode_named(member, depth)? {
                symbols.push(symbol);
            }
        }
        Ok(symbols)
    }

    fn walk_module(
        &mut self,
        module: AstNode,
        depth: usize,
    ) -> Result<Vec<DocumentSymbolEvidence>, AstError> {
        // Real ModuleDeclaration payloads use children bit 3 for `Body`
        // (mask 0x0a/0x0b; the high byte carries the namespace/module keyword).
        let Some(body) = self.ast.child_property(module, 3)? else {
            self.record_gap(module, "module declaration has no body");
            return Ok(Vec::new());
        };
        match body.kind {
            MODULE_BLOCK_KIND => {
                // Real ModuleBlock payloads use children bit 0 for `Statements`.
                let Some(list) = self.ast.child_property(body, 0)? else {
                    self.record_gap(body, "module block has no statement list");
                    return Ok(Vec::new());
                };
                self.walk_statements(list, depth)
            }
            // `namespace A.B {}` nests the second module as the body.
            MODULE_DECLARATION_KIND => {
                let symbol = self.decode_named(body, depth)?;
                Ok(symbol.into_iter().collect())
            }
            _ => {
                self.record_gap(body, "unsupported namespace body kind");
                Ok(Vec::new())
            }
        }
    }

    fn verified_name_range(&self, name: &AstName) -> Option<AstRange> {
        if name.range.start >= name.range.end {
            return None;
        }
        let text = utf16_slice(self.source, name.range)?;
        (text == name.text).then_some(name.range)
    }

    fn ranges(&self, node: AstNode, selection: AstRange) -> Option<EvidenceRanges> {
        let declaration = node.pos..node.end;
        let selection_is_valid = selection.start < selection.end
            && selection.end <= node.end
            && declaration.contains(&selection.start);
        if declaration.is_empty() || !selection_is_valid {
            return None;
        }
        let start_utf8 = utf16_to_utf8(self.source, Utf16Offset::new(node.pos)).ok()?;
        let end_utf8 = utf16_to_utf8(self.source, Utf16Offset::new(node.end)).ok()?;
        let selection_start_utf8 =
            utf16_to_utf8(self.source, Utf16Offset::new(selection.start)).ok()?;
        let selection_end_utf8 =
            utf16_to_utf8(self.source, Utf16Offset::new(selection.end)).ok()?;
        Some(EvidenceRanges {
            start_utf16: Utf16Offset::new(node.pos),
            end_utf16: Utf16Offset::new(node.end),
            start_utf8,
            end_utf8,
            selection_start_utf16: Utf16Offset::new(selection.start),
            selection_end_utf16: Utf16Offset::new(selection.end),
            selection_start_utf8,
            selection_end_utf8,
        })
    }

    fn record_gap(&mut self, node: AstNode, reason: &'static str) {
        self.push_gap(node, None, reason);
    }

    fn record_name_gap(&mut self, node: AstNode, name_kind: u32) {
        self.push_gap(node, Some(name_kind), "name slot is not an Identifier");
    }

    fn push_gap(&mut self, node: AstNode, name_kind: Option<u32>, reason: &'static str) {
        self.skipped = self.skipped.saturating_add(1);
        if self.gaps.len() < MAX_DOCUMENT_SYMBOL_GAPS {
            self.gaps.push(DocumentSymbolGap {
                node_kind: node.kind,
                name_kind,
                start_utf16: node.pos,
                reason,
            });
        }
    }
}

struct EvidenceRanges {
    start_utf16: Utf16Offset,
    end_utf16: Utf16Offset,
    start_utf8: Utf8Offset,
    end_utf8: Utf8Offset,
    selection_start_utf16: Utf16Offset,
    selection_end_utf16: Utf16Offset,
    selection_start_utf8: Utf8Offset,
    selection_end_utf8: Utf8Offset,
}

fn normalize(symbols: &mut Vec<DocumentSymbolEvidence>) {
    symbols.sort_by(compare_symbols);
    symbols.dedup_by(|left, right| compare_symbols(left, right).is_eq());
    for symbol in symbols {
        normalize(&mut symbol.children);
    }
}

fn compare_symbols(
    left: &DocumentSymbolEvidence,
    right: &DocumentSymbolEvidence,
) -> std::cmp::Ordering {
    (
        left.start_utf16,
        left.end_utf16,
        left.selection_start_utf16,
        left.selection_end_utf16,
        left.name.as_str(),
        left.node_kind,
    )
        .cmp(&(
            right.start_utf16,
            right.end_utf16,
            right.selection_start_utf16,
            right.selection_end_utf16,
            right.name.as_str(),
            right.node_kind,
        ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assert_is_empty, clippy::too_many_lines)]

    use super::*;
    use crate::native::ast::{IDENTIFIER_KIND, NODE_LIST_KIND, SOURCE_FILE_KIND};
    use crate::native::strings::{utf8_to_utf16, utf16_len};
    use std::fmt::Write as _;

    struct TreeBuilder {
        strings: Vec<String>,
        nodes: Vec<[u32; 7]>,
    }

    impl TreeBuilder {
        fn new() -> Self {
            Self {
                strings: Vec::new(),
                nodes: vec![[0; 7]],
            }
        }

        fn intern(&mut self, text: &str) -> u32 {
            let word = u32::try_from(self.strings.len()).expect("string index") * 2;
            self.strings.push(text.to_owned());
            word
        }

        fn node(&mut self, kind: u32, pos: u32, end: u32, parent: u32, data: u32) -> u32 {
            let index = u32::try_from(self.nodes.len()).expect("node index");
            self.nodes.push([kind, pos, end, 0, parent, data, 0]);
            index
        }

        fn ident(&mut self, pos: u32, end: u32, parent: u32, text: &str) -> u32 {
            let data = 0x4000_0000 | self.intern(text);
            self.node(IDENTIFIER_KIND, pos, end, parent, data)
        }

        fn leaf(&mut self, kind: u32, pos: u32, end: u32, parent: u32, data: u32) -> u32 {
            self.node(kind, pos, end, parent, data)
        }

        fn set_next(&mut self, index: u32, next: u32) {
            self.nodes[usize::try_from(index).expect("index")][3] = next;
        }

        fn set_data(&mut self, index: u32, data: u32) {
            self.nodes[usize::try_from(index).expect("index")][5] = data;
        }

        fn payload(&self) -> Vec<u8> {
            let mut table = Vec::new();
            let mut pool = Vec::new();
            for string in &self.strings {
                let start = u32::try_from(pool.len()).expect("pool start");
                pool.extend_from_slice(string.as_bytes());
                let end = u32::try_from(pool.len()).expect("pool end");
                table.extend_from_slice(&start.to_le_bytes());
                table.extend_from_slice(&end.to_le_bytes());
            }
            let string_data = 64 + u32::try_from(table.len()).expect("table length");
            let extended = string_data + u32::try_from(pool.len()).expect("pool length");
            let mut payload = vec![0u8; 64];
            payload[0..4].copy_from_slice(&(9u32 << 24).to_le_bytes());
            payload[24..28].copy_from_slice(&64u32.to_le_bytes());
            payload[28..32].copy_from_slice(&string_data.to_le_bytes());
            payload[32..36].copy_from_slice(&extended.to_le_bytes());
            payload[36..40].copy_from_slice(&extended.to_le_bytes());
            payload[40..44].copy_from_slice(&extended.to_le_bytes());
            payload[44..52].copy_from_slice(&7u64.to_le_bytes());
            payload.extend_from_slice(&table);
            payload.extend_from_slice(&pool);
            for node in &self.nodes {
                for word in node {
                    payload.extend_from_slice(&word.to_le_bytes());
                }
            }
            payload
        }
    }

    fn root(builder: &mut TreeBuilder, source: &str) -> (u32, u32) {
        let len = utf16_len(source);
        let source_file = builder.node(SOURCE_FILE_KIND, 0, len, 0, 0x8000_0000);
        let list = builder.node(NODE_LIST_KIND, 0, len, source_file, 0);
        (source_file, list)
    }

    fn at(source: &str, marker: &str) -> usize {
        source.find(marker).expect("marker")
    }

    fn at_from(source: &str, from: usize, marker: &str) -> usize {
        from + source[from..].find(marker).expect("marker after offset")
    }

    fn utf16(source: &str, byte: usize) -> u32 {
        utf8_to_utf16(
            source,
            Utf8Offset::new(u32::try_from(byte).expect("byte offset")),
        )
        .expect("utf16 offset")
        .get()
    }

    fn u8_offset(byte: usize) -> u32 {
        u32::try_from(byte).expect("byte offset")
    }

    /// Returns `(start_utf16, end_utf16, start_utf8, end_utf8)`.
    fn name_range(source: &str, marker: &str, name: &str) -> (u32, u32, u32, u32) {
        let byte = at(source, marker) + marker.rfind(name).expect("name in marker");
        let end = byte + name.len();
        (
            utf16(source, byte),
            utf16(source, end),
            u8_offset(byte),
            u8_offset(end),
        )
    }

    fn bind_siblings(builder: &mut TreeBuilder, list: u32, items: &[u32]) {
        for pair in items.windows(2) {
            builder.set_next(pair[0], pair[1]);
        }
        builder.set_data(list, u32::try_from(items.len()).expect("item count"));
    }

    #[test]
    fn fake_ast_yields_exact_nested_ranges_and_deterministic_order() {
        // Masks below are copied from decoded pinned payloads
        // (FunctionDeclaration 0x75, Class/Interface 0x12, Enum 0x06,
        // ModuleDeclaration 0x0a/0x0b, VariableDeclaration 0x09,
        // PropertyDeclaration 0x12/0x1a, MethodDeclaration 0xe4). They are
        // deliberately not read from `name_slot_bit`.
        let source = "// 😀 文档\nfunction alpha() {}\nclass Beta {\n  field: number;\n  method(): void {}\n}\ninterface Gamma {\n  area(): number;\n}\ntype Delta = string;\nenum Epsilon { One, Two }\nnamespace Zeta {\n  const inner = 1;\n}\n";
        let mut builder = TreeBuilder::new();
        let (_source_file, list) = root(&mut builder, source);

        let function_start = 0;
        let function_end = utf16(source, at(source, "\nclass Beta"));
        let function = builder.node(
            FUNCTION_DECLARATION_KIND,
            function_start,
            function_end,
            list,
            0x04, // real FunctionDeclaration name bit 2
        );
        let (start, end, _, _) = name_range(source, "function alpha", "alpha");
        builder.ident(start, end, function, "alpha");

        let class_start = function_end;
        let class_end = utf16(source, at(source, "\ninterface Gamma"));
        let class = builder.node(
            CLASS_DECLARATION_KIND,
            class_start,
            class_end,
            list,
            0x12, // real ClassDeclaration name bit 1 + members bit 4
        );
        let (start, end, _, _) = name_range(source, "class Beta", "Beta");
        let class_name = builder.ident(start, end, class, "Beta");
        let members = builder.node(
            NODE_LIST_KIND,
            utf16(source, at(source, "  field")),
            class_end,
            class,
            0,
        );
        builder.set_next(class_name, members);

        let field_start = utf16(source, at(source, "  field"));
        let field_end = utf16(source, at(source, "  method"));
        let field = builder.node(
            PROPERTY_DECLARATION_KIND,
            field_start,
            field_end,
            members,
            0x02, // real PropertyDeclaration name bit 1
        );
        let (start, end, _, _) = name_range(source, "  field", "field");
        builder.ident(start, end, field, "field");

        let method_start = field_end;
        let method_end = utf16(source, at_from(source, at(source, "  method"), "\n}") + 2);
        let method = builder.node(
            METHOD_DECLARATION_KIND,
            method_start,
            method_end,
            members,
            0x04, // real MethodDeclaration name bit 2
        );
        let (start, end, _, _) = name_range(source, "  method", "method");
        builder.ident(start, end, method, "method");
        bind_siblings(&mut builder, members, &[field, method]);

        let interface_start = class_end;
        let interface_end = utf16(source, at(source, "\ntype Delta"));
        let interface = builder.node(
            INTERFACE_DECLARATION_KIND,
            interface_start,
            interface_end,
            list,
            0x12, // real InterfaceDeclaration name bit 1 + members bit 4
        );
        let (start, end, _, _) = name_range(source, "interface Gamma", "Gamma");
        let interface_name = builder.ident(start, end, interface, "Gamma");
        let interface_members = builder.node(
            NODE_LIST_KIND,
            utf16(source, at(source, "  area")),
            interface_end,
            interface,
            0,
        );
        builder.set_next(interface_name, interface_members);
        let area_start = utf16(source, at(source, "  area"));
        let area_end = utf16(source, at_from(source, at(source, "  area"), "\n}") + 2);
        let area = builder.node(
            METHOD_SIGNATURE_KIND,
            area_start,
            area_end,
            interface_members,
            0x02, // real MethodSignature name bit 1
        );
        let (start, end, _, _) = name_range(source, "  area", "area");
        builder.ident(start, end, area, "area");
        bind_siblings(&mut builder, interface_members, &[area]);

        let alias_start = interface_end;
        let alias_end = utf16(source, at(source, "\nenum Epsilon"));
        let alias = builder.node(
            TYPE_ALIAS_DECLARATION_KIND,
            alias_start,
            alias_end,
            list,
            0x02, // real TypeAliasDeclaration name bit 1
        );
        let (start, end, _, _) = name_range(source, "type Delta", "Delta");
        builder.ident(start, end, alias, "Delta");

        let enum_start = alias_end;
        let enum_end = utf16(source, at(source, "\nnamespace Zeta"));
        let enum_node = builder.node(
            ENUM_DECLARATION_KIND,
            enum_start,
            enum_end,
            list,
            0x06, // real EnumDeclaration name bit 1 + members bit 2
        );
        let (start, end, _, _) = name_range(source, "enum Epsilon", "Epsilon");
        let enum_name = builder.ident(start, end, enum_node, "Epsilon");
        let enum_members = builder.node(
            NODE_LIST_KIND,
            utf16(source, at(source, "{ One")),
            enum_end,
            enum_node,
            0,
        );
        builder.set_next(enum_name, enum_members);
        let one_start = utf16(source, at(source, "One"));
        let one_end = utf16(source, at(source, ", Two"));
        let one = builder.node(ENUM_MEMBER_KIND, one_start, one_end, enum_members, 0x01);
        builder.ident(one_start, one_end, one, "One");
        let two_start = utf16(source, at(source, "Two"));
        let two_end = utf16(source, at(source, "Two") + "Two".len());
        let two = builder.node(ENUM_MEMBER_KIND, two_start, two_end, enum_members, 0x01);
        builder.ident(two_start, two_end, two, "Two");
        bind_siblings(&mut builder, enum_members, &[one, two]);

        let module_start = enum_end;
        let module_end = utf16_len(source);
        let module = builder.node(
            MODULE_DECLARATION_KIND,
            module_start,
            module_end,
            list,
            0x0a, // real ModuleDeclaration name bit 1 + body bit 3
        );
        let (start, end, _, _) = name_range(source, "namespace Zeta", "Zeta");
        let module_name = builder.ident(start, end, module, "Zeta");
        let body_start = utf16(source, at_from(source, at(source, "namespace Zeta"), " {"));
        let body = builder.node(MODULE_BLOCK_KIND, body_start, module_end, module, 0x01);
        builder.set_next(module_name, body);
        let body_list = builder.node(NODE_LIST_KIND, body_start, module_end, body, 0);
        let declarations_start = at_from(source, at(source, "namespace Zeta"), "  const");
        let declarations_end = at_from(source, declarations_start, ";\n") + 1;
        let statement = builder.node(
            VARIABLE_STATEMENT_KIND,
            utf16(source, declarations_start),
            utf16(source, declarations_end),
            body_list,
            0x02, // real VariableStatement declarationList bit 1
        );
        let declaration_list = builder.node(
            VARIABLE_DECLARATION_LIST_KIND,
            utf16(source, declarations_start),
            utf16(source, declarations_end),
            statement,
            0x01, // real VariableDeclarationList declarations bit 0
        );
        let declarations = builder.node(
            NODE_LIST_KIND,
            utf16(source, declarations_start),
            utf16(source, declarations_end),
            declaration_list,
            0,
        );
        let inner_start = at_from(source, declarations_start, "inner");
        let inner_end = at_from(source, inner_start, "= 1") + 3;
        let declaration = builder.node(
            VARIABLE_DECLARATION_KIND,
            utf16(source, at_from(source, declarations_start, " inner")),
            utf16(source, inner_end),
            declarations,
            0x01, // real VariableDeclaration name bit 0
        );
        builder.ident(
            utf16(source, inner_start),
            utf16(source, inner_start + "inner".len()),
            declaration,
            "inner",
        );
        bind_siblings(&mut builder, declarations, &[declaration]);
        bind_siblings(&mut builder, body_list, &[statement]);
        bind_siblings(
            &mut builder,
            list,
            &[function, class, interface, alias, enum_node, module],
        );

        let payload = builder.payload();
        let ast = AstTree::parse(&payload, utf16_len(source)).expect("ast");
        let evidence = document_symbols(&ast, source).expect("symbols");

        assert!(!evidence.truncated);
        assert_eq!(evidence.skipped, 0);
        assert_eq!(
            evidence
                .symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta"]
        );
        assert_eq!(evidence.symbols[0].node_kind, FUNCTION_DECLARATION_KIND);
        assert_eq!(evidence.symbols[1].node_kind, CLASS_DECLARATION_KIND);
        assert_eq!(evidence.symbols[2].node_kind, INTERFACE_DECLARATION_KIND);
        assert_eq!(evidence.symbols[3].node_kind, TYPE_ALIAS_DECLARATION_KIND);
        assert_eq!(evidence.symbols[4].node_kind, ENUM_DECLARATION_KIND);
        assert_eq!(evidence.symbols[5].node_kind, MODULE_DECLARATION_KIND);

        let beta = &evidence.symbols[1];
        assert_eq!(
            beta.children
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["field", "method"]
        );
        assert_eq!(evidence.symbols[2].children[0].name, "area");
        assert_eq!(
            evidence.symbols[4]
                .children
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["One", "Two"]
        );
        assert_eq!(evidence.symbols[5].children[0].name, "inner");

        let alpha = &evidence.symbols[0];
        let (start, end, start_utf8, end_utf8) = name_range(source, "function alpha", "alpha");
        assert_eq!(alpha.start_utf16.get(), 0);
        assert_eq!(alpha.start_utf8.get(), 0);
        assert_eq!(alpha.end_utf16.get(), function_end);
        assert_eq!(alpha.end_utf8.get(), u8_offset(at(source, "\nclass Beta")));
        assert_eq!(alpha.selection_start_utf16.get(), start);
        assert_eq!(alpha.selection_end_utf16.get(), end);
        assert_eq!(alpha.selection_start_utf8.get(), start_utf8);
        assert_eq!(alpha.selection_end_utf8.get(), end_utf8);
        assert_ne!(
            alpha.selection_start_utf16.get(),
            alpha.selection_start_utf8.get()
        );

        let field = &beta.children[0];
        let (start, end, start_utf8, end_utf8) = name_range(source, "  field", "field");
        assert_eq!(field.selection_start_utf16.get(), start);
        assert_eq!(field.selection_end_utf16.get(), end);
        assert_eq!(field.selection_start_utf8.get(), start_utf8);
        assert_eq!(field.selection_end_utf8.get(), end_utf8);

        let inner = &evidence.symbols[5].children[0];
        assert_eq!(inner.selection_start_utf8.get(), u8_offset(inner_start));
        assert_eq!(inner.selection_end_utf8.get(), u8_offset(inner_start + 5));
    }

    #[test]
    fn empty_source_is_a_supported_empty_symbol_list() {
        let source = "";
        let mut builder = TreeBuilder::new();
        let (_source_file, _list) = root(&mut builder, source);
        let payload = builder.payload();
        let ast = AstTree::parse(&payload, 0).expect("ast");
        let evidence = document_symbols(&ast, source).expect("symbols");
        assert!(evidence.symbols.is_empty());
        assert!(evidence.gaps.is_empty());
        assert_eq!(evidence.skipped, 0);
        assert!(!evidence.truncated);
    }

    #[test]
    fn import_and_export_declarations_are_explicit_gaps() {
        let source = "import x from \"./m\";\nexport { x };\n";
        let mut builder = TreeBuilder::new();
        let (_source_file, list) = root(&mut builder, source);
        let import_start = at(source, "import");
        let import_end = at(source, ";\n") + 1;
        let import = builder.node(
            IMPORT_DECLARATION_KIND,
            utf16(source, import_start),
            utf16(source, import_end),
            list,
            0,
        );
        let export_start = at(source, "export");
        let export_end = at_from(source, export_start, ";\n") + 1;
        let export = builder.node(
            EXPORT_DECLARATION_KIND,
            utf16(source, export_start),
            utf16(source, export_end),
            list,
            0,
        );
        let unresolved = builder.node(999, 0, 0, list, 0);
        bind_siblings(&mut builder, list, &[import, export, unresolved]);
        let payload = builder.payload();
        let ast = AstTree::parse(&payload, utf16_len(source)).expect("ast");
        let evidence = document_symbols(&ast, source).expect("symbols");
        assert!(evidence.symbols.is_empty());
        assert_eq!(evidence.skipped, 3);
        assert_eq!(evidence.gaps.len(), 3);
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.reason.contains("not supported"))
        );
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.reason == "unsupported statement kind")
        );
    }

    #[test]
    fn unsupported_members_and_malformed_names_become_explicit_gaps() {
        let source =
            "class Bad {\n  constructor() {}\n  \"quoted\": number;\n}\nfunction other() {}\n";
        let mut builder = TreeBuilder::new();
        let (_source_file, list) = root(&mut builder, source);

        let class_start = 0;
        let class_end = utf16(source, at(source, "\nfunction other"));
        let class = builder.node(
            CLASS_DECLARATION_KIND,
            class_start,
            class_end,
            list,
            0x12, // real ClassDeclaration name bit 1 + members bit 4
        );
        let (start, end, _, _) = name_range(source, "class Bad", "Bad");
        let class_name = builder.ident(start, end, class, "Bad");
        let members = builder.node(NODE_LIST_KIND, class_start, class_end, class, 0);
        builder.set_next(class_name, members);

        let constructor_start = at(source, "  constructor");
        let constructor_end = at_from(source, constructor_start, " {}\n") + 3;
        let constructor = builder.node(
            CONSTRUCTOR_KIND,
            utf16(source, constructor_start),
            utf16(source, constructor_end),
            members,
            0,
        );
        let property_start = at(source, "  \"quoted");
        let property_end = at_from(source, property_start, ";\n") + 1;
        let property = builder.node(
            PROPERTY_DECLARATION_KIND,
            utf16(source, property_start),
            utf16(source, property_end),
            members,
            0x02, // real PropertyDeclaration name bit 1
        );
        // A string-literal name slot is not an Identifier; the string data type
        // is deliberately not decoded here.
        let quoted_name = builder.leaf(
            10,
            utf16(source, property_start + 2),
            utf16(source, property_start + 10),
            property,
            0x8000_0000,
        );
        builder.set_next(quoted_name, 0);
        bind_siblings(&mut builder, members, &[constructor, property]);

        let function_start = class_end;
        let function_end = utf16_len(source);
        let function = builder.node(
            FUNCTION_DECLARATION_KIND,
            function_start,
            function_end,
            list,
            0x04, // real FunctionDeclaration name bit 2
        );
        // The pool name is "ghost" while the source token is "other"; the
        // walker must not publish the shifted range.
        let (start, end, _, _) = name_range(source, "function other", "other");
        builder.ident(start, end, function, "ghost");
        bind_siblings(&mut builder, list, &[class, function]);

        let payload = builder.payload();
        let ast = AstTree::parse(&payload, utf16_len(source)).expect("ast");
        let evidence = document_symbols(&ast, source).expect("symbols");

        assert_eq!(evidence.symbols.len(), 1);
        assert_eq!(evidence.symbols[0].name, "Bad");
        assert!(evidence.symbols[0].children.is_empty());
        assert_eq!(evidence.skipped, 3);
        assert_eq!(evidence.gaps.len(), 3);
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.node_kind == CONSTRUCTOR_KIND)
        );
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.reason.contains("no Identifier name slot"))
        );
        assert!(evidence.gaps.iter().any(|gap| gap.name_kind == Some(10)));
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.reason == "name slot is not an Identifier")
        );
        assert!(
            evidence
                .gaps
                .iter()
                .any(|gap| gap.reason.contains("does not match the source"))
        );
        assert!(!evidence.symbols.iter().any(|symbol| symbol.name == "ghost"));
    }

    #[test]
    fn size_cap_truncates_at_256_without_dropping_an_error() {
        let mut source = String::new();
        for index in 0..=MAX_DOCUMENT_SYMBOLS {
            writeln!(source, "function f{index}() {{}}").expect("write source");
        }
        let mut builder = TreeBuilder::new();
        let (_source_file, list) = root(&mut builder, &source);
        let mut statements = Vec::new();
        let mut offset = 0;
        for index in 0..=MAX_DOCUMENT_SYMBOLS {
            let name = format!("f{index}");
            let line = format!("function {name}() {{}}\n");
            let start = offset;
            let end = start + line.len();
            let node = builder.node(
                FUNCTION_DECLARATION_KIND,
                utf16(&source, start),
                utf16(&source, end),
                list,
                0x04, // real FunctionDeclaration name bit 2
            );
            let name_start = start + "function ".len();
            let name_end = name_start + name.len();
            builder.ident(
                utf16(&source, name_start),
                utf16(&source, name_end),
                node,
                &name,
            );
            statements.push(node);
            offset = end;
        }
        bind_siblings(&mut builder, list, &statements);
        let payload = builder.payload();
        let ast = AstTree::parse(&payload, utf16_len(&source)).expect("ast");
        let evidence = document_symbols(&ast, &source).expect("symbols");
        assert_eq!(evidence.symbols.len(), MAX_DOCUMENT_SYMBOLS);
        assert!(evidence.truncated);
        assert_eq!(evidence.symbols.last().expect("last").name, "f255");
        assert!(!evidence.symbols.iter().any(|symbol| symbol.name == "f256"));
    }

    #[test]
    fn a_class_static_block_is_a_gap_not_a_fake_name() {
        let source = "class Bad {\n  static {}\n}\n";
        let mut builder = TreeBuilder::new();
        let (_source_file, list) = root(&mut builder, source);
        let class = builder.node(
            CLASS_DECLARATION_KIND,
            0,
            utf16_len(source),
            list,
            0x12, // real ClassDeclaration name bit 1 + members bit 4
        );
        let (start, end, _, _) = name_range(source, "class Bad", "Bad");
        let class_name = builder.ident(start, end, class, "Bad");
        let members = builder.node(NODE_LIST_KIND, 0, utf16_len(source), class, 0);
        builder.set_next(class_name, members);
        let block = builder.node(
            CLASS_STATIC_BLOCK_KIND,
            utf16(source, at(source, "  static")),
            utf16_len(source),
            members,
            0,
        );
        bind_siblings(&mut builder, members, &[block]);
        bind_siblings(&mut builder, list, &[class]);
        let payload = builder.payload();
        let ast = AstTree::parse(&payload, utf16_len(source)).expect("ast");
        let evidence = document_symbols(&ast, source).expect("symbols");
        assert_eq!(evidence.symbols.len(), 1);
        assert!(evidence.symbols[0].children.is_empty());
        assert_eq!(evidence.skipped, 1);
        assert_eq!(evidence.gaps[0].node_kind, CLASS_STATIC_BLOCK_KIND);
    }
}
