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
//! The module reads at most [`MAX_STATIC_FILES`] files, keeps imports
//! non-transitive (matching WXML scope), and ignores URL or absolute
//! specifiers. Dynamic template names and computed WXS exports are reported by
//! the core analysis (`wxml_types`), never guessed here.

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
/// Largest documentation chunk copied into a hover.
const MAX_HOVER_DOC_CHARS: usize = 512;
/// Largest number of export rows shown in one module hover.
const MAX_HOVER_EXPORTS: usize = 64;

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
    /// Reasons for a configured module whose source could not be loaded.
    wxs_unavailable: HashMap<String, String>,
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
        if index.templates.len() >= crate::project_host::MAX_PROJECT_VIEWS {
            index.semantics.truncated = true;
            index.semantics.issues.push(StaticIssue {
                kind: StaticIssueKind::LimitReached,
                range: definition.element_range,
            });
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
    let mut wxs_budget_reported = false;
    for block in &blocks {
        let resolved_src = block
            .src
            .as_deref()
            .and_then(|src| resolve_static_specifier(path, src));
        if let Some(src) = &resolved_src {
            if !visited.contains(src) && visited.len() >= MAX_STATIC_FILES {
                index.semantics.truncated = true;
                if !wxs_budget_reported {
                    wxs_budget_reported = true;
                    index.semantics.issues.push(StaticIssue {
                        kind: StaticIssueKind::LimitReached,
                        range: block.element_range,
                    });
                }
                if let Some(module) = &block.module {
                    index
                        .wxs_unavailable
                        .insert(module.clone(), "WXS file budget reached".to_owned());
                }
                // Inline modules after the budget do not read files and remain
                // analyzable; only the oversized `src` graph is skipped.
                continue;
            }
            visited.insert(src.clone());
        }
        match load_wxs(path, text, block, access, &mut wxs_cache) {
            Some(wxs) => index.wxs.push(wxs),
            None => {
                if let Some(module) = block.module.clone() {
                    let reason = if block.src.is_some() {
                        "WXS source is missing, too large, or not a readable project file"
                    } else {
                        "WXS inline body is missing"
                    };
                    index.wxs_unavailable.insert(module, reason.to_owned());
                }
            }
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
        if !visited.contains(&resolved) && visited.len() >= MAX_STATIC_FILES {
            index.semantics.truncated = true;
            index.semantics.issues.push(StaticIssue {
                kind: StaticIssueKind::LimitReached,
                range: import.element_range,
            });
            return;
        }
        if !visited.insert(resolved.clone()) {
            continue;
        }
        let Some(text) = access(&resolved) else {
            continue;
        };
        let imported_tree = parse(&text).tree;
        let semantics = analyze_template_semantics(&imported_tree);
        for definition in semantics.definitions {
            if index.templates.len() >= crate::project_host::MAX_PROJECT_VIEWS {
                index.semantics.truncated = true;
                index.semantics.issues.push(StaticIssue {
                    kind: StaticIssueKind::LimitReached,
                    range: import.element_range,
                });
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
        || index.wxs.iter().any(|wxs| wxs.summary.dynamic.is_some())
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
        if contains(block.body_range, offset)
            && let Some(hover) = wxs_body_hover(path, text, wxs, offset, access)
        {
            return Some(hover);
        }
    }
    // A static `src` whose target is not in the snapshot still explains why
    // nothing was loaded instead of silently falling through.
    for block in &index.semantics.wxs {
        if contains(block.src_range, offset) || contains(block.module_range, offset) {
            let reason = block
                .module
                .as_deref()
                .and_then(|module| index.wxs_unavailable.get(module))
                .map_or(
                    "WXS module source is not a readable local project file",
                    String::as_str,
                );
            let mut contents = reason.to_owned();
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
        let mut hover = module_hover(wxs, path, first_block(index, &wxs.module)?);
        // The module attribute hover keeps the attribute range; a mustache root
        // must report the reference the cursor is on.
        hover.range = chain.range;
        return Some(hover);
    }
    let member_name = chain.member?;
    let member = exported_member(&wxs.summary, &member_name)?;
    let positions = vec![defined_position(path, wxs, member.name_range)];
    Some(StaticHover {
        contents: member_markdown(Some(&wxs.module), &wxs.summary, member, &member_name),
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
    if let Some(token) = word_at(text, offset)
        && let Some(member) = wxs
            .summary
            .members
            .iter()
            .find(|member| member.name == token)
    {
        return Some(StaticHover {
            contents: member_markdown(Some(&wxs.module), &wxs.summary, member, &token),
            range: Some(word_range(text, offset)?),
            positions: vec![defined_position(path, wxs, member.name_range)],
        });
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
    let mut exports = String::new();
    let mut listed = 0_usize;
    let mut total = 0_usize;
    for member in wxs.summary.exported() {
        total += 1;
        if listed == MAX_HOVER_EXPORTS {
            continue;
        }
        listed += 1;
        let _ = write!(
            exports,
            "\n- {}: {}",
            inline_code(&member.name),
            inline_code(&wxs.summary.describe(member))
        );
    }
    if total > 0 {
        let _ = write!(contents, "\n\nExports:{exports}");
    }
    // Only statically enumerated exports are listed; dynamic forms stay a
    // `Partial:` note instead of being guessed.
    if total > listed {
        let _ = write!(contents, "\n- … and {} more", total - listed);
    }
    if let Some(dynamic) = &wxs.summary.dynamic {
        let _ = write!(contents, "\n\nPartial: {dynamic}");
    }
    let positions = wxs.path.as_ref().map_or_else(
        || {
            block.body_range.map_or_default(|body| {
                vec![StaticPosition {
                    path: path.to_owned(),
                    range: body,
                }]
            })
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

/// Renders untrusted text as one inline code span, so it cannot break the line.
fn inline_code(text: &str) -> String {
    let safe = crate::handlers::code_text(text, MAX_HOVER_DOC_CHARS)
        .replace(['\n', '\r'], " ")
        .replace('`', "'");
    format!("`{safe}`")
}

fn member_markdown(
    module: Option<&str>,
    summary: &WxsSummary,
    member: &WxsMember,
    name: &str,
) -> String {
    let qualified = module.map_or_else(|| name.to_owned(), |module| format!("{module}.{name}"));
    let mut contents = format!("```wxs\n{qualified}: {}\n```", summary.describe(member));
    if !member.doc.is_empty() {
        contents.push_str("\n\n");
        contents.push_str(&crate::handlers::markdown_doc(
            &member.doc,
            MAX_HOVER_DOC_CHARS,
        ));
    }
    if member.kind == wxml_types::WxsMemberKind::Module
        && let Some(specifier) = &member.module_specifier
    {
        let _ = write!(contents, "\n\nRequires `{specifier}`");
    }
    if !member_is_exported(summary, member) {
        contents.push_str("\n\nLocal declaration; not exported from this module.");
    }
    contents
}

/// True when `member` reaches templates, either directly or through an export
/// that aliases its declaration.
fn member_is_exported(summary: &WxsSummary, member: &WxsMember) -> bool {
    member.exported
        || summary.exported().any(|other| {
            other.name == member.name
                || other
                    .alias_of
                    .is_some_and(|range| contains_any(range, member.name_range.start()))
        })
}

/// Answers a hover inside an open `.wxs` source document.
///
/// The hover covers the word under the cursor and shows the declaration that
/// owns it: the declaration name, a `module.exports` key, or an in-file
/// reference. Positions stay empty because a WXS hover would otherwise point
/// at the very declaration it already describes.
pub(crate) fn wxs_source_hover(text: &str, offset: u32) -> Option<StaticHover> {
    let range = word_range(text, offset)?;
    let token = text.get(range.start() as usize..range.end() as usize)?;
    let summary = summarize_wxs(text);
    // A declaration or export key under the cursor wins over a name lookup:
    // `member()` prefers the exported entry, whose range is the `module.exports`
    // key rather than the declaration the user is hovering.
    let member = summary
        .members
        .iter()
        .find(|member| contains_any(member.name_range, offset))
        .or_else(|| summary.member(token))?;
    Some(StaticHover {
        contents: member_markdown(None, &summary, member, &member.name),
        range: Some(range),
        positions: Vec::new(),
    })
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
                if contains(Some(shift(wxs, require.specifier_range)), offset)
                    && let Some(resolved) = resolve_static_specifier(
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
            if let Some(token) = word_at(text, offset)
                && let Some(member) = wxs
                    .summary
                    .members
                    .iter()
                    .find(|member| member.name == token)
            {
                return Some(vec![defined_position(path, wxs, member.name_range)]);
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
    let member = exported_member(&wxs.summary, &chain.member?)?;
    Some(vec![defined_position(
        path,
        wxs,
        member.alias_of.unwrap_or(member.name_range),
    )])
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

/// Runtime-visible member: only `module.exports` entries can be accessed.
fn exported_member<'a>(summary: &'a WxsSummary, name: &str) -> Option<&'a WxsMember> {
    summary.exported().find(|member| member.name == name)
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

    let dot_source = before.trim_end();
    if let Some(dot) = dot_source
        .strip_suffix("?.")
        .or_else(|| dot_source.strip_suffix('.'))
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
            .filter(|member| is_dot_member_name(&member.name) && member.name.starts_with(prefix))
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
    if before.trim().is_empty() {
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
            if let Some(data) = &call.data
                && !data.keys.is_empty()
            {
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
                .take(crate::workspace::MAX_DEFINITION_TARGETS)
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
                                .take(crate::workspace::MAX_DEFINITION_TARGETS)
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

/// A dot member must be a legal identifier in the current expression grammar.
fn is_dot_member_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        && !matches!(
            name,
            "true"
                | "false"
                | "null"
                | "undefined"
                | "this"
                | "super"
                | "default"
                | "class"
                | "function"
                | "return"
                | "new"
                | "delete"
                | "typeof"
                | "void"
                | "in"
                | "instanceof"
                | "var"
                | "let"
                | "const"
                | "if"
                | "else"
                | "for"
                | "while"
                | "do"
                | "switch"
                | "case"
                | "break"
                | "continue"
                | "try"
                | "catch"
                | "finally"
                | "throw"
                | "with"
                | "yield"
                | "await"
                | "import"
                | "export"
                | "extends"
        )
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
        .map_or_default(|data| data.keys.iter().map(|key| key.name.clone()).collect());
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
            if property.range() == token
                && let ExprKind::Identifier { .. } = property.kind()
            {
                let root = root_identifier(object, text)?;
                return Some((root, Some(token_text.to_owned())));
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
        && let Some(object) = value.as_object_mut()
    {
        object.insert("range".to_owned(), range);
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
    for position in positions
        .iter()
        .take(crate::workspace::MAX_DEFINITION_TARGETS)
    {
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
            if let Some(documentation) = &item.documentation
                && let Some(object) = value.as_object_mut()
            {
                object.insert(
                    "documentation".to_owned(),
                    json!({"kind": "plaintext", "value": documentation}),
                );
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
        if let Some(document) = server.workspace.get_by_file(&file) {
            return Some(document.snapshot().text().to_owned());
        }
        if let Some(uri) = server.host.uri_for(&file)
            && let Some(document) = server.workspace.get(&uri)
        {
            return Some(document.snapshot().text().to_owned());
        }
        server.host.file_text(&file).map(|text| text.to_string())
    }
}

/// Project-bound variant of [`host_access`] for multi-app WXS resolution.
pub(crate) fn host_access_in<'a>(
    server: &'a Server,
    project: &'a wxml_types::ProjectId,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |path: &str| {
        let path = ProjectPath::new(path).ok()?;
        let file = server.host.file_id_in(project, &path).ok()?;
        if let Some(document) = server.workspace.get_by_file(&file) {
            return Some(document.snapshot().text().to_owned());
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
    fn only_exported_wxs_members_are_hoverable_and_navigable() {
        let wxml = r#"<wxs module="u">const hidden = 1; const visible = 2; module.exports = { visible };</wxs><view>{{ u.hidden }}</view><view>{{ u.visible }}</view>"#;
        let hidden = u32::try_from(wxml.find("hidden }").expect("hidden use")).expect("offset");
        assert!(hover("pages/index/index.wxml", wxml, hidden, &no_access).is_none());
        assert!(definition("pages/index/index.wxml", wxml, hidden, &no_access).is_none());
        let visible = u32::try_from(wxml.rfind("visible").expect("visible use")).expect("offset");
        let visible_hover =
            hover("pages/index/index.wxml", wxml, visible, &no_access).expect("visible hover");
        assert!(
            visible_hover.contents.contains("number"),
            "{visible_hover:?}"
        );
    }

    #[test]
    fn explicit_wxs_export_type_wins_over_a_local_alias() {
        let wxml = r#"<wxs module="u">const foo = 1; module.exports = { foo: "s" };</wxs><view>{{ u.foo }}</view>"#;
        let offset = u32::try_from(wxml.rfind("foo").expect("use")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &no_access).expect("hover");
        assert!(hover.contents.contains("string"), "{hover:?}");
        assert!(!hover.contents.contains("number"), "{hover:?}");
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &no_access).expect("definition");
        let range = definition[0].range;
        assert_eq!(&wxml[range.start() as usize..range.end() as usize], "foo");
        // The definition is the explicit export property, not the earlier local.
        assert!(range.start() > u32::try_from(wxml.find("const foo").expect("local")).unwrap());
    }

    #[test]
    fn wxs_alias_definition_points_at_the_local_declaration() {
        let wxml = r#"<wxs module="u">const foo = 1; module.exports = { foo };</wxs><view>{{ u.foo }}</view>"#;
        let offset = u32::try_from(wxml.rfind("foo").expect("use")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &no_access).expect("hover");
        assert!(hover.contents.contains("number"), "{hover:?}");
        let definition =
            definition("pages/index/index.wxml", wxml, offset, &no_access).expect("definition");
        let range = definition[0].range;
        let local = &wxml[range.start() as usize..range.end() as usize];
        assert_eq!(local, "foo");
        assert!(range.start() < u32::try_from(wxml.find("module.exports").unwrap()).unwrap());
    }

    #[test]
    fn indexed_template_budget_marks_completion_incomplete() {
        use std::fmt::Write as _;
        let mut wxml = String::new();
        for index in 0..513 {
            let _ = write!(wxml, "<template name=\"t{index}\">x</template>");
        }
        wxml.push_str("<template is=\"x");
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", &wxml, offset, &no_access).expect("completion");
        assert!(result.incomplete);
    }

    #[test]
    fn wxs_file_budget_is_reported_as_a_budget_not_a_missing_file() {
        let mut wxml = String::new();
        for index in 0..40 {
            let _ = write!(wxml, "<wxs module=\"u{index}\" src=\"./u{index}.wxs\" />");
        }
        let offset = u32::try_from(wxml.rfind("./u39.wxs").expect("last src")).expect("offset");
        let hover = hover("pages/index/index.wxml", &wxml, offset, &no_access).expect("hover");
        assert!(hover.contents.contains("file budget reached"), "{hover:?}");
        assert!(!hover.contents.contains("missing"), "{hover:?}");
    }

    #[test]
    fn wxs_dot_completion_omits_illegal_member_names() {
        let wxml =
            r#"<wxs module="u">module.exports = { "a-b": 1, 2: "two", ok: 3 };</wxs><view>{{ u."#;
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["ok"]);
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
            src_hover.contents.contains("WXS source is missing"),
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
        for index in 0..=crate::project_host::MAX_PROJECT_VIEWS {
            let _ = write!(wxml, "<template name=\"t{index}\">x</template>");
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
    fn wxs_dot_completion_tolerates_whitespace_after_the_dot() {
        for hole in [".  ", ".\n  "] {
            let wxml = format!(
                r#"<wxs module="u">module.exports = {{ label: 1, fmt(a) {{ return a; }} }};</wxs><view>{{{{ u{hole}}}}}</view>"#
            );
            let start = wxml.find(&format!("u{hole}")).expect("site");
            let offset = u32::try_from(start + 1 + hole.len()).expect("offset after whitespace");
            let result = completion("pages/index/index.wxml", &wxml, offset, &no_access)
                .expect("completion");
            let labels: Vec<&str> = result
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect();
            assert_eq!(labels, vec!["fmt", "label"], "hole {hole:?}");
            assert!(
                result
                    .items
                    .iter()
                    .all(|item| { item.replace.start() == offset && item.replace.end() == offset })
            );
        }
    }

    #[test]
    fn wxs_optional_chain_completion_tolerates_whitespace() {
        let wxml = r#"<wxs module="u">module.exports = { label: 1 };</wxs><view>{{ u?.  }}</view>"#;
        let start = wxml.find("u?.  ").expect("site");
        let offset = u32::try_from(start + 5).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("completion");
        assert!(result.items.iter().any(|item| item.label == "label"));
    }

    #[test]
    fn wxs_completion_replaces_only_the_typed_prefix() {
        let wxml = r#"<wxs module="u">module.exports = { label: 1 };</wxs><view>{{ u.la }}</view>"#;
        let offset = u32::try_from(wxml.find("u.la").expect("site") + 4).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("completion");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].label, "label");
        assert_eq!(
            &wxml[result.items[0].replace.start() as usize..result.items[0].replace.end() as usize],
            "la"
        );
    }

    #[test]
    fn wxs_module_completion_tolerates_leading_whitespace() {
        let wxml = r#"<wxs module="m1">module.exports = { label: 1 };</wxs><view>{{ m"#;
        let offset = u32::try_from(wxml.len()).expect("offset");
        let result =
            completion("pages/index/index.wxml", wxml, offset, &no_access).expect("completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["m1"]);
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

    const FORMAT_WXS: &str = r"
/**
 * Formats a name for display.
 * @param {string} name input name
 * @returns {string} formatted name
 */
function someFunc(name) { return name; }
const count = 2;
module.exports = { someFunc, count };
";

    #[test]
    fn mustache_module_hover_range_covers_the_reference_for_src_modules() {
        let wxml = r#"<wxs module="m1" src="./fmt.wxs" /><view>{{ m1.someFunc(name) }}</view>"#;
        let entries = [("pages/index/fmt.wxs", FORMAT_WXS)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("m1.someFunc").expect("m1 use")).expect("offset");
        let mustache_hover =
            hover("pages/index/index.wxml", wxml, offset, &access).expect("module hover");
        let range = mustache_hover.range.expect("hover range");
        assert_eq!(slice(wxml, range), "m1");
        assert!(
            mustache_hover.contents.contains("WXS module `m1`"),
            "{}",
            mustache_hover.contents
        );
        assert!(
            mustache_hover
                .contents
                .contains("Source: `pages/index/fmt.wxs`"),
            "{}",
            mustache_hover.contents
        );
        assert_eq!(mustache_hover.positions[0].path, "pages/index/fmt.wxs");

        let attribute_offset = u32::try_from(
            wxml.find("module=\"m1\"").expect("module attribute") + "module=\"".len(),
        )
        .expect("offset");
        let attribute_hover = hover("pages/index/index.wxml", wxml, attribute_offset, &access)
            .expect("attribute hover");
        let attribute_range = attribute_hover.range.expect("attribute range");
        assert_eq!(slice(wxml, attribute_range), "m1");
        assert_ne!(
            range, attribute_range,
            "a mustache hover must not point at the wxs attribute"
        );
    }

    #[test]
    fn mustache_module_hover_range_covers_the_reference_for_inline_modules() {
        let wxml = r#"<wxs module="m2">const label = "hi"; module.exports = { label };</wxs><view>{{ m2.label }}</view>"#;
        let offset = u32::try_from(wxml.find("m2.label").expect("m2 use")).expect("offset");
        let mustache_hover =
            hover("pages/index/index.wxml", wxml, offset, &no_access).expect("module hover");
        let range = mustache_hover.range.expect("hover range");
        assert_eq!(slice(wxml, range), "m2");
        assert!(
            mustache_hover.contents.contains("Source: inline block"),
            "{}",
            mustache_hover.contents
        );

        let attribute_offset = u32::try_from(
            wxml.find("module=\"m2\"").expect("module attribute") + "module=\"".len(),
        )
        .expect("offset");
        let attribute_hover = hover("pages/index/index.wxml", wxml, attribute_offset, &no_access)
            .expect("attribute hover");
        let attribute_range = attribute_hover.range.expect("attribute range");
        assert_eq!(slice(wxml, attribute_range), "m2");
        assert_ne!(
            range, attribute_range,
            "a mustache hover must not point at the wxs attribute"
        );
    }

    #[test]
    fn module_hover_lists_export_signatures_and_types() {
        let wxml = r#"<wxs module="m1" src="./fmt.wxs" /><view>{{ m1.someFunc(name) }}</view>"#;
        let entries = [("pages/index/fmt.wxs", FORMAT_WXS)];
        let access = access_map(&entries);
        let offset = u32::try_from(wxml.find("m1.someFunc").expect("m1 use")).expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &access).expect("module hover");
        assert!(
            hover
                .contents
                .contains("- `someFunc`: `(name: string) => string`"),
            "{}",
            hover.contents
        );
        assert!(
            hover.contents.contains("- `count`: `number`"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("Exports:"), "{}", hover.contents);
    }

    #[test]
    fn module_hover_caps_the_export_list() {
        let mut body = String::new();
        for index in 0..(MAX_HOVER_EXPORTS + 3) {
            let _ = writeln!(body, "exports.value{index} = {index};");
        }
        let wxml = format!(r#"<wxs module="m">{body}</wxs><view>{{{{ m.value0 }}}}</view>"#);
        let offset = u32::try_from(wxml.find("m.value0").expect("m use")).expect("offset");
        let hover = hover("pages/index/index.wxml", &wxml, offset, &no_access).expect("hover");
        assert!(
            hover.contents.contains("- `value0`: `number`"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("and 3 more"), "{}", hover.contents);
        assert_eq!(
            hover.contents.matches("\n- `value").count(),
            MAX_HOVER_EXPORTS
        );
        assert!(
            !hover.contents.contains("`value66`"),
            "unlisted rows must not leak: {}",
            hover.contents
        );
    }

    #[test]
    fn wxs_member_hover_keeps_multiline_jsdoc_readable() {
        let wxml = r#"<wxs module="m1">/**
 * Formats a name for display.
 * @param {string} name input name
 * @returns {string} formatted name
 */
function someFunc(name) { return name; }
module.exports = { someFunc };</wxs><view>{{ m1.someFunc(name) }}</view>"#;
        let offset = u32::try_from(wxml.find("m1.someFunc").expect("member use") + "m1.".len())
            .expect("offset");
        let hover = hover("pages/index/index.wxml", wxml, offset, &no_access).expect("hover");
        assert_eq!(slice(wxml, hover.range.expect("range")), "someFunc");
        assert!(hover.contents.contains('\n'), "{}", hover.contents);
        assert!(
            hover.contents.contains("@param {string} name input name"),
            "{}",
            hover.contents
        );
        assert!(!hover.contents.contains("\\{"), "{}", hover.contents);
    }

    #[test]
    fn wxs_member_hover_truncates_long_docs_but_keeps_the_signature() {
        let mut body =
            String::from("/**\n * Formats a name.\n * @param {string} name input name\n");
        for _ in 0..40 {
            body.push_str(" * Extra documentation line for the budget.\n");
        }
        body.push_str(" * @returns {string} formatted name\n */\n");
        body.push_str("function someFunc(name) { return name; }\n");
        body.push_str("module.exports = { someFunc };");
        let wxml =
            format!(r#"<wxs module="m">{body}</wxs><view>{{{{ m.someFunc(name) }}}}</view>"#);
        let offset =
            u32::try_from(wxml.rfind("someFunc(name)").expect("member use") + 1).expect("offset");
        let hover = hover("pages/index/index.wxml", &wxml, offset, &no_access).expect("hover");
        assert!(
            hover
                .contents
                .contains("m.someFunc: (name: string) => string"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains('…'), "{}", hover.contents);
    }

    const SOURCE_WXS: &str = r#"/**
 * Formats a name for display.
 * @param {string} name input name
 * @returns {string} formatted name
 */
function someFunc(name) { return name; }
const count = 2;
function hidden(value) { return value; }
module.exports = { someFunc, count };
const label = someFunc("x");
"#;

    #[test]
    fn wxs_source_hover_answers_declarations_exports_and_calls() {
        let declaration = u32::try_from(
            SOURCE_WXS.find("function someFunc").expect("declaration") + "function ".len(),
        )
        .expect("offset");
        let hover = wxs_source_hover(SOURCE_WXS, declaration).expect("declaration hover");
        assert_eq!(slice(SOURCE_WXS, hover.range.expect("range")), "someFunc");
        assert!(
            hover
                .contents
                .starts_with("```wxs\nsomeFunc: (name: string) => string"),
            "{}",
            hover.contents
        );
        assert!(
            hover.contents.contains("@param {string} name input name"),
            "{}",
            hover.contents
        );
        assert!(
            !hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );
        assert!(hover.positions.is_empty(), "{hover:?}");

        let export_key = u32::try_from(
            SOURCE_WXS.find("module.exports = {").expect("exports") + "module.exports = { ".len(),
        )
        .expect("offset");
        let hover = wxs_source_hover(SOURCE_WXS, export_key).expect("export key hover");
        assert_eq!(slice(SOURCE_WXS, hover.range.expect("range")), "someFunc");
        assert!(
            hover
                .contents
                .contains("someFunc: (name: string) => string"),
            "{}",
            hover.contents
        );
        assert!(
            !hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );

        let call = u32::try_from(SOURCE_WXS.rfind("someFunc").expect("call") + 1).expect("offset");
        let hover = wxs_source_hover(SOURCE_WXS, call).expect("call hover");
        assert_eq!(slice(SOURCE_WXS, hover.range.expect("range")), "someFunc");
        assert!(
            hover
                .contents
                .contains("someFunc: (name: string) => string"),
            "{}",
            hover.contents
        );
        assert!(
            !hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn wxs_source_hover_uses_the_export_key_for_aliases() {
        let text = "const foo = 1;\nmodule.exports = { foo };\n";
        let export_key = u32::try_from(
            text.find("module.exports = {").expect("exports") + "module.exports = { ".len(),
        )
        .expect("offset");
        let hover = wxs_source_hover(text, export_key).expect("export key hover");
        assert_eq!(slice(text, hover.range.expect("range")), "foo");
        assert!(hover.contents.contains("foo: number"), "{}", hover.contents);
        assert!(
            !hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn wxs_source_hover_does_not_mark_alias_declarations_as_not_exported() {
        let text = "function foo() { return 1; }\nmodule.exports = { out: foo };\n";
        let offset =
            u32::try_from(text.find("function foo").expect("declaration") + "function ".len())
                .expect("offset");
        let hover = wxs_source_hover(text, offset).expect("declaration hover");
        assert_eq!(slice(text, hover.range.expect("range")), "foo");
        assert!(hover.contents.contains("foo: "), "{}", hover.contents);
        assert!(
            !hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn wxs_source_hover_marks_local_declarations_as_not_exported() {
        let offset =
            u32::try_from(SOURCE_WXS.find("function hidden").expect("local") + "function ".len())
                .expect("offset");
        let hover = wxs_source_hover(SOURCE_WXS, offset).expect("local hover");
        assert_eq!(slice(SOURCE_WXS, hover.range.expect("range")), "hidden");
        assert!(hover.contents.contains("hidden: "), "{}", hover.contents);
        assert!(
            hover.contents.contains("not exported"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn wxs_source_hover_ignores_non_member_words() {
        let keyword =
            u32::try_from(SOURCE_WXS.find("return value").expect("keyword") + 1).expect("offset");
        assert!(wxs_source_hover(SOURCE_WXS, keyword).is_none());
        let string = u32::try_from(SOURCE_WXS.find("\"x\"").expect("string") + 1).expect("offset");
        assert!(wxs_source_hover(SOURCE_WXS, string).is_none());
    }
}

/// One WXS function call signature resolved from static module facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WxsCallSignature {
    /// Rendered signature, for example `(a: any, b: number) => string`.
    pub(crate) label: String,
    /// Parameter labels in order.
    pub(crate) parameters: Vec<String>,
    /// Zero-based active parameter.
    pub(crate) active_parameter: u32,
    /// Member documentation, when present.
    pub(crate) documentation: Option<String>,
}

/// Resolves signature help for a WXS static function call at `offset`.
///
/// Only a call whose callee is exactly `<module>.<exported function>` is
/// answered. Page methods, dynamic exports and calls inside strings are not
/// signature sources.
pub(crate) fn wxs_call_signature(
    path: &str,
    text: &str,
    offset: u32,
    access: &Access<'_>,
) -> Option<WxsCallSignature> {
    if !has_static_constructs(text) {
        return None;
    }
    let index = build_index(path, text, access);
    if index.wxs.is_empty() {
        return None;
    }
    let tree = parse(text).tree;
    let expr = innermost_mustache_expr(&tree, offset)?;
    let call = innermost_call(expr, offset)?;
    let ExprKind::Call { callee, arguments } = call.kind() else {
        return None;
    };
    let ExprKind::Member {
        object, property, ..
    } = callee.kind()
    else {
        return None;
    };
    let ExprKind::Identifier { name: root } = object.kind() else {
        return None;
    };
    let ExprKind::Identifier { name: member } = property.kind() else {
        return None;
    };
    let module = text.get(root.start() as usize..root.end() as usize)?;
    let member = text.get(member.start() as usize..member.end() as usize)?;
    let wxs = index.wxs.iter().find(|wxs| wxs.module == module)?;
    let exported = exported_member(&wxs.summary, member)?;
    let resolved = if exported.signature.is_none() {
        exported
            .alias_of
            .and_then(|alias| {
                wxs.summary
                    .members
                    .iter()
                    .find(|candidate| !candidate.exported && candidate.name_range == alias)
            })
            .unwrap_or(exported)
    } else {
        exported
    };
    if !matches!(resolved.kind, wxml_types::WxsMemberKind::Function) {
        return None;
    }
    let signature = resolved.signature.clone()?;
    let parameters = signature_parameters(&signature);
    let active_parameter = active_parameter(arguments, offset);
    Some(WxsCallSignature {
        label: signature,
        parameters,
        active_parameter,
        documentation: (!resolved.doc.trim().is_empty()).then(|| resolved.doc.clone()),
    })
}

fn innermost_mustache_expr(tree: &SyntaxTree, offset: u32) -> Option<&Expr> {
    let mut found = None;
    for (id, node) in tree.nodes() {
        if node.kind() != wxml_syntax::NodeKind::MustacheExpr {
            continue;
        }
        let range = node.text_range();
        if range.start() <= offset
            && offset <= range.end()
            && found.is_none_or(|(best, _)| range.len() < best)
            && let Some(expr) = tree.expr(id)
        {
            found = Some((range.len(), expr));
        }
    }
    found.map(|(_, expr)| expr)
}

fn innermost_call(expr: &Expr, offset: u32) -> Option<&Expr> {
    let range = expr.range();
    if !(range.start() <= offset && offset <= range.end()) {
        return None;
    }
    if let ExprKind::Call { callee, arguments } = expr.kind() {
        if let Some(inner) = innermost_call(callee, offset) {
            return Some(inner);
        }
        for argument in arguments {
            if let Some(inner) = innermost_call(argument, offset) {
                return Some(inner);
            }
        }
        Some(expr)
    } else {
        for child in expr.children() {
            if let Some(inner) = innermost_call(child, offset) {
                return Some(inner);
            }
        }
        None
    }
}

fn active_parameter(arguments: &[Expr], offset: u32) -> u32 {
    let mut active = 0_u32;
    for argument in arguments {
        if argument.range().end() <= offset {
            active = active.saturating_add(1);
            continue;
        }
        if argument.range().start() <= offset {
            break;
        }
        break;
    }
    active
}

/// Splits the outer parameter list of a rendered function signature.
fn signature_parameters(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut end = None;
    for (index, character) in signature
        .char_indices()
        .skip_while(|(index, _)| *index < open)
    {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return Vec::new();
    };
    let inner = signature.get(open + 1..end).unwrap_or_default();
    let mut parameters = Vec::new();
    let mut start = 0_usize;
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, character) in inner.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                if let Some(parameter) = inner.get(start..index).map(str::trim)
                    && !parameter.is_empty()
                {
                    parameters.push(parameter.to_owned());
                }
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if let Some(parameter) = inner.get(start..).map(str::trim)
        && !parameter.is_empty()
    {
        parameters.push(parameter.to_owned());
    }
    parameters
}

#[cfg(test)]
mod semantic_call_tests {
    use super::*;

    #[test]
    fn resolves_wxs_call_signature_in_template_definition() {
        let text = "<wxs module=\"u\">\nfunction add(a, b) { return a + b; }\nmodule.exports = { add: add };\n</wxs>\n<template name=\"row\"><view>{{ u.add(1, 2) }}</view></template>";
        let offset = u32::try_from(text.find("u.add(1, ").expect("call") + "u.add(1, ".len())
            .expect("offset");
        let access = |_: &str| None;
        let signature =
            wxs_call_signature("pages/index/index.wxml", text, offset, &access).expect("signature");
        assert_eq!(signature.parameters.len(), 2);
        assert_eq!(signature.active_parameter, 1);
        assert!(signature.label.starts_with('('));
    }
}
