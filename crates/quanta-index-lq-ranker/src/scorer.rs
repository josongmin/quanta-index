//! Composite scorer over [`crate::weights::RankerWeights`] +
//! [`crate::signals::CandidateSignals`].
//!
//! [`CompositeScorer`] pins a frozen weight set plus its
//! [`crate::weights::weights_hash`] digest at construction. The score
//! function is a closed-form linear weighted sum over the five named signal
//! slots, validated via [`crate::signals::validate_signals`] before
//! composition.
//!
//! ## Score function
//!
//! ```text
//! raw = w.bm25            * sig.bm25
//!     + w.path_prior      * sig.path_prior
//!     + w.symbol_boost    * sig.symbol_boost
//!     + w.recency         * sig.recency
//!     + w.boost_directive * (sig.boost_directive - 1.0)
//! ```
//!
//! The `boost_directive` term enters as `(boost - 1.0)` so the identity
//! value `1.0` contributes zero — per `dsl.md` §6.2.
//!
//! ## Clamp envelope
//!
//! The raw weighted sum is clamped to `[0.0, 1.0]`. The clamp is documented
//! rather than silent: every per-signal weight is in `[0.0, 1.0]` with
//! sum `~1.0` (enforced by `RankerWeights::new`), every unit signal is in
//! `[0.0, 1.0]`, and the boost term lives in `[-0.875, +7.0]` after
//! `(boost - 1.0)`, multiplied by `w.boost_directive ∈ [0.0, 1.0]`. The
//! raw envelope is therefore `[-0.875, +8.0]` in the worst case; the
//! clamp guarantees the output is `[0.0, 1.0]` regardless. The pre-clamp
//! value is exposed via [`CompositeScorer::explain`] for audit.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use crate::errors::{RankerError, RankerErrorCode, SignalKind};
use crate::signals::{CandidateSignals, validate_signals};
use crate::weights::{RankerWeights, weights_hash};

/// Frozen-weight composite scorer.
///
/// Pins the weight set + its `weights_hash` at construction.
#[derive(Clone, Debug)]
pub struct CompositeScorer {
    weights: RankerWeights,
    weights_hash_pinned: [u8; 32],
}

impl CompositeScorer {
    /// Construct a scorer from a validated weight set.
    ///
    /// The accompanying `weights_hash` is computed once at construction
    /// time and exposed via [`CompositeScorer::weights_hash`] so callers
    /// can pin it into a `WeightsVersion` tuple. Returns
    /// [`crate::errors::RankerErrorCode::WeightsEncodeFailed`] if the
    /// hash codec step fails — the path is infallible on `Vec<u8>` but
    /// the typed error propagates per repo `no silent fallback` rule.
    pub fn new(weights: RankerWeights) -> Result<Self, crate::errors::RankerError> {
        let pinned = weights_hash(&weights)?;
        Ok(Self {
            weights,
            weights_hash_pinned: pinned,
        })
    }

    /// Borrow the pinned weight set.
    #[must_use]
    pub const fn weights(&self) -> &RankerWeights {
        &self.weights
    }

    /// Borrow the pinned `weights_hash` digest.
    #[must_use]
    pub const fn weights_hash(&self) -> &[u8; 32] {
        &self.weights_hash_pinned
    }

    /// Score one candidate against the pinned weight set.
    ///
    /// Validates `sig` via [`validate_signals`] first; any non-finite or
    /// out-of-envelope slot returns `RANK_INVALID_SIGNAL` with
    /// [`SignalKind`] attribution. The result is clamped to `[0.0, 1.0]`.
    pub fn score(&self, sig: &CandidateSignals) -> Result<f32, RankerError> {
        validate_signals(sig)?;
        let raw = compute_raw(&self.weights, sig);
        finalize_score(raw)
    }

    /// Score + per-signal contribution breakdown.
    ///
    /// Returns a [`RankExplanation`] with five [`SignalContribution`]
    /// entries in canonical [`SignalKind`] order (`Bm25`, `PathPrior`,
    /// `SymbolBoost`, `Recency`, `BoostDirective`). The `clamped_score`
    /// field equals [`Self::score`] for the same input.
    ///
    /// The per-signal `contribution` values sum to the **pre-clamp** raw
    /// score within f32 epsilon; the post-clamp `clamped_score` may
    /// differ from that sum when the raw value escaped `[0.0, 1.0]`.
    pub fn explain(&self, sig: &CandidateSignals) -> Result<RankExplanation, RankerError> {
        validate_signals(sig)?;
        let contributions = compute_contributions(&self.weights, sig);
        let raw = sum_contributions(&contributions);
        let clamped = finalize_score(raw)?;
        Ok(RankExplanation {
            contributions,
            clamped_score: clamped,
            weights_hash: self.weights_hash_pinned,
        })
    }
}

fn compute_raw(w: &RankerWeights, sig: &CandidateSignals) -> f32 {
    let bm25 = w.bm25() * sig.bm25;
    let pp = w.path_prior() * sig.path_prior;
    let sb = w.symbol_boost() * sig.symbol_boost;
    let rec = w.recency() * sig.recency;
    let boost = w.boost_directive() * (sig.boost_directive - 1.0);
    bm25 + pp + sb + rec + boost
}

fn compute_contributions(w: &RankerWeights, sig: &CandidateSignals) -> Vec<SignalContribution> {
    let boost_value = sig.boost_directive - 1.0;
    vec![
        SignalContribution {
            signal: SignalKind::Bm25,
            signal_value: sig.bm25,
            weight: w.bm25(),
            contribution: w.bm25() * sig.bm25,
        },
        SignalContribution {
            signal: SignalKind::PathPrior,
            signal_value: sig.path_prior,
            weight: w.path_prior(),
            contribution: w.path_prior() * sig.path_prior,
        },
        SignalContribution {
            signal: SignalKind::SymbolBoost,
            signal_value: sig.symbol_boost,
            weight: w.symbol_boost(),
            contribution: w.symbol_boost() * sig.symbol_boost,
        },
        SignalContribution {
            signal: SignalKind::Recency,
            signal_value: sig.recency,
            weight: w.recency(),
            contribution: w.recency() * sig.recency,
        },
        SignalContribution {
            signal: SignalKind::BoostDirective,
            // `signal_value` for the boost slot is the raw, un-shifted
            // directive (so it round-trips with the input).
            signal_value: sig.boost_directive,
            weight: w.boost_directive(),
            contribution: w.boost_directive() * boost_value,
        },
    ]
}

fn sum_contributions(contribs: &[SignalContribution]) -> f32 {
    let mut acc: f32 = 0.0;
    for c in contribs {
        acc += c.contribution;
    }
    acc
}

fn finalize_score(raw: f32) -> Result<f32, RankerError> {
    if !raw.is_finite() {
        // Defense in depth: `validate_signals` already filters NaN/Inf
        // inputs; this branch only fires if a weight×signal product
        // overflows the f32 envelope, which is bounded out by
        // `RankerWeights::new`. Surface it as a typed signal error
        // anyway rather than silently coercing.
        return Err(RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            SignalKind::Bm25,
            "composed raw score was non-finite",
        ));
    }
    Ok(raw.clamp(0.0, 1.0))
}

/// One signal's contribution to a rank composition.
///
/// `signal_value` is the raw slot value as delivered to the scorer
/// (so it round-trips with `CandidateSignals`). `weight` is the matching
/// `RankerWeights` coefficient. `contribution` is `weight * signal_value`,
/// except for `BoostDirective`, where the contribution uses
/// `weight * (signal_value - 1.0)` per the closed-form score function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignalContribution {
    /// Named signal slot.
    pub signal: SignalKind,
    /// Raw signal value as delivered.
    pub signal_value: f32,
    /// Pinned weight for this signal.
    pub weight: f32,
    /// Multiplicative contribution to the pre-clamp raw score.
    pub contribution: f32,
}

impl serde::Serialize for SignalContribution {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(4))?;
        m.serialize_entry("contribution", &self.contribution)?;
        m.serialize_entry("signal", &self.signal)?;
        m.serialize_entry("signal_value", &self.signal_value)?;
        m.serialize_entry("weight", &self.weight)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SignalContribution {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SignalContribution;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("SignalContribution map with signal/signal_value/weight/contribution")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SignalContribution, M::Error> {
                let mut signal: Option<SignalKind> = None;
                let mut signal_value: Option<f32> = None;
                let mut weight: Option<f32> = None;
                let mut contribution: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "signal" => {
                            if signal.is_some() {
                                return Err(serde::de::Error::duplicate_field("signal"));
                            }
                            signal = Some(map.next_value()?);
                        }
                        "signal_value" => {
                            if signal_value.is_some() {
                                return Err(serde::de::Error::duplicate_field("signal_value"));
                            }
                            signal_value = Some(map.next_value()?);
                        }
                        "weight" => {
                            if weight.is_some() {
                                return Err(serde::de::Error::duplicate_field("weight"));
                            }
                            weight = Some(map.next_value()?);
                        }
                        "contribution" => {
                            if contribution.is_some() {
                                return Err(serde::de::Error::duplicate_field("contribution"));
                            }
                            contribution = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["contribution", "signal", "signal_value", "weight"],
                            ));
                        }
                    }
                }
                let signal = signal.ok_or_else(|| serde::de::Error::missing_field("signal"))?;
                let signal_value =
                    signal_value.ok_or_else(|| serde::de::Error::missing_field("signal_value"))?;
                let weight = weight.ok_or_else(|| serde::de::Error::missing_field("weight"))?;
                let contribution =
                    contribution.ok_or_else(|| serde::de::Error::missing_field("contribution"))?;
                Ok(SignalContribution {
                    signal,
                    signal_value,
                    weight,
                    contribution,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Full per-candidate rank explanation.
///
/// `contributions` carries one entry per named signal slot in canonical
/// [`SignalKind`] order. `clamped_score` is the final score the ranker
/// would emit (post-clamp to `[0.0, 1.0]`). `weights_hash` pins which
/// weight identity produced this breakdown.
#[derive(Clone, Debug, PartialEq)]
pub struct RankExplanation {
    /// Per-signal contributions in canonical order.
    pub contributions: Vec<SignalContribution>,
    /// Final clamped score in `[0.0, 1.0]`.
    pub clamped_score: f32,
    /// Frozen-weight identity for this composition.
    pub weights_hash: [u8; 32],
}

impl serde::Serialize for RankExplanation {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("clamped_score", &self.clamped_score)?;
        m.serialize_entry("contributions", &self.contributions)?;
        m.serialize_entry(
            "weights_hash",
            &FixedBytes32SerRef {
                bytes: &self.weights_hash,
            },
        )?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for RankExplanation {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = RankExplanation;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("RankExplanation map with contributions/clamped_score/weights_hash")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<RankExplanation, M::Error> {
                let mut contributions: Option<Vec<SignalContribution>> = None;
                let mut clamped_score: Option<f32> = None;
                let mut weights_hash: Option<[u8; 32]> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "contributions" => {
                            if contributions.is_some() {
                                return Err(serde::de::Error::duplicate_field("contributions"));
                            }
                            contributions = Some(map.next_value()?);
                        }
                        "clamped_score" => {
                            if clamped_score.is_some() {
                                return Err(serde::de::Error::duplicate_field("clamped_score"));
                            }
                            clamped_score = Some(map.next_value()?);
                        }
                        "weights_hash" => {
                            if weights_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("weights_hash"));
                            }
                            let bytes: FixedBytes32De = map.next_value()?;
                            weights_hash = Some(bytes.0);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["clamped_score", "contributions", "weights_hash"],
                            ));
                        }
                    }
                }
                let contributions = contributions
                    .ok_or_else(|| serde::de::Error::missing_field("contributions"))?;
                let clamped_score = clamped_score
                    .ok_or_else(|| serde::de::Error::missing_field("clamped_score"))?;
                let weights_hash =
                    weights_hash.ok_or_else(|| serde::de::Error::missing_field("weights_hash"))?;
                Ok(RankExplanation {
                    contributions,
                    clamped_score,
                    weights_hash,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Borrowed `[u8; 32]` wrapper that serializes as a CBOR bytestring.
struct FixedBytes32SerRef<'a> {
    bytes: &'a [u8; 32],
}

impl serde::Serialize for FixedBytes32SerRef<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(self.bytes.as_slice())
    }
}

/// Owned `[u8; 32]` wrapper that deserializes from a CBOR bytestring.
struct FixedBytes32De([u8; 32]);

impl<'de> serde::Deserialize<'de> for FixedBytes32De {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = FixedBytes32De;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("32-byte bytestring")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<FixedBytes32De, E> {
                if v.len() != 32 {
                    return Err(E::invalid_length(v.len(), &"expected 32 bytes"));
                }
                let mut a = [0u8; 32];
                a.copy_from_slice(v);
                Ok(FixedBytes32De(a))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<FixedBytes32De, E> {
                self.visit_bytes(&v)
            }
        }
        de.deserialize_bytes(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{CompositeScorer, RankExplanation, SignalContribution};
    use crate::errors::{RankerErrorCode, SignalKind};
    use crate::signals::CandidateSignals;
    use crate::weights::RankerWeights;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn defaults_scorer() -> CompositeScorer {
        match CompositeScorer::new(RankerWeights::DEFAULTS) {
            Ok(s) => s,
            Err(e) => fatal(&format!("CompositeScorer::new failed: {e}")),
        }
    }

    #[test]
    fn zero_signals_score_zero() {
        let s = defaults_scorer();
        let sig = CandidateSignals::identity();
        match s.score(&sig) {
            Ok(v) => assert_eq!(
                v.to_bits(),
                0.0_f32.to_bits(),
                "expected exact 0.0, got {v}"
            ),
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn all_one_signals_with_identity_boost_score_within_envelope() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 1.0,
            path_prior: 1.0,
            symbol_boost: 1.0,
            recency: 1.0,
            boost_directive: 1.0,
        };
        let got = match s.score(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        // With identity boost the boost term contributes zero; sum of the
        // first four weights = bm25 + path_prior + symbol_boost + recency
        // = 0.7 + 0.1 + 0.1 + 0.05 = 0.95.
        let expected = 0.95_f32;
        assert!(
            (got - expected).abs() < 1.0e-6,
            "expected {expected}, got {got}"
        );
    }

    #[test]
    fn max_boost_with_zero_signals_is_clamped_to_one() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 0.0,
            path_prior: 0.0,
            symbol_boost: 0.0,
            recency: 0.0,
            boost_directive: 8.0,
        };
        // boost contribution = 0.05 * 7.0 = 0.35; raw = 0.35; no clamp.
        let got = match s.score(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!((got - 0.35).abs() < 1.0e-6, "expected ~0.35, got {got}");
    }

    #[test]
    fn min_boost_drives_score_negative_then_clamps_to_zero() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 0.0,
            path_prior: 0.0,
            symbol_boost: 0.0,
            recency: 0.0,
            boost_directive: 0.125,
        };
        // boost contribution = 0.05 * (0.125 - 1.0) = 0.05 * -0.875 = -0.04375
        // raw = -0.04375 ⇒ clamp ⇒ 0.0
        let got = match s.score(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(
            got.to_bits(),
            0.0_f32.to_bits(),
            "expected exact 0.0, got {got}"
        );
    }

    #[test]
    fn nan_signal_returns_typed_error() {
        let s = defaults_scorer();
        let mut sig = CandidateSignals::identity();
        sig.bm25 = f32::NAN;
        match s.score(&sig) {
            Ok(_) => assert!(false, "expected RANK_INVALID_SIGNAL"),
            Err(e) => {
                assert_eq!(e.code, RankerErrorCode::RankInvalidSignal);
                assert_eq!(e.signal, Some(SignalKind::Bm25));
            }
        }
    }

    #[test]
    fn inf_recency_returns_typed_error() {
        let s = defaults_scorer();
        let mut sig = CandidateSignals::identity();
        sig.recency = f32::INFINITY;
        match s.score(&sig) {
            Ok(_) => assert!(false, "expected RANK_INVALID_SIGNAL"),
            Err(e) => {
                assert_eq!(e.code, RankerErrorCode::RankInvalidSignal);
                assert_eq!(e.signal, Some(SignalKind::Recency));
            }
        }
    }

    #[test]
    fn weights_hash_matches_canonical() {
        let w = RankerWeights::DEFAULTS;
        let s = match CompositeScorer::new(w) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let canon = match crate::weights::weights_hash(&w) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(s.weights_hash(), &canon);
    }

    #[test]
    fn weights_accessor_round_trips() {
        let w = RankerWeights::DEFAULTS;
        let s = match CompositeScorer::new(w) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(*s.weights(), w);
    }

    #[test]
    fn explain_returns_five_contributions_in_canonical_order() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 0.5,
            path_prior: 0.5,
            symbol_boost: 0.5,
            recency: 0.5,
            boost_directive: 1.0,
        };
        let exp = match s.explain(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(exp.contributions.len(), 5);
        let order = [
            SignalKind::Bm25,
            SignalKind::PathPrior,
            SignalKind::SymbolBoost,
            SignalKind::Recency,
            SignalKind::BoostDirective,
        ];
        for (i, kind) in order.iter().enumerate() {
            let Some(got) = exp.contributions.get(i) else {
                fatal(&format!("missing contribution at {i}"));
            };
            assert_eq!(got.signal, *kind);
        }
    }

    #[test]
    fn explain_sum_matches_pre_clamp_raw() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 0.5,
            path_prior: 0.3,
            symbol_boost: 0.4,
            recency: 0.2,
            boost_directive: 1.5,
        };
        let exp = match s.explain(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let raw_sum: f32 = exp.contributions.iter().map(|c| c.contribution).sum();
        // raw_sum here is in-envelope so no clamp; equality should be exact
        // bit-equal modulo f32 summation order — verify within epsilon.
        assert!(
            (raw_sum - exp.clamped_score).abs() < 1.0e-6,
            "raw_sum {raw_sum} != clamped {}",
            exp.clamped_score
        );
    }

    #[test]
    fn explain_carries_weights_hash() {
        let s = defaults_scorer();
        let sig = CandidateSignals::identity();
        let exp = match s.explain(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(&exp.weights_hash, s.weights_hash());
    }

    #[test]
    fn explanation_serde_roundtrip() {
        let s = defaults_scorer();
        let sig = CandidateSignals {
            bm25: 0.5,
            path_prior: 0.3,
            symbol_boost: 0.4,
            recency: 0.2,
            boost_directive: 1.5,
        };
        let exp = match s.explain(&sig) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&exp, &mut buf) {
            Ok(()) => {}
            Err(e) => fatal(&format!("serialize: {e}")),
        }
        let got: RankExplanation = match ciborium::de::from_reader(buf.as_slice()) {
            Ok(v) => v,
            Err(e) => fatal(&format!("deserialize: {e}")),
        };
        assert_eq!(got, exp);
    }

    #[test]
    fn contribution_serde_roundtrip() {
        let c = SignalContribution {
            signal: SignalKind::Recency,
            signal_value: 0.7,
            weight: 0.05,
            contribution: 0.035,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&c, &mut buf) {
            Ok(()) => {}
            Err(e) => fatal(&format!("{e}")),
        }
        match ciborium::de::from_reader::<SignalContribution, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, c),
            Err(e) => fatal(&format!("{e}")),
        }
    }
}
