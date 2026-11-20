//! Tests for the fail-closed post-edit recheck seam.
//!
//! Everything here is in-memory: no workspace, host, handler or disk state is
//! involved except the explicit read-only regression test.

use std::sync::atomic::{AtomicU64, Ordering};

use wxml_text::SourceRange;

use super::*;

/// A provider that asserts the complete staged snapshot exactly.
struct ExactText<'a>(&'a str);

impl TypeCheckProvider for ExactText<'_> {
    fn check(&self, text: &str) -> TypeCheckOutcome {
        assert_eq!(text, self.0, "unexpected staged snapshot");
        TypeCheckOutcome::Clean
    }
}

/// A provider that must never run when an earlier stage rejects the plan.
struct PanicCheck;

impl TypeCheckProvider for PanicCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        panic!("the typecheck stage ran for a rejected edit set");
    }
}

/// A provider that reports one type error.
struct TypeErrorCheck;

impl TypeCheckProvider for TypeErrorCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        TypeCheckOutcome::Errors(vec![RecheckDiagnostic {
            range: range(0, 0),
            message: "type error".to_owned(),
        }])
    }
}

/// A provider with an empty error list, which must not be treated as clean.
struct EmptyErrorCheck;

impl TypeCheckProvider for EmptyErrorCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        TypeCheckOutcome::Errors(Vec::new())
    }
}

/// A provider that reports one more diagnostic than the report retains.
struct ManyDiagnosticsCheck;

impl TypeCheckProvider for ManyDiagnosticsCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        TypeCheckOutcome::Errors(
            (0..=MAX_RECHECK_DIAGNOSTICS)
                .map(|_| RecheckDiagnostic {
                    range: range(0, 0),
                    message: "type error".to_owned(),
                })
                .collect(),
        )
    }
}

/// A provider with one over-long diagnostic message.
struct LongMessageCheck;

impl TypeCheckProvider for LongMessageCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        TypeCheckOutcome::Errors(vec![RecheckDiagnostic {
            range: range(0, 0),
            message: "x".repeat(MAX_RECHECK_DIAGNOSTIC_BYTES + 1),
        }])
    }
}

/// A provider whose diagnostic range is not valid in the snapshot.
struct FixedRangeErrorCheck(SourceRange);

impl TypeCheckProvider for FixedRangeErrorCheck {
    fn check(&self, _text: &str) -> TypeCheckOutcome {
        TypeCheckOutcome::Errors(vec![RecheckDiagnostic {
            range: self.0,
            message: "type error".to_owned(),
        }])
    }
}

fn range(start: usize, end: usize) -> SourceRange {
    SourceRange::from_bounds(
        u32::try_from(start).expect("start"),
        u32::try_from(end).expect("end"),
    )
    .expect("range")
}

fn edit<'a>(text: &'a str, needle: &'a str, replacement: &'a str) -> RecheckEdit<'a> {
    let start = text.find(needle).expect("needle");
    RecheckEdit::new(range(start, start + needle.len()), needle, replacement)
}

#[test]
fn clean_edit_reparses_with_exact_snapshot() {
    let original = r#"<view class="card">{{ user.name }}</view>"#;
    let edits = [edit(original, "user.name", "account.name")];
    let expected = r#"<view class="card">{{ account.name }}</view>"#;
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&ExactText(expected))),
        RecheckReport::Clean
    );
}

#[test]
fn multiple_edits_apply_exactly_once_in_order() {
    let original = r"<view>{{ user.name }} and {{ user.name }}</view>";
    let first = original.find("user").expect("first occurrence");
    let second = original.rfind("user").expect("second occurrence");
    let edits = [
        RecheckEdit::new(range(first, first + "user".len()), "user", "account"),
        RecheckEdit::new(range(second, second + "user".len()), "user", "owner"),
    ];
    // A partial application, an appended replacement or a duplicated
    // replacement all differ from this exact snapshot.
    let expected = r"<view>{{ account.name }} and {{ owner.name }}</view>";
    let snapshot = apply_recheck_edits(original, &edits).expect("complete snapshot");
    assert_eq!(snapshot.text(), expected);
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&ExactText(expected))),
        RecheckReport::Clean
    );
}

#[test]
fn identical_edits_are_idempotent() {
    let original = "<view>{{ user }}</view>";
    let duplicate = edit(original, "user", "account");
    let expected = "<view>{{ account }}</view>";
    let snapshot =
        apply_recheck_edits(original, &[duplicate, duplicate]).expect("deduplicated snapshot");
    assert_eq!(snapshot.text(), expected);
    assert_eq!(
        recheck_edit_snapshot(
            original,
            &[duplicate, duplicate],
            Some(&ExactText(expected))
        ),
        RecheckReport::Clean
    );
}

#[test]
fn introduced_syntax_error_is_reported_before_typecheck() {
    let original = r#"<view class="card">{{ user }}</view>"#;
    let edits = [edit(original, "</view>", "")];
    match recheck_edit_snapshot(original, &edits, Some(&PanicCheck)) {
        RecheckReport::SyntaxErrors {
            stage,
            diagnostics,
            truncated,
            total,
        } => {
            assert_eq!(stage, RecheckStage::Parse);
            assert!(!diagnostics.is_empty(), "parse errors must not be empty");
            assert_eq!(total, diagnostics.len());
            assert!(!truncated);
        }
        other => panic!("expected parse errors, got {other:?}"),
    }
}

#[test]
fn stale_edit_reports_the_input_index_in_a_multi_edit_plan() {
    let original = "<view>{{ user }}</view>";
    let user_start = original.find("user").expect("user");
    let close_start = original.rfind("</view>").expect("closing tag");
    let valid_later = RecheckEdit::new(
        range(close_start, close_start + "</view>".len()),
        "</view>",
        "</view>",
    );
    let stale_earlier = RecheckEdit::new(
        range(user_start, user_start + "user".len()),
        "wrong",
        "account",
    );
    // The stale edit is the second input entry even though it sorts first.
    let edits = [valid_later, stale_earlier];
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&PanicCheck)),
        RecheckReport::StaleEdit { edit: 1 }
    );
    assert!(matches!(
        apply_recheck_edits(original, &edits),
        Err(ApplyFailure::Stale { edit: 1 })
    ));
}

#[test]
fn out_of_bounds_and_empty_plans_report_causes() {
    let original = "<view>{{ user }}</view>";
    let out_of_bounds = [RecheckEdit::new(
        range(original.len() - 1, original.len() + 1),
        ">",
        "x",
    )];
    assert_eq!(
        recheck_edit_snapshot(original, &out_of_bounds, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::Range,
            edit: Some(0),
        }
    );

    let empty: [RecheckEdit<'_>; 0] = [];
    assert_eq!(
        recheck_edit_snapshot(original, &empty, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::Empty,
            edit: None,
        }
    );
}

#[test]
fn overlap_never_reaches_the_typecheck_stage() {
    let original = "<view>{{ user }}</view>";
    let start = original.find("user").expect("user");
    let overlapping = [
        RecheckEdit::new(range(start, start + 3), "use", "xxx"),
        RecheckEdit::new(range(start + 1, start + 4), "ser", "yyy"),
    ];
    assert_eq!(
        recheck_edit_snapshot(original, &overlapping, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::Overlap,
            edit: Some(1),
        }
    );
}

#[test]
fn duplicate_zero_width_anchors_are_invalid() {
    let original = "<view></view>";
    let anchor = original.find('>').expect("opening tag end") + 1;
    let edits = [
        RecheckEdit::new(range(anchor, anchor), "", "a"),
        RecheckEdit::new(range(anchor, anchor), "", "b"),
    ];
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::Overlap,
            edit: Some(1),
        }
    );
}

#[test]
fn dedup_runs_before_the_replacement_budget() {
    let original = "<view></view>";
    let replacement = "x".repeat(MAX_RECHECK_REPLACEMENT_BYTES);
    let duplicate = RecheckEdit::new(range(0, 0), "", replacement.as_str());
    // Two copies would exceed the budget if dedup ran after the sum. The exact
    // snapshot also proves the replacement was applied once, not twice.
    let snapshot =
        apply_recheck_edits(original, &[duplicate, duplicate]).expect("deduplicated snapshot");
    let expected = format!("{replacement}{original}");
    assert_eq!(snapshot.text(), expected.as_str());
}

#[test]
fn replacement_byte_budget_is_cumulative_and_bounded() {
    let original = "<view></view>";
    let chunk = "x".repeat(MAX_RECHECK_REPLACEMENT_BYTES / 2 + 1);
    let start_edit = RecheckEdit::new(range(0, 0), "", chunk.as_str());
    let end_edit = RecheckEdit::new(range(original.len(), original.len()), "", chunk.as_str());
    assert!(matches!(
        apply_recheck_edits(original, &[start_edit, end_edit]),
        Err(ApplyFailure::Invalid {
            cause: InvalidPlanCause::ReplacementTooLarge,
            edit: Some(1),
        })
    ));
    assert_eq!(
        recheck_edit_snapshot(original, &[start_edit, end_edit], Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::ReplacementTooLarge,
            edit: Some(1),
        }
    );

    let at_cap = "x".repeat(MAX_RECHECK_REPLACEMENT_BYTES);
    let at_cap_edit = RecheckEdit::new(range(0, 0), "", at_cap.as_str());
    assert!(apply_recheck_edits(original, &[at_cap_edit]).is_ok());
}

#[test]
fn source_and_snapshot_sizes_are_bounded() {
    let too_large_source = "a".repeat(MAX_RECHECK_SOURCE_BYTES + 1);
    let edit = [RecheckEdit::new(range(0, 0), "", "x")];
    assert_eq!(
        recheck_edit_snapshot(&too_large_source, &edit, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::SourceTooLarge,
            edit: None,
        }
    );

    let at_source_cap = "a".repeat(MAX_RECHECK_SOURCE_BYTES);
    let grow = [RecheckEdit::new(
        range(at_source_cap.len(), at_source_cap.len()),
        "",
        "x",
    )];
    assert_eq!(
        recheck_edit_snapshot(&at_source_cap, &grow, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::SnapshotTooLarge,
            edit: None,
        }
    );

    let fits_snapshot = "a".repeat(MAX_RECHECK_SOURCE_BYTES - 1);
    let fit = [RecheckEdit::new(
        range(fits_snapshot.len(), fits_snapshot.len()),
        "",
        "x",
    )];
    assert!(apply_recheck_edits(&fits_snapshot, &fit).is_ok());
}

#[test]
fn zero_width_insertions_reject_crlf_and_bom_anchors() {
    let crlf = "<view>\r\n</view>";
    let between_crlf = crlf.find("\r\n").expect("crlf") + 1;
    let between = [RecheckEdit::new(range(between_crlf, between_crlf), "", "x")];
    assert_eq!(
        recheck_edit_snapshot(crlf, &between, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::CrlfBoundary,
            edit: Some(0),
        }
    );

    let bom = "\u{feff}<view></view>";
    let before_bom = [RecheckEdit::new(range(0, 0), "", "x")];
    assert_eq!(
        recheck_edit_snapshot(bom, &before_bom, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::BomBoundary,
            edit: Some(0),
        }
    );

    let after_newline = crlf.find('\n').expect("lf") + 1;
    let allowed = [RecheckEdit::new(
        range(after_newline, after_newline),
        "",
        "x",
    )];
    assert!(apply_recheck_edits(crlf, &allowed).is_ok());

    let after_bom = '\u{feff}'.len_utf8();
    let allowed = [RecheckEdit::new(range(after_bom, after_bom), "", "x")];
    assert!(apply_recheck_edits(bom, &allowed).is_ok());
}

#[test]
fn missing_or_empty_typecheck_is_never_clean() {
    let original = "<view>{{ user }}</view>";
    let edits = [edit(original, "user", "account")];
    assert_eq!(
        recheck_edit_snapshot(original, &edits, None),
        RecheckReport::CheckUnavailable
    );
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&UnavailableTypeCheck)),
        RecheckReport::CheckUnavailable
    );
    assert_eq!(
        recheck_edit_snapshot(original, &edits, Some(&EmptyErrorCheck)),
        RecheckReport::CheckUnavailable
    );
}

#[test]
fn utf16_boundary_validation_handles_cjk_and_emoji() {
    let original = r#"<view class="卡片😀">{{ user.name }}</view>"#;
    let valid = [edit(original, "user.name", "account.name")];
    let expected = r#"<view class="卡片😀">{{ account.name }}</view>"#;
    assert_eq!(
        recheck_edit_snapshot(original, &valid, Some(&ExactText(expected))),
        RecheckReport::Clean
    );

    let emoji_start = original.find('😀').expect("emoji");
    let inside_emoji = emoji_start + 1;
    let split_scalar = [RecheckEdit::new(
        range(inside_emoji, inside_emoji + 1),
        "😀",
        "x",
    )];
    assert_eq!(
        recheck_edit_snapshot(original, &split_scalar, Some(&PanicCheck)),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::Range,
            edit: Some(0),
        }
    );
}

#[test]
fn typecheck_errors_use_the_typecheck_stage() {
    let original = "<view>{{ user }}</view>";
    let edits = [edit(original, "user", "account")];
    match recheck_edit_snapshot(original, &edits, Some(&TypeErrorCheck)) {
        RecheckReport::SyntaxErrors {
            stage,
            diagnostics,
            truncated,
            total,
        } => {
            assert_eq!(stage, RecheckStage::TypeCheck);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(total, 1);
            assert!(!truncated);
        }
        other => panic!("expected typecheck errors, got {other:?}"),
    }
}

#[test]
fn diagnostics_are_truncated_with_total_count() {
    let original = "<view>{{ user }}</view>";
    let edits = [edit(original, "user", "account")];
    match recheck_edit_snapshot(original, &edits, Some(&ManyDiagnosticsCheck)) {
        RecheckReport::SyntaxErrors {
            stage,
            diagnostics,
            truncated,
            total,
        } => {
            assert_eq!(stage, RecheckStage::TypeCheck);
            assert_eq!(diagnostics.len(), MAX_RECHECK_DIAGNOSTICS);
            assert_eq!(total, MAX_RECHECK_DIAGNOSTICS + 1);
            assert!(truncated);
        }
        other => panic!("expected bounded typecheck errors, got {other:?}"),
    }
}

#[test]
fn long_diagnostic_messages_are_bounded() {
    let original = "<view>{{ user }}</view>";
    let edits = [edit(original, "user", "account")];
    match recheck_edit_snapshot(original, &edits, Some(&LongMessageCheck)) {
        RecheckReport::SyntaxErrors {
            diagnostics,
            truncated,
            total,
            ..
        } => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].message.len(), MAX_RECHECK_DIAGNOSTIC_BYTES);
            assert!(diagnostics[0].message.bytes().all(|byte| byte == b'x'));
            assert_eq!(total, 1);
            assert!(truncated);
        }
        other => panic!("expected bounded typecheck errors, got {other:?}"),
    }
}

#[test]
fn provider_diagnostic_ranges_are_validated() {
    let original = "<view>😀</view>";
    let edits = [edit(original, "<view>", "<view>")];

    let out_of_bounds = original.len() + 1;
    assert_eq!(
        recheck_edit_snapshot(
            original,
            &edits,
            Some(&FixedRangeErrorCheck(range(
                out_of_bounds,
                out_of_bounds + 1
            )))
        ),
        RecheckReport::CheckUnavailable
    );

    let inside_emoji = original.find('😀').expect("emoji") + 1;
    assert_eq!(
        recheck_edit_snapshot(
            original,
            &edits,
            Some(&FixedRangeErrorCheck(range(inside_emoji, inside_emoji + 1)))
        ),
        RecheckReport::CheckUnavailable
    );
}

#[test]
fn the_edit_count_is_bounded() {
    let original = "a".repeat(MAX_RECHECK_EDITS + 1);
    let edits: Vec<RecheckEdit<'_>> = (0..=MAX_RECHECK_EDITS)
        .map(|offset| RecheckEdit::new(range(offset, offset), "", "x"))
        .collect();
    assert_eq!(
        recheck_edit_snapshot(&original, &edits, None),
        RecheckReport::InvalidPlan {
            cause: InvalidPlanCause::TooManyEdits,
            edit: None,
        }
    );
}

#[test]
fn recheck_is_read_only_across_multiple_calls() {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let original = "<view>{{ user }}</view>";
    let path = std::env::temp_dir().join(format!(
        "wxml-recheck-{}-{}.wxml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, original).expect("write fixture");
    let original_permissions = std::fs::metadata(&path).expect("metadata").permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).expect("mark fixture read-only");

    let on_disk = std::fs::read_to_string(&path).expect("read fixture");
    let edits = [edit(&on_disk, "user", "account")];
    let expected = "<view>{{ account }}</view>";
    for _ in 0..2 {
        assert_eq!(
            recheck_edit_snapshot(&on_disk, &edits, Some(&ExactText(expected))),
            RecheckReport::Clean
        );
    }
    let stale = [RecheckEdit::new(range(0, "<view>".len()), "wrong", "x")];
    assert_eq!(
        recheck_edit_snapshot(&on_disk, &stale, Some(&PanicCheck)),
        RecheckReport::StaleEdit { edit: 0 }
    );
    assert_eq!(on_disk, original);
    assert_eq!(
        std::fs::read_to_string(&path).expect("reread fixture"),
        original
    );

    std::fs::set_permissions(&path, original_permissions).expect("restore permissions");
    std::fs::remove_file(&path).expect("remove fixture");
}
