#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::missing_const_for_fn
)]

//! Integration tests for UTF-16 mapping, generated offsets and BOM/CRLF rules.
//!
//! Test names carry the TypeScript oracle case id. Diagnostic-mapping cases
//! that need the checker are listed in [`BLOCKED_CASES`].

use wxml_backend_tsgo::input::{DocumentRevision, SourceFile};
use wxml_backend_tsgo::native::strings::Utf16Offset;
use wxml_backend_tsgo::shadow::ShadowError;
use wxml_backend_tsgo::shadow::layout::{
    FragmentSpec, JsCheckPolicy, RootKind, ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::mapping::{
    MappingLayer, MountRecord, ShadowMap, ShadowMapper, SourceRange, byte_to_utf16,
    map_span_to_source, utf16_len, utf16_to_byte,
};
use wxml_backend_tsgo::shadow::rewrite::{EditKind, MountedSegment, ShadowEdit, apply_edits};

const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-MAPPING-0188",
        "nullable receiver diagnostics require the checker",
    ),
    (
        "TS-MAPPING-0208",
        "diagnostic de-duplication requires the checker",
    ),
    (
        "TS-MAPPING-0306",
        "suppressed diagnostics require the checker",
    ),
];

fn source_file(path: &str, text: &str) -> SourceFile {
    SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid source")
}

fn build(
    path: &str,
    text: &str,
    root: RootKind,
    fragments: &[FragmentSpec],
    js_check: JsCheckPolicy,
    check_js: bool,
) -> wxml_backend_tsgo::shadow::layout::BuiltShadow {
    let script = source_file(path, text);
    let request = ShadowRequest {
        script: &script,
        root,
        fragments,
        js_check,
        check_js,
    };
    build_in_place_shadow(&request).expect("layout")
}

fn segment(start: u32, length: u32, source: Option<SourceRange>) -> MountedSegment {
    MountedSegment {
        start_utf16: start,
        length_utf16: length,
        source,
    }
}

fn mount(text: &str, start: u32, segments: Vec<MountedSegment>) -> MountRecord {
    MountRecord {
        mount_id: "f#0".to_owned(),
        fragment_id: "f".to_owned(),
        insertion_id: "probe".to_owned(),
        text: text.to_owned(),
        start_utf16: start,
        target_start_utf16: 0,
        target_length_utf16: 0,
        segments,
    }
}

fn utf16_slice(text: &str, start: u32, length: u32) -> String {
    let units: Vec<u16> = text
        .encode_utf16()
        .skip(usize::try_from(start).expect("offset"))
        .take(usize::try_from(length).expect("length"))
        .collect();
    String::from_utf16(&units).expect("valid UTF-16 slice")
}

#[test]
fn case_mapping_0010_utf16_and_byte_offsets_agree_around_astral_characters() {
    let text = "😀中文\nabc";
    assert_eq!(utf16_to_byte(text, 0).expect("boundary"), 0);
    assert_eq!(utf16_to_byte(text, 2).expect("boundary"), 4);
    assert_eq!(utf16_to_byte(text, 3).expect("boundary"), 7);
    assert!(utf16_to_byte(text, 1).is_err(), "mid-surrogate");
    assert_eq!(byte_to_utf16(text, 4).expect("boundary"), 2);
    assert!(byte_to_utf16(text, 3).is_err(), "mid-codepoint");
}

#[test]
fn case_mapping_0020_mapped_span_trims_a_synthetic_prefix_and_keeps_the_source_part() {
    let record = mount(
        "__wxml_root.maybeUser",
        100,
        vec![
            segment(100, 12, None),
            segment(112, 9, Some(SourceRange::new(10, 19))),
        ],
    );
    let mapped = map_span_to_source(&record, Utf16Offset::new(100), 21).expect("mapped");
    assert_eq!(mapped.range, Some(SourceRange::new(10, 19)));
    // The inserted root prefix has no original bytes, so trimming it is the
    // exact source projection, not an approximate crop. This is the E2E-16
    // `maybeUser` path: a `null` object diagnostic must stay `Supported`.
    assert!(!mapped.trimmed_synthetic_prefix);
}

#[test]
fn case_mapping_0034_a_span_entirely_inside_synthetic_text_is_not_attributable() {
    let record = mount(
        "__wxml_root.title",
        0,
        vec![
            segment(0, 12, None),
            segment(12, 5, Some(SourceRange::new(3, 8))),
        ],
    );
    assert!(
        map_span_to_source(&record, Utf16Offset::new(0), 12)
            .expect("valid")
            .unattributable
    );
    assert!(
        map_span_to_source(&record, Utf16Offset::new(8), 2)
            .expect("valid")
            .unattributable
    );
    assert_eq!(
        map_span_to_source(&record, Utf16Offset::new(14), 2)
            .expect("valid")
            .range,
        Some(SourceRange::new(5, 7))
    );
}

#[test]
fn case_mapping_0048_assemble_maps_offsets_after_an_insertion_with_several_synthetic_prefixes() {
    let assembled = apply_edits(
        "abc",
        &[ShadowEdit::insert("probe", EditKind::Probe, 0, "x ")],
    )
    .expect("valid edit");
    let map = ShadowMap::new("abc", &assembled);
    assert!(map.to_original_offset(Utf16Offset::new(0)).is_err());
    assert_eq!(
        map.to_original_offset(Utf16Offset::new(2))
            .expect("after insertion"),
        Utf16Offset::new(0)
    );
    assert_eq!(
        map.to_original_offset(Utf16Offset::new(3))
            .expect("after insertion"),
        Utf16Offset::new(1)
    );
    assert_eq!(
        map.to_original_offset(Utf16Offset::new(5)).expect("end"),
        Utf16Offset::new(3)
    );
}

#[test]
fn case_mapping_0078_a_mapped_span_never_exceeds_the_recorded_source_range() {
    let record = mount(
        "__wxml_root.title",
        0,
        vec![
            segment(0, 12, None),
            segment(12, 5, Some(SourceRange::new(2, 4))),
        ],
    );
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(12), 5),
            Err(ShadowError::MappingSourceMismatch {
                needed: 5,
                available: 2
            })
        ),
        "5 text bytes cannot fit 2 source bytes"
    );
    assert_eq!(
        map_span_to_source(&record, Utf16Offset::new(12), 2)
            .expect("valid")
            .range,
        Some(SourceRange::new(2, 4))
    );
}

#[test]
fn case_mapping_0091_out_of_bounds_spans_and_non_character_boundaries_are_typed_errors() {
    let mut record = mount(
        "__wxml_root.😀",
        100,
        vec![
            segment(100, 12, None),
            segment(112, 2, Some(SourceRange::new(10, 14))),
        ],
    );
    record.target_start_utf16 = 112;
    record.target_length_utf16 = 2;
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(100), 15),
            Err(ShadowError::PositionOutOfRange {
                position: 100,
                length: 114,
                unit: "utf-16"
            })
        ),
        "end beyond the mount"
    );
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(99), 2),
            Err(ShadowError::PositionOutOfRange {
                position: 99,
                length: 114,
                unit: "utf-16"
            })
        ),
        "start before the mount"
    );
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(112), 3),
            Err(ShadowError::PositionOutOfRange {
                position: 112,
                length: 114,
                unit: "utf-16"
            })
        ),
        "target length + 1"
    );
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(113), 1),
            Err(ShadowError::NotCharBoundary { position: 13 })
        ),
        "low-surrogate start"
    );
    assert!(
        matches!(
            map_span_to_source(&record, Utf16Offset::new(112), 1),
            Err(ShadowError::NotCharBoundary { position: 13 })
        ),
        "high-surrogate end"
    );
    assert_eq!(
        map_span_to_source(&record, Utf16Offset::new(112), 2)
            .expect("valid")
            .range,
        Some(SourceRange::new(10, 14))
    );
}

#[test]
fn case_mapping_0110_fractional_and_negative_bounds_are_unrepresentable_in_the_typed_api() {
    // The Rust API uses u32 + Utf16Offset, so negative/fractional positions
    // cannot be constructed. The remaining failure mode is arithmetic overflow,
    // which is a typed error rather than a clamp.
    let record = mount(
        "__wxml_root.title",
        100,
        vec![segment(100, 17, Some(SourceRange::new(0, 5)))],
    );
    assert!(map_span_to_source(&record, Utf16Offset::new(112), u32::MAX).is_err());
}

#[test]
fn case_mapping_0126_cross_gap_target_is_used_only_when_fully_inside_the_span() {
    let mut record = mount(
        "__wxml_root.a + __wxml_root.b",
        0,
        vec![
            segment(0, 12, None),
            segment(12, 1, Some(SourceRange::new(0, 1))),
            segment(13, 3, Some(SourceRange::new(1, 4))),
            segment(16, 12, None),
            segment(28, 1, Some(SourceRange::new(4, 5))),
        ],
    );
    record.target_start_utf16 = 28;
    record.target_length_utf16 = 1;
    let mapped = map_span_to_source(&record, Utf16Offset::new(10), 8).expect("mapped");
    assert!(mapped.trimmed_synthetic_prefix);
    assert_eq!(mapped.range, Some(SourceRange::new(1, 4)));
}

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end segment audit over both mounts
fn case_mapping_0145_every_mount_carries_an_explicit_synthetic_source_split() {
    let text = "Page({ data: { user: { name: 'Ada' }, users: [{ name: 'Bo' }] } });";
    let fragments = [
        FragmentSpec::new("user", "user.name", SourceRange::new(0, 9)),
        FragmentSpec::new("users", "users[0].name", SourceRange::new(20, 33)),
    ];
    let built = build(
        "index.ts",
        text,
        RootKind::Page,
        &fragments,
        JsCheckPolicy::Inherit,
        false,
    );
    // Original WXML bytes the mount source ranges point into. The first
    // fragment starts at 0 while the second starts at 20, so the second mount
    // proves the generated-vs-original split is absolute, not
    // expression-relative.
    let mut wxml = [b'.'; 40];
    wxml[0..9].copy_from_slice(fragments[0].expr.as_bytes());
    wxml[20..33].copy_from_slice(fragments[1].expr.as_bytes());

    assert_eq!(built.view.mounts.len(), 2);
    let insertion_ids: std::collections::BTreeSet<&str> = built
        .view
        .insertions
        .iter()
        .map(|insertion| insertion.insertion_id.as_str())
        .collect();
    for record in &built.view.mounts {
        assert!(
            insertion_ids.contains(record.insertion_id.as_str()),
            "{} must reference a recorded insertion",
            record.mount_id
        );
        assert_eq!(
            utf16_slice(
                &built.view.text,
                record.start_utf16,
                utf16_len(&record.text)
            ),
            record.text,
            "{} absolute mount offsets point at the mounted text",
            record.mount_id
        );
        let mut cursor = record.start_utf16;
        let mut source_bytes = 0_u32;
        let mut has_source = false;
        let mut has_synthetic = false;
        let mut rebuilt = String::new();
        for segment in &record.segments {
            assert_eq!(segment.start_utf16, cursor, "{}", record.mount_id);
            let slice = utf16_slice(&built.view.text, segment.start_utf16, segment.length_utf16);
            rebuilt.push_str(&slice);
            cursor += segment.length_utf16;
            if let Some(source) = segment.source {
                assert!(source.end.get() >= source.start.get());
                has_source = true;
                source_bytes += source.len();
                assert_eq!(
                    slice.as_bytes(),
                    &wxml[usize::try_from(source.start.get()).expect("source start")
                        ..usize::try_from(source.end.get()).expect("source end")],
                    "{} source segment must be the verbatim expression bytes",
                    record.mount_id
                );
            } else {
                has_synthetic = true;
                assert!(!slice.is_empty(), "synthetic segments are non-empty");
            }
        }
        assert_eq!(rebuilt, record.text, "segments cover the mount");
        assert_eq!(
            cursor,
            record.end_utf16().expect("checked mount end"),
            "segments cover the mount"
        );
        assert!(has_source);
        assert!(has_synthetic);
        assert_eq!(
            source_bytes,
            u32::try_from(
                fragments
                    .iter()
                    .find(|fragment| fragment.id == record.fragment_id)
                    .expect("fragment")
                    .expr
                    .len()
            )
            .expect("length"),
            "source segments cover the expression exactly"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // one table-driven BOM/pragma policy matrix
fn case_mapping_0223_bom_is_dropped_whenever_a_pragma_must_be_honoured() {
    struct Case {
        id: &'static str,
        text: &'static str,
        policy: JsCheckPolicy,
        check_js: bool,
        injected_pragma: bool,
        /// Exact derived offset of the `// @ts-nocheck` insertion, if any.
        neutralize_at: Option<u32>,
    }
    let cases = [
        Case {
            id: "js-bom-check-off",
            text: "\u{FEFF}#!/usr/bin/env node\n// @ts-check\nPage({ data: { title: '首页' } });",
            policy: JsCheckPolicy::Off,
            check_js: true,
            injected_pragma: false,
            // Original offset 34 (BOM + shebang + pragma line) minus the
            // dropped BOM; the insertion sits before `Page`.
            neutralize_at: Some(33),
        },
        Case {
            id: "js-bom-check-inherit",
            text: "\u{FEFF}#!/usr/bin/env node\n// @ts-check\nPage({ data: { title: '首页' } });",
            policy: JsCheckPolicy::Inherit,
            check_js: false,
            injected_pragma: false,
            neutralize_at: None,
        },
        Case {
            id: "js-shebang-bom",
            text: "\u{FEFF}#!/usr/bin/env node\nPage({ data: { title: '首页' } });",
            policy: JsCheckPolicy::Pragma,
            check_js: false,
            injected_pragma: true,
            neutralize_at: None,
        },
    ];
    for case in cases {
        let built = build(
            "index.js",
            case.text,
            RootKind::Page,
            &[],
            case.policy,
            case.check_js,
        );
        assert!(!built.view.text.starts_with('\u{FEFF}'), "{}", case.id);
        assert!(built.view.text.starts_with("#!"), "{}", case.id);
        assert!(
            built.notes.iter().any(|note| note.contains("BOM dropped")),
            "{}: note must explain the drop",
            case.id
        );
        let bom = built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "bom")
            .unwrap_or_else(|| panic!("{}: BOM replacement must be recorded", case.id));
        assert_eq!(bom.kind, EditKind::Bom, "{}", case.id);
        assert_eq!(bom.start_utf16, 0, "{}", case.id);
        assert_eq!(bom.length_utf16, 0, "{}", case.id);
        assert_eq!(bom.replace_length_utf16, 1, "{}", case.id);

        let pragma = built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "pragma");
        if case.injected_pragma {
            let pragma = pragma.unwrap_or_else(|| panic!("{}: pragma insertion", case.id));
            assert_eq!(pragma.kind, EditKind::Pragma, "{}", case.id);
            assert_eq!(pragma.replace_length_utf16, 0, "{}", case.id);
            assert_eq!(pragma.start_utf16, 20, "{}: after the shebang", case.id);
            assert_eq!(
                utf16_slice(&built.view.text, pragma.start_utf16, pragma.length_utf16),
                "// @ts-check\n",
                "{}",
                case.id
            );
        } else {
            assert!(pragma.is_none(), "{}: no injected pragma", case.id);
        }

        let nocheck = built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "nocheck");
        if let Some(expected_start) = case.neutralize_at {
            let nocheck = nocheck.unwrap_or_else(|| panic!("{}: nocheck insertion", case.id));
            assert_eq!(nocheck.kind, EditKind::Pragma, "{}", case.id);
            assert_eq!(
                nocheck.start_utf16, expected_start,
                "{}: derived offset after the dropped BOM",
                case.id
            );
            assert_eq!(
                nocheck.replace_length_utf16, 0,
                "{}: the neutraliser is inserted, not a replacement",
                case.id
            );
            assert_eq!(
                utf16_slice(&built.view.text, nocheck.start_utf16, nocheck.length_utf16),
                "// @ts-nocheck\n",
                "{}",
                case.id
            );
            assert_eq!(
                utf16_slice(
                    &built.view.text,
                    nocheck.start_utf16 + nocheck.length_utf16,
                    4
                ),
                "Page",
                "{}: the neutraliser is inserted before the registration",
                case.id
            );
        } else {
            assert!(nocheck.is_none(), "{}: no neutralised pragma", case.id);
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // one table-driven audit across four layout scenarios
fn case_mapping_0239_published_evidence_carries_insertions_and_mounts_for_every_run() {
    struct Scenario {
        id: &'static str,
        path: &'static str,
        text: &'static str,
        root: RootKind,
        policy: JsCheckPolicy,
        check_js: bool,
        expr: &'static str,
        origin: u32,
    }
    let scenarios = [
        Scenario {
            id: "page-basic",
            path: "index.ts",
            text: "Page({ data: { user: { name: 'Ada' } } });",
            root: RootKind::Page,
            policy: JsCheckPolicy::Inherit,
            check_js: false,
            expr: "user.name",
            origin: 0,
        },
        Scenario {
            id: "component-properties",
            path: "index.ts",
            text: "Component({ properties: { title: { type: String, value: '' } }, data: { user: { name: 'Ada' } } });",
            root: RootKind::Component,
            policy: JsCheckPolicy::Inherit,
            check_js: false,
            expr: "title",
            origin: 40,
        },
        Scenario {
            id: "js-shebang-bom",
            path: "index.js",
            text: "\u{FEFF}#!/usr/bin/env node\nPage({ data: { user: { name: 'Ada' } } });",
            root: RootKind::Page,
            policy: JsCheckPolicy::Pragma,
            check_js: false,
            expr: "user.name",
            origin: 0,
        },
        Scenario {
            id: "nullable",
            path: "index.ts",
            text: "Page({ data: { maybeUser: null, external: null, users: [] } });",
            root: RootKind::Page,
            policy: JsCheckPolicy::Inherit,
            check_js: false,
            expr: "users[0].name",
            origin: 80,
        },
    ];
    for scenario in scenarios {
        let expr_bytes = u32::try_from(scenario.expr.len()).expect("expression bytes");
        let fragment = FragmentSpec::new(
            "f",
            scenario.expr,
            SourceRange::new(scenario.origin, scenario.origin + expr_bytes),
        );
        let built = build(
            scenario.path,
            scenario.text,
            scenario.root,
            &[fragment],
            scenario.policy,
            scenario.check_js,
        );
        assert!(!built.view.insertions.is_empty(), "{}", scenario.id);
        assert!(!built.view.mounts.is_empty(), "{}", scenario.id);
        let insertion_ids: std::collections::BTreeSet<&str> = built
            .view
            .insertions
            .iter()
            .map(|insertion| insertion.insertion_id.as_str())
            .collect();
        let mut wxml =
            vec![b'.'; usize::try_from(scenario.origin + expr_bytes).expect("buffer") + 1];
        let origin = usize::try_from(scenario.origin).expect("origin");
        wxml[origin..origin + scenario.expr.len()].copy_from_slice(scenario.expr.as_bytes());

        for record in &built.view.mounts {
            assert!(
                insertion_ids.contains(record.insertion_id.as_str()),
                "{}: mount references a recorded insertion",
                scenario.id
            );
            assert_eq!(
                record.segments.len(),
                2,
                "{}: synthetic + source",
                scenario.id
            );
            assert!(
                record.segments[0].source.is_none(),
                "{}: first segment is synthetic",
                scenario.id
            );
            assert_eq!(
                record.segments[0].start_utf16, record.start_utf16,
                "{}: the synthetic prefix starts at the mount",
                scenario.id
            );
            assert!(
                record.segments[0].length_utf16 > 0,
                "{}: the synthetic prefix is non-empty",
                scenario.id
            );
            let source = record.segments[1]
                .source
                .unwrap_or_else(|| panic!("{}: second segment is source", scenario.id));
            assert_eq!(
                source.len(),
                expr_bytes,
                "{}: source segment covers the expression exactly",
                scenario.id
            );
            assert_eq!(
                utf16_slice(
                    &built.view.text,
                    record.segments[1].start_utf16,
                    record.segments[1].length_utf16
                )
                .as_bytes(),
                &wxml[usize::try_from(source.start.get()).expect("source start")
                    ..usize::try_from(source.end.get()).expect("source end")],
                "{}: source segment is the verbatim expression bytes",
                scenario.id
            );
            let mut cursor = record.start_utf16;
            let mut rebuilt = String::new();
            for segment in &record.segments {
                assert_eq!(
                    segment.start_utf16, cursor,
                    "{}: segments are contiguous",
                    scenario.id
                );
                rebuilt.push_str(&utf16_slice(
                    &built.view.text,
                    segment.start_utf16,
                    segment.length_utf16,
                ));
                cursor += segment.length_utf16;
            }
            assert_eq!(
                rebuilt, record.text,
                "{}: segments cover the mounted text",
                scenario.id
            );
            assert_eq!(
                cursor,
                record.end_utf16().expect("checked mount end"),
                "{}: segments cover the mount",
                scenario.id
            );
            assert_eq!(
                record.segments[0].length_utf16 + record.segments[1].length_utf16,
                utf16_len(&record.text),
                "{}: segment lengths sum to the mounted text",
                scenario.id
            );
            assert_eq!(
                utf16_slice(
                    &built.view.text,
                    record.start_utf16,
                    utf16_len(&record.text)
                ),
                record.text,
                "{}: absolute mount offsets point at the mounted text",
                scenario.id
            );
        }
        let mut previous_end = 0;
        for insertion in &built.view.insertions {
            assert!(
                insertion.start_utf16 >= previous_end,
                "{}: insertions do not overlap",
                scenario.id
            );
            previous_end =
                insertion.start_utf16 + insertion.replace_length_utf16.max(insertion.length_utf16);
        }
    }
}

#[test]
fn case_mapping_0261_mount_target_length_is_published_and_matches_the_mounted_identifier() {
    let text = "Page({ data: { user: { name: 'Ada' } } });";
    let fragments = [FragmentSpec::new(
        "user",
        "user.name",
        SourceRange::new(40, 49),
    )];
    let built = build(
        "index.ts",
        text,
        RootKind::Page,
        &fragments,
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.view.mounts.len(), 1);
    let record = &built.view.mounts[0];
    assert_eq!(record.target_length_utf16, 4);
    assert_eq!(
        utf16_slice(
            &built.view.text,
            record.target_start_utf16,
            record.target_length_utf16
        ),
        "name"
    );
    let mapped = map_span_to_source(
        record,
        Utf16Offset::new(record.target_start_utf16),
        record.target_length_utf16,
    )
    .expect("mapped target");
    assert_eq!(mapped.range, Some(SourceRange::new(45, 49)));

    // CJK identifiers are one UTF-16 unit each but three UTF-8 bytes; the
    // published length follows the mounted text, not the source bytes. The
    // leading emoji moves every derived offset after an astral character.
    let text = "/* 😀 */ Page({ data: { 用户: { 名字: '阿' } } });";
    let expr = "用户.名字";
    let fragments = [FragmentSpec::new(
        "cjk",
        expr,
        SourceRange::new(60, 60 + u32::try_from(expr.len()).expect("length")),
    )];
    let built = build(
        "index.ts",
        text,
        RootKind::Page,
        &fragments,
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.view.mounts.len(), 1);
    let record = &built.view.mounts[0];
    assert_eq!(record.target_length_utf16, 2);
    assert_eq!(
        utf16_slice(
            &built.view.text,
            record.target_start_utf16,
            record.target_length_utf16
        ),
        "名字"
    );
    assert_eq!(
        utf16_slice(
            &built.view.text,
            record.start_utf16,
            utf16_len(&record.text)
        ),
        record.text,
        "astral characters before the mount do not shift the recorded offsets"
    );
    let mapped = map_span_to_source(
        record,
        Utf16Offset::new(record.target_start_utf16),
        record.target_length_utf16,
    )
    .expect("mapped target");
    assert_eq!(
        mapped.range,
        Some(SourceRange::new(60 + 7, 60 + 13)),
        "`名字` starts after `用户.` (7 bytes)"
    );
}

#[test]
fn case_mapping_0279_bom_removal_is_a_one_character_replacement_not_an_insertion() {
    let built = build(
        "index.js",
        "\u{FEFF}#!/usr/bin/env node\n// @ts-check\nComponent({ methods: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    );
    assert!(built.view.text.starts_with("#!"));
    let bom = built
        .view
        .insertions
        .iter()
        .find(|insertion| insertion.insertion_id == "bom")
        .expect("BOM edit");
    assert_eq!(bom.kind, EditKind::Bom);
    assert_eq!(bom.start_utf16, 0);
    assert_eq!(bom.length_utf16, 0);
    assert_eq!(bom.replace_length_utf16, 1);
    assert!(
        built
            .view
            .insertions
            .iter()
            .all(|insertion| insertion.insertion_id != "pragma"),
        "an existing source pragma needs no injected one"
    );
}

#[test]
fn malformed_mount_records_return_typed_errors() {
    let mut overflow_end = mount("x", u32::MAX, Vec::new());
    overflow_end.segments = vec![segment(u32::MAX, 1, None)];
    assert!(map_span_to_source(&overflow_end, Utf16Offset::new(u32::MAX), 0).is_err());

    let mut overflow_segment = mount("x", 0, vec![segment(0, u32::MAX, None)]);
    assert!(map_span_to_source(&overflow_segment, Utf16Offset::new(0), 1).is_err());

    overflow_segment.segments = vec![segment(0, 1, Some(SourceRange::new(0, 1)))];
    overflow_segment.target_start_utf16 = u32::MAX;
    overflow_segment.target_length_utf16 = 2;
    assert!(map_span_to_source(&overflow_segment, Utf16Offset::new(0), 1).is_err());
}

#[test]
fn cjk_emoji_and_crlf_offsets_survive_the_whole_layout() {
    let text = "Page({\r\n  data: { 名字: '😀', 用户: { 名字: '阿' } },\r\n});\r\n";
    let expr = "用户.名字";
    let fragments = [FragmentSpec::new(
        "cjk",
        expr,
        SourceRange::new(500, 500 + u32::try_from(expr.len()).expect("length")),
    )];
    let built = build(
        "index.ts",
        text,
        RootKind::Page,
        &fragments,
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.view.mounts.len(), 1);
    let record = &built.view.mounts[0];
    assert_eq!(record.target_length_utf16, 2);
    assert_eq!(
        utf16_slice(
            &built.view.text,
            record.start_utf16,
            utf16_len(&record.text)
        ),
        record.text,
        "CRLF line endings and astral characters before the mount do not shift the absolute base"
    );
    let mapped = map_span_to_source(
        record,
        Utf16Offset::new(record.target_start_utf16),
        record.target_length_utf16,
    )
    .expect("mapped");
    assert_eq!(
        mapped.range,
        Some(SourceRange::new(
            500 + 7,
            500 + 13 // `.` is one byte, 名字 is six bytes
        ))
    );
    assert_eq!(utf16_len("\r\n😀名字"), 2 + 2 + 2);
}

#[test]
fn multi_edit_ordering_is_stable_and_offsets_do_not_overlap() {
    let built = build(
        "index.js",
        "\u{FEFF}#!/usr/bin/env node\nComponent({ methods: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Pragma,
        false,
    );
    let mut previous_end = 0;
    for insertion in &built.view.insertions {
        assert!(
            insertion.start_utf16 >= previous_end,
            "insertions must not overlap"
        );
        previous_end =
            insertion.start_utf16 + insertion.replace_length_utf16.max(insertion.length_utf16);
    }
    let ids: Vec<&str> = built
        .view
        .insertions
        .iter()
        .map(|insertion| insertion.insertion_id.as_str())
        .collect();
    assert_eq!(ids, ["bom", "pragma", "probe"]);
}

#[test]
fn shadow_map_round_trips_unchanged_offsets_and_rejects_ambiguous_ones() {
    let original = "abc";
    let assembled = apply_edits(
        original,
        &[ShadowEdit::insert("probe", EditKind::Probe, 1, "XYZ")],
    )
    .expect("valid");
    let map = ShadowMap::new(original, &assembled);
    assert_eq!(
        map.to_derived_offset(Utf16Offset::new(2))
            .expect("after insertion"),
        Utf16Offset::new(5)
    );
    assert!(map.to_derived_offset(Utf16Offset::new(1)).is_err());
    assert_eq!(
        map.to_shadow(MappingLayer::OriginalScript, SourceRange::new(0, 1))
            .expect("left of edit"),
        SourceRange::new(0, 1)
    );
    assert!(
        map.to_shadow(MappingLayer::OriginalScript, SourceRange::new(0, 3))
            .is_err()
    );
    assert!(
        map.to_original(MappingLayer::Template, SourceRange::new(0, 1))
            .is_err()
    );
}

#[test]
fn member_fragments_map_every_handler_offset_back_to_the_wxml_token() {
    let text = "Page({ data: { title: 'x' }, onBackTap() {} });";
    let wxml = "<view bindtap=\"onBackTap\">back</view>";
    let handler = "onBackTap";
    let handler_start =
        u32::try_from(wxml.find(handler).expect("handler in wxml")).expect("offset");
    let handler_end = handler_start + u32::try_from(handler.len()).expect("length");

    let mut fragment = FragmentSpec::new(
        "event#0",
        handler,
        SourceRange::new(handler_start, handler_end),
    );
    fragment.member = true;
    let fragments = [fragment];
    let built = build(
        "index.ts",
        text,
        RootKind::Page,
        &fragments,
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.view.mounts.len(), 1);
    let record = &built.view.mounts[0];
    assert_eq!(record.text, "this.onBackTap");
    assert_eq!(record.target_start_utf16 - record.start_utf16, 5);
    assert_eq!(record.target_length_utf16, 9);

    // Every UTF-16 position inside the mounted name maps to the corresponding
    // WXML byte; the handler name is ASCII, so units and bytes line up.
    let name_start = record.start_utf16 + 5;
    for offset in 0..9_u32 {
        let mapped = map_span_to_source(record, Utf16Offset::new(name_start + offset), 1)
            .expect("valid name offset");
        assert_eq!(
            mapped.range,
            Some(SourceRange::new(
                handler_start + offset,
                handler_start + offset + 1
            )),
            "handler offset {offset}"
        );
        assert!(!mapped.trimmed_synthetic_prefix);
        assert!(!mapped.unattributable);
    }

    // The synthetic `this.` prefix has no source bytes, including the position
    // exactly at its start and the whole-prefix span.
    for offset in 0..5_u32 {
        let mapped = map_span_to_source(record, Utf16Offset::new(record.start_utf16 + offset), 1)
            .expect("valid prefix offset");
        assert_eq!(mapped.range, None, "`this.` offset {offset} is synthetic");
        assert!(mapped.unattributable);
    }
    let whole_prefix =
        map_span_to_source(record, Utf16Offset::new(record.start_utf16), 5).expect("valid prefix");
    assert_eq!(whole_prefix.range, None);
    assert!(whole_prefix.unattributable);
}

#[test]
fn blocked_cases_are_documented_and_not_claimed() {
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-MAPPING-0306")
    );
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-MAPPING-0010")
    );
}
