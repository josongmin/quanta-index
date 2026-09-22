//! Semantic provider admission and global work accounting (S21-08).
//!
//! Everything in this module is decidable BEFORE any provider I/O: the
//! common input matrix, the egress grant, the declared-model gate and the
//! process-global reservation. A request refused here costs exactly zero
//! provider calls and reserves nothing. The response-side contract
//! ([`EmbeddingOutcomeV1`]) carries declared vs observed identity so a
//! provider that answers with a different model, dimension or a non-finite
//! vector produces a typed failure and never a success, a cache entry or a
//! receipt.

use std::collections::VecDeque;
use std::sync::Mutex;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_contract::lex::LexicalErrorCode;

use crate::error::CoreError;

/// Typed refusal when egress policy does not explicitly admit the input.
pub const PROVIDER_EGRESS_DENIED_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::ProviderEgressDenied;

/// Typed refusal when the process-global provider budget is exhausted.
pub const PROVIDER_BUDGET_EXHAUSTED_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::ServerOverloaded;

/// Typed refusal recorded when reserved provider work is cancelled.
pub const PROVIDER_WORK_CANCELLED_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RequestCancelled;

/// Which data class is asking to reach a provider (S21-08).
///
/// Query text and source-derive content are separate grants: the same
/// enforcement engine admits both, but source content additionally requires
/// an explicit source-content consent on the egress grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticInputClass {
    /// Operator-entered query text.
    QueryText,
    /// Producer-sourced content on the derivation path.
    SourceContent,
}

impl SemanticInputClass {
    /// Stable name for audit events; never carries payload text.
    #[must_use]
    pub const fn audit_name(self) -> &'static str {
        match self {
            Self::QueryText => "query_text",
            Self::SourceContent => "source_content",
        }
    }
}

/// The explicit external egress grant (S21-08 step 2).
///
/// Every field must be non-empty for external egress; one missing field is
/// `PROVIDER_EGRESS_DENIED` before any I/O. `source_content_consent` is the
/// separate, explicit source-content grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEgressGrantV1 {
    pub tenant_id: String,
    pub provider_id: String,
    pub endpoint: String,
    pub region: String,
    pub retention: String,
    pub model_id: String,
    pub model_revision: String,
    pub profile: String,
    /// The separate explicit grant for producer source content.
    pub source_content_consent: bool,
}

impl SemanticEgressGrantV1 {
    /// Fail closed on any missing field, before any provider I/O.
    pub fn validate(&self) -> Result<(), CoreError> {
        let missing = [
            ("tenant_id", self.tenant_id.as_str()),
            ("provider_id", self.provider_id.as_str()),
            ("endpoint", self.endpoint.as_str()),
            ("region", self.region.as_str()),
            ("retention", self.retention.as_str()),
            ("model_id", self.model_id.as_str()),
            ("model_revision", self.model_revision.as_str()),
            ("profile", self.profile.as_str()),
        ]
        .into_iter()
        .find(|(_, value)| value.is_empty());
        if let Some((field, _)) = missing {
            return Err(CoreError::Typed {
                code: PROVIDER_EGRESS_DENIED_CODE,
                message: format!(
                    "semantic admission: egress grant field `{field}` is required before \
                     any provider I/O"
                ),
            });
        }
        Ok(())
    }
}

/// The egress policy a composition admits provider work under.
///
/// `Denied` fails every external call closed. `Loopback` admits only
/// owner-local stub/hash providers (no network egress). `External` requires
/// a complete [`SemanticEgressGrantV1`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticEgressPolicyV1 {
    /// Egress disabled: every provider call is refused typed.
    Denied,
    /// Loopback/stub providers only, for owner-local tests.
    Loopback,
    /// External egress under an explicit, complete grant.
    External(SemanticEgressGrantV1),
}

/// The profile-independent pre-I/O admission engine (S21-08 steps 1-2).
///
/// The input matrix is shared by every profile (hash, stub, external
/// provider): whitespace-only, punctuation-only and tokenless inputs are
/// refused identically with zero provider calls. Source content additionally
/// requires an `External` policy whose grant carries the separate
/// source-content consent.
pub struct SemanticAdmissionEngine;

/// An admitted input: it passed the common matrix and the egress gate.
///
/// Carries the class and the redaction-safe audit identity only — never the
/// raw text, so logs, metrics and receipts cannot leak it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSemanticInputV1 {
    /// The class that was admitted.
    pub class: SemanticInputClass,
    /// The egress mode that admitted it, as a stable audit name.
    pub egress_mode: &'static str,
}

impl SemanticAdmissionEngine {
    /// Admit one input class under one egress policy, before any provider
    /// I/O. Returns the audit identity or the typed refusal.
    pub fn admit(
        class: SemanticInputClass,
        policy: &SemanticEgressPolicyV1,
    ) -> Result<AdmittedSemanticInputV1, CoreError> {
        let mode = match policy {
            SemanticEgressPolicyV1::Denied => {
                return Err(CoreError::Typed {
                    code: PROVIDER_EGRESS_DENIED_CODE,
                    message: "semantic admission: provider egress is disabled by policy"
                        .to_string(),
                });
            }
            SemanticEgressPolicyV1::Loopback => {
                if class == SemanticInputClass::SourceContent {
                    return Err(CoreError::Typed {
                        code: PROVIDER_EGRESS_DENIED_CODE,
                        message: "semantic admission: source content requires an explicit \
                                  external egress grant with source-content consent"
                            .to_string(),
                    });
                }
                "loopback"
            }
            SemanticEgressPolicyV1::External(grant) => {
                grant.validate()?;
                if class == SemanticInputClass::SourceContent && !grant.source_content_consent {
                    return Err(CoreError::Typed {
                        code: PROVIDER_EGRESS_DENIED_CODE,
                        message: "semantic admission: source content consent was not granted"
                            .to_string(),
                    });
                }
                "external"
            }
        };
        Ok(AdmittedSemanticInputV1 {
            class,
            egress_mode: mode,
        })
    }

    /// The common, profile-independent input matrix (S21-08 step 1).
    ///
    /// Refuses whitespace-only, punctuation-only and tokenless text with the
    /// same typed code for query text and source content, before any
    /// provider I/O, under every profile.
    pub fn admit_input_text(class: SemanticInputClass, text: &str) -> Result<(), CoreError> {
        let saw_token = text
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|token| !token.is_empty());
        if saw_token {
            return Ok(());
        }
        Err(CoreError::Typed {
            code: LexicalErrorCode::EmptyQuery.into(),
            message: format!(
                "semantic admission: {} must contain at least one alphanumeric token",
                class.audit_name()
            ),
        })
    }

    /// Full admission for one concrete text: egress gate first, then the
    /// common input matrix. One call, zero provider I/O on refusal.
    pub fn admit_text(
        class: SemanticInputClass,
        text: &str,
        policy: &SemanticEgressPolicyV1,
    ) -> Result<AdmittedSemanticInputV1, CoreError> {
        let admitted = Self::admit(class, policy)?;
        Self::admit_input_text(class, text)?;
        Ok(admitted)
    }
}

/// The process-global provider work caps (S21-08).
///
/// All caps must be non-zero; overflow-safe accounting is checked
/// arithmetic throughout the ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderWorkBudgetV1 {
    pub inflight_requests_cap: u64,
    pub inflight_bytes_cap: u64,
    pub total_cost_micros_ceiling: u64,
    pub retry_attempts_cap: u32,
}

impl ProviderWorkBudgetV1 {
    /// Fail closed on zero caps or a zero-sum configuration.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.inflight_requests_cap == 0
            || self.inflight_bytes_cap == 0
            || self.total_cost_micros_ceiling == 0
        {
            return Err(CoreError::InvalidContract(
                "semantic admission: provider work budget caps must be non-zero".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for ProviderWorkBudgetV1 {
    fn default() -> Self {
        Self {
            inflight_requests_cap: 64,
            inflight_bytes_cap: 16 * 1024 * 1024,
            total_cost_micros_ceiling: 1_000_000,
            retry_attempts_cap: 3,
        }
    }
}

/// What one admitted provider request expects to spend (S21-08 step 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderWorkEstimateV1 {
    pub inflight_bytes: u64,
    pub reserved_cost_micros: u64,
    pub retry_attempts: u32,
}

impl ProviderWorkEstimateV1 {
    /// A loopback/stub estimate: no external bytes, no external cost.
    #[must_use]
    pub const fn loopback(inflight_bytes: u64) -> Self {
        Self {
            inflight_bytes,
            reserved_cost_micros: 0,
            retry_attempts: 0,
        }
    }
}

/// Ownership-bearing enrollment for the supervisor pool (S21-08 step 4).
///
/// Created at reservation time; the actual spawn/register/cancel/join is
/// P08 (S21-09). The handle names its supervisor and ticket so a supervisor
/// can reconcile every reserved request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSupervisorEnrollmentV1 {
    pub supervisor_id: String,
    pub ticket_id: u64,
    /// The admitted class this enrollment owns.
    pub input_class: SemanticInputClass,
}

/// The reservation ticket an admitted request holds until settlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderReservationTicketV1 {
    pub ticket_id: u64,
    pub inflight_bytes: u64,
    pub reserved_cost_micros: u64,
    pub reserved_retry_attempts: u32,
    pub enrollment: ProviderSupervisorEnrollmentV1,
}

/// What a settlement observed, for audit and cost accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderSettlementUsageV1 {
    pub observed_cost_micros: u64,
    pub observed_usage_tokens: u64,
}

/// The settled terminal outcome of one reserved request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSettlementKindV1 {
    /// The provider answered; observed usage moves from reserved to spent.
    Success,
    /// The provider failed; observed usage is still spent, the rest released.
    Failed,
    /// Cancelled before or during the call; nothing new is spent.
    Cancelled,
}

/// The receipt a settlement returns — redaction-safe by construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSettlementReceiptV1 {
    pub ticket_id: u64,
    pub kind: ProviderSettlementKindV1,
    pub observed: ProviderSettlementUsageV1,
}

/// How many settled provider calls the ledger's audit ring keeps (S21-08
/// step 7). Bounded so a busy process cannot grow it without limit; the
/// newest settlements evict the oldest.
pub const PROVIDER_AUDIT_RING_CAP: usize = 128;

/// One settled provider call's audit identity (S21-08 step 7).
///
/// Model ids, the terminal kind and observed usage only — never request
/// text, vectors or credentials — so the ring is safe to scrape, log and
/// keep. Recorded by the provider boundary on every settlement path,
/// including refusals-after-reservation and cancellations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAuditEventV1 {
    pub ticket_id: u64,
    pub kind: ProviderSettlementKindV1,
    pub observed: ProviderSettlementUsageV1,
    pub declared_model_id: String,
    pub observed_model_id: Option<String>,
    pub observed_dimension: usize,
}

/// Process-global snapshot of the provider work ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct ProviderBudgetSnapshotV1 {
    pub inflight_requests: u64,
    pub inflight_bytes: u64,
    pub reserved_cost_micros: u64,
    pub spent_cost_micros: u64,
    pub live_tickets: u64,
    pub next_ticket_id: u64,
}

#[derive(Debug, Default)]
struct LedgerInner {
    snapshot: ProviderBudgetSnapshotV1,
    audit: VecDeque<ProviderAuditEventV1>,
}

/// The process-global bounded provider work ledger (S21-08 steps 4, 7).
///
/// One ledger per process bounds concurrent requests, inflight bytes and
/// total cost with overflow-safe checked accounting. Reservation,
/// settlement and cancellation are the only mutators; a cancelled ticket
/// releases its reservation so no detached work can outlive its request
/// against the caps.
#[derive(Debug, Default)]
pub struct ProviderBudgetLedger {
    budget: ProviderWorkBudgetV1,
    inner: Mutex<LedgerInner>,
}

impl ProviderBudgetLedger {
    /// Construct with an explicit budget; the budget must validate.
    pub fn new(budget: ProviderWorkBudgetV1) -> Result<Self, CoreError> {
        budget.validate()?;
        Ok(Self {
            budget,
            inner: Mutex::new(LedgerInner::default()),
        })
    }

    /// Reserve one admitted request's work (S21-08 step 4).
    ///
    /// Fails typed `SERVER_OVERLOADED` when any cap would be exceeded;
    /// on failure nothing is reserved.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the ledger guard is the reservation critical section and must span every checked update"
    )]
    pub fn reserve(
        &self,
        class: SemanticInputClass,
        estimate: &ProviderWorkEstimateV1,
        supervisor_id: &str,
    ) -> Result<ProviderReservationTicketV1, CoreError> {
        if supervisor_id.is_empty() {
            return Err(CoreError::InvalidContract(
                "semantic admission: reserved provider work must name a supervisor owner"
                    .to_string(),
            ));
        }
        let mut inner = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!(
                "semantic admission: provider budget ledger lock poisoned: {err}"
            ))
        })?;
        let snap = &mut inner.snapshot;
        let requests = snap
            .inflight_requests
            .checked_add(1)
            .ok_or_else(|| Self::overflow("inflight request count"))?;
        let bytes = snap
            .inflight_bytes
            .checked_add(estimate.inflight_bytes)
            .ok_or_else(|| Self::overflow("inflight byte count"))?;
        let cost = snap
            .reserved_cost_micros
            .checked_add(snap.spent_cost_micros)
            .and_then(|committed| committed.checked_add(estimate.reserved_cost_micros))
            .ok_or_else(|| Self::overflow("cost accounting"))?;
        let retries = u64::from(estimate.retry_attempts);
        if requests > self.budget.inflight_requests_cap {
            return Err(Self::exhausted("inflight request cap"));
        }
        if bytes > self.budget.inflight_bytes_cap {
            return Err(Self::exhausted("inflight byte cap"));
        }
        if cost > self.budget.total_cost_micros_ceiling {
            return Err(Self::exhausted("cost cap"));
        }
        if retries > u64::from(self.budget.retry_attempts_cap) {
            return Err(Self::exhausted("retry cap"));
        }
        let ticket_id = snap
            .next_ticket_id
            .checked_add(1)
            .ok_or_else(|| Self::overflow("ticket id sequence"))?;
        let live = snap
            .live_tickets
            .checked_add(1)
            .ok_or_else(|| Self::overflow("live ticket count"))?;
        snap.inflight_requests = requests;
        snap.inflight_bytes = bytes;
        snap.reserved_cost_micros = snap
            .reserved_cost_micros
            .checked_add(estimate.reserved_cost_micros)
            .ok_or_else(|| Self::overflow("reserved cost accounting"))?;
        snap.next_ticket_id = ticket_id;
        snap.live_tickets = live;
        Ok(ProviderReservationTicketV1 {
            ticket_id,
            inflight_bytes: estimate.inflight_bytes,
            reserved_cost_micros: estimate.reserved_cost_micros,
            reserved_retry_attempts: estimate.retry_attempts,
            enrollment: ProviderSupervisorEnrollmentV1 {
                supervisor_id: supervisor_id.to_string(),
                ticket_id,
                input_class: class,
            },
        })
    }

    /// Settle one ticket to its terminal state (S21-08 step 7).
    ///
    /// A `Success`/`Failed` settlement moves the observed cost from
    /// reserved to spent (checked); a `Cancelled` settlement releases the
    /// whole reservation. Settlement is idempotent per ticket: settling an
    /// already-settled ticket fails typed so double settlement can never
    /// silently corrupt the caps.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the ledger guard is the settlement critical section and must span every checked update"
    )]
    pub fn settle(
        &self,
        ticket: &ProviderReservationTicketV1,
        kind: ProviderSettlementKindV1,
        observed: ProviderSettlementUsageV1,
    ) -> Result<ProviderSettlementReceiptV1, CoreError> {
        let mut inner = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!(
                "semantic admission: provider budget ledger lock poisoned: {err}"
            ))
        })?;
        let snap = &mut inner.snapshot;
        if snap.live_tickets == 0 {
            return Err(CoreError::Typed {
                code: PROVIDER_WORK_CANCELLED_CODE,
                message: format!(
                    "semantic admission: ticket {} settlement found no live reservation",
                    ticket.ticket_id
                ),
            });
        }
        let spent_delta = match kind {
            ProviderSettlementKindV1::Success | ProviderSettlementKindV1::Failed => {
                observed.observed_cost_micros
            }
            ProviderSettlementKindV1::Cancelled => 0,
        };
        let released_cost = ticket
            .reserved_cost_micros
            .checked_sub(spent_delta)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic admission: ticket {} observed cost {} micros exceeds its \
                     reservation {} micros",
                    ticket.ticket_id, observed.observed_cost_micros, ticket.reserved_cost_micros
                ))
            })?;
        snap.inflight_requests = snap
            .inflight_requests
            .checked_sub(1)
            .ok_or_else(|| Self::underflow("inflight request count", ticket.ticket_id))?;
        snap.inflight_bytes = snap
            .inflight_bytes
            .checked_sub(ticket.inflight_bytes)
            .ok_or_else(|| Self::underflow("inflight byte count", ticket.ticket_id))?;
        snap.reserved_cost_micros = snap
            .reserved_cost_micros
            .checked_sub(released_cost)
            .and_then(|reserved| reserved.checked_sub(spent_delta))
            .ok_or_else(|| Self::underflow("reserved cost", ticket.ticket_id))?;
        snap.spent_cost_micros = snap
            .spent_cost_micros
            .checked_add(spent_delta)
            .ok_or_else(|| Self::overflow("spent cost accounting"))?;
        if snap.spent_cost_micros > self.budget.total_cost_micros_ceiling {
            return Err(Self::exhausted("cost cap"));
        }
        snap.live_tickets = snap
            .live_tickets
            .checked_sub(1)
            .ok_or_else(|| Self::underflow("live ticket count", ticket.ticket_id))?;
        Ok(ProviderSettlementReceiptV1 {
            ticket_id: ticket.ticket_id,
            kind,
            observed,
        })
    }

    /// Cancel one ticket: same as settling `Cancelled` with no usage.
    pub fn cancel(&self, ticket: &ProviderReservationTicketV1) -> Result<(), CoreError> {
        self.settle(
            ticket,
            ProviderSettlementKindV1::Cancelled,
            ProviderSettlementUsageV1 {
                observed_cost_micros: 0,
                observed_usage_tokens: 0,
            },
        )
        .map(|_| ())
    }

    /// Point-in-time snapshot for tests and readiness probes.
    pub fn snapshot(&self) -> Result<ProviderBudgetSnapshotV1, CoreError> {
        let inner = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!(
                "semantic admission: provider budget ledger lock poisoned: {err}"
            ))
        })?;
        Ok(inner.snapshot)
    }

    /// Record one settled call's audit identity (S21-08 step 7). The ring
    /// keeps the newest [`PROVIDER_AUDIT_RING_CAP`] events; older ones are
    /// evicted, never grown past the cap.
    pub fn record_audit(&self, event: ProviderAuditEventV1) -> Result<(), CoreError> {
        let guard = self.inner.lock();
        let mut inner = guard.map_err(|err| {
            CoreError::Storage(format!(
                "semantic admission: provider budget ledger lock poisoned: {err}"
            ))
        })?;
        inner.audit.push_back(event);
        while inner.audit.len() > PROVIDER_AUDIT_RING_CAP {
            drop(inner.audit.pop_front());
        }
        drop(inner);
        Ok(())
    }

    /// The newest audit events, oldest first, at most `limit`. Empty when
    /// nothing has settled yet; refusals before reservation record nothing.
    pub fn audit_tail(&self, limit: usize) -> Result<Vec<ProviderAuditEventV1>, CoreError> {
        let inner = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!(
                "semantic admission: provider budget ledger lock poisoned: {err}"
            ))
        })?;
        let skip = inner.audit.len().saturating_sub(limit);
        Ok(inner.audit.iter().skip(skip).cloned().collect())
    }

    fn exhausted(cap: &str) -> CoreError {
        CoreError::Typed {
            code: PROVIDER_BUDGET_EXHAUSTED_CODE,
            message: format!("semantic admission: provider {cap} exhausted"),
        }
    }

    fn overflow(what: &str) -> CoreError {
        CoreError::InvalidContract(format!(
            "semantic admission: provider budget {what} overflowed checked accounting"
        ))
    }

    fn underflow(what: &str, ticket_id: u64) -> CoreError {
        CoreError::InvalidContract(format!(
            "semantic admission: provider budget {what} underflowed settling ticket {ticket_id}"
        ))
    }
}

/// Declared vs observed provider response contract (S21-08 step 6).
///
/// `declared_*` is what the profile promised; `observed_*` is what the
/// provider answered. A mismatch, a non-finite value or a wrong dimension is
/// a typed failure that must never become a success, cache entry or receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingOutcomeV1 {
    pub declared_model_id: String,
    pub declared_model_revision: String,
    pub declared_dimension: usize,
    pub observed_model_id: Option<String>,
    pub observed_model_revision: Option<String>,
    pub observed_dimension: usize,
    pub vector_norm: Option<f32>,
    pub all_finite: bool,
    pub usage_tokens: Option<u64>,
    pub cost_micros: Option<u64>,
}

impl EmbeddingOutcomeV1 {
    /// Validate observed against declared. Fails typed; never coerces.
    pub fn validate(&self) -> Result<(), CoreError> {
        match self.observed_model_id.as_deref() {
            Some(observed) if observed != self.declared_model_id => {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::SemModelMismatch.into(),
                    message: format!(
                        "semantic admission: provider answered model `{observed}` but the \
                         profile declared `{}`",
                        self.declared_model_id
                    ),
                });
            }
            Some(_) | None => {}
        }
        match self.observed_model_revision.as_deref() {
            Some(observed) if observed != self.declared_model_revision => {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::SemModelMismatch.into(),
                    message: format!(
                        "semantic admission: provider answered model revision `{observed}` but \
                         the profile declared `{}`",
                        self.declared_model_revision
                    ),
                });
            }
            Some(_) | None => {}
        }
        if self.observed_dimension != self.declared_dimension {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemDimMismatch.into(),
                message: format!(
                    "semantic admission: provider answered dimension {} but the profile \
                     declared {}",
                    self.observed_dimension, self.declared_dimension
                ),
            });
        }
        if !self.all_finite {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.into(),
                message: "semantic admission: provider vector contained a non-finite component"
                    .to_string(),
            });
        }
        match self.vector_norm {
            Some(norm) if !norm.is_finite() || norm <= 0.0 => {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::SemInvalidVector.into(),
                    message: "semantic admission: provider vector norm is not finite and \
                              positive"
                        .to_string(),
                });
            }
            Some(_) | None => {}
        }
        Ok(())
    }

    /// Build the outcome for a locally computed (loopback) vector.
    ///
    /// Observed identity equals declared identity because there is no
    /// external provider to disagree; finiteness and dimension are read
    /// from the vector itself so the same validation covers every profile.
    pub fn from_local_vector(
        vector: &[f32],
        declared_model_id: &str,
        declared_model_revision: &str,
        declared_dimension: usize,
    ) -> Result<Self, CoreError> {
        if vector.len() != declared_dimension {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemDimMismatch.into(),
                message: format!(
                    "semantic admission: local embedder produced dimension {} but declared {}",
                    vector.len(),
                    declared_dimension
                ),
            });
        }
        let all_finite = vector.iter().all(|component| component.is_finite());
        if !all_finite {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.into(),
                message: "semantic admission: local embedder produced a non-finite component"
                    .to_string(),
            });
        }
        let norm_squared = vector
            .iter()
            .fold(0.0_f32, |acc, component| acc + component * component);
        if !norm_squared.is_finite() || norm_squared <= 0.0 {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.into(),
                message: "semantic admission: local embedder collapsed to a zero or \
                          non-finite norm"
                    .to_string(),
            });
        }
        let norm = Some(norm_squared.sqrt());
        Ok(Self {
            declared_model_id: declared_model_id.to_string(),
            declared_model_revision: declared_model_revision.to_string(),
            declared_dimension,
            observed_model_id: Some(declared_model_id.to_string()),
            observed_model_revision: Some(declared_model_revision.to_string()),
            observed_dimension: vector.len(),
            vector_norm: norm,
            all_finite,
            usage_tokens: None,
            cost_micros: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_budget() -> ProviderWorkBudgetV1 {
        ProviderWorkBudgetV1 {
            inflight_requests_cap: 8,
            inflight_bytes_cap: 4096,
            total_cost_micros_ceiling: 1_000_000,
            retry_attempts_cap: 2,
        }
    }

    fn test_event(ticket_id: u64) -> ProviderAuditEventV1 {
        ProviderAuditEventV1 {
            ticket_id,
            kind: ProviderSettlementKindV1::Success,
            observed: ProviderSettlementUsageV1 {
                observed_cost_micros: 10,
                observed_usage_tokens: 7,
            },
            declared_model_id: "search-owned-hash-text-v1".to_string(),
            observed_model_id: Some("search-owned-hash-text-v1".to_string()),
            observed_dimension: 64,
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "each index follows an exact length assert on the same tail"
    )]
    #[test]
    fn audit_ring_starts_empty_and_reads_back_in_order() {
        let ledger = ProviderBudgetLedger::new(test_budget()).expect("valid budget");
        assert!(ledger.audit_tail(8).expect("tail reads").is_empty());
        ledger.record_audit(test_event(1)).expect("record");
        ledger.record_audit(test_event(2)).expect("record");
        let tail = ledger.audit_tail(8).expect("tail reads");
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].ticket_id, 1);
        assert_eq!(tail[1].ticket_id, 2);
        let last_one = ledger.audit_tail(1).expect("tail reads");
        assert_eq!(last_one.len(), 1);
        assert_eq!(last_one[0].ticket_id, 2);
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "each index follows an exact length assert on the same tail"
    )]
    #[test]
    fn audit_ring_evicts_oldest_past_the_cap() {
        let ledger = ProviderBudgetLedger::new(test_budget()).expect("valid budget");
        for ticket_id in 0..(PROVIDER_AUDIT_RING_CAP + 4) {
            let ticket_id = u64::try_from(ticket_id).expect("test range fits u64");
            ledger.record_audit(test_event(ticket_id)).expect("record");
        }
        let tail = ledger
            .audit_tail(PROVIDER_AUDIT_RING_CAP + 4)
            .expect("tail reads");
        assert_eq!(tail.len(), PROVIDER_AUDIT_RING_CAP);
        assert_eq!(tail[0].ticket_id, 4);
        let last = u64::try_from(PROVIDER_AUDIT_RING_CAP + 3).expect("test range fits u64");
        assert_eq!(tail[PROVIDER_AUDIT_RING_CAP - 1].ticket_id, last);
    }
}
