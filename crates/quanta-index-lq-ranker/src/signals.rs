//! Per-candidate ranking signals.
//!
//! [`CandidateSignals`] carries every named, finite-domain signal slot
//! consumed by the composite ranker. Every slot is an `f32`; the envelope
//! per slot is:
//!
//! - `bm25`            ∈ `[0.0, 1.0]`
//! - `path_prior`      ∈ `[0.0, 1.0]`
//! - `symbol_boost`    ∈ `[0.0, 1.0]`
//! - `recency`         ∈ `[0.0, 1.0]`
//! - `boost_directive` ∈ `[0.125, 8.0]` (dsl.md §13), default identity `1.0`
//!
//! [`validate_signals`] is fail-closed: a `NaN`/`Inf` slot or an
//! out-of-envelope value yields `RANK_INVALID_SIGNAL` with signal
//! attribution.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{RankerError, RankerErrorCode, SignalKind};

/// `boost:` directive minimum per dsl.md §13.
pub const BOOST_DIRECTIVE_MIN: f32 = 0.125;
/// `boost:` directive maximum per dsl.md §13.
pub const BOOST_DIRECTIVE_MAX: f32 = 8.0;
/// `boost:` directive identity value (default; contributes zero).
pub const BOOST_DIRECTIVE_IDENTITY: f32 = 1.0;

/// Per-candidate ranking signals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateSignals {
    /// BM25 (LEX-01) normalized score, `[0.0, 1.0]`.
    pub bm25: f32,
    /// Path-prior classifier signal, `[0.0, 1.0]`.
    pub path_prior: f32,
    /// Symbol-class boost signal, `[0.0, 1.0]`.
    pub symbol_boost: f32,
    /// Document-recency signal, `[0.0, 1.0]`, newer = higher.
    pub recency: f32,
    /// `boost:` directive signal, `[0.125, 8.0]`. Identity = `1.0`.
    pub boost_directive: f32,
}

impl CandidateSignals {
    /// Build a zero-signal envelope with `boost_directive` at identity.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            bm25: 0.0,
            path_prior: 0.0,
            symbol_boost: 0.0,
            recency: 0.0,
            boost_directive: BOOST_DIRECTIVE_IDENTITY,
        }
    }
}

impl Default for CandidateSignals {
    fn default() -> Self {
        Self::identity()
    }
}

impl fmt::Display for CandidateSignals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CandidateSignals(bm25={}, path_prior={}, symbol_boost={}, recency={}, boost_directive={})",
            self.bm25, self.path_prior, self.symbol_boost, self.recency, self.boost_directive,
        )
    }
}

/// Validate every signal slot in `sig`.
///
/// Returns `Ok(())` if every slot is finite and inside its declared
/// envelope; otherwise returns `RANK_INVALID_SIGNAL` with the offending
/// slot named via [`SignalKind`].
pub fn validate_signals(sig: &CandidateSignals) -> Result<(), RankerError> {
    validate_unit(sig.bm25, SignalKind::Bm25)?;
    validate_unit(sig.path_prior, SignalKind::PathPrior)?;
    validate_unit(sig.symbol_boost, SignalKind::SymbolBoost)?;
    validate_unit(sig.recency, SignalKind::Recency)?;
    validate_boost(sig.boost_directive)?;
    Ok(())
}

fn validate_unit(v: f32, kind: SignalKind) -> Result<(), RankerError> {
    if !v.is_finite() {
        return Err(RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            kind,
            format!("signal must be finite (not NaN/Inf), got {v}"),
        ));
    }
    if !(0.0..=1.0).contains(&v) {
        return Err(RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            kind,
            format!("signal must be in [0.0, 1.0], got {v}"),
        ));
    }
    Ok(())
}

fn validate_boost(v: f32) -> Result<(), RankerError> {
    if !v.is_finite() {
        return Err(RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            SignalKind::BoostDirective,
            format!("boost_directive must be finite (not NaN/Inf), got {v}"),
        ));
    }
    if !(BOOST_DIRECTIVE_MIN..=BOOST_DIRECTIVE_MAX).contains(&v) {
        return Err(RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            SignalKind::BoostDirective,
            format!(
                "boost_directive must be in [{BOOST_DIRECTIVE_MIN}, {BOOST_DIRECTIVE_MAX}], got {v}"
            ),
        ));
    }
    Ok(())
}

impl serde::Serialize for CandidateSignals {
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

impl<'de> serde::Deserialize<'de> for CandidateSignals {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = CandidateSignals;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CandidateSignals map with five f32 fields")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<CandidateSignals, M::Error> {
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
                Ok(CandidateSignals {
                    bm25,
                    path_prior,
                    symbol_boost,
                    recency,
                    boost_directive,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateSignals, validate_signals};
    use crate::errors::{RankerErrorCode, SignalKind};

    fn assert_signal_err(sig: CandidateSignals, expect: SignalKind) {
        match validate_signals(&sig) {
            Ok(()) => assert!(false, "expected RANK_INVALID_SIGNAL[{expect}]"),
            Err(e) => {
                assert_eq!(e.code, RankerErrorCode::RankInvalidSignal);
                assert_eq!(e.signal, Some(expect));
            }
        }
    }

    #[test]
    fn identity_validates() {
        match validate_signals(&CandidateSignals::identity()) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn populated_signal_set_validates() {
        let sig = CandidateSignals {
            bm25: 0.6,
            path_prior: 0.2,
            symbol_boost: 0.4,
            recency: 0.8,
            boost_directive: 1.5,
        };
        match validate_signals(&sig) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn nan_bm25_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.bm25 = f32::NAN;
        assert_signal_err(s, SignalKind::Bm25);
    }

    #[test]
    fn inf_path_prior_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.path_prior = f32::INFINITY;
        assert_signal_err(s, SignalKind::PathPrior);
    }

    #[test]
    fn nan_symbol_boost_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.symbol_boost = f32::NAN;
        assert_signal_err(s, SignalKind::SymbolBoost);
    }

    #[test]
    fn negative_recency_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.recency = -0.001;
        assert_signal_err(s, SignalKind::Recency);
    }

    #[test]
    fn above_envelope_recency_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.recency = 1.1;
        assert_signal_err(s, SignalKind::Recency);
    }

    #[test]
    fn nan_boost_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.boost_directive = f32::NAN;
        assert_signal_err(s, SignalKind::BoostDirective);
    }

    #[test]
    fn boost_below_min_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.boost_directive = 0.0;
        assert_signal_err(s, SignalKind::BoostDirective);
    }

    #[test]
    fn boost_above_max_is_invalid() {
        let mut s = CandidateSignals::identity();
        s.boost_directive = 10.0;
        assert_signal_err(s, SignalKind::BoostDirective);
    }

    #[test]
    fn boost_at_endpoints_is_valid() {
        let mut s = CandidateSignals::identity();
        s.boost_directive = 0.125;
        if let Err(e) = validate_signals(&s) {
            assert!(false, "{e}");
        }
        s.boost_directive = 8.0;
        if let Err(e) = validate_signals(&s) {
            assert!(false, "{e}");
        }
    }

    #[test]
    fn serde_ciborium_roundtrip() {
        let s = CandidateSignals {
            bm25: 0.6,
            path_prior: 0.2,
            symbol_boost: 0.4,
            recency: 0.8,
            boost_directive: 1.5,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&s, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<CandidateSignals, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, s),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
