//! Typed native method names and outbound request shells.
//!
//! The enum is the only way to name a wire method; there is deliberately no
//! `call(method, params)` escape hatch. The allow-list itself is data in
//! [`crate::config`], and a unit test keeps both in sync.

use crate::native::rpc::RawPayload;

macro_rules! native_methods {
    ($($variant:ident => $wire:literal,)+) => {
        /// Every method permitted by the frozen F0 allow-list.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum NativeMethod {
            $(
                #[doc = concat!("The native `", $wire, "` method.")]
                $variant,
            )+
        }

        impl NativeMethod {
            /// All variants in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// Returns the exact wire name sent in `method`.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }
    };
}

native_methods! {
    Initialize => "initialize",
    CreateSnapshot => "createSnapshot",
    UpdateSnapshot => "updateSnapshot",
    Release => "release",
    CreateSourceFile => "createSourceFile",
    RetainSourceFile => "retainSourceFile",
    ReleaseSourceFile => "releaseSourceFile",
    GetCachedSourceFile => "getCachedSourceFile",
    GetSourceFile => "getSourceFile",
    GetSourceFileNames => "getSourceFileNames",
    GetSourceFileMetadata => "getSourceFileMetadata",
    GetSemanticDiagnostics => "getSemanticDiagnostics",
    GetSyntacticDiagnostics => "getSyntacticDiagnostics",
    GetTypeAtPosition => "getTypeAtPosition",
    GetTypeAtLocation => "getTypeAtLocation",
    GetTypeOfSymbol => "getTypeOfSymbol",
    GetDeclaredTypeOfSymbol => "getDeclaredTypeOfSymbol",
    TypeToString => "typeToString",
    GetSymbolAtPosition => "getSymbolAtPosition",
    GetSymbolAtLocation => "getSymbolAtLocation",
    GetSymbolOfNode => "getSymbolOfNode",
    GetDocumentationComment => "getDocumentationComment",
    GetJsDocTags => "getJsDocTags",
    GetReferencedSymbolsForNode => "getReferencedSymbolsForNode",
    GetCompletionsAtPosition => "getCompletionsAtPosition",
    GetDefaultProjectForFile => "getDefaultProjectForFile",
}

/// One typed outbound request. Construction does not send anything.
#[derive(Debug, Clone)]
pub struct NativeRequest {
    method: NativeMethod,
    params: RawPayload,
}

impl NativeRequest {
    /// Creates a request shell for `P3-N` to serialize.
    #[must_use]
    pub const fn new(method: NativeMethod, params: RawPayload) -> Self {
        Self { method, params }
    }

    /// Returns the typed method.
    #[must_use]
    pub const fn method(&self) -> NativeMethod {
        self.method
    }

    /// Returns the raw params value.
    #[must_use]
    pub const fn params(&self) -> &RawPayload {
        &self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ALLOWED_METHODS;

    #[test]
    fn typed_methods_match_the_frozen_allowlist() {
        let names: Vec<&str> = NativeMethod::ALL
            .iter()
            .map(|method| method.as_str())
            .collect();
        assert_eq!(names.as_slice(), ALLOWED_METHODS);
    }
}
