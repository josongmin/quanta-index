//! The relevance score of one history row (QI-BB-023 follow-up #1).
//!
//! Under the history route's `relevance` order every row carries the
//! BM25 score the epoch's text index emitted for it, and the relevance
//! total order leads with it. A score is always finite: the engine never
//! emits `NaN` or an infinity, and a wire value that is either does not
//! decode, so the order the score leads is total and a cursor carrying it
//! positions exactly one place in the walk.

use core::cmp::Ordering;
use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A finite relevance score.
///
/// Two scores compare by value; equality is exact, which is what a keyset
/// continuation needs: the cursor carries the last row's score bit for bit
/// and the next page starts strictly after it under the total order.
#[derive(Clone, Copy, Debug)]
pub struct HistoryScoreV1(f32);

/// Why a value is not a history score.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryScoreError {
    kind: HistoryScoreErrorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryScoreErrorKind {
    NotFinite,
}

impl fmt::Display for HistoryScoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            HistoryScoreErrorKind::NotFinite => {
                formatter.write_str("history score must be finite (not NaN or infinite)")
            }
        }
    }
}

impl std::error::Error for HistoryScoreError {}

impl HistoryScoreV1 {
    /// A score from an engine value; refused when the value is not finite.
    pub fn try_new(score: f32) -> Result<Self, HistoryScoreError> {
        if score.is_finite() {
            Ok(Self(score))
        } else {
            Err(HistoryScoreError {
                kind: HistoryScoreErrorKind::NotFinite,
            })
        }
    }

    #[must_use]
    pub const fn get(self) -> f32 {
        self.0
    }
}

impl PartialEq for HistoryScoreV1 {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HistoryScoreV1 {}

impl PartialOrd for HistoryScoreV1 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HistoryScoreV1 {
    /// Numeric order; total because both values are finite by construction.
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl fmt::Display for HistoryScoreV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for HistoryScoreV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f32(self.0)
    }
}

struct HistoryScoreV1Visitor;

impl de::Visitor<'_> for HistoryScoreV1Visitor {
    type Value = HistoryScoreV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a finite history score as a 32-bit float")
    }

    fn visit_f32<E>(self, value: f32) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        HistoryScoreV1::try_new(value).map_err(de::Error::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        // A JSON number arrives as f64; the wire value is an f32, so a
        // magnitude an f32 cannot hold is not a score either.
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "the narrowing is the point: an f64 wire number is checked to be a finite f32 after the cast"
        )]
        let narrowed = value as f32;
        if !value.is_finite() {
            return Err(de::Error::custom(HistoryScoreError {
                kind: HistoryScoreErrorKind::NotFinite,
            }));
        }
        HistoryScoreV1::try_new(narrowed).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for HistoryScoreV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_f32(HistoryScoreV1Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryScoreV1;

    #[test]
    fn a_score_round_trips_exactly_through_json() {
        let score = HistoryScoreV1::try_new(1.234_567_9_f32).expect("finite");
        let json = serde_json::to_value(score).expect("serializes");
        let decoded: HistoryScoreV1 = serde_json::from_value(json).expect("decodes");
        assert_eq!(decoded, score);
        assert_eq!(decoded.get().to_bits(), score.get().to_bits());
    }

    #[test]
    fn non_finite_values_are_refused_at_construction_and_on_the_wire() {
        for wrong in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                HistoryScoreV1::try_new(wrong).is_err(),
                "{wrong} is not a score"
            );
        }
        for wrong in [
            serde_json::json!("1.0"),
            serde_json::json!(null),
            serde_json::json!(1e300),
            serde_json::json!({ "score": 1.0 }),
        ] {
            assert!(
                serde_json::from_value::<HistoryScoreV1>(wrong.clone()).is_err(),
                "{wrong} must not decode as a score"
            );
        }
    }

    #[test]
    fn scores_order_by_value() {
        let low = HistoryScoreV1::try_new(0.5).expect("finite");
        let high = HistoryScoreV1::try_new(2.0).expect("finite");
        assert!(low < high);
        assert_eq!(low, HistoryScoreV1::try_new(0.5).expect("finite"));
    }
}
