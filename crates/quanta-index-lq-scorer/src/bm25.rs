//! BM25 parameter carrier.
//!
//! `Bm25Params` pins the `(k1, b)` pair that defines the BM25 saturation +
//! length-normalization curves for a generation. The defaults match the
//! industry-standard Tantivy / Sourcegraph posture (`k1 = 1.2`, `b = 0.75`).
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{ScorerError, ScorerErrorCode};

/// BM25 saturation + length-normalization parameters.
///
/// Fields are private behind accessors so the invariants checked by
/// [`Bm25Params::new`] cannot be bypassed; literal construction is reserved
/// for [`Bm25Params::DEFAULTS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bm25Params {
    k1: f32,
    b: f32,
}

impl Bm25Params {
    /// Industry-standard BM25 default `(k1 = 1.2, b = 0.75)`.
    pub const DEFAULTS: Self = Self { k1: 1.2, b: 0.75 };

    /// Construct + validate a `(k1, b)` pair.
    ///
    /// Rejects: any non-finite input, `k1 <= 0.0`, `k1 > 5.0`, `b` outside
    /// `[0.0, 1.0]` with [`ScorerErrorCode::InvalidBm25Param`].
    pub fn new(k1: f32, b: f32) -> Result<Self, ScorerError> {
        if !k1.is_finite() || !b.is_finite() {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidBm25Param,
                "BM25 params must be finite (not NaN/inf)",
            ));
        }
        // `k1 > 0.0 && k1 <= 5.0` — clippy's manual_range_contains check
        // wants `(0.0..=5.0).contains(&k1)` but `k1` must be strictly > 0,
        // not >= 0, so we write the bounded check explicitly with the
        // ranged-form for the `b` parameter only.
        if !k1.is_sign_positive() || k1 == 0.0 || k1 > 5.0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidBm25Param,
                "BM25 `k1` must be in (0.0, 5.0]",
            ));
        }
        if !(0.0..=1.0).contains(&b) {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidBm25Param,
                "BM25 `b` must be in [0.0, 1.0]",
            ));
        }
        Ok(Self { k1, b })
    }

    #[must_use]
    pub const fn k1(self) -> f32 {
        self.k1
    }

    #[must_use]
    pub const fn b(self) -> f32 {
        self.b
    }
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

impl fmt::Display for Bm25Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bm25Params(k1={}, b={})", self.k1, self.b)
    }
}

impl serde::Serialize for Bm25Params {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("k1", &self.k1)?;
        m.serialize_entry("b", &self.b)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for Bm25Params {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = Bm25Params;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Bm25Params map with `k1` and `b` f32 fields")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<Bm25Params, M::Error> {
                let mut k1: Option<f32> = None;
                let mut b: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "k1" => {
                            if k1.is_some() {
                                return Err(serde::de::Error::duplicate_field("k1"));
                            }
                            k1 = Some(map.next_value()?);
                        }
                        "b" => {
                            if b.is_some() {
                                return Err(serde::de::Error::duplicate_field("b"));
                            }
                            b = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["k1", "b"],
                            ));
                        }
                    }
                }
                let k1v = k1.ok_or_else(|| serde::de::Error::missing_field("k1"))?;
                let bv = b.ok_or_else(|| serde::de::Error::missing_field("b"))?;
                Bm25Params::new(k1v, bv).map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::Bm25Params;
    use crate::errors::ScorerErrorCode;

    fn unwrap_or_fail(r: Result<Bm25Params, crate::errors::ScorerError>) -> Bm25Params {
        match r {
            Ok(v) => v,
            Err(e) => unreachable_fail(&format!("{e}")),
        }
    }

    fn unreachable_fail(msg: &str) -> ! {
        assert!(false, "{msg}");
        // assert!(false) already terminates; this never executes.
        std::process::abort();
    }

    fn assert_invalid(r: Result<Bm25Params, crate::errors::ScorerError>) {
        match r {
            Ok(_) => assert!(false, "expected InvalidBm25Param"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::InvalidBm25Param),
        }
    }

    #[test]
    fn defaults_round_trip_via_new() {
        let p = Bm25Params::DEFAULTS;
        let got = unwrap_or_fail(Bm25Params::new(p.k1(), p.b()));
        assert!((got.k1() - 1.2).abs() < f32::EPSILON);
        assert!((got.b() - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_zero_k1() {
        assert_invalid(Bm25Params::new(0.0, 0.75));
    }

    #[test]
    fn rejects_negative_k1() {
        assert_invalid(Bm25Params::new(-0.1, 0.75));
    }

    #[test]
    fn rejects_k1_above_ceiling() {
        assert_invalid(Bm25Params::new(5.001, 0.75));
    }

    #[test]
    fn accepts_k1_at_ceiling() {
        let p = unwrap_or_fail(Bm25Params::new(5.0, 0.75));
        assert!((p.k1() - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_b_below_floor() {
        assert_invalid(Bm25Params::new(1.2, -0.01));
    }

    #[test]
    fn rejects_b_above_ceiling() {
        assert_invalid(Bm25Params::new(1.2, 1.01));
    }

    #[test]
    fn accepts_b_at_endpoints() {
        let p0 = unwrap_or_fail(Bm25Params::new(1.2, 0.0));
        assert!(p0.b().abs() < f32::EPSILON);
        let p1 = unwrap_or_fail(Bm25Params::new(1.2, 1.0));
        assert!((p1.b() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_nan_inputs() {
        assert_invalid(Bm25Params::new(f32::NAN, 0.75));
        assert_invalid(Bm25Params::new(1.2, f32::NAN));
    }

    #[test]
    fn rejects_infinite_inputs() {
        assert_invalid(Bm25Params::new(f32::INFINITY, 0.75));
    }

    #[test]
    fn serde_ciborium_roundtrip() {
        let p = unwrap_or_fail(Bm25Params::new(1.5, 0.6));
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&p, &mut buf) {
            Ok(()) => {}
            Err(e) => unreachable_fail(&format!("serialize: {e}")),
        }
        match ciborium::de::from_reader::<Bm25Params, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, p),
            Err(e) => unreachable_fail(&format!("deserialize: {e}")),
        }
    }
}
