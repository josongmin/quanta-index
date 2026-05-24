//! Property test: rejection is monotone in the cardinality guard.
//!
//! Once a `(tenant_id, repo_id)` key (or a `tenant_id` key) crosses its cap,
//! every subsequent emit for that key must continue to reject with the same
//! [`ObsErrorCode::ObsCardinalityGuard`] tag. ≥ 256 cases enforced.

use std::collections::BTreeSet;

use proptest::prelude::*;

use quanta_index_lq_obs::cardinality_guard::CardinalityGuard;
use quanta_index_lq_obs::dim::Dimensions;
use quanta_index_lq_obs::errors::ObsErrorCode;

fn arb_label() -> impl Strategy<Value = String> {
    // Keep the alphabet small so we get plenty of collisions and force the
    // guard to actually advance counts.
    "[a-d]{1,4}".prop_map(String::from)
}

fn arb_dim() -> impl Strategy<Value = (String, String)> {
    (arb_label(), arb_label())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn rejection_is_monotone(seq in proptest::collection::vec(arb_dim(), 1..64)) {
        // Tight caps so we force rejections.
        let mut g = CardinalityGuard::new(2, 2);
        let mut rejected_keys: BTreeSet<(String, String)> = BTreeSet::new();
        let mut rejected_tenants: BTreeSet<String> = BTreeSet::new();
        for (t, r) in &seq {
            let d = Dimensions::new("OBS-01", "8", t.as_str(), r.as_str(), 1);
            let res = g.observe(&d);
            // Once we recorded a rejection for this exact key, it must stay
            // rejected.
            if rejected_keys.contains(&(t.clone(), r.clone())) {
                match res {
                    Ok(()) => prop_assert!(false, "monotonicity broken for ({t}, {r})"),
                    Err(e) => prop_assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard),
                }
                continue;
            }
            if rejected_tenants.contains(t) {
                match res {
                    Ok(()) => prop_assert!(false, "tenant monotonicity broken for {t}"),
                    Err(e) => prop_assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard),
                }
                continue;
            }
            match res {
                Ok(()) => {}
                Err(e) => {
                    prop_assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard);
                    let _inserted: bool = rejected_keys.insert((t.clone(), r.clone()));
                    // If tenant is the overflowing dim, mark the tenant.
                    if e.dim_overflow.as_deref() == Some("tenant_id") {
                        let _inserted: bool = rejected_tenants.insert(t.clone());
                    }
                }
            }
        }
    }
}
