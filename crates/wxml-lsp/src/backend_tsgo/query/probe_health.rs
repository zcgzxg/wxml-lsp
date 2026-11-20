//! Probe-health decisions for the in-place shadow.
//!
//! A shadow can look healthy while the injected helpers failed to type-check:
//! a duplicate `__wxml_root`, a probe root that resolves to `any`, or a
//! diagnostic on generated text all produce "no user diagnostics". This module
//! ports the TypeScript `checkProbeHealth` decision table. It consumes facts
//! gathered elsewhere (symbol lookup at exact probe offsets, type quality,
//! module classification, diagnostics, insertions and mounts); it never calls
//! the engine and never searches for a helper by name.
//!
//! This slice deliberately does not wire the result into hover, diagnostics or
//! capability status. Publishing probe health changes the existing `Supported`
//! semantics and needs its own regression slice.

use thiserror::Error;

use crate::backend_tsgo::query::type_info::TypeQuality;
use wxml_backend_tsgo::shadow::layout::InsertionRecord;
use wxml_backend_tsgo::shadow::mapping::MountRecord;

/// Maximum helper-fact entries accepted for one check.
pub(crate) const MAX_PROBE_HELPERS: usize = 256;

/// Maximum diagnostic-fact entries accepted for one check.
pub(crate) const MAX_PROBE_DIAGNOSTICS: usize = 1_000;

/// Maximum insertion records accepted for one check.
pub(crate) const MAX_PROBE_INSERTIONS: usize = 1_000;

/// Maximum mount records accepted for one check.
pub(crate) const MAX_PROBE_MOUNTS: usize = 1_000;

/// Maximum bytes accepted in one helper name or insertion identifier.
pub(crate) const MAX_PROBE_LABEL_BYTES: usize = 256;

/// Maximum problem strings retained in one result.
pub(crate) const MAX_PROBE_PROBLEMS: usize = 64;

/// Module classification before or after deriving the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleClassification {
    /// The file is a script (no import/export).
    Script,
    /// The file is an external module.
    Module,
}

impl ModuleClassification {
    /// Stable marker used in probe-health problems.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Module => "module",
        }
    }
}

/// Resolution of one helper anchor at its exact recorded offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelperResolution {
    /// The exact-offset node resolved to a symbol.
    Symbol,
    /// The exact-offset node exists but has no symbol.
    NoSymbol,
    /// The helper's recorded file is not part of the program.
    OutsideProgram,
}

/// Probe-root evidence gathered for the recorded root offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeRootFact {
    /// No root offset was recorded, so the root check is skipped.
    NotChecked,
    /// The quick-info lookup at the recorded offset returned nothing.
    NoQuickInfo,
    /// Quick-info type quality at the recorded offset.
    Quality(TypeQuality),
}

/// One helper anchor and the symbol evidence gathered for it.
///
/// Entries are one-to-one with [`wxml_backend_tsgo::shadow::layout::ProbeAnchors::helpers`]
/// offsets. Two entries may carry the same name; identity is the offset, not
/// the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProbeHelperFact<'a> {
    /// Helper name as recorded by the layout (for example `__wxml_check_0`).
    pub(crate) name: &'a str,
    /// Exact-offset resolution.
    pub(crate) resolution: HelperResolution,
}

/// One diagnostic fact in derived-script UTF-16 coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProbeDiagnosticFact {
    /// Engine diagnostic code.
    pub(crate) code: u32,
    /// Inclusive UTF-16 start in the derived script.
    pub(crate) start_utf16: u32,
    /// Diagnostic length in UTF-16 code units.
    pub(crate) length_utf16: u32,
}

/// Inputs for one probe-health decision.
///
/// The insertions and mounts all belong to the derived script being checked;
/// the TypeScript oracle filters its records by resolved path, while the native
/// shadow is single-script, so no path field is needed here.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProbeHealthInput<'a> {
    /// Whether the derived script is part of the engine program.
    pub(crate) derived_script_in_program: bool,
    /// Exact-offset helper resolutions.
    pub(crate) helpers: &'a [ProbeHelperFact<'a>],
    /// Probe-root evidence for the recorded root offset.
    pub(crate) root: ProbeRootFact,
    /// Module classification before the derived view.
    pub(crate) module_before: Option<ModuleClassification>,
    /// Module classification after the derived view.
    pub(crate) module_after: Option<ModuleClassification>,
    /// False when template checking is off and no diagnostics are expected.
    pub(crate) check_diagnostics: bool,
    /// Semantic diagnostics in derived-script coordinates.
    pub(crate) diagnostics: &'a [ProbeDiagnosticFact],
    /// Generated insertion records.
    pub(crate) insertions: &'a [InsertionRecord],
    /// Generated mount records.
    pub(crate) mounts: &'a [MountRecord],
}

/// Probe-health problems for one shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeHealth {
    /// Stable problem strings; empty means no probe-health issue was found.
    pub(crate) problems: Vec<String>,
    /// True when [`MAX_PROBE_PROBLEMS`] cut the list short.
    pub(crate) truncated: bool,
}

/// Input beyond this module's documented bounds.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ProbeHealthError {
    /// An input array exceeds its cap.
    #[error("{field} exceeds the {max}-item limit: {count}")]
    TooMany {
        /// Input field name.
        field: &'static str,
        /// Observed item count.
        count: usize,
        /// Accepted maximum.
        max: usize,
    },
    /// A helper name or insertion identifier exceeds its byte cap.
    #[error("{field} exceeds the {max}-byte limit: {bytes}")]
    LabelTooLong {
        /// Input field name.
        field: &'static str,
        /// Observed byte length.
        bytes: usize,
        /// Accepted maximum.
        max: usize,
    },
    /// A span cannot be represented after adding start and length.
    #[error("{field}[{index}] overflows UTF-16 span arithmetic")]
    SpanOverflow {
        /// Input field name.
        field: &'static str,
        /// Zero-based record index.
        index: usize,
    },
}

/// Evaluates the probe-health decision table for one shadow.
///
/// Problems are appended in the oracle's order: helpers, root, module flip,
/// then diagnostics. When the derived script is absent from the program the
/// result contains only `derived script is not part of the program`, matching
/// the early return in the TypeScript oracle.
pub(crate) fn check_probe_health(
    input: &ProbeHealthInput<'_>,
) -> Result<ProbeHealth, ProbeHealthError> {
    validate_input(input)?;

    if !input.derived_script_in_program {
        return Ok(ProbeHealth {
            problems: vec!["derived script is not part of the program".to_owned()],
            truncated: false,
        });
    }

    let mut problems = Vec::new();
    let mut truncated = false;
    collect_helper_problems(input, &mut problems, &mut truncated);
    collect_root_problem(input, &mut problems, &mut truncated);
    collect_module_problem(input, &mut problems, &mut truncated);
    if input.check_diagnostics && !truncated {
        collect_diagnostic_problems(input, &mut problems, &mut truncated)?;
    }

    Ok(ProbeHealth {
        problems,
        truncated,
    })
}

/// Rejects input outside the documented count and label bounds.
fn validate_input(input: &ProbeHealthInput<'_>) -> Result<(), ProbeHealthError> {
    check_count("helpers", input.helpers.len(), MAX_PROBE_HELPERS)?;
    check_count(
        "diagnostics",
        input.diagnostics.len(),
        MAX_PROBE_DIAGNOSTICS,
    )?;
    check_count("insertions", input.insertions.len(), MAX_PROBE_INSERTIONS)?;
    check_count("mounts", input.mounts.len(), MAX_PROBE_MOUNTS)?;
    for helper in input.helpers {
        check_label("helper.name", helper.name)?;
    }
    for insertion in input.insertions {
        check_label("insertion.insertion_id", &insertion.insertion_id)?;
    }
    Ok(())
}

/// Appends one problem for every helper anchor without a symbol.
fn collect_helper_problems(
    input: &ProbeHealthInput<'_>,
    problems: &mut Vec<String>,
    truncated: &mut bool,
) {
    for helper in input.helpers {
        if *truncated {
            break;
        }
        match helper.resolution {
            HelperResolution::Symbol => {}
            HelperResolution::NoSymbol => push_problem(
                problems,
                truncated,
                format!("probe helper {} has no symbol", helper.name),
            ),
            HelperResolution::OutsideProgram => push_problem(
                problems,
                truncated,
                format!(
                    "probe helper {} lives in a file outside the program",
                    helper.name
                ),
            ),
        }
    }
}

/// Appends the probe-root problem, if any.
fn collect_root_problem(
    input: &ProbeHealthInput<'_>,
    problems: &mut Vec<String>,
    truncated: &mut bool,
) {
    let problem = match input.root {
        ProbeRootFact::NotChecked
        | ProbeRootFact::Quality(TypeQuality::Unknown | TypeQuality::Known) => None,
        ProbeRootFact::NoQuickInfo => Some("probe root has no quick info".to_owned()),
        ProbeRootFact::Quality(TypeQuality::Any) => Some("probe root resolves to any".to_owned()),
        ProbeRootFact::Quality(TypeQuality::Missing) => {
            Some("probe root resolves to missing".to_owned())
        }
    };
    if let Some(problem) = problem {
        push_problem(problems, truncated, problem);
    }
}

/// Appends a problem when the module classification changed.
fn collect_module_problem(
    input: &ProbeHealthInput<'_>,
    problems: &mut Vec<String>,
    truncated: &mut bool,
) {
    let (Some(before), Some(after)) = (input.module_before, input.module_after) else {
        return;
    };
    if before != after {
        push_problem(
            problems,
            truncated,
            format!(
                "module classification flipped from {} to {}",
                before.as_str(),
                after.as_str()
            ),
        );
    }
}

/// Appends one problem per diagnostic overlapping a generated insertion.
fn collect_diagnostic_problems(
    input: &ProbeHealthInput<'_>,
    problems: &mut Vec<String>,
    truncated: &mut bool,
) -> Result<(), ProbeHealthError> {
    let mount_ends = input
        .mounts
        .iter()
        .enumerate()
        .map(|(index, mount)| {
            mount
                .end_utf16()
                .map_err(|_| ProbeHealthError::SpanOverflow {
                    field: "mounts",
                    index,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let insertion_ends = input
        .insertions
        .iter()
        .enumerate()
        .map(|(index, insertion)| {
            insertion
                .start_utf16
                .checked_add(insertion.length_utf16)
                .ok_or(ProbeHealthError::SpanOverflow {
                    field: "insertions",
                    index,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (index, diagnostic) in input.diagnostics.iter().enumerate() {
        if *truncated {
            break;
        }
        let end = diagnostic
            .start_utf16
            .checked_add(diagnostic.length_utf16)
            .ok_or(ProbeHealthError::SpanOverflow {
                field: "diagnostics",
                index,
            })?;
        let inside_mount = input
            .mounts
            .iter()
            .zip(&mount_ends)
            .any(|(mount, mount_end)| {
                diagnostic.start_utf16 >= mount.start_utf16 && diagnostic.start_utf16 < *mount_end
            });
        if inside_mount {
            continue;
        }
        let hit =
            input
                .insertions
                .iter()
                .zip(&insertion_ends)
                .find(|(insertion, insertion_end)| {
                    diagnostic.start_utf16 < **insertion_end && end > insertion.start_utf16
                });
        if let Some((insertion, _)) = hit {
            push_problem(
                problems,
                truncated,
                format!(
                    "diagnostic {} intersects insertion {}",
                    diagnostic.code, insertion.insertion_id
                ),
            );
        }
    }
    Ok(())
}

/// Rejects an input array longer than `max`.
const fn check_count(
    field: &'static str,
    count: usize,
    max: usize,
) -> Result<(), ProbeHealthError> {
    if count > max {
        return Err(ProbeHealthError::TooMany { field, count, max });
    }
    Ok(())
}

/// Rejects a helper name or insertion identifier longer than the byte cap.
const fn check_label(field: &'static str, label: &str) -> Result<(), ProbeHealthError> {
    if label.len() > MAX_PROBE_LABEL_BYTES {
        return Err(ProbeHealthError::LabelTooLong {
            field,
            bytes: label.len(),
            max: MAX_PROBE_LABEL_BYTES,
        });
    }
    Ok(())
}

/// Appends one problem unless the result cap is already reached.
fn push_problem(problems: &mut Vec<String>, truncated: &mut bool, problem: String) {
    if *truncated {
        return;
    }
    if problems.len() >= MAX_PROBE_PROBLEMS {
        *truncated = true;
        return;
    }
    problems.push(problem);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wxml_backend_tsgo::shadow::rewrite::EditKind;

    fn helper(name: &str, resolution: HelperResolution) -> ProbeHelperFact<'_> {
        ProbeHelperFact { name, resolution }
    }

    fn insertion(id: &str, start_utf16: u32, length_utf16: u32) -> InsertionRecord {
        InsertionRecord {
            insertion_id: id.to_owned(),
            kind: EditKind::Probe,
            start_utf16,
            length_utf16,
            replace_length_utf16: 0,
        }
    }

    fn mount(start_utf16: u32, text: &str) -> MountRecord {
        MountRecord {
            mount_id: "f0#0".to_owned(),
            fragment_id: "f0".to_owned(),
            insertion_id: "probe".to_owned(),
            text: text.to_owned(),
            start_utf16,
            target_start_utf16: start_utf16,
            target_length_utf16: 0,
            segments: Vec::new(),
        }
    }

    fn input<'a>(helpers: &'a [ProbeHelperFact<'a>]) -> ProbeHealthInput<'a> {
        ProbeHealthInput {
            derived_script_in_program: true,
            helpers,
            root: ProbeRootFact::NotChecked,
            module_before: None,
            module_after: None,
            check_diagnostics: true,
            diagnostics: &[],
            insertions: &[],
            mounts: &[],
        }
    }

    #[test]
    fn case_probes_0064_probe_health_rejects_a_probe_root_that_resolves_to_any() {
        let health = check_probe_health(&ProbeHealthInput {
            root: ProbeRootFact::Quality(TypeQuality::Any),
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(health.problems, ["probe root resolves to any"]);
        assert!(!health.truncated);
    }

    #[test]
    fn case_probes_0080_probe_health_rejects_diagnostics_that_intersect_an_insertion() {
        let insertions = [insertion("probe", 10, 40)];
        let diagnostics = [ProbeDiagnosticFact {
            code: 2339,
            start_utf16: 20,
            length_utf16: 5,
        }];
        let health = check_probe_health(&ProbeHealthInput {
            diagnostics: &diagnostics,
            insertions: &insertions,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["diagnostic 2339 intersects insertion probe"]
        );
    }

    #[test]
    fn case_probes_0098_probe_health_uses_the_exact_offsets_it_is_given_not_a_name_search() {
        // The two entries are distinct `ProbeAnchors` offsets with the same
        // name. Both resolve, so a name-based search would report nothing;
        // the exact-offset list must also report nothing.
        let exact = [
            helper("__wxml_root", HelperResolution::Symbol),
            helper("__wxml_root", HelperResolution::Symbol),
        ];
        let health = check_probe_health(&input(&exact)).expect("input is within bounds");
        assert!(health.problems.is_empty(), "{:?}", health.problems);

        // The second anchor is a keyword at its exact offset. It has no
        // symbol even though the same name resolves elsewhere.
        let keyword = [
            helper("__wxml_root", HelperResolution::Symbol),
            helper("__wxml_root", HelperResolution::NoSymbol),
        ];
        let health = check_probe_health(&input(&keyword)).expect("input is within bounds");
        assert_eq!(health.problems, ["probe helper __wxml_root has no symbol"]);
    }

    #[test]
    fn case_probes_0135_a_duplicate_helper_is_reported_through_the_insertion_check() {
        let insertions = [insertion("probe", 10, 100)];
        let diagnostics = [ProbeDiagnosticFact {
            code: 2300,
            start_utf16: 48,
            length_utf16: 18,
        }];
        let health = check_probe_health(&ProbeHealthInput {
            diagnostics: &diagnostics,
            insertions: &insertions,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["diagnostic 2300 intersects insertion probe"]
        );
    }

    #[test]
    fn case_probes_0153_probe_health_reports_a_module_classification_flip() {
        let health = check_probe_health(&ProbeHealthInput {
            module_before: Some(ModuleClassification::Script),
            module_after: Some(ModuleClassification::Module),
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["module classification flipped from script to module"]
        );
    }

    #[test]
    fn missing_derived_script_is_the_only_problem() {
        let health = check_probe_health(&ProbeHealthInput {
            derived_script_in_program: false,
            root: ProbeRootFact::Quality(TypeQuality::Any),
            module_before: Some(ModuleClassification::Script),
            module_after: Some(ModuleClassification::Module),
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["derived script is not part of the program"]
        );
    }

    #[test]
    fn diagnostics_inside_a_mount_are_skipped() {
        let mounts = [mount(10, "abcde")];
        let insertions = [insertion("probe", 10, 100)];
        let diagnostics = [
            ProbeDiagnosticFact {
                code: 2339,
                start_utf16: 12,
                length_utf16: 2,
            },
            ProbeDiagnosticFact {
                code: 2532,
                start_utf16: 60,
                length_utf16: 2,
            },
        ];
        let health = check_probe_health(&ProbeHealthInput {
            diagnostics: &diagnostics,
            insertions: &insertions,
            mounts: &mounts,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["diagnostic 2532 intersects insertion probe"]
        );
    }

    #[test]
    fn disabled_diagnostics_keep_pre_diagnostic_problems() {
        let helpers = [helper("__wxml_check_0", HelperResolution::NoSymbol)];
        let diagnostics = [ProbeDiagnosticFact {
            code: 2339,
            start_utf16: 20,
            length_utf16: 5,
        }];
        let insertions = [insertion("probe", 10, 40)];
        let health = check_probe_health(&ProbeHealthInput {
            check_diagnostics: false,
            helpers: &helpers,
            diagnostics: &diagnostics,
            insertions: &insertions,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(
            health.problems,
            ["probe helper __wxml_check_0 has no symbol"]
        );
    }

    #[test]
    fn zero_length_and_boundary_diagnostics_do_not_intersect() {
        let insertions = [insertion("probe", 10, 20)];
        let diagnostics = [
            ProbeDiagnosticFact {
                code: 1,
                start_utf16: 30,
                length_utf16: 5,
            },
            ProbeDiagnosticFact {
                code: 2,
                start_utf16: 5,
                length_utf16: 5,
            },
        ];
        let health = check_probe_health(&ProbeHealthInput {
            diagnostics: &diagnostics,
            insertions: &insertions,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert!(health.problems.is_empty(), "{:?}", health.problems);
    }

    #[test]
    fn input_caps_are_typed_errors() {
        let helpers =
            vec![helper("__wxml_check_0", HelperResolution::Symbol); MAX_PROBE_HELPERS + 1];
        assert_eq!(
            check_probe_health(&input(&helpers)),
            Err(ProbeHealthError::TooMany {
                field: "helpers",
                count: MAX_PROBE_HELPERS + 1,
                max: MAX_PROBE_HELPERS,
            })
        );

        let long_name = "x".repeat(MAX_PROBE_LABEL_BYTES + 1);
        let helpers = [helper(&long_name, HelperResolution::Symbol)];
        assert_eq!(
            check_probe_health(&input(&helpers)),
            Err(ProbeHealthError::LabelTooLong {
                field: "helper.name",
                bytes: MAX_PROBE_LABEL_BYTES + 1,
                max: MAX_PROBE_LABEL_BYTES,
            })
        );
    }

    #[test]
    fn overflowing_spans_are_typed_errors() {
        let diagnostics = [ProbeDiagnosticFact {
            code: 1,
            start_utf16: u32::MAX,
            length_utf16: 1,
        }];
        assert_eq!(
            check_probe_health(&ProbeHealthInput {
                diagnostics: &diagnostics,
                ..input(&[])
            }),
            Err(ProbeHealthError::SpanOverflow {
                field: "diagnostics",
                index: 0,
            })
        );
    }

    #[test]
    fn root_quality_is_distinct_from_no_quick_info_and_not_checked() {
        let no_info = check_probe_health(&ProbeHealthInput {
            root: ProbeRootFact::NoQuickInfo,
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(no_info.problems, ["probe root has no quick info"]);

        let missing = check_probe_health(&ProbeHealthInput {
            root: ProbeRootFact::Quality(TypeQuality::Missing),
            ..input(&[])
        })
        .expect("input is within bounds");
        assert_eq!(missing.problems, ["probe root resolves to missing"]);

        let known = check_probe_health(&ProbeHealthInput {
            root: ProbeRootFact::Quality(TypeQuality::Known),
            ..input(&[])
        })
        .expect("input is within bounds");
        assert!(known.problems.is_empty(), "{:?}", known.problems);
    }
}
