//! The history route's result orders (QI-BB-023 follow-up #1).
//!
//! A history request says which total order it wants its pages in; there
//! is no default. Both orders are keyset-pageable and both end in the
//! commit sha (then the diff path) so that every page and every restart
//! agree on one sequence:
//!
//! - `recency`: `committer_time_ms` descending, then `sha` ascending, then
//!   — for diffs — `file_path` ascending. The text expression is a filter.
//! - `relevance`: the row's BM25 score over the generation's indexed
//!   commit-message / diff text descending, then the recency key. Every
//!   row carries its score; the text expression is what is scored.
//!
//! # One row predicate (보완 #3)
//!
//! The order chooses *which* matching rows come first, never *what*
//! matches. Under either order a row is on the result set exactly when it
//! passes the query's filters and its text expression, evaluated with the
//! one text normalizer: a keyword or phrase leaf is a whole-token
//! sequence after NFC under the query's case mode (`case:no`, the
//! default, folds per character; `case:yes` does not fold), a raw string
//! is an NFC substring under the same mode, and a filter pattern
//! (`author:`, `committer:`, `message:`, `file:`, `diff.*:`) is an NFC
//! substring under the same mode. The same query therefore reports the
//! same exact `window` total and the same row set under both orders.
//!
//! What differs is scorability. `relevance` scores keyword and phrase
//! leaves; a raw string is admitted only where a scored clause bounds it
//! (a conjunct of a keyword, or negated), so every row on the page is
//! reached by a scored leaf. A query a row could satisfy through a raw
//! string alone (a raw string as the whole expression or as an
//! alternative), an empty expression, or a negation with no positive
//! clause beside it is refused typed under `relevance`
//! (`HISTORY_TEXT_QUERY_UNSCORABLE`) and served as a filter under
//! `recency`. A keyword or phrase literal with no token is refused typed
//! under both orders (`LEX_TEXT_QUERY_NO_TOKENS`), before any row is
//! read.
//!
//! A `relevance` score is a function of the epoch's live rows only: a
//! row superseded by a later upsert never counts in the BM25 statistics,
//! so the same rows rank the same whatever sequence of ingests produced
//! them, and a restart serves the same pages and cursors.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Which total order a history page walk uses.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HistoryOrderV1 {
    Recency,
    Relevance,
}

impl HistoryOrderV1 {
    /// Every order, in wire-code order.
    pub const ALL: [Self; 2] = [Self::Recency, Self::Relevance];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Recency => "recency",
            Self::Relevance => "relevance",
        }
    }

    /// The order a wire code names, if any.
    #[must_use]
    pub fn from_code_str(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|order| order.as_code_str() == code)
    }
}

impl fmt::Display for HistoryOrderV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for HistoryOrderV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct HistoryOrderV1Visitor;

impl de::Visitor<'_> for HistoryOrderV1Visitor {
    type Value = HistoryOrderV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a history order code (`recency` or `relevance`)")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        HistoryOrderV1::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["recency", "relevance"]))
    }
}

impl<'de> Deserialize<'de> for HistoryOrderV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(HistoryOrderV1Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryOrderV1;

    #[test]
    fn orders_round_trip_as_their_codes_and_refuse_anything_else() {
        for order in HistoryOrderV1::ALL {
            let json = serde_json::to_value(order).expect("serializes");
            assert_eq!(json, serde_json::json!(order.as_code_str()));
            let decoded: HistoryOrderV1 = serde_json::from_value(json).expect("decodes");
            assert_eq!(decoded, order);
        }
        for wrong in [
            serde_json::json!("newest"),
            serde_json::json!("Recency"),
            serde_json::json!(0),
            serde_json::json!(null),
        ] {
            assert!(
                serde_json::from_value::<HistoryOrderV1>(wrong.clone()).is_err(),
                "{wrong} must not decode as an order"
            );
        }
    }
}
