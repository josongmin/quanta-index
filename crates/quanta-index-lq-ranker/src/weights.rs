//! Frozen-per-generation ranker weight carrier.
//!
//! [`RankerWeightsV1`] pins the linear-weighted-sum coefficients for the
//! composite ranker. Default values land via ADR-006 and are exported as
//! [`RankerWeightsV1::DEFAULTS`]; every other construction must pass through
//! [`RankerWeightsV1::new`] which validates the envelope.
//!
//! The accompanying [`weights_hash`] is the SHA-256 of the canonical CBOR
//! encoding of the weight set, version-tagged with `b"RankerWeightsV1\0"`
//! so a hypothetical future `RankerWeightsV2` can never collide.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use sha2::{Digest as _, Sha256};

use crate::errors::{RankerError, RankerErrorCode};

/// Version tag prefix for `weights_hash`.
const VERSION_TAG: &[u8] = b"RankerWeightsV1\0";

/// Tolerance for the weight-sum invariant check.
const SUM_TOLERANCE: f32 = 1.0e-3;

/// Linear-weighted-sum coefficients for the composite ranker.
///
/// Invariants enforced by [`RankerWeightsV1::new`]:
///
/// - every field is finite (no `NaN`, no infinity);
/// - every field is in the closed range `[0.0, 1.0]`;
/// - the field sum is within `1e-3` of `1.0`.
///
/// Fields are kept private behind accessors so the invariants cannot be
/// bypassed by direct construction; the only literal construction is
/// [`RankerWeightsV1::DEFAULTS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankerWeightsV1 {
    bm25: f32,
    path_prior: f32,
    symbol_boost: f32,
    recency: f32,
    boost_directive: f32,
}

impl RankerWeightsV1 {
    /// Default weight set per ADR-006 candidate.
    pub const DEFAULTS: Self = Self {
        bm25: 0.7,
        path_prior: 0.1,
        symbol_boost: 0.1,
        recency: 0.05,
        boost_directive: 0.05,
    };

    /// Construct + validate a weight set.
    pub fn new(
        bm25: f32,
        path_prior: f32,
        symbol_boost: f32,
        recency: f32,
        boost_directive: f32,
    ) -> Result<Self, RankerError> {
        for (name, v) in [
            ("bm25", bm25),
            ("path_prior", path_prior),
            ("symbol_boost", symbol_boost),
            ("recency", recency),
            ("boost_directive", boost_directive),
        ] {
            if !v.is_finite() {
                return Err(RankerError::new(
                    RankerErrorCode::InvalidWeights,
                    format!("weight `{name}` must be finite (not NaN/Inf), got {v}"),
                ));
            }
            if !(0.0..=1.0).contains(&v) {
                return Err(RankerError::new(
                    RankerErrorCode::InvalidWeights,
                    format!("weight `{name}` must be in [0.0, 1.0], got {v}"),
                ));
            }
        }
        let sum = bm25 + path_prior + symbol_boost + recency + boost_directive;
        let diff = (sum - 1.0).abs();
        if diff > SUM_TOLERANCE {
            return Err(RankerError::new(
                RankerErrorCode::InvalidWeights,
                format!("weight sum must be ~1.0 (tol {SUM_TOLERANCE}); got sum={sum}"),
            ));
        }
        Ok(Self {
            bm25,
            path_prior,
            symbol_boost,
            recency,
            boost_directive,
        })
    }

    /// BM25 (LEX-01) component weight.
    #[must_use]
    pub const fn bm25(self) -> f32 {
        self.bm25
    }

    /// Path-prior signal weight.
    #[must_use]
    pub const fn path_prior(self) -> f32 {
        self.path_prior
    }

    /// Symbol-class boost signal weight.
    #[must_use]
    pub const fn symbol_boost(self) -> f32 {
        self.symbol_boost
    }

    /// Document-recency signal weight.
    #[must_use]
    pub const fn recency(self) -> f32 {
        self.recency
    }

    /// `boost:` directive signal weight.
    #[must_use]
    pub const fn boost_directive(self) -> f32 {
        self.boost_directive
    }
}

impl Default for RankerWeightsV1 {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

impl fmt::Display for RankerWeightsV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RankerWeightsV1(bm25={}, path_prior={}, symbol_boost={}, recency={}, boost_directive={})",
            self.bm25, self.path_prior, self.symbol_boost, self.recency, self.boost_directive,
        )
    }
}

/// SHA-256 of the canonical CBOR encoding of `weights`, version-tagged.
///
/// The digest is computed over `VERSION_TAG || cbor(weights)`, where
/// `cbor(weights)` is produced by the hand-rolled serde implementation
/// on [`RankerWeightsV1`]. The version tag guarantees that no future
/// `RankerWeightsV2` can hash-collide with `RankerWeightsV1`.
///
/// Returns [`RankerErrorCode::WeightsEncodeFailed`] if the CBOR
/// serialize step fails. `Vec<u8>` writes are infallible in practice,
/// but the function propagates the typed error rather than falling
/// back to a heuristic alternate-algorithm digest — a single weight
/// set must hash to exactly one digest under exactly one algorithm.
pub fn weights_hash(weights: &RankerWeightsV1) -> Result<[u8; 32], crate::errors::RankerError> {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    ciborium::ser::into_writer(weights, &mut buf).map_err(|e| {
        crate::errors::RankerError::new(
            crate::errors::RankerErrorCode::WeightsEncodeFailed,
            format!("cbor encode of RankerWeightsV1 failed: {e}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(VERSION_TAG);
    hasher.update(&buf);
    let out = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(out.as_slice());
    Ok(arr)
}

impl serde::Serialize for RankerWeightsV1 {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(5))?;
        m.serialize_entry("bm25", &self.bm25)?;
        m.serialize_entry("boost_directive", &self.boost_directive)?;
        m.serialize_entry("path_prior", &self.path_prior)?;
        m.serialize_entry("recency", &self.recency)?;
        m.serialize_entry("symbol_boost", &self.symbol_boost)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for RankerWeightsV1 {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = RankerWeightsV1;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "RankerWeightsV1 map with fields bm25, boost_directive, path_prior, recency, symbol_boost",
                )
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<RankerWeightsV1, M::Error> {
                let mut bm25: Option<f32> = None;
                let mut path_prior: Option<f32> = None;
                let mut symbol_boost: Option<f32> = None;
                let mut recency: Option<f32> = None;
                let mut boost_directive: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "bm25" => {
                            if bm25.is_some() {
                                return Err(serde::de::Error::duplicate_field("bm25"));
                            }
                            bm25 = Some(map.next_value()?);
                        }
                        "path_prior" => {
                            if path_prior.is_some() {
                                return Err(serde::de::Error::duplicate_field("path_prior"));
                            }
                            path_prior = Some(map.next_value()?);
                        }
                        "symbol_boost" => {
                            if symbol_boost.is_some() {
                                return Err(serde::de::Error::duplicate_field("symbol_boost"));
                            }
                            symbol_boost = Some(map.next_value()?);
                        }
                        "recency" => {
                            if recency.is_some() {
                                return Err(serde::de::Error::duplicate_field("recency"));
                            }
                            recency = Some(map.next_value()?);
                        }
                        "boost_directive" => {
                            if boost_directive.is_some() {
                                return Err(serde::de::Error::duplicate_field("boost_directive"));
                            }
                            boost_directive = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &[
                                    "bm25",
                                    "boost_directive",
                                    "path_prior",
                                    "recency",
                                    "symbol_boost",
                                ],
                            ));
                        }
                    }
                }
                let bm25 = bm25.ok_or_else(|| serde::de::Error::missing_field("bm25"))?;
                let path_prior =
                    path_prior.ok_or_else(|| serde::de::Error::missing_field("path_prior"))?;
                let symbol_boost =
                    symbol_boost.ok_or_else(|| serde::de::Error::missing_field("symbol_boost"))?;
                let recency = recency.ok_or_else(|| serde::de::Error::missing_field("recency"))?;
                let boost_directive = boost_directive
                    .ok_or_else(|| serde::de::Error::missing_field("boost_directive"))?;
                RankerWeightsV1::new(bm25, path_prior, symbol_boost, recency, boost_directive)
                    .map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{RankerWeightsV1, weights_hash};
    use crate::errors::RankerErrorCode;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn assert_invalid(r: Result<RankerWeightsV1, crate::errors::RankerError>) {
        match r {
            Ok(_) => assert!(false, "expected InvalidWeights"),
            Err(e) => assert_eq!(e.code, RankerErrorCode::InvalidWeights),
        }
    }

    #[test]
    fn defaults_validate() {
        let d = RankerWeightsV1::DEFAULTS;
        let got = match RankerWeightsV1::new(
            d.bm25(),
            d.path_prior(),
            d.symbol_boost(),
            d.recency(),
            d.boost_directive(),
        ) {
            Ok(v) => v,
            Err(e) => fatal(&format!("defaults must validate: {e}")),
        };
        assert_eq!(got, d);
    }

    #[test]
    fn defaults_sum_to_one() {
        let d = RankerWeightsV1::DEFAULTS;
        let s = d.bm25() + d.path_prior() + d.symbol_boost() + d.recency() + d.boost_directive();
        assert!((s - 1.0).abs() < 1.0e-6, "defaults sum {s} != 1.0");
    }

    #[test]
    fn rejects_nan() {
        assert_invalid(RankerWeightsV1::new(f32::NAN, 0.1, 0.1, 0.05, 0.05));
    }

    #[test]
    fn rejects_infinity() {
        assert_invalid(RankerWeightsV1::new(0.7, f32::INFINITY, 0.1, 0.05, 0.05));
    }

    #[test]
    fn rejects_negative_weight() {
        assert_invalid(RankerWeightsV1::new(0.7, -0.1, 0.1, 0.05, 0.05));
    }

    #[test]
    fn rejects_weight_above_one() {
        assert_invalid(RankerWeightsV1::new(1.5, 0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn rejects_sum_far_from_one() {
        assert_invalid(RankerWeightsV1::new(0.5, 0.5, 0.5, 0.5, 0.5));
    }

    #[test]
    fn accepts_sum_within_tolerance() {
        match RankerWeightsV1::new(0.7005, 0.1, 0.1, 0.05, 0.05) {
            Ok(_) => {}
            Err(e) => fatal(&format!("should accept near-1 sum: {e}")),
        }
    }

    #[test]
    fn rejects_sum_at_zero() {
        assert_invalid(RankerWeightsV1::new(0.0, 0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn hash_is_deterministic() {
        let d = RankerWeightsV1::DEFAULTS;
        let h1 = match weights_hash(&d) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let h2 = match weights_hash(&d) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_when_weight_changes() {
        let a = RankerWeightsV1::DEFAULTS;
        let b = match RankerWeightsV1::new(0.69, 0.11, 0.1, 0.05, 0.05) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let ha = match weights_hash(&a) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let hb = match weights_hash(&b) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_ne!(ha, hb);
    }

    #[test]
    fn cbor_roundtrip_preserves_value() {
        let d = RankerWeightsV1::DEFAULTS;
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&d, &mut buf) {
            Ok(()) => {}
            Err(e) => fatal(&format!("serialize: {e}")),
        }
        match ciborium::de::from_reader::<RankerWeightsV1, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, d),
            Err(e) => fatal(&format!("deserialize: {e}")),
        }
    }

    #[test]
    fn cbor_encoding_byte_identical_across_calls() {
        let d = RankerWeightsV1::DEFAULTS;
        let mut a: Vec<u8> = Vec::new();
        let mut b: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&d, &mut a) {
            fatal(&format!("{e}"));
        }
        if let Err(e) = ciborium::ser::into_writer(&d, &mut b) {
            fatal(&format!("{e}"));
        }
        assert_eq!(a, b);
    }
}
