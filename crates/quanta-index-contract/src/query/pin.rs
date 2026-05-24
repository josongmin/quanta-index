//! Generation pin for query requests / responses.
//!
//! In the channel architecture, a query targets exactly one joint generation across
//! lexical and semantic tracks. The joint key is `manifest_generation`, scoped by
//! `(repo_id, revision_id)`. Per-track generation IDs are channel-internal and do
//! not appear on the wire.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GenerationPin {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
}

impl GenerationPin {
    #[must_use]
    pub const fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        manifest_generation: ManifestGeneration,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            manifest_generation,
        }
    }
}

const GENERATION_PIN_FIELDS: &[&str] = &["repo_id", "revision_id", "manifest_generation"];

impl Serialize for GenerationPin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("GenerationPin", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.end()
    }
}

struct GenerationPinVisitor;

impl<'de> Visitor<'de> for GenerationPinVisitor {
    type Value = GenerationPin;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GenerationPin map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, GENERATION_PIN_FIELDS));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        Ok(GenerationPin {
            repo_id,
            revision_id,
            manifest_generation,
        })
    }
}

impl<'de> Deserialize<'de> for GenerationPin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "GenerationPin",
            GENERATION_PIN_FIELDS,
            GenerationPinVisitor,
        )
    }
}
