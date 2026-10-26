use std::sync::Arc;

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::EmbeddingNormalization;
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError, ProviderAuditEventV1, ProviderBudgetLedger, ProviderSettlementKindV1,
    ProviderSettlementReceiptV1, ProviderSettlementUsageV1, ProviderWorkEstimateV1,
    RequestBudgetV1, RequestProviderStageV1, SemanticAdmissionEngine, SemanticEgressPolicyV1,
    SemanticInputClass, SemanticNormalizationFailureV1, SemanticPolicy, TextEmbeddingProvider,
    EMBED_CHECKPOINT,
};

/// Scalar failure from the canonical hash and normalization path. It can be
/// retained by an admitted caller without constructing a diagnostic String.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HashingQueryTextFailureV1 {
    DimensionMismatch,
    ZeroDimension,
    DimensionConversion,
    SlotConversion {
        slot: u64,
    },
    SlotOutOfBounds {
        slot: usize,
        dimension: usize,
        primary: bool,
    },
    EmptyQuery,
    Normalization(SemanticNormalizationFailureV1),
}

impl HashingQueryTextFailureV1 {
    /// Preserve the existing search-plane error taxonomy without rendering it.
    #[must_use]
    pub fn code_v1(self) -> SearchPlaneErrorCodeV2 {
        match self {
            Self::DimensionMismatch
            | Self::ZeroDimension
            | Self::DimensionConversion
            | Self::SlotConversion { .. } => SearchPlaneErrorCodeV2::InvalidRequest,
            Self::SlotOutOfBounds { .. } => SearchPlaneErrorCodeV2::Internal,
            Self::EmptyQuery => LexicalErrorCode::EmptyQuery.into(),
            Self::Normalization(_) => LexicalErrorCode::SemInvalidVector.into(),
        }
    }

    /// A static diagnostic retained without allocating a String.
    #[must_use]
    pub const fn detail_v1(self) -> &'static str {
        match self {
            Self::DimensionMismatch => "semantic: hashing embedder output dimension mismatch",
            Self::ZeroDimension => "semantic: hashing embedder dimension must be non-zero",
            Self::DimensionConversion => "semantic: hashing embedder dimension conversion failed",
            Self::SlotConversion { .. } => "semantic: hashed slot conversion failed",
            Self::SlotOutOfBounds { primary: true, .. } => {
                "semantic: primary hashed slot out of bounds"
            }
            Self::SlotOutOfBounds { primary: false, .. } => {
                "semantic: secondary hashed slot out of bounds"
            }
            Self::EmptyQuery => "semantic: query text must contain at least one alphanumeric token",
            Self::Normalization(_) => {
                "semantic: hashed query text collapsed to a zero-norm embedding"
            }
        }
    }

    fn into_core_error_v1(self) -> CoreError {
        match self {
            Self::DimensionMismatch => CoreError::InvalidContract(
                "semantic: hashing embedder output dimension mismatch".to_string(),
            ),
            Self::ZeroDimension => CoreError::InvalidContract(
                "semantic: hashing embedder dimension must be non-zero".to_string(),
            ),
            Self::DimensionConversion => CoreError::InvalidContract(
                "semantic: hashing embedder dimension conversion failed: out of range integral type conversion attempted".to_string(),
            ),
            Self::SlotConversion { slot } => CoreError::InvalidContract(format!(
                "semantic: hashed slot conversion failed for {slot}: out of range integral type conversion attempted"
            )),
            Self::SlotOutOfBounds { slot, dimension, primary } => CoreError::Storage(format!(
                "semantic: {} hashed slot {slot} out of bounds for dimension {dimension}",
                if primary { "primary" } else { "secondary" }
            )),
            Self::EmptyQuery => CoreError::Typed {
                code: LexicalErrorCode::EmptyQuery.into(),
                message: "semantic: query text must contain at least one alphanumeric token"
                    .to_string(),
            },
            Self::Normalization(_) => CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.into(),
                message: "semantic: hashed query text collapsed to a zero-norm embedding"
                    .to_string(),
            },
        }
    }
}

/// Output dimension of the search-owned hash embedder.
pub const SEARCH_OWNED_SEMANTIC_DIMENSION: usize = 64;

/// SSOT for the search-owned hash embedder's model identity.
///
/// This lives with the embedder that defines it: both the query-time
/// [`HashingQueryTextEmbedder`] (here) and the corpus derivation contract
/// (`ingest_dispatcher`/`semantic_derive`) read it so the two sides cannot
/// silently disagree on model identity; query-time enforcement rejects a
/// mismatch (`SEM_MODEL_MISMATCH`).
pub const SEARCH_OWNED_SEMANTIC_MODEL_ID: &str = "search-owned-hash-text-v1";

/// The hash embedder's revision (QI-BB-028).
///
/// It names the slot hashing and the normalization it applies, so a change
/// to either is a new revision and never shares a cache namespace or a query
/// gate with the old one.
pub const SEARCH_OWNED_SEMANTIC_MODEL_REVISION: &str = "fnv1a64-slots-l2unit-v1";

pub trait QueryTextEmbedderPort {
    /// Embed one query text under the request's budget (QI-BB-002).
    ///
    /// An embedder that reaches a provider observes the budget inside
    /// that call — the deadline caps every attempt and a cancellation
    /// abandons the attempt in flight — and answers with the typed
    /// interruption at the `semantic:embed` checkpoint; an embedder with
    /// nothing to interrupt checks the budget once and computes.
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError>;

    /// Stable identity of the model this embedder produces query vectors for.
    /// Query vectors are only comparable (cosine) against a corpus indexed by the
    /// SAME model; the query path enforces this against the indexed generation's
    /// persisted model identity and fails closed (`SEM_MODEL_MISMATCH`) on drift.
    fn model_id(&self) -> &str;

    /// The model revision, compared alongside [`Self::model_id`].
    fn model_revision(&self) -> &str;
}

pub struct HashingQueryTextEmbedder {
    dimension: usize,
}

impl HashingQueryTextEmbedder {
    #[must_use]
    pub const fn new(dimension: usize) -> Self {
        Self { dimension }
    }

    /// Upper bound on source-level visits by the shared hash calculation.
    /// A UTF-8 byte bounds a scalar or token: split (1), trim (2), filter
    /// (1), token loop (1), FNV byte pass (1), and two slot calculations plus
    /// two updates (4). Filter also visits the final empty segment once.
    /// Fill, norm validation, and normalization visit each component once.
    /// This bounds work without tokenizing or allocating before admission.
    #[must_use]
    pub fn checked_work_upper_bound_v1(&self, text: &str) -> Option<usize> {
        text.len()
            .checked_mul(10)?
            .checked_add(1)?
            .checked_add(self.dimension.checked_mul(3)?)
    }

    /// Fill a caller-owned vector without allocating another embedding backing.
    /// The caller is responsible for admitting its storage and work first.
    pub fn embed_query_into_v1(&self, text: &str, values: &mut [f32]) -> Result<(), CoreError> {
        self.embed_query_into_checked_v1(text, values)
            .map_err(HashingQueryTextFailureV1::into_core_error_v1)
    }

    /// Use the same calculation with an allocation-free typed failure.
    pub fn embed_query_into_checked_v1(
        &self,
        text: &str,
        values: &mut [f32],
    ) -> Result<(), HashingQueryTextFailureV1> {
        if values.len() != self.dimension {
            return Err(HashingQueryTextFailureV1::DimensionMismatch);
        }
        hash_query_text_into_checked(text, values)
    }
}

impl QueryTextEmbedderPort for HashingQueryTextEmbedder {
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        hash_query_text(query_text, self.dimension)
    }

    fn model_id(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    }
}

impl TextEmbeddingProvider for HashingQueryTextEmbedder {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        texts
            .iter()
            .map(|text| hash_query_text(text, self.dimension))
            .collect()
    }

    fn model_id(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn normalization(&self) -> EmbeddingNormalization {
        // `hash_query_text` normalizes through the shared policy, so this
        // embedder promises unit vectors itself (QI-BB-031).
        EmbeddingNormalization::L2Unit
    }
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private hashing helper is shared across sibling search-plane modules"
)]
pub(crate) fn hash_query_text(text: &str, dimension: usize) -> Result<Vec<f32>, CoreError> {
    let mut vector = vec![0.0_f32; dimension];
    hash_query_text_into_checked(text, &mut vector)
        .map_err(HashingQueryTextFailureV1::into_core_error_v1)?;
    Ok(vector)
}

fn hash_query_text_into_checked(
    text: &str,
    vector: &mut [f32],
) -> Result<(), HashingQueryTextFailureV1> {
    let dimension = vector.len();
    if dimension == 0 {
        return Err(HashingQueryTextFailureV1::ZeroDimension);
    }
    vector.fill(0.0);
    let dimension_u64 =
        u64::try_from(dimension).map_err(|_| HashingQueryTextFailureV1::DimensionConversion)?;
    let mut saw_token = false;
    for token in text
        .split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        saw_token = true;
        let hash = stable_fnv1a64(token.as_bytes());
        let primary_slot = hashed_slot(hash, dimension_u64)?;
        let secondary_slot = hashed_slot(hash.rotate_right(32), dimension_u64)?;
        let primary =
            vector
                .get_mut(primary_slot)
                .ok_or(HashingQueryTextFailureV1::SlotOutOfBounds {
                    slot: primary_slot,
                    dimension,
                    primary: true,
                })?;
        *primary += 1.0;
        let secondary =
            vector
                .get_mut(secondary_slot)
                .ok_or(HashingQueryTextFailureV1::SlotOutOfBounds {
                    slot: secondary_slot,
                    dimension,
                    primary: false,
                })?;
        *secondary -= 0.5;
    }
    if !saw_token {
        return Err(HashingQueryTextFailureV1::EmptyQuery);
    }
    // The same normalization every provider gets (QI-BB-031); a text whose
    // slots cancel to a zero vector is refused typed here, not turned into
    // NaNs.
    SemanticPolicy::normalize_l2_unit_checked_v1(vector)
        .map_err(HashingQueryTextFailureV1::Normalization)?;
    Ok(())
}

fn hashed_slot(hash: u64, dimension_u64: u64) -> Result<usize, HashingQueryTextFailureV1> {
    let slot_u64 = hash
        .checked_rem(dimension_u64)
        .ok_or(HashingQueryTextFailureV1::ZeroDimension)?;
    usize::try_from(slot_u64)
        .map_err(|_| HashingQueryTextFailureV1::SlotConversion { slot: slot_u64 })
}

fn stable_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod admitted_output_tests {
    use super::{HashingQueryTextEmbedder, HashingQueryTextFailureV1, QueryTextEmbedderPort};
    use quanta_index_core::RequestBudgetV1;

    #[test]
    fn caller_owned_slice_matches_ordinary_hash_gold() {
        let embedder = HashingQueryTextEmbedder::new(64);
        for input in ["alpha beta", "alpha-alpha", "x", "a b c"] {
            let ordinary = embedder
                .embed_query(input, &RequestBudgetV1::unbounded())
                .expect("ordinary hash");
            let mut admitted = [0.0_f32; 64];
            embedder
                .embed_query_into_v1(input, &mut admitted)
                .expect("caller-owned hash");
            assert_eq!(ordinary, admitted);
        }
        let mut fixed = [0.0_f32; 64];
        embedder
            .embed_query_into_v1("alpha beta", &mut fixed)
            .expect("fixed hash gold");
        for (index, actual) in fixed.iter().enumerate() {
            let expected = match index {
                27 | 59 => -0.316_227_76_f32,
                39 | 43 => 0.632_455_5_f32,
                _ => 0.0,
            };
            assert!((actual - expected).abs() < 1e-6, "slot {index}");
        }
    }

    #[test]
    fn caller_owned_slice_refuses_wrong_dimension() {
        let embedder = HashingQueryTextEmbedder::new(64);
        assert!(embedder
            .embed_query_into_v1("alpha", &mut [0.0; 63])
            .is_err());
        assert_eq!(
            embedder.embed_query_into_checked_v1("alpha", &mut [0.0; 63]),
            Err(HashingQueryTextFailureV1::DimensionMismatch),
        );
        assert_eq!(
            embedder.embed_query_into_checked_v1(" -- ", &mut [0.0; 64]),
            Err(HashingQueryTextFailureV1::EmptyQuery),
        );
    }

    #[test]
    fn canonical_work_bound_covers_text_and_vector_visits() {
        let embedder = HashingQueryTextEmbedder::new(64);
        assert_eq!(
            embedder.checked_work_upper_bound_v1("alpha beta"),
            Some(293)
        );
        assert_eq!(embedder.checked_work_upper_bound_v1(""), Some(193));
        assert_eq!(
            embedder.checked_work_upper_bound_v1("한글 α"),
            Some(10 * "한글 α".len() + 193)
        );
        assert_eq!(
            HashingQueryTextEmbedder::new(usize::MAX).checked_work_upper_bound_v1("a"),
            None
        );
    }
}

/// The provider boundary every admitted embedder sits behind (S21-08).
///
/// Order is the contract: admission (class + egress policy + common input
/// matrix), the declared-model gate, the global reservation, and only then
/// the inner embedder's I/O. A request refused at any pre-I/O step costs
/// exactly zero provider calls and reserves nothing — the owner-local suite
/// proves this with a counting spy. Cancellation and failure settle the
/// reservation on the way out, so no detached work can outlive the request
/// against the global caps. Searchd enrolls the provider attempt pool at
/// serve (`spawn_provider_child`); this type settles the request-path
/// reservation. Host-process signal/lease probes remain qualification work,
/// not a missing spawn implementation.
pub struct ProviderBoundaryQueryEmbedder {
    inner: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
    ledger: Arc<ProviderBudgetLedger>,
    policy: SemanticEgressPolicyV1,
    supervisor_id: String,
}

impl ProviderBoundaryQueryEmbedder {
    /// Compose one embedder behind the boundary. The supervisor id is the
    /// P08 supervisor this reservation is enrolled to; it must be named so
    /// reserved work always has an owner.
    #[must_use]
    pub fn new(
        inner: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
        ledger: Arc<ProviderBudgetLedger>,
        policy: SemanticEgressPolicyV1,
        supervisor_id: &str,
    ) -> Self {
        Self {
            inner,
            ledger,
            policy,
            supervisor_id: supervisor_id.to_string(),
        }
    }

    /// Embed one query text through the full admission pipeline.
    ///
    /// Returns the validated outcome alongside the vector; a provider that
    /// answers with an unexpected model, dimension or a non-finite vector
    /// produces a typed failure and neither a success nor a receipt.
    pub fn embed_query_admitted(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<(Vec<f32>, quanta_index_core::EmbeddingOutcomeV1), CoreError> {
        let _admitted = SemanticAdmissionEngine::admit_text(
            SemanticInputClass::QueryText,
            query_text,
            &self.policy,
        )?;
        // Declared-model gate: refuse a composition whose declared profile
        // model is not the inner embedder's, before any provider I/O. A
        // loopback/stub policy declares no external model, so the gate
        // applies only to `External` profiles.
        match declared_model_id(&self.policy) {
            Some(declared) if self.inner.model_id() != declared => {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::SemModelMismatch.into(),
                    message: "semantic admission: embedder model does not match the \
                              declared egress profile model"
                        .to_string(),
                });
            }
            Some(_) | None => {}
        }
        let inflight_bytes = u64::try_from(query_text.len()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "semantic admission: query text byte length conversion failed: {err}"
            ))
        })?;
        budget.checkpoint(EMBED_CHECKPOINT)?;
        let ticket = self.ledger.reserve(
            SemanticInputClass::QueryText,
            &ProviderWorkEstimateV1::loopback(inflight_bytes),
            &self.supervisor_id,
            budget.correlation(),
        )?;
        budget.record_provider_stage_v1(RequestProviderStageV1::Started {
            ticket_id: ticket.ticket_id,
        });
        // A synchronous native provider can finish after the peer cancelled.
        // Reject its late vector before a Success receipt is committed.
        let embedded = self
            .inner
            .embed_query(query_text, budget)
            .and_then(|vector| budget.checkpoint(EMBED_CHECKPOINT).map(|()| vector));
        budget.record_provider_stage_v1(RequestProviderStageV1::Returned {
            ticket_id: ticket.ticket_id,
        });
        match embedded {
            Ok(vector) => {
                let outcome = quanta_index_core::EmbeddingOutcomeV1::from_local_vector(
                    &vector,
                    self.inner.model_id(),
                    self.inner.model_revision(),
                    vector.len(),
                );
                match outcome {
                    Ok(outcome) => {
                        let usage = ProviderSettlementUsageV1 {
                            observed_cost_micros: 0,
                            observed_usage_tokens: 0,
                        };
                        let receipt: ProviderSettlementReceiptV1 = self.ledger.settle(
                            &ticket,
                            ProviderSettlementKindV1::Success,
                            usage,
                        )?;
                        self.record_audit(
                            &receipt,
                            outcome.observed_model_id.as_deref(),
                            outcome.observed_dimension,
                        )?;
                        Ok((vector, outcome))
                    }
                    Err(refusal) => {
                        let receipt: ProviderSettlementReceiptV1 = self.ledger.settle(
                            &ticket,
                            ProviderSettlementKindV1::Failed,
                            zero_usage(),
                        )?;
                        self.record_audit(&receipt, None, 0)?;
                        Err(refusal)
                    }
                }
            }
            Err(err) => {
                let wire_code = err.clone().into_search_plane_wire().0;
                let kind = if matches!(
                    wire_code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled
                        | quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded
                ) {
                    ProviderSettlementKindV1::Cancelled
                } else {
                    ProviderSettlementKindV1::Failed
                };
                let receipt: ProviderSettlementReceiptV1 =
                    self.ledger.settle(&ticket, kind, zero_usage())?;
                self.record_audit(&receipt, None, 0)?;
                Err(err)
            }
        }
    }

    /// The ledger this boundary reserves against and records audit events
    /// into: the composition root shares one per process.
    #[must_use]
    pub fn ledger(&self) -> &Arc<ProviderBudgetLedger> {
        &self.ledger
    }

    /// Record one settlement's audit identity (S21-08 step 7): the ticket,
    /// the terminal kind, observed usage and the declared/observed model
    /// pair. Redaction-safe by construction — no request text.
    fn record_audit(
        &self,
        receipt: &ProviderSettlementReceiptV1,
        observed_model_id: Option<&str>,
        observed_dimension: usize,
    ) -> Result<(), CoreError> {
        self.ledger.record_audit(ProviderAuditEventV1 {
            ticket_id: receipt.ticket_id,
            kind: receipt.kind,
            observed: receipt.observed,
            declared_model_id: self.inner.model_id().to_string(),
            observed_model_id: observed_model_id.map(str::to_string),
            observed_dimension,
            // W10-R2: the receipt carries the reservation-time
            // correlation; the audit event never re-reads the budget.
            correlation: receipt.correlation,
        })
    }
}

impl QueryTextEmbedderPort for ProviderBoundaryQueryEmbedder {
    /// The production query path (S21-08): full admission, reservation
    /// and settlement around the inner embedder, behind the port the
    /// routes already call. The audit identity lands in the ledger ring;
    /// only the vector crosses the port.
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        self.embed_query_admitted(query_text, budget)
            .map(|(vector, _outcome)| vector)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn model_revision(&self) -> &str {
        self.inner.model_revision()
    }
}

/// The zero-usage settlement for paths that never reached a provider bill.
fn zero_usage() -> ProviderSettlementUsageV1 {
    ProviderSettlementUsageV1 {
        observed_cost_micros: 0,
        observed_usage_tokens: 0,
    }
}

/// The model id an external policy declared; `None` when the policy
/// declares no external model (loopback/stub compositions).
fn declared_model_id(policy: &SemanticEgressPolicyV1) -> Option<&str> {
    match policy {
        SemanticEgressPolicyV1::External(grant) => Some(grant.model_id.as_str()),
        SemanticEgressPolicyV1::Denied | SemanticEgressPolicyV1::Loopback => None,
    }
}

/// Admit producer source content through the same boundary the query
/// path uses (S21-08).
///
/// Same engine, same class separation, and a separate explicit grant
/// without which source content is refused before any I/O.
pub fn admit_source_derive_content(
    text: &str,
    policy: &SemanticEgressPolicyV1,
) -> Result<(), CoreError> {
    SemanticAdmissionEngine::admit_text(SemanticInputClass::SourceContent, text, policy).map(|_| ())
}
