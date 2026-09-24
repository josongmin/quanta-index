//! P02A compiler DTO section: `RepoMapCompileRefusalV1` and
//! `CompiledRepoMapCandidateV1` plus their private helpers.
//!
//! These types are the whole-bundle validated compiler boundary defined by
//! S21-03. The compiler never mutates durable state; every failure is a typed
//! refusal carrying `stage`, `code`, optional `limit`, and `observed`, and
//! never any input payload bytes. Wire shapes are hand-rolled (proc-macro
//! derive is banned workspace-wide).

use core::fmt;
use std::error::Error;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::{
    ArtifactContentDigestV1, ArtifactIdentityV1, CanonicalRepoMapCodecErrorV1,
    RepoMapCandidateCommitmentsV1, RepoMapCandidateEnvelopeV1, RepoMapDocType, RepoMapEdgeKind,
    RepoMapNodeRef, RepoRelativePath,
};
use quanta_index_contract_base::ids::LogicalGenerationIdentityV1;

repomap_string_enum! {
    pub enum RepoMapCompileStageV1 {
        BundleValidation => "BundleValidation",
        GraphValidation => "GraphValidation",
        Budget => "Budget",
        Projection => "Projection",
        Commitment => "Commitment",
    }
}

#[cfg(test)]
mod current_wire_tests {
    use super::{RepoMapCompileRefusalCodeV1, RepoMapCompileRefusalV1, RepoMapCompileStageV1};

    #[test]
    fn compile_refusal_requires_explicit_optional_limit() {
        let refusal = RepoMapCompileRefusalV1::new(
            RepoMapCompileStageV1::BundleValidation,
            RepoMapCompileRefusalCodeV1::EmptyBundle,
            None,
            0,
        );
        let mut wire = serde_json::to_value(&refusal).expect("serialize refusal");
        assert!(wire.get("limit").is_some_and(serde_json::Value::is_null));
        let _removed = wire.as_object_mut().expect("refusal map").remove("limit");
        assert!(serde_json::from_value::<RepoMapCompileRefusalV1>(wire.clone()).is_err());
        let mut old_cbor = Vec::new();
        ciborium::ser::into_writer(&wire, &mut old_cbor).expect("serialize old refusal");
        assert!(
            ciborium::de::from_reader::<RepoMapCompileRefusalV1, _>(old_cbor.as_slice()).is_err()
        );
    }
}

repomap_string_enum! {
    pub enum RepoMapCompileRefusalCodeV1 {
        EmptyBundle => "EmptyBundle",
        ProducerDigestInvalid => "ProducerDigestInvalid",
        CrossVariantIdentityCollision => "CrossVariantIdentityCollision",
        DuplicateNode => "DuplicateNode",
        DanglingEdgeEndpoint => "DanglingEdgeEndpoint",
        IllegalEdgeVariant => "IllegalEdgeVariant",
        SelfLoop => "SelfLoop",
        TokenlessInput => "TokenlessInput",
        FocusSubjectNotFound => "FocusSubjectNotFound",
        NodeLimitExceeded => "NodeLimitExceeded",
        EdgeLimitExceeded => "EdgeLimitExceeded",
        OwnerSymbolLimitExceeded => "OwnerSymbolLimitExceeded",
        PreviewLimitExceeded => "PreviewLimitExceeded",
        MaterializedByteLimitExceeded => "MaterializedByteLimitExceeded",
        WorkLimitExceeded => "WorkLimitExceeded",
        ArithmeticOverflow => "ArithmeticOverflow",
    }
}

/// Typed whole-bundle compile refusal. Never carries input payload bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapCompileRefusalV1 {
    stage: RepoMapCompileStageV1,
    code: RepoMapCompileRefusalCodeV1,
    limit: Option<u64>,
    observed: u64,
}

impl RepoMapCompileRefusalV1 {
    #[must_use]
    pub const fn new(
        stage: RepoMapCompileStageV1,
        code: RepoMapCompileRefusalCodeV1,
        limit: Option<u64>,
        observed: u64,
    ) -> Self {
        Self {
            stage,
            code,
            limit,
            observed,
        }
    }

    #[must_use]
    pub const fn stage(&self) -> RepoMapCompileStageV1 {
        self.stage
    }

    #[must_use]
    pub const fn code(&self) -> RepoMapCompileRefusalCodeV1 {
        self.code
    }

    #[must_use]
    pub const fn limit(&self) -> Option<u64> {
        self.limit
    }

    #[must_use]
    pub const fn observed(&self) -> u64 {
        self.observed
    }
}

impl fmt::Display for RepoMapCompileRefusalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.limit {
            Some(limit) => write!(
                formatter,
                "repomap compile refused: stage={} code={} observed={} limit={}",
                self.stage.as_code_str(),
                self.code.as_code_str(),
                self.observed,
                limit
            ),
            None => write!(
                formatter,
                "repomap compile refused: stage={} code={} observed={}",
                self.stage.as_code_str(),
                self.code.as_code_str(),
                self.observed
            ),
        }
    }
}

impl Error for RepoMapCompileRefusalV1 {}

const REPOMAP_COMPILE_REFUSAL_FIELDS: &[&str] = &["stage", "code", "limit", "observed"];

impl Serialize for RepoMapCompileRefusalV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapCompileRefusalV1", 4)?;
        state.serialize_field("stage", &self.stage)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("limit", &self.limit)?;
        state.serialize_field("observed", &self.observed)?;
        state.end()
    }
}

struct RepoMapCompileRefusalVisitor;

impl<'de> Visitor<'de> for RepoMapCompileRefusalVisitor {
    type Value = RepoMapCompileRefusalV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapCompileRefusalV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut stage: Option<RepoMapCompileStageV1> = None;
        let mut code: Option<RepoMapCompileRefusalCodeV1> = None;
        let mut limit: Option<Option<u64>> = None;
        let mut observed: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "stage" => {
                    if stage.is_some() {
                        return Err(de::Error::duplicate_field("stage"));
                    }
                    stage = Some(map.next_value()?);
                }
                "code" => {
                    if code.is_some() {
                        return Err(de::Error::duplicate_field("code"));
                    }
                    code = Some(map.next_value()?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(de::Error::duplicate_field("limit"));
                    }
                    limit = Some(map.next_value()?);
                }
                "observed" => {
                    if observed.is_some() {
                        return Err(de::Error::duplicate_field("observed"));
                    }
                    observed = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_COMPILE_REFUSAL_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapCompileRefusalV1 {
            stage: stage.ok_or_else(|| de::Error::missing_field("stage"))?,
            code: code.ok_or_else(|| de::Error::missing_field("code"))?,
            limit: limit.ok_or_else(|| de::Error::missing_field("limit"))?,
            observed: observed.ok_or_else(|| de::Error::missing_field("observed"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapCompileRefusalV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapCompileRefusalV1",
            REPOMAP_COMPILE_REFUSAL_FIELDS,
            RepoMapCompileRefusalVisitor,
        )
    }
}

/// Immutable per-stage compiler ceilings. Every ceiling must be positive;
/// a zero ceiling is refused at construction because it would make the whole
/// compiler unusable instead of bounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "field names are the public contract surface; stripping the `max_` prefix would break the typed API"
)]
pub struct RepoMapCompilerBudgetV1 {
    pub max_nodes: u64,
    pub max_edges: u64,
    pub max_owner_symbols: u64,
    pub max_preview_bytes: u64,
    pub max_materialized_bytes: u64,
    pub max_work_units: u64,
}

impl RepoMapCompilerBudgetV1 {
    pub const fn new(
        max_nodes: u64,
        max_edges: u64,
        max_owner_symbols: u64,
        max_preview_bytes: u64,
        max_materialized_bytes: u64,
        max_work_units: u64,
    ) -> Result<Self, RepoMapCompileRefusalV1> {
        if max_nodes == 0
            || max_edges == 0
            || max_owner_symbols == 0
            || max_preview_bytes == 0
            || max_materialized_bytes == 0
            || max_work_units == 0
        {
            return Err(RepoMapCompileRefusalV1::new(
                RepoMapCompileStageV1::Budget,
                RepoMapCompileRefusalCodeV1::ArithmeticOverflow,
                None,
                0,
            ));
        }
        Ok(Self {
            max_nodes,
            max_edges,
            max_owner_symbols,
            max_preview_bytes,
            max_materialized_bytes,
            max_work_units,
        })
    }

    #[must_use]
    pub const fn default_ceiling() -> Self {
        Self {
            max_nodes: 200_000,
            max_edges: 2_000_000,
            max_owner_symbols: 100_000,
            max_preview_bytes: 1 << 20,
            max_materialized_bytes: 64 << 20,
            max_work_units: 1 << 30,
        }
    }
}

impl Default for RepoMapCompilerBudgetV1 {
    fn default() -> Self {
        Self::default_ceiling()
    }
}

const REPOMAP_COMPILER_BUDGET_FIELDS: &[&str] = &[
    "max_nodes",
    "max_edges",
    "max_owner_symbols",
    "max_preview_bytes",
    "max_materialized_bytes",
    "max_work_units",
];

impl Serialize for RepoMapCompilerBudgetV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapCompilerBudgetV1", 6)?;
        state.serialize_field("max_nodes", &self.max_nodes)?;
        state.serialize_field("max_edges", &self.max_edges)?;
        state.serialize_field("max_owner_symbols", &self.max_owner_symbols)?;
        state.serialize_field("max_preview_bytes", &self.max_preview_bytes)?;
        state.serialize_field("max_materialized_bytes", &self.max_materialized_bytes)?;
        state.serialize_field("max_work_units", &self.max_work_units)?;
        state.end()
    }
}

struct RepoMapCompilerBudgetVisitor;

impl<'de> Visitor<'de> for RepoMapCompilerBudgetVisitor {
    type Value = RepoMapCompilerBudgetV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapCompilerBudgetV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut max_nodes: Option<u64> = None;
        let mut max_edges: Option<u64> = None;
        let mut max_owner_symbols: Option<u64> = None;
        let mut max_preview_bytes: Option<u64> = None;
        let mut max_materialized_bytes: Option<u64> = None;
        let mut max_work_units: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "max_nodes" => {
                    if max_nodes.is_some() {
                        return Err(de::Error::duplicate_field("max_nodes"));
                    }
                    max_nodes = Some(map.next_value()?);
                }
                "max_edges" => {
                    if max_edges.is_some() {
                        return Err(de::Error::duplicate_field("max_edges"));
                    }
                    max_edges = Some(map.next_value()?);
                }
                "max_owner_symbols" => {
                    if max_owner_symbols.is_some() {
                        return Err(de::Error::duplicate_field("max_owner_symbols"));
                    }
                    max_owner_symbols = Some(map.next_value()?);
                }
                "max_preview_bytes" => {
                    if max_preview_bytes.is_some() {
                        return Err(de::Error::duplicate_field("max_preview_bytes"));
                    }
                    max_preview_bytes = Some(map.next_value()?);
                }
                "max_materialized_bytes" => {
                    if max_materialized_bytes.is_some() {
                        return Err(de::Error::duplicate_field("max_materialized_bytes"));
                    }
                    max_materialized_bytes = Some(map.next_value()?);
                }
                "max_work_units" => {
                    if max_work_units.is_some() {
                        return Err(de::Error::duplicate_field("max_work_units"));
                    }
                    max_work_units = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_COMPILER_BUDGET_FIELDS,
                    ));
                }
            }
        }
        let budget = RepoMapCompilerBudgetV1 {
            max_nodes: max_nodes.ok_or_else(|| de::Error::missing_field("max_nodes"))?,
            max_edges: max_edges.ok_or_else(|| de::Error::missing_field("max_edges"))?,
            max_owner_symbols: max_owner_symbols
                .ok_or_else(|| de::Error::missing_field("max_owner_symbols"))?,
            max_preview_bytes: max_preview_bytes
                .ok_or_else(|| de::Error::missing_field("max_preview_bytes"))?,
            max_materialized_bytes: max_materialized_bytes
                .ok_or_else(|| de::Error::missing_field("max_materialized_bytes"))?,
            max_work_units: max_work_units
                .ok_or_else(|| de::Error::missing_field("max_work_units"))?,
        };
        if budget.max_nodes == 0
            || budget.max_edges == 0
            || budget.max_owner_symbols == 0
            || budget.max_preview_bytes == 0
            || budget.max_materialized_bytes == 0
            || budget.max_work_units == 0
        {
            return Err(de::Error::custom(
                "compiler budget ceiling must be positive",
            ));
        }
        Ok(budget)
    }
}

impl<'de> Deserialize<'de> for RepoMapCompilerBudgetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapCompilerBudgetV1",
            REPOMAP_COMPILER_BUDGET_FIELDS,
            RepoMapCompilerBudgetVisitor,
        )
    }
}

/// Observed resource usage recorded once by the compiler. `within` re-derives
/// the cap decision so downstream owners can verify the receipt without
/// trusting the compiler's accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepoMapResourceReceiptV1 {
    pub nodes: u64,
    pub edges: u64,
    pub owner_symbols: u64,
    pub preview_bytes: u64,
    pub materialized_bytes: u64,
    pub work_units: u64,
    pub budget: RepoMapCompilerBudgetV1,
}

impl RepoMapResourceReceiptV1 {
    #[must_use]
    pub const fn observed(&self) -> bool {
        self.nodes <= self.budget.max_nodes
            && self.edges <= self.budget.max_edges
            && self.owner_symbols <= self.budget.max_owner_symbols
            && self.preview_bytes <= self.budget.max_preview_bytes
            && self.materialized_bytes <= self.budget.max_materialized_bytes
            && self.work_units <= self.budget.max_work_units
    }
}

const REPOMAP_RESOURCE_RECEIPT_FIELDS: &[&str] = &[
    "nodes",
    "edges",
    "owner_symbols",
    "preview_bytes",
    "materialized_bytes",
    "work_units",
    "budget",
];

impl Serialize for RepoMapResourceReceiptV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapResourceReceiptV1", 7)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("edges", &self.edges)?;
        state.serialize_field("owner_symbols", &self.owner_symbols)?;
        state.serialize_field("preview_bytes", &self.preview_bytes)?;
        state.serialize_field("materialized_bytes", &self.materialized_bytes)?;
        state.serialize_field("work_units", &self.work_units)?;
        state.serialize_field("budget", &self.budget)?;
        state.end()
    }
}

struct RepoMapResourceReceiptVisitor;

impl<'de> Visitor<'de> for RepoMapResourceReceiptVisitor {
    type Value = RepoMapResourceReceiptV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapResourceReceiptV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut nodes: Option<u64> = None;
        let mut edges: Option<u64> = None;
        let mut owner_symbols: Option<u64> = None;
        let mut preview_bytes: Option<u64> = None;
        let mut materialized_bytes: Option<u64> = None;
        let mut work_units: Option<u64> = None;
        let mut budget: Option<RepoMapCompilerBudgetV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "nodes" => {
                    if nodes.is_some() {
                        return Err(de::Error::duplicate_field("nodes"));
                    }
                    nodes = Some(map.next_value()?);
                }
                "edges" => {
                    if edges.is_some() {
                        return Err(de::Error::duplicate_field("edges"));
                    }
                    edges = Some(map.next_value()?);
                }
                "owner_symbols" => {
                    if owner_symbols.is_some() {
                        return Err(de::Error::duplicate_field("owner_symbols"));
                    }
                    owner_symbols = Some(map.next_value()?);
                }
                "preview_bytes" => {
                    if preview_bytes.is_some() {
                        return Err(de::Error::duplicate_field("preview_bytes"));
                    }
                    preview_bytes = Some(map.next_value()?);
                }
                "materialized_bytes" => {
                    if materialized_bytes.is_some() {
                        return Err(de::Error::duplicate_field("materialized_bytes"));
                    }
                    materialized_bytes = Some(map.next_value()?);
                }
                "work_units" => {
                    if work_units.is_some() {
                        return Err(de::Error::duplicate_field("work_units"));
                    }
                    work_units = Some(map.next_value()?);
                }
                "budget" => {
                    if budget.is_some() {
                        return Err(de::Error::duplicate_field("budget"));
                    }
                    budget = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_RESOURCE_RECEIPT_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapResourceReceiptV1 {
            nodes: nodes.ok_or_else(|| de::Error::missing_field("nodes"))?,
            edges: edges.ok_or_else(|| de::Error::missing_field("edges"))?,
            owner_symbols: owner_symbols
                .ok_or_else(|| de::Error::missing_field("owner_symbols"))?,
            preview_bytes: preview_bytes
                .ok_or_else(|| de::Error::missing_field("preview_bytes"))?,
            materialized_bytes: materialized_bytes
                .ok_or_else(|| de::Error::missing_field("materialized_bytes"))?,
            work_units: work_units.ok_or_else(|| de::Error::missing_field("work_units"))?,
            budget: budget.ok_or_else(|| de::Error::missing_field("budget"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapResourceReceiptV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapResourceReceiptV1",
            REPOMAP_RESOURCE_RECEIPT_FIELDS,
            RepoMapResourceReceiptVisitor,
        )
    }
}

/// One canonical graph node with typed variant+domain identity and the exact
/// in/out degrees derived from the validated edge table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRepoMapNodeV1 {
    pub identity: RepoMapNodeRef,
    pub degree_in: u32,
    pub degree_out: u32,
}

/// One canonical graph edge. `source`/`target` carry the full typed identity;
/// the pair (kind, source, target) is unique in the canonical table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRepoMapEdgeV1 {
    pub kind: RepoMapEdgeKind,
    pub source: RepoMapNodeRef,
    pub target: RepoMapNodeRef,
}

/// Bounded search projection entry materialized by the compiler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRepoMapProjectionEntryV1 {
    pub subject: RepoMapNodeRef,
    pub doc_type: RepoMapDocType,
    pub owner_path: RepoRelativePath,
    pub search_text: String,
    pub final_score_millis: i64,
}

/// Canonical sorted graph tables.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CompiledRepoMapGraphV1 {
    pub nodes: Vec<CompiledRepoMapNodeV1>,
    pub edges: Vec<CompiledRepoMapEdgeV1>,
}

/// The whole-bundle validated compiler output.
///
/// Carries everything P03 needs to build `RepoMapCandidateEnvelopeV1` without
/// the raw bundle: canonical graph, bounded projection, all five commitments,
/// the compiled payload bytes, and the resource receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRepoMapCandidateV1 {
    graph: CompiledRepoMapGraphV1,
    projection: Vec<CompiledRepoMapProjectionEntryV1>,
    commitments: RepoMapCandidateCommitmentsV1,
    compiled_payload: Vec<u8>,
    resource_receipt: RepoMapResourceReceiptV1,
}

impl CompiledRepoMapCandidateV1 {
    #[must_use]
    pub fn from_parts(
        graph: CompiledRepoMapGraphV1,
        projection: Vec<CompiledRepoMapProjectionEntryV1>,
        commitments: RepoMapCandidateCommitmentsV1,
        compiled_payload: Vec<u8>,
        resource_receipt: RepoMapResourceReceiptV1,
    ) -> Self {
        Self {
            graph,
            projection,
            commitments,
            compiled_payload,
            resource_receipt,
        }
    }

    #[must_use]
    pub fn graph(&self) -> &CompiledRepoMapGraphV1 {
        &self.graph
    }

    #[must_use]
    pub fn projection(&self) -> &[CompiledRepoMapProjectionEntryV1] {
        &self.projection
    }

    #[must_use]
    pub const fn commitments(&self) -> RepoMapCandidateCommitmentsV1 {
        self.commitments
    }

    #[must_use]
    pub fn compiled_payload(&self) -> &[u8] {
        &self.compiled_payload
    }

    #[must_use]
    pub const fn resource_receipt(&self) -> RepoMapResourceReceiptV1 {
        self.resource_receipt
    }

    #[must_use]
    pub fn content_digest(&self) -> ArtifactContentDigestV1 {
        ArtifactContentDigestV1::for_payload(&self.compiled_payload)
    }

    #[must_use]
    pub fn byte_size(&self) -> u64 {
        // usize wider than u64 cannot occur on supported targets; keep the
        // saturating ceiling rather than panicking or silently truncating.
        u64::try_from(self.compiled_payload.len()).map_or(u64::MAX, |size| size)
    }

    #[must_use]
    pub fn artifact_identity(
        &self,
        logical_identity: LogicalGenerationIdentityV1,
    ) -> ArtifactIdentityV1 {
        ArtifactIdentityV1::new(logical_identity, self.content_digest(), self.byte_size())
    }

    /// Build the immutable P01A envelope for this candidate. The logical
    /// identity comes from the publish caller, never from the bundle bytes.
    pub fn envelope(
        &self,
        logical_identity: LogicalGenerationIdentityV1,
    ) -> Result<RepoMapCandidateEnvelopeV1, CanonicalRepoMapCodecErrorV1> {
        let artifact = ArtifactIdentityV1::new(
            logical_identity.clone(),
            self.content_digest(),
            self.byte_size(),
        );
        RepoMapCandidateEnvelopeV1::new(
            logical_identity,
            self.commitments,
            artifact,
            self.compiled_payload.clone(),
        )
    }
}
