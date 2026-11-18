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
    assert!(mapped.trimmed_synthetic_prefix);
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
    assert!(map_span_to_source(&record, Utf16Offset::new(12), 5).is_err());
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
    assert!(map_span_to_source(&record, Utf16Offset::new(100), 15).is_err());
    assert!(map_span_to_source(&record, Utf16Offset::new(99), 2).is_err());
    assert!(map_span_to_source(&record, Utf16Offset::new(113), 1).is_err());
    assert!(map_span_to_source(&record, Utf16Offset::new(112), 1).is_err());
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
    assert_eq!(built.view.mounts.len(), 2);
    for record in &built.view.mounts {
        assert!(!record.segments.is_empty(), "{}", record.mount_id);
        let mut cursor = record.start_utf16;
        let mut source_bytes = 0_u32;
        let mut has_source = false;
        let mut has_synthetic = false;
        for segment in &record.segments {
            assert_eq!(segment.start_utf16, cursor, "{}", record.mount_id);
            cursor += segment.length_utf16;
            match segment.source {
                Some(source) => {
                    has_source = true;
                    source_bytes += source.len();
                }
                None => has_synthetic = true,
            }
        }
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
fn case_mapping_0223_bom_is_dropped_whenever_a_pragma_must_be_honoured() {
    let built = build(
        "index.js",
        "\u{FEFF}#!/usr/bin/env node\nComponent({ methods: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Pragma,
        false,
    );
    assert!(!built.view.text.starts_with('\u{FEFF}'));
    assert!(built.view.text.starts_with("#!"));
    assert!(built.notes.iter().any(|note| note.contains("BOM dropped")));
    let bom = built
        .view
        .insertions
        .iter()
        .find(|insertion| insertion.insertion_id == "bom")
        .expect("BOM edit");
    assert_eq!(bom.replace_length_utf16, 1);
}

#[test]
fn case_mapping_0239_published_evidence_carries_insertions_and_mounts_for_every_run() {
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
    assert!(!built.view.insertions.is_empty());
    assert!(!built.view.mounts.is_empty());
    let ids: std::collections::BTreeSet<&str> = built
        .view
        .insertions
        .iter()
        .map(|insertion| insertion.insertion_id.as_str())
        .collect();
    for record in &built.view.mounts {
        assert!(ids.contains(record.insertion_id.as_str()));
        assert!(!record.segments.is_empty());
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
    let record = &built.view.mounts[0];
    assert_eq!(record.target_length_utf16, 2);
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
