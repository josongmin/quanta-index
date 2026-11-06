//! Observation of the original renewal predicates; no clock, query or authority.

use quanta_index_core::{DiagnosticOwnedRenewalRefusalReasonV1, DiagnosticOwnedRenewalRefusalV1};

use super::{IndexError, SqliteCatalog};

pub(super) fn fence_lost(
    observer: Option<&SqliteCatalog>,
    reason: DiagnosticOwnedRenewalRefusalReasonV1,
    now_ms: u64,
    lease_until_ms: Option<u64>,
    deadline_ms: Option<u64>,
) -> IndexError {
    if let Some(catalog) = observer {
        catalog.observe_owned_renewal_refusal(DiagnosticOwnedRenewalRefusalV1 {
            reason,
            now_ms,
            lease_until_ms,
            deadline_ms,
        });
    }
    IndexError::WorkFenceLost
}
