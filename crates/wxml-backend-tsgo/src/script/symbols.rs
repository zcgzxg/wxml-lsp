//! Stable script symbols built from conservative syntactic evidence.
//!
//! A symbol always keeps the exact original token range in both UTF-8 bytes and
//! UTF-16 code units. Its [`DefinitionOrigin`] records whether the declaration
//! lives in the analyzed script, in an imported behavior file or in a shape
//! whose runtime origin cannot be proven.
//!
//! Rename safety is conservative: a symbol is only editable when every fact it
//! depends on came from an original token in the analyzed file, the
//! registration is unique and the surrounding overlay is complete. Unsafe
//! symbols return `None` from [`ScriptSymbol::rename_edit`]; no partial edit
//! plan is produced.

use crate::native::strings::Utf16Offset;
use crate::shadow::expression;
use crate::shadow::mapping::SourceRange;

use super::ScriptError;

/// Reusable UTF-8 byte → UTF-16 code-unit index for one text snapshot.
///
/// Only non-ASCII character boundaries are stored, so the index is empty for
/// ASCII-only scripts and every conversion is a binary search over the actual
/// non-ASCII boundaries. This keeps per-symbol range conversion linear in the
/// script size instead of quadratic in the member count.
#[derive(Debug, Clone, Default)]
pub(crate) struct Utf16Index {
    boundaries: Vec<(usize, i32)>,
}

impl Utf16Index {
    /// Builds the boundary index once for `text`.
    pub(crate) fn new(text: &str) -> Self {
        let mut boundaries = Vec::new();
        let mut units = 0_u32;
        for (offset, character) in text.char_indices() {
            let width = if character.len_utf16() == 2 { 2 } else { 1 };
            units = units.saturating_add(width);
            let end = offset + character.len_utf8();
            if character.len_utf8() != character.len_utf16() {
                let delta = i64::from(units) - i64::try_from(end).unwrap_or(i64::MAX);
                boundaries.push((end, i32::try_from(delta).unwrap_or(0)));
            }
        }
        Self { boundaries }
    }

    /// Converts a UTF-8 byte offset to UTF-16 code units.
    pub(crate) fn offset(&self, text: &str, byte: usize) -> Result<u32, ScriptError> {
        if byte > text.len() || !text.is_char_boundary(byte) {
            return Err(ScriptError::Invalid {
                feature: "symbol.range",
                reason: "offset is outside the text or not a character boundary".to_owned(),
            });
        }
        let delta = match self.boundaries.binary_search_by_key(&byte, |(at, _)| *at) {
            Ok(index) => self.boundaries[index].1,
            Err(index) => {
                if index == 0 {
                    0
                } else {
                    self.boundaries[index - 1].1
                }
            }
        };
        let value = i64::try_from(byte).unwrap_or(i64::MAX) + i64::from(delta);
        u32::try_from(value).map_err(|_| ScriptError::Invalid {
            feature: "symbol.range",
            reason: "UTF-16 offset is out of range".to_owned(),
        })
    }
}

/// Half-open range in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Utf16Range {
    /// Inclusive code-unit start.
    pub start: Utf16Offset,
    /// Exclusive code-unit end.
    pub end: Utf16Offset,
}

impl Utf16Range {
    /// Creates a range.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            start: Utf16Offset::new(start),
            end: Utf16Offset::new(end),
        }
    }

    /// Returns the length in code units.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.get().saturating_sub(self.start.get())
    }

    /// Returns whether the range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start.get() >= self.end.get()
    }
}

/// One range in both coordinate systems used by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptRange {
    /// Exact UTF-8 byte range in the original file.
    pub utf8: SourceRange,
    /// The same range in UTF-16 code units.
    pub utf16: Utf16Range,
}

impl ScriptRange {
    /// Converts a UTF-8 byte range against a reusable UTF-16 index.
    pub(crate) fn from_index(
        index: &Utf16Index,
        text: &str,
        utf8: SourceRange,
    ) -> Result<Self, ScriptError> {
        let start = index.offset(text, utf8.start.get() as usize)?;
        let end = index.offset(text, utf8.end.get() as usize)?;
        Ok(Self {
            utf8,
            utf16: Utf16Range::new(start, end),
        })
    }

    /// Returns the exact original text of this range.
    #[must_use]
    pub fn text<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.get(self.utf8.start.get() as usize..self.utf8.end.get() as usize)
    }
}

/// Owner registration of one symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolOwner {
    /// `Page(...)` registration.
    Page,
    /// `Component(...)` registration.
    Component,
    /// `Behavior(...)` registration.
    Behavior,
}

impl SymbolOwner {
    /// Returns the stable lower-case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Component => "component",
            Self::Behavior => "behavior",
        }
    }
}

/// Kind of a script symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// The registration call itself.
    Registration,
    /// A `data` field.
    Data,
    /// A component property.
    Property,
    /// A page/component method.
    Method,
    /// A behavior registration.
    Behavior,
    /// A method contributed by a behavior.
    BehaviorMethod,
}

impl SymbolKind {
    /// Returns the stable lower-case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Data => "data",
            Self::Property => "property",
            Self::Method => "method",
            Self::Behavior => "behavior",
            Self::BehaviorMethod => "behavior-method",
        }
    }
}

/// Where the declaration of a symbol was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionOrigin {
    /// The analyzed script itself.
    Original,
    /// A behavior file imported from the analyzed script.
    ImportedBehavior {
        /// Project-relative path of the behavior file.
        path: String,
        /// Import binding or alias used by the component, when known.
        alias: Option<String>,
    },
    /// The evidence is dynamic and no original declaration can be named.
    Dynamic {
        /// Stable reason marker.
        reason: String,
    },
}

impl DefinitionOrigin {
    /// Returns the project-relative path when a declaration file is known.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::ImportedBehavior { path, .. } => Some(path),
            Self::Original | Self::Dynamic { .. } => None,
        }
    }
}

/// Why a symbol is not safe to rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameBlock {
    /// The registration is not unique or not a literal object.
    AmbiguousRegistration,
    /// The declaration value or member shape is not statically readable.
    IncompleteEvidence,
    /// The symbol is declared in another file; cross-file references are not
    /// proven by this slice.
    CrossFileEvidence,
    /// The name is written as a quoted key; a safe rename needs requoting.
    QuotedKey,
    /// The symbol is synthetic or has no original token.
    NotOriginalToken,
    /// Registrar names (`Page`/`Component`/`Behavior`) are not user symbols.
    RegistrationName,
}

impl RenameBlock {
    /// Returns the stable reason marker.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguousRegistration => "ambiguous-registration",
            Self::IncompleteEvidence => "incomplete-evidence",
            Self::CrossFileEvidence => "cross-file-evidence",
            Self::QuotedKey => "quoted-key",
            Self::NotOriginalToken => "not-original-token",
            Self::RegistrationName => "registration-name",
        }
    }
}

/// Rename safety decision for one symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameSafety {
    /// True only when [`Self::blocks`] is empty.
    pub rename_safe: bool,
    /// Reasons that make the symbol unsafe.
    pub blocks: Vec<RenameBlock>,
}

impl RenameSafety {
    /// Builds a safe decision.
    #[must_use]
    pub const fn safe() -> Self {
        Self {
            rename_safe: true,
            blocks: Vec::new(),
        }
    }

    /// Builds an unsafe decision.
    #[must_use]
    pub fn blocked(block: RenameBlock) -> Self {
        Self {
            rename_safe: false,
            blocks: vec![block],
        }
    }

    /// Adds a block reason if it is not already present.
    pub(crate) fn block(&mut self, block: RenameBlock) {
        if !self.blocks.contains(&block) {
            self.blocks.push(block);
        }
        self.rename_safe = false;
    }
}

/// One stable script symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSymbol {
    /// Owning registration kind.
    pub owner: SymbolOwner,
    /// Project-relative script that declares the symbol.
    pub path: String,
    /// Symbol name as written (unquoted for string keys).
    pub name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Exact original name-token range.
    pub range: ScriptRange,
    /// Declaration origin.
    pub origin: DefinitionOrigin,
    /// Rename safety flags.
    pub safety: RenameSafety,
    /// Chinese/English documentation preserved verbatim from `JSDoc`.
    pub documentation: String,
    /// Conservative type text for data/property symbols.
    pub type_text: Option<String>,
    /// Exact default value source text for property symbols.
    pub default_text: Option<String>,
}

/// A fully checked, minimal rename of one original token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEdit {
    /// Project-relative script path.
    pub path: String,
    /// Exact original range to replace.
    pub range: ScriptRange,
    /// Text that must equal the current token, as the backend expects.
    pub expected_text: String,
    /// Replacement text.
    pub new_text: String,
}

impl ScriptSymbol {
    /// Returns a rename edit only when this symbol is safe.
    ///
    /// A caller must still validate the document revision and the current text
    /// against [`SymbolEdit::expected_text`]; this method only enforces the
    /// script-fact safety rules. Unsafe symbols never produce a partial plan.
    #[must_use]
    pub fn rename_edit(&self, new_name: &str) -> Option<SymbolEdit> {
        if !self.safety.rename_safe
            || !expression::is_binding_identifier(new_name)
            || self.origin != DefinitionOrigin::Original
        {
            return None;
        }
        Some(SymbolEdit {
            path: self.path.clone(),
            range: self.range,
            expected_text: self.name.clone(),
            new_text: new_name.to_owned(),
        })
    }

    /// Returns whether this symbol can be renamed.
    #[must_use]
    pub const fn rename_safe(&self) -> bool {
        self.safety.rename_safe
    }
}
