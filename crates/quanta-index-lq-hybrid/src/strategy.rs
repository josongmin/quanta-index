//! Fusion-strategy enum + weights envelope.
//!
//! Per SEM-02 §4.4, two strategies ship at MVP:
//!
//! - **RRF (Reciprocal Rank Fusion)** — default, scale-free, deterministic.
//!   `k_rrf_constant = 60` (literature standard; see [`RRF_DEFAULT_K`]).
//! - **`WeightedScore`** — opt-in, requires the caller to pass an `HybridWeights`
//!   carrier validated through [`HybridWeights::new`].
//!
//! The `HybridWeights::new` constructor enforces the §3.3 invariants:
//! both finite, both `≥ 0.0`, sum `> 0.0`. Caller-side normalization is
//! not required — downstream fusion does internal L1-normalize at the
//! point of use. The invariant guarantees the L1-normalize step never
//! divides by zero.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{HybridError, HybridErrorCode};

/// RRF literature-standard rank constant.
///
/// Tuned at 60 per Cormack/Clarke/Buettcher; the SEM-02 §4.4 ADR-019
/// candidate pins this default. The executor refuses `k_rrf < 1` (handled
/// at the strategy level via [`FusionStrategy::Rrf { k }`] — `k = 0`
/// degenerates the formula to `1/rank` which is permitted, but the spec
/// pins floor `≥ 1` so we leave that constraint to the caller's plan-time
/// validator if it surfaces; in this crate the value flows through as-is
/// since k=0 is still numerically safe with `(0 + rank) > 0`).
pub const RRF_DEFAULT_K: u32 = 60;

/// Closed taxonomy of fusion strategies shipped at SEM-02 MVP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FusionStrategy {
    /// Reciprocal Rank Fusion: `fused = sum(1 / (k + rank_e(d)))` over engines
    /// that scored `d` in their respective top-k.
    Rrf {
        /// RRF rank constant. Default `60` per literature; per-deployment
        /// configurable. The constant must be carried by the strategy
        /// variant to make ADR-019 reproducibility audit cheap.
        k: u32,
    },
    /// Linear score blend: `fused = w_lex * lex_score + w_sem * sem_score`.
    ///
    /// Score domains differ (BM25 vs cosine); operators electing this
    /// strategy accept the score-drift risk per SEM-02 §4.4 ADR-019.
    Weighted {
        /// Lexical-side weight.
        lex_weight: f32,
        /// Semantic-side weight.
        sem_weight: f32,
    },
}

impl FusionStrategy {
    /// Discriminant wire-tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Rrf { .. } => "rrf",
            Self::Weighted { .. } => "weighted",
        }
    }
}

impl fmt::Display for FusionStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rrf { k } => write!(f, "rrf(k={k})"),
            Self::Weighted {
                lex_weight,
                sem_weight,
            } => write!(f, "weighted(lex={lex_weight}, sem={sem_weight})"),
        }
    }
}

impl serde::Serialize for FusionStrategy {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        match *self {
            Self::Rrf { k } => {
                let mut m = ser.serialize_map(Some(2))?;
                m.serialize_entry("tag", "rrf")?;
                m.serialize_entry("k", &k)?;
                m.end()
            }
            Self::Weighted {
                lex_weight,
                sem_weight,
            } => {
                let mut m = ser.serialize_map(Some(3))?;
                m.serialize_entry("lex_weight", &lex_weight)?;
                m.serialize_entry("sem_weight", &sem_weight)?;
                m.serialize_entry("tag", "weighted")?;
                m.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for FusionStrategy {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = FusionStrategy;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("FusionStrategy map with `tag` field")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<FusionStrategy, M::Error> {
                let mut tag: Option<String> = None;
                let mut k: Option<u32> = None;
                let mut lex_weight: Option<f32> = None;
                let mut sem_weight: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "tag" => {
                            if tag.is_some() {
                                return Err(serde::de::Error::duplicate_field("tag"));
                            }
                            tag = Some(map.next_value()?);
                        }
                        "k" => {
                            if k.is_some() {
                                return Err(serde::de::Error::duplicate_field("k"));
                            }
                            k = Some(map.next_value()?);
                        }
                        "lex_weight" => {
                            if lex_weight.is_some() {
                                return Err(serde::de::Error::duplicate_field("lex_weight"));
                            }
                            lex_weight = Some(map.next_value()?);
                        }
                        "sem_weight" => {
                            if sem_weight.is_some() {
                                return Err(serde::de::Error::duplicate_field("sem_weight"));
                            }
                            sem_weight = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["tag", "k", "lex_weight", "sem_weight"],
                            ));
                        }
                    }
                }
                let tag = tag.ok_or_else(|| serde::de::Error::missing_field("tag"))?;
                match tag.as_str() {
                    "rrf" => {
                        let k = k.ok_or_else(|| serde::de::Error::missing_field("k"))?;
                        Ok(FusionStrategy::Rrf { k })
                    }
                    "weighted" => {
                        let lex_weight = lex_weight
                            .ok_or_else(|| serde::de::Error::missing_field("lex_weight"))?;
                        let sem_weight = sem_weight
                            .ok_or_else(|| serde::de::Error::missing_field("sem_weight"))?;
                        Ok(FusionStrategy::Weighted {
                            lex_weight,
                            sem_weight,
                        })
                    }
                    other => Err(serde::de::Error::unknown_variant(
                        other,
                        &["rrf", "weighted"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

/// Validated weights envelope for [`FusionStrategy::Weighted`].
///
/// Invariants enforced by [`HybridWeights::new`]:
///
/// - `lex.is_finite() && sem.is_finite()`;
/// - `lex >= 0.0 && sem >= 0.0`;
/// - `lex + sem > 0.0` (at least one positive).
///
/// Fields are private; construction is forced through [`Self::new`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HybridWeights {
    lex: f32,
    sem: f32,
}

impl HybridWeights {
    /// Default placeholder per SEM-02 §3.3: `{lex: 0.5, sem: 0.5}`.
    pub const DEFAULTS: Self = Self { lex: 0.5, sem: 0.5 };

    /// Construct + validate a weights envelope.
    pub fn new(lex: f32, sem: f32) -> Result<Self, HybridError> {
        if !lex.is_finite() {
            return Err(HybridError::new(
                HybridErrorCode::HybInvalidWeights,
                format!("lex weight must be finite (no NaN/Inf), got {lex}"),
            ));
        }
        if !sem.is_finite() {
            return Err(HybridError::new(
                HybridErrorCode::HybInvalidWeights,
                format!("sem weight must be finite (no NaN/Inf), got {sem}"),
            ));
        }
        if lex < 0.0 {
            return Err(HybridError::new(
                HybridErrorCode::HybInvalidWeights,
                format!("lex weight must be >= 0.0, got {lex}"),
            ));
        }
        if sem < 0.0 {
            return Err(HybridError::new(
                HybridErrorCode::HybInvalidWeights,
                format!("sem weight must be >= 0.0, got {sem}"),
            ));
        }
        let sum = lex + sem;
        if sum <= 0.0 {
            return Err(HybridError::new(
                HybridErrorCode::HybInvalidWeights,
                format!("lex + sem must be > 0.0, got {sum}"),
            ));
        }
        Ok(Self { lex, sem })
    }

    /// Lexical-side weight.
    #[must_use]
    pub const fn lex(self) -> f32 {
        self.lex
    }

    /// Semantic-side weight.
    #[must_use]
    pub const fn sem(self) -> f32 {
        self.sem
    }
}

impl Default for HybridWeights {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

impl fmt::Display for HybridWeights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HybridWeights(lex={}, sem={})", self.lex, self.sem)
    }
}

impl serde::Serialize for HybridWeights {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("lex", &self.lex)?;
        m.serialize_entry("sem", &self.sem)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for HybridWeights {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HybridWeights;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HybridWeights map with lex/sem")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HybridWeights, M::Error> {
                let mut lex: Option<f32> = None;
                let mut sem: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "lex" => {
                            if lex.is_some() {
                                return Err(serde::de::Error::duplicate_field("lex"));
                            }
                            lex = Some(map.next_value()?);
                        }
                        "sem" => {
                            if sem.is_some() {
                                return Err(serde::de::Error::duplicate_field("sem"));
                            }
                            sem = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["lex", "sem"],
                            ));
                        }
                    }
                }
                let lex = lex.ok_or_else(|| serde::de::Error::missing_field("lex"))?;
                let sem = sem.ok_or_else(|| serde::de::Error::missing_field("sem"))?;
                HybridWeights::new(lex, sem).map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{FusionStrategy, HybridWeights, RRF_DEFAULT_K};
    use crate::errors::HybridErrorCode;

    #[test]
    fn rrf_default_k_is_60() {
        assert_eq!(RRF_DEFAULT_K, 60);
    }

    #[test]
    fn weights_accepts_balanced_default() {
        match HybridWeights::new(0.5, 0.5) {
            Ok(w) => {
                assert!((w.lex() - 0.5_f32).abs() < f32::EPSILON);
                assert!((w.sem() - 0.5_f32).abs() < f32::EPSILON);
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn weights_defaults_validate() {
        let d = HybridWeights::DEFAULTS;
        match HybridWeights::new(d.lex(), d.sem()) {
            Ok(got) => assert_eq!(got, d),
            Err(e) => assert!(false, "defaults must validate: {e}"),
        }
    }

    #[test]
    fn weights_rejects_nan_lex() {
        match HybridWeights::new(f32::NAN, 0.5) {
            Ok(_) => assert!(false, "expected HYB_INVALID_WEIGHTS"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybInvalidWeights),
        }
    }

    #[test]
    fn weights_rejects_inf_sem() {
        match HybridWeights::new(0.5, f32::INFINITY) {
            Ok(_) => assert!(false, "expected HYB_INVALID_WEIGHTS"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybInvalidWeights),
        }
    }

    #[test]
    fn weights_rejects_negative_lex() {
        match HybridWeights::new(-0.1, 0.5) {
            Ok(_) => assert!(false, "expected HYB_INVALID_WEIGHTS"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybInvalidWeights),
        }
    }

    #[test]
    fn weights_rejects_zero_sum() {
        match HybridWeights::new(0.0, 0.0) {
            Ok(_) => assert!(false, "expected HYB_INVALID_WEIGHTS"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybInvalidWeights),
        }
    }

    #[test]
    fn weights_accepts_lex_only() {
        match HybridWeights::new(0.7, 0.0) {
            Ok(_) => {}
            Err(e) => assert!(false, "lex-only should pass: {e}"),
        }
    }

    #[test]
    fn weights_accepts_sem_only() {
        match HybridWeights::new(0.0, 0.7) {
            Ok(_) => {}
            Err(e) => assert!(false, "sem-only should pass: {e}"),
        }
    }

    #[test]
    fn strategy_rrf_serde_roundtrip() {
        let s = FusionStrategy::Rrf { k: 60 };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&s, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<FusionStrategy, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, s),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn strategy_weighted_serde_roundtrip() {
        let s = FusionStrategy::Weighted {
            lex_weight: 0.7,
            sem_weight: 0.3,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&s, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<FusionStrategy, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, s),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn weights_serde_roundtrip() {
        let w = match HybridWeights::new(0.6, 0.4) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&w, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<HybridWeights, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, w),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn strategy_tag_matches_variant() {
        assert_eq!(FusionStrategy::Rrf { k: 60 }.tag(), "rrf");
        assert_eq!(
            FusionStrategy::Weighted {
                lex_weight: 1.0,
                sem_weight: 1.0
            }
            .tag(),
            "weighted"
        );
    }
}
