//! F1-22 hover composition and evidence tests.
//!
//! The pure tests freeze the three-part Markdown order and the
//! checked-empty / Unsupported / Partial distinction. The real-engine test is
//! opt-in through `TSGO_BIN`; `WXML_REQUIRE_REAL_ENGINE=1` turns a missing
//! engine into a hard failure instead of a skip.

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use wxml_backend_tsgo::input::{
    AnalysisInput, AssetPath, CompilerOptions, DocumentRevision, MINIPROGRAM_TYPINGS, ProjectKey,
    ProjectRoot, SnapshotRevision, SourceFile, TypeAssetFile, TypeAssets, TypeAssetsPin,
};
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::{Utf16Offset, Utf16Position, offset_to_position};
use wxml_backend_tsgo::query::documentation::{DocumentationResult, JsDocTagInfo};
use wxml_backend_tsgo::query::hover::{
    HoverDeclaration, HoverEvidence, HoverGap, HoverSymbolFacts, MAX_HOVER_DECLARATIONS,
    compose_hover_markdown,
};
use wxml_backend_tsgo::query::type_info::{TypeInfo, TypeQuality};
use wxml_backend_tsgo::script::symbols::Utf16Range;
use wxml_backend_tsgo::shadow::mapping::SourceRange;
use wxml_types::CapabilityStatus;

fn type_info(text: &str) -> TypeInfo {
    TypeInfo {
        text: text.to_owned(),
        handle: 1,
        flags: 1,
    }
}

fn tagged(name: &str, text: Option<&str>) -> JsDocTagInfo {
    JsDocTagInfo {
        name: name.to_owned(),
        text: text.map(str::to_owned),
    }
}

fn documentation(
    comment: Option<&str>,
    tags: Vec<JsDocTagInfo>,
    status: CapabilityStatus,
) -> DocumentationResult {
    DocumentationResult {
        comment: comment.map(str::to_owned),
        tags,
        status,
    }
}

const fn evidence(
    type_info: Option<TypeInfo>,
    type_quality: TypeQuality,
    documentation: Option<DocumentationResult>,
) -> HoverEvidence {
    HoverEvidence {
        type_info,
        type_quality,
        symbol: None,
        documentation,
        declarations: Vec::new(),
        status: CapabilityStatus::Supported,
        gaps: Vec::new(),
    }
}

#[test]
fn markdown_uses_the_frozen_three_part_order_without_prefixes() {
    let mut hover = evidence(
        Some(type_info("string")),
        TypeQuality::Known,
        Some(documentation(
            Some("Display name.\nSecond line."),
            vec![
                tagged("deprecated", Some("use fullName")),
                tagged("since", Some("1.0")),
            ],
            CapabilityStatus::Supported,
        )),
    );
    hover.finalize_status();

    assert_eq!(hover.status, CapabilityStatus::Supported);
    assert_eq!(
        compose_hover_markdown(&hover),
        "```ts\nstring\n```\n\nDisplay name.\nSecond line.\n\n\
         @deprecated use fullName\n@since 1.0"
    );
    assert!(!compose_hover_markdown(&hover).contains("(property)"));
    assert!(!compose_hover_markdown(&hover).contains("value:"));
}

#[test]
fn overload_text_and_merged_documentation_are_not_segmented() {
    let overload = "{ (value: string): string; (value: number): number; }";
    let mut hover = evidence(
        Some(type_info(overload)),
        TypeQuality::Known,
        Some(documentation(
            Some("Format a string.\nFormat a number."),
            Vec::new(),
            CapabilityStatus::Supported,
        )),
    );
    hover.finalize_status();

    let markdown = compose_hover_markdown(&hover);
    assert_eq!(
        markdown,
        format!("```ts\n{overload}\n```\n\nFormat a string.\nFormat a number.")
    );
    assert_eq!(markdown.matches("```").count(), 2);
}

#[test]
fn any_is_rendered_literally_but_unknown_is_never_wrapped_as_a_type() {
    let mut any = evidence(Some(type_info("any")), TypeQuality::Any, None);
    any.finalize_status();
    assert_eq!(any.status, CapabilityStatus::Supported);
    assert_eq!(compose_hover_markdown(&any), "```ts\nany\n```");

    let mut unknown = evidence(Some(type_info("unknown")), TypeQuality::Unknown, None);
    unknown.finalize_status();
    assert!(matches!(
        unknown.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(compose_hover_markdown(&unknown), "");

    let mut documented = evidence(
        Some(type_info("unknown")),
        TypeQuality::Unknown,
        Some(documentation(
            Some("Only documentation."),
            Vec::new(),
            CapabilityStatus::Supported,
        )),
    );
    documented.finalize_status();
    assert!(matches!(
        documented.status,
        CapabilityStatus::Partial { .. }
    ));
    assert_eq!(compose_hover_markdown(&documented), "Only documentation.");
}

#[test]
fn a_tag_without_text_is_a_bare_at_name_line() {
    let mut hover = evidence(
        Some(type_info("number")),
        TypeQuality::Known,
        Some(documentation(
            None,
            vec![tagged("deprecated", None)],
            CapabilityStatus::Supported,
        )),
    );
    hover.finalize_status();
    assert_eq!(
        compose_hover_markdown(&hover),
        "```ts\nnumber\n```\n\n@deprecated"
    );
}

#[test]
fn empty_evidence_is_unsupported_not_a_supported_empty_hover() {
    let mut hover = evidence(None, TypeQuality::Missing, None);
    hover.finalize_status();
    assert!(matches!(hover.status, CapabilityStatus::Unsupported { .. }));
    assert_eq!(compose_hover_markdown(&hover), "");
}

#[test]
fn checked_empty_documentation_stays_supported_but_unavailable_is_partial() {
    let mut checked = evidence(
        Some(type_info("string")),
        TypeQuality::Known,
        Some(documentation(None, Vec::new(), CapabilityStatus::Supported)),
    );
    checked.finalize_status();
    assert_eq!(checked.status, CapabilityStatus::Supported);
    assert_eq!(compose_hover_markdown(&checked), "```ts\nstring\n```");

    let mut unavailable = evidence(
        Some(type_info("string")),
        TypeQuality::Known,
        Some(documentation(
            None,
            Vec::new(),
            CapabilityStatus::Unsupported {
                reason: "documentation methods are unavailable".to_owned(),
            },
        )),
    );
    unavailable.finalize_status();
    assert!(matches!(
        unavailable.status,
        CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn missing_type_with_symbol_or_declaration_evidence_is_blocked_not_guessed() {
    let mut hover = evidence(
        None,
        TypeQuality::Missing,
        Some(documentation(
            Some("Documented symbol."),
            Vec::new(),
            CapabilityStatus::Supported,
        )),
    );
    hover.symbol = Some(HoverSymbolFacts {
        name: "u".to_owned(),
        flags: 2,
        declarations: Vec::new(),
    });
    hover.finalize_status();

    assert_eq!(hover.type_quality, TypeQuality::Missing);
    assert!(
        hover
            .gaps
            .iter()
            .any(|gap| matches!(gap, HoverGap::Blocked { .. }))
    );
    assert!(matches!(hover.status, CapabilityStatus::Partial { .. }));
    assert_eq!(compose_hover_markdown(&hover), "Documented symbol.");
}

#[test]
fn external_declaration_gap_degrades_a_typed_hover_to_partial() {
    let mut hover = evidence(Some(type_info("number")), TypeQuality::Known, None);
    hover.gaps.push(HoverGap::ExternalDeclaration {
        path: Some("lib.es5.d.ts".to_owned()),
    });
    hover.finalize_status();

    assert!(matches!(hover.status, CapabilityStatus::Partial { .. }));
    assert_eq!(compose_hover_markdown(&hover), "```ts\nnumber\n```");
}

#[test]
fn declaration_targets_keep_both_coordinate_systems_and_the_shared_cap() {
    let declaration = HoverDeclaration {
        path: "model.ts".to_owned(),
        range: SourceRange::new(17, 21),
        utf16: Utf16Range::new(13, 17),
    };
    assert_eq!(declaration.range.start.get(), 17);
    assert_eq!(declaration.range.end.get(), 21);
    assert_eq!(declaration.utf16.start.get(), 13);
    assert_eq!(declaration.utf16.end.get(), 17);
    assert_eq!(MAX_HOVER_DECLARATIONS, 16);
}

const MAIN: &str = r#"import { User as Person, fmt } from "./model";
/** 😀 文档 */
const 用户: Person = { name: "张三 😀", age: 1 };
用户.name;
fmt("x");
const anyValue: any = 1;
anyValue;
const unknownValue: unknown = 1;
unknownValue;

"#;

const MODEL: &str = r"/**
 * A user.
 * @deprecated use NewUser
 */
export interface User {
  /**
   * 😀 Display name.
   * @deprecated use fullName
   */
  name: string;
  age: number;
}
/** Format a string. */
export function fmt(value: string): string;
/** Format a number. */
export function fmt(value: number): number;
export function fmt(value: string | number): string | number { return value; }
";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const fn position(line: u32, character: u32) -> Utf16Position {
    Utf16Position { line, character }
}

fn unique(tag: &str) -> String {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wxml-tsgo-f1-hover-{tag}-{}-{count}", std::process::id())
}

fn position_at(source: &str, needle: &str) -> Utf16Position {
    let byte = source.find(needle).expect("needle in source");
    let units = source
        .get(..byte)
        .expect("char boundary")
        .chars()
        .map(char::len_utf16)
        .sum::<usize>();
    offset_to_position(
        source,
        Utf16Offset::new(u32::try_from(units).expect("utf16 offset")),
    )
    .expect("position")
}

fn start_engine(tag: &str) -> Option<(NativeSession, Cleanup)> {
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "real-engine hover test skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return None;
    };
    let executable = Path::new(&bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {bin}"
    );

    let sandbox = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    assert!(
        std::fs::read_dir(&sandbox)
            .expect("sandbox listing")
            .next()
            .is_none(),
        "sandbox must start empty"
    );
    let guard = Cleanup(sandbox.clone());
    let session =
        NativeSession::start(SessionOptions::new(executable, sandbox)).expect("start real engine");
    Some((session, guard))
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn sha(text: &str) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(text.as_bytes()) {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn declaration_files(root: &Path, recursive: bool, lib_only: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                files.extend(declaration_files(&path, recursive, lib_only));
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".d.ts") || (lib_only && !name.starts_with("lib")) {
            continue;
        }
        files.push(path);
    }
    files.sort();
    files
}

fn load_pinned_assets() -> Result<TypeAssets, String> {
    let bin = std::env::var("TSGO_BIN").map_err(|_| "TSGO_BIN is unset".to_owned())?;
    let executable = PathBuf::from(bin);
    let standard_lib = std::env::var_os("TSGO_ASSET_ROOT")
        .map(PathBuf::from)
        .and_then(|root| {
            [root.join("engine/lib"), root.join("lib")]
                .into_iter()
                .find(|candidate| candidate.join("lib.d.ts").is_file())
        })
        .or_else(|| {
            executable
                .parent()
                .filter(|parent| parent.join("lib.d.ts").is_file())
                .map(Path::to_path_buf)
        })
        .ok_or_else(|| "no standard-library lib.d.ts near TSGO_BIN".to_owned())?;
    let standard_lib = std::fs::canonicalize(&standard_lib)
        .map_err(|error| format!("canonicalize {}: {error}", standard_lib.display()))?;
    let typings_root = std::env::var_os("TSGO_TYPINGS_ROOT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backends/typescript/node_modules"),
        PathBuf::from,
    );
    let typings_root = std::fs::canonicalize(&typings_root)
        .map_err(|error| format!("canonicalize {}: {error}", typings_root.display()))?;
    let typings_package = typings_root.join(MINIPROGRAM_TYPINGS);
    if !typings_package.join("index.d.ts").is_file() {
        return Err(format!(
            "no pinned typings package at {}",
            typings_package.display()
        ));
    }

    let mut files = Vec::new();
    for path in declaration_files(&standard_lib, false, true)
        .into_iter()
        .chain(declaration_files(&typings_package, true, false))
    {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let size = u64::try_from(text.len()).map_err(|_| "asset is too large".to_owned())?;
        files.push(
            TypeAssetFile::new(
                AssetPath::new(path.to_string_lossy().into_owned())
                    .map_err(|error| error.to_string())?,
                text.clone(),
                sha(&text),
                size,
            )
            .map_err(|error| error.to_string())?,
        );
    }
    TypeAssets::new(
        AssetPath::new(standard_lib.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())?,
        AssetPath::new(typings_root.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())?,
        files,
        TypeAssetsPin::pinned_runtime(),
    )
    .map_err(|error| error.to_string())
}

#[test]
fn real_engine_hover_evidence_preserves_type_docs_tags_and_declaration_ranges() {
    let Some((mut session, _sandbox)) = start_engine("engine") else {
        return;
    };
    let project_root = format!("/tmp/opencode/{}", unique("project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );
    let input = AnalysisInput::new(
        ProjectKey::new("f1-hover"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(1),
        vec![
            SourceFile::new("main.ts", MAIN, DocumentRevision::new(1)).expect("main.ts"),
            SourceFile::new("model.ts", MODEL, DocumentRevision::new(1)).expect("model.ts"),
        ],
    )
    .expect("analysis input");
    session.open_project(&input).expect("open project");

    // Plain field: alias-typed receiver, exact type, JSDoc comment and tag.
    let field = session
        .hover_evidence("main.ts", position(3, 3))
        .expect("field hover");
    println!(
        "field: quality={:?} type={:?} docs={:?} tags={:?} decls={:?} gaps={:?} status={:?}",
        field.type_quality,
        field.type_info.as_ref().map(|info| info.text.as_str()),
        field
            .documentation
            .as_ref()
            .and_then(|docs| docs.comment.as_deref()),
        field
            .documentation
            .as_ref()
            .map(|docs| docs.tags.as_slice()),
        field
            .declarations
            .iter()
            .map(|declaration| {
                (
                    declaration.path.as_str(),
                    declaration.range.start.get(),
                    declaration.range.end.get(),
                    declaration.utf16.start.get(),
                    declaration.utf16.end.get(),
                )
            })
            .collect::<Vec<_>>(),
        field.gaps,
        field.status
    );
    assert_eq!(field.type_quality, TypeQuality::Known);
    assert_eq!(
        field.type_info.as_ref().expect("type").text.as_str(),
        "string"
    );
    let field_docs = field.documentation.as_ref().expect("field docs");
    assert!(
        field_docs
            .comment
            .as_deref()
            .is_some_and(|comment| comment.contains("Display name")),
        "the property JSDoc must be preserved: {field_docs:?}"
    );
    assert!(
        field_docs
            .tags
            .iter()
            .any(|tag| tag.name == "deprecated" && tag.text.as_deref() == Some("use fullName")),
        "the @deprecated tag must be preserved: {field_docs:?}"
    );
    assert_eq!(field.status, CapabilityStatus::Supported);

    let markdown = compose_hover_markdown(&field);
    assert!(markdown.starts_with("```ts\nstring\n```\n\n"));
    assert!(markdown.contains("@deprecated use fullName"));
    assert!(!markdown.contains("(property)"));

    // Declaration target: project-relative model.ts with verified UTF-8 and
    // UTF-16 offsets after an emoji/JSDoc prefix.
    let declaration = field
        .declarations
        .iter()
        .find(|declaration| declaration.path == "model.ts")
        .expect("model.ts declaration");
    let start = usize::try_from(declaration.range.start.get()).expect("utf8 start");
    let end = usize::try_from(declaration.range.end.get()).expect("utf8 end");
    assert_eq!(MODEL.get(start..end), Some("name"));
    assert_eq!(
        MODEL[start..end].encode_utf16().count(),
        usize::try_from(declaration.utf16.len()).expect("utf16 length")
    );
    assert_ne!(
        declaration.range.start.get(),
        declaration.utf16.start.get(),
        "the emoji before the declaration must make UTF-8 and UTF-16 start differ"
    );

    // Import alias: the alias itself is reported, and the member query above
    // already proved that its declaration resolves through the alias.
    let alias = session
        .hover_evidence("main.ts", position_at(MAIN, "Person"))
        .expect("alias hover");
    println!(
        "alias: symbol={:?} quality={:?} type={:?} decls={:?} status={:?}",
        alias.symbol.as_ref().map(|symbol| symbol.name.as_str()),
        alias.type_quality,
        alias.type_info.as_ref().map(|info| info.text.as_str()),
        alias
            .declarations
            .iter()
            .map(|declaration| declaration.path.as_str())
            .collect::<Vec<_>>(),
        alias.status
    );
    assert_eq!(
        alias.symbol.as_ref().map(|symbol| symbol.name.as_str()),
        Some("Person")
    );
    let alias_declaration = alias
        .declarations
        .iter()
        .find(|declaration| declaration.path == "main.ts")
        .expect("main.ts alias declaration");
    let alias_start = usize::try_from(alias_declaration.range.start.get()).expect("alias start");
    let alias_end = usize::try_from(alias_declaration.range.end.get()).expect("alias end");
    assert_eq!(MAIN.get(alias_start..alias_end), Some("Person"));

    // Overloaded function: one combined type text and one fence only.
    let overload = session
        .hover_evidence("main.ts", position(4, 0))
        .expect("overload hover");
    println!(
        "overload: quality={:?} type={:?} docs={:?} status={:?}",
        overload.type_quality,
        overload.type_info.as_ref().map(|info| info.text.as_str()),
        overload
            .documentation
            .as_ref()
            .and_then(|docs| docs.comment.as_deref()),
        overload.status
    );
    let overload_text = overload
        .type_info
        .as_ref()
        .expect("overload type")
        .text
        .clone();
    assert_eq!(overload.type_quality, TypeQuality::Known);
    assert!(
        overload_text.contains("string") && overload_text.contains("number"),
        "the native merged overload text must be preserved: {overload_text}"
    );
    let overload_markdown = compose_hover_markdown(&overload);
    assert_eq!(overload_markdown.matches("```").count(), 2);
    assert!(!overload_markdown.contains("(property)"));

    // Explicit any is shown literally; unknown is never wrapped in a type
    // fence, even if documentation is available.
    let any = session
        .hover_evidence("main.ts", position(6, 6))
        .expect("any hover");
    assert_eq!(any.type_quality, TypeQuality::Any);
    assert_eq!(
        any.type_info.as_ref().expect("any type").text.as_str(),
        "any"
    );
    assert_eq!(compose_hover_markdown(&any), "```ts\nany\n```");

    let unknown = session
        .hover_evidence("main.ts", position(8, 6))
        .expect("unknown hover");
    assert_eq!(unknown.type_quality, TypeQuality::Unknown);
    assert!(!compose_hover_markdown(&unknown).contains("```"));

    // A position with no type, symbol or documentation is Unsupported, not an
    // empty Supported hover.
    let empty = session
        .hover_evidence("main.ts", position(1, 8))
        .expect("empty hover");
    println!(
        "empty: type={:?} symbol={:?} docs={:?} status={:?}",
        empty.type_info, empty.symbol, empty.documentation, empty.status
    );
    assert!(matches!(empty.status, CapabilityStatus::Unsupported { .. }));
    assert_eq!(compose_hover_markdown(&empty), "");

    session.release().expect("release snapshot");
    session.shutdown().expect("shutdown");
}

#[test]
fn real_engine_hover_marks_external_library_declarations_partial() {
    let Some((mut session, _sandbox)) = start_engine("external-engine") else {
        return;
    };
    let assets = match load_pinned_assets() {
        Ok(assets) => assets,
        Err(reason) => {
            let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
            assert!(
                !required,
                "pinned type assets are required when WXML_REQUIRE_REAL_ENGINE=1: {reason}"
            );
            eprintln!("real-engine external-declaration hover skipped: {reason}");
            return;
        }
    };

    let source = "const text = \"abc\";\ntext.length;\n";
    let project_root = format!("/tmp/opencode/{}", unique("external-project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );
    let input = AnalysisInput::with_environment(
        ProjectKey::new("f1-hover-external"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(1),
        vec![SourceFile::new("main.ts", source, DocumentRevision::new(1)).expect("main.ts")],
        CompilerOptions::new(BTreeMap::new()).expect("compiler options"),
        Arc::new(assets),
    )
    .expect("analysis input");
    session.open_project(&input).expect("open project");

    let external = session
        .hover_evidence("main.ts", position(1, 5))
        .expect("external hover");
    println!(
        "external: quality={:?} type={:?} decls={:?} gaps={:?} status={:?}",
        external.type_quality,
        external.type_info.as_ref().map(|info| info.text.as_str()),
        external.declarations,
        external.gaps,
        external.status
    );
    assert_eq!(external.type_quality, TypeQuality::Known);
    assert_eq!(
        external.type_info.as_ref().expect("type").text.as_str(),
        "number"
    );
    assert!(
        external
            .gaps
            .iter()
            .any(|gap| matches!(gap, HoverGap::ExternalDeclaration { .. })),
        "the standard-library declaration must be reported as a typed gap: {:?}",
        external.gaps
    );
    assert_eq!(external.declarations, Vec::new());
    assert!(matches!(external.status, CapabilityStatus::Partial { .. }));

    session.release().expect("release snapshot");
    session.shutdown().expect("shutdown");
}
