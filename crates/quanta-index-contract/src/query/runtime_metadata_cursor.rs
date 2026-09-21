//! The runtime-metadata route's result order and keyset cursor (QI-BB-025
//! W4).
//!
//! The route ranks nothing: every row it returns is a projection of one
//! chunk the runtime authority names, carried with the constant score
//! `1.0`. Its pages are therefore ordered by the one key that is a pure
//! function of the row and total over the chunk universe — `candidate_id`
//! ascending, byte-wise — which is also the order the chunk authority is
//! keyed in, so a walk seeks past its cursor instead of rescanning.
//!
//! A [`RuntimeMetadataCursorV1`] names the last row a page returned under
//! that order; the next page holds the rows strictly after it. It is a
//! boundary, not a lookup: a key that names no row still positions the
//! walk correctly. The route joins two auxiliary authorities — the
//! runtime authority for its predicates and the structural authority for
//! the chunk universe — and a continuation must be cut from the same
//! snapshot of both, so the cursor names both epochs (QI-BB-020 W2): an
//! ingest between two pages neither repeats, skips nor conjures a row.
//! A continuation whose epoch the plane no longer retains is refused
//! typed (`AUX_EPOCH_EXPIRED`), never served from a newer epoch.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::query::AuxEpochV1;

/// The position of one runtime-metadata row under the candidate-id
/// order, in the read epochs the page walk started in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMetadataCursorV1 {
    /// The row's candidate id; lower sorts first, byte-wise.
    pub candidate_id: String,
    /// The runtime-metadata authority epoch the page that issued this
    /// cursor was cut from; the continuation's predicates are evaluated
    /// against exactly that snapshot.
    pub aux_epoch: AuxEpochV1,
    /// The structural authority epoch the page's chunk universe was
    /// joined from; the continuation reads exactly that snapshot of it.
    pub universe_epoch: AuxEpochV1,
}

const RUNTIME_METADATA_CURSOR_V1_FIELDS: &[&str] = &["candidate_id", "aux_epoch", "universe_epoch"];

impl Serialize for RuntimeMetadataCursorV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeMetadataCursorV1", 3)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("aux_epoch", &self.aux_epoch)?;
        state.serialize_field("universe_epoch", &self.universe_epoch)?;
        state.end()
    }
}

struct RuntimeMetadataCursorV1Visitor;

impl<'de> Visitor<'de> for RuntimeMetadataCursorV1Visitor {
    type Value = RuntimeMetadataCursorV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeMetadataCursorV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate_id: Option<String> = None;
        let mut aux_epoch: Option<AuxEpochV1> = None;
        let mut universe_epoch: Option<AuxEpochV1> = None;
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
                "universe_epoch" => {
                    if universe_epoch.is_some() {
                        return Err(de::Error::duplicate_field("universe_epoch"));
                    }
                    universe_epoch = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, RUNTIME_METADATA_CURSOR_V1_FIELDS));
                }
            }
        }
        Ok(RuntimeMetadataCursorV1 {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            aux_epoch: aux_epoch.ok_or_else(|| de::Error::missing_field("aux_epoch"))?,
            universe_epoch: universe_epoch
                .ok_or_else(|| de::Error::missing_field("universe_epoch"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeMetadataCursorV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeMetadataCursorV1",
            RUNTIME_METADATA_CURSOR_V1_FIELDS,
            RuntimeMetadataCursorV1Visitor,
        )
    }
}
