//! Lifecycle methods and method dispatch.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use wxml_types::{CapabilityStatus, ProjectId, SourceSnapshot};

use super::{log_safe_method, log_safe_text, status_label, template_document};
use crate::configuration::InitializationOptions;
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::project_host::{DocumentRoute, HostError, WatchedChangeKind, WatchedFileChange};
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

/// Builds the `InitializeResult`.
fn initialize_result(server: &Server) -> Value {
    json!({
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
            "hoverProvider": server.capabilities.hover.is_supported(),
            "definitionProvider": server.capabilities.definition.is_supported(),
            // Static template completion is always available; the flag only
            // covers Node-backed expression/event queries.
            "completionProvider": {
                "triggerCharacters": ["<", " ", ":", "\"", "'", ".", "{", "="],
                "resolveProvider": false,
            },
        },
        "serverInfo": {
            "name": "wxml-lsp",
            "version": crate::version(),
        },
    })
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
            "textDocument/completion" => self.completion(id, params),
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
            "$/cancelRequest" => {
                self.cancel_request(params.as_ref());
                Vec::new()
            }
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
        self.client_watched_files = params
            .and_then(|params| params.get("capabilities"))
            .and_then(|capabilities| capabilities.get("workspace"))
            .and_then(|workspace| workspace.get("didChangeWatchedFiles"))
            .and_then(|watched| watched.get("dynamicRegistration"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let root = requested_root(params);
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
            DocumentRoute::Script(_) | DocumentRoute::Json(_) => {
                match self.host.save_document(uri) {
                    Ok(update) => self.apply_host_update(&update),
                    Err(error) => log(format_args!("overlay save failed: {error}")),
                }
                Vec::new()
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
        for (project, changes) in groups {
            if let Some(project) = project {
                if let Err(error) = self.host.activate_project(&project) {
                    log(format_args!("watched file project unavailable: {error}"));
                    continue;
                }
            }
            match self.host.watched_files_changed(&changes) {
                Ok(update) => self.apply_host_update(&update),
                Err(error) => log(format_args!("watched file change failed: {error}")),
            }
        }
        Vec::new()
    }

    /// Registers dynamic watched-file events when a real project is open.
    ///
    /// Clients without a project (and the mock/`NullHost` server) must keep
    /// the current message flow, so no registration is sent for them.
    fn initialized(&self) -> Vec<Outgoing> {
        if !self.client_watched_files || !self.host.is_project_open() {
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
    fn save_wxml(&mut self, uri: &str, path: &wxml_project::ProjectPath) {
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

    /// Sets the cancel flag of one in-flight request.
    fn cancel_request(&self, params: Option<&Value>) {
        let Some(params) = params else {
            log(format_args!("ignoring $/cancelRequest without parameters"));
            return;
        };
        let Some(id) = params.as_object().and_then(|object| object.get("id")) else {
            log(format_args!("ignoring $/cancelRequest without an id"));
            return;
        };
        let Some(key) = RequestKey::from_id(id) else {
            log(format_args!("ignoring $/cancelRequest with an invalid id"));
            return;
        };
        match self.inflight.get(&key) {
            Some(inflight) => {
                inflight.cancel.store(true, Ordering::SeqCst);
                log(format_args!("cancelling request"));
            }
            None => log(format_args!("ignoring cancellation of an unknown request")),
        }
    }
}

/// Identifier of the watched-files registration sent to the client.
const WATCH_REGISTRATION_ID: &str = "wxml-lsp-watch";

/// `FileChangeType` mask covering create, change and delete.
const WATCH_KIND_ALL: u8 = 7;

/// Glob patterns registered for `workspace/didChangeWatchedFiles`.
const WATCH_GLOBS: [&str; 11] = [
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

    use super::{MAX_WATCHED_CHANGES, saved_uri, watched_changes};
    use crate::project_host::WatchedChangeKind;

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
}
