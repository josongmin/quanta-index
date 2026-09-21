//! The history route's keyset cursor (QI-BB-023).
//!
//! History results are ordered by the request's [`HistoryOrderV1`], a
//! total order every page and every restart agree on:
//!
//! - `recency`: `committer_time_ms` descending, then `sha` ascending, then
//!   — for diffs — `file_path` ascending;
//! - `relevance`: `score` descending, then the recency key.
//!
//! A [`HistoryCursor`] names the last element a page returned under that
//! order; the next page holds the elements strictly after it. The cursor
//! is a keyset, not an offset: it carries the order it was issued under
//! (with the row's score when that order is relevance), so a cursor of one
//! order cannot position a walk of the other, and it names the read epoch
//! the walk started in (QI-BB-020 W2): the next page is cut from that
//! epoch's snapshot and — under relevance — scored by that epoch's text
//! index, so an ingest between two pages neither skips nor repeats an
//! element and never changes a score mid-walk. A continuation whose epoch
//! the plane no longer retains is refused typed (`AUX_EPOCH_EXPIRED`),
//! never served from a newer epoch.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::CommitSha;
use crate::query::{AuxEpochV1, HistoryOrderV1};
use crate::results::HistoryScoreV1;

/// The order a cursor was issued under, with the component that leads the
/// key under that order.
///
/// A recency key is led by the committer time the cursor already carries;
/// a relevance key is led by the row's score, which only exists under
/// that order — so the type, not a convention, keeps a recency cursor
/// from carrying a score or a relevance cursor from lacking one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryCursorOrderV1 {
    Recency,
    Relevance { score: HistoryScoreV1 },
}

impl HistoryCursorOrderV1 {
    /// The order this cursor positions.
    #[must_use]
    pub const fn order(self) -> HistoryOrderV1 {
        match self {
            Self::Recency => HistoryOrderV1::Recency,
            Self::Relevance { .. } => HistoryOrderV1::Relevance,
        }
    }
}

/// The position of one history element under one order, in the read
/// epoch the page walk started in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryCursor {
    /// The order the cursor was issued under and, under relevance, the
    /// row's score, which leads the key.
    pub order: HistoryCursorOrderV1,
    /// The commit's committer time; higher sorts first.
    pub committer_time_ms: u64,
    /// The commit; lower sorts first among equal keys so far.
    pub sha: CommitSha,
    /// For diff pages, the hunk's path; lower sorts first among equal
    /// commits. Absent on commit pages.
    pub file_path: Option<String>,
    /// The history authority epoch the page that issued this cursor was
    /// cut from; the continuation is served from exactly that epoch.
    pub aux_epoch: AuxEpochV1,
}

const HISTORY_CURSOR_FIELDS: &[&str] = &[
    "order",
    "score",
    "committer_time_ms",
    "sha",
    "file_path",
    "aux_epoch",
];

impl Serialize for HistoryCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let score = match self.order {
            HistoryCursorOrderV1::Recency => None,
            HistoryCursorOrderV1::Relevance { score } => Some(score),
        };
        let field_count = 4_usize
            .saturating_add(usize::from(score.is_some()))
            .saturating_add(usize::from(self.file_path.is_some()));
        let mut state = serializer.serialize_struct("HistoryCursor", field_count)?;
        state.serialize_field("order", &self.order.order())?;
        if let Some(score) = score {
            state.serialize_field("score", &score)?;
        }
        state.serialize_field("committer_time_ms", &self.committer_time_ms)?;
        state.serialize_field("sha", &self.sha)?;
        if let Some(file_path) = &self.file_path {
            state.serialize_field("file_path", file_path)?;
        }
        state.serialize_field("aux_epoch", &self.aux_epoch)?;
        state.end()
    }
}

struct HistoryCursorVisitor;

impl<'de> Visitor<'de> for HistoryCursorVisitor {
    type Value = HistoryCursor;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryCursor map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut order: Option<HistoryOrderV1> = None;
        let mut score: Option<HistoryScoreV1> = None;
        let mut committer_time_ms: Option<u64> = None;
        let mut sha: Option<CommitSha> = None;
        let mut file_path: Option<String> = None;
        let mut file_path_seen = false;
        let mut aux_epoch: Option<AuxEpochV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "order" => {
                    if order.is_some() {
                        return Err(de::Error::duplicate_field("order"));
                    }
                    order = Some(map.next_value()?);
                }
                "score" => {
                    if score.is_some() {
                        return Err(de::Error::duplicate_field("score"));
                    }
                    score = Some(map.next_value()?);
                }
                "committer_time_ms" => {
                    if committer_time_ms.is_some() {
                        return Err(de::Error::duplicate_field("committer_time_ms"));
                    }
                    committer_time_ms = Some(map.next_value()?);
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(de::Error::duplicate_field("sha"));
                    }
                    sha = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path_seen {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path_seen = true;
                    file_path = map.next_value()?;
                }
                "aux_epoch" => {
                    if aux_epoch.is_some() {
                        return Err(de::Error::duplicate_field("aux_epoch"));
                    }
                    aux_epoch = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_CURSOR_FIELDS)),
            }
        }
        let order = match (order.ok_or_else(|| de::Error::missing_field("order"))?, score) {
            (HistoryOrderV1::Recency, None) => HistoryCursorOrderV1::Recency,
            (HistoryOrderV1::Recency, Some(_)) => {
                return Err(de::Error::custom("a recency history cursor must not carry a score"));
            }
            (HistoryOrderV1::Relevance, Some(score)) => HistoryCursorOrderV1::Relevance { score },
            (HistoryOrderV1::Relevance, None) => {
                return Err(de::Error::missing_field("score"));
            }
        };
        Ok(HistoryCursor {
            order,
            committer_time_ms: committer_time_ms
                .ok_or_else(|| de::Error::missing_field("committer_time_ms"))?,
            sha: sha.ok_or_else(|| de::Error::missing_field("sha"))?,
            file_path,
            aux_epoch: aux_epoch.ok_or_else(|| de::Error::missing_field("aux_epoch"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryCursor",
            HISTORY_CURSOR_FIELDS,
            HistoryCursorVisitor,
        )
    }
}
