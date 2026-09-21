//! Layered cardinality guard per OBS-01 § 4.4.
//!
//! # 4-layer defense
//!
//! - **Layer A — closed label set.** Adding a new label requires an
//!   `lq_version` minor bump per RFC § Migration and Versioning Policy
//!   and a coordinated edit to [`crate::dim::Dimensions`]. Enforced
//!   structurally by the type — no runtime cost.
//! - **Layer B — per-dimension cardinality cap.** Each emit is recorded
//!   here; counts beyond the cap surface
//!   [`crate::errors::ObsErrorCode::ObsCardinalityGuard`] with the
//!   overflowing dimension name carried on
//!   [`crate::errors::ObsError::dim_overflow`].
//! - **Layer C — emit-time fallback bucket.** Over-cap dimensions are
//!   relabelled to the bucket label `__OBS_OVERFLOW__` (see
//!   [`OBS_OVERFLOW_LABEL`]) and the typed
//!   [`crate::errors::ObsErrorCode::ObsCardinalityGuard`] event is
//!   emitted so the drop is auditable. Never a silent drop.
//! - **Layer D — alerting.** The overflow counter (operated on by the
//!   integration layer, not this crate) feeds the `lq_otel_dropped_dim_total`
//!   metric whose P2/P1 alert thresholds live in the dashboard ticket.
//!
//! The guard is **fail-closed**: once a key crosses its cap it stays
//! rejected for the lifetime of this [`CardinalityGuard`]. The
//! property test in `tests/property_cardinality_guard.rs` proves
//! rejection is monotone.

use std::collections::BTreeMap;

use crate::dim::{Dimensions, MAX_DISTINCT_REPOS_PER_TENANT, MAX_DISTINCT_TENANTS_GLOBAL};
use crate::errors::ObsError;

/// Bucket label used at Layer C when a dimension overflows its cap. See the
/// module docs for the 4-layer defense write-up.
pub const OBS_OVERFLOW_LABEL: &str = "__OBS_OVERFLOW__";

/// Stateful per-dimension cardinality guard.
///
/// Tracks distinct `tenant_id` values globally and distinct `repo_id` values
/// per `tenant_id`. Construct via [`CardinalityGuard::new`] with the spec
/// defaults from [`crate::dim`].
#[derive(Clone, Debug)]
pub struct CardinalityGuard {
    tenant_count: BTreeMap<Box<str>, u32>,
    repo_per_tenant: BTreeMap<(Box<str>, Box<str>), u32>,
    /// Inclusive cap on distinct `tenant_id` values.
    max_tenants: u32,
    /// Inclusive cap on distinct `repo_id` values per `tenant_id`.
    max_repos_per_tenant: u32,
}

impl Default for CardinalityGuard {
    fn default() -> Self {
        Self::new(MAX_DISTINCT_TENANTS_GLOBAL, MAX_DISTINCT_REPOS_PER_TENANT)
    }
}

impl CardinalityGuard {
    /// New guard with explicit caps. Callers that want the spec defaults
    /// should use [`CardinalityGuard::default`].
    #[must_use]
    pub fn new(max_tenants: u32, max_repos_per_tenant: u32) -> Self {
        Self {
            tenant_count: BTreeMap::new(),
            repo_per_tenant: BTreeMap::new(),
            max_tenants,
            max_repos_per_tenant,
        }
    }

    /// Observe a [`Dimensions`] payload. On the first emit for a previously
    /// unseen `tenant_id` / `repo_id`, the per-dimension counter is bumped.
    /// On any subsequent emit the call is a no-op increment (still O(log n)).
    ///
    /// Returns
    /// [`crate::errors::ObsErrorCode::ObsCardinalityGuard`] with
    /// `dim_overflow = Some("tenant_id")` when the global tenant cap is
    /// exceeded, or `Some("repo_id")` when the per-tenant repo cap is
    /// exceeded. Once a key is rejected it stays rejected — there is no
    /// retry path.
    pub fn observe(&mut self, d: &Dimensions) -> Result<(), ObsError> {
        // Per-dimension cardinality cap for tenant_id.
        if !self.tenant_count.contains_key(d.tenant_id.as_ref()) {
            let next_count = u32::try_from(self.tenant_count.len()).map_err(|err| {
                ObsError::cardinality("tenant_id", format!("tenant_count overflow u32: {err}"))
            })?;
            if next_count >= self.max_tenants {
                return Err(ObsError::cardinality(
                    "tenant_id",
                    format!("distinct tenant_id values would exceed cap {}", self.max_tenants),
                ));
            }
            let _prev: Option<u32> = self.tenant_count.insert(d.tenant_id.clone(), 0);
        }
        if let Some(slot) = self.tenant_count.get_mut(d.tenant_id.as_ref()) {
            *slot = slot.saturating_add(1);
        }

        // Per-tenant repo_id cap.
        let key = (d.tenant_id.clone(), d.repo_id.clone());
        if !self.repo_per_tenant.contains_key(&key) {
            let tenant_key = d.tenant_id.clone();
            let existing_for_tenant: u32 = u32::try_from(
                self.repo_per_tenant
                    .keys()
                    .filter(|(t, _)| *t == tenant_key)
                    .count(),
            )
            .map_err(|err| {
                ObsError::cardinality("repo_id", format!("repo_per_tenant overflow u32: {err}"))
            })?;
            if existing_for_tenant >= self.max_repos_per_tenant {
                return Err(ObsError::cardinality(
                    "repo_id",
                    format!(
                        "distinct repo_id values per tenant would exceed cap {}",
                        self.max_repos_per_tenant
                    ),
                ));
            }
            let _prev: Option<u32> = self.repo_per_tenant.insert(key.clone(), 0);
        }
        if let Some(slot) = self.repo_per_tenant.get_mut(&key) {
            *slot = slot.saturating_add(1);
        }
        Ok(())
    }

    /// Number of distinct `tenant_id` values observed so far.
    #[must_use]
    pub fn distinct_tenants(&self) -> usize {
        self.tenant_count.len()
    }

    /// Number of distinct `(tenant_id, repo_id)` pairs observed so far.
    #[must_use]
    pub fn distinct_repos(&self) -> usize {
        self.repo_per_tenant.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{CardinalityGuard, OBS_OVERFLOW_LABEL};
    use crate::dim::Dimensions;
    use crate::errors::ObsErrorCode;

    fn d(tenant: &str, repo: &str) -> Dimensions {
        Dimensions::new("OBS-01", "8", tenant, repo, 1)
    }

    #[test]
    fn overflow_label_matches_spec() {
        assert_eq!(OBS_OVERFLOW_LABEL, "__OBS_OVERFLOW__");
    }

    #[test]
    fn under_cap_accepted() {
        let mut g = CardinalityGuard::new(2, 2);
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match g.observe(&d("t1", "r2")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match g.observe(&d("t2", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        // Same tenant + same repo => no new distinct keys.
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        assert_eq!(g.distinct_tenants(), 2);
        assert_eq!(g.distinct_repos(), 3);
    }

    #[test]
    fn over_tenant_cap_rejects_with_tag() {
        let mut g = CardinalityGuard::new(1, 100);
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match g.observe(&d("t2", "r1")) {
            Ok(()) => assert!(false, "must reject"),
            Err(e) => {
                assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard);
                assert_eq!(e.dim_overflow.as_deref(), Some("tenant_id"));
            }
        }
    }

    #[test]
    fn over_repo_cap_rejects_with_tag() {
        let mut g = CardinalityGuard::new(10, 1);
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match g.observe(&d("t1", "r2")) {
            Ok(()) => assert!(false, "must reject"),
            Err(e) => {
                assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard);
                assert_eq!(e.dim_overflow.as_deref(), Some("repo_id"));
            }
        }
    }

    #[test]
    fn rejection_is_monotone_for_a_key() {
        let mut g = CardinalityGuard::new(1, 1);
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        // (t1, r2) over-caps repo for tenant t1.
        for _ in 0..5 {
            match g.observe(&d("t1", "r2")) {
                Ok(()) => assert!(false, "must reject"),
                Err(e) => assert_eq!(e.dim_overflow.as_deref(), Some("repo_id")),
            }
        }
    }

    #[test]
    fn second_tenant_under_global_cap_keeps_per_tenant_independent() {
        let mut g = CardinalityGuard::new(2, 1);
        match g.observe(&d("t1", "r1")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        // t2 fresh: should accept under per-tenant cap = 1.
        match g.observe(&d("t2", "rA")) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        // t2 second repo blows the per-tenant cap.
        match g.observe(&d("t2", "rB")) {
            Ok(()) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.dim_overflow.as_deref(), Some("repo_id")),
        }
    }
}
