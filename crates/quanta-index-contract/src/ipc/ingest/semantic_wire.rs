//! Semantic ingest wire DTOs and normalization policy identifiers.

use super::super::semantic_source::{ClusterMembershipReplaceV1, SemanticSourceScopeKeyV1};
use super::{BatchIngestMode, SearchScopeKey, SearchScopeSurface};
use crate::semantic_kinds::SemanticCorpusKindV1;
use crate::{EmbeddingRecord, ManifestGeneration, RepoId, RevisionId};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EmbeddingNormalization {
    None,
    L2Unit,
}

const EMBEDDING_NORMALIZATION_VARIANTS: &[&str] = &["None", "L2Unit"];

impl Serialize for EmbeddingNormalization {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "None",
            Self::L2Unit => "L2Unit",
        })
    }
}

struct EmbeddingNormalizationVisitor;

impl Visitor<'_> for EmbeddingNormalizationVisitor {
    type Value = EmbeddingNormalization;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingNormalization tag")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "None" => Ok(EmbeddingNormalization::None),
            "L2Unit" => Ok(EmbeddingNormalization::L2Unit),
            other => Err(de::Error::unknown_variant(
                other,
                EMBEDDING_NORMALIZATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingNormalization {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EmbeddingNormalizationVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EmbeddingDistanceMetric {
    Cosine,
    Dot,
    Euclidean,
}

const EMBEDDING_DISTANCE_METRIC_VARIANTS: &[&str] = &["Cosine", "Dot", "Euclidean"];

impl Serialize for EmbeddingDistanceMetric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Cosine => "Cosine",
            Self::Dot => "Dot",
            Self::Euclidean => "Euclidean",
        })
    }
}

struct EmbeddingDistanceMetricVisitor;

impl Visitor<'_> for EmbeddingDistanceMetricVisitor {
    type Value = EmbeddingDistanceMetric;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingDistanceMetric tag")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "Cosine" => Ok(EmbeddingDistanceMetric::Cosine),
            "Dot" => Ok(EmbeddingDistanceMetric::Dot),
            "Euclidean" => Ok(EmbeddingDistanceMetric::Euclidean),
            other => Err(de::Error::unknown_variant(
                other,
                EMBEDDING_DISTANCE_METRIC_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingDistanceMetric {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EmbeddingDistanceMetricVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingModelContract {
    pub model_id: Box<str>,
    pub model_version: Option<Box<str>>,
    pub dimension: u32,
    pub normalization: EmbeddingNormalization,
    pub distance_metric: EmbeddingDistanceMetric,
    pub policy_digest: Box<str>,
    pub view_policy_digest: Option<Box<str>>,
}

const EMBEDDING_MODEL_CONTRACT_FIELDS: &[&str] = &[
    "model_id",
    "model_version",
    "dimension",
    "normalization",
    "distance_metric",
    "policy_digest",
    "view_policy_digest",
];

impl Serialize for EmbeddingModelContract {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("EmbeddingModelContract", 7)?;
        state.serialize_field("model_id", self.model_id.as_ref())?;
        state.serialize_field("model_version", &self.model_version.as_deref())?;
        state.serialize_field("dimension", &self.dimension)?;
        state.serialize_field("normalization", &self.normalization)?;
        state.serialize_field("distance_metric", &self.distance_metric)?;
        state.serialize_field("policy_digest", self.policy_digest.as_ref())?;
        state.serialize_field("view_policy_digest", &self.view_policy_digest.as_deref())?;
        state.end()
    }
}

struct EmbeddingModelContractVisitor;

impl<'de> Visitor<'de> for EmbeddingModelContractVisitor {
    type Value = EmbeddingModelContract;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingModelContract map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut model_id: Option<String> = None;
        let mut model_version: Option<Option<String>> = None;
        let mut dimension: Option<u32> = None;
        let mut normalization: Option<EmbeddingNormalization> = None;
        let mut distance_metric: Option<EmbeddingDistanceMetric> = None;
        let mut policy_digest: Option<String> = None;
        let mut view_policy_digest: Option<Option<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "model_id" => {
                    if model_id.is_some() {
                        return Err(de::Error::duplicate_field("model_id"));
                    }
                    model_id = Some(map.next_value()?);
                }
                "model_version" => {
                    if model_version.is_some() {
                        return Err(de::Error::duplicate_field("model_version"));
                    }
                    model_version = Some(map.next_value()?);
                }
                "dimension" => {
                    if dimension.is_some() {
                        return Err(de::Error::duplicate_field("dimension"));
                    }
                    dimension = Some(map.next_value()?);
                }
                "normalization" => {
                    if normalization.is_some() {
                        return Err(de::Error::duplicate_field("normalization"));
                    }
                    normalization = Some(map.next_value()?);
                }
                "distance_metric" => {
                    if distance_metric.is_some() {
                        return Err(de::Error::duplicate_field("distance_metric"));
                    }
                    distance_metric = Some(map.next_value()?);
                }
                "policy_digest" => {
                    if policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("policy_digest"));
                    }
                    policy_digest = Some(map.next_value()?);
                }
                "view_policy_digest" => {
                    if view_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("view_policy_digest"));
                    }
                    view_policy_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        EMBEDDING_MODEL_CONTRACT_FIELDS,
                    ));
                }
            }
        }
        let dimension = dimension.ok_or_else(|| de::Error::missing_field("dimension"))?;
        if dimension == 0 {
            return Err(de::Error::invalid_value(
                de::Unexpected::Unsigned(0),
                &"a non-zero embedding dimension",
            ));
        }
        Ok(EmbeddingModelContract {
            model_id: model_id
                .ok_or_else(|| de::Error::missing_field("model_id"))?
                .into_boxed_str(),
            model_version: model_version
                .ok_or_else(|| de::Error::missing_field("model_version"))?
                .map(String::into_boxed_str),
            dimension,
            normalization: normalization
                .ok_or_else(|| de::Error::missing_field("normalization"))?,
            distance_metric: distance_metric
                .ok_or_else(|| de::Error::missing_field("distance_metric"))?,
            policy_digest: policy_digest
                .ok_or_else(|| de::Error::missing_field("policy_digest"))?
                .into_boxed_str(),
            view_policy_digest: view_policy_digest
                .ok_or_else(|| de::Error::missing_field("view_policy_digest"))?
                .map(String::into_boxed_str),
        })
    }
}

impl<'de> Deserialize<'de> for EmbeddingModelContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "EmbeddingModelContract",
            EMBEDDING_MODEL_CONTRACT_FIELDS,
            EmbeddingModelContractVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub embeddings: Vec<EmbeddingRecord>,
    pub cluster_memberships: Vec<ClusterMembershipReplaceV1>,
}

const SEMANTIC_REPLACE_SCOPE_FIELDS: &[&str] =
    &["scope", "scope_digest", "embeddings", "cluster_memberships"];

impl Serialize for SemanticReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticReplaceScope", 4)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("embeddings", &self.embeddings)?;
        state.serialize_field("cluster_memberships", &self.cluster_memberships)?;
        state.end()
    }
}

struct SemanticReplaceScopeVisitor;

impl<'de> Visitor<'de> for SemanticReplaceScopeVisitor {
    type Value = SemanticReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticReplaceScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        let mut scope_digest: Option<String> = None;
        let mut embeddings: Option<Vec<EmbeddingRecord>> = None;
        let mut cluster_memberships: Option<Vec<ClusterMembershipReplaceV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
                }
                "embeddings" => {
                    if embeddings.is_some() {
                        return Err(de::Error::duplicate_field("embeddings"));
                    }
                    embeddings = Some(map.next_value()?);
                }
                "cluster_memberships" => {
                    if cluster_memberships.is_some() {
                        return Err(de::Error::duplicate_field("cluster_memberships"));
                    }
                    cluster_memberships = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            embeddings: embeddings.ok_or_else(|| de::Error::missing_field("embeddings"))?,
            cluster_memberships: cluster_memberships
                .ok_or_else(|| de::Error::missing_field("cluster_memberships"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticReplaceScope",
            SEMANTIC_REPLACE_SCOPE_FIELDS,
            SemanticReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTombstoneScope {
    pub semantic_scope: SemanticSourceScopeKeyV1,
}

const SEMANTIC_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["semantic_scope"];

impl Serialize for SemanticTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticTombstoneScope", 1)?;
        state.serialize_field("semantic_scope", &self.semantic_scope)?;
        state.end()
    }
}

struct SemanticTombstoneScopeVisitor;

impl<'de> Visitor<'de> for SemanticTombstoneScopeVisitor {
    type Value = SemanticTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut semantic_scope: Option<SemanticSourceScopeKeyV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "semantic_scope" => {
                    if semantic_scope.is_some() {
                        return Err(de::Error::duplicate_field("semantic_scope"));
                    }
                    semantic_scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        let semantic_scope =
            semantic_scope.ok_or_else(|| de::Error::missing_field("semantic_scope"))?;
        if semantic_scope.owner_id.is_empty() {
            return Err(de::Error::custom(
                "semantic tombstone owner_id must not be empty",
            ));
        }
        Ok(SemanticTombstoneScope { semantic_scope })
    }
}

impl<'de> Deserialize<'de> for SemanticTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticTombstoneScope",
            SEMANTIC_TOMBSTONE_SCOPE_FIELDS,
            SemanticTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub model_contract: EmbeddingModelContract,
    pub required_corpora: Vec<SemanticCorpusKindV1>,
    pub corpus_policy_digest: Option<String>,
    pub clear_surfaces: Vec<SearchScopeSurface>,
    pub replace_scopes: Vec<SemanticReplaceScope>,
    pub tombstone_scopes: Vec<SemanticTombstoneScope>,
    pub seal: bool,
}

const SEMANTIC_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "model_contract",
    "required_corpora",
    "corpus_policy_digest",
    "clear_surfaces",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for SemanticIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticIngestBatch", 14)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("model_contract", &self.model_contract)?;
        state.serialize_field("required_corpora", &self.required_corpora)?;
        state.serialize_field("corpus_policy_digest", &self.corpus_policy_digest)?;
        state.serialize_field("clear_surfaces", &self.clear_surfaces)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct SemanticIngestBatchVisitor;

impl<'de> Visitor<'de> for SemanticIngestBatchVisitor {
    type Value = SemanticIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut base_generation: Option<Option<ManifestGeneration>> = None;
        let mut manifest_digest: Option<String> = None;
        let mut batch_digest: Option<String> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut model_contract: Option<EmbeddingModelContract> = None;
        let mut required_corpora: Option<Vec<SemanticCorpusKindV1>> = None;
        let mut corpus_policy_digest: Option<Option<String>> = None;
        let mut clear_surfaces: Option<Vec<SearchScopeSurface>> = None;
        let mut replace_scopes: Option<Vec<SemanticReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<SemanticTombstoneScope>> = None;
        let mut seal: Option<bool> = None;
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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "base_generation" => {
                    if base_generation.is_some() {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "model_contract" => {
                    if model_contract.is_some() {
                        return Err(de::Error::duplicate_field("model_contract"));
                    }
                    model_contract = Some(map.next_value()?);
                }
                "required_corpora" => {
                    if required_corpora.is_some() {
                        return Err(de::Error::duplicate_field("required_corpora"));
                    }
                    required_corpora = Some(map.next_value()?);
                }
                "corpus_policy_digest" => {
                    if corpus_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("corpus_policy_digest"));
                    }
                    corpus_policy_digest = Some(map.next_value()?);
                }
                "clear_surfaces" => {
                    if clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("clear_surfaces"));
                    }
                    clear_surfaces = Some(map.next_value()?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(map.next_value()?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(map.next_value()?);
                }
                "seal" => {
                    if seal.is_some() {
                        return Err(de::Error::duplicate_field("seal"));
                    }
                    seal = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            model_contract: model_contract
                .ok_or_else(|| de::Error::missing_field("model_contract"))?,
            required_corpora: required_corpora
                .ok_or_else(|| de::Error::missing_field("required_corpora"))?,
            corpus_policy_digest: corpus_policy_digest
                .ok_or_else(|| de::Error::missing_field("corpus_policy_digest"))?,
            clear_surfaces: clear_surfaces
                .ok_or_else(|| de::Error::missing_field("clear_surfaces"))?,
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticIngestBatch",
            SEMANTIC_INGEST_BATCH_FIELDS,
            SemanticIngestBatchVisitor,
        )
    }
}
