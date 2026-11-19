//! Reusable native-parity case and runner contract.
//!
//! P3-T deliberately wires no native backend. A case that needs the engine is
//! reported as [`ParityOutcome::Unavailable`], and a case that carries an
//! integrator decision is reported as [`ParityOutcome::Blocked`]. Neither is a
//! success: [`ParityOutcome::is_pass`] is false and
//! [`ParityOutcome::require_pass`] returns an error. The runner also rejects an
//! empty evidence list and any failed assertion, so a backend cannot turn
//! "nothing ran" into a green parity result.
//!
//! This module never starts a server, a child process, Node, or the network.

use std::fmt;

/// Pinned native engine identity. Every parity verdict must match all four.
pub(crate) const PINNED_ENGINE_VERSION: &str = "7.1.0-beta";
/// Pinned native engine git head.
pub(crate) const PINNED_GIT_HEAD: &str = "3e9236361fa97a9e6c685f4833ac457ae2d8c50a";
/// Pinned AST schema version.
pub(crate) const PINNED_AST_VERSION: u32 = 9;
/// Pinned native engine artifact SHA-256.
pub(crate) const PINNED_ARTIFACT_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";

/// One native-parity partition from `docs/reference/tsgo-test-contract.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Partition {
    /// Stable partition name matching the module and ledger target.
    pub(crate) name: &'static str,
    /// Number of non-obsolete ledger cases routed to this partition.
    pub(crate) routed_cases: usize,
}

impl Partition {
    /// Creates a partition descriptor.
    pub(crate) const fn new(name: &'static str, routed_cases: usize) -> Self {
        Self { name, routed_cases }
    }
}

/// A future native test that must be written before an old silent path can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockedGate {
    /// Destination test function name.
    pub(crate) test_id: &'static str,
    /// Why the gate cannot pass yet.
    pub(crate) reason: &'static str,
}

impl BlockedGate {
    /// Creates a blocked-gate registration.
    #[allow(
        dead_code,
        reason = "symbols_edits authored every registered native edit gate; the type stays for future partitions"
    )]
    pub(crate) const fn new(test_id: &'static str, reason: &'static str) -> Self {
        Self { test_id, reason }
    }
}

/// What a future case needs before it can produce a real verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseGate {
    /// A real native engine query is mandatory.
    NativeEngineRequired,
    /// A native query plus a mandatory post-edit reparse/typecheck is required.
    NativeEngineAndPostEditTypecheck,
    /// An integrator decision retires or defers this case.
    Blocked {
        /// Decision reference.
        reason: &'static str,
    },
}

/// One ledger case or native-risk case that future parity tests can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeParityCase {
    /// Stable ledger case ID (`TS-...`) or a documented native-risk ID.
    pub(crate) case_id: &'static str,
    /// Future Rust test identifier, including its module path.
    pub(crate) target_test_id: &'static str,
    /// Partition module name.
    pub(crate) partition: &'static str,
    /// Required preconditions.
    pub(crate) gate: CaseGate,
}

impl NativeParityCase {
    /// Creates a case that requires the native engine.
    pub(crate) const fn new(
        case_id: &'static str,
        target_test_id: &'static str,
        partition: &'static str,
    ) -> Self {
        Self {
            case_id,
            target_test_id,
            partition,
            gate: CaseGate::NativeEngineRequired,
        }
    }

    /// Creates a case with an explicit gate.
    pub(crate) const fn with_gate(
        case_id: &'static str,
        target_test_id: &'static str,
        partition: &'static str,
        gate: CaseGate,
    ) -> Self {
        Self {
            case_id,
            target_test_id,
            partition,
            gate,
        }
    }

    /// Creates a case retired or deferred by an integrator decision.
    pub(crate) const fn blocked(
        case_id: &'static str,
        target_test_id: &'static str,
        partition: &'static str,
        reason: &'static str,
    ) -> Self {
        Self::with_gate(
            case_id,
            target_test_id,
            partition,
            CaseGate::Blocked { reason },
        )
    }
}

/// Native engine identity that must accompany every real parity run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeIdentity {
    /// Native engine version, for example the pinned tsgo version.
    pub(crate) engine_version: String,
    /// Native engine git head.
    pub(crate) git_head: String,
    /// AST schema version.
    pub(crate) ast_version: u32,
    /// SHA-256 of the engine artifact.
    pub(crate) artifact_sha256: String,
}

impl NativeIdentity {
    /// Returns the pinned identity for future native backends and self-tests.
    pub(crate) fn pinned() -> Self {
        Self {
            engine_version: PINNED_ENGINE_VERSION.to_owned(),
            git_head: PINNED_GIT_HEAD.to_owned(),
            ast_version: PINNED_AST_VERSION,
            artifact_sha256: PINNED_ARTIFACT_SHA256.to_owned(),
        }
    }

    /// Returns true only when every identity field matches the exact pin.
    pub(crate) fn is_complete(&self) -> bool {
        self.engine_version == PINNED_ENGINE_VERSION
            && self.git_head == PINNED_GIT_HEAD
            && self.ast_version == PINNED_AST_VERSION
            && self.artifact_sha256 == PINNED_ARTIFACT_SHA256
    }
}

/// One assertion result produced by a native backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssertionEvidence {
    /// Assertion or contract identifier.
    pub(crate) id: String,
    /// Whether the assertion held.
    pub(crate) passed: bool,
    /// Human-readable evidence.
    pub(crate) detail: String,
}

impl AssertionEvidence {
    /// Creates a passing assertion record, rejecting empty identifiers/details.
    pub(crate) fn passed(
        id: impl Into<String>,
        detail: impl Into<String>,
    ) -> Result<Self, CaseFailure> {
        Self::checked(id.into(), detail.into(), true)
    }

    /// Creates a failing assertion record, rejecting empty identifiers/details.
    pub(crate) fn failed(
        id: impl Into<String>,
        detail: impl Into<String>,
    ) -> Result<Self, CaseFailure> {
        Self::checked(id.into(), detail.into(), false)
    }

    /// Returns true only for a non-empty assertion with a real detail.
    pub(crate) fn is_well_formed(&self) -> bool {
        !self.id.trim().is_empty() && !self.detail.trim().is_empty()
    }

    fn checked(id: String, detail: String, passed: bool) -> Result<Self, CaseFailure> {
        if id.trim().is_empty() {
            return Err(CaseFailure::new("assertion id must not be empty"));
        }
        if detail.trim().is_empty() {
            return Err(CaseFailure::new(format!(
                "assertion {id} detail must not be empty"
            )));
        }
        Ok(Self { id, passed, detail })
    }
}

/// A backend-level failure that is already a test failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaseFailure {
    /// Diagnostic summary.
    pub(crate) summary: String,
}

impl CaseFailure {
    /// Creates a failure with a diagnostic summary.
    pub(crate) fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

/// The only verdict a parity case can produce.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParityOutcome {
    /// The backend produced non-empty evidence and every assertion passed.
    Passed {
        /// Engine identity recorded with the result.
        identity: NativeIdentity,
        /// Assertion evidence.
        evidence: Vec<AssertionEvidence>,
    },
    /// The backend ran and reported a mismatch.
    Failed(CaseFailure),
    /// No usable native backend exists in this wave.
    Unavailable {
        /// Why the case could not run.
        reason: String,
    },
    /// An integrator decision prevents the case from running.
    Blocked {
        /// Decision reference.
        reason: String,
    },
}

impl ParityOutcome {
    /// Returns true only for a valid [`ParityOutcome::Passed`] verdict.
    ///
    /// A manually constructed `Passed` with an unpinned identity, empty
    /// evidence, or malformed assertions is not a pass.
    pub(crate) fn is_pass(&self) -> bool {
        self.require_pass().is_ok()
    }

    /// Returns the explicit reason for a non-passing outcome.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Passed { .. } => None,
            Self::Failed(failure) => Some(failure.summary.as_str()),
            Self::Unavailable { reason } | Self::Blocked { reason } => Some(reason.as_str()),
        }
    }

    /// Converts the outcome into evidence or a hard gate error.
    ///
    /// This re-validates the identity and evidence even for a manually
    /// constructed [`ParityOutcome::Passed`], so a false pass cannot bypass
    /// the runner.
    pub(crate) fn require_pass(&self) -> Result<&[AssertionEvidence], ParityGateError> {
        match self {
            Self::Passed { identity, evidence } => {
                if !identity.is_complete() {
                    return Err(ParityGateError::Unavailable(
                        "parity pass does not carry the pinned native identity".to_owned(),
                    ));
                }
                if evidence.is_empty() {
                    return Err(ParityGateError::Failed(
                        "empty evidence is not a parity pass".to_owned(),
                    ));
                }
                let malformed: Vec<&str> = evidence
                    .iter()
                    .filter(|assertion| !assertion.is_well_formed())
                    .map(|assertion| assertion.id.as_str())
                    .collect();
                if !malformed.is_empty() {
                    return Err(ParityGateError::Failed(format!(
                        "malformed assertion evidence: {}",
                        malformed.join(", ")
                    )));
                }
                let failed: Vec<&str> = evidence
                    .iter()
                    .filter(|assertion| !assertion.passed)
                    .map(|assertion| assertion.id.as_str())
                    .collect();
                if !failed.is_empty() {
                    return Err(ParityGateError::Failed(format!(
                        "failed assertions: {}",
                        failed.join(", ")
                    )));
                }
                Ok(evidence)
            }
            Self::Failed(failure) => Err(ParityGateError::Failed(failure.summary.clone())),
            Self::Unavailable { reason } => Err(ParityGateError::Unavailable(reason.clone())),
            Self::Blocked { reason } => Err(ParityGateError::Blocked(reason.clone())),
        }
    }
}

/// A non-passing outcome, preserved as a typed error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParityGateError {
    /// Backend mismatch.
    Failed(String),
    /// No backend or incomplete identity.
    Unavailable(String),
    /// Integrator decision.
    Blocked(String),
}

impl fmt::Display for ParityGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(reason) => write!(formatter, "parity case failed: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "parity case unavailable: {reason}"),
            Self::Blocked(reason) => write!(formatter, "parity case blocked: {reason}"),
        }
    }
}

impl std::error::Error for ParityGateError {}

/// A native backend that future waves must provide.
pub(crate) trait NativeParityBackend {
    /// Returns the pinned engine identity for this run.
    fn identity(&self) -> &NativeIdentity;

    /// Evaluates one case. An empty `Ok` vector is treated as a failure.
    fn evaluate(&self, case: &NativeParityCase) -> Result<Vec<AssertionEvidence>, CaseFailure>;
}

/// Runs native-parity cases without starting a server or any process.
pub(crate) struct ParityRunner<'a> {
    backend: Option<&'a dyn NativeParityBackend>,
}

impl<'a> ParityRunner<'a> {
    /// Creates a runner with no native backend.
    ///
    /// Every runnable case is `Unavailable`, never a pass.
    pub(crate) const fn without_backend() -> Self {
        Self { backend: None }
    }

    /// Creates a runner around a real or fake backend.
    pub(crate) fn with_backend(backend: &'a dyn NativeParityBackend) -> Self {
        Self {
            backend: Some(backend),
        }
    }

    /// Runs one case and fails closed.
    pub(crate) fn run(&self, case: &NativeParityCase) -> ParityOutcome {
        if let CaseGate::Blocked { reason } = case.gate {
            return ParityOutcome::Blocked {
                reason: format!("{}: {reason}", case.case_id),
            };
        }

        let Some(backend) = self.backend else {
            return ParityOutcome::Unavailable {
                reason: format!(
                    "{}: native backend is not wired in P3-T (target {})",
                    case.case_id, case.target_test_id
                ),
            };
        };

        let identity = backend.identity();
        if !identity.is_complete() {
            return ParityOutcome::Unavailable {
                reason: format!(
                    "{}: native backend identity does not match the pinned engine",
                    case.case_id
                ),
            };
        }

        match backend.evaluate(case) {
            Err(failure) => ParityOutcome::Failed(failure),
            Ok(evidence) if evidence.is_empty() => ParityOutcome::Failed(CaseFailure::new(
                format!("{}: empty evidence is not a parity pass", case.case_id),
            )),
            Ok(evidence) => {
                let malformed: Vec<&str> = evidence
                    .iter()
                    .filter(|assertion| !assertion.is_well_formed())
                    .map(|assertion| assertion.id.as_str())
                    .collect();
                if !malformed.is_empty() {
                    return ParityOutcome::Failed(CaseFailure::new(format!(
                        "{}: malformed assertion evidence: {}",
                        case.case_id,
                        malformed.join(", ")
                    )));
                }
                let failed: Vec<&str> = evidence
                    .iter()
                    .filter(|assertion| !assertion.passed)
                    .map(|assertion| assertion.id.as_str())
                    .collect();
                if failed.is_empty() {
                    ParityOutcome::Passed {
                        identity: identity.clone(),
                        evidence,
                    }
                } else {
                    ParityOutcome::Failed(CaseFailure::new(format!(
                        "{}: failed assertions: {}",
                        case.case_id,
                        failed.join(", ")
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASE: NativeParityCase = NativeParityCase::new(
        "HARNESS-SELF-CHECK",
        "crates/wxml-lsp/tests/native_parity/harness.rs::self_check",
        "hover_docs",
    );

    fn good_evidence() -> AssertionEvidence {
        AssertionEvidence::passed("a", "exact match").expect("valid evidence")
    }

    struct FakeBackend {
        identity: NativeIdentity,
        result: Result<Vec<AssertionEvidence>, CaseFailure>,
    }

    impl FakeBackend {
        fn passing(evidence: Vec<AssertionEvidence>) -> Self {
            Self {
                identity: NativeIdentity::pinned(),
                result: Ok(evidence),
            }
        }

        fn failing(summary: &str) -> Self {
            Self {
                identity: NativeIdentity::pinned(),
                result: Err(CaseFailure::new(summary)),
            }
        }
    }

    impl NativeParityBackend for FakeBackend {
        fn identity(&self) -> &NativeIdentity {
            &self.identity
        }

        fn evaluate(
            &self,
            _case: &NativeParityCase,
        ) -> Result<Vec<AssertionEvidence>, CaseFailure> {
            self.result.clone()
        }
    }

    #[test]
    fn missing_backend_is_unavailable_not_a_pass() {
        let outcome = ParityRunner::without_backend().run(&CASE);
        assert!(!outcome.is_pass());
        assert!(matches!(outcome, ParityOutcome::Unavailable { .. }));
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("HARNESS-SELF-CHECK"), "{reason}");
        assert!(matches!(
            outcome.require_pass(),
            Err(ParityGateError::Unavailable(_))
        ));
    }

    #[test]
    fn blocked_case_stays_blocked_even_with_a_backend() {
        let backend = FakeBackend::passing(vec![good_evidence()]);
        let case = NativeParityCase::blocked(
            "HARNESS-BLOCKED",
            "crates/wxml-lsp/tests/native_parity/harness.rs::blocked",
            "scenario_matrix",
            "integrator retired the case",
        );
        let outcome = ParityRunner::with_backend(&backend).run(&case);
        assert!(!outcome.is_pass());
        assert!(matches!(outcome, ParityOutcome::Blocked { .. }));
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("HARNESS-BLOCKED"), "{reason}");
    }

    #[test]
    fn empty_evidence_is_a_failure_not_a_pass() {
        let backend = FakeBackend::passing(Vec::new());
        let outcome = ParityRunner::with_backend(&backend).run(&CASE);
        assert!(!outcome.is_pass());
        assert!(matches!(outcome, ParityOutcome::Failed(_)));
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("empty evidence"), "{reason}");
    }

    #[test]
    fn failed_assertion_is_a_failure() {
        let backend = FakeBackend::passing(vec![
            good_evidence(),
            AssertionEvidence::failed("b", "mismatch").expect("valid evidence"),
        ]);
        let outcome = ParityRunner::with_backend(&backend).run(&CASE);
        assert!(!outcome.is_pass());
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("failed assertions: b"), "{reason}");
    }

    #[test]
    fn malformed_assertion_evidence_is_a_failure() {
        let backend = FakeBackend::passing(vec![AssertionEvidence {
            id: String::new(),
            passed: true,
            detail: String::new(),
        }]);
        let outcome = ParityRunner::with_backend(&backend).run(&CASE);
        assert!(!outcome.is_pass());
        assert!(matches!(outcome, ParityOutcome::Failed(_)));
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("malformed assertion evidence"), "{reason}");
    }

    #[test]
    fn assertion_constructors_reject_empty_text() {
        assert!(AssertionEvidence::passed("", "").is_err());
        assert!(AssertionEvidence::passed("id", "").is_err());
        assert!(AssertionEvidence::passed("", "detail").is_err());
        assert!(AssertionEvidence::failed("id", " ").is_err());
    }

    #[test]
    fn backend_error_is_a_failure() {
        let backend = FakeBackend::failing("engine query returned a mismatch");
        let outcome = ParityRunner::with_backend(&backend).run(&CASE);
        assert!(!outcome.is_pass());
        assert!(matches!(outcome, ParityOutcome::Failed(_)));
        let reason = outcome.reason().expect("reason");
        assert!(reason.contains("mismatch"), "{reason}");
    }

    #[test]
    fn wrong_pinned_identity_is_unavailable() {
        let pinned = NativeIdentity::pinned();
        let wrong = [
            NativeIdentity {
                engine_version: "7.1.0".to_owned(),
                ..pinned.clone()
            },
            NativeIdentity {
                git_head: "b9616c5".to_owned(),
                ..pinned.clone()
            },
            NativeIdentity {
                ast_version: 8,
                ..pinned.clone()
            },
            NativeIdentity {
                artifact_sha256: "b".repeat(64),
                ..pinned
            },
        ];
        for identity in wrong {
            let backend = FakeBackend {
                identity,
                result: Ok(vec![good_evidence()]),
            };
            let outcome = ParityRunner::with_backend(&backend).run(&CASE);
            assert!(!outcome.is_pass());
            assert!(matches!(outcome, ParityOutcome::Unavailable { .. }));
            let reason = outcome.reason().expect("reason");
            assert!(reason.contains("pinned engine"), "{reason}");
        }
    }

    #[test]
    fn require_pass_revalidates_identity_and_evidence() {
        let identity = NativeIdentity::pinned();
        let empty = ParityOutcome::Passed {
            identity: identity.clone(),
            evidence: Vec::new(),
        };
        assert!(!empty.is_pass());
        assert!(matches!(
            empty.require_pass(),
            Err(ParityGateError::Failed(_))
        ));

        let malformed = ParityOutcome::Passed {
            identity: identity.clone(),
            evidence: vec![AssertionEvidence {
                id: String::new(),
                passed: true,
                detail: String::new(),
            }],
        };
        assert!(!malformed.is_pass());
        assert!(matches!(
            malformed.require_pass(),
            Err(ParityGateError::Failed(_))
        ));

        let failed = ParityOutcome::Passed {
            identity,
            evidence: vec![AssertionEvidence {
                id: "a".to_owned(),
                passed: false,
                detail: "mismatch".to_owned(),
            }],
        };
        assert!(!failed.is_pass());
        assert!(matches!(
            failed.require_pass(),
            Err(ParityGateError::Failed(_))
        ));

        let wrong_identity = ParityOutcome::Passed {
            identity: NativeIdentity {
                engine_version: "7.1.0".to_owned(),
                ..NativeIdentity::pinned()
            },
            evidence: vec![good_evidence()],
        };
        assert!(!wrong_identity.is_pass());
        assert!(matches!(
            wrong_identity.require_pass(),
            Err(ParityGateError::Unavailable(_))
        ));

        let valid = ParityOutcome::Passed {
            identity: NativeIdentity::pinned(),
            evidence: vec![good_evidence()],
        };
        assert!(valid.is_pass());
    }

    #[test]
    fn non_empty_evidence_from_a_pinned_backend_is_a_pass() {
        let backend = FakeBackend::passing(vec![good_evidence()]);
        let case = NativeParityCase::with_gate(
            "HARNESS-SELF-CHECK-PASS",
            "crates/wxml-lsp/tests/native_parity/harness.rs::self_check_pass",
            "symbols_edits",
            CaseGate::NativeEngineAndPostEditTypecheck,
        );
        let outcome = ParityRunner::with_backend(&backend).run(&case);
        assert!(outcome.is_pass());
        let evidence = outcome.require_pass().expect("evidence");
        assert_eq!(evidence.len(), 1);
        match outcome {
            ParityOutcome::Passed { identity, .. } => {
                assert_eq!(identity.engine_version, PINNED_ENGINE_VERSION);
                assert_eq!(identity.git_head, PINNED_GIT_HEAD);
                assert_eq!(identity.ast_version, PINNED_AST_VERSION);
                assert_eq!(identity.artifact_sha256, PINNED_ARTIFACT_SHA256);
            }
            other => panic!("expected a pass, got {other:?}"),
        }
    }
}
