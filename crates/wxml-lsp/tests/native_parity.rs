//! Native parity harness and machine-enforced migration ledger gates.
//!
//! P3-T freezes the oracle contract: the ledger inventory, fixture v1 hashes,
//! ignored/silent-skip gates, and the 16 partition skeletons. No native engine,
//! server, Node process, or network request is started here. The `symbols_edits`
//! partition declares the three authored native edit gates; every other
//! partition remains an empty blocked skeleton. `ParityRunner::without_backend`
//! reports `Unavailable`, never a pass.

#![allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::redundant_pub_crate,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

#[path = "native_parity/behaviors.rs"]
mod behaviors;
#[path = "native_parity/completion.rs"]
mod completion;
#[path = "native_parity/components.rs"]
mod components;
#[path = "native_parity/definitions.rs"]
mod definitions;
#[path = "native_parity/events.rs"]
mod events;
#[path = "native_parity/fixture_hash.rs"]
mod fixture_hash;
#[path = "native_parity/guards_js.rs"]
mod guards_js;
#[path = "native_parity/harness.rs"]
mod harness;
#[path = "native_parity/hover_docs.rs"]
mod hover_docs;
#[path = "native_parity/ledger.rs"]
mod ledger;
#[path = "native_parity/loops.rs"]
mod loops;
#[path = "native_parity/mapping_utf16.rs"]
mod mapping_utf16;
#[path = "native_parity/messages_evidence.rs"]
mod messages_evidence;
#[path = "native_parity/native_engine_real.rs"]
mod native_engine_real;
#[path = "native_parity/native_gate_env.rs"]
mod native_gate_env;
#[path = "native_parity/native_gates.rs"]
mod native_gates;
#[path = "native_parity/native_lsp.rs"]
mod native_lsp;
#[cfg(target_os = "linux")]
#[path = "native_parity/native_measure.rs"]
mod native_measure;
#[path = "native_parity/native_package.rs"]
mod native_package;
#[path = "native_parity/oracle_manifest.rs"]
mod oracle_manifest;
#[path = "native_parity/perf_budget.rs"]
mod perf_budget;
#[path = "native_parity/project_overlay.rs"]
mod project_overlay;
#[path = "native_parity/protocol_limits.rs"]
mod protocol_limits;
#[path = "native_parity/real_matrix.rs"]
mod real_matrix;
#[path = "native_parity/scenario_matrix.rs"]
mod scenario_matrix;
#[path = "native_parity/shadow_rewrite.rs"]
mod shadow_rewrite;
#[path = "native_parity/source_scan.rs"]
mod source_scan;
#[path = "native_parity/symbols_edits.rs"]
mod symbols_edits;

use std::collections::{BTreeMap, BTreeSet};

use harness::{NativeParityCase, ParityOutcome, ParityRunner, Partition};

struct PartitionSkeleton {
    module: &'static str,
    partition: Partition,
    migrated_cases: &'static [&'static str],
}

const PARTITIONS: &[PartitionSkeleton] = &[
    PartitionSkeleton {
        module: "hover_docs",
        partition: hover_docs::PARTITION,
        migrated_cases: hover_docs::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "definitions",
        partition: definitions::PARTITION,
        migrated_cases: definitions::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "completion",
        partition: completion::PARTITION,
        migrated_cases: completion::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "loops",
        partition: loops::PARTITION,
        migrated_cases: loops::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "guards_js",
        partition: guards_js::PARTITION,
        migrated_cases: guards_js::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "components",
        partition: components::PARTITION,
        migrated_cases: components::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "behaviors",
        partition: behaviors::PARTITION,
        migrated_cases: behaviors::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "events",
        partition: events::PARTITION,
        migrated_cases: events::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "project_overlay",
        partition: project_overlay::PARTITION,
        migrated_cases: project_overlay::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "symbols_edits",
        partition: symbols_edits::PARTITION,
        migrated_cases: symbols_edits::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "scenario_matrix",
        partition: scenario_matrix::PARTITION,
        migrated_cases: scenario_matrix::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "protocol_limits",
        partition: protocol_limits::PARTITION,
        migrated_cases: protocol_limits::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "messages_evidence",
        partition: messages_evidence::PARTITION,
        migrated_cases: messages_evidence::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "mapping_utf16",
        partition: mapping_utf16::PARTITION,
        migrated_cases: mapping_utf16::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "shadow_rewrite",
        partition: shadow_rewrite::PARTITION,
        migrated_cases: shadow_rewrite::MIGRATED_CASES,
    },
    PartitionSkeleton {
        module: "perf_budget",
        partition: perf_budget::PARTITION,
        migrated_cases: perf_budget::MIGRATED_CASES,
    },
];

const SYMBOLS_EDITS_MIGRATED_CASES: &[&str] = &[
    "native_component_property_edit_applies_and_reparses",
    "assert_native_edit_reparses_and_typechecks",
    "native_rejects_cross_namespace_and_dynamic_edits",
];

const SCENARIO_MATRIX_MIGRATED_CASES: &[&str] = &[
    "real_matrix_manifest_is_frozen_and_oracle_derived",
    "real_matrix_comparator_rejects_missing_shifted_and_perturbed_results",
    "real_matrix_in_place_matches_frozen_oracle",
    "real_matrix_known_gaps_are_characterized",
    "real_matrix_top_level_cases",
];

/// The file that declares the `scenario_matrix` migrated tests.
///
/// The ledger target stays `scenario_matrix.rs` (43 routed cases); the real
/// engine tests live in the sibling `real_matrix.rs` so the thin wrapper keeps
/// the target path stable while the tests can use the crate harness modules.
const SCENARIO_MATRIX_TEST_FILE: &str = "real_matrix.rs";

#[test]
fn partition_skeletons_match_manifest_and_fail_closed() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let cases = ledger::required_array(root, "cases", "ledger root");
    let routed = ledger::non_obsolete_case_count(cases);

    let runner = ParityRunner::without_backend();
    let mut total = 0_usize;

    for skeleton in PARTITIONS {
        let expected = match skeleton.module {
            "symbols_edits" => SYMBOLS_EDITS_MIGRATED_CASES,
            "scenario_matrix" => SCENARIO_MATRIX_MIGRATED_CASES,
            _ => &[],
        };
        assert_eq!(
            skeleton.migrated_cases, expected,
            "{} migrated cases drifted from the partition manifest",
            skeleton.module
        );
        assert_eq!(skeleton.partition.name, skeleton.module);

        let case = NativeParityCase::new(
            "P3-T-SKELETON",
            "crates/wxml-lsp/tests/native_parity.rs::partition_skeletons_match_manifest_and_fail_closed",
            skeleton.partition.name,
        );
        let outcome = runner.run(&case);
        assert!(
            matches!(outcome, ParityOutcome::Unavailable { .. }),
            "{} returned {outcome:?}; a skeleton case must be Unavailable",
            skeleton.module
        );
        assert!(!outcome.is_pass(), "{} must never pass", skeleton.module);
        let reason = outcome.reason().expect("unavailable reason");
        assert!(
            reason.contains("P3-T-SKELETON"),
            "{} reason must identify the case: {reason}",
            skeleton.module
        );
        total += skeleton.partition.routed_cases;
    }

    assert_eq!(PARTITIONS.len(), 16);
    assert_eq!(
        total, routed,
        "all 16 partitions must cover every non-obsolete ledger case"
    );
}

#[test]
fn partition_routed_counts_match_ledger_targets() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let cases = ledger::required_array(root, "cases", "ledger root");

    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    for case in cases {
        let case = ledger::object(case, "case");
        if ledger::required_str(case, "disposition", "case") == "obsolete" {
            continue;
        }
        let target = ledger::required_str(case, "target_rust_test", "case");
        let module = ledger::target_partition(target);
        *expected.entry(module.to_owned()).or_default() += 1;
    }
    assert_eq!(
        expected.len(),
        16,
        "all 16 partition targets must be routed"
    );

    let mut declared = 0_usize;
    let mut created = Vec::new();
    for skeleton in PARTITIONS {
        assert_eq!(
            expected.get(skeleton.module).copied().unwrap_or_default(),
            skeleton.partition.routed_cases,
            "{} count drifted from the ledger target paths",
            skeleton.module
        );
        declared += skeleton.partition.routed_cases;
        created.push(skeleton.module);
    }
    assert_eq!(
        declared,
        ledger::non_obsolete_case_count(cases),
        "partition counts must cover every non-obsolete ledger case"
    );

    created.sort_unstable();
    let mut expected_partitions = ledger::PARTITION_NAMES.to_vec();
    expected_partitions.sort_unstable();
    assert_eq!(created, expected_partitions);

    let covered: BTreeSet<&str> = created.iter().copied().collect();
    assert_eq!(
        covered,
        ledger::PARTITION_NAMES.iter().copied().collect(),
        "created partitions must equal the contract partition set"
    );
    assert_eq!(
        covered.len(),
        expected.len(),
        "no pending partition modules remain"
    );
    assert!(
        expected
            .keys()
            .all(|module| covered.contains(module.as_str())),
        "a ledger target module is not declared as a partition"
    );
}

#[test]
fn partition_modules_declare_only_listed_tests() {
    let workspace = ledger::workspace_root();
    let partition_root = workspace.join("crates/wxml-lsp/tests/native_parity");
    for skeleton in PARTITIONS {
        let path = if skeleton.module == "scenario_matrix" {
            partition_root.join(SCENARIO_MATRIX_TEST_FILE)
        } else {
            partition_root.join(format!("{}.rs", skeleton.module))
        };
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let lines: Vec<&str> = source.lines().collect();
        let mut declared = BTreeSet::new();
        for (index, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with("#[test]") {
                continue;
            }
            let name = lines
                .iter()
                .skip(index + 1)
                .take(16)
                .find_map(|line| source_scan::parse_fn_name(line))
                .unwrap_or_else(|| {
                    panic!("{} has a #[test] without a following fn", skeleton.module)
                });
            assert!(
                declared.insert(name),
                "{} declares a duplicate #[test]",
                skeleton.module
            );
        }
        let listed: BTreeSet<&str> = skeleton.migrated_cases.iter().copied().collect();
        assert_eq!(
            skeleton.migrated_cases.len(),
            listed.len(),
            "{} has duplicate MIGRATED_CASES entries",
            skeleton.module
        );
        assert_eq!(
            declared,
            listed
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "{} #[test] functions must exactly match MIGRATED_CASES",
            skeleton.module
        );
    }
}
