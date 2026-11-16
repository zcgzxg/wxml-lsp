//! Static WXML template/WXS semantics for hover, definition and completion.
//!
//! Unlike mustache type queries, these features are fully static and never
//! start the type backend:
//!
//! - `<template name>`/`<template is>`/`<import>`/`<include>` definitions and
//!   calls, with imported definitions resolved through the project host;
//! - per-instance `data` object keys and the data identifiers a template body
//!   reads;
//! - `<wxs module>` blocks (inline and local `src`), their exports and
//!   `require("./x.wxs")` edges.
//!
//! Everything is bounded: the module reads at most [`MAX_STATIC_FILES`] files,
//! keeps imports non-transitive (matching WXML scope), and ignores URL or
//! absolute specifiers. Dynamic template names and computed WXS exports are
//! reported by the core analysis (`wxml_types`), never guessed here.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::{Value, json};
use wxml_project::ProjectPath;
use wxml_syntax::expr::{Expr, ExprKind};
use wxml_syntax::{Mustache, SyntaxTree, parse};
use wxml_text::{SourceRange, TextRange};
use wxml_types::{
    StaticImport, StaticImportKind, StaticIssue, StaticIssueKind, TemplateDefinition,
    TemplateSemantics, WxsBlock, WxsMember, WxsSummary, analyze_template_semantics,
    resolve_static_specifier, summarize_wxs,
};

use crate::handlers::{line_index_range_value, source_range_value};
use crate::server::Server;
use crate::workspace::Document;

/// Largest number of files read while resolving one template/WXS query.
pub(crate) const MAX_STATIC_FILES: usize = 32;
/// Largest number of template definitions indexed for one query.
pub(crate) const MAX_INDEXED_TEMPLATES: usize = 512;
/// Largest WXS source accepted for one module summary.
const MAX_WXS_SOURCE_BYTES: usize = 1024 * 1024;
/// Largest number of definition locations returned for one query.
const MAX_DEFINITION_LOCATIONS: usize = 16;
/// Largest documentation chunk copied into a hover.
const MAX_HOVER_DOC_CHARS: usize = 512;

type Access<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// One template definition with the file it lives in.
#[derive(Clone, Debug)]
pub(crate) struct LocatedTemplate {
    /// Project-relative source path.
    pub(crate) path: String,
    /// Definition facts.
    pub(crate) definition: TemplateDefinition,
    /// `true` for a definition in the queried document.
    pub(crate) local: bool,
}

/// One WXS module with its source and static summary.
#[derive(Clone, Debug)]
pub(crate) struct LocatedWxs {
    /// `module` name.
    pub(crate) module: String,
    /// Project-relative source path (`None` for an inline body).
    pub(crate) path: Option<String>,
    /// Bounded export summary.
    pub(crate) summary: WxsSummary,
    /// Offset of the summarized text in the current WXML document; `0` for a
    /// `src` file and the body start for an inline block.
    pub(crate) offset_base: u32,
}

/// The static index for one query.
#[derive(Default)]
struct StaticIndex {
    semantics: TemplateSemantics,
    templates: Vec<LocatedTemplate>,
    wxs: Vec<LocatedWxs>,
}

/// A definition/hover navigation target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticPosition {
    /// Project-relative path.
    pub(crate) path: String,
    /// Byte range in that file.
    pub(crate) range: SourceRange,
}

/// A static hover answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticHover {
    /// Markdown content.
    pub(crate) contents: String,
    /// Range of the hovered token in the current document.
    pub(crate) range: Option<SourceRange>,
    /// Navigation targets shown as a "Go to definition" hint.
    pub(crate) positions: Vec<StaticPosition>,
}

/// Completion item kinds produced by the static layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StaticItemKind {
    /// WXS function export.
    Function,
    /// WXS value export.
    Value,
    /// WXS module binding.
    Module,
    /// Template name.
    Template,
    /// Template instance data key.
    Property,
}

/// Static completion items plus coverage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticCompletionList {
    /// Items in display order.
    pub(crate) items: Vec<StaticCompletion>,
    /// `true` when a WXS/template budget or dynamic summary may hide items.
    pub(crate) incomplete: bool,
}

/// One static completion item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticCompletion {
    /// Display label.
    pub(crate) label: String,
    /// Short detail line.
    pub(crate) detail: String,
    /// Optional documentation.
    pub(crate) documentation: Option<String>,
    /// Byte range replaced by the edit.
    pub(crate) replace: SourceRange,
    /// Item kind.
    pub(crate) kind: StaticItemKind,
}

/// Builds the static index for a document.
fn build_index(path: &str, text: &str, access: &Access<'_>) -> StaticIndex {
    let mut index = StaticIndex::default();
    let tree = parse(text).tree;
    index.semantics = analyze_template_semantics(&tree);
    for definition in &index.semantics.definitions {
        if index.templates.len() >= MAX_INDEXED_TEMPLATES {
            break;
        }
        index.templates.push(LocatedTemplate {
            path: path.to_owned(),
            definition: definition.clone(),
            local: true,
        });
    }
    let mut visited = HashSet::new();
    visited.insert(path.to_owned());
    let imports = index.semantics.imports.clone();
    collect_imports(path, &imports, access, &mut visited, &mut index);
    // Local definitions shadow imported ones; the first imported definition of
    // a name wins (matching a single `import` namespace, not a merge).
    let local_names: HashSet<String> = index
        .templates
        .iter()
        .filter(|template| template.local)
        .map(|template| template.definition.name.clone())
        .collect();
    let mut seen_imported = HashSet::new();
    index.templates.retain(|template| {
        if template.local {
            return true;
        }
        if local_names.contains(&template.definition.name) {
            return false;
        }
        if !seen_imported.insert(template.definition.name.clone()) {
            return false;
        }
        true
    });
    let blocks = index.semantics.wxs.clone();
    let mut wxs_cache: HashMap<String, Option<String>> = HashMap::new();
    for block in &blocks {
        let resolved_src = block
            .src
            .as_deref()
            .and_then(|src| resolve_static_specifier(path, src));
        if let Some(src) = &resolved_src {
            if !visited.contains(src) && visited.len() >= MAX_STATIC_FILES {
                index.semantics.truncated = true;
                index.semantics.issues.push(StaticIssue {
                    kind: StaticIssueKind::LimitReached,
                    range: block.element_range,
                });
                break;
            }
            visited.insert(src.clone());
        }
        if let Some(wxs) = load_wxs(path, text, block, access, &mut wxs_cache) {
            index.wxs.push(wxs);
        }
    }
    index
}

fn collect_imports(
    path: &str,
    imports: &[StaticImport],
    access: &Access<'_>,
    visited: &mut HashSet<String>,
    index: &mut StaticIndex,
) {
    for import in imports {
        if import.kind != StaticImportKind::Import {
            continue;
        }
        let Some(resolved) = resolve_static_specifier(path, &import.src) else {
            continue;
        };
        if !visited.insert(resolved.clone()) {
            continue;
        }
        if visited.len() > MAX_STATIC_FILES {
            break;
        }
        let Some(text) = access(&resolved) else {
            continue;
        };
        let imported_tree = parse(&text).tree;
        let semantics = analyze_template_semantics(&imported_tree);
        for definition in semantics.definitions {
            if index.templates.len() >= MAX_INDEXED_TEMPLATES {
                return;
            }
            index.templates.push(LocatedTemplate {
                path: resolved.clone(),
                definition,
                local: false,
            });
        }
        // WXML `import` deliberately has no transitive scope: only templates
        // defined in the target file become visible, not the target's imports.
        let _ = &semantics.imports;
    }
}

fn load_wxs(
    current_path: &str,
    current_text: &str,
    block: &WxsBlock,
    access: &Access<'_>,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<LocatedWxs> {
    let module = block.module.clone()?;
    let (path, text, offset_base) = if let Some(src) = &block.src {
        let resolved = resolve_static_specifier(current_path, src)?;
        let text = if let Some(cached) = cache.get(&resolved) {
            cached.clone()?
        } else {
            let read = access(&resolved);
            cache.insert(resolved.clone(), read.clone());
            read?
        };
        (Some(resolved), text, 0)
    } else {
        let range = block.body_range?;
        let start = range.start() as usize;
        let end = range.end() as usize;
        let body = current_text.get(start..end)?;
        (None, body.to_owned(), range.start())
    };
    if text.len() > MAX_WXS_SOURCE_BYTES {
        return None;
    }
    Some(LocatedWxs {
        module,
        path,
        summary: summarize_wxs(&text),
        offset_base,
    })
}

/// Answers a static hover, or `None` when the point is not a static construct.
pub(crate) fn hover(
    path: &str,
    text: &str,
    offset: u32,
    access: &Access<'_>,
) -> Option<StaticHover> {
    if !has_static_constructs(text) {
        return None;
    }
    let index = build_index(path, text, access);
    let tree = parse(text).tree;

    if let Some(hover) = wxs_hover(path, text, &tree, offset, &index, access) {
        return Some(hover);
    }
    template_hover(path, &tree, &index, offset, access)
}

/// Answers a static definition, or `None` when the point is not static.
pub(crate) fn definition(
    path: &str,
    text: &str,
    offset: u32,
    access: &Access<'_>,
) -> Option<Vec<StaticPosition>> {
    if !has_static_constructs(text) {
        return None;
    }
    let index = build_index(path, text, access);
    let tree = parse(text).tree;
    if let Some(positions) = wxs_definition(path, text, &tree, offset, &index, access) {
        return Some(positions);
    }
    template_definition(position_in_tree(path, &tree, &index, offset, access))
}

/// Answers static completion, or `None` when the site is handled elsewhere.
pub(crate) fn completion(
    path: &str,
    text: &str,
    offset: u32,
    access: &Access<'_>,
) -> Option<StaticCompletionList> {
    let template_site = template_attribute_site(text, offset, "is").is_some()
        || template_attribute_site(text, offset, "data").is_some();
    let wxs_member_site = text.contains("<wxs") && unclosed_mustache(text, offset).is_some();
    if !template_site && !wxs_member_site {
        return None;
    }
    let index = build_index(path, text, access);
    let incomplete = index_incomplete(&index);
    let items = if let Some(items) = wxs_member_completion(text, offset, &index) {
        items
    } else if let Some(items) = template_is_completion(text, offset, &index) {
        items
    } else {
        template_data_completion(text, offset, &index)?
    };
    Some(StaticCompletionList { items, incomplete })
}

/// True when a static budget or dynamic export may have hidden facts.
fn index_incomplete(index: &StaticIndex) -> bool {
    index.semantics.truncated
        || index
            .semantics
            .issues
            .iter()
            .any(|issue| issue.kind == StaticIssueKind::LimitReached)
        || index
            .wxs
            .iter()
            .any(|wxs| wxs.summary.truncated || wxs.summary.dynamic.is_some())
}

/// Cheap gate: building the index reads imported files and parses WXS bodies.
fn has_static_constructs(text: &str) -> bool {
    ["<wxs", "<template", "<import", "<include"]
        .iter()
        .any(|marker| text.contains(marker))
}

/* ------------------------------------------------------------------ WXS */

fn wxs_hover(
    path: &str,
    text: &str,
    tree: &SyntaxTree,
    offset: u32,
    index: &StaticIndex,
    access: &Access<'_>,
) -> Option<StaticHover> {
    for block in &index.semantics.wxs {
        let Some(wxs) = index
            .wxs
            .iter()
            .find(|wxs| Some(wxs.module.as_str()) == block.module.as_deref())
        else {
            continue;
        };
        // `module` attribute, inline body and `src` value in the queried file.
        if contains(block.module_range, offset) || contains(block.src_range, offset) {
            return Some(module_hover(wxs, path, block));
        }
        if contains(block.body_range, offset) {
            if let Some(hover) = wxs_body_hover(path, text, wxs, offset, access) {
                return Some(hover);
            }
        }
    }
    // A static `src` whose target is not in the snapshot still explains why
    // nothing was loaded instead of silently falling through.
    for block in &index.semantics.wxs {
        if contains(block.src_range, offset) || contains(block.module_range, offset) {
            let mut contents =
                "WXS module source is not a readable local project file (missing, too large, or not a relative path)"
                    .to_owned();
            if let Some(src) = &block.src {
                let _ = write!(contents, ": `{src}`");
            }
            return Some(StaticHover {
                contents,
                range: block.src_range.or(block.module_range),
                positions: Vec::new(),
            });
        }
    }
    // A WXS module or member used inside a mustache.
    let mustache = enclosing_mustache(tree, offset)?;
    let chain = mustache_chain(mustache, tree.text(), offset)?;
    let wxs = index.wxs.iter().find(|wxs| wxs.module == chain.root)?;
    if chain.member.is_none() {
        return Some(module_hover(wxs, path, first_block(index, &wxs.module)?));
    }
    let member_name = chain.member?;
    let member = declaration_member(&wxs.summary, &member_name)?;
    let positions = vec![defined_position(path, wxs, member.name_range)];
    Some(StaticHover {
        contents: member_markdown(wxs, member, &member_name),
        range: chain.range,
        positions,
    })
}

fn wxs_body_hover(
    path: &str,
    text: &str,
    wxs: &LocatedWxs,
    offset: u32,
    access: &Access<'_>,
) -> Option<StaticHover> {
    // `require("./x.wxs")`: the specifier is a module target, not a member.
    for require in &wxs.summary.requires {
        if contains(Some(shift(wxs, require.specifier_range)), offset) {
            let resolved =
                resolve_static_specifier(wxs.path.as_deref().unwrap_or(path), &require.specifier)
                    .filter(|resolved| access(resolved).is_some());
            let mut contents = format!("module `{}`", require.specifier);
            let mut positions = Vec::new();
            if let Some(resolved) = resolved {
                let _ = write!(contents, "\n\nResolved: `{resolved}`");
                positions.push(StaticPosition {
                    path: resolved,
                    range: zero_range(),
                });
            } else {
                contents.push_str("\n\nNot found in the project snapshot.");
            }
            return Some(StaticHover {
                contents,
                range: Some(shift(wxs, require.specifier_range)),
                positions,
            });
        }
    }
    if let Some(token) = word_at(text, offset) {
        if let Some(member) = wxs
            .summary
            .members
            .iter()
            .find(|member| member.name == token)
        {
            return Some(StaticHover {
                contents: member_markdown(wxs, member, &token),
                range: Some(word_range(text, offset)?),
                positions: vec![defined_position(path, wxs, member.name_range)],
            });
        }
    }
    None
}

fn module_hover(wxs: &LocatedWxs, path: &str, block: &WxsBlock) -> StaticHover {
    let mut contents = format!("WXS module `{}`", wxs.module);
    if let Some(source) = &wxs.path {
        let _ = write!(contents, "\n\nSource: `{source}`");
    } else {
        contents.push_str("\n\nSource: inline block");
    }
    let exports = wxs
        .summary
        .exported()
        .map(|member| format!("`{}`", member.name))
        .collect::<Vec<_>>();
    if !exports.is_empty() {
        let _ = write!(contents, "\n\nExports: {}", exports.join(", "));
    }
    if let Some(dynamic) = &wxs.summary.dynamic {
        let _ = write!(contents, "\n\nPartial: {dynamic}");
    }
    if wxs.summary.truncated {
        contents.push_str("\n\nPartial: WXS summary truncated; later members were skipped.");
    }
    let positions = wxs.path.as_ref().map_or_else(
        || {
            block
                .body_range
                .map(|body| {
                    vec![StaticPosition {
                        path: path.to_owned(),
                        range: body,
                    }]
                })
                .unwrap_or_default()
        },
        |source| {
            vec![StaticPosition {
                path: source.clone(),
                range: zero_range(),
            }]
        },
    );
    StaticHover {
        contents,
        range: block.module_range,
        positions,
    }
}

fn member_markdown(wxs: &LocatedWxs, member: &WxsMember, name: &str) -> String {
    let mut contents = format!(
        "```wxs\n{}.{}: {}\n```",
        wxs.module,
        name,
        wxs.summary.describe(member)
    );
    if !member.doc.is_empty() {
        contents.push_str("\n\n");
        contents.push_str(&crate::handlers::markdown_escape(
            &member
                .doc
                .chars()
                .take(MAX_HOVER_DOC_CHARS)
                .collect::<String>(),
        ));
    }
    if member.kind == wxml_types::WxsMemberKind::Module {
        if let Some(specifier) = &member.module_specifier {
            let _ = write!(contents, "\n\nRequires `{specifier}`");
        }
    }
    if wxs.summary.truncated {
        contents.push_str("\n\nPartial: WXS summary truncated; later members were skipped.");
    }
    contents
}

fn wxs_definition(
    path: &str,
    text: &str,
    tree: &SyntaxTree,
    offset: u32,
    index: &StaticIndex,
    access: &Access<'_>,
) -> Option<Vec<StaticPosition>> {
    for block in &index.semantics.wxs {
        let Some(wxs) = index
            .wxs
            .iter()
            .find(|wxs| Some(wxs.module.as_str()) == block.module.as_deref())
        else {
            continue;
        };
        if contains(block.module_range, offset) || contains(block.src_range, offset) {
            if let Some(src) = &wxs.path {
                return Some(vec![StaticPosition {
                    path: src.clone(),
                    range: zero_range(),
                }]);
            }
            if let Some(body) = block.body_range {
                return Some(vec![StaticPosition {
                    path: path.to_owned(),
                    range: body,
                }]);
            }
        }
        if contains(block.body_range, offset) {
            for require in &wxs.summary.requires {
                if contains(Some(shift(wxs, require.specifier_range)), offset) {
                    if let Some(resolved) = resolve_static_specifier(
                        wxs.path.as_deref().unwrap_or(path),
                        &require.specifier,
                    )
                    .filter(|resolved| access(resolved).is_some())
                    {
                        return Some(vec![StaticPosition {
                            path: resolved,
                            range: zero_range(),
                        }]);
                    }
                }
            }
            if let Some(token) = word_at(text, offset) {
                if let Some(member) = wxs
                    .summary
                    .members
                    .iter()
                    .find(|member| member.name == token)
                {
                    return Some(vec![defined_position(path, wxs, member.name_range)]);
                }
            }
        }
    }
    let mustache = enclosing_mustache(tree, offset)?;
    let chain = mustache_chain(mustache, tree.text(), offset)?;
    let wxs = index.wxs.iter().find(|wxs| wxs.module == chain.root)?;
    if chain.member.is_none() {
        if let Some(src) = &wxs.path {
            return Some(vec![StaticPosition {
                path: src.clone(),
                range: zero_range(),
            }]);
        }
        return first_block(index, &wxs.module).and_then(|block| {
            block.body_range.map(|body| {
                vec![StaticPosition {
                    path: path.to_owned(),
                    range: body,
                }]
            })
        });
    }
    let member = declaration_member(&wxs.summary, &chain.member?)?;
    Some(vec![defined_position(path, wxs, member.name_range)])
}

/// Definition target for a WXS member, translating inline body offsets.
fn defined_position(current_path: &str, wxs: &LocatedWxs, range: SourceRange) -> StaticPosition {
    StaticPosition {
        path: wxs.path.clone().unwrap_or_else(|| current_path.to_owned()),
        range: shift(wxs, range),
    }
}

fn shift(wxs: &LocatedWxs, range: SourceRange) -> SourceRange {
    if wxs.offset_base == 0 {
        return range;
    }
    SourceRange::from_bounds(
        range.start().saturating_add(wxs.offset_base),
        range.end().saturating_add(wxs.offset_base),
    )
    .unwrap_or(range)
}

/// Definition target prefers the original declaration over an export alias.
fn declaration_member<'a>(summary: &'a WxsSummary, name: &str) -> Option<&'a WxsMember> {
    summary
        .members
        .iter()
        .find(|member| member.name == name && !member.exported)
        .or_else(|| summary.member(name))
}

fn first_block<'a>(index: &'a StaticIndex, module: &str) -> Option<&'a WxsBlock> {
    index
        .semantics
        .wxs
        .iter()
        .find(|block| block.module.as_deref() == Some(module))
}

fn wxs_member_completion(
    text: &str,
    offset: u32,
    index: &StaticIndex,
) -> Option<Vec<StaticCompletion>> {
    let (inner_start, expr) = unclosed_mustache(text, offset)?;
    let quote_count = expr
        .chars()
        .filter(|character| matches!(character, '\'' | '"' | '`'))
        .count();
    if quote_count % 2 == 1 {
        return None;
    }
    let prefix_len = expr
        .chars()
        .rev()
        .take_while(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        .map(char::len_utf8)
        .sum::<usize>();
    let prefix = &expr[expr.len().saturating_sub(prefix_len)..];
    let before = &expr[..expr.len().saturating_sub(prefix_len)];
    let replace = SourceRange::from_bounds(
        inner_start + u32::try_from(expr.len().saturating_sub(prefix_len)).ok()?,
        inner_start + u32::try_from(expr.len()).ok()?,
    )
    .ok()?;

    if let Some(dot) = before
        .strip_suffix("?.")
        .or_else(|| before.strip_suffix('.'))
    {
        let root = dot
            .trim_end()
            .chars()
            .rev()
            .take_while(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        if root.is_empty() {
            return None;
        }
        let wxs = index.wxs.iter().find(|wxs| wxs.module == root)?;
        let mut items: Vec<StaticCompletion> = wxs
            .summary
            .exported()
            .filter(|member| member.name.starts_with(prefix))
            .map(|member| StaticCompletion {
                label: member.name.clone(),
                detail: wxs.summary.describe(member),
                documentation: (!member.doc.is_empty()).then(|| member.doc.clone()),
                replace,
                kind: match member.kind {
                    wxml_types::WxsMemberKind::Function => StaticItemKind::Function,
                    wxml_types::WxsMemberKind::Module => StaticItemKind::Module,
                    _ => StaticItemKind::Value,
                },
            })
            .collect();
        items.sort_by(|left, right| left.label.cmp(&right.label));
        return Some(items);
    }

    // Module-name completion at the start of an expression.
    if before.is_empty() {
        let items: Vec<StaticCompletion> = index
            .wxs
            .iter()
            .filter(|wxs| wxs.module.starts_with(prefix))
            .map(|wxs| StaticCompletion {
                label: wxs.module.clone(),
                detail: "WXS module".to_owned(),
                documentation: wxs.summary.dynamic.clone(),
                replace,
                kind: StaticItemKind::Module,
            })
            .collect();
        if !items.is_empty() {
            return Some(items);
        }
    }
    None
}

/* ------------------------------------------------------------- templates */

#[allow(clippy::too_many_lines)]
fn template_hover(
    path: &str,
    tree: &SyntaxTree,
    index: &StaticIndex,
    offset: u32,
    access: &Access<'_>,
) -> Option<StaticHover> {
    let semantics = &index.semantics;
    let partial = template_partial_note(index);
    for call in &semantics.calls {
        if contains(Some(call.name_range), offset) {
            let definitions = templates_named(index, &call.name);
            let mut contents = format!("template `{}`", call.name);
            if let Some(data) = &call.data {
                if !data.keys.is_empty() {
                    let _ = write!(
                        contents,
                        "\n\nInstance data: {}",
                        data.keys
                            .iter()
                            .map(|key| format!("`{}`", key.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            if let Some(partial) = &partial {
                let _ = write!(contents, "\n\nPartial: {partial}");
            }
            if definitions.is_empty() {
                contents.push_str(
                    "\n\nNo static definition found in this template or its local imports.",
                );
            } else if definitions.len() > 1 {
                let _ = write!(
                    contents,
                    "\n\n{} definitions found; the first is used.",
                    definitions.len()
                );
            }
            return Some(StaticHover {
                contents,
                range: Some(call.name_range),
                positions: definitions
                    .iter()
                    .map(|template| StaticPosition {
                        path: template.path.clone(),
                        range: template.definition.name_range,
                    })
                    .collect(),
            });
        }
        if let Some(data) = &call.data {
            for key in &data.keys {
                if contains(Some(key.range), offset) {
                    let uses = templates_named(index, &call.name)
                        .iter()
                        .flat_map(|template| {
                            template
                                .definition
                                .data_uses
                                .iter()
                                .filter(|use_| use_.name == key.name)
                                .map(|use_| StaticPosition {
                                    path: template.path.clone(),
                                    range: use_.range,
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let mut contents =
                        format!("data key `{}` for template `{}`", key.name, call.name);
                    if let Some(partial) = &partial {
                        let _ = write!(contents, "\n\nPartial: {partial}");
                    }
                    return Some(StaticHover {
                        contents,
                        range: Some(key.range),
                        positions: uses,
                    });
                }
            }
        }
    }
    for definition in &semantics.definitions {
        if contains(Some(definition.name_range), offset) {
            let mut contents = format!("template definition `{}`", definition.name);
            if !definition.data_uses.is_empty() {
                let _ = write!(
                    contents,
                    "\n\nReads: {}",
                    definition
                        .data_uses
                        .iter()
                        .map(|use_| use_.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if let Some(incomplete) = &definition.incomplete {
                let _ = write!(contents, "\n\nPartial: {incomplete}");
            }
            if let Some(partial) = &partial {
                let _ = write!(contents, "\n\nPartial: {partial}");
            }
            return Some(StaticHover {
                contents,
                range: Some(definition.name_range),
                positions: vec![StaticPosition {
                    path: path.to_owned(),
                    range: definition.name_range,
                }],
            });
        }
    }
    for import in &semantics.imports {
        if contains(Some(import.src_range), offset) {
            let kind = import.kind.as_str();
            let resolved = resolve_static_specifier(path, &import.src)
                .filter(|resolved| access(resolved).is_some());
            let mut contents = format!("{kind} source `{}`", import.src);
            if let Some(resolved) = &resolved {
                let _ = write!(contents, "\n\nResolved: `{resolved}`");
            } else {
                contents.push_str(
                    "\n\nNot a local relative path in the project snapshot; not resolved.",
                );
            }
            if let Some(partial) = &partial {
                let _ = write!(contents, "\n\nPartial: {partial}");
            }
            return Some(StaticHover {
                contents,
                range: Some(import.src_range),
                positions: resolved
                    .into_iter()
                    .map(|resolved| StaticPosition {
                        path: resolved,
                        range: zero_range(),
                    })
                    .collect(),
            });
        }
    }
    let _ = tree;
    None
}

fn template_definition(positions: Option<Vec<StaticPosition>>) -> Option<Vec<StaticPosition>> {
    positions.filter(|positions| !positions.is_empty())
}

fn position_in_tree(
    path: &str,
    tree: &SyntaxTree,
    index: &StaticIndex,
    offset: u32,
    access: &Access<'_>,
) -> Option<Vec<StaticPosition>> {
    let semantics = &index.semantics;
    for call in &semantics.calls {
        if contains(Some(call.name_range), offset) {
            let positions: Vec<StaticPosition> = templates_named(index, &call.name)
                .iter()
                .map(|template| StaticPosition {
                    path: template.path.clone(),
                    range: template.definition.name_range,
                })
                .take(MAX_DEFINITION_LOCATIONS)
                .collect();
            return Some(positions);
        }
        if let Some(data) = &call.data {
            for key in &data.keys {
                if contains(Some(key.range), offset) {
                    let positions = templates_named(index, &call.name)
                        .iter()
                        .flat_map(|template| {
                            template
                                .definition
                                .data_uses
                                .iter()
                                .filter(|use_| use_.name == key.name)
                                .map(|use_| StaticPosition {
                                    path: template.path.clone(),
                                    range: use_.range,
                                })
                                .take(MAX_DEFINITION_LOCATIONS)
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    return Some(positions);
                }
            }
        }
    }
    for definition in &semantics.definitions {
        if contains(Some(definition.name_range), offset) {
            return Some(vec![StaticPosition {
                path: path.to_owned(),
                range: definition.name_range,
            }]);
        }
    }
    for import in &semantics.imports {
        if contains(Some(import.src_range), offset) {
            let resolved = resolve_static_specifier(path, &import.src)
                .filter(|resolved| access(resolved).is_some())?;
            return Some(vec![StaticPosition {
                path: resolved,
                range: zero_range(),
            }]);
        }
    }
    let _ = tree;
    None
}

fn template_partial_note(index: &StaticIndex) -> Option<String> {
    if index.semantics.truncated {
        return Some("template/WXS budget reached; later facts were skipped".to_owned());
    }
    if index
        .semantics
        .issues
        .iter()
        .any(|issue| issue.kind == StaticIssueKind::LimitReached)
    {
        return Some("template/WXS budget reached; later facts were skipped".to_owned());
    }
    None
}

fn templates_named<'a>(index: &'a StaticIndex, name: &str) -> Vec<&'a LocatedTemplate> {
    let mut seen = HashSet::new();
    index
        .templates
        .iter()
        .filter(|template| template.definition.name == name && seen.insert(template.path.clone()))
        .collect()
}

fn template_is_completion(
    text: &str,
    offset: u32,
    index: &StaticIndex,
) -> Option<Vec<StaticCompletion>> {
    let site = template_attribute_site(text, offset, "is")?;
    let offset = usize::try_from(offset).ok()?;
    if text
        .get(site.value_range.start() as usize..offset)
        .is_some_and(|value| value.contains("{{"))
    {
        return None;
    }
    let names: BTreeSet<&str> = index
        .templates
        .iter()
        .map(|template| template.definition.name.as_str())
        .collect();
    let items: Vec<StaticCompletion> = names
        .into_iter()
        .filter(|name| name.starts_with(&site.prefix))
        .map(|name| StaticCompletion {
            label: name.to_owned(),
            detail: "template".to_owned(),
            documentation: index
                .templates
                .iter()
                .find(|template| template.definition.name == name && !template.local)
                .map(|template| format!("imported from `{}`", template.path)),
            replace: site.replace,
            kind: StaticItemKind::Template,
        })
        .collect();
    Some(items)
}

fn template_data_completion(
    text: &str,
    offset: u32,
    index: &StaticIndex,
) -> Option<Vec<StaticCompletion>> {
    let site = template_attribute_site(text, offset, "data")?;
    let call = index
        .semantics
        .calls
        .iter()
        .find(|_call| contains_any(site.value_range, offset));
    // Incomplete `data="{{ { ...` may not have a parsed call yet; the `is`
    // attribute on the same tag still names the template.
    let name = call
        .map(|call| call.name.clone())
        .or_else(|| template_is_in_tag(text, offset))?;
    let template = templates_named(index, &name).first().copied()?;
    let mut present: HashSet<String> = call
        .and_then(|call| call.data.as_ref())
        .map(|data| data.keys.iter().map(|key| key.name.clone()).collect())
        .unwrap_or_default();
    // Keys already typed before the cursor in an unparsed object literal.
    let value_text =
        &text[site.value_range.start() as usize..(site.value_range.end() as usize).min(text.len())];
    for key in object_keys_in(value_text) {
        present.insert(key);
    }
    let items: Vec<StaticCompletion> = template
        .definition
        .data_uses
        .iter()
        .filter(|use_| !present.contains(&use_.name))
        .filter(|use_| use_.name.starts_with(&site.prefix))
        .map(|use_| StaticCompletion {
            label: use_.name.clone(),
            detail: format!("data key for template `{name}`"),
            documentation: None,
            replace: site.replace,
            kind: StaticItemKind::Property,
        })
        .collect();
    Some(items)
}

/// Reads a static `is="..."` value from the template tag before `offset`.
fn template_is_in_tag(text: &str, offset: u32) -> Option<String> {
    let offset = usize::try_from(offset).ok()?;
    let before = text.get(..offset)?;
    let tag_start = before.rfind('<')?;
    // Look at the whole start tag: `data` may precede `is` in the source.
    let tag_end = text[tag_start..]
        .find('>')
        .map_or(text.len(), |end| tag_start + end);
    let tag = text.get(tag_start..tag_end)?;
    let needle = "is=";
    let mut search_end = tag.len();
    while let Some(index) = tag[..search_end].rfind(needle) {
        let after = index + needle.len();
        let quote = tag[after..].chars().next()?;
        if quote == '"' || quote == '\'' {
            let start = after + 1;
            let end = tag[start..]
                .find(quote)
                .map_or(tag.len(), |end| start + end);
            let value = tag.get(start..end.min(tag.len()))?.trim();
            if !value.is_empty() && !value.contains("{{") {
                return Some(value.to_owned());
            }
        }
        search_end = index;
    }
    None
}

/// One attribute value site detected from raw text.
struct AttributeSite {
    prefix: String,
    replace: SourceRange,
    value_range: SourceRange,
}

/// Detects a cursor inside `<template ... attr="...">` value.
fn template_attribute_site(text: &str, offset: u32, attribute: &str) -> Option<AttributeSite> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let before = &text[..offset];
    let tag_start = before.rfind('<')?;
    let tag = text.get(tag_start..offset)?;
    if tag.starts_with("</") || tag.starts_with("<!") {
        return None;
    }
    let mut characters = tag.chars();
    if characters.next()? != '<' {
        return None;
    }
    let name: String = characters
        .take_while(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | ':'))
        .collect();
    if name != "template" {
        return None;
    }
    if has_unquoted_tag_end(tag) {
        return None;
    }
    let needle = format!("{attribute}=");
    let relative_position = offset - tag_start;
    // Find the last occurrence of the attribute before the cursor that is
    // followed by a quote and has no other attribute boundary between it and
    // the cursor.
    let mut search_end = relative_position;
    let mut value_start = None;
    while let Some(index) = tag[..search_end].rfind(&needle) {
        let after = index + needle.len();
        let Some(quote) = tag[after..].chars().next() else {
            break;
        };
        if quote == '"' || quote == '\'' {
            let value_open = after + 1;
            let value_close = tag[value_open..]
                .find(quote)
                .map_or(tag.len(), |close| value_open + close);
            if relative_position <= value_close {
                value_start = Some((value_open, value_close));
                break;
            }
        }
        search_end = index;
    }
    let (value_open, value_close) = value_start?;
    let prefix_text = &tag[value_open..relative_position];
    if prefix_text.contains('"') || prefix_text.contains('\'') || prefix_text.contains('>') {
        return None;
    }
    let prefix_len = prefix_text
        .chars()
        .rev()
        .take_while(|character| character.is_alphanumeric() || matches!(character, '_' | '$' | '-'))
        .map(char::len_utf8)
        .sum::<usize>();
    let prefix_start = relative_position - prefix_len;
    let value_range = SourceRange::from_bounds(
        u32::try_from(tag_start + value_open).ok()?,
        u32::try_from(tag_start + value_close.min(relative_position)).ok()?,
    )
    .ok()?;
    let replace = SourceRange::from_bounds(
        u32::try_from(tag_start + prefix_start).ok()?,
        u32::try_from(tag_start + relative_position).ok()?,
    )
    .ok()?;
    Some(AttributeSite {
        prefix: tag[prefix_start..relative_position].to_owned(),
        replace,
        value_range,
    })
}

fn has_unquoted_tag_end(tag: &str) -> bool {
    let mut quote = None;
    for character in tag.chars() {
        match character {
            '"' | '\'' if quote.is_none() => quote = Some(character),
            character if Some(character) == quote => quote = None,
            '>' if quote.is_none() => return true,
            _ => {}
        }
    }
    false
}

/* -------------------------------------------------------------- mustaches */

/// Finds the innermost closed mustache containing `offset`.
fn enclosing_mustache(tree: &SyntaxTree, offset: u32) -> Option<Mustache<'_>> {
    tree.document()
        .mustaches()
        .filter(|mustache| contains(Some(SourceRange::new(mustache.text_range())), offset))
        .min_by_key(|mustache| mustache.text_range().len())
}

/// Root identifier and optional first member of the expression at `offset`.
struct ExpressionChain {
    root: String,
    member: Option<String>,
    range: Option<SourceRange>,
}

fn mustache_chain(mustache: Mustache<'_>, text: &str, offset: u32) -> Option<ExpressionChain> {
    let expr = mustache.expr()?;
    let inner = mustache.inner_range()?;
    let base = inner.start();
    let relative = offset.checked_sub(base)?;
    let token_range = identifier_at(text, base, relative)?;
    let token = text
        .get(token_range.start() as usize..token_range.end() as usize)?
        .to_owned();
    // Walk the expression to find the identifier node at this offset and its
    // chain root/member relationship.
    let (root, member) = chain_at(expr, text, token_range, &token)?;
    Some(ExpressionChain {
        root,
        member,
        range: Some(SourceRange::new(token_range)),
    })
}

fn chain_at(
    expr: &Expr,
    text: &str,
    token: TextRange,
    token_text: &str,
) -> Option<(String, Option<String>)> {
    match expr.kind() {
        ExprKind::Member {
            object, property, ..
        } => {
            if expr.range() == token {
                // The cursor is on the whole chain.
            }
            if property.range() == token {
                if let ExprKind::Identifier { .. } = property.kind() {
                    let root = root_identifier(object, text)?;
                    return Some((root, Some(token_text.to_owned())));
                }
            }
            chain_at(object, text, token, token_text)
        }
        ExprKind::Index { object, .. } => chain_at(object, text, token, token_text),
        ExprKind::Call { callee, .. } => chain_at(callee, text, token, token_text),
        ExprKind::Parenthesized { expr } => chain_at(expr, text, token, token_text),
        ExprKind::Identifier { .. } if expr.range() == token => Some((token_text.to_owned(), None)),
        _ => None,
    }
}

fn root_identifier(expr: &Expr, text: &str) -> Option<String> {
    match expr.kind() {
        ExprKind::Identifier { name } => text
            .get(name.start() as usize..name.end() as usize)
            .map(str::to_owned),
        ExprKind::Member { object, .. } | ExprKind::Index { object, .. } => {
            root_identifier(object, text)
        }
        ExprKind::Call { callee, .. } => root_identifier(callee, text),
        ExprKind::Parenthesized { expr } => root_identifier(expr, text),
        _ => None,
    }
}

/* --------------------------------------------------------------- helpers */

/// Keys (`name:`) present in an unparsed/partial object literal.
fn object_keys_in(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut quote: Option<char> = None;
    for (index, character) in text.char_indices() {
        match character {
            '\'' | '"' | '`' if quote.is_none() => quote = Some(character),
            character if Some(character) == quote => quote = None,
            ':' if quote.is_none() => {
                let prefix = &text[..index];
                let start = prefix
                    .char_indices()
                    .rev()
                    .take_while(|(_, character)| {
                        character.is_alphanumeric() || matches!(character, '_' | '$')
                    })
                    .last()
                    .map_or(prefix.len(), |(start, _)| start);
                let key = prefix[start..].trim();
                if !key.is_empty() {
                    keys.push(key.to_owned());
                }
            }
            _ => {}
        }
    }
    keys
}

fn contains(range: Option<SourceRange>, offset: u32) -> bool {
    range.is_some_and(|range| offset >= range.start() && offset <= range.end())
}

const fn contains_any(range: SourceRange, offset: u32) -> bool {
    offset >= range.start() && offset <= range.end()
}

fn zero_range() -> SourceRange {
    SourceRange::new(TextRange::new(0, 0).unwrap_or_else(|_| unreachable!()))
}

/// Returns the identifier containing `offset` within `text`.
fn word_at(text: &str, offset: u32) -> Option<String> {
    let range = word_range(text, offset)?;
    Some(
        text.get(range.start() as usize..range.end() as usize)?
            .to_owned(),
    )
}

fn word_range(text: &str, offset: u32) -> Option<SourceRange> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$');
    let mut start = offset;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = offset;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    SourceRange::from_bounds(u32::try_from(start).ok()?, u32::try_from(end).ok()?).ok()
}

fn identifier_at(text: &str, base: u32, relative: u32) -> Option<TextRange> {
    let range = word_range(text, base.checked_add(relative)?)?;
    Some(range.range())
}

/// Finds the unclosed `{{` before `offset`; returns the inner start and text.
fn unclosed_mustache(text: &str, offset: u32) -> Option<(u32, String)> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let before = &text[..offset];
    let open = before.rfind("{{")?;
    if let Some(close) = before[open + 2..].rfind("}}") {
        if open + 2 + close + 2 > before.len() {
            return None;
        }
        if open + 2 + close + 2 <= offset {
            return None;
        }
    }
    let inner = &text[open + 2..offset];
    Some((u32::try_from(open + 2).ok()?, inner.to_owned()))
}

/* ------------------------------------------------------------ LSP output */

/// Maps a static hover to the LSP hover shape.
pub(crate) fn hover_value(document: &Document, hover: &StaticHover) -> Value {
    let mut contents = hover.contents.clone();
    if let Some(definition) = hover.positions.first() {
        let _ = write!(contents, "\n\n---\n\nDefinition: `{}`", definition.path);
    }
    let mut value = json!({
        "contents": {
            "kind": "markdown",
            "value": contents,
        },
    });
    if let Some(range) = hover
        .range
        .and_then(|range| source_range_value(document, range))
    {
        if let Some(object) = value.as_object_mut() {
            object.insert("range".to_owned(), range);
        }
    }
    value
}

/// Maps static definition positions to LSP locations.
pub(crate) fn definition_value(
    server: &Server,
    document: &Document,
    positions: &[StaticPosition],
) -> Vec<Value> {
    let mut locations = Vec::new();
    let mut indexes: HashMap<String, Option<wxml_text::LineIndex>> = HashMap::new();
    let current = document.file().path().to_owned();
    for position in positions.iter().take(MAX_DEFINITION_LOCATIONS) {
        if position.path == current {
            if let Some(range) = source_range_value(document, position.range) {
                locations.push(json!({"uri": document.uri(), "range": range}));
            }
            continue;
        }
        let Some(path) = ProjectPath::new(position.path.as_str()).ok() else {
            continue;
        };
        let Some(uri) = server
            .host
            .file_id(&path)
            .ok()
            .and_then(|file| server.host.uri_for(&file))
        else {
            continue;
        };
        if !indexes.contains_key(&position.path) {
            let text = server
                .host
                .file_id(&path)
                .ok()
                .and_then(|file| server.host.file_text(&file));
            indexes.insert(
                position.path.clone(),
                text.map(|text| wxml_text::LineIndex::new(text.to_string())),
            );
        }
        let Some(Some(index)) = indexes.get(&position.path) else {
            continue;
        };
        if let Some(range) = line_index_range_value(index, position.range) {
            locations.push(json!({"uri": uri, "range": range}));
        }
    }
    locations
}

/// Maps static completion items to LSP `CompletionItem` values.
pub(crate) fn completion_values(document: &Document, result: &StaticCompletionList) -> Value {
    let values: Vec<Value> = result
        .items
        .iter()
        .filter_map(|item| {
            let range = source_range_value(document, item.replace)?;
            let kind = match item.kind {
                StaticItemKind::Function => 3,
                StaticItemKind::Value => 12,
                StaticItemKind::Module => 9,
                StaticItemKind::Template => 7,
                StaticItemKind::Property => 10,
            };
            let mut value = json!({
                "label": item.label,
                "kind": kind,
                "detail": item.detail,
                "textEdit": {"range": range, "newText": item.label},
            });
            if let Some(documentation) = &item.documentation {
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "documentation".to_owned(),
                        json!({"kind": "plaintext", "value": documentation}),
                    );
                }
            }
            Some(value)
        })
        .collect();
    json!({"isIncomplete": result.incomplete, "items": values})
}

/// Reads the text of a project-relative path through the LSP workspace first
/// (open/unsaved buffers, including `.wxs`), then the project host.
pub(crate) fn host_access(server: &Server) -> impl Fn(&str) -> Option<String> + '_ {
    move |path: &str| {
        let path = ProjectPath::new(path).ok()?;
        let file = server.host.file_id(&path).ok()?;
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wxs"))
            && server
                .host
                .file_size(&file)
                .is_some_and(|size| size > u64::try_from(MAX_WXS_SOURCE_BYTES).unwrap_or(u64::MAX))
        {
            return None;
        }
        if let Some(document) = server.workspace.get_by_file(&file) {
            return Some(document.snapshot().text().to_owned());
        }
        if let Some(uri) = server.host.uri_for(&file) {
            if let Some(document) = server.workspace.get(&uri) {
                return Some(document.snapshot().text().to_owned());
            }
        }
        server.host.file_text(&file).map(|text| text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_access(_: &str) -> Option<String> {
        None
    }

    fn access_map<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |path: &str| {
            entries
                .iter()
                .find(|(candidate, _)| *candidate == path)
                .map(|(_, text)| (*text).to_owned())
        }
    }

    fn slice(text: &str, range: SourceRange) -> &str {
        &text[range.start() as usize..range.end() as usize]
    }

    #[test]
    fn wxs_member_hover_and_definition_use_the_export() {
        let text = r#"<wxs module="utils">function format(v) { return "x"; }
module.exports = { format };</wxs>
<view>{{ utils.format(title) }}</view>"#;
        let x = text.find("format(title)").expect("use") + 1;
        let offset = u32::try_from(x).expect("offset");
        let hover = hover("pages/index/index.wxml", text, offset, &no_access).expect("hover");
        assert!(
            hover.contents.contains("utils.format"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("=> string"), "{}", hover.contents);
        let definition =
            definition("pages/index/index.wxml", text, offset, &no_access).expect("definition");
        assert_eq!(definition[0].path, "pages/index/index.wxml");
        let body = definition[0].range;
        let body_text = &text[body.start() as usize..body.end() as usize];
        assert_eq!(body_text, "format");
    }

    #[test]
    fn wxs_src_module_reads_the_project_file() {
        let wxml = r#"<wxs module="ext" src="./ext.wxs" /><view>{{ ext.label }}</view>"#;
        let wxs = "module.exports = { label: \"hi\", count: 2 };";
        let entries = [("pages/index/ext.wxs", wxs)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("label").expect("label")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &access).expect("hover");
        assert!(hover.contents.contains("string"), "{}", hover.contents);
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &access).expect("definition");
        assert_eq!(definition[0].path, "pages/index/ext.wxs");
        assert_eq!(slice(wxs, definition[0].range), "label");
    }

    #[test]
    fn template_is_hover_and_definition_follow_imports() {
        let wxml = r#"<import src="./shared.wxml" /><template is="card" data="{{ { title: pageTitle } }}" />"#;
        let shared = r#"<template name="card"><view>{{ title }}</view></template>"#;
        let entries = [("pages/index/shared.wxml", shared)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("card").expect("card")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &access).expect("hover");
        assert!(
            hover.contents.contains("template `card`"),
            "{}",
            hover.contents
        );
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &access).expect("definition");
        assert_eq!(definition[0].path, "pages/index/shared.wxml");
        assert_eq!(slice(shared, definition[0].range), "card");
    }

    #[test]
    fn template_instance_data_keys_hover_and_complete() {
        let wxml = r#"<template name="card"><view>{{ title }} {{ count }}</view></template><template is="card" data="{{ { ti } }}" /><template is="card" data="{{ { title: pageTitle } }}" />"#;
        let key_offset = u32::try_from(wxml.find("{ ti }").expect("data") + 2).expect("offset");
        let result = completion("pages/index/index.wxml", wxml, key_offset, &no_access)
            .expect("data completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert!(labels.contains(&"title"), "{labels:?}");
        let title_offset =
            u32::try_from(wxml.find("title: pageTitle").expect("use")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, title_offset, &no_access).expect("hover");
        assert!(
            hover.contents.contains("data key `title`"),
            "{}",
            hover.contents
        );
        assert_eq!(hover.positions.len(), 1);
    }

    #[test]
    fn template_is_completion_offers_local_and_imported_names() {
        let wxml = r#"<import src="./shared.wxml" /><template is="c" />"#;
        let shared = r#"<template name="card">x</template><template name="chip">y</template>"#;
        let entries = [("pages/index/shared.wxml", shared)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("\"c\"").expect("is") + 2).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &access).expect("completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["card", "chip"]);
    }

    #[test]
    fn wxs_require_specifier_navigates_to_the_target_file() {
        let wxml = r#"<wxs module="utils">const ext = require("./ext.wxs"); module.exports = { ext };</wxs>"#;
        let entries = [("pages/index/ext.wxs", "module.exports = { x: 1 };")];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("./ext.wxs").expect("specifier")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &access).expect("hover");
        assert!(
            hover.contents.contains("module `./ext.wxs`"),
            "{}",
            hover.contents
        );
        let target =
            definition("pages/index/index.wxml", wxml, offset, &access).expect("definition");
        assert_eq!(target[0].path, "pages/index/ext.wxs");
        assert_eq!(target[0].range.start(), 0);
        assert!(definition("pages/index/index.wxml", wxml, offset, &no_access).is_none());
    }

    #[test]
    fn include_source_definition_points_at_the_target_file() {
        let wxml = r#"<include src="./parts/body.wxml" />"#;
        let entries = [("pages/index/parts/body.wxml", "<view />")];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("./parts/body.wxml").expect("src")).expect("offset");
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &access).expect("definition");
        assert_eq!(definition[0].path, "pages/index/parts/body.wxml");
    }

    #[test]
    fn local_template_names_shadow_imported_ones() {
        let wxml = r#"<import src="./shared.wxml" /><template name="card">local</template><template is="card" />"#;
        let shared =
            r#"<template name="card">imported</template><template name="chip">x</template>"#;
        let entries = [("pages/index/shared.wxml", shared)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("is=\"card\"").expect("is") + 4).expect("offset");
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &access).expect("definition");
        assert_eq!(definition.len(), 1);
        assert_eq!(definition[0].path, "pages/index/index.wxml");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &access).expect("completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["card", "chip"]);
    }

    #[test]
    fn imports_are_not_transitive() {
        let wxml = r#"<import src="./b.wxml" /><template is="fromC" />"#;
        let b = r#"<import src="./c.wxml" /><template name="fromB">b</template>"#;
        let c = r#"<template name="fromC">c</template>"#;
        let entries = [("pages/index/b.wxml", b), ("pages/index/c.wxml", c)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("is=\"fromC\"").expect("is") + 4).expect("offset");
        assert!(definition("pages/index/index.wxml", wxml, offset, &access).is_none());
        let result =
            completion("pages/index/index.wxml", wxml, offset, &access).expect("completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["fromB"]);
    }

    #[test]
    fn missing_imports_and_wxs_sources_are_reported_without_a_target() {
        let wxml = r#"<import src="./missing.wxml" /><wxs module="m" src="./missing.wxs" />"#;
        let import_offset =
            u32::try_from(wxml.find("./missing.wxml").expect("import")).expect("offset");
        let import_hover =
            hover("pages/index/index.wxml", wxml, import_offset, &no_access).expect("import hover");
        assert!(
            import_hover.contents.contains("not resolved"),
            "{}",
            import_hover.contents
        );
        assert!(definition("pages/index/index.wxml", wxml, import_offset, &no_access).is_none());
        let src_offset = u32::try_from(wxml.find("./missing.wxs").expect("wxs")).expect("offset");
        let src_hover =
            hover("pages/index/index.wxml", wxml, src_offset, &no_access).expect("wxs hover");
        assert!(
            src_hover
                .contents
                .contains("not a readable local project file"),
            "{}",
            src_hover.contents
        );
        assert!(definition("pages/index/index.wxml", wxml, src_offset, &no_access).is_none());
    }

    #[test]
    fn dynamic_wxs_summary_marks_completion_incomplete() {
        let wxml = r#"<wxs module="u">module.exports = { a: 1 }; exports[k] = 2;</wxs><view>{{ u."#;
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("wxs completion");
        assert!(
            result.incomplete,
            "dynamic exports must mark completion incomplete"
        );
        assert!(result.items.iter().any(|item| item.label == "a"));
    }

    #[test]
    fn static_ranges_stay_on_the_original_utf16_surface() {
        let wxml = "😀<template name=\"card\">x</template><template is=\"card\" />";
        let offset = u32::try_from(wxml.find("is=\"card\"").expect("is") + 4).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &no_access).expect("hover");
        let range = hover.range.expect("range");
        assert_eq!(&wxml[range.start() as usize..range.end() as usize], "card");
        assert!(range.start() > 4, "emoji bytes must be preserved");
    }

    #[test]
    fn template_budget_marks_completion_incomplete() {
        let mut wxml = String::new();
        for index in 0..1025 {
            let _ = write!(wxml, "<template is=\"t{index}\" />");
        }
        wxml.push_str("<template is=\"x");
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", &wxml, offset, &no_access).expect("completion");
        assert!(result.incomplete);
    }

    #[test]
    fn optional_chain_wxs_member_completion_works() {
        let wxml = r#"<wxs module="u">module.exports = { label: 1 };</wxs><view>{{ u?."#;
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("completion");
        assert!(result.items.iter().any(|item| item.label == "label"));
    }

    #[test]
    fn dynamic_template_is_is_not_completed_as_a_name() {
        let wxml = r#"<template name="card">x</template><template is="{{ na"#;
        let offset = u32::try_from(wxml.len()).expect("offset");
        assert!(completion("pages/index/index.wxml", wxml, offset, &no_access).is_none());
    }

    #[test]
    fn include_does_not_contribute_template_definitions() {
        let wxml = r#"<include src="./part.wxml" /><template is="inside" />"#;
        let part = r#"<template name="inside">x</template>"#;
        let entries = [("pages/index/part.wxml", part)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("is=\"inside\"").expect("is") + 4).expect("offset");
        assert!(definition("pages/index/index.wxml", wxml, offset, &access).is_none());
        let items =
            completion("pages/index/index.wxml", wxml, offset, &access).expect("completion");
        assert!(items.items.is_empty(), "{items:?}");
    }

    #[test]
    fn dynamic_forms_do_not_invent_static_results() {
        let wxml = r#"<template is="{{ name }}" /><wxs module="m">eval("x")</wxs>"#;
        let offset = u32::try_from(wxml.find("name").expect("name")).expect("offset");
        assert!(hover("pages/index/index.wxml", wxml, offset, &no_access).is_none());
    }
}
