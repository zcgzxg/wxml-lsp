//! Lifecycle methods and method dispatch.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use wxml_project::{ProjectPath, StyleConfig, StyleDiagnosticsPolicy};
use wxml_types::{CapabilityStatus, ProjectId, SourceSnapshot, ViewRevision};

use super::{log_safe_method, log_safe_text, status_label, template_document};
use crate::configuration::InitializationOptions;
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::project_host::{
    DocumentRoute, HostError, OpenDocument, WatchedChangeKind, WatchedFileChange,
};
use crate::server::{Lifecycle, Server, log};

/// Reads `rootUri`, falling back to the first workspace folder.
fn requested_root(params: Option<&Value>) -> Option<&str> {
    let object = params?.as_object()?;
    if let Some(uri) = object.get("rootUri").and_then(Value::as_str) {
        return Some(uri);
    }
    object
        .get("workspaceFolders")?
        .as_array()?
        .first()?
        .get("uri")?
        .as_str()
}

/// Reads one boolean capability nested under `capabilities`.
fn client_capability(params: Option<&Value>, path: &[&str]) -> bool {
    let Some(mut value) = params.and_then(|params| params.get("capabilities")) else {
        return false;
    };
    for key in path {
        let Some(next) = value.get(*key) else {
            return false;
        };
        value = next;
    }
    value.as_bool().unwrap_or(false)
}

/// Reads an optional boolean client capability path.
fn client_capability_optional(params: Option<&Value>, path: &[&str]) -> Option<bool> {
    let mut value = params.and_then(|params| params.get("capabilities"))?;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_bool()
}

/// Reads one string initialization option.
fn initialization_string<'a>(params: Option<&'a Value>, key: &str) -> Option<&'a str> {
    params?.get("initializationOptions")?.get(key)?.as_str()
}

/// Reads one boolean initialization option.
fn initialization_bool(params: Option<&Value>, key: &str) -> Option<bool> {
    params?.get("initializationOptions")?.get(key)?.as_bool()
}

/// Builds the `InitializeResult`.
fn initialize_result(server: &Server) -> Value {
    let mut result = json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                // 2 = incremental; full changes are also accepted.
                "change": 2,
                // Zed only forwards `textDocument/didSave` when the server
                // advertises save support. The handler reads the open buffer
                // instead of asking the client to resend its text.
                "save": { "includeText": false },
            },
            // Static tag/attribute documentation and component definitions
            // work without a backend; a real Node backend adds expression
            // hover and symbol definitions.
            "hoverProvider": true,
            "definitionProvider": server.host.is_project_open()
                || server.capabilities.definition.is_supported(),
            // Static template completion is always available; the flag only
            // covers Node-backed expression/event queries.
            "completionProvider": {
                "triggerCharacters": ["<", " ", ":", "\"", "'", ".", "{", "="],
                "resolveProvider": false,
            },
            // Formatting and structure are answered locally from the WXML
            // syntax tree, so they stay available when Node is offline.
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": true,
            "linkedEditingRangeProvider": true,
            "foldingRangeProvider": true,
            "selectionRangeProvider": true,
            "documentSymbolProvider": true,
            // Local file links for proven resource attributes; resolution is
            // synchronous and never fetches a remote URL.
            "documentLinkProvider": {
                "resolveProvider": false,
            },
            // Semantic tools implemented by the S package. Only features with a
            // real handler and failing-safe tests are advertised.
            "referencesProvider": true,
            "renameProvider": {"prepareProvider": true},
            "codeActionProvider": {
                "codeActionKinds": ["quickfix"],
                "resolveProvider": false,
            },
            "semanticTokensProvider": {
                "legend": crate::semantics::tokens::legend(),
                "full": true,
            },
            "workspaceSymbolProvider": true,
            "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
            "executeCommandProvider": {"commands": ["wxml.references"]},
        },
        "serverInfo": {
            "name": "wxml-lsp",
            "version": crate::version(),
        },
    });
    if let Some(capabilities) = result
        .get_mut("capabilities")
        .and_then(Value::as_object_mut)
    {
        if server.semantic_tools.inlay_hints {
            capabilities.insert(
                "inlayHintProvider".to_owned(),
                json!({"resolveProvider": false}),
            );
        }
        if server.semantic_tools.code_lens {
            capabilities.insert(
                "codeLensProvider".to_owned(),
                json!({"resolveProvider": false}),
            );
        }
        if !server.client.document_changes {
            capabilities.remove("renameProvider");
            capabilities.remove("codeActionProvider");
        }
    }
    result
}

/// Reads an optional diagnostics debounce from initialization options.
///
/// Shape: `{"wxml":{"diagnostics":{"debounceMs":120}}}`. A value outside
/// `0..=5000` is ignored in favour of the environment/default setting. This is
/// configuration, not project content, and tests use it to pin behavior.
fn diagnostics_debounce_override(params: Option<&Value>) -> Option<std::time::Duration> {
    let root = params?.get("initializationOptions")?;
    let options = root
        .get("wxml")
        .and_then(Value::as_object)
        .or_else(|| root.get("style").and_then(Value::as_object))
        .or_else(|| root.as_object())?;
    let milliseconds = options
        .get("diagnostics")?
        .as_object()?
        .get("debounceMs")?
        .as_u64()?;
    (milliseconds <= 5_000).then(|| std::time::Duration::from_millis(milliseconds))
}

/// Reads optional `wxml` style settings from initialization options.
///
/// The namespaced form is preferred:
/// `{"wxml":{"globalStyles":["styles/theme.scss"],
/// "styleDiagnostics":{"resources":true,"classes":false}}}`. Top-level keys
/// are accepted for early clients, but unknown keys are ignored.
fn style_initialization_options(params: Option<&Value>) -> (StyleConfig, StyleDiagnosticsPolicy) {
    let mut policy = StyleDiagnosticsPolicy::default();
    let mut config = StyleConfig::default();
    let Some(options) = params
        .and_then(|params| params.get("initializationOptions"))
        .and_then(Value::as_object)
    else {
        return (config, policy);
    };
    let options = options
        .get("wxml")
        .and_then(Value::as_object)
        .or_else(|| options.get("style").and_then(Value::as_object))
        .unwrap_or(options);
    if let Some(paths) = options.get("globalStyles").and_then(Value::as_array) {
        for path in paths.iter().take(64) {
            let Some(path) = path.as_str() else {
                config
                    .invalid_global_styles
                    .push("<non-string globalStyles entry>".to_owned());
                continue;
            };
            if path.len() > 1024 {
                config
                    .invalid_global_styles
                    .push(path.chars().take(256).collect());
                continue;
            }
            match ProjectPath::new(path) {
                Ok(path) => config.global_styles.push(path),
                Err(_) => config
                    .invalid_global_styles
                    .push(path.chars().take(256).collect()),
            }
        }
    }
    let diagnostics = options
        .get("styleDiagnostics")
        .or_else(|| options.get("diagnostics"))
        .and_then(Value::as_object);
    if let Some(diagnostics) = diagnostics {
        if let Some(resources) = diagnostics.get("resources").and_then(Value::as_bool) {
            policy.resources = resources;
        }
        if let Some(classes) = diagnostics.get("classes").and_then(Value::as_bool) {
            policy.classes = classes;
        }
    }
    (config, policy)
}

/// One-line description of a status for stderr logs.
fn describe_status(status: &CapabilityStatus) -> String {
    status.reason().map_or_else(
        || status_label(status).to_owned(),
        |reason| format!("{} ({})", status_label(status), log_safe_text(reason, 200)),
    )
}

impl Server {
    /// Handles one request.
    pub(crate) fn on_request(
        &mut self,
        id: Value,
        method: &str,
        params: Option<Value>,
    ) -> Vec<Outgoing> {
        match self.lifecycle {
            Lifecycle::Uninitialized if method != "initialize" => {
                // The protocol asks for `InvalidRequest` on an early
                // `shutdown` and `ServerNotInitialized` for other methods.
                let code = if method == "shutdown" {
                    jsonrpc::error_codes::INVALID_REQUEST
                } else {
                    jsonrpc::error_codes::SERVER_NOT_INITIALIZED
                };
                return vec![Outgoing::failure(id, code, "server is not initialized")];
            }
            Lifecycle::ShuttingDown => {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INVALID_REQUEST,
                    "server is shutting down",
                )];
            }
            Lifecycle::Uninitialized | Lifecycle::Running => {}
        }

        match method {
            "initialize" => self.initialize(id, params.as_ref()),
            "shutdown" => self.shutdown(id),
            "textDocument/hover" => self.hover(id, params),
            "textDocument/definition" => self.definition(id, params),
            "textDocument/references" => self.references(id, params),
            "textDocument/prepareRename" => self.prepare_rename(id, params),
            "textDocument/rename" => self.rename(id, params),
            "textDocument/codeAction" => self.code_action(id, params),
            "textDocument/semanticTokens/full" => self.semantic_tokens_full(id, params),
            "workspace/symbol" => self.workspace_symbol(id, params),
            "textDocument/inlayHint" => self.inlay_hint(id, params),
            "textDocument/signatureHelp" => self.signature_help(id, params),
            "textDocument/codeLens" => self.code_lens(id, params),
            "workspace/executeCommand" => self.execute_command(id, params),
            "textDocument/completion" => self.completion(id, params),
            "textDocument/formatting" => self.formatting(id, params),
            "textDocument/rangeFormatting" => self.range_formatting(id, params),
            "textDocument/linkedEditingRange" => self.linked_editing(id, params),
            "textDocument/foldingRange" => self.folding_range(id, params),
            "textDocument/selectionRange" => self.selection_range(id, params),
            "textDocument/documentSymbol" => self.document_symbol(id, params),
            "textDocument/documentLink" => self.document_link(id, params),
            other => {
                log(format_args!(
                    "unknown request method `{}`",
                    log_safe_method(other)
                ));
                vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::METHOD_NOT_FOUND,
                    "method not found",
                )]
            }
        }
    }

    /// Handles one notification.
    pub(crate) fn on_notification(&mut self, method: &str, params: Option<Value>) -> Vec<Outgoing> {
        if method == "exit" {
            self.stop = true;
            self.clean_shutdown = self.lifecycle == Lifecycle::ShuttingDown;
            return Vec::new();
        }
        match self.lifecycle {
            Lifecycle::Uninitialized => {
                log(format_args!(
                    "ignoring notification `{}` before initialize",
                    log_safe_method(method)
                ));
                return Vec::new();
            }
            Lifecycle::ShuttingDown => {
                log(format_args!(
                    "ignoring notification `{}` after shutdown",
                    log_safe_method(method)
                ));
                return Vec::new();
            }
            Lifecycle::Running => {}
        }

        match method {
            "initialized" => self.initialized(),
            "textDocument/didOpen" => self.did_open(params),
            "textDocument/didChange" => self.did_change(params),
            "textDocument/didClose" => self.did_close(params),
            "textDocument/didSave" => self.did_save(params),
            "workspace/didChangeWatchedFiles" => self.did_change_watched_files(params),
            "workspace/didChangeWorkspaceFolders" => {
                self.did_change_workspace_folders(params.as_ref())
            }
            "$/cancelRequest" => self.cancel_request(params.as_ref()),
            other => {
                log(format_args!(
                    "ignoring unknown notification `{}`",
                    log_safe_method(other)
                ));
                Vec::new()
            }
        }
    }

    /// Answers `initialize` once and opens the requested root.
    #[allow(clippy::too_many_lines)] // one bounded option/root parsing sequence
    fn initialize(&mut self, id: Value, params: Option<&Value>) -> Vec<Outgoing> {
        if self.lifecycle != Lifecycle::Uninitialized {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_REQUEST,
                "server is already initialized",
            )];
        }
        let options = params
            .and_then(|params| params.get("initializationOptions"))
            .unwrap_or(&Value::Null);
        let options = match InitializationOptions::parse(options) {
            Ok(options) => options,
            Err(error) => {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INVALID_PARAMS,
                    error.to_string(),
                )];
            }
        };
        self.initialization = options.clone();
        if let Some(debounce) = diagnostics_debounce_override(params) {
            self.diagnostics_debounce = debounce;
        }
        self.client.watched_files = client_capability(
            params,
            &["workspace", "didChangeWatchedFiles", "dynamicRegistration"],
        );
        self.client.snippets = client_capability(
            params,
            &[
                "textDocument",
                "completion",
                "completionItem",
                "snippetSupport",
            ],
        );
        self.client.hierarchical_symbols = client_capability(
            params,
            &[
                "textDocument",
                "documentSymbol",
                "hierarchicalDocumentSymbolSupport",
            ],
        );
        self.client.document_changes =
            client_capability_optional(params, &["workspace", "workspaceEdit", "documentChanges"])
                .unwrap_or(false);
        let root = requested_root(params);
        self.workspace_root = root.map(str::to_owned);
        let root_path = root.and_then(|uri| wxml_project::root_path_from_uri(uri).ok());
        let configured = initialization_string(params, "snippetsPath")
            .or_else(|| initialization_string(params, "wxmlSnippetsPath"));
        let snippets_enabled = initialization_bool(params, "snippetsEnabled")
            .or_else(|| initialization_bool(params, "wxmlSnippetsEnabled"))
            .unwrap_or(true);
        let snippets_path =
            crate::completion::user_snippets::resolve_path(root_path.as_deref(), configured);
        if configured.is_some() && snippets_path.is_none() {
            log(format_args!(
                "ignoring snippetsPath: relative paths require a file workspace root"
            ));
        }
        self.snippets.configure(snippets_path, snippets_enabled);
        if let Some(semantic) = options.semantic_tools.as_ref() {
            if let Some(enabled) = semantic.inlay_hints {
                self.semantic_tools.inlay_hints = enabled;
            }
            if let Some(enabled) = semantic.code_lens {
                self.semantic_tools.code_lens = enabled;
            }
        }
        match self.host.open_root_with_options(root, &options) {
            Ok(()) => {
                if root.is_some() {
                    let project_id = self.host.project_id();
                    let revision = self.host.project_revision();
                    self.workspace.set_project(project_id, revision);
                }
            }
            Err(HostError::Configuration(error)) => {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INVALID_PARAMS,
                    error.to_string(),
                )];
            }
            Err(error) => log(format_args!("project root could not be opened: {error}")),
        }
        let (style_config, style_diagnostics) = style_initialization_options(params);
        self.host.configure_styles(style_config, style_diagnostics);
        self.lifecycle = Lifecycle::Running;
        let result = initialize_result(self);
        log(format_args!(
            "initialized with hover {}, diagnostics {}, definition {}, completion {}",
            describe_status(&self.capabilities.hover),
            describe_status(&self.capabilities.diagnostics),
            describe_status(&self.capabilities.definition),
            describe_status(&self.capabilities.completion),
        ));
        vec![Outgoing::success(id, result)]
    }

    /// Answers `shutdown` and stops accepting new requests.
    fn shutdown(&mut self, id: Value) -> Vec<Outgoing> {
        self.lifecycle = Lifecycle::ShuttingDown;
        self.clean_shutdown = true;
        vec![Outgoing::success(id, Value::Null)]
    }

    /// Handles `textDocument/didSave`.
    ///
    /// A saved WXML document refreshes its analysis snapshot; a saved script
    /// or JSON document keeps its overlay but rediscovery/dirty views are
    /// refreshed so dependent WXML diagnostics run again. Non-WXML documents
    /// never publish diagnostics themselves.
    pub(crate) fn did_save(&mut self, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!("ignoring didSave without parameters"));
            return Vec::new();
        };
        let Some(uri) = saved_uri(&params) else {
            log(format_args!(
                "ignoring didSave without a valid text document"
            ));
            return Vec::new();
        };
        if uri.is_empty() || uri.len() > crate::workspace::MAX_URI_BYTES {
            log(format_args!("ignoring didSave with an invalid URI"));
            return Vec::new();
        }
        match self.host.route(uri) {
            DocumentRoute::Wxml(path) => {
                self.save_wxml(uri, &path);
                self.schedule_diagnostics(uri)
            }
            DocumentRoute::Script(path) => {
                let mut outgoing = match self.host.save_document(uri) {
                    Ok(update) => self.apply_host_update(&update),
                    Err(error) => {
                        log(format_args!("overlay save failed: {error}"));
                        Vec::new()
                    }
                };
                if super::diagnostics::script_diagnostics_path(&path) {
                    self.cancel_debounced_diagnostics(uri);
                    outgoing.extend(self.schedule_script_diagnostics(uri));
                }
                outgoing
            }
            DocumentRoute::Json(_) | DocumentRoute::Style(_) => {
                match self.host.save_document(uri) {
                    Ok(update) => self.apply_host_update(&update),
                    Err(error) => {
                        log(format_args!("overlay save failed: {error}"));
                        Vec::new()
                    }
                }
            }
            DocumentRoute::Unknown => {
                // Synthetic documents keep their text in the LSP workspace;
                // they have no host snapshot to refresh.
                if template_document(self, uri) {
                    self.schedule_diagnostics(uri)
                } else {
                    log(format_args!("ignoring didSave for a non-template document"));
                    Vec::new()
                }
            }
        }
    }

    /// Handles `workspace/didChangeWatchedFiles`.
    pub(crate) fn did_change_watched_files(&mut self, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!(
                "ignoring didChangeWatchedFiles without parameters"
            ));
            return Vec::new();
        };
        let changes = watched_changes(&params);
        if changes.is_empty() {
            log(format_args!(
                "ignoring didChangeWatchedFiles without a valid changes array"
            ));
            return Vec::new();
        }
        self.snippets.watched_files_changed(&changes);
        // An open buffer is authoritative: a watched-file event for it may be
        // a delayed notification for a save that `didSave` already handled, or
        // an unrelated on-disk edit that must not discard unsaved text.
        let total = changes.len();
        // A notification can contain changes for several configured apps.
        // Resolve every owner against the same foreground state, before any
        // update activates another app. Shared files keep their active owner.
        let mut groups: Vec<(Option<ProjectId>, Vec<WatchedFileChange>)> = Vec::new();
        let mut kept = 0;
        for change in changes {
            let file = self.host.resolve_file(&change.uri).ok();
            if self.workspace.get(&change.uri).is_some()
                || file
                    .as_ref()
                    .is_some_and(|file| self.workspace.get_by_file(file).is_some())
            {
                continue;
            }
            kept += 1;
            let project = file.map(|file| file.project().clone());
            if let Some((_, group)) = groups.iter_mut().find(|(id, _)| id == &project) {
                group.push(change);
            } else {
                groups.push((project, vec![change]));
            }
        }
        if kept < total {
            log(format_args!(
                "keeping overlays for {} open document(s) during watched file changes",
                total - kept
            ));
        }
        let mut outgoing = Vec::new();
        for (project, changes) in groups {
            if let Some(project) = project {
                if let Err(error) = self.host.activate_project(&project) {
                    log(format_args!("watched file project unavailable: {error}"));
                    continue;
                }
            }
            match self.host.watched_files_changed(&changes) {
                Ok(update) => outgoing.extend(self.apply_host_update(&update)),
                Err(error) => log(format_args!("watched file change failed: {error}")),
            }
        }
        outgoing
    }

    /// Registers dynamic watched-file events when a real project is open.
    ///
    /// Clients without a project (and the mock/`NullHost` server) must keep
    /// the current message flow, so no registration is sent for them.
    fn initialized(&self) -> Vec<Outgoing> {
        if !self.client.watched_files || !self.host.is_project_open() {
            return Vec::new();
        }
        let watchers: Vec<Value> = WATCH_GLOBS
            .iter()
            .map(|glob| json!({"globPattern": glob, "kind": WATCH_KIND_ALL}))
            .collect();
        vec![Outgoing::request(
            json!(WATCH_REGISTRATION_ID),
            "client/registerCapability",
            json!({
                "registrations": [{
                    "id": WATCH_REGISTRATION_ID,
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": {"watchers": watchers},
                }],
            }),
        )]
    }

    /// Refreshes the stored WXML snapshot for a saved template.
    fn save_wxml(&mut self, uri: &str, path: &ProjectPath) {
        let open = self.workspace.get(uri).map(|document| {
            (
                document.file().clone(),
                document.revision(),
                document.snapshot().snapshot(),
                document.text().to_owned(),
            )
        });
        let Some((file, revision, snapshot_id, open_text)) = open else {
            // No open workspace document: there is nothing to refresh, and
            // storing unbound save text would grow host snapshots without
            // bound.
            return;
        };
        // The open buffer is authoritative. `didChange` already delivered its
        // text; a redundant `didSave.text` (when a client sends one) must not
        // desynchronize the workspace snapshot from the host snapshot.
        let text = open_text;
        let snapshot = Arc::new(SourceSnapshot::new(
            file.clone(),
            revision,
            snapshot_id,
            text,
        ));
        self.host.set_wxml_snapshot(file, snapshot);
        if let Ok(target) = self.host.view_for(path) {
            self.workspace.set_view_revision(uri, target.view_revision);
        }
    }

    /// Answers a cancelled request immediately and drops its queued work.
    ///
    /// A running backend call is synchronous and cannot be interrupted; the
    /// shared flag makes its late result a no-op. The protocol loop must not
    /// wait for that call to answer `$/cancelRequest`, otherwise cancellation
    /// latency would equal the slowest backend call.
    fn cancel_request(&mut self, params: Option<&Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!("ignoring $/cancelRequest without parameters"));
            return Vec::new();
        };
        let Some(id) = params.as_object().and_then(|object| object.get("id")) else {
            log(format_args!("ignoring $/cancelRequest without an id"));
            return Vec::new();
        };
        let Some(key) = RequestKey::from_id(id) else {
            log(format_args!("ignoring $/cancelRequest with an invalid id"));
            return Vec::new();
        };
        let Some(inflight) = self.inflight.remove(&key) else {
            // A cancelled semantic request already answered by a previous
            // path must not leave auxiliary intent state behind.
            self.semantic_pending.remove(&key);
            log(format_args!("ignoring cancellation of an unknown request"));
            return Vec::new();
        };
        self.semantic_pending.remove(&key);
        inflight.cancel.store(true, Ordering::SeqCst);
        self.dispatch.cancel(&key);
        log(format_args!("cancelling request"));
        vec![Outgoing::failure(
            inflight.id,
            jsonrpc::error_codes::REQUEST_CANCELLED,
            "request cancelled",
        )]
    }

    /// Clears project-bound state when the active workspace folder is removed.
    ///
    /// A removed root must not keep serving cached types or diagnostics. Open
    /// buffers stay in the workspace so a later save/close can still be
    /// routed, but the host project is closed and every published diagnostic
    /// for a template is cleared. Re-adding exactly one root reopens it with
    /// the original initialization options; adding roots to a live project is
    /// not hot-applied (the multi-root registry belongs to the project host).
    pub(crate) fn did_change_workspace_folders(&mut self, params: Option<&Value>) -> Vec<Outgoing> {
        let Some(event) = params
            .and_then(|params| params.get("event"))
            .and_then(Value::as_object)
        else {
            log(format_args!(
                "ignoring didChangeWorkspaceFolders without an event"
            ));
            return Vec::new();
        };
        let added = workspace_folder_uris(event.get("added"));
        let removed = workspace_folder_uris(event.get("removed"));
        if removed.is_empty() {
            if added.is_empty() {
                log(format_args!(
                    "ignoring an empty didChangeWorkspaceFolders event"
                ));
                return Vec::new();
            }
            if added.len() == 1 && !self.host.is_project_open() {
                return self.open_replacement_workspace(&added[0]);
            }
            log(format_args!(
                "workspace folder additions are not hot-applied; restart the server to change roots"
            ));
            return Vec::new();
        }
        let removes_active = self
            .workspace_root
            .as_deref()
            .is_some_and(|root| removed.iter().any(|uri| same_root_uri(root, uri)));
        if !removes_active {
            log(format_args!(
                "ignoring removal of a workspace folder that is not active"
            ));
            return Vec::new();
        }
        self.close_removed_workspace(&added)
    }

    /// Drops project state and clears diagnostics after a root removal.
    fn close_removed_workspace(&mut self, added: &[String]) -> Vec<Outgoing> {
        let mut outgoing = Vec::new();
        let open: Vec<String> = self
            .workspace
            .documents()
            .map(|document| document.uri().to_owned())
            .collect();
        let mut clear_scripts = Vec::new();
        for uri in open {
            if template_document(self, &uri) {
                outgoing.push(Outgoing::notification(
                    "textDocument/publishDiagnostics",
                    json!({"uri": uri, "diagnostics": []}),
                ));
            } else if self.script_diagnostics_published.contains(&uri) {
                clear_scripts.push(uri);
            }
        }
        for uri in clear_scripts {
            outgoing.push(Outgoing::notification(
                "textDocument/publishDiagnostics",
                json!({"uri": uri, "diagnostics": []}),
            ));
        }
        // `inflight` is the only owner of response delivery. A semantic request
        // also has an auxiliary `semantic_pending` intent; clear that table
        // without answering it a second time.
        let inflight = std::mem::take(&mut self.inflight);
        for (key, request) in inflight {
            request.cancel.store(true, Ordering::SeqCst);
            self.dispatch.cancel(&key);
            outgoing.push(Outgoing::failure(
                request.id,
                jsonrpc::error_codes::REQUEST_CANCELLED,
                "workspace folder was removed",
            ));
        }
        self.semantic_pending.clear();
        for cancel in self.diagnostics_cancel.drain().map(|(_, cancel)| cancel) {
            cancel.store(true, Ordering::SeqCst);
        }
        self.backend_diagnostics.clear();
        self.script_diagnostics_inflight.clear();
        self.script_diagnostics_published.clear();
        self.dirty.clear();
        self.debounced_diagnostics.clear();
        self.synthetic_templates.clear();
        self.workspace_root = None;
        // Cached backend analyses belong to the removed project; clear them
        // and advance the backend epoch before any later request can answer.
        self.backend.invalidate();
        match self.host.open_root(None) {
            Ok(()) => {}
            Err(error) => log(format_args!("closing the removed project failed: {error}")),
        }
        self.workspace
            .set_project(self.host.project_id(), self.host.project_revision());
        if let Some(root) = added.first() {
            if added.len() > 1 {
                log(format_args!(
                    "opening only the first of {} added workspace folders; multi-root hot update is not supported",
                    added.len()
                ));
            }
            outgoing.extend(self.open_replacement_workspace(root));
        }
        outgoing
    }

    /// Opens one replacement root after a removal, keeping open buffers.
    fn open_replacement_workspace(&mut self, root: &str) -> Vec<Outgoing> {
        match self
            .host
            .open_root_with_options(Some(root), &self.initialization)
        {
            Ok(()) => {
                self.workspace_root = Some(root.to_owned());
                self.workspace
                    .set_project(self.host.project_id(), self.host.project_revision());
                log(format_args!("opened the replacement workspace folder"));
                return self.replay_open_documents();
            }
            Err(error) => log(format_args!("opening the workspace folder failed: {error}")),
        }
        Vec::new()
    }

    /// Replays open WXML snapshots and script/style/JSON overlays into a
    /// replacement project.
    ///
    /// `ProjectHost::open_root_with_options` clears the host snapshots and
    /// overlays, while the LSP workspace still owns the unsaved buffers. Every
    /// document identity is resolved again in the new project; old `FileId`s
    /// are never reused. Diagnostics are scheduled only after all overlays are
    /// back in place, so a replay cannot analyze against disk text.
    fn replay_open_documents(&mut self) -> Vec<Outgoing> {
        let documents: Vec<(String, String, u64)> = self
            .workspace
            .documents()
            .map(|document| {
                (
                    document.uri().to_owned(),
                    document.text().to_owned(),
                    document.client_version().get(),
                )
            })
            .collect();
        let mut updates = Vec::new();
        let mut templates = Vec::new();
        let mut scripts = Vec::new();
        for (uri, text, version) in documents {
            match self.host.route(&uri) {
                DocumentRoute::Wxml(path) => {
                    let Ok(file) = self.host.file_id(&path) else {
                        log(format_args!("replay skipped a template without a file id"));
                        continue;
                    };
                    if let Err(error) = self.workspace.open_file(
                        &uri,
                        file.clone(),
                        text,
                        Some(version),
                        ViewRevision::new(1),
                    ) {
                        log(format_args!("template replay failed: {error}"));
                        continue;
                    }
                    if let Some(document) = self.workspace.get(&uri) {
                        self.host
                            .set_wxml_snapshot(file, Arc::clone(document.snapshot()));
                        templates.push(uri.clone());
                    }
                    if let Ok(target) = self.host.view_for(&path) {
                        self.workspace.set_view_revision(&uri, target.view_revision);
                    }
                }
                DocumentRoute::Script(path)
                | DocumentRoute::Json(path)
                | DocumentRoute::Style(path) => {
                    let script = super::diagnostics::script_diagnostics_path(&path);
                    if let Ok(file) = self.host.file_id(&path) {
                        if let Err(error) = self.workspace.open_file(
                            &uri,
                            file,
                            text.clone(),
                            Some(version),
                            ViewRevision::new(0),
                        ) {
                            log(format_args!("overlay replay identity failed: {error}"));
                        }
                    } else {
                        log(format_args!(
                            "overlay replay skipped a document without a file id"
                        ));
                    }
                    match self.host.open_document(OpenDocument {
                        uri: uri.clone(),
                        text,
                        version: Some(version),
                    }) {
                        Ok(update) => {
                            updates.push(update);
                            if script {
                                scripts.push(uri.clone());
                            }
                        }
                        Err(error) => log(format_args!("overlay replay failed: {error}")),
                    }
                }
                DocumentRoute::Unknown => {}
            }
        }
        let mut outgoing = Vec::new();
        for update in updates {
            outgoing.extend(self.apply_host_update(&update));
        }
        for uri in templates {
            let _ = self.schedule_diagnostics(&uri);
        }
        for uri in scripts {
            let _ = self.schedule_script_diagnostics(&uri);
        }
        outgoing
    }
}

/// Reads workspace-folder URIs from one `didChangeWorkspaceFolders` list.
fn workspace_folder_uris(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |folders| {
            folders
                .iter()
                .take(64)
                .filter_map(|folder| {
                    let uri = folder.get("uri")?.as_str()?;
                    if uri.is_empty() || uri.len() > crate::workspace::MAX_URI_BYTES {
                        return None;
                    }
                    Some(uri.to_owned())
                })
                .collect()
        })
}

/// Compares root URIs without relying on the removed directory existing.
fn same_root_uri(left: &str, right: &str) -> bool {
    if left.trim_end_matches('/') == right.trim_end_matches('/') {
        return true;
    }
    match (
        wxml_project::root_path_from_uri(left),
        wxml_project::root_path_from_uri(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Identifier of the watched-files registration sent to the client.
const WATCH_REGISTRATION_ID: &str = "wxml-lsp-watch";

/// `FileChangeType` mask covering create, change and delete.
const WATCH_KIND_ALL: u8 = 7;

/// Glob patterns registered for `workspace/didChangeWatchedFiles`.
const WATCH_GLOBS: [&str; 30] = [
    "**/*.wxml",
    "**/*.json",
    "**/*.ts",
    "**/*.tsx",
    "**/*.js",
    "**/*.jsx",
    "**/*.mts",
    "**/*.cts",
    "**/*.mjs",
    "**/*.cjs",
    "**/*.wxss",
    "**/*.scss",
    "**/*.css",
    "**/*.wxs",
    "**/*.png",
    "**/*.jpg",
    "**/*.jpeg",
    "**/*.gif",
    "**/*.svg",
    "**/*.webp",
    "**/*.bmp",
    "**/*.ico",
    "**/*.mp4",
    "**/*.mov",
    "**/*.webm",
    "**/*.mp3",
    "**/*.wav",
    "**/*.m4a",
    "**/*.ogg",
    "**/*.ttf",
];

/// Largest number of `didChangeWatchedFiles` entries processed per call.
const MAX_WATCHED_CHANGES: usize = 4_096;

/// Reads the document URI of a `didSave` notification.
fn saved_uri(params: &Value) -> Option<&str> {
    params
        .as_object()?
        .get("textDocument")?
        .as_object()?
        .get("uri")?
        .as_str()
}

/// Parses the `changes` array of a `didChangeWatchedFiles` notification.
///
/// Malformed entries are skipped and the processed count is bounded so a
/// hostile client cannot force unbounded rediscovery work.
fn watched_changes(params: &Value) -> Vec<WatchedFileChange> {
    let Some(changes) = params
        .as_object()
        .and_then(|object| object.get("changes"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    changes
        .iter()
        .take(MAX_WATCHED_CHANGES)
        .filter_map(|change| {
            let change = change.as_object()?;
            let uri = change.get("uri")?.as_str()?;
            if uri.is_empty() || uri.len() > crate::workspace::MAX_URI_BYTES {
                return None;
            }
            let kind = WatchedChangeKind::from_lsp(change.get("type")?.as_i64()?)?;
            Some(WatchedFileChange {
                uri: uri.to_owned(),
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        MAX_WATCHED_CHANGES, diagnostics_debounce_override, same_root_uri, saved_uri,
        style_initialization_options, watched_changes, workspace_folder_uris,
    };
    use crate::project_host::WatchedChangeKind;

    #[test]
    fn parses_style_initialization_options() {
        let (config, policy) = style_initialization_options(Some(&json!({
            "initializationOptions": {
                "style": {
                    "globalStyles": ["styles/theme.scss"],
                    "diagnostics": {"resources": false, "classes": true}
                }
            }
        })));
        assert_eq!(
            config
                .global_styles
                .iter()
                .map(wxml_project::ProjectPath::as_str)
                .collect::<Vec<_>>(),
            vec!["styles/theme.scss"]
        );
        assert!(!policy.resources);
        assert!(policy.classes);

        let (invalid, _) = style_initialization_options(Some(&json!({
            "initializationOptions": {"style": {"globalStyles": ["/abs/theme.scss", 3]}}
        })));
        assert_eq!(invalid.global_styles, Vec::new());
        assert_eq!(
            invalid.invalid_global_styles,
            vec!["/abs/theme.scss", "<non-string globalStyles entry>"]
        );

        let (_, defaults) = style_initialization_options(None);
        assert!(defaults.resources);
        assert!(!defaults.classes);
    }

    #[test]
    fn reads_saved_uri_only_from_a_text_document() {
        assert_eq!(
            saved_uri(&json!({"textDocument": {"uri": "file:///a.wxml"}})),
            Some("file:///a.wxml")
        );
        for params in [
            json!({}),
            json!({"textDocument": {}}),
            json!({"textDocument": {"uri": 1}}),
            json!({"textDocument": null}),
            json!(null),
        ] {
            assert_eq!(saved_uri(&params), None, "params {params}");
        }
    }

    #[test]
    fn reads_watched_changes_and_skips_malformed_entries() {
        let changes = watched_changes(&json!({
            "changes": [
                {"uri": "file:///a.ts", "type": 1},
                {"uri": "file:///b.json", "type": 2},
                {"uri": "file:///c.wxml", "type": 3},
                {"uri": "file:///d.ts", "type": 4},
                {"uri": 1, "type": 2},
                {"uri": "file:///e.ts"},
                null,
            ],
        }));
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].kind, WatchedChangeKind::Created);
        assert_eq!(changes[1].kind, WatchedChangeKind::Changed);
        assert_eq!(changes[2].kind, WatchedChangeKind::Deleted);

        for params in [
            json!({}),
            json!({"changes": {}}),
            json!({"changes": null}),
            json!(null),
        ] {
            assert!(watched_changes(&params).is_empty(), "params {params}");
        }
    }

    #[test]
    fn bounds_the_number_of_watched_changes() {
        let changes: Vec<_> = (0..=MAX_WATCHED_CHANGES)
            .map(|_| json!({"uri": "file:///a.ts", "type": 2}))
            .collect();
        let parsed = watched_changes(&json!({"changes": changes}));
        assert_eq!(parsed.len(), MAX_WATCHED_CHANGES);
    }

    #[test]
    fn reads_bounded_workspace_folder_uris() {
        let folders = workspace_folder_uris(Some(&json!([
            {"uri": "file:///workspace/a"},
            {"uri": "file:///workspace/b/"},
            {"uri": 3},
            null,
        ])));
        assert_eq!(folders, vec!["file:///workspace/a", "file:///workspace/b/"]);
        assert_eq!(workspace_folder_uris(None), Vec::<String>::new());
        assert_eq!(
            workspace_folder_uris(Some(&json!({}))),
            Vec::<String>::new()
        );
        assert_eq!(
            workspace_folder_uris(Some(&json!([{"uri": ""}, {"uri": "x".repeat(9_000)}]))),
            Vec::<String>::new()
        );
        assert!(same_root_uri("file:///workspace/", "file:///workspace"));
        assert!(same_root_uri(
            "file:///workspace/a%20b",
            "file:///workspace/a%20b/"
        ));
        assert!(!same_root_uri("file:///workspace/a", "file:///workspace/b"));
    }

    #[test]
    fn reads_a_bounded_diagnostics_debounce_override() {
        assert_eq!(
            diagnostics_debounce_override(Some(&json!({
                "initializationOptions": {"wxml": {"diagnostics": {"debounceMs": 50}}}
            }))),
            Some(std::time::Duration::from_millis(50))
        );
        assert_eq!(
            diagnostics_debounce_override(Some(&json!({
                "initializationOptions": {"diagnostics": {"debounceMs": 0}}
            }))),
            Some(std::time::Duration::ZERO)
        );
        for params in [
            json!({}),
            json!({"initializationOptions": {"wxml": {"diagnostics": {"debounceMs": 5001}}}}),
            json!({"initializationOptions": {"wxml": {"diagnostics": {"debounceMs": "50"}}}}),
            json!({"initializationOptions": {"wxml": {"diagnostics": {}}}}),
        ] {
            assert_eq!(
                diagnostics_debounce_override(Some(&params)),
                None,
                "{params}"
            );
        }
    }
}
