//! The structural route's result order and keyset cursor (QI-BB-025 W4).
//!
//! A structural page holds one row per matched chunk, and the route
//! ranks nothing among them: the rows are ordered by `candidate_id`
//! ascending, byte-wise — the one key that is a pure function of the row
//! and total over the match set. Evaluation must still materialize the
//! whole match set (a boolean tree of `match { ... }` leaves is decided
//! by set algebra over every leaf's matches, so no leaf can be cut
//! early), which is why the window's count is exact; the page is then
//! selected after the cursor with a bounded collector, so the response is
//! proportional to the page, not to the match set.
//!
//! A [`StructuralCursorV1`] names the last row a page returned under that
//! order; the next page holds the rows strictly after it. It is a
//! boundary, not a lookup: a key that names no row still positions the
//! walk correctly. The cursor names the structural authority epoch the
//! walk started in (QI-BB-020 W2): the next page is evaluated against
//! that epoch's snapshot — the pinned universe, every parse-tree leaf,
//! every symbol projection — so an ingest between two pages neither
//! repeats nor skips a row. A continuation whose epoch the plane no
//! longer retains is refused typed (`AUX_EPOCH_EXPIRED`), never served
//! from a newer epoch.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::query::AuxEpochV1;

/// The position of one structural row under the candidate-id order, in
/// the read epoch the page walk started in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralCursorV1 {
    /// The row's candidate id; lower sorts first, byte-wise.
    pub candidate_id: String,
    /// The structural authority epoch the page that issued this cursor
    /// was cut from; the continuation is evaluated against exactly that
    /// epoch.
    pub aux_epoch: AuxEpochV1,
}

const STRUCTURAL_CURSOR_V1_FIELDS: &[&str] = &["candidate_id", "aux_epoch"];

impl Serialize for StructuralCursorV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralCursorV1", 2)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("aux_epoch", &self.aux_epoch)?;
        state.end()
    }
}

struct StructuralCursorV1Visitor;

impl<'de> Visitor<'de> for StructuralCursorV1Visitor {
    type Value = StructuralCursorV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralCursorV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate_id: Option<String> = None;
        let mut aux_epoch: Option<AuxEpochV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate_id" => {
                    if candidate_id.is_some() {
                        return Err(de::Error::duplicate_field("candidate_id"));
                    }
                    candidate_id = Some(map.next_value()?);
                }
                "aux_epoch" => {
                    if aux_epoch.is_some() {
                        return Err(de::Error::duplicate_field("aux_epoch"));
                    }
                    aux_epoch = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, STRUCTURAL_CURSOR_V1_FIELDS));
                }
            }
        }
        Ok(StructuralCursorV1 {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            aux_epoch: aux_epoch.ok_or_else(|| de::Error::missing_field("aux_epoch"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralCursorV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralCursorV1",
            STRUCTURAL_CURSOR_V1_FIELDS,
            StructuralCursorV1Visitor,
        )
    }
}
