//! Static value resolution and conservative type inference.
//!
//! This is the syntactic subset the TypeScript backend reads without the
//! checker: literals, arrays, object literals, local `const` aliases and
//! imported `const` exports. Identifier or call results that cannot be resolved
//! from the snapshot become `any` with `exact = false`; no type name from a
//! private module is invented.

use std::collections::BTreeSet;

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;

use super::scanner::{MemberKind, Script, scan_members, unquote};
use super::snapshot::Snapshot;

/// Maximum binding/export hops before giving up.
pub(crate) const MAX_VALUE_DEPTH: usize = 16;

/// One resolved expression in a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExprRange {
    /// Project-relative path.
    pub(crate) path: String,
    /// Inclusive UTF-8 byte start of the expression.
    pub(crate) start: usize,
    /// Exclusive UTF-8 byte end of the expression.
    pub(crate) end: usize,
}

/// Conservative inferred type text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inferred {
    /// Type text without module-local names.
    pub(crate) text: String,
    /// False when the value depends on something this slice cannot read.
    pub(crate) exact: bool,
}

impl Inferred {
    fn any() -> Self {
        Self {
            text: "any".to_owned(),
            exact: false,
        }
    }
}

/// Infers the type of an expression token range in an already lexed script.
pub(crate) fn infer_initializer(
    script: &Script<'_>,
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    start: usize,
    end: usize,
    depth: usize,
) -> Inferred {
    if depth > MAX_VALUE_DEPTH {
        return Inferred::any();
    }
    let (start, end) = strip_parens(script, start, end);
    if start >= end {
        return Inferred::any();
    }
    let Some(token) = script.tokens.get(start) else {
        return Inferred::any();
    };
    match token.kind {
        TokenKind::StringLiteral if start + 1 == end => Inferred {
            text: "string".to_owned(),
            exact: true,
        },
        TokenKind::TemplateChunk => {
            if start + 1 != end || is_template_with_substitutions(script, start, end) {
                Inferred::any()
            } else if token.text(script.text).starts_with('`') {
                Inferred {
                    text: "string".to_owned(),
                    exact: true,
                }
            } else {
                Inferred::any()
            }
        }
        TokenKind::Number if start + 1 == end => Inferred {
            text: "number".to_owned(),
            exact: true,
        },
        TokenKind::Punctuator('-')
            if start + 2 == end && script.kind(start + 1) == Some(TokenKind::Number) =>
        {
            Inferred {
                text: "number".to_owned(),
                exact: true,
            }
        }
        TokenKind::Punctuator('[') => infer_array(script, start, end, snapshot, file, depth),
        TokenKind::Punctuator('{') => infer_object(script, start, end, snapshot, file, depth),
        TokenKind::Identifier => infer_identifier(snapshot, file, script, start, end, depth),
        _ => Inferred::any(),
    }
}

fn is_template_with_substitutions(script: &Script<'_>, start: usize, end: usize) -> bool {
    (start..end).any(|index| script.kind(index) == Some(TokenKind::TemplateExpressionStart))
}

fn infer_identifier(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    script: &Script<'_>,
    start: usize,
    end: usize,
    depth: usize,
) -> Inferred {
    if start + 1 != end {
        return Inferred::any();
    }
    match script.token_text(start) {
        Some("true" | "false") => Inferred {
            text: "boolean".to_owned(),
            exact: true,
        },
        Some("null") => Inferred {
            text: "any".to_owned(),
            exact: true,
        },
        Some("undefined") | None => Inferred::any(),
        Some(name) => {
            let Some(resolved) = resolve_binding_in(script, snapshot, file, name, depth + 1) else {
                return Inferred::any();
            };
            let Some(target) = snapshot.file(&resolved.path) else {
                return Inferred::any();
            };
            let Ok(target_script) = Script::new(target.text()) else {
                return Inferred::any();
            };
            let (start, end) = token_bounds(&target_script, &resolved);
            infer_initializer(&target_script, snapshot, target, start, end, depth + 1)
        }
    }
}

fn infer_array(
    script: &Script<'_>,
    start: usize,
    end: usize,
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    depth: usize,
) -> Inferred {
    let Ok(close) = script.close_of(start) else {
        return Inferred::any();
    };
    if close + 1 != end {
        return Inferred::any();
    }
    let mut types: BTreeSet<String> = BTreeSet::new();
    let mut exact = true;
    let mut index = start + 1;
    while index < close {
        while index < close && script.is_punct(index, ',') {
            index += 1;
        }
        if index >= close {
            break;
        }
        let element_end = element_end(script, index, close);
        if element_end <= index {
            break;
        }
        if script.is_punct(index, '.')
            && script.is_punct(index + 1, '.')
            && script.is_punct(index + 2, '.')
        {
            exact = false;
        } else {
            let inferred = infer_initializer(script, snapshot, file, index, element_end, depth + 1);
            if !inferred.exact {
                exact = false;
            }
            types.insert(inferred.text);
        }
        index = element_end;
        if index < close && script.is_punct(index, ',') {
            index += 1;
        }
    }
    if !exact {
        return Inferred {
            text: "any[]".to_owned(),
            exact: false,
        };
    }
    if types.is_empty() {
        return Inferred {
            text: "any[]".to_owned(),
            exact: true,
        };
    }
    if types.len() == 1 {
        let Some(only) = types.iter().next() else {
            return Inferred::any();
        };
        return Inferred {
            text: format!("{only}[]"),
            exact: true,
        };
    }
    let union = types.into_iter().collect::<Vec<_>>().join(" | ");
    Inferred {
        text: format!("({union})[]"),
        exact: true,
    }
}

fn infer_object(
    script: &Script<'_>,
    start: usize,
    end: usize,
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    depth: usize,
) -> Inferred {
    let Ok(close) = script.close_of(start) else {
        return Inferred::any();
    };
    if close + 1 != end {
        return Inferred::any();
    }
    let members = scan_members(script, start + 1, close);
    let mut fields: Vec<String> = Vec::new();
    let mut exact = true;
    for member in &members {
        if member.kind != MemberKind::Property {
            exact = false;
            continue;
        }
        let Some(name) = &member.name else {
            exact = false;
            continue;
        };
        let inferred = infer_initializer(
            script,
            snapshot,
            file,
            member.value_start,
            member.value_end,
            depth + 1,
        );
        if !inferred.exact {
            exact = false;
        }
        fields.push(format!("{name:?}: {}", inferred.text));
    }
    if exact {
        Inferred {
            text: format!("{{ {} }}", fields.join("; ")),
            exact: true,
        }
    } else {
        Inferred::any()
    }
}

/// Resolves a binding using an already lexed script.
pub(crate) fn resolve_binding_in(
    script: &Script<'_>,
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    name: &str,
    depth: usize,
) -> Option<ExprRange> {
    if depth > MAX_VALUE_DEPTH {
        return None;
    }
    let depths = script.depths();
    for index in 0..script.tokens.len() {
        if depths.get(index).copied().unwrap_or(1) != 0 {
            continue;
        }
        if script.is_ident(index, "const")
            || script.is_ident(index, "let")
            || script.is_ident(index, "var")
        {
            if script.is_ident(index + 1, name) && script.is_punct(index + 2, '=') {
                let start = index + 3;
                let end = statement_end(script, start);
                if start < end {
                    return Some(ExprRange {
                        path: file.path().to_owned(),
                        start: script.token_start(start)?,
                        end: script.token_start(end)?,
                    });
                }
            }
            continue;
        }
        if script.is_ident(index, "import") {
            if let Some((module, export)) = exact_import(script, index, name) {
                return resolve_export(snapshot, &module, export.as_deref(), file, depth + 1);
            }
        }
    }
    None
}

/// Resolves one exported expression from `specifier`.
pub(crate) fn resolve_export(
    snapshot: &Snapshot<'_>,
    specifier: &str,
    export_name: Option<&str>,
    from_file: &SourceFile,
    depth: usize,
) -> Option<ExprRange> {
    if depth > MAX_VALUE_DEPTH {
        return None;
    }
    let resolved = snapshot.resolve(specifier, from_file.path())?;
    let target = snapshot.file(&resolved)?;
    let script = Script::new(target.text()).ok()?;
    let depths = script.depths();
    for index in 0..script.tokens.len() {
        if depths.get(index).copied().unwrap_or(1) != 0 {
            continue;
        }
        if script.is_ident(index, "export") {
            if let Some(range) = export_from(&script, index, export_name, target)? {
                return Some(range);
            }
        }
        if export_name.is_none()
            && script.is_ident(index, "module")
            && script.is_punct(index + 1, '.')
            && script.is_ident(index + 2, "exports")
            && script.is_punct(index + 3, '=')
        {
            let start = index + 4;
            let end = statement_end(&script, start);
            if start < end {
                return Some(ExprRange {
                    path: target.path().to_owned(),
                    start: script.token_start(start)?,
                    end: script.token_start(end)?,
                });
            }
        }
    }
    None
}

#[allow(clippy::option_option)] // three-state lookup: not handled / unresolved / found
fn export_from(
    script: &Script<'_>,
    export_index: usize,
    export_name: Option<&str>,
    file: &SourceFile,
) -> Option<Option<ExprRange>> {
    let default = export_name.is_none();
    if script.is_ident(export_index + 1, "default") {
        if !default {
            return Some(None);
        }
        let start = export_index + 2;
        let end = statement_end(script, start);
        return Some((start < end).then(|| ExprRange {
            path: file.path().to_owned(),
            start: script.token_start(start).unwrap_or(0),
            end: script.token_start(end).unwrap_or(0),
        }));
    }
    if script.is_punct(export_index + 1, '=') {
        if !default {
            return Some(None);
        }
        let start = export_index + 2;
        let end = statement_end(script, start);
        return Some((start < end).then(|| ExprRange {
            path: file.path().to_owned(),
            start: script.token_start(start).unwrap_or(0),
            end: script.token_start(end).unwrap_or(0),
        }));
    }
    if script.is_punct(export_index + 1, '{') {
        let close = script.close_of(export_index + 1).ok()?;
        if script.is_ident(close + 1, "from") {
            // Re-exports need another resolver hop; this slice reports them as
            // unresolved instead of guessing.
            return Some(None);
        }
        return Some(export_clause(
            script,
            export_index + 1,
            close,
            export_name,
            file,
        ));
    }
    if script.is_ident(export_index + 1, "const")
        || script.is_ident(export_index + 1, "let")
        || script.is_ident(export_index + 1, "var")
    {
        let Some(name) = script.token_text(export_index + 2) else {
            return Some(None);
        };
        let wanted = default && matches!(name, "default" | "behavior") || export_name == Some(name);
        if !wanted {
            return Some(None);
        }
        return Some(declaration_initializer(script, export_index + 2, file));
    }
    Some(None)
}

fn export_clause(
    script: &Script<'_>,
    open: usize,
    close: usize,
    export_name: Option<&str>,
    file: &SourceFile,
) -> Option<ExprRange> {
    let wanted = export_name.unwrap_or("default");
    let mut index = open + 1;
    while index < close {
        let local = script.token_text(index)?;
        let mut exported = local;
        let mut cursor = index + 1;
        while cursor < close && !script.is_punct(cursor, ',') {
            if script.is_ident(cursor, "as") {
                exported = script.token_text(cursor + 1)?;
            }
            cursor += 1;
        }
        if exported == wanted {
            return resolve_binding_in_script(script, file, local);
        }
        index = cursor + 1;
    }
    None
}

fn declaration_initializer(
    script: &Script<'_>,
    name_index: usize,
    file: &SourceFile,
) -> Option<ExprRange> {
    let mut cursor = name_index + 1;
    while cursor < script.tokens.len() {
        if script.is_punct(cursor, '=') {
            let start = cursor + 1;
            let end = statement_end(script, start);
            if start < end {
                return Some(ExprRange {
                    path: file.path().to_owned(),
                    start: script.token_start(start)?,
                    end: script.token_start(end)?,
                });
            }
            return None;
        }
        if script.is_punct(cursor, ';') || script.is_punct(cursor, '}') {
            return None;
        }
        cursor += 1;
    }
    None
}

fn resolve_binding_in_script(
    script: &Script<'_>,
    file: &SourceFile,
    name: &str,
) -> Option<ExprRange> {
    let depths = script.depths();
    for index in 0..script.tokens.len() {
        if depths.get(index).copied().unwrap_or(1) != 0 {
            continue;
        }
        if (script.is_ident(index, "const")
            || script.is_ident(index, "let")
            || script.is_ident(index, "var"))
            && script.is_ident(index + 1, name)
            && script.is_punct(index + 2, '=')
        {
            let start = index + 3;
            let end = statement_end(script, start);
            if start < end {
                return Some(ExprRange {
                    path: file.path().to_owned(),
                    start: script.token_start(start)?,
                    end: script.token_start(end)?,
                });
            }
        }
    }
    None
}

/// Finds an import binding by local name in an already lexed script.
pub(crate) fn import_for_name(script: &Script<'_>, name: &str) -> Option<(String, Option<String>)> {
    let depths = script.depths();
    for index in 0..script.tokens.len() {
        if depths.get(index).copied().unwrap_or(1) != 0 || !script.is_ident(index, "import") {
            continue;
        }
        if let Some((module, export)) = exact_import(script, index, name) {
            return Some((module, export));
        }
    }
    None
}

fn exact_import(
    script: &Script<'_>,
    import_index: usize,
    name: &str,
) -> Option<(String, Option<String>)> {
    let first = import_index + 1;
    if script.is_punct(first, '{') {
        let close = script.close_of(first).ok()?;
        if !script.is_ident(close + 1, "from") {
            return None;
        }
        let module = unquote(script.token_text(close + 2)?)?;
        for member in scan_members(script, first + 1, close) {
            let exported = member.name.clone()?;
            let local = if member.kind == MemberKind::Shorthand {
                exported.clone()
            } else {
                let mut cursor = member.value_start;
                let mut renamed = None;
                while cursor < member.value_end {
                    if script.is_ident(cursor, "as") {
                        renamed = script.token_text(cursor + 1).map(str::to_owned);
                        break;
                    }
                    cursor += 1;
                }
                renamed.unwrap_or_else(|| exported.clone())
            };
            if local == name {
                return Some((module, Some(exported)));
            }
        }
        return None;
    }
    if script.is_ident(first, "type") {
        return None;
    }
    let module = if script.is_ident(first + 1, "from") {
        unquote(script.token_text(first + 2)?)?
    } else if script.kind(first) == Some(TokenKind::Identifier)
        && script.is_punct(first + 1, '=')
        && script.is_ident(first + 2, "require")
    {
        unquote(script.token_text(first + 4)?)?
    } else {
        return None;
    };
    if script.token_text(first) == Some(name) {
        Some((module, None))
    } else {
        None
    }
}

fn statement_end(script: &Script<'_>, start: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < script.tokens.len() {
        match script.kind(index) {
            Some(TokenKind::Punctuator('(' | '[' | '{') | TokenKind::TemplateExpressionStart) => {
                depth += 1;
            }
            Some(TokenKind::Punctuator(')' | ']' | '}') | TokenKind::TemplateExpressionEnd) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Some(TokenKind::Punctuator(';')) if depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    index
}

fn element_end(script: &Script<'_>, start: usize, end: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < end {
        match script.kind(index) {
            Some(TokenKind::Punctuator('(' | '[' | '{') | TokenKind::TemplateExpressionStart) => {
                depth += 1;
            }
            Some(TokenKind::Punctuator(')' | ']' | '}') | TokenKind::TemplateExpressionEnd) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Some(TokenKind::Punctuator(',')) if depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    index
}

fn strip_parens(script: &Script<'_>, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && script.is_punct(start, '(') {
        let Ok(close) = script.close_of(start) else {
            break;
        };
        if close + 1 != end {
            break;
        }
        start += 1;
        end = close;
    }
    (start, end)
}

/// Converts a resolved byte range back to token indices in its file.
pub(crate) fn token_bounds(script: &Script<'_>, resolved: &ExprRange) -> (usize, usize) {
    let start = script
        .tokens
        .partition_point(|token| token.start < resolved.start);
    let end = script
        .tokens
        .partition_point(|token| token.start < resolved.end);
    (start, end)
}

/// One `type`/`optionalTypes` constructor value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Constructor {
    /// A recognized constructor mapped to its TypeScript type text.
    Known(&'static str),
    /// A value with no union contribution, such as `undefined`.
    Ignored,
    /// A value this syntactic slice cannot prove.
    Unreadable,
}

/// Reads `type` plus `optionalTypes` into a conservative union text.
///
/// The text order matches the Node overlay: `type` first, then every
/// `optionalTypes` entry in array order, with duplicate texts removed.
/// `null` maps to `any`; `undefined` is ignored exactly like the Node
/// `constructorText` lookup. An unreadable entry is skipped and reported via
/// the returned completeness flag instead of being guessed.
pub(crate) fn union_text(
    script: &Script<'_>,
    options: &[super::scanner::Member],
) -> Option<(String, bool)> {
    let mut parts: Vec<&'static str> = Vec::new();
    let mut complete = true;
    match constructor_member(script, options, "type") {
        Some(Constructor::Known(text)) => parts.push(text),
        Some(Constructor::Ignored) => {}
        Some(Constructor::Unreadable) | None => {
            if super::scanner::member_last(options, "type").is_some() {
                complete = false;
            }
        }
    }
    if let Some(optional) = super::scanner::member_last(options, "optionalTypes") {
        if optional.kind == MemberKind::Property
            && optional.value_start < optional.value_end
            && script.is_punct(optional.value_start, '[')
        {
            if let Ok(close) = script.close_of(optional.value_start) {
                if close + 1 == optional.value_end {
                    for (start, end) in array_elements(script, optional.value_start, close) {
                        match constructor_token(script, start, end) {
                            Constructor::Known(text) => {
                                if !parts.contains(&text) {
                                    parts.push(text);
                                }
                            }
                            Constructor::Ignored => {}
                            Constructor::Unreadable => complete = false,
                        }
                    }
                } else {
                    complete = false;
                }
            } else {
                complete = false;
            }
        } else {
            complete = false;
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some((parts.join(" | "), complete))
    }
}

fn constructor_member(
    script: &Script<'_>,
    options: &[super::scanner::Member],
    name: &str,
) -> Option<Constructor> {
    let member = super::scanner::member_last(options, name)?;
    if member.kind != MemberKind::Property {
        return Some(Constructor::Unreadable);
    }
    Some(constructor_token(
        script,
        member.value_start,
        member.value_end,
    ))
}

fn constructor_token(script: &Script<'_>, start: usize, end: usize) -> Constructor {
    if start + 1 != end {
        return Constructor::Unreadable;
    }
    let Some(token) = script.tokens.get(start) else {
        return Constructor::Unreadable;
    };
    match token.kind {
        TokenKind::Identifier => match token.text(script.text) {
            "String" => Constructor::Known("string"),
            "Number" => Constructor::Known("number"),
            "Boolean" => Constructor::Known("boolean"),
            "Array" => Constructor::Known("any[]"),
            "Object" => Constructor::Known("Record<string, any>"),
            "Function" => Constructor::Known("(...args: any[]) => any"),
            "null" => Constructor::Known("any"),
            "undefined" => Constructor::Ignored,
            _ => Constructor::Unreadable,
        },
        _ => Constructor::Unreadable,
    }
}

/// Splits array literal elements into token ranges.
pub(crate) fn array_elements(
    script: &Script<'_>,
    open: usize,
    close: usize,
) -> Vec<(usize, usize)> {
    let mut elements = Vec::new();
    let mut index = open + 1;
    while index < close {
        while index < close && script.is_punct(index, ',') {
            index += 1;
        }
        if index >= close {
            break;
        }
        let end = element_end(script, index, close);
        if end <= index {
            break;
        }
        elements.push((index, end));
        index = end;
        if index < close && script.is_punct(index, ',') {
            index += 1;
        }
    }
    elements
}
