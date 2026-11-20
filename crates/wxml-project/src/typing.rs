//! Static parsing of component type declarations.
//!
//! A declaration file maps WXML tags to interfaces through `WxmlComponentMap`.
//! Each interface may declare `props` and `events`; the recognised type subset is
//! self-contained, and anything outside it is reported as unproven instead of
//! being guessed.
//!
//! Only declarations referenced by the map produce component facts. Everything
//! else in the file (global types, utilities, namespaces) is ignored without an
//! issue, so a project can keep unrelated `.d.ts` content next to component
//! declarations.
//!
//! The parser is deliberately smaller than the TypeScript checker. It
//! understands primitive types, string/number/boolean literals, unions, `T[]`,
//! `Record<string, T>` and nested object literal types. External identifiers,
//! qualified names, generics, conditional types, intersections, template
//! literal types and `import(...)` types stay unproven: they keep their source
//! text for display, but consumers must not derive diagnostics from them.
//!
//! Discovery is not part of this module. [`parse_component_typing_facts`] takes
//! the already-collected [`SourceSet`] and keeps only `.d.ts` entries, so the
//! workspace scanning budget, overlay handling and directory exclusions stay in
//! one place.

use std::collections::BTreeMap;

use crate::files::ProjectPath;
use crate::sources::{SourceFile, SourceKind, SourceSet};

/// Maximum number of WXML tags kept from one declaration set.
///
/// When more tags are declared, the first [`MAX_TYPING_COMPONENTS`] names in
/// tag order survive and one issue records the truncation.
pub const MAX_TYPING_COMPONENTS: usize = 512;

/// Maximum number of members kept for one component segment.
///
/// Applies separately to `props` and `events` of each component, after the
/// members are ordered by name.
pub const MAX_TYPING_MEMBERS: usize = 1024;

/// Maximum issues recorded for one declaration set.
///
/// The last slot becomes an issue-limit marker, so a pathological declaration
/// file cannot grow the fact object without bound.
const MAX_TYPING_ISSUES: usize = 64;

/// Maximum characters kept from one type expression.
///
/// Longer expressions stay usable for display in truncated form, but the
/// member is marked unproven instead of being silently cut.
const MAX_TYPE_TEXT_CHARS: usize = 512;

/// Maximum nesting accepted while scanning a declaration file.
const MAX_SCAN_DEPTH: usize = 256;

/// Maximum local declarations chained before a type reference is rejected.
const MAX_RESOLVE_DEPTH: usize = 4;

/// Name of the entry interface that maps WXML tags to component declarations.
const COMPONENT_MAP: &str = "WxmlComponentMap";

/// Component declaration segment holding attribute declarations.
const PROPS_SEGMENT: &str = "props";

/// Component declaration segment holding event declarations.
const EVENTS_SEGMENT: &str = "events";

/// Member of an event object that carries the event detail type.
const DETAIL_MEMBER: &str = "detail";

/// Primitive TypeScript types understood by the declaration subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrimitiveType {
    /// `string`
    String,
    /// `number`
    Number,
    /// `boolean`
    Boolean,
    /// `any`
    Any,
    /// `unknown`
    Unknown,
    /// `null`
    Null,
    /// `undefined`
    Undefined,
    /// `never`
    Never,
    /// `void`
    Void,
    /// `object`
    Object,
}

impl PrimitiveType {
    /// Returns the TypeScript spelling of the primitive.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Any => "any",
            Self::Unknown => "unknown",
            Self::Null => "null",
            Self::Undefined => "undefined",
            Self::Never => "never",
            Self::Void => "void",
            Self::Object => "object",
        }
    }

    /// Maps a bare identifier to a primitive type.
    fn from_identifier(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "any" => Some(Self::Any),
            "unknown" => Some(Self::Unknown),
            "null" => Some(Self::Null),
            "undefined" => Some(Self::Undefined),
            "never" => Some(Self::Never),
            "void" => Some(Self::Void),
            "object" => Some(Self::Object),
            _ => None,
        }
    }
}

/// One member of an object literal type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectTypeMember {
    /// Member name as written, without quotes.
    pub name: String,
    /// Whether the member carries a `?` marker.
    pub optional: bool,
    /// Parsed member type.
    pub ty: TypeText,
}

/// Mini syntax tree for the self-contained type subset.
///
/// [`TypeText::Unproven`] marks a type expression this parser does not
/// understand. A compound type is proven only when every part of it is proven,
/// which keeps a partially understood union or object from being used as if it
/// were exact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeText {
    /// A primitive type such as `string` or `any`.
    Primitive(PrimitiveType),
    /// A string literal type, with the inner text as written.
    StringLiteral(String),
    /// A number literal type, with its source spelling.
    NumberLiteral(String),
    /// A boolean literal type.
    BooleanLiteral(bool),
    /// A union of alternatives in declaration order.
    Union(Vec<Self>),
    /// An array `T[]`.
    Array(Box<Self>),
    /// A `Record<string, T>` mapping.
    Record(Box<Self>),
    /// An object literal type.
    Object(Vec<ObjectTypeMember>),
    /// A type expression outside the supported subset.
    Unproven,
}

impl TypeText {
    /// Parses one type expression.
    ///
    /// The whole input must be consumed; trailing syntax such as a conditional
    /// type or an intersection yields [`TypeText::Unproven`] instead of a
    /// prefix.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut parser = TypeParser::new(text);
        let parsed = parser.parse_union();
        parser.skip_trivia();
        match parsed {
            Some(ty) if parser.at_end() => ty,
            _ => Self::Unproven,
        }
    }

    /// Returns whether every component of the type is understood.
    #[must_use]
    pub fn is_proven(&self) -> bool {
        match self {
            Self::Unproven => false,
            Self::Union(parts) => parts.iter().all(Self::is_proven),
            Self::Array(inner) | Self::Record(inner) => inner.is_proven(),
            Self::Object(members) => members.iter().all(|member| member.ty.is_proven()),
            Self::Primitive(_)
            | Self::StringLiteral(_)
            | Self::NumberLiteral(_)
            | Self::BooleanLiteral(_) => true,
        }
    }

    /// Returns one member of an object type by name.
    #[must_use]
    pub fn object_member(&self, name: &str) -> Option<&ObjectTypeMember> {
        match self {
            Self::Object(members) => members.iter().find(|member| member.name == name),
            _ => None,
        }
    }
}

/// One declared component attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypingProperty {
    /// Attribute name as declared.
    pub name: String,
    /// Original type expression text, kept for hover.
    pub type_text: String,
    /// Whether the declaration used a `?` marker.
    pub optional: bool,
    /// Whether [`TypingProperty::ty`] covers the whole expression exactly.
    pub proven: bool,
    /// Parsed type expression.
    pub ty: TypeText,
}

/// One declared component event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypingEvent {
    /// Event name as declared.
    pub name: String,
    /// Detail type text, or `None` when the event carries no detail.
    pub detail_text: Option<String>,
    /// Parsed detail type, or `None` when the event carries no detail.
    pub detail: Option<TypeText>,
    /// Whether the detail type is fully understood.
    pub proven: bool,
}

/// One WXML tag's declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypingComponent {
    /// Declared attributes, ordered by name.
    pub props: Vec<TypingProperty>,
    /// Declared events, ordered by name.
    pub events: Vec<TypingEvent>,
    /// Whether the whole declaration was understood structurally.
    ///
    /// Type expressions outside the subset keep `complete` true but mark the
    /// affected member `proven = false`.
    pub complete: bool,
    /// Declaration file that supplied the component.
    pub source: ProjectPath,
}

impl TypingComponent {
    /// Returns one declared attribute by name.
    #[must_use]
    pub fn prop(&self, name: &str) -> Option<&TypingProperty> {
        self.props.iter().find(|property| property.name == name)
    }

    /// Returns one declared event by name.
    #[must_use]
    pub fn event(&self, name: &str) -> Option<&TypingEvent> {
        self.events.iter().find(|event| event.name == name)
    }
}

/// Why a declaration could not be used exactly as written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TypingIssueKind {
    /// Source collection stopped before every declaration file was read.
    TruncatedSources,
    /// Two declaration files mapped the same WXML tag.
    DuplicateTag,
    /// A mapped tag does not reference a type declared in the same file.
    UnresolvedComponent,
    /// A map or component declaration is outside the supported subset.
    IncompleteDeclaration,
    /// [`MAX_TYPING_COMPONENTS`] dropped further tags.
    ComponentLimit,
    /// [`MAX_TYPING_MEMBERS`] dropped further members.
    MemberLimit,
    /// The issue cap dropped further issues.
    IssueLimit,
}

impl TypingIssueKind {
    /// Returns the stable lower-case name of the kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TruncatedSources => "truncated-sources",
            Self::DuplicateTag => "duplicate-tag",
            Self::UnresolvedComponent => "unresolved-component",
            Self::IncompleteDeclaration => "incomplete-declaration",
            Self::ComponentLimit => "component-limit",
            Self::MemberLimit => "member-limit",
            Self::IssueLimit => "issue-limit",
        }
    }
}

/// One degradation recorded while parsing declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypingIssue {
    /// Issue category.
    pub kind: TypingIssueKind,
    /// Project-relative declaration file the issue came from, when it has one.
    pub source: Option<ProjectPath>,
    /// Stable human-readable explanation. Never contains an absolute path.
    pub message: String,
}

/// Component typing facts for one project.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentTypingFacts {
    /// Components keyed by WXML tag, in tag order.
    pub components: BTreeMap<String, TypingComponent>,
    /// Project-relative declaration files that were read, in path order.
    pub sources: Vec<ProjectPath>,
    /// Whether every declaration was understood and nothing was dropped.
    pub complete: bool,
    /// Degradations in discovery order.
    pub issues: Vec<TypingIssue>,
}

impl ComponentTypingFacts {
    /// Returns one component by WXML tag.
    #[must_use]
    pub fn component(&self, tag: &str) -> Option<&TypingComponent> {
        self.components.get(tag)
    }
}

/// Parses component typing facts from collected sources.
///
/// Only [`SourceKind::Dts`] entries are read; files are processed in path
/// order, so the first declaration file that maps a tag wins. Other entries and
/// declarations that the map does not reference are ignored without an issue.
///
/// Source-collection truncation is reported only when at least one declaration
/// file was read. A project whose collection stopped before any `.d.ts` entry
/// behaves as if it had no declarations, so an unrelated file budget does not
/// degrade projects that never opted in.
#[must_use]
pub fn parse_component_typing_facts(sources: &SourceSet) -> ComponentTypingFacts {
    let mut facts = ComponentTypingFacts::default();
    let mut ordered: Vec<&SourceFile> = sources
        .files
        .iter()
        .filter(|file| file.kind == SourceKind::Dts)
        .collect();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    facts.sources = ordered.iter().map(|file| file.path.clone()).collect();

    let mut issues = IssueLog::new();
    if sources.truncated && !facts.sources.is_empty() {
        issues.push(
            TypingIssueKind::TruncatedSources,
            None,
            "source collection was truncated; some declaration files were not read".to_owned(),
        );
    }

    let mut declared: BTreeMap<String, ProjectPath> = BTreeMap::new();
    let mut components: Vec<(String, TypingComponent)> = Vec::new();
    for file in ordered {
        let declarations = scan_declarations(&file.text);
        for map in &declarations.maps {
            collect_map_components(
                file,
                map,
                &declarations,
                &mut declared,
                &mut components,
                &mut issues,
            );
        }
    }

    components.sort_by(|left, right| left.0.cmp(&right.0));
    if components.len() > MAX_TYPING_COMPONENTS {
        components.truncate(MAX_TYPING_COMPONENTS);
        issues.push(
            TypingIssueKind::ComponentLimit,
            None,
            format!(
                "component limit ({MAX_TYPING_COMPONENTS}) reached; remaining tags were dropped"
            ),
        );
    }

    facts.complete =
        components.iter().all(|(_, component)| component.complete) && issues.is_empty();
    facts.components = components.into_iter().collect();
    facts.issues = issues.into_vec();
    facts
}

/// Adds every tag mapped by one `WxmlComponentMap` body.
fn collect_map_components(
    file: &SourceFile,
    map: &TypeDeclaration,
    declarations: &FileDeclarations,
    declared: &mut BTreeMap<String, ProjectPath>,
    components: &mut Vec<(String, TypingComponent)>,
    issues: &mut IssueLog,
) {
    if map.unsupported {
        issues.push(
            TypingIssueKind::IncompleteDeclaration,
            Some(&file.path),
            format!(
                "the component map in '{}' inherits or has type parameters, which is not supported",
                file.path.as_str()
            ),
        );
    }
    let Some(members) = resolve_object_members(declarations, &map.text, 0) else {
        issues.push(
            TypingIssueKind::IncompleteDeclaration,
            Some(&file.path),
            format!(
                "the component map in '{}' is not a plain object type",
                file.path.as_str()
            ),
        );
        return;
    };
    for member in members {
        if member.name.is_empty() {
            continue;
        }
        if let Some(first) = declared.get(&member.name) {
            issues.push(
                TypingIssueKind::DuplicateTag,
                Some(&file.path),
                format!(
                    "tag '{}' is declared more than once; using '{}'",
                    member.name,
                    first.as_str()
                ),
            );
            continue;
        }
        declared.insert(member.name.clone(), file.path.clone());
        let component = build_component(
            &member.name,
            &member.type_text,
            declarations,
            &file.path,
            issues,
        );
        components.push((member.name, component));
    }
}

/// Builds one component from its mapped type reference.
fn build_component(
    tag: &str,
    target_text: &str,
    declarations: &FileDeclarations,
    source: &ProjectPath,
    issues: &mut IssueLog,
) -> TypingComponent {
    let mut component = TypingComponent {
        props: Vec::new(),
        events: Vec::new(),
        complete: true,
        source: source.clone(),
    };
    let Some(target) = bare_identifier(target_text) else {
        return incomplete_component(
            component,
            TypingIssueKind::UnresolvedComponent,
            issues,
            tag,
            source,
            "must map to a type declared in the same file",
        );
    };
    let Some(declaration) = declarations.declaration(target) else {
        return incomplete_component(
            component,
            TypingIssueKind::UnresolvedComponent,
            issues,
            tag,
            source,
            &format!("refers to '{target}', which is not declared in this file"),
        );
    };
    if declaration.unsupported {
        return incomplete_component(
            component,
            TypingIssueKind::IncompleteDeclaration,
            issues,
            tag,
            source,
            &format!(
                "refers to '{target}', which inherits or has type parameters that are not supported"
            ),
        );
    }
    let Some(members) = resolve_object_members(declarations, &declaration.text, 0) else {
        return incomplete_component(
            component,
            TypingIssueKind::IncompleteDeclaration,
            issues,
            tag,
            source,
            "does not declare a plain object type",
        );
    };

    let mut has_props = false;
    let mut has_events = false;
    for member in members {
        match member.name.as_str() {
            PROPS_SEGMENT if !has_props => {
                has_props = true;
                if let Some(props) = resolve_object_members(declarations, &member.type_text, 0) {
                    component.props = build_properties(props);
                } else {
                    mark_incomplete(
                        &mut component,
                        issues,
                        tag,
                        source,
                        "has a 'props' segment that is not a plain object type",
                    );
                }
            }
            EVENTS_SEGMENT if !has_events => {
                has_events = true;
                if let Some(events) = resolve_object_members(declarations, &member.type_text, 0) {
                    component.events = build_events(events, declarations);
                } else {
                    mark_incomplete(
                        &mut component,
                        issues,
                        tag,
                        source,
                        "has an 'events' segment that is not a plain object type",
                    );
                }
            }
            _ => {}
        }
    }
    finalize_members(&mut component, tag, source, issues);
    component
}

/// Turns a component into an incomplete one and records why.
fn incomplete_component(
    mut component: TypingComponent,
    kind: TypingIssueKind,
    issues: &mut IssueLog,
    tag: &str,
    source: &ProjectPath,
    reason: &str,
) -> TypingComponent {
    component.complete = false;
    issues.push(kind, Some(source), format!("component '{tag}' {reason}"));
    component
}

/// Marks one component incomplete with a segment-level reason.
fn mark_incomplete(
    component: &mut TypingComponent,
    issues: &mut IssueLog,
    tag: &str,
    source: &ProjectPath,
    reason: &str,
) {
    component.complete = false;
    issues.push(
        TypingIssueKind::IncompleteDeclaration,
        Some(source),
        format!("component '{tag}' {reason}"),
    );
}

/// Sorts component members by name and applies the member budget.
fn finalize_members(
    component: &mut TypingComponent,
    tag: &str,
    source: &ProjectPath,
    issues: &mut IssueLog,
) {
    component
        .props
        .sort_by(|left, right| left.name.cmp(&right.name));
    component
        .props
        .dedup_by(|left, right| left.name == right.name);
    if component.props.len() > MAX_TYPING_MEMBERS {
        component.props.truncate(MAX_TYPING_MEMBERS);
        push_member_limit(issues, tag, source);
    }
    component
        .events
        .sort_by(|left, right| left.name.cmp(&right.name));
    component
        .events
        .dedup_by(|left, right| left.name == right.name);
    if component.events.len() > MAX_TYPING_MEMBERS {
        component.events.truncate(MAX_TYPING_MEMBERS);
        push_member_limit(issues, tag, source);
    }
}

/// Records the member-truncation issue for one component.
fn push_member_limit(issues: &mut IssueLog, tag: &str, source: &ProjectPath) {
    issues.push(
        TypingIssueKind::MemberLimit,
        Some(source),
        format!("member limit ({MAX_TYPING_MEMBERS}) reached for tag '{tag}'; remaining members were dropped"),
    );
}

/// Builds attribute declarations from one parsed object type.
fn build_properties(members: Vec<RawMember>) -> Vec<TypingProperty> {
    members
        .into_iter()
        .filter(|member| !member.name.is_empty())
        .map(|member| {
            let (type_text, bounded) = bounded_type_text(&member.type_text);
            let mut ty = TypeText::parse(&type_text);
            let proven = bounded && ty.is_proven();
            if !bounded {
                ty = TypeText::Unproven;
            }
            TypingProperty {
                name: member.name,
                type_text,
                optional: member.optional,
                proven,
                ty,
            }
        })
        .collect()
}

/// Builds event declarations from one parsed object type.
fn build_events(members: Vec<RawMember>, declarations: &FileDeclarations) -> Vec<TypingEvent> {
    members
        .into_iter()
        .filter(|member| !member.name.is_empty())
        .map(|member| {
            let resolved = event_detail(declarations, &member.type_text);
            let (detail_text, detail, proven) = match resolved {
                EventDetail::None => (None, None, true),
                EventDetail::Unproven => (None, None, false),
                EventDetail::Text(text) => {
                    let (text, bounded) = bounded_type_text(&text);
                    let detail = TypeText::parse(&text);
                    let proven = bounded && detail.is_proven();
                    (Some(text), Some(detail), proven)
                }
            };
            TypingEvent {
                name: member.name,
                detail_text,
                detail,
                proven,
            }
        })
        .collect()
}

/// Resolved event detail of one `events` member.
enum EventDetail {
    /// The event carries no detail.
    None,
    /// The detail type expression text.
    Text(String),
    /// The event declaration is outside the supported subset.
    Unproven,
}

/// Resolves the detail type of one declared event.
///
/// An event object with a `detail` member uses that member's type. An object
/// without `detail` is taken as the detail type itself. `void`, `undefined` and
/// `never` mean no detail; anything else is unproven.
fn event_detail(declarations: &FileDeclarations, text: &str) -> EventDetail {
    let trimmed = text.trim();
    if matches!(trimmed, "void" | "undefined" | "never") {
        return EventDetail::None;
    }
    resolve_object_members(declarations, trimmed, 0).map_or(EventDetail::Unproven, |members| {
        members
            .into_iter()
            .find(|member| member.name == DETAIL_MEMBER)
            .map_or_else(
                || EventDetail::Text(trimmed.to_owned()),
                |member| EventDetail::Text(member.type_text),
            )
    })
}

/// Keeps a type expression within [`MAX_TYPE_TEXT_CHARS`].
///
/// Returns the retained text and whether it is the complete original.
fn bounded_type_text(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_TYPE_TEXT_CHARS {
        return (text.to_owned(), true);
    }
    (text.chars().take(MAX_TYPE_TEXT_CHARS).collect(), false)
}

/// One object member read from a type literal.
#[derive(Clone, Debug)]
struct RawMember {
    /// Member name without quotes.
    name: String,
    /// Whether the member carries a `?` marker.
    optional: bool,
    /// Type expression text, trimmed.
    type_text: String,
}

impl From<RawMember> for ObjectTypeMember {
    fn from(member: RawMember) -> Self {
        Self {
            name: member.name,
            optional: member.optional,
            ty: TypeText::parse(&member.type_text),
        }
    }
}

/// One `interface` or `type` declaration body from a declaration file.
#[derive(Clone, Debug)]
struct TypeDeclaration {
    /// Interface member list or alias type expression, trimmed.
    text: String,
    /// Whether heritage, type parameters or another unsupported form was seen.
    unsupported: bool,
}

/// Declarations collected from one file.
#[derive(Debug, Default)]
struct FileDeclarations {
    /// Non-map interfaces by name; the first declaration wins.
    interfaces: BTreeMap<String, TypeDeclaration>,
    /// Type aliases by name; the first declaration wins.
    aliases: BTreeMap<String, TypeDeclaration>,
    /// Every `WxmlComponentMap` body, in source order.
    maps: Vec<TypeDeclaration>,
}

impl FileDeclarations {
    /// Looks a type up in the interfaces first, then the aliases.
    fn declaration(&self, name: &str) -> Option<&TypeDeclaration> {
        self.interfaces.get(name).or_else(|| self.aliases.get(name))
    }
}

/// Resolves a type expression to object members, following local references.
///
/// Returns `None` when the expression is not a plain object type, when a local
/// declaration has unsupported heritage, or when resolution does not terminate
/// within [`MAX_RESOLVE_DEPTH`].
fn resolve_object_members(
    declarations: &FileDeclarations,
    text: &str,
    depth: usize,
) -> Option<Vec<RawMember>> {
    if depth > MAX_RESOLVE_DEPTH {
        return None;
    }
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let bytes = trimmed.as_bytes();
        let close = find_matching(bytes, 0, bytes.len())?;
        if !trimmed.get(close..)?.trim().is_empty() {
            return None;
        }
        return parse_object_members(trimmed.get(1..close - 1)?);
    }
    let name = bare_identifier(trimmed)?;
    let declaration = declarations.declaration(name)?;
    if declaration.unsupported {
        return None;
    }
    resolve_object_members(declarations, &declaration.text, depth + 1)
}

/// Returns `text` when the whole trimmed input is one bare identifier.
fn bare_identifier(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    let first = *bytes.first()?;
    if !is_identifier_start(first) {
        return None;
    }
    if identifier_end(bytes, 0) != bytes.len() {
        return None;
    }
    Some(trimmed)
}

/// Parses one object type member list.
///
/// Returns `None` when any member cannot be read structurally, so a partially
/// understood object is never presented as a complete one.
fn parse_object_members(inner: &str) -> Option<Vec<RawMember>> {
    let bytes = inner.as_bytes();
    let mut members = Vec::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        index = skip_trivia(bytes, index);
        match bytes.get(index) {
            None => break,
            Some(b';' | b',') => {
                index += 1;
                continue;
            }
            Some(b'}') => return None,
            Some(_) => {}
        }
        let (name, after) = member_name(inner, index)?;
        index = skip_trivia(bytes, after);
        let optional = bytes.get(index) == Some(&b'?');
        if optional {
            index = skip_trivia(bytes, index + 1);
        }
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        let (start, end, next) = read_type_range(bytes, index + 1, bytes.len())?;
        members.push(RawMember {
            name,
            optional,
            type_text: inner.get(start..end)?.to_owned(),
        });
        index = next;
    }
    Some(members)
}

/// Reads one member name, keeping quoted names verbatim.
fn member_name(text: &str, index: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    match *bytes.get(index)? {
        quote @ (b'\'' | b'"') => {
            let after = skip_lexeme(bytes, index)?;
            if bytes.get(after.checked_sub(1)?) != Some(&quote) {
                return None;
            }
            Some((text.get(index + 1..after - 1)?.to_owned(), after))
        }
        byte if is_identifier_start(byte) => {
            let end = identifier_end(bytes, index);
            Some((text.get(index..end)?.to_owned(), end))
        }
        byte if byte.is_ascii_digit() => {
            let end = number_end(bytes, index);
            Some((text.get(index..end)?.to_owned(), end))
        }
        _ => None,
    }
}

/// Reads the type expression of one member.
///
/// Returns the trimmed source range plus the index scanning stopped at. A
/// newline ends the expression unless the type clearly continues, so
/// semicolon-free declaration files still parse one member at a time.
fn read_type_range(bytes: &[u8], start: usize, limit: usize) -> Option<(usize, usize, usize)> {
    let mut index = start;
    let mut paren = 0_usize;
    let mut bracket = 0_usize;
    let mut brace = 0_usize;
    let mut angle = 0_usize;
    let mut stop = limit;
    let mut content_end = start;
    while index < limit {
        if let Some(next) = skip_lexeme(bytes, index) {
            if next > limit {
                stop = limit;
                break;
            }
            let comment = bytes[index] == b'/'
                && bytes
                    .get(index + 1)
                    .is_some_and(|byte| matches!(byte, b'/' | b'*'));
            if !comment {
                content_end = next;
            }
            index = next;
            continue;
        }
        let flat = paren == 0 && bracket == 0 && brace == 0 && angle == 0;
        match bytes[index] {
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => {
                if brace == 0 {
                    stop = index;
                    break;
                }
                brace -= 1;
            }
            b'<' => angle += 1,
            b'>' => angle = angle.saturating_sub(1),
            b';' | b',' if flat => {
                stop = index;
                break;
            }
            b'\n' if flat => {
                let next = skip_trivia(bytes, index + 1);
                let continues = bytes
                    .get(next)
                    .is_some_and(|byte| matches!(byte, b'|' | b'&' | b'?' | b':'))
                    || ends_with_operator(bytes, start, index);
                if !continues {
                    stop = index;
                    break;
                }
            }
            _ => {}
        }
        if !bytes[index].is_ascii_whitespace() {
            content_end = index + 1;
        }
        index += 1;
    }
    let begin = skip_trivia(bytes, start);
    let mut end = content_end.max(begin);
    while end > begin && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (begin < end).then_some((begin, end, stop))
}

/// Returns whether the line ends with a token that continues a type expression.
fn ends_with_operator(bytes: &[u8], start: usize, end: usize) -> bool {
    (start..end)
        .rev()
        .map(|index| bytes[index])
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| {
            matches!(
                byte,
                b'|' | b'&' | b',' | b':' | b'(' | b'[' | b'<' | b'=' | b'.' | b'?'
            )
        })
}

/// Collects declarations from one declaration file.
fn scan_declarations(text: &str) -> FileDeclarations {
    let mut declarations = FileDeclarations::default();
    let bytes = text.as_bytes();
    scan_scope(bytes, text, 0, bytes.len(), 0, &mut declarations);
    declarations
}

/// Scans one declaration scope (file top level or a `declare global` body).
fn scan_scope(
    bytes: &[u8],
    text: &str,
    start: usize,
    end: usize,
    depth: usize,
    declarations: &mut FileDeclarations,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let mut index = start;
    while index < end {
        if let Some(next) = skip_lexeme(bytes, index) {
            if next > end {
                break;
            }
            index = next;
            continue;
        }
        match bytes[index] {
            b'{' => index = find_matching(bytes, index, end).unwrap_or(end),
            byte if is_identifier_start(byte) => {
                let name_end = identifier_end(bytes, index);
                let Some(identifier) = text.get(index..name_end) else {
                    index = name_end;
                    continue;
                };
                index = match identifier {
                    "declare" => scan_declare(bytes, text, name_end, end, depth, declarations),
                    "interface" => scan_interface(bytes, text, name_end, end, declarations),
                    "type" => scan_alias(bytes, text, name_end, end, declarations),
                    _ => name_end,
                };
            }
            _ => index += 1,
        }
    }
}

/// Handles `declare global { ... }` and lets other `declare` forms continue.
fn scan_declare(
    bytes: &[u8],
    text: &str,
    cursor: usize,
    limit: usize,
    depth: usize,
    declarations: &mut FileDeclarations,
) -> usize {
    let next = skip_trivia(bytes, cursor);
    let Some((start, end)) = identifier_at(bytes, next) else {
        return cursor;
    };
    if text.get(start..end) != Some("global") {
        return end;
    }
    let open = skip_trivia(bytes, end);
    if bytes.get(open) != Some(&b'{') {
        return end;
    }
    let Some(close) = find_matching(bytes, open, limit) else {
        return limit;
    };
    if close > open + 1 {
        scan_scope(bytes, text, open + 1, close - 1, depth + 1, declarations);
    }
    close
}

/// Reads one `interface NAME ... { ... }` declaration.
fn scan_interface(
    bytes: &[u8],
    text: &str,
    cursor: usize,
    limit: usize,
    declarations: &mut FileDeclarations,
) -> usize {
    let name_start = skip_trivia(bytes, cursor);
    if name_start >= limit
        || !bytes
            .get(name_start)
            .is_some_and(|byte| is_identifier_start(*byte))
    {
        return cursor;
    }
    let name_end = identifier_end(bytes, name_start);
    let Some(name) = text.get(name_start..name_end) else {
        return name_end;
    };
    let mut index = name_end;
    let mut unsupported = false;
    loop {
        index = skip_trivia(bytes, index);
        if index >= limit {
            return index;
        }
        match bytes[index] {
            b'{' => break,
            b';' | b'}' => return index,
            b'<' => match find_matching_angle(bytes, index, limit) {
                Some(close) => {
                    unsupported = true;
                    index = close;
                }
                None => return index,
            },
            _ => {
                unsupported = true;
                index += 1;
            }
        }
    }
    let Some(close) = find_matching(bytes, index, limit) else {
        return limit;
    };
    let Some(body) = text.get(index..close) else {
        return close;
    };
    let declaration = TypeDeclaration {
        text: body.trim().to_owned(),
        unsupported,
    };
    if name == COMPONENT_MAP {
        declarations.maps.push(declaration);
    } else {
        declarations
            .interfaces
            .entry(name.to_owned())
            .or_insert(declaration);
    }
    close
}

/// Reads one `type NAME = ...` alias declaration.
fn scan_alias(
    bytes: &[u8],
    text: &str,
    cursor: usize,
    limit: usize,
    declarations: &mut FileDeclarations,
) -> usize {
    let name_start = skip_trivia(bytes, cursor);
    if name_start >= limit
        || !bytes
            .get(name_start)
            .is_some_and(|byte| is_identifier_start(*byte))
    {
        return cursor;
    }
    let name_end = identifier_end(bytes, name_start);
    let Some(name) = text.get(name_start..name_end) else {
        return name_end;
    };
    let equals = skip_trivia(bytes, name_end);
    if bytes.get(equals) != Some(&b'=') {
        return name_end;
    }
    let Some((start, end, next)) = read_type_range(bytes, equals + 1, limit) else {
        return name_end;
    };
    let Some(body) = text.get(start..end) else {
        return next;
    };
    let definition = TypeDeclaration {
        text: body.to_owned(),
        unsupported: false,
    };
    declarations
        .aliases
        .entry(name.to_owned())
        .or_insert(definition);
    next
}

/// Accumulates issues with a hard cap.
struct IssueLog {
    /// Collected issues.
    items: Vec<TypingIssue>,
    /// Whether the cap replaced further issues with one marker.
    capped: bool,
}

impl IssueLog {
    /// Creates an empty log.
    const fn new() -> Self {
        Self {
            items: Vec::new(),
            capped: false,
        }
    }

    /// Records one issue, keeping the last slot for a cap marker.
    fn push(&mut self, kind: TypingIssueKind, source: Option<&ProjectPath>, message: String) {
        if self.capped {
            return;
        }
        if self.items.len() >= MAX_TYPING_ISSUES.saturating_sub(1) {
            self.items.push(TypingIssue {
                kind: TypingIssueKind::IssueLimit,
                source: source.cloned(),
                message: format!(
                    "issue limit ({MAX_TYPING_ISSUES}) reached; remaining issues were dropped"
                ),
            });
            self.capped = true;
            return;
        }
        self.items.push(TypingIssue {
            kind,
            source: source.cloned(),
            message,
        });
    }

    /// Returns whether no issue was recorded.
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Consumes the log.
    fn into_vec(self) -> Vec<TypingIssue> {
        self.items
    }
}

/// Recursive-descent parser for the supported type subset.
struct TypeParser<'a> {
    /// Whole type expression.
    text: &'a str,
    /// Bytes of [`TypeParser::text`].
    bytes: &'a [u8],
    /// Current byte offset.
    index: usize,
}

impl<'a> TypeParser<'a> {
    /// Creates a parser at the start of `text`.
    const fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            index: 0,
        }
    }

    /// Returns whether the whole expression was consumed.
    const fn at_end(&self) -> bool {
        self.index >= self.bytes.len()
    }

    /// Skips whitespace and comments.
    fn skip_trivia(&mut self) {
        self.index = skip_trivia(self.bytes, self.index);
    }

    /// Returns the current byte without consuming it.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    /// Parses a union, tolerating a leading `|`.
    fn parse_union(&mut self) -> Option<TypeText> {
        self.skip_trivia();
        while self.peek() == Some(b'|') {
            self.index += 1;
            self.skip_trivia();
        }
        let mut parts = Vec::new();
        loop {
            parts.push(self.parse_primary()?);
            self.skip_trivia();
            if self.peek() != Some(b'|') {
                break;
            }
            self.index += 1;
        }
        Some(if parts.len() == 1 {
            parts.pop()?
        } else {
            TypeText::Union(parts)
        })
    }

    /// Parses one primary type with its postfix `[]` suffixes.
    fn parse_primary(&mut self) -> Option<TypeText> {
        self.skip_trivia();
        let mut ty = self.parse_atom()?;
        loop {
            self.skip_trivia();
            if self.peek() != Some(b'[') {
                break;
            }
            let after = skip_trivia(self.bytes, self.index + 1);
            if self.bytes.get(after) == Some(&b']') {
                self.index = after + 1;
                ty = TypeText::Array(Box::new(ty));
                continue;
            }
            // Indexed access such as `T['key']` is outside the subset.
            self.index =
                find_matching(self.bytes, self.index, self.bytes.len()).unwrap_or(self.bytes.len());
            ty = TypeText::Unproven;
        }
        self.skip_trivia();
        if self.identifier_here("extends") {
            self.skip_unsupported();
            ty = TypeText::Unproven;
        }
        Some(ty)
    }

    /// Parses one atom of a type expression.
    fn parse_atom(&mut self) -> Option<TypeText> {
        self.skip_trivia();
        let byte = self.peek()?;
        match byte {
            b'(' => self.parse_parenthesised(),
            b'{' => self.parse_object_literal(),
            b'\'' | b'"' => self.parse_string_literal(),
            b'`' => {
                self.index = skip_lexeme(self.bytes, self.index).unwrap_or(self.bytes.len());
                Some(TypeText::Unproven)
            }
            b'-' | b'0'..=b'9' => self.parse_number_literal(),
            _ if is_identifier_start(byte) => self.parse_identifier_atom(),
            _ => {
                self.skip_unsupported();
                Some(TypeText::Unproven)
            }
        }
    }

    /// Parses a parenthesised type.
    fn parse_parenthesised(&mut self) -> Option<TypeText> {
        let close = find_matching(self.bytes, self.index, self.bytes.len())?;
        let inner = self.text.get(self.index + 1..close - 1)?;
        self.index = close;
        Some(TypeText::parse(inner))
    }

    /// Parses an object literal type.
    fn parse_object_literal(&mut self) -> Option<TypeText> {
        let close = find_matching(self.bytes, self.index, self.bytes.len())?;
        let inner = self.text.get(self.index + 1..close - 1)?;
        self.index = close;
        Some(
            parse_object_members(inner).map_or(TypeText::Unproven, |members| {
                TypeText::Object(members.into_iter().map(ObjectTypeMember::from).collect())
            }),
        )
    }

    /// Parses a quoted string literal type.
    fn parse_string_literal(&mut self) -> Option<TypeText> {
        let quote = self.peek()?;
        let after = skip_lexeme(self.bytes, self.index)?;
        if self.bytes.get(after.checked_sub(1)?) != Some(&quote) {
            self.index = after;
            return Some(TypeText::Unproven);
        }
        let inner = self.text.get(self.index + 1..after - 1)?;
        self.index = after;
        Some(TypeText::StringLiteral(inner.to_owned()))
    }

    /// Parses a numeric literal type.
    fn parse_number_literal(&mut self) -> Option<TypeText> {
        let start = self.index;
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        let digits = self.index;
        while self.bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
            self.index += 1;
        }
        if self.index == digits {
            return Some(TypeText::Unproven);
        }
        if self.peek() == Some(b'.') {
            self.index += 1;
            while self.bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
                self.index += 1;
            }
        }
        Some(TypeText::NumberLiteral(
            self.text.get(start..self.index)?.to_owned(),
        ))
    }

    /// Parses an identifier atom: primitive, literal, generic or reference.
    fn parse_identifier_atom(&mut self) -> Option<TypeText> {
        let start = self.index;
        let end = identifier_end(self.bytes, start);
        let name = self.text.get(start..end)?;
        self.index = end;
        match name {
            "true" => return Some(TypeText::BooleanLiteral(true)),
            "false" => return Some(TypeText::BooleanLiteral(false)),
            _ => {}
        }
        if let Some(primitive) = PrimitiveType::from_identifier(name) {
            return Some(TypeText::Primitive(primitive));
        }
        let after = skip_trivia(self.bytes, self.index);
        if name == "import" && self.bytes.get(after) == Some(&b'(') {
            self.index =
                find_matching(self.bytes, after, self.bytes.len()).unwrap_or(self.bytes.len());
            return Some(TypeText::Unproven);
        }
        if matches!(
            name,
            "keyof" | "typeof" | "infer" | "readonly" | "unique" | "asserts"
        ) {
            self.skip_unsupported();
            return Some(TypeText::Unproven);
        }
        if self.bytes.get(after) == Some(&b'<') {
            let Some(close) = find_matching_angle(self.bytes, after, self.bytes.len()) else {
                self.index = self.bytes.len();
                return Some(TypeText::Unproven);
            };
            let args = self.text.get(after + 1..close - 1)?;
            self.index = close;
            return Some(record_or_unproven(name, args));
        }
        if self.bytes.get(after) == Some(&b'.') {
            self.index = self.skip_qualified_name(after);
            return Some(TypeText::Unproven);
        }
        Some(TypeText::Unproven)
    }

    /// Skips a dotted name chain starting at `.`.
    fn skip_qualified_name(&self, index: usize) -> usize {
        let mut cursor = index;
        while self.bytes.get(cursor) == Some(&b'.') {
            let next = skip_trivia(self.bytes, cursor + 1);
            if !self
                .bytes
                .get(next)
                .is_some_and(|byte| is_identifier_start(*byte))
            {
                return next;
            }
            cursor = identifier_end(self.bytes, next);
        }
        cursor
    }

    /// Returns whether `keyword` is the identifier at the current byte.
    fn identifier_here(&self, keyword: &str) -> bool {
        let end = self.index.saturating_add(keyword.len());
        self.bytes
            .get(self.index..end)
            .is_some_and(|slice| slice == keyword.as_bytes())
            && self
                .bytes
                .get(end)
                .is_none_or(|byte| !is_identifier_continue(*byte))
    }

    /// Skips an unsupported term up to the next top-level union or separator.
    fn skip_unsupported(&mut self) {
        let mut index = self.index;
        let mut depth = 0_usize;
        while index < self.bytes.len() {
            if let Some(next) = skip_lexeme(self.bytes, index) {
                index = next;
                continue;
            }
            match self.bytes[index] {
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
                b'|' | b';' | b',' if depth == 0 => break,
                _ => {}
            }
            index += 1;
        }
        self.index = index;
    }
}

/// Maps `Record<string, T>` to a record type; other generics stay unproven.
fn record_or_unproven(name: &str, args: &str) -> TypeText {
    if name != "Record" {
        return TypeText::Unproven;
    }
    let parts = split_top_level_args(args);
    if parts.len() != 2 || parts[0].trim() != "string" {
        return TypeText::Unproven;
    }
    TypeText::Record(Box::new(TypeText::parse(parts[1])))
}

/// Splits generic arguments on top-level commas.
fn split_top_level_args(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    let mut index = 0_usize;
    let mut depth = 0_usize;
    while index < bytes.len() {
        if let Some(next) = skip_lexeme(bytes, index) {
            index = next;
            continue;
        }
        match bytes[index] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(text.get(start..index).unwrap_or_default());
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start <= text.len() {
        parts.push(text.get(start..).unwrap_or_default());
    }
    parts
}

/// Returns `true` for the first byte of an identifier.
const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// Returns `true` for a continuation byte of an identifier.
const fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

/// Returns the first byte after the identifier starting at `start`.
fn identifier_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start.saturating_add(1);
    while bytes
        .get(index)
        .is_some_and(|byte| is_identifier_continue(*byte))
    {
        index += 1;
    }
    index
}

/// Returns the first byte after the number starting at `start`.
fn number_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
    {
        index += 1;
    }
    index
}

/// Returns the identifier at `index`, if any.
fn identifier_at(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let start = skip_trivia(bytes, index);
    if !bytes
        .get(start)
        .is_some_and(|byte| is_identifier_start(*byte))
    {
        return None;
    }
    Some((start, identifier_end(bytes, start)))
}

/// Skips whitespace and comments.
fn skip_trivia(bytes: &[u8], mut index: usize) -> usize {
    loop {
        match bytes.get(index) {
            Some(byte) if byte.is_ascii_whitespace() => index += 1,
            Some(b'/') => match skip_lexeme(bytes, index) {
                Some(next) if next > index => index = next,
                _ => return index,
            },
            _ => return index,
        }
    }
}

/// Skips a comment or string literal starting at `index`.
///
/// Returns `None` when the byte does not start a lexeme. Unterminated lexemes
/// consume the rest of the input.
fn skip_lexeme(bytes: &[u8], index: usize) -> Option<usize> {
    match *bytes.get(index)? {
        b'/' if bytes.get(index + 1) == Some(&b'/') => {
            let mut cursor = index + 2;
            while bytes.get(cursor).is_some_and(|byte| *byte != b'\n') {
                cursor += 1;
            }
            Some(cursor)
        }
        b'/' if bytes.get(index + 1) == Some(&b'*') => {
            let mut cursor = index + 2;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    return Some(cursor + 2);
                }
                cursor += 1;
            }
            Some(bytes.len())
        }
        quote @ (b'\'' | b'"') => {
            let mut cursor = index + 1;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'\\' => cursor += 2,
                    byte if byte == quote => return Some(cursor + 1),
                    _ => cursor += 1,
                }
            }
            Some(bytes.len())
        }
        b'`' => {
            let mut cursor = index + 1;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'\\' => cursor += 2,
                    b'`' => return Some(cursor + 1),
                    _ => cursor += 1,
                }
            }
            Some(bytes.len())
        }
        _ => None,
    }
}

/// Finds the byte after the delimiter opened at `open`.
fn find_matching(bytes: &[u8], open: usize, limit: usize) -> Option<usize> {
    let expected = matching_close(*bytes.get(open)?)?;
    let mut pending = vec![expected];
    let mut index = open + 1;
    while index < limit {
        if let Some(next) = skip_lexeme(bytes, index) {
            if next > limit {
                return None;
            }
            index = next;
            continue;
        }
        match bytes[index] {
            byte @ (b'{' | b'[' | b'(') => {
                if pending.len() >= MAX_SCAN_DEPTH {
                    return None;
                }
                pending.push(matching_close(byte)?);
            }
            b'}' | b']' | b')' => {
                if pending.last() != Some(&bytes[index]) {
                    return None;
                }
                pending.pop();
                if pending.is_empty() {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Finds the byte after the matching `>` of a generic argument list.
fn find_matching_angle(bytes: &[u8], open: usize, limit: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut index = open;
    while index < limit {
        if let Some(next) = skip_lexeme(bytes, index) {
            index = next;
            continue;
        }
        match bytes[index] {
            b'<' => depth += 1,
            b'>' => {
                let arrow = index
                    .checked_sub(1)
                    .and_then(|previous| bytes.get(previous))
                    == Some(&b'=');
                if !arrow {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index + 1);
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Maps an opening delimiter to its closing byte.
const fn matching_close(open: u8) -> Option<u8> {
    match open {
        b'{' => Some(b'}'),
        b'[' => Some(b']'),
        b'(' => Some(b')'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one source entry, classifying `.d.ts` files as declarations.
    fn source(path: &str, text: &str) -> SourceFile {
        let kind = if path.ends_with(".d.ts") {
            SourceKind::Dts
        } else {
            SourceKind::Ts
        };
        SourceFile {
            path: ProjectPath::new(path).expect("valid path"),
            kind,
            text: text.to_owned(),
            source_revision: 1,
        }
    }

    /// Builds a source set from declaration files.
    fn source_set(files: &[(&str, &str)]) -> SourceSet {
        SourceSet {
            files: files
                .iter()
                .map(|(path, text)| source(path, text))
                .collect(),
            truncated: false,
            issues: Vec::new(),
        }
    }

    /// Parses facts from declaration files.
    fn facts(files: &[(&str, &str)]) -> ComponentTypingFacts {
        parse_component_typing_facts(&source_set(files))
    }

    #[test]
    fn parses_the_supported_type_subset() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'demo-box': DemoBox;
            }
            interface DemoBox {
              props: {
                text: string;
                count: number;
                flag: boolean;
                kind: 'small' | 'large';
                ratio: 1 | 2;
                yes: true;
                items: string[];
                matrix: number[][];
                grouped: Record<string, string>;
                nested: { value?: string; deep: { id: number } };
                parenthesised: (string | number)[];
              };
            }
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("demo-box").expect("component");
        assert!(component.complete);

        let prop = |name: &str| component.prop(name).expect(name);
        assert_eq!(prop("text").ty, TypeText::Primitive(PrimitiveType::String));
        assert_eq!(prop("count").ty, TypeText::Primitive(PrimitiveType::Number));
        assert_eq!(prop("flag").ty, TypeText::Primitive(PrimitiveType::Boolean));
        assert_eq!(
            prop("kind").ty,
            TypeText::Union(vec![
                TypeText::StringLiteral("small".to_owned()),
                TypeText::StringLiteral("large".to_owned()),
            ])
        );
        assert_eq!(prop("kind").type_text, "'small' | 'large'");
        assert_eq!(
            prop("ratio").ty,
            TypeText::Union(vec![
                TypeText::NumberLiteral("1".to_owned()),
                TypeText::NumberLiteral("2".to_owned()),
            ])
        );
        assert_eq!(prop("yes").ty, TypeText::BooleanLiteral(true));
        assert_eq!(
            prop("items").ty,
            TypeText::Array(Box::new(TypeText::Primitive(PrimitiveType::String)))
        );
        assert_eq!(
            prop("matrix").ty,
            TypeText::Array(Box::new(TypeText::Array(Box::new(TypeText::Primitive(
                PrimitiveType::Number
            )))))
        );
        assert_eq!(
            prop("grouped").ty,
            TypeText::Record(Box::new(TypeText::Primitive(PrimitiveType::String)))
        );
        let nested = prop("nested");
        assert!(nested.proven);
        assert_eq!(
            nested
                .ty
                .object_member("value")
                .map(|member| member.optional),
            Some(true)
        );
        assert_eq!(
            nested
                .ty
                .object_member("deep")
                .and_then(|member| member.ty.object_member("id"))
                .map(|member| member.ty.clone()),
            Some(TypeText::Primitive(PrimitiveType::Number))
        );
        assert_eq!(
            prop("parenthesised").ty,
            TypeText::Array(Box::new(TypeText::Union(vec![
                TypeText::Primitive(PrimitiveType::String),
                TypeText::Primitive(PrimitiveType::Number),
            ])))
        );
        assert!(component.props.iter().all(|property| property.proven));
    }

    #[test]
    fn parses_members_without_semicolons_and_multiline_unions() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'loose-box': LooseBox;
            }
            interface LooseBox {
              props: {
                /* 类型注释不进入 type_text */
                kind: 'small'
                  | 'large'
                text: string // 尾注释同样不进入 type_text
                nested: {
                  id: number
                  label?: string
                }
              }
            }
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("loose-box").expect("component");
        assert_eq!(component.props.len(), 3);

        let kind = component.prop("kind").expect("kind");
        assert!(kind.proven);
        assert_eq!(
            kind.ty,
            TypeText::Union(vec![
                TypeText::StringLiteral("small".to_owned()),
                TypeText::StringLiteral("large".to_owned()),
            ])
        );
        assert!(!kind.type_text.contains("注释"));

        let text = component.prop("text").expect("text");
        assert_eq!(text.type_text, "string");
        assert!(text.proven);

        let nested = component.prop("nested").expect("nested");
        assert!(nested.proven);
        assert_eq!(
            nested
                .ty
                .object_member("id")
                .map(|member| member.ty.clone()),
            Some(TypeText::Primitive(PrimitiveType::Number))
        );
        assert_eq!(
            nested
                .ty
                .object_member("label")
                .map(|member| member.optional),
            Some(true)
        );
    }

    #[test]
    fn marks_outside_types_unproven() {
        for text in [
            "SomeLib.Options",
            "ReadonlyArray<string>",
            "T extends string ? A : B",
            "import('lib').Thing",
            "{ a: string } & { b: number }",
            "(value: string) => void",
            "T['key']",
            "keyof T",
            "`prefix-${string}`",
            "",
        ] {
            let ty = TypeText::parse(text);
            assert!(!ty.is_proven(), "{text}");
        }
        assert!(TypeText::parse("string").is_proven());
        assert!(TypeText::parse("Record<string, string>").is_proven());
        assert!(!TypeText::parse("Record<number, string>").is_proven());
    }

    #[test]
    fn keeps_unproven_members_but_still_completes_the_declaration() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'demo-box': DemoBox;
            }
            interface DemoBox {
              props: {
                options?: SomeLib.Options;
                text: string;
              };
            }
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("demo-box").expect("component");
        assert!(component.complete);
        let options = component.prop("options").expect("options");
        assert!(!options.proven);
        assert_eq!(options.type_text, "SomeLib.Options");
        assert!(options.optional);
        assert!(component.prop("text").expect("text").proven);
    }

    #[test]
    fn parses_top_level_and_global_component_maps() {
        let facts = facts(&[
            (
                "types/a.d.ts",
                r"
                interface WxmlComponentMap {
                  'a-box': ABox;
                }
                interface ABox { props: { a: string } }
                ",
            ),
            (
                "types/b.d.ts",
                r"
                declare global {
                  interface WxmlComponentMap {
                    'b-box': BBox;
                  }
                  interface BBox { props: { b: number } }
                }
                export {};
                ",
            ),
        ]);
        assert!(facts.complete, "{:?}", facts.issues);
        assert_eq!(facts.sources.len(), 2);
        assert_eq!(facts.sources[0].as_str(), "types/a.d.ts");
        assert!(facts.component("a-box").is_some());
        assert!(facts.component("b-box").is_some());
    }

    #[test]
    fn reads_props_and_events_from_a_mapped_interface() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            declare global {
              interface WxmlComponentMap {
                'van-button': VanButtonProps;
              }
              interface VanButtonProps {
                props: {
                  type?: 'primary' | 'default' | 'warning';
                  loading?: boolean;
                  text: string;
                };
                events: {
                  change: { detail: { index: number } };
                  input: { detail: { value: string } };
                  tap: void;
                };
              }
            }
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("van-button").expect("component");
        assert_eq!(
            component
                .props
                .iter()
                .map(|property| property.name.as_str())
                .collect::<Vec<_>>(),
            ["loading", "text", "type"]
        );
        let type_prop = component.prop("type").expect("type");
        assert!(type_prop.optional);
        assert!(type_prop.proven);
        assert_eq!(type_prop.type_text, "'primary' | 'default' | 'warning'");
        assert!(!component.prop("text").expect("text").optional);

        let change = component.event("change").expect("change");
        assert_eq!(change.detail_text.as_deref(), Some("{ index: number }"));
        assert!(change.proven);
        assert_eq!(
            change
                .detail
                .as_ref()
                .and_then(|detail| detail.object_member("index"))
                .map(|member| member.ty.clone()),
            Some(TypeText::Primitive(PrimitiveType::Number))
        );
        assert_eq!(component.props.len(), 3);
        assert_eq!(component.events.len(), 3);

        let tap = component.event("tap").expect("tap");
        assert_eq!(tap.detail_text, None);
        assert!(tap.proven);
    }

    #[test]
    fn ignores_interfaces_not_referenced_by_the_map() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'used-box': UsedBox;
            }
            interface UsedBox { props: { a: string } }
            interface Orphan { props: { unused: string } }
            ",
        )]);
        assert_eq!(facts.components.len(), 1);
        assert!(facts.component("used-box").is_some());
        assert!(facts.complete);
    }

    #[test]
    fn ignores_unrelated_declarations() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            import type { Something } from 'somewhere';

            interface WxmlComponentMap {
              'used-box': UsedBox;
            }

            interface UsedBox {
              props: { a: string };
            }

            interface Utility { value: string }
            type Unused = 'a' | 'b';
            declare function helper(): void;
            declare namespace Hidden {
              interface Inner { b: number }
            }
            declare global {
              interface Window { marker: string }
            }
            export {};
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        assert_eq!(facts.issues, Vec::new());
        assert_eq!(facts.components.len(), 1);
    }

    #[test]
    fn resolves_local_props_and_events_aliases() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'aliased-box': AliasedBox;
            }
            interface AliasedBox {
              props: AliasedProps;
              events: AliasedEvents;
            }
            type AliasedProps = { size?: 'small' | 'large' };
            interface AliasedEvents {
              change: ChangeDetail;
            }
            type ChangeDetail = { detail: { ok: boolean } };
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("aliased-box").expect("component");
        assert_eq!(
            component.prop("size").map(|property| property.proven),
            Some(true)
        );
        let change = component.event("change").expect("change");
        assert_eq!(change.detail_text.as_deref(), Some("{ ok: boolean }"));
        assert!(change.proven);
    }

    #[test]
    fn marks_component_incomplete_when_the_target_is_missing() {
        let facts = facts(&[(
            "types/components.d.ts",
            "interface WxmlComponentMap { 'ghost-box': GhostBox; }",
        )]);
        assert!(!facts.complete);
        let component = facts.component("ghost-box").expect("component");
        assert!(!component.complete);
        let issue = facts.issues.first().expect("issue");
        assert_eq!(issue.kind, TypingIssueKind::UnresolvedComponent);
        assert_eq!(
            issue.message,
            "component 'ghost-box' refers to 'GhostBox', which is not declared in this file"
        );
        assert_eq!(
            issue.source.as_ref().map(ProjectPath::as_str),
            Some("types/components.d.ts")
        );
    }

    #[test]
    fn marks_component_incomplete_when_a_segment_is_not_an_object() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'plain-box': PlainBox;
            }
            interface PlainBox { props: string }
            ",
        )]);
        assert!(!facts.complete);
        let component = facts.component("plain-box").expect("component");
        assert!(!component.complete);
        assert_eq!(component.props, Vec::new());
        assert_eq!(facts.issues[0].kind, TypingIssueKind::IncompleteDeclaration);
        assert_eq!(
            facts.issues[0].message,
            "component 'plain-box' has a 'props' segment that is not a plain object type"
        );
    }

    #[test]
    fn marks_component_incomplete_when_the_target_inherits() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'child-box': ChildBox;
            }
            interface ChildBox extends ParentBox {
              props: { a: string };
            }
            interface ParentBox { props: { b: number } }
            ",
        )]);
        assert!(!facts.complete);
        let component = facts.component("child-box").expect("component");
        assert!(!component.complete);
        assert_eq!(facts.issues[0].kind, TypingIssueKind::IncompleteDeclaration);
    }

    #[test]
    fn keeps_the_first_declaration_for_a_duplicate_tag() {
        let facts = facts(&[
            (
                "types/b.d.ts",
                "interface WxmlComponentMap { 'shared-box': SecondBox; } interface SecondBox { props: { b: number } }",
            ),
            (
                "types/a.d.ts",
                "interface WxmlComponentMap { 'shared-box': FirstBox; } interface FirstBox { props: { a: string } }",
            ),
        ]);
        assert!(!facts.complete);
        let component = facts.component("shared-box").expect("component");
        assert!(component.prop("a").is_some());
        assert!(component.prop("b").is_none());
        assert_eq!(component.source.as_str(), "types/a.d.ts");
        let issue = facts
            .issues
            .iter()
            .find(|issue| issue.kind == TypingIssueKind::DuplicateTag)
            .expect("duplicate issue");
        assert_eq!(
            issue.message,
            "tag 'shared-box' is declared more than once; using 'types/a.d.ts'"
        );
        assert_eq!(
            issue.source.as_ref().map(ProjectPath::as_str),
            Some("types/b.d.ts")
        );
    }

    #[test]
    fn truncates_components_by_tag_order() {
        use std::fmt::Write as _;

        let mut text = String::from("interface WxmlComponentMap {\n");
        for index in 0..600 {
            writeln!(text, "  'tag-{index:03}': C{index};").expect("write tag");
        }
        text.push_str("}\n");
        for index in 0..600 {
            writeln!(text, "interface C{index} {{ props: {{ a: string }} }}").expect("write type");
        }
        let facts = facts(&[("types/many.d.ts", text.as_str())]);
        assert_eq!(facts.components.len(), MAX_TYPING_COMPONENTS);
        assert!(facts.component("tag-000").is_some());
        assert!(facts.component("tag-511").is_some());
        assert!(facts.component("tag-512").is_none());
        assert!(!facts.complete);
        let issue = facts
            .issues
            .iter()
            .find(|issue| issue.kind == TypingIssueKind::ComponentLimit)
            .expect("component limit issue");
        assert_eq!(
            issue.message,
            "component limit (512) reached; remaining tags were dropped"
        );
    }

    #[test]
    fn truncates_members_by_name_order() {
        use std::fmt::Write as _;

        let mut text = String::from(
            "interface WxmlComponentMap { 'wide-box': WideBox; }\ninterface WideBox {\n  props: {\n",
        );
        for index in 0..1100 {
            writeln!(text, "    p{index:04}: string;").expect("write property");
        }
        text.push_str("  };\n}\n");
        let facts = facts(&[("types/wide.d.ts", text.as_str())]);
        assert!(facts.issues.iter().any(|issue| {
            issue.kind == TypingIssueKind::MemberLimit
                && issue.message
                    == "member limit (1024) reached for tag 'wide-box'; remaining members were dropped"
        }));
        let component = facts.component("wide-box").expect("component");
        assert_eq!(component.props.len(), MAX_TYPING_MEMBERS);
        assert!(component.prop("p0000").is_some());
        assert!(component.prop("p1023").is_some());
        assert!(component.prop("p1024").is_none());
        assert!(!facts.complete);
    }

    #[test]
    fn records_source_collection_truncation() {
        let mut sources = source_set(&[(
            "types/components.d.ts",
            "interface WxmlComponentMap { 'a-box': ABox; } interface ABox { props: { a: string } }",
        )]);
        sources.truncated = true;
        let facts = parse_component_typing_facts(&sources);
        assert!(!facts.complete);
        assert_eq!(facts.issues[0].kind, TypingIssueKind::TruncatedSources);
    }

    #[test]
    fn does_not_report_truncation_without_declaration_files() {
        let mut sources = source_set(&[("pages/index/index.ts", "Page({});")]);
        sources.truncated = true;
        let facts = parse_component_typing_facts(&sources);
        assert!(facts.complete);
        assert_eq!(facts.issues, Vec::new());
        assert_eq!(facts.sources, Vec::new());
    }

    #[test]
    fn unproven_event_details_are_not_proven() {
        let facts = facts(&[(
            "types/components.d.ts",
            r"
            interface WxmlComponentMap {
              'event-box': EventBox;
            }
            interface EventBox {
              events: {
                known: { detail: { ok: boolean } };
                bare: { value: string };
                external: CustomEvent<{ id: number }>;
                empty: undefined;
              };
            }
            ",
        )]);
        assert!(facts.complete, "{:?}", facts.issues);
        let component = facts.component("event-box").expect("component");
        assert_eq!(
            component
                .events
                .iter()
                .map(|event| event.name.as_str())
                .collect::<Vec<_>>(),
            ["bare", "empty", "external", "known"]
        );
        assert_eq!(
            component
                .event("bare")
                .and_then(|event| event.detail_text.clone()),
            Some("{ value: string }".to_owned())
        );
        assert!(component.event("bare").expect("bare").proven);
        assert_eq!(component.event("empty").expect("empty").detail_text, None);
        assert!(component.event("empty").expect("empty").proven);
        let external = component.event("external").expect("external");
        assert!(!external.proven);
        assert_eq!(external.detail_text, None);
        assert!(external.detail.is_none());
    }

    #[test]
    fn ignores_non_declaration_sources() {
        let sources = SourceSet {
            files: vec![SourceFile {
                path: ProjectPath::new("types/components.ts").expect("path"),
                kind: SourceKind::Ts,
                text: "interface WxmlComponentMap { 'a-box': ABox; }".to_owned(),
                source_revision: 1,
            }],
            truncated: false,
            issues: Vec::new(),
        };
        let facts = parse_component_typing_facts(&sources);
        assert_eq!(facts.components, BTreeMap::new());
        assert_eq!(facts.sources, Vec::new());
        assert!(facts.complete);
    }

    #[test]
    fn malformed_declarations_do_not_panic() {
        for text in [
            "",
            "interface",
            "interface WxmlComponentMap {",
            "interface WxmlComponentMap { 'a-box': }",
            "interface WxmlComponentMap { 'a-box': ABox; } interface ABox { props: {",
            "interface WxmlComponentMap { 'a-box': ABox; } interface ABox { props: { a: 'unterminated }",
            "declare global {",
            "type A = {",
            "}}}}{{{ interface WxmlComponentMap {}",
            "interface WxmlComponentMap { 'a-b': A; } type A = A;",
        ] {
            let facts = facts(&[("types/broken.d.ts", text)]);
            assert_eq!(
                facts.sources,
                vec![ProjectPath::new("types/broken.d.ts").expect("path")]
            );
            for issue in &facts.issues {
                assert!(!issue.message.starts_with('/'), "{}", issue.message);
                assert!(!issue.message.contains("file://"), "{}", issue.message);
            }
        }
    }

    #[test]
    fn arbitrary_declaration_text_does_not_panic() {
        let alphabet = b"{}[]()<>|&,:;'\"`/*\n abcXYZ0123456789.=-?";
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..64 {
            let mut text = String::new();
            for _ in 0..48 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let index = usize::try_from(seed >> 33).unwrap_or_default() % alphabet.len();
                text.push(char::from(alphabet[index]));
            }
            let facts = facts(&[("types/random.d.ts", text.as_str())]);
            assert_eq!(facts.sources.len(), 1);
            let _ = TypeText::parse(text.as_str()).is_proven();
        }
    }
}
