use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// Public structural query-response binding DTO.
///
/// Internal authority/matcher layers should project into this shape only at
/// the search-plane response boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralBinding {
    pub metavariable: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
}

const STRUCTURAL_BINDING_FIELDS: &[&str] = &[
    "metavariable",
    "start_byte",
    "end_byte",
    "start_line",
    "end_line",
];

impl Serialize for StructuralBinding {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralBinding", 5)?;
        state.serialize_field("metavariable", &self.metavariable)?;
        state.serialize_field("start_byte", &self.start_byte)?;
        state.serialize_field("end_byte", &self.end_byte)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.end()
    }
}

struct StructuralBindingVisitor;

impl<'de> Visitor<'de> for StructuralBindingVisitor {
    type Value = StructuralBinding;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralBinding map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut metavariable: Option<String> = None;
        let mut start_byte: Option<u32> = None;
        let mut end_byte: Option<u32> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "metavariable" => metavariable = Some(map.next_value()?),
                "start_byte" => start_byte = Some(map.next_value()?),
                "end_byte" => end_byte = Some(map.next_value()?),
                "start_line" => start_line = Some(map.next_value()?),
                "end_line" => end_line = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, STRUCTURAL_BINDING_FIELDS)),
            }
        }
        Ok(StructuralBinding {
            metavariable: metavariable.ok_or_else(|| de::Error::missing_field("metavariable"))?,
            start_byte: start_byte.ok_or_else(|| de::Error::missing_field("start_byte"))?,
            end_byte: end_byte.ok_or_else(|| de::Error::missing_field("end_byte"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralBinding",
            STRUCTURAL_BINDING_FIELDS,
            StructuralBindingVisitor,
        )
    }
}

/// Public structural query-response candidate DTO.
///
/// This wire shape is intentionally separate from the internal authority-side
/// structural match carriers used inside `quanta-index-core` and `searchd`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralCandidate {
    pub candidate_id: String,
    pub bindings: Vec<StructuralBinding>,
}

const STRUCTURAL_CANDIDATE_FIELDS: &[&str] = &["candidate_id", "bindings"];

impl Serialize for StructuralCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralCandidate", 2)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("bindings", &self.bindings)?;
        state.end()
    }
}

struct StructuralCandidateVisitor;

impl<'de> Visitor<'de> for StructuralCandidateVisitor {
    type Value = StructuralCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate_id: Option<String> = None;
        let mut bindings: Option<Vec<StructuralBinding>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate_id" => candidate_id = Some(map.next_value()?),
                "bindings" => bindings = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, STRUCTURAL_CANDIDATE_FIELDS)),
            }
        }
        Ok(StructuralCandidate {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            bindings: bindings.ok_or_else(|| de::Error::missing_field("bindings"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralCandidate",
            STRUCTURAL_CANDIDATE_FIELDS,
            StructuralCandidateVisitor,
        )
    }
}
