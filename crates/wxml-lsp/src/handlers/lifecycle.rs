//! Lifecycle methods and method dispatch.

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use wxml_types::CapabilityStatus;

use super::{log_safe_method, log_safe_text, status_label};
use crate::jsonrpc::{self, Outgoing, RequestKey};
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
            "initialized" => Vec::new(),
            "textDocument/didOpen" => self.did_open(params),
            "textDocument/didChange" => self.did_change(params),
            "textDocument/didClose" => self.did_close(params),
            // Phase 0 dispatch skeleton: the notifications are parsed and
            // logged here so later worktrees only fill in the behavior.
            "textDocument/didSave" => Self::did_save(params.as_ref()),
            "workspace/didChangeWatchedFiles" => Self::did_change_watched_files(params.as_ref()),
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
        let root = requested_root(params);
        match self.host.open_root(root) {
            Ok(()) => {
                if root.is_some() {
                    let project_id = self.host.project_id();
                    let revision = self.host.project_revision();
                    self.workspace.set_project(project_id, revision);
                }
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
    /// Phase 0 only validates and logs the notification; snapshot refresh and
    /// diagnostics are added by the Zed synchronization worktree.
    pub(crate) fn did_save(params: Option<&Value>) -> Vec<Outgoing> {
        match params.and_then(saved_uri) {
            Some(uri) => log(format_args!(
                "didSave for {} acknowledged (no-op in this phase)",
                log_safe_text(uri, MAX_LOG_URI_BYTES)
            )),
            None => log(format_args!(
                "ignoring didSave without a valid text document"
            )),
        }
        Vec::new()
    }

    /// Handles `workspace/didChangeWatchedFiles`.
    ///
    /// Phase 0 only validates and logs the notification; cache invalidation is
    /// added by the Zed synchronization worktree.
    pub(crate) fn did_change_watched_files(params: Option<&Value>) -> Vec<Outgoing> {
        match params.and_then(watched_change_count) {
            Some(count) => log(format_args!(
                "didChangeWatchedFiles with {count} change(s) acknowledged (no-op in this phase)"
            )),
            None => log(format_args!(
                "ignoring didChangeWatchedFiles without a valid changes array"
            )),
        }
        Vec::new()
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

/// Longest document URI written to stderr for an acknowledged notification.
const MAX_LOG_URI_BYTES: usize = 256;

/// Reads the document URI of a `didSave` notification.
fn saved_uri(params: &Value) -> Option<&str> {
    params
        .as_object()?
        .get("textDocument")?
        .as_object()?
        .get("uri")?
        .as_str()
}

/// Returns the number of entries in a `didChangeWatchedFiles` notification.
fn watched_change_count(params: &Value) -> Option<usize> {
    params.as_object()?.get("changes")?.as_array().map(Vec::len)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{saved_uri, watched_change_count};

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
    fn reads_watched_change_count_only_from_an_array() {
        assert_eq!(
            watched_change_count(&json!({"changes": [{"uri": "file:///a", "type": 2}]})),
            Some(1)
        );
        assert_eq!(watched_change_count(&json!({"changes": []})), Some(0));
        for params in [
            json!({}),
            json!({"changes": {}}),
            json!({"changes": null}),
            json!(null),
        ] {
            assert_eq!(watched_change_count(&params), None, "params {params}");
        }
    }
}
