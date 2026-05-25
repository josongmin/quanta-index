use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use quanta_index_contract_base::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SearchPlaneTrackKind {
    Lexical,
    Semantic,
}

impl SearchPlaneTrackKind {
    const VARIANTS: &'static [&'static str] = &["Lexical", "Semantic"];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Lexical => "Lexical",
            Self::Semantic => "Semantic",
        }
    }
}

impl Serialize for SearchPlaneTrackKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SearchPlaneTrackKindVisitor;

impl Visitor<'_> for SearchPlaneTrackKindVisitor {
    type Value = SearchPlaneTrackKind;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneTrackKind string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "Lexical" => Ok(SearchPlaneTrackKind::Lexical),
            "Semantic" => Ok(SearchPlaneTrackKind::Semantic),
            other => Err(de::Error::unknown_variant(
                other,
                SearchPlaneTrackKind::VARIANTS,
            )),
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SearchPlaneTrackKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SearchPlaneTrackKindVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneActivateGenerationRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub tracks: Vec<SearchPlaneTrackKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneActivationAck {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub tracks: Vec<SearchPlaneTrackKind>,
}

const SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "manifest_digest",
    "tracks",
];

fn serialize_activation_like<S>(
    name: &'static str,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    manifest_generation: &ManifestGeneration,
    manifest_digest: &str,
    tracks: &[SearchPlaneTrackKind],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct(name, 5)?;
    state.serialize_field("repo_id", repo_id)?;
    state.serialize_field("revision_id", revision_id)?;
    state.serialize_field("manifest_generation", manifest_generation)?;
    state.serialize_field("manifest_digest", manifest_digest)?;
    state.serialize_field("tracks", tracks)?;
    state.end()
}

impl Serialize for SearchPlaneActivateGenerationRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_activation_like(
            "SearchPlaneActivateGenerationRequest",
            &self.repo_id,
            &self.revision_id,
            &self.manifest_generation,
            &self.manifest_digest,
            &self.tracks,
            serializer,
        )
    }
}

impl Serialize for SearchPlaneActivationAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_activation_like(
            "SearchPlaneActivationAck",
            &self.repo_id,
            &self.revision_id,
            &self.manifest_generation,
            &self.manifest_digest,
            &self.tracks,
            serializer,
        )
    }
}

struct SearchPlaneActivateGenerationRequestVisitor;
struct SearchPlaneActivationAckVisitor;

type ActivationLikeTuple = (
    RepoId,
    RevisionId,
    ManifestGeneration,
    String,
    Vec<SearchPlaneTrackKind>,
);

fn deserialize_activation_like<'de, A>(mut map: A) -> Result<ActivationLikeTuple, A::Error>
where
    A: MapAccess<'de>,
{
    let mut repo_id: Option<RepoId> = None;
    let mut revision_id: Option<RevisionId> = None;
    let mut manifest_generation: Option<ManifestGeneration> = None;
    let mut manifest_digest: Option<String> = None;
    let mut tracks: Option<Vec<SearchPlaneTrackKind>> = None;
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
            "manifest_digest" => {
                if manifest_digest.is_some() {
                    return Err(de::Error::duplicate_field("manifest_digest"));
                }
                manifest_digest = Some(map.next_value()?);
            }
            "tracks" => {
                if tracks.is_some() {
                    return Err(de::Error::duplicate_field("tracks"));
                }
                tracks = Some(map.next_value()?);
            }
            other => {
                return Err(de::Error::unknown_field(
                    other,
                    SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
                ));
            }
        }
    }
    Ok((
        repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
        revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
        manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
        manifest_digest.ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
        tracks.ok_or_else(|| de::Error::missing_field("tracks"))?,
    ))
}

impl<'de> Visitor<'de> for SearchPlaneActivateGenerationRequestVisitor {
    type Value = SearchPlaneActivateGenerationRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneActivateGenerationRequest map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (repo_id, revision_id, manifest_generation, manifest_digest, tracks) =
            deserialize_activation_like(map)?;
        Ok(SearchPlaneActivateGenerationRequest {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
            tracks,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneActivateGenerationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneActivateGenerationRequest",
            SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
            SearchPlaneActivateGenerationRequestVisitor,
        )
    }
}

impl<'de> Visitor<'de> for SearchPlaneActivationAckVisitor {
    type Value = SearchPlaneActivationAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneActivationAck map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (repo_id, revision_id, manifest_generation, manifest_digest, tracks) =
            deserialize_activation_like(map)?;
        Ok(SearchPlaneActivationAck {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
            tracks,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneActivationAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneActivationAck",
            SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
            SearchPlaneActivationAckVisitor,
        )
    }
}
