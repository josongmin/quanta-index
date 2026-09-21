//! SLO targets per OBS-01 § 9.2 (RFC-GAP-4 reconciliation).
//!
//! [`SloTarget`] is the typed `(p50, p95, p99)` triple. Per-wave defaults are
//! exposed as `pub const`. [`SloViolation`] is the typed event emitted when
//! observed p99 exceeds the locked target.
//!
//! D18 — hand-rolled serde, no proc-macro derives.

use core::fmt;

/// Locked SLO target. All values are in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[expect(
    clippy::struct_field_names,
    reason = "field names p50_ms/p95_ms/p99_ms are spec-locked by OBS-01 § 9.2"
)]
pub struct SloTarget {
    /// p50 latency target.
    pub p50_ms: u32,
    /// p95 latency target.
    pub p95_ms: u32,
    /// p99 latency target.
    pub p99_ms: u32,
}

impl SloTarget {
    /// Build from explicit percentile targets.
    #[must_use]
    pub const fn new(p50_ms: u32, p95_ms: u32, p99_ms: u32) -> Self {
        Self {
            p50_ms,
            p95_ms,
            p99_ms,
        }
    }

    /// `true` when `observed_p99_ms` exceeds the locked p99 target.
    #[must_use]
    pub const fn violates_p99(&self, observed_p99_ms: u32) -> bool {
        observed_p99_ms > self.p99_ms
    }
}

/// Single-repo lexical query — RFC § Latency SLOs.
pub const LEX_SINGLE_REPO_SLO: SloTarget = SloTarget::new(50, 250, 1_000);

/// 100-repo fanout query — RFC § Latency SLOs (only p95 is RFC-fixed; the
/// p50 and p99 numbers are the OBS-01 § 9.2 reconciled targets).
pub const LEX_100_REPO_FANOUT_SLO: SloTarget = SloTarget::new(500, 2_000, 5_000);

/// Symbol query (single-repo) — OBS-01 § 9.2 RFC-GAP-4.
pub const LEX_SYMBOL_SINGLE_REPO_SLO: SloTarget = SloTarget::new(30, 150, 500);

/// History query (`type:commit` / `type:diff`, single-repo) — OBS-01 § 9.2.
pub const HISTORY_SINGLE_REPO_SLO: SloTarget = SloTarget::new(100, 500, 2_000);

/// Structural query (single-language, single-repo) — OBS-01 § 9.2.
pub const STRUCTURAL_SINGLE_REPO_SLO: SloTarget = SloTarget::new(200, 1_000, 3_000);

/// Runtime metadata catalog-only query — OBS-01 § 9.2.
pub const RUNTIME_CATALOG_SLO: SloTarget = SloTarget::new(10, 50, 250);

/// Bridge translate (Sourcegraph syntax) — BRIDGE-01 § 9. Microsecond
/// targets are encoded as `≤ 1` millisecond at the [`SloTarget`] grain.
pub const BRIDGE_TRANSLATE_SLO: SloTarget = SloTarget::new(1, 1, 1);

/// Bridge end-to-end route (mock sink) — OBS-01 § 9.2.
pub const BRIDGE_ROUTE_SLO: SloTarget = SloTarget::new(5, 50, 500);

/// Semantic ANN top-k (single-tenant) — OBS-01 § 9.2.
pub const SEMANTIC_ANN_SLO: SloTarget = SloTarget::new(20, 100, 500);

/// Hybrid lexical+semantic merge — OBS-01 § 9.2.
pub const HYBRID_MERGE_SLO: SloTarget = SloTarget::new(60, 300, 1_200);

impl serde::Serialize for SloTarget {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("SloTarget", 3)?;
        st.serialize_field("p50_ms", &self.p50_ms)?;
        st.serialize_field("p95_ms", &self.p95_ms)?;
        st.serialize_field("p99_ms", &self.p99_ms)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for SloTarget {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            P50,
            P95,
            P99,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("SloTarget field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "p50_ms" => Ok(Field::P50),
                            "p95_ms" => Ok(Field::P95),
                            "p99_ms" => Ok(Field::P99),
                            other => Err(E::unknown_field(other, &["p50_ms", "p95_ms", "p99_ms"])),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct SV;
        impl<'d> serde::de::Visitor<'d> for SV {
            type Value = SloTarget;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SloTarget struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<SloTarget, A::Error> {
                let mut p50: Option<u32> = None;
                let mut p95: Option<u32> = None;
                let mut p99: Option<u32> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::P50 => {
                            if p50.is_some() {
                                return Err(serde::de::Error::duplicate_field("p50_ms"));
                            }
                            p50 = Some(map.next_value()?);
                        }
                        Field::P95 => {
                            if p95.is_some() {
                                return Err(serde::de::Error::duplicate_field("p95_ms"));
                            }
                            p95 = Some(map.next_value()?);
                        }
                        Field::P99 => {
                            if p99.is_some() {
                                return Err(serde::de::Error::duplicate_field("p99_ms"));
                            }
                            p99 = Some(map.next_value()?);
                        }
                    }
                }
                Ok(SloTarget {
                    p50_ms: p50.ok_or_else(|| serde::de::Error::missing_field("p50_ms"))?,
                    p95_ms: p95.ok_or_else(|| serde::de::Error::missing_field("p95_ms"))?,
                    p99_ms: p99.ok_or_else(|| serde::de::Error::missing_field("p99_ms"))?,
                })
            }
        }

        de.deserialize_struct("SloTarget", &["p50_ms", "p95_ms", "p99_ms"], SV)
    }
}

/// Typed SLO-violation event. Emitted when measured p99 exceeds the locked
/// target. Wire form is `{ observed_p99_ms, target_p99_ms }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SloViolation {
    /// Observed p99 latency in the measurement window.
    pub observed_p99_ms: u32,
    /// Locked p99 target.
    pub target_p99_ms: u32,
}

impl SloViolation {
    /// Build a violation event from observed and target latencies.
    #[must_use]
    pub const fn new(observed_p99_ms: u32, target_p99_ms: u32) -> Self {
        Self {
            observed_p99_ms,
            target_p99_ms,
        }
    }
}

impl serde::Serialize for SloViolation {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("SloViolation", 2)?;
        st.serialize_field("observed_p99_ms", &self.observed_p99_ms)?;
        st.serialize_field("target_p99_ms", &self.target_p99_ms)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for SloViolation {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Observed,
            Target,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("SloViolation field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "observed_p99_ms" => Ok(Field::Observed),
                            "target_p99_ms" => Ok(Field::Target),
                            other => {
                                Err(E::unknown_field(other, &["observed_p99_ms", "target_p99_ms"]))
                            }
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct VV;
        impl<'d> serde::de::Visitor<'d> for VV {
            type Value = SloViolation;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SloViolation struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<SloViolation, A::Error> {
                let mut observed: Option<u32> = None;
                let mut target: Option<u32> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Observed => {
                            if observed.is_some() {
                                return Err(serde::de::Error::duplicate_field("observed_p99_ms"));
                            }
                            observed = Some(map.next_value()?);
                        }
                        Field::Target => {
                            if target.is_some() {
                                return Err(serde::de::Error::duplicate_field("target_p99_ms"));
                            }
                            target = Some(map.next_value()?);
                        }
                    }
                }
                Ok(SloViolation {
                    observed_p99_ms: observed
                        .ok_or_else(|| serde::de::Error::missing_field("observed_p99_ms"))?,
                    target_p99_ms: target
                        .ok_or_else(|| serde::de::Error::missing_field("target_p99_ms"))?,
                })
            }
        }

        de.deserialize_struct("SloViolation", &["observed_p99_ms", "target_p99_ms"], VV)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BRIDGE_ROUTE_SLO, BRIDGE_TRANSLATE_SLO, HISTORY_SINGLE_REPO_SLO, HYBRID_MERGE_SLO,
        LEX_100_REPO_FANOUT_SLO, LEX_SINGLE_REPO_SLO, LEX_SYMBOL_SINGLE_REPO_SLO,
        RUNTIME_CATALOG_SLO, SEMANTIC_ANN_SLO, STRUCTURAL_SINGLE_REPO_SLO, SloTarget, SloViolation,
    };

    #[test]
    fn lex_single_repo_matches_rfc() {
        assert_eq!(LEX_SINGLE_REPO_SLO, SloTarget::new(50, 250, 1_000));
    }

    #[test]
    fn lex_100_fanout_p95_matches_rfc() {
        assert_eq!(LEX_100_REPO_FANOUT_SLO.p95_ms, 2_000);
    }

    #[test]
    fn symbol_slo_matches_spec() {
        assert_eq!(LEX_SYMBOL_SINGLE_REPO_SLO, SloTarget::new(30, 150, 500));
    }

    #[test]
    fn history_slo_matches_spec() {
        assert_eq!(HISTORY_SINGLE_REPO_SLO, SloTarget::new(100, 500, 2_000));
    }

    #[test]
    fn structural_slo_matches_spec() {
        assert_eq!(STRUCTURAL_SINGLE_REPO_SLO, SloTarget::new(200, 1_000, 3_000));
    }

    #[test]
    fn runtime_slo_matches_spec() {
        assert_eq!(RUNTIME_CATALOG_SLO, SloTarget::new(10, 50, 250));
    }

    #[test]
    fn bridge_translate_slo_matches_spec() {
        assert_eq!(BRIDGE_TRANSLATE_SLO, SloTarget::new(1, 1, 1));
    }

    #[test]
    fn bridge_route_slo_matches_spec() {
        assert_eq!(BRIDGE_ROUTE_SLO, SloTarget::new(5, 50, 500));
    }

    #[test]
    fn semantic_ann_slo_matches_spec() {
        assert_eq!(SEMANTIC_ANN_SLO, SloTarget::new(20, 100, 500));
    }

    #[test]
    fn hybrid_merge_slo_matches_spec() {
        assert_eq!(HYBRID_MERGE_SLO, SloTarget::new(60, 300, 1_200));
    }

    #[test]
    fn violation_detected() {
        let t = LEX_SINGLE_REPO_SLO;
        assert!(t.violates_p99(t.p99_ms.saturating_add(1)));
        assert!(!t.violates_p99(t.p99_ms));
        assert!(!t.violates_p99(t.p99_ms.saturating_sub(1)));
    }

    #[test]
    fn slo_target_serde_roundtrip() {
        let t = LEX_SINGLE_REPO_SLO;
        let buf = match serde_json::to_vec(&t) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "ser: {e}");
                return;
            }
        };
        match serde_json::from_slice::<SloTarget>(&buf) {
            Ok(got) => assert_eq!(got, t),
            Err(e) => assert!(false, "de: {e}"),
        }
    }

    #[test]
    fn slo_violation_serde_roundtrip() {
        let v = SloViolation::new(1_234, 1_000);
        let buf = match serde_json::to_vec(&v) {
            Ok(b) => b,
            Err(e) => {
                assert!(false, "ser: {e}");
                return;
            }
        };
        match serde_json::from_slice::<SloViolation>(&buf) {
            Ok(got) => assert_eq!(got, v),
            Err(e) => assert!(false, "de: {e}"),
        }
    }
}
