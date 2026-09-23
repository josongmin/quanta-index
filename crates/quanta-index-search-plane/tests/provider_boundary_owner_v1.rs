//! P07 owner tests (integration surface): semantic admission and the
//! provider boundary (S21-08), exercised through the public search-plane
//! and core APIs only.
//!
//! Registered `provider-boundary-owner-v1` target. Mirrors the ticket `DoD`:
//! locally decidable refusals cost exactly zero provider calls (a counting
//! spy embedder proves it), global reservation/settlement accounting is
//! bounded and overflow-safe, cancellation leaves no detached work against
//! the caps, declared-vs-observed mismatches produce typed failures and no
//! success, and the tokenless/whitespace/punctuation refusal matrix is one
//! common profile-independent matrix for query text and source content.

// The workspace denies `expect`/`panic`/wildcard matches everywhere; this
// owner test asserts on typed refusal values and settles through a spy
// mutex, which is exactly what those lints guard in production paths.
#![expect(
    clippy::expect_used,
    clippy::let_underscore_untyped,
    clippy::panic,
    clippy::significant_drop_in_scrutinee,
    reason = "owner negative-matrix test: asserting on typed error values and spy counters, not production fallibility"
)]
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use quanta_index_core::{
    CoreError, ProviderBudgetLedger, ProviderSettlementKindV1, ProviderSettlementUsageV1,
    ProviderWorkBudgetV1, ProviderWorkEstimateV1, RequestBudgetV1, RequestCorrelationV1,
    SemanticAdmissionEngine, SemanticEgressGrantV1, SemanticEgressPolicyV1, SemanticInputClass,
};
use quanta_index_search_plane::{
    ProviderBoundaryQueryEmbedder, QueryTextEmbedderPort, SEARCH_OWNED_SEMANTIC_DIMENSION,
    SEARCH_OWNED_SEMANTIC_MODEL_ID, SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
    admit_source_derive_content,
};

/// Counting spy embedder: proves a refused request made zero provider
/// calls. Returns a fixed valid unit vector so the happy path settles.
struct SpyEmbedder {
    calls: AtomicUsize,
    vector: Vec<f32>,
    fail_with: Mutex<Option<CoreError>>,
    cancel_during_call: bool,
}

impl SpyEmbedder {
    fn unit() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            vector: vec![1.0],
            fail_with: Mutex::new(None),
            cancel_during_call: false,
        }
    }

    fn returns_after_cancellation() -> Self {
        Self {
            cancel_during_call: true,
            ..Self::unit()
        }
    }

    fn failing(err: CoreError) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            vector: vec![1.0],
            fail_with: Mutex::new(Some(err)),
            cancel_during_call: false,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl QueryTextEmbedderPort for SpyEmbedder {
    fn embed_query(
        &self,
        _query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        let _ = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.cancel_during_call {
            budget.cancel_handle().cancel();
        }
        if let Some(err) = self.fail_with.lock().expect("spy mutex").clone() {
            return Err(err);
        }
        Ok(self.vector.clone())
    }

    fn model_id(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    }
}

fn loopback_boundary(
    spy: Arc<SpyEmbedder>,
    ledger: Arc<ProviderBudgetLedger>,
) -> ProviderBoundaryQueryEmbedder {
    ProviderBoundaryQueryEmbedder::new(
        spy,
        ledger,
        SemanticEgressPolicyV1::Loopback,
        "p07-owner-suite-supervisor",
    )
}

fn open_ledger() -> Arc<ProviderBudgetLedger> {
    Arc::new(ProviderBudgetLedger::new(default_budget()).expect("valid default budget"))
}

fn default_budget() -> ProviderWorkBudgetV1 {
    ProviderWorkBudgetV1 {
        inflight_requests_cap: 2,
        inflight_bytes_cap: 1024,
        total_cost_micros_ceiling: 1_000,
        retry_attempts_cap: 1,
    }
}

fn request_budget() -> RequestBudgetV1 {
    RequestBudgetV1::unbounded()
}

fn refused_code(err: &CoreError) -> &str {
    let CoreError::Typed { code, .. } = err else {
        panic!("expected typed refusal, got {err:?}")
    };
    code.as_wire_str()
}

/// Zero-call refusal matrix: every locally decidable refusal costs exactly
/// zero provider calls and reserves nothing.
#[test]
fn zero_call_refusal_matrix_query_text() {
    for text in ["   ", " \n\t ", "!!! ??? ---", "…—·", ""] {
        let spy = Arc::new(SpyEmbedder::unit());
        let ledger = open_ledger();
        let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
        let err = boundary
            .embed_query_admitted(text, &request_budget())
            .expect_err("tokenless input must be refused");
        assert_eq!(refused_code(&err), "EMPTY_QUERY", "text {text:?}");
        assert_eq!(spy.calls(), 0, "refusal must cost zero provider calls");
        let snap = ledger.snapshot().expect("ledger snapshot");
        assert_eq!(snap.inflight_requests, 0);
        assert_eq!(snap.live_tickets, 0, "refusal must reserve nothing");
    }
}

/// Egress-denied policy: every query is refused before any I/O.
#[test]
fn egress_denied_policy_fails_closed_zero_calls() {
    let spy = Arc::new(SpyEmbedder::unit());
    let ledger = open_ledger();
    let boundary = ProviderBoundaryQueryEmbedder::new(
        spy.clone(),
        Arc::clone(&ledger),
        SemanticEgressPolicyV1::Denied,
        "p07-owner-suite-supervisor",
    );
    let err = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect_err("denied egress must refuse");
    assert_eq!(refused_code(&err), "PROVIDER_EGRESS_DENIED");
    assert_eq!(spy.calls(), 0);
    assert_eq!(ledger.snapshot().expect("snapshot").live_tickets, 0);
}

/// Declared-model mismatch: an external profile declaring a different
/// model than the embedder is refused before any I/O.
#[test]
fn declared_model_mismatch_refused_before_io() {
    let spy = Arc::new(SpyEmbedder::unit());
    let ledger = open_ledger();
    let grant = complete_grant();
    let boundary = ProviderBoundaryQueryEmbedder::new(
        spy.clone(),
        Arc::clone(&ledger),
        SemanticEgressPolicyV1::External(grant),
        "p07-owner-suite-supervisor",
    );
    let err = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect_err("model mismatch must refuse");
    assert_eq!(refused_code(&err), "SEM_MODEL_MISMATCH");
    assert_eq!(spy.calls(), 0);
}

/// One missing egress-grant field is `PROVIDER_EGRESS_DENIED` before I/O.
#[test]
fn incomplete_egress_grant_refused_before_io() {
    let mut grant = complete_grant();
    grant.region = String::new();
    let err = SemanticAdmissionEngine::admit(
        SemanticInputClass::QueryText,
        &SemanticEgressPolicyV1::External(grant),
    )
    .expect_err("incomplete grant must refuse");
    assert_eq!(refused_code(&err), "PROVIDER_EGRESS_DENIED");
}

/// Source content requires its own explicit grant; loopback and
/// consent-less external policies both refuse with zero provider calls.
#[test]
fn source_content_requires_explicit_grant() {
    let err = admit_source_derive_content("fn main() {}", &SemanticEgressPolicyV1::Loopback)
        .expect_err("loopback must not carry source content out");
    assert_eq!(refused_code(&err), "PROVIDER_EGRESS_DENIED");

    let mut grant = complete_grant();
    grant.model_id = SEARCH_OWNED_SEMANTIC_MODEL_ID.to_string();
    grant.model_revision = SEARCH_OWNED_SEMANTIC_MODEL_REVISION.to_string();
    let without_consent = SemanticEgressPolicyV1::External(grant.clone());
    let err = admit_source_derive_content("fn main() {}", &without_consent)
        .expect_err("source consent must be explicit");
    assert_eq!(refused_code(&err), "PROVIDER_EGRESS_DENIED");

    grant.source_content_consent = true;
    let with_consent = SemanticEgressPolicyV1::External(grant);
    admit_source_derive_content("fn main() {}", &with_consent)
        .expect("explicit consent admits source content");
}

/// The tokenless matrix is one common matrix: query text and source
/// content refuse identically under every profile.
#[test]
fn common_tokenless_matrix_shared_by_both_classes() {
    for text in ["   ", "!!!", ""] {
        for class in [
            SemanticInputClass::QueryText,
            SemanticInputClass::SourceContent,
        ] {
            let err = SemanticAdmissionEngine::admit_input_text(class, text)
                .expect_err("tokenless input must refuse");
            assert_eq!(refused_code(&err), "EMPTY_QUERY");
        }
    }
}

/// Reservation/settlement bounds: the inflight request cap refuses typed
/// `SERVER_OVERLOADED` and settles return the ledger to its baseline.
#[test]
fn reservation_settlement_bounds() {
    let ledger = ProviderBudgetLedger::new(default_budget()).expect("valid budget");
    let first = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "supervisor-a",
            None,
        )
        .expect("first reservation fits");
    let second = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "supervisor-a",
            None,
        )
        .expect("second reservation fits the cap of two");
    let err = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "supervisor-a",
            None,
        )
        .expect_err("third reservation must exceed the cap");
    assert_eq!(refused_code(&err), "SERVER_OVERLOADED");

    let usage = ProviderSettlementUsageV1 {
        observed_cost_micros: 0,
        observed_usage_tokens: 8,
    };
    let _ = ledger
        .settle(&first, ProviderSettlementKindV1::Success, usage)
        .expect("settle first");
    let _ = ledger
        .settle(&second, ProviderSettlementKindV1::Cancelled, zero_usage())
        .expect("cancel second");
    let snap = ledger.snapshot().expect("snapshot");
    assert_eq!(snap.inflight_requests, 0);
    assert_eq!(snap.inflight_bytes, 0);
    assert_eq!(snap.live_tickets, 0);
    // After full settlement the cap is available again.
    let again = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "supervisor-a",
            None,
        )
        .expect("cap recovered after settlement");
    assert_eq!(again.ticket_id, 3);
}

/// Settling the same ticket twice is a typed failure, never a silent cap
/// corruption.
#[test]
fn double_settlement_fails_typed() {
    let ledger = ProviderBudgetLedger::new(default_budget()).expect("valid budget");
    let ticket = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "supervisor-a",
            None,
        )
        .expect("reservation");
    let _ = ledger
        .settle(&ticket, ProviderSettlementKindV1::Success, zero_usage())
        .expect("first settlement");
    let err = ledger
        .settle(&ticket, ProviderSettlementKindV1::Success, zero_usage())
        .expect_err("double settlement must fail");
    assert_eq!(refused_code(&err), "REQUEST_CANCELLED");
}

/// Cost accounting is checked: observed cost beyond the reservation is an
/// `InvalidContract` refusal, not a silent overflow.
#[test]
fn observed_cost_beyond_reservation_fails_closed() {
    let ledger = ProviderBudgetLedger::new(default_budget()).expect("valid budget");
    let ticket = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1 {
                inflight_bytes: 10,
                reserved_cost_micros: 50,
                retry_attempts: 0,
            },
            "supervisor-a",
            None,
        )
        .expect("reservation");
    let err = ledger
        .settle(
            &ticket,
            ProviderSettlementKindV1::Success,
            ProviderSettlementUsageV1 {
                observed_cost_micros: 51,
                observed_usage_tokens: 1,
            },
        )
        .expect_err("over-observed cost must fail");
    assert!(matches!(err, CoreError::InvalidContract(_)));
}

/// Zero caps are an invalid budget: fail closed at construction.
#[test]
fn zero_budget_caps_refused() {
    let budget = ProviderWorkBudgetV1 {
        inflight_requests_cap: 0,
        inflight_bytes_cap: 1024,
        total_cost_micros_ceiling: 1_000,
        retry_attempts_cap: 1,
    };
    assert!(ProviderBudgetLedger::new(budget).is_err());
}

/// Cancellation: a cancelled provider call settles `Cancelled` and returns
/// the ledger to its baseline — no detached reservation survives the
/// request that reserved it.
#[test]
fn cancelled_call_settles_and_leaves_no_detached_work() {
    let spy = Arc::new(SpyEmbedder::failing(CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled,
        message: "test cancellation".to_string(),
    }));
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let err = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect_err("cancelled provider call propagates");
    assert_eq!(refused_code(&err), "REQUEST_CANCELLED");
    assert_eq!(spy.calls(), 1, "the call happened once and was settled");
    let snap = ledger.snapshot().expect("snapshot");
    assert_eq!(snap.live_tickets, 0, "cancellation settles its reservation");
    assert_eq!(snap.inflight_requests, 0);
    assert_eq!(snap.inflight_bytes, 0);
}

/// Native inference cannot always stop mid-call. A vector returned after the
/// peer cancelled must never become a success receipt or a served result.
#[test]
fn completed_vector_after_peer_cancellation_is_not_a_success() {
    let spy = Arc::new(SpyEmbedder::returns_after_cancellation());
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let err = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect_err("a cancelled request must not serve a late vector");
    assert_eq!(refused_code(&err), "REQUEST_CANCELLED");
    assert_eq!(spy.calls(), 1);
    let snapshot = ledger.snapshot().expect("snapshot");
    assert_eq!(snapshot.live_tickets, 0);
    assert_eq!(snapshot.inflight_requests, 0);
    let audit = ledger.audit_tail(1).expect("audit tail");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].kind, ProviderSettlementKindV1::Cancelled);
}

#[test]
fn cancelled_before_provider_admission_costs_zero_calls() {
    let spy = Arc::new(SpyEmbedder::unit());
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let budget = request_budget();
    budget.cancel_handle().cancel();
    let err = boundary
        .embed_query_admitted("valid query text", &budget)
        .expect_err("pre-cancelled request must not enter provider I/O");
    assert_eq!(refused_code(&err), "REQUEST_CANCELLED");
    assert_eq!(spy.calls(), 0);
    assert_eq!(ledger.snapshot().expect("snapshot").live_tickets, 0);
    assert!(ledger.audit_tail(1).expect("audit tail").is_empty());
}

/// A failing (non-cancelled) provider call settles `Failed` and the next
/// request still fits the caps.
#[test]
fn failed_call_settles_failed_and_recovers() {
    let spy = Arc::new(SpyEmbedder::failing(CoreError::Storage(
        "provider transport failed".to_string(),
    )));
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let err = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect_err("provider failure propagates");
    assert!(matches!(err, CoreError::Storage(_)));
    assert_eq!(ledger.snapshot().expect("snapshot").live_tickets, 0);
}

/// Happy path: the admitted call settles Success and the outcome carries
/// declared == observed identity with a finite positive norm.
#[test]
fn admitted_call_settles_success_with_outcome() {
    let spy = Arc::new(SpyEmbedder::unit());
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let (vector, outcome) = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect("admitted query embeds");
    assert_eq!(vector.len(), outcome.observed_dimension);
    assert_eq!(outcome.declared_model_id, SEARCH_OWNED_SEMANTIC_MODEL_ID);
    assert_eq!(
        outcome.observed_model_id.as_deref(),
        Some(SEARCH_OWNED_SEMANTIC_MODEL_ID)
    );
    assert!(outcome.all_finite);
    let norm = outcome.vector_norm.expect("local outcomes report the norm");
    assert!(norm.is_finite() && norm > 0.0);
    assert_eq!(spy.calls(), 1);
    assert_eq!(ledger.snapshot().expect("snapshot").live_tickets, 0);
}

/// W10-R2: the audit event carries the settling budget's correlation —
/// reserve stamps it pre-call, settle copies it through the receipt, and
/// the ring event reads it from the receipt, never from ambient state.
/// The off-transport twin on the same ledger stays `None`, proving the
/// ring mixes correlated and uncorrelated events without confusion.
#[test]
fn audit_event_carries_the_settling_budgets_correlation() {
    let spy = Arc::new(SpyEmbedder::unit());
    let ledger = open_ledger();
    let boundary = loopback_boundary(Arc::clone(&spy), Arc::clone(&ledger));
    let correlation = RequestCorrelationV1::from_raw(77).expect("77 is nonzero");
    let correlated = RequestBudgetV1::unbounded().with_correlation(correlation);
    let (correlated_vector, _) = boundary
        .embed_query_admitted("valid query text", &correlated)
        .expect("correlated call settles");
    let (plain_vector, _) = boundary
        .embed_query_admitted("valid query text", &request_budget())
        .expect("uncorrelated call settles");
    assert!(!correlated_vector.is_empty() && !plain_vector.is_empty());
    assert_eq!(spy.calls(), 2);
    let audit = ledger.audit_tail(2).expect("audit tail");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].kind, ProviderSettlementKindV1::Success);
    assert_eq!(audit[1].kind, ProviderSettlementKindV1::Success);
    assert_eq!(audit[0].correlation, Some(correlation));
    assert_eq!(
        audit[0].correlation.map(RequestCorrelationV1::get),
        Some(77)
    );
    assert_eq!(audit[1].correlation, None);
}

/// Declared vs observed: a provider answering with an unexpected model,
/// dimension or non-finite vector produces a typed failure and no success.
#[test]
fn declared_vs_observed_mismatch_is_typed_failure() {
    let declared = (
        SEARCH_OWNED_SEMANTIC_MODEL_ID,
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
        1_usize,
    );

    let mut wrong_model = local_outcome(declared);
    wrong_model.observed_model_id = Some("other-model-v9".to_string());
    let err = wrong_model.validate().expect_err("model mismatch refuses");
    assert_eq!(refused_code(&err), "SEM_MODEL_MISMATCH");

    let mut wrong_revision = local_outcome(declared);
    wrong_revision.observed_model_revision = Some("other-revision".to_string());
    let err = wrong_revision
        .validate()
        .expect_err("revision mismatch refuses");
    assert_eq!(refused_code(&err), "SEM_MODEL_MISMATCH");

    let mut wrong_dimension = local_outcome(declared);
    wrong_dimension.observed_dimension = SEARCH_OWNED_SEMANTIC_DIMENSION;
    let err = wrong_dimension
        .validate()
        .expect_err("dimension mismatch refuses");
    assert_eq!(refused_code(&err), "SEM_DIM_MISMATCH");

    let mut non_finite = local_outcome(declared);
    non_finite.all_finite = false;
    let err = non_finite.validate().expect_err("non-finite refuses");
    assert_eq!(refused_code(&err), "SEM_INVALID_VECTOR");

    let mut bad_norm = local_outcome(declared);
    bad_norm.vector_norm = Some(0.0);
    let err = bad_norm.validate().expect_err("zero norm refuses");
    assert_eq!(refused_code(&err), "SEM_INVALID_VECTOR");

    local_outcome(declared)
        .validate()
        .expect("declared == observed validates");
}

/// A local vector with a non-finite component cannot even construct an
/// outcome: the boundary refuses it typed before settlement succeeds.
#[test]
fn local_non_finite_vector_refused() {
    let err = quanta_index_core::EmbeddingOutcomeV1::from_local_vector(
        &[f32::NAN],
        SEARCH_OWNED_SEMANTIC_MODEL_ID,
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
        1,
    )
    .expect_err("NaN vector must refuse");
    assert_eq!(refused_code(&err), "SEM_INVALID_VECTOR");

    let err = quanta_index_core::EmbeddingOutcomeV1::from_local_vector(
        &[0.0],
        SEARCH_OWNED_SEMANTIC_MODEL_ID,
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
        1,
    )
    .expect_err("zero-norm vector must refuse");
    assert_eq!(refused_code(&err), "SEM_INVALID_VECTOR");
}

/// Retry reservation: an estimate whose retry attempts exceed the cap is
/// refused before any provider work starts.
#[test]
fn retry_cap_reservation_refused() {
    let ledger = ProviderBudgetLedger::new(default_budget()).expect("valid budget");
    let err = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1 {
                inflight_bytes: 10,
                reserved_cost_micros: 0,
                retry_attempts: 2,
            },
            "supervisor-a",
            None,
        )
        .expect_err("retry overrun must refuse");
    assert_eq!(refused_code(&err), "SERVER_OVERLOADED");
}

/// Reserved work must name its supervisor owner; anonymous reservations
/// are invalid contracts.
#[test]
fn anonymous_reservation_refused() {
    let ledger = ProviderBudgetLedger::new(default_budget()).expect("valid budget");
    let err = ledger
        .reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(10),
            "",
            None,
        )
        .expect_err("anonymous reservation must refuse");
    assert!(matches!(err, CoreError::InvalidContract(_)));
}

fn zero_usage() -> ProviderSettlementUsageV1 {
    ProviderSettlementUsageV1 {
        observed_cost_micros: 0,
        observed_usage_tokens: 0,
    }
}

fn complete_grant() -> SemanticEgressGrantV1 {
    SemanticEgressGrantV1 {
        tenant_id: "tenant-a".to_string(),
        provider_id: "provider-x".to_string(),
        endpoint: "https://providers.invalid/embeddings".to_string(),
        region: "us-east-1".to_string(),
        retention: "30d".to_string(),
        model_id: "declared-model-v1".to_string(),
        model_revision: "rev-1".to_string(),
        profile: "profile-a".to_string(),
        source_content_consent: false,
    }
}

fn local_outcome(declared: (&str, &str, usize)) -> quanta_index_core::EmbeddingOutcomeV1 {
    quanta_index_core::EmbeddingOutcomeV1::from_local_vector(
        &[1.0],
        declared.0,
        declared.1,
        declared.2,
    )
    .expect("valid local vector outcome")
}
