//! Local class and resource features in the LSP layer.
//!
//! Class completion/hover/definition and document links are answered from the
//! project style index and file resolver. They never start Node, so they keep
//! working when the type backend is unavailable. Anything the index cannot
//! prove (dynamic classes, remote URLs, missing files) is omitted rather than
//! guessed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};
use wxml_project::{ClassDefinition, ProjectPath, ResourceResolution, StyleViewFacts};
use wxml_text::{LineIndex, SourceRange};
use wxml_types::{FileId, ProjectId};

use super::{
    code_text, hover_target, line_index_range_value, markdown_escape, source_range_value, string,
    template_document, text_document,
};
use crate::completion::context::{Context as Detected, Site};
use crate::jsonrpc::{self, Outgoing};
use crate::server::{Server, log};
use crate::styles::{self, ClassSite, ResourceSite};
use crate::workspace::Document;

/// Maximum distinct style files read while mapping locations.
const MAX_LOCAL_FILES: usize = 8;

/// Maximum estimated text plus line-index bytes read for local locations.
const MAX_LOCAL_INDEX_BYTES: usize = 64 * 1024 * 1024;

/// Maximum definitions returned by one definition request.
const MAX_LOCAL_DEFINITIONS: usize = 16;

/// Maximum distinct class comments shown in one hover.
const MAX_HOVER_COMMENTS: usize = 4;

/// Maximum document links returned for one template.
const MAX_DOCUMENT_LINKS: usize = 512;

/// Maximum URI length stored in a link or location.
const MAX_LOCAL_URI_BYTES: usize = 8 * 1024;

/// Longest class name shown in hover.
const MAX_HOVER_CLASS_BYTES: usize = 128;

/// Longest class comment shown in hover.
const MAX_HOVER_COMMENT_BYTES: usize = 512;

/// Lazily loaded target files shared by hover and definition mapping.
struct Targets<'a> {
    server: &'a Server,
    project: ProjectId,
    files: HashMap<ProjectPath, Option<(String, Arc<LineIndex>)>>,
    bytes: usize,
    exhausted: bool,
}

impl<'a> Targets<'a> {
    /// Creates an empty per-request cache bound to one project identity.
    fn new(server: &'a Server, project: ProjectId) -> Self {
        Self {
            server,
            project,
            files: HashMap::new(),
            bytes: 0,
            exhausted: false,
        }
    }

    /// Loads one target file and its line index.
    fn load(&mut self, path: &ProjectPath) -> Option<(String, Arc<LineIndex>)> {
        if let Some(cached) = self.files.get(path) {
            return cached.clone();
        }
        if self.exhausted || self.files.len() >= MAX_LOCAL_FILES {
            self.exhausted = true;
            self.files.insert(path.clone(), None);
            log(format_args!("local target file budget reached"));
            return None;
        }
        let loaded = (|| {
            let file = self.server.host.file_id_in(&self.project, path).ok()?;
            let uri = self.server.host.uri_for(&file)?;
            if uri.len() > MAX_LOCAL_URI_BYTES {
                return None;
            }
            let text = self.server.host.file_text(&file)?;
            let cost = text
                .len()
                .saturating_add(8_usize.saturating_mul(text.match_indices('\n').count()));
            if self.bytes.saturating_add(cost) > MAX_LOCAL_INDEX_BYTES {
                return None;
            }
            self.bytes = self.bytes.saturating_add(cost);
            Some((uri, Arc::new(LineIndex::new(text.as_ref()))))
        })();
        self.files.insert(path.clone(), loaded.clone());
        if loaded.is_none() {
            log(format_args!("local target file is unavailable"));
        }
        loaded
    }

    /// Maps a byte range in `path` to an LSP location.
    fn location(&mut self, path: &ProjectPath, range: SourceRange) -> Option<Value> {
        let (uri, index) = self.load(path)?;
        let range = line_index_range_value(&index, range)?;
        Some(json!({"uri": uri, "range": range}))
    }
}

/// Short scope label for hover/detail text.
fn scope_label(class: &ClassDefinition) -> String {
    let mut label = class.scope.label().to_owned();
    if class.imported {
        label.push_str(" (imported)");
    }
    label
}

/// Builds hover markdown for one class.
///
/// The location and selector list is intentionally omitted: editors already
/// provide "go to definition" for that, and the partial-index note is an
/// internal coverage detail, not user-facing documentation.
fn class_hover_value(document: &Document, facts: &StyleViewFacts, site: &ClassSite) -> Value {
    if site.name.is_empty() {
        return Value::Null;
    }
    let definitions = facts.definitions(&site.name);
    if definitions.is_empty() {
        return Value::Null;
    }
    let mut lines = vec![format!(
        "`{}` — WXSS class",
        code_text(&site.name, MAX_HOVER_CLASS_BYTES)
    )];
    let mut comments = 0_usize;
    let mut seen = HashSet::new();
    for definition in &definitions {
        if comments >= MAX_HOVER_COMMENTS {
            break;
        }
        let Some(comment) = definition.comment() else {
            continue;
        };
        let comment = code_text(comment, MAX_HOVER_COMMENT_BYTES);
        let comment = comment.trim();
        if comment.is_empty() || !seen.insert(comment.to_owned()) {
            continue;
        }
        lines.push(String::new());
        lines.push(markdown_escape(comment));
        comments = comments.saturating_add(1);
    }
    let mut value = json!({
        "contents": {
            "kind": "markdown",
            "value": lines.join("\n"),
        },
    });
    if let Some(range) = source_range_value(document, SourceRange::new(site.token))
        && let Some(object) = value.as_object_mut()
    {
        object.insert("range".to_owned(), range);
    }
    value
}

/// Empty but handled LSP success value.
const fn empty_links() -> Value {
    Value::Array(Vec::new())
}

/// Builds the local definition response for a class or resource site.
fn local_definition_locations(
    server: &Server,
    file: &FileId,
    class: Option<&ClassSite>,
    resource: Option<&ResourceSite>,
) -> Option<Value> {
    if let Some(site) = class {
        let facts = server.host.style_facts_for(file)?;
        if site.name.is_empty() {
            return Some(empty_links());
        }
        if !facts.complete {
            log(format_args!(
                "class definition lookup uses a partial style index"
            ));
        }
        let mut targets = Targets::new(server, file.project().clone());
        let mut locations = Vec::new();
        for definition in facts
            .definitions(&site.name)
            .into_iter()
            .take(MAX_LOCAL_DEFINITIONS)
        {
            if let Some(location) = targets.location(definition.file(), definition.range()) {
                locations.push(location);
            }
        }
        return Some(Value::Array(locations));
    }
    let site = resource?;
    let resolution = server.host.resource_resolution_for(file, &site.text)?;
    let ResourceResolution::File(target) = resolution else {
        return Some(empty_links());
    };
    if !site.kind.accepts(&target) {
        return Some(empty_links());
    }
    // Resource targets may be binary; never run them through the UTF-8
    // text/line-index path. The protocol only needs the file URI and a range.
    let Ok(target_file) = server.host.file_id_in(file.project(), &target) else {
        return Some(empty_links());
    };
    let Some(uri) = server.host.uri_for(&target_file) else {
        return Some(empty_links());
    };
    if uri.len() > MAX_LOCAL_URI_BYTES {
        return Some(empty_links());
    }
    Some(Value::Array(vec![json!({
        "uri": uri,
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 0},
        },
    })]))
}

impl Server {
    /// Answers a class hover before falling back to the type backend.
    pub(crate) fn local_hover_response(
        &self,
        id: &Value,
        params: Option<&Value>,
    ) -> Option<Vec<Outgoing>> {
        let params = params?;
        let (uri, position) = hover_target(params)?;
        if !template_document(self, uri) {
            return None;
        }
        let document = self.workspace.get(uri)?;
        let offset = document.offset_at(position)?;
        let file = self.host.resolve_file(uri).ok()?;
        let site = styles::class_site_at(document.text(), offset)?;
        let facts = self.host.style_facts_for(&file)?;
        let value = class_hover_value(document, &facts, &site);
        Some(vec![Outgoing::success(id.clone(), value)])
    }

    /// Answers a class or resource definition before the type backend.
    pub(crate) fn local_definition_response(
        &self,
        id: &Value,
        params: Option<&Value>,
    ) -> Option<Vec<Outgoing>> {
        let params = params?;
        let (uri, position) = hover_target(params)?;
        if !template_document(self, uri) {
            return None;
        }
        let document = self.workspace.get(uri)?;
        let offset = document.offset_at(position)?;
        let file = self.host.resolve_file(uri).ok()?;
        let text = document.text();
        let class = styles::class_site_at(text, offset);
        let resource = if class.is_none() {
            styles::resource_site_at(text, offset)
        } else {
            None
        };
        if class.is_none() && resource.is_none() {
            return None;
        }
        let value = local_definition_locations(self, &file, class.as_ref(), resource.as_ref())?;
        Some(vec![Outgoing::success(id.clone(), value)])
    }

    /// Builds a class completion list for the detected value site.
    pub(crate) fn class_completion_value(
        &self,
        uri: &str,
        document: &Document,
        text: &str,
        offset: u32,
        detected: &Detected,
    ) -> Option<Value> {
        let site = styles::class_completion_site(text, offset, detected)?;
        let file = self.host.resolve_file(uri).ok()?;
        let facts = self.host.style_facts_for(&file)?;
        let replace = source_range_value(document, SourceRange::new(site.token))?;
        let external = site
            .existing
            .iter()
            .any(|name| styles::is_external_class(name));
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut truncated = false;
        for (order, class) in facts.classes.iter().enumerate() {
            if !seen.insert(class.name()) {
                continue;
            }
            if site.existing.contains(class.name()) || !class.name().starts_with(&site.prefix) {
                continue;
            }
            if items.len() >= wxml_types::MAX_COMPLETION_ITEMS {
                truncated = true;
                break;
            }
            let mut item = json!({
                "label": class.name(),
                "kind": 7,
                "detail": format!("{} ({})", class.file().as_str(), scope_label(class)),
                "filterText": class.name(),
                "sortText": format!("{order:010}_{}", class.name()),
                "textEdit": {
                    "range": replace,
                    "newText": class.name(),
                },
            });
            if let Some(comment) = class.comment()
                && let Some(object) = item.as_object_mut()
            {
                object.insert(
                    "documentation".to_owned(),
                    Value::String(code_text(comment, MAX_HOVER_COMMENT_BYTES)),
                );
            }
            items.push(item);
        }
        Some(json!({
            "isIncomplete": truncated || !facts.complete || external,
            "items": items,
        }))
    }

    /// Builds a resource path completion list for the detected value site.
    pub(crate) fn resource_completion_value(
        &self,
        uri: &str,
        document: &Document,
        text: &str,
        offset: u32,
        detected: &Detected,
    ) -> Option<Value> {
        let Site::AttributeValue { tag, attribute, .. } = &detected.site else {
            return None;
        };
        let kind = styles::resource_attribute(tag, attribute)?;
        let (value, quote) = styles::value_bounds(text, offset, detected)?;
        let value_start = usize::try_from(value.start()).ok()?;
        let cursor = usize::try_from(offset).ok()?;
        let prefix = text.get(value_start..cursor)?;
        let file = self.host.resolve_file(uri).ok()?;
        let candidates = self.host.resource_candidates_for(&file, prefix)?;
        let replace = source_range_value(document, SourceRange::new(value))?;
        let mut items = Vec::new();
        for candidate in &candidates.items {
            if candidate.kind == wxml_project::ResourceKind::File && !kind.accepts(&candidate.path)
            {
                continue;
            }
            if let Some(quote) = quote {
                if candidate
                    .insert
                    .bytes()
                    .any(|byte| byte == quote || byte.is_ascii_control())
                {
                    continue;
                }
            } else if candidate.insert.contains('"') || candidate.insert.contains('\'') {
                continue;
            }
            let insert = if quote.is_some() {
                candidate.insert.clone()
            } else {
                format!("\"{}\"", candidate.insert)
            };
            let (item_kind, category) = match candidate.kind {
                wxml_project::ResourceKind::Directory => (19, 0),
                wxml_project::ResourceKind::File => (17, 1),
            };
            items.push(json!({
                "label": candidate.label,
                "kind": item_kind,
                "detail": if category == 0 { "Directory" } else { kind.label() },
                "filterText": candidate.label,
                "sortText": format!("{category}_{}", candidate.label),
                "textEdit": {
                    "range": replace,
                    "newText": insert,
                },
            }));
            if items.len() >= wxml_types::MAX_COMPLETION_ITEMS {
                break;
            }
        }
        Some(json!({
            "isIncomplete": candidates.truncated,
            "items": items,
        }))
    }

    /// Handles `textDocument/documentLink`.
    pub(crate) fn document_link(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(document) = text_document(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument",
            )];
        };
        let Some(uri) = string(document, "uri") else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument",
            )];
        };
        if !template_document(self, uri) {
            return vec![Outgoing::success(id, empty_links())];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, empty_links())];
        };
        let Ok(file) = self.host.resolve_file(uri) else {
            return vec![Outgoing::success(id, empty_links())];
        };
        let text = document.text();
        let mut links = Vec::new();
        let mut seen = HashSet::new();
        let mut resolutions = 0_usize;
        for site in styles::resource_sites(text, styles::MAX_WXML_RESOURCE_SITES) {
            if resolutions >= styles::MAX_RESOURCE_RESOLUTIONS {
                log(format_args!("document link resolution budget reached"));
                break;
            }
            resolutions = resolutions.saturating_add(1);
            let Some(ResourceResolution::File(target)) =
                self.host.resource_resolution_for(&file, &site.text)
            else {
                continue;
            };
            if !site.kind.accepts(&target) {
                continue;
            }
            let Ok(target_file) = self.host.file_id_in(file.project(), &target) else {
                continue;
            };
            let Some(target_uri) = self.host.uri_for(&target_file) else {
                continue;
            };
            if target_uri.len() > MAX_LOCAL_URI_BYTES {
                continue;
            }
            let Some(range) = source_range_value(document, SourceRange::new(site.value)) else {
                continue;
            };
            let dedupe = (site.value.start(), site.value.end(), target_uri.clone());
            if !seen.insert(dedupe) {
                continue;
            }
            links.push(json!({
                "range": range,
                "target": target_uri,
                "tooltip": target.as_str(),
            }));
            if links.len() >= MAX_DOCUMENT_LINKS {
                log(format_args!("document link budget reached"));
                break;
            }
        }
        vec![Outgoing::success(id, Value::Array(links))]
    }
}
