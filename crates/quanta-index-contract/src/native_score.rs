//! Finite native query scores shared by wire DTOs and read adapters.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiniteScoreError;

impl fmt::Display for FiniteScoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native query score must be finite")
    }
}

impl std::error::Error for FiniteScoreError {}

/// A finite BM25 or cosine score. Negative zero is canonicalized to positive
/// zero so equality agrees across JSON/CBOR round trips and implementations.
#[derive(Clone, Copy, Debug)]
pub struct FiniteScore(f64);

impl FiniteScore {
    pub fn new(value: f64) -> Result<Self, FiniteScoreError> {
        if !value.is_finite() {
            return Err(FiniteScoreError);
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for FiniteScore {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FiniteScore {}

impl Serialize for FiniteScore {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for FiniteScore {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::FiniteScore;

    #[test]
    fn json_scores_preserve_decimal_edge_values() -> Result<(), Box<dyn std::error::Error>> {
        for value in [
            0.395_562_849_621_198_75,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
        ] {
            let score = FiniteScore::new(value)?;
            let json = serde_json::to_vec(&score)?;
            let restored: FiniteScore = serde_json::from_slice(&json)?;
            if restored != score {
                return Err(format!("JSON score changed: {json:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn finite_scores_roundtrip_and_nonfinite_cbor_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        for value in [-0.0, 0.0, -1.0, 1.234_567_890_123] {
            let score = FiniteScore::new(value)?;
            let json = serde_json::to_vec(&score)?;
            let restored: FiniteScore = serde_json::from_slice(&json)?;
            if restored != score {
                return Err("JSON score changed during roundtrip".into());
            }
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&score, &mut cbor)?;
            let restored: FiniteScore = ciborium::de::from_reader(cbor.as_slice())?;
            if restored != score {
                return Err("CBOR score changed during roundtrip".into());
            }
        }
        if FiniteScore::new(-0.0)? != FiniteScore::new(0.0)? {
            return Err("signed zero scores must normalize equally".into());
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            if FiniteScore::new(value).is_ok() {
                return Err("nonfinite score was accepted".into());
            }
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&value, &mut cbor)?;
            if ciborium::de::from_reader::<FiniteScore, _>(cbor.as_slice()).is_ok() {
                return Err("nonfinite CBOR score was accepted".into());
            }
        }
        Ok(())
    }
}
