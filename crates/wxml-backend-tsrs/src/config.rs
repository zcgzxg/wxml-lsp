//! Pinned in-process engine identity and native method allow-list, modelled as
//! data.
//!
//! The TypeScript standard library and compiler are linked into this process
//! through `tsrs_*`; there is no executable, tarball or launch argv to verify.
//! This module freezes the TypeScript version and AST schema that the rest of
//! the backend relies on, plus the method allow-list.
//!
//! [`PinnedEngine`] has no public fields and only [`PinnedEngine::pinned`] may
//! produce it, so downstream code cannot forge a different identity.

/// Pinned TypeScript release. `7.0.2` is explicitly excluded.
pub const ENGINE_VERSION: &str = "7.1.0-beta";

/// Native AST schema version frozen by the contract.
pub const AST_PROTOCOL_VERSION: u32 = 9;

/// Native AST header size in bytes frozen by the contract.
pub const AST_HEADER_BYTES: usize = 64;

/// Native AST node record size in bytes.
pub const AST_NODE_STRIDE_BYTES: usize = 28;

/// Maximum outbound request body.
pub const MAX_REQUEST_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Maximum number of AST nodes accepted from one source file.
pub const MAX_AST_NODES: usize = 1_000_000;

/// Maximum number of AST string-pool entries accepted from one source file.
pub const MAX_AST_STRINGS: usize = 1_000_000;

/// Maximum number of engine snapshots that may stay alive in one session.
///
/// The contract freezes this initial budget; a session must release an old
/// snapshot before it would exceed the bound.
pub const MAX_LIVE_SNAPSHOTS: usize = 8;

/// Default deadline for one native request, in milliseconds.
pub const DEFAULT_REQUEST_DEADLINE_MS: u64 = 15_000;

/// Deadline for snapshot creation or update, in milliseconds.
pub const SNAPSHOT_DEADLINE_MS: u64 = 30_000;

/// Methods permitted by the frozen allow-list, in wire order.
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

/// Methods that must never be sent by the Rust client.
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

/// Identity and policy data for the only accepted in-process engine.
///
/// Fields are private so only [`Self::pinned`] can produce an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedEngine {
    version: &'static str,
    allowed_methods: &'static [&'static str],
}

const PINNED_ENGINE: PinnedEngine = PinnedEngine {
    version: ENGINE_VERSION,
    allowed_methods: ALLOWED_METHODS,
};

impl PinnedEngine {
    /// Returns the frozen engine identity.
    ///
    /// This is the only production constructor; fields are private.
    #[must_use]
    pub const fn pinned() -> Self {
        PINNED_ENGINE
    }

    /// TypeScript release.
    #[must_use]
    pub const fn version(&self) -> &'static str {
        self.version
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_identity_uses_literal_f0_values() {
        let engine = PinnedEngine::pinned();

        assert_eq!(engine.version(), "7.1.0-beta");
        assert!(engine.allows_method("createSnapshot"));
        assert!(!engine.allows_method("emit"));
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
