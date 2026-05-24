use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{BundleArtifactRef, PublishedGenerationSet, PublishedSearchBundleManifest};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleInspectRequest {
    pub generation: PublishedGenerationSet,
}

const PUBLISHED_SEARCH_BUNDLE_INSPECT_REQUEST_FIELDS: &[&str] = &["generation"];

impl Serialize for PublishedSearchBundleInspectRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PublishedSearchBundleInspectRequest", 1)?;
        state.serialize_field("generation", &self.generation)?;
        state.end()
    }
}

struct PublishedSearchBundleInspectRequestVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundleInspectRequestVisitor {
    type Value = PublishedSearchBundleInspectRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundleInspectRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<PublishedGenerationSet> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_BUNDLE_INSPECT_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        Ok(PublishedSearchBundleInspectRequest { generation })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundleInspectRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundleInspectRequest",
            PUBLISHED_SEARCH_BUNDLE_INSPECT_REQUEST_FIELDS,
            PublishedSearchBundleInspectRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleInspectResponse {
    pub manifest: PublishedSearchBundleManifest,
    pub mode: String,
    pub artifacts: Vec<BundleArtifactRef>,
    pub state: String,
}

const PUBLISHED_SEARCH_BUNDLE_INSPECT_RESPONSE_FIELDS: &[&str] =
    &["manifest", "mode", "artifacts", "state"];

impl Serialize for PublishedSearchBundleInspectResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PublishedSearchBundleInspectResponse", 4)?;
        state.serialize_field("manifest", &self.manifest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("artifacts", &self.artifacts)?;
        state.serialize_field("state", &self.state)?;
        state.end()
    }
}

struct PublishedSearchBundleInspectResponseVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundleInspectResponseVisitor {
    type Value = PublishedSearchBundleInspectResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundleInspectResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut manifest: Option<PublishedSearchBundleManifest> = None;
        let mut mode: Option<String> = None;
        let mut artifacts: Option<Vec<BundleArtifactRef>> = None;
        let mut state_field: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "manifest" => {
                    if manifest.is_some() {
                        return Err(de::Error::duplicate_field("manifest"));
                    }
                    manifest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "artifacts" => {
                    if artifacts.is_some() {
                        return Err(de::Error::duplicate_field("artifacts"));
                    }
                    artifacts = Some(map.next_value()?);
                }
                "state" => {
                    if state_field.is_some() {
                        return Err(de::Error::duplicate_field("state"));
                    }
                    state_field = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_BUNDLE_INSPECT_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let manifest = manifest.ok_or_else(|| de::Error::missing_field("manifest"))?;
        let mode = mode.ok_or_else(|| de::Error::missing_field("mode"))?;
        let artifacts = artifacts.ok_or_else(|| de::Error::missing_field("artifacts"))?;
        let state_field = state_field.ok_or_else(|| de::Error::missing_field("state"))?;
        Ok(PublishedSearchBundleInspectResponse {
            manifest,
            mode,
            artifacts,
            state: state_field,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundleInspectResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundleInspectResponse",
            PUBLISHED_SEARCH_BUNDLE_INSPECT_RESPONSE_FIELDS,
            PublishedSearchBundleInspectResponseVisitor,
        )
    }
}
