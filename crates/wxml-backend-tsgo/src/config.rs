//! Pinned engine identity and native method allow-list, modelled as data.
//!
//! Every value here mirrors the frozen F0 contract in
//! `docs/reference/tsgo-native-contract.md`. This module does not locate,
//! download, verify, start or talk to an engine; it only freezes the identity
//! that `P3-N`/`P4` must check before a session may be used.
//!
//! [`PinnedEngine`] has no public fields and only [`PinnedEngine::pinned`] may
//! produce it, so downstream code cannot forge a different identity. The
//! launch data is intentionally inert: [`PinnedEngine::launch_plan`] returns
//! argv for review and tests after validating the sandbox root, and never
//! spawns a process.
//!
//! Field names are not a manifest schema. F3 owns the manifest/index schema;
//! this struct only carries runtime identity and policy data.

use thiserror::Error;

/// Pinned engine release (F0-01). `7.0.2` is explicitly excluded.
pub const ENGINE_VERSION: &str = "7.1.0-beta";

/// Exact accepted `lib/tsc --version` output.
///
/// F0 compares the command output after trimming trailing ASCII whitespace.
pub const ENGINE_VERSION_LINE: &str = "Version 7.1.0-beta";

/// Distribution package the pinned executable comes from.
pub const ENGINE_PACKAGE: &str = "@typescript/typescript-linux-x64";

/// Source commit recorded by the package metadata.
pub const ENGINE_GIT_HEAD: &str = "3e9236361fa97a9e6c685f4833ac457ae2d8c50a";

/// SHA-256 of the pinned release tarball.
pub const ENGINE_TARBALL_SHA256: &str =
    "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855";

/// SRI integrity string for the pinned release tarball.
pub const ENGINE_SRI: &str = "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==";

/// Installation-relative path of the native executable.
pub const ENGINE_EXECUTABLE_RELATIVE_PATH: &str = "engine/lib/tsc";

/// SHA-256 of the pinned executable.
pub const ENGINE_EXECUTABLE_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";

/// Unpacked size of the pinned package in bytes.
pub const ENGINE_UNPACKED_BYTES: u64 = 28_450_796;

/// Number of files in the pinned package.
pub const ENGINE_FILE_COUNT: u32 = 122;

/// Native AST schema version frozen by F0.
pub const AST_PROTOCOL_VERSION: u32 = 9;

/// Native AST header size in bytes frozen by F0.
pub const AST_HEADER_BYTES: usize = 64;

/// Native AST node record size in bytes.
pub const AST_NODE_STRIDE_BYTES: usize = 28;

/// Maximum accepted `Content-Length` header block.
pub const MAX_FRAME_HEADER_BYTES: usize = 8 * 1024;

/// Maximum outbound request body.
pub const MAX_REQUEST_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Maximum accepted response body.
pub const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Maximum accepted callback request or response body.
pub const MAX_CALLBACK_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Maximum accepted JSON nesting depth.
pub const MAX_JSON_DEPTH: usize = 128;

/// Maximum array entries or object members in one message.
pub const MAX_MESSAGE_ENTRIES: usize = 100_000;

/// Maximum length of one JSON string in bytes.
pub const MAX_STRING_BYTES: usize = 32 * 1024 * 1024;

/// Maximum number of AST nodes accepted from one source file.
pub const MAX_AST_NODES: usize = 1_000_000;

/// Maximum number of AST string-pool entries accepted from one source file.
pub const MAX_AST_STRINGS: usize = 1_000_000;

/// Maximum concurrent in-flight client requests.
pub const MAX_IN_FLIGHT_REQUESTS: usize = 8;

/// Maximum queued client requests while all in-flight slots are busy.
pub const MAX_PENDING_QUEUE: usize = 64;

/// Maximum concurrent in-flight engine callbacks.
pub const MAX_IN_FLIGHT_CALLBACKS: usize = 16;

/// Default deadline for one native request, in milliseconds.
pub const DEFAULT_REQUEST_DEADLINE_MS: u64 = 15_000;

/// Deadline for snapshot creation or update, in milliseconds.
pub const SNAPSHOT_DEADLINE_MS: u64 = 30_000;

/// Callback names passed to the engine, in the frozen order.
pub const CALLBACK_NAMES: &[&str] = &[
    "readFile",
    "fileExists",
    "directoryExists",
    "getAccessibleEntries",
    "realpath",
    "writeFile:noop",
    "removeFile:noop",
];

/// Frozen callback argument string (without the `--callbacks=` prefix).
pub const CALLBACK_ARGUMENT: &str = "readFile,fileExists,directoryExists,\
getAccessibleEntries,realpath,writeFile:noop,removeFile:noop";

/// Methods permitted by the frozen F0 allow-list, in wire order.
///
/// The typed enum in [`crate::native::protocol`] is the only construction
/// path for a native request; this slice is data for validation and tests.
pub const ALLOWED_METHODS: &[&str] = &[
    "initialize",
    "createSnapshot",
    "updateSnapshot",
    "release",
    "createSourceFile",
    "retainSourceFile",
    "releaseSourceFile",
    "getCachedSourceFile",
    "getSourceFile",
    "getSourceFileNames",
    "getSourceFileMetadata",
    "getSemanticDiagnostics",
    "getSyntacticDiagnostics",
    "getTypeAtPosition",
    "getTypeAtLocation",
    "getTypeOfSymbol",
    "getDeclaredTypeOfSymbol",
    "typeToString",
    "getSymbolAtPosition",
    "getSymbolAtLocation",
    "getSymbolOfNode",
    "getDocumentationComment",
    "getJsDocTags",
    "getReferencedSymbolsForNode",
    "getCompletionsAtPosition",
    "getDefaultProjectForFile",
];

/// Methods that must never be sent by the Rust client (F0-13).
pub const FORBIDDEN_METHODS: &[&str] = &[
    "emit",
    "emitToString",
    "getJavaScriptEmit",
    "getDeclarationEmit",
    "build",
    "buildReferences",
    "cleanBuild",
    "cleanReferences",
    "createBuildOrchestrator",
    "disposeBuildOrchestrator",
    "startCPUProfile",
    "stopCPUProfile",
    "saveHeapProfile",
    "createModuleResolver",
    "releaseModuleResolver",
    "resolveModuleName",
    "parseCommandLine",
    "readConfigFile",
    "parseConfigFile",
    "parseJsonConfigFileContent",
    "transpileModule",
    "transpileModuleFromFile",
    "transpileDeclaration",
    "transpileDeclarationFromFile",
    "getImportAdderEdits",
    "formatNodeForInsertion",
    "printNode",
    "getCurrentLanguageServerSnapshot",
    "batchRequests",
];

/// Identity and policy data for the only accepted native engine.
///
/// Fields are private so only [`Self::pinned`] can produce an identity. The
/// values are runtime pins, not a manifest schema (F3 owns that schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedEngine {
    package: &'static str,
    version: &'static str,
    version_line: &'static str,
    git_head: &'static str,
    tarball_sha256: &'static str,
    sri: &'static str,
    executable: &'static str,
    executable_sha256: &'static str,
    unpacked_bytes: u64,
    file_count: u32,
    ast_protocol_version: u32,
    ast_header_bytes: usize,
    callbacks: &'static [&'static str],
    allowed_methods: &'static [&'static str],
}

const PINNED_ENGINE: PinnedEngine = PinnedEngine {
    package: ENGINE_PACKAGE,
    version: ENGINE_VERSION,
    version_line: ENGINE_VERSION_LINE,
    git_head: ENGINE_GIT_HEAD,
    tarball_sha256: ENGINE_TARBALL_SHA256,
    sri: ENGINE_SRI,
    executable: ENGINE_EXECUTABLE_RELATIVE_PATH,
    executable_sha256: ENGINE_EXECUTABLE_SHA256,
    unpacked_bytes: ENGINE_UNPACKED_BYTES,
    file_count: ENGINE_FILE_COUNT,
    ast_protocol_version: AST_PROTOCOL_VERSION,
    ast_header_bytes: AST_HEADER_BYTES,
    callbacks: CALLBACK_NAMES,
    allowed_methods: ALLOWED_METHODS,
};

/// Data-only launch description for review and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Installation-relative executable path.
    pub executable: &'static str,
    /// Complete argument vector, excluding `argv[0]`.
    pub args: Vec<String>,
}

/// Failure to build an inert [`LaunchPlan`].
#[derive(Debug, Error)]
pub enum LaunchError {
    /// The sandbox root is empty, relative, contains control characters or
    /// unresolved `.`/`..` components.
    #[error("invalid sandbox root: {reason}")]
    InvalidSandboxRoot {
        /// Stable description of the failed invariant.
        reason: &'static str,
    },
}

impl PinnedEngine {
    /// Returns the frozen engine identity.
    ///
    /// This is the only production constructor; fields are private.
    #[must_use]
    pub const fn pinned() -> Self {
        PINNED_ENGINE
    }

    /// Distribution package name.
    #[must_use]
    pub const fn package(&self) -> &'static str {
        self.package
    }

    /// Release version.
    #[must_use]
    pub const fn version(&self) -> &'static str {
        self.version
    }

    /// Exact accepted `--version` output.
    #[must_use]
    pub const fn version_line(&self) -> &'static str {
        self.version_line
    }

    /// Source commit from package metadata.
    #[must_use]
    pub const fn git_head(&self) -> &'static str {
        self.git_head
    }

    /// SHA-256 of the release tarball.
    #[must_use]
    pub const fn tarball_sha256(&self) -> &'static str {
        self.tarball_sha256
    }

    /// SRI integrity string for the release tarball.
    #[must_use]
    pub const fn sri(&self) -> &'static str {
        self.sri
    }

    /// Installation-relative executable path.
    #[must_use]
    pub const fn executable(&self) -> &'static str {
        self.executable
    }

    /// SHA-256 of the executable.
    #[must_use]
    pub const fn executable_sha256(&self) -> &'static str {
        self.executable_sha256
    }

    /// Unpacked package size in bytes.
    #[must_use]
    pub const fn unpacked_bytes(&self) -> u64 {
        self.unpacked_bytes
    }

    /// Number of files in the pinned package.
    #[must_use]
    pub const fn file_count(&self) -> u32 {
        self.file_count
    }

    /// Native AST protocol version.
    #[must_use]
    pub const fn ast_protocol_version(&self) -> u32 {
        self.ast_protocol_version
    }

    /// Native AST header size in bytes.
    #[must_use]
    pub const fn ast_header_bytes(&self) -> usize {
        self.ast_header_bytes
    }

    /// Callback names registered at startup.
    #[must_use]
    pub const fn callbacks(&self) -> &'static [&'static str] {
        self.callbacks
    }

    /// Methods the client may send.
    #[must_use]
    pub const fn allowed_methods(&self) -> &'static [&'static str] {
        self.allowed_methods
    }

    /// Returns whether `method` is on the frozen allow-list.
    #[must_use]
    pub fn allows_method(&self, method: &str) -> bool {
        self.allowed_methods.contains(&method)
    }

    /// Returns the frozen argv for `sandbox_root`; never spawns a process.
    ///
    /// `sandbox_root` must be an absolute path with no control characters and
    /// no unresolved `.`/`..` components. `P3-N` still owns the final
    /// normalization and the empty-sandbox creation.
    pub fn launch_plan(&self, sandbox_root: &str) -> Result<LaunchPlan, LaunchError> {
        validate_sandbox_root(sandbox_root)?;

        Ok(LaunchPlan {
            executable: self.executable,
            args: vec![
                "--api".to_owned(),
                "--async".to_owned(),
                "--cwd".to_owned(),
                sandbox_root.to_owned(),
                "--useCaseSensitiveFileNames=true".to_owned(),
                format!("--callbacks={CALLBACK_ARGUMENT}"),
            ],
        })
    }
}

/// Rejects roots that cannot be an already-normalized absolute sandbox path.
fn validate_sandbox_root(sandbox_root: &str) -> Result<(), LaunchError> {
    if sandbox_root.is_empty() {
        return Err(LaunchError::InvalidSandboxRoot {
            reason: "must not be empty",
        });
    }

    if sandbox_root.chars().any(char::is_control) {
        return Err(LaunchError::InvalidSandboxRoot {
            reason: "must not contain NUL or control characters",
        });
    }

    if !sandbox_root.starts_with('/') {
        return Err(LaunchError::InvalidSandboxRoot {
            reason: "must be an absolute path",
        });
    }

    if sandbox_root
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err(LaunchError::InvalidSandboxRoot {
            reason: "must not contain unresolved . or .. components",
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_identity_uses_literal_f0_values() {
        let engine = PinnedEngine::pinned();

        assert_eq!(engine.package(), "@typescript/typescript-linux-x64");
        assert_eq!(engine.version(), "7.1.0-beta");
        assert_eq!(engine.version_line(), "Version 7.1.0-beta");
        assert_eq!(
            engine.git_head(),
            "3e9236361fa97a9e6c685f4833ac457ae2d8c50a"
        );
        assert_eq!(
            engine.tarball_sha256(),
            "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855"
        );
        assert_eq!(
            engine.sri(),
            "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ=="
        );
        assert_eq!(engine.executable(), "engine/lib/tsc");
        assert_eq!(
            engine.executable_sha256(),
            "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70"
        );
        assert_eq!(engine.unpacked_bytes(), 28_450_796);
        assert_eq!(engine.file_count(), 122);
        assert_eq!(engine.ast_protocol_version(), 9);
        assert_eq!(engine.ast_header_bytes(), 64);
        assert!(engine.allows_method("createSnapshot"));
        assert!(!engine.allows_method("emit"));
    }

    #[test]
    fn launch_plan_matches_the_frozen_six_argument_argv() {
        let plan = PinnedEngine::pinned()
            .launch_plan("/tmp/tsgo-sandbox")
            .expect("valid absolute sandbox root");

        assert_eq!(plan.executable, "engine/lib/tsc");
        assert_eq!(
            plan.args,
            vec![
                "--api".to_owned(),
                "--async".to_owned(),
                "--cwd".to_owned(),
                "/tmp/tsgo-sandbox".to_owned(),
                "--useCaseSensitiveFileNames=true".to_owned(),
                "--callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,writeFile:noop,removeFile:noop".to_owned(),
            ]
        );
    }

    #[test]
    fn launch_plan_rejects_unusable_sandbox_roots() {
        let engine = PinnedEngine::pinned();

        assert!(engine.launch_plan("").is_err());
        assert!(engine.launch_plan("relative/path").is_err());
        assert!(engine.launch_plan("/tmp/bad\0root").is_err());
        assert!(engine.launch_plan("/tmp/../escape").is_err());
    }

    #[test]
    fn callback_data_is_consistent() {
        assert_eq!(CALLBACK_NAMES.join(","), CALLBACK_ARGUMENT);
        assert!(CALLBACK_NAMES.contains(&"writeFile:noop"));
        assert!(CALLBACK_NAMES.contains(&"removeFile:noop"));
        assert!(!CALLBACK_NAMES.contains(&"appendFile"));
        assert!(!CALLBACK_NAMES.contains(&"chtimes"));
    }

    #[test]
    fn allow_and_deny_lists_are_unique_and_disjoint() {
        let unique: std::collections::BTreeSet<_> = ALLOWED_METHODS.iter().copied().collect();
        assert_eq!(unique.len(), ALLOWED_METHODS.len());
        assert_eq!(ALLOWED_METHODS.len(), 26);

        for method in FORBIDDEN_METHODS {
            assert!(
                !ALLOWED_METHODS.contains(method),
                "method {method:?} is both allowed and forbidden"
            );
        }
    }
}
