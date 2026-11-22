//! Immutable ANN region directory and bounded delta ownership.
//!
//! The directory is one Catalog child. Its map roots retain every selected
//! region descriptor and delta member until the containing view is reclaimed.

use std::io;

use quanta_index_core::index::{
    AttemptContext, CoverageBinding, Digest, IndexError, InstanceSpec, MapEdit, MapPage, MapRoot,
    MemoryLease, MemoryLifetime, ObjectChildren, ObjectRef, OwnerKey, OwnerMemberBinding,
    StoredObject, WorkBudget, WorkUsage, owner_map_key,
};

use crate::codec::cbor_serde;

use super::super::ann_files;
use super::super::ann_region_policy::{MAX_REGION_CELLS, MAX_REGION_DIMENSION, MAX_REGION_ROWS};
use super::native_graph;
use super::selected_owner::scan_ordered_page;
use super::{EmbeddingRecipe, LoadedOwnerArtifact, ReadContext, load_query_owner_artifact};

mod consolidation;
mod maintenance;
mod source_reverse;

const GRAPH_DIRECTORY_FORMAT: u32 = 4;
const LANCEDB_DIRECTORY_FORMAT: u32 = 5;
const MAX_DIRECTORY_BYTES: usize = 4096;
const MAX_DESCRIPTOR_BYTES: usize = 256 * 1024;
pub(super) const MAX_REGIONS: u64 = 4096;
pub(super) const MAX_DELTA_OWNERS: u32 = 64;
pub(super) const MAX_DELTA_ROWS: u32 = 8_192;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AnnBackend {
    Graph,
    LanceDb,
}

impl AnnBackend {
    const fn directory_format(self) -> u32 {
        match self {
            Self::Graph => GRAPH_DIRECTORY_FORMAT,
            Self::LanceDb => LANCEDB_DIRECTORY_FORMAT,
        }
    }

    fn from_directory_format(format: u32) -> Result<Self, IndexError> {
        match format {
            GRAPH_DIRECTORY_FORMAT => Ok(Self::Graph),
            LANCEDB_DIRECTORY_FORMAT => Ok(Self::LanceDb),
            _ => Err(IndexError::RebuildRequired),
        }
    }
}

pub(super) struct RegionSealMetadata {
    pub recipe: Digest,
    pub generation: u64,
    pub dimension: u32,
    pub rows: u32,
    pub index_kind: quanta_index_core::DiagnosticAnnIndexKindV1,
    pub files: u32,
    pub chunks: u32,
}

pub(super) struct PreparedDeltaEdit {
    pub(super) edit: MapEdit,
    pub(super) next_owners: u32,
    pub(super) next_rows: u32,
    _key_memory: Option<MemoryLease>,
}

/// One canonical member and artifact validation, scoped to this attempt.
/// Holding the decoded artifact also holds its charged memory lease until the
/// delta edit is staged or this candidate is abandoned.
pub(super) struct ValidatedDeltaMember {
    reference: ObjectRef,
    binding: OwnerMemberBinding,
    artifact: LoadedOwnerArtifact,
    recipe: Digest,
}

impl ValidatedDeltaMember {
    pub(super) fn load(
        reference: ObjectRef,
        owner: OwnerKey,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        let binding = load_member(reference, owner, context.objects, context.budget)?;
        Self::from_binding(reference, binding, recipe, context)
    }

    pub(super) fn from_binding(
        reference: ObjectRef,
        binding: OwnerMemberBinding,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        let artifact =
            load_query_owner_artifact(context.objects, context.budget, &binding, recipe)?;
        Ok(Self {
            reference,
            binding,
            artifact,
            recipe: recipe.fingerprint(),
        })
    }

    pub(super) fn binding(&self) -> &OwnerMemberBinding {
        &self.binding
    }

    pub(super) fn rows(&self) -> Result<u32, IndexError> {
        u32::try_from(self.artifact.vectors.len()).map_err(|_error| IndexError::ResourceLimited)
    }
}

#[derive(serde::Deserialize)]
struct DirectoryFormatHeader {
    format: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AnnDirectory {
    pub format: u32,
    pub recipe: Digest,
    pub owners: MapRoot,
    pub regions: MapRoot,
    pub delta: MapRoot,
    /// Complete source-owner to semantic-owner membership for this selected
    /// coverage root. `SourceLocal` instances keep the empty root.
    pub source_reverse: MapRoot,
    pub next_region: u64,
    pub region_count: u32,
    pub maintenance_after: Option<u64>,
    pub delta_owners: u32,
    pub delta_rows: u32,
}

cbor_serde!(AnnDirectory {
    format: u32,
    recipe: Digest,
    owners: MapRoot,
    regions: MapRoot,
    delta: MapRoot,
    source_reverse: MapRoot,
    next_region: u64,
    region_count: u32,
    maintenance_after: Option<u64>,
    delta_owners: u32,
    delta_rows: u32,
});

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RegionDescriptor {
    pub fileset: ObjectRef,
    pub members: MapRoot,
    pub center: Vec<f32>,
    pub rows: u32,
}

cbor_serde!(RegionDescriptor {
    fileset: ObjectRef,
    members: MapRoot,
    center: Vec<f32>,
    rows: u32,
});

pub(super) struct LoadedRegionDescriptor {
    pub descriptor: RegionDescriptor,
    object: StoredObject,
    _memory: Option<MemoryLease>,
}

impl LoadedRegionDescriptor {
    pub(super) fn reference(&self) -> ObjectRef {
        self.object.registration.reference
    }

    /// Keep the authenticated DATA while releasing the routing-only decode.
    pub(super) fn into_stored_object(self) -> StoredObject {
        let Self {
            descriptor,
            object,
            _memory: memory,
        } = self;
        drop(descriptor);
        drop(memory);
        object
    }
}

fn invalid(message: &'static str) -> IndexError {
    IndexError::InvalidInput(message.into())
}

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

pub(super) fn validate_coverage_layout(coverage: &CoverageBinding) -> Result<(), IndexError> {
    if coverage.owners.0.is_some() {
        if coverage.searchable.is_none() {
            return Err(corrupt("Native ANN coverage has no searchable directory"));
        }
        if coverage.searchable == coverage.owners.0 {
            return Err(IndexError::RebuildRequired);
        }
    }
    Ok(())
}

// Use only after an edit whose expected and next values differ.
fn require_changed_map_root(previous: MapRoot, next: MapRoot) -> Result<MapRoot, IndexError> {
    if previous == next {
        return Err(corrupt("Native ANN map edit did not change the root"));
    }
    Ok(next)
}

fn refs(roots: [Option<ObjectRef>; 3]) -> ObjectChildren {
    ObjectChildren::from_references([roots[0], roots[1], roots[2], None])
}

fn directory_refs(directory: &AnnDirectory) -> ObjectChildren {
    ObjectChildren::from_references([
        directory.owners.0,
        directory.regions.0,
        directory.delta.0,
        directory.source_reverse.0,
    ])
}

fn validate_delta_counts(owners: u32, rows: u32) -> Result<(), IndexError> {
    if owners > MAX_DELTA_OWNERS
        || rows > MAX_DELTA_ROWS
        || (owners == 0 && rows != 0)
        || rows < owners
    {
        return Err(corrupt("Native ANN delta counters are invalid"));
    }
    Ok(())
}

fn validate_directory(directory: &AnnDirectory) -> Result<(), IndexError> {
    let _backend = directory.backend()?;
    if u64::from(directory.region_count) > MAX_REGIONS
        || u64::from(directory.region_count) > directory.next_region
        || (directory.region_count == 0) != directory.regions.0.is_none()
        || directory
            .maintenance_after
            .is_some_and(|id| id >= directory.next_region)
        || (directory.region_count == 0 && directory.maintenance_after.is_some())
        || (directory.delta_owners == 0) != directory.delta.0.is_none()
        || (directory.owners.0.is_none() && directory.source_reverse.0.is_some())
    {
        return Err(corrupt("Native ANN directory shape is invalid"));
    }
    validate_delta_counts(directory.delta_owners, directory.delta_rows)
}

fn validate_descriptor(descriptor: &RegionDescriptor, dimension: u32) -> Result<(), IndexError> {
    let rows = usize::try_from(descriptor.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let dimension = usize::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?;
    if descriptor.members.0.is_none()
        || rows == 0
        || rows > MAX_REGION_ROWS
        || dimension == 0
        || dimension > MAX_REGION_DIMENSION
        || descriptor.center.len() != dimension
        || rows
            .checked_mul(dimension)
            .is_none_or(|cells| cells > MAX_REGION_CELLS)
        || descriptor.center.iter().any(|value| !value.is_finite())
        || descriptor
            .center
            .iter()
            .all(|value| value.abs().total_cmp(&0.0).is_eq())
    {
        return Err(corrupt("Native ANN region descriptor shape is invalid"));
    }
    Ok(())
}

struct BoundedJsonWriter<'a> {
    bytes: Vec<u8>,
    maximum: usize,
    budget: &'a mut WorkBudget,
    failure: Option<IndexError>,
}

impl BoundedJsonWriter<'_> {
    fn refuse(&mut self, cause: IndexError) -> io::Error {
        self.failure = Some(cause);
        io::Error::other("Native ANN metadata encoding refused")
    }
}

impl io::Write for BoundedJsonWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(cause) = self.budget.checkpoint() {
            return Err(self.refuse(cause));
        }
        let Some(next) = self.bytes.len().checked_add(bytes.len()) else {
            return Err(self.refuse(IndexError::ResourceLimited));
        };
        if next > self.maximum {
            return Err(self.refuse(IndexError::ResourceLimited));
        }
        let Ok(charged_bytes) = u64::try_from(bytes.len()) else {
            return Err(self.refuse(IndexError::ResourceLimited));
        };
        if let Err(cause) = self.budget.charge(WorkUsage {
            bytes: charged_bytes,
            ..WorkUsage::default()
        }) {
            return Err(self.refuse(cause));
        }
        if self.bytes.try_reserve(bytes.len()).is_err() {
            return Err(self.refuse(IndexError::ResourceLimited));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode<T: serde::Serialize>(
    value: &T,
    maximum: usize,
    budget: &mut WorkBudget,
) -> Result<Vec<u8>, IndexError> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::new(),
        maximum,
        budget,
        failure: None,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return Err(writer.failure.take().unwrap_or_else(|| {
            IndexError::Storage(format!("Native ANN metadata encode: {error}"))
        }));
    }
    Ok(writer.bytes)
}

fn read_metadata<T: serde::de::DeserializeOwned + serde::Serialize>(
    object: &StoredObject,
    reference: ObjectRef,
    maximum: usize,
    budget: &mut WorkBudget,
) -> Result<(T, Option<MemoryLease>), IndexError> {
    if object.registration.reference != reference
        || object.payload.is_empty()
        || object.payload.len() > maximum
    {
        return Err(corrupt(
            "Native ANN metadata reference or length is invalid",
        ));
    }
    let grant = maximum.checked_mul(3).ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(grant).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(object.payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let value: T = serde_json::from_slice(&object.payload)
        .map_err(|error| IndexError::CorruptObject(format!("Native ANN metadata: {error}")))?;
    if encode(&value, maximum, budget)? != object.payload {
        return Err(corrupt("Native ANN metadata is not canonical"));
    }
    Ok((value, memory))
}

/// The artifact version gate is intentionally separate from its strict current
/// shape decoder: old persisted layouts require rebuild, never an IR upcast.
fn require_directory_format(
    object: &StoredObject,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if object.registration.reference != reference
        || object.payload.is_empty()
        || object.payload.len() > MAX_DIRECTORY_BYTES
    {
        return Err(corrupt("Native ANN directory reference or size is invalid"));
    }
    let _memory = budget.reserve_memory(
        u64::try_from(MAX_DIRECTORY_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(object.payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let header: DirectoryFormatHeader =
        serde_json::from_slice(&object.payload).map_err(|error| {
            IndexError::CorruptObject(format!("Native ANN directory format: {error}"))
        })?;
    let _backend = AnnBackend::from_directory_format(header.format)?;
    Ok(())
}

impl AnnDirectory {
    pub(super) fn load_for_coverage(
        coverage: &CoverageBinding,
        recipe: &EmbeddingRecipe,
        read: &mut ReadContext<'_>,
    ) -> Result<Option<Self>, IndexError> {
        if coverage.searchable == coverage.owners.0 {
            return Ok(None);
        }
        let reference = coverage
            .searchable
            .ok_or_else(|| corrupt("Native ANN coverage has no searchable directory"))?;
        let directory = Self::load(reference, recipe.fingerprint(), read)?;
        if directory.owners != coverage.owners {
            return Err(corrupt("Native ANN directory selects different owners"));
        }
        Ok(Some(directory))
    }

    /// Derive the complete projected source membership from the immutable
    /// prior and next coverage roots inside this publication attempt.
    pub(super) fn refresh_source_reverse(
        &mut self,
        instance: &InstanceSpec,
        next_owners: MapRoot,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        self.source_reverse = source_reverse::refresh(
            self.source_reverse,
            self.owners,
            next_owners,
            instance,
            context,
        )?;
        Ok(())
    }

    /// Coalesce small immutable runs inside this original publication attempt.
    /// Commit caller metadata only after every source and staging step succeeds.
    /// An aborted attempt still owns any unreachable COW objects it staged.
    pub(super) fn consolidated_members(
        &mut self,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<consolidation::RegionMembers, IndexError> {
        // Separate carry work from native graph build and installation.
        // The existing optional producer starts no clock when tracing is Off.
        let span = crate::build_diagnostic::begin_build_stage(
            context.budget.request().diagnostic_context(),
            quanta_index_core::DiagnosticStageV1::Compaction,
        );
        let child = span.as_ref().and_then(|span| {
            context
                .budget
                .request()
                .diagnostic_context()
                .map(|diagnostic| diagnostic.child_context(span))
        });
        let mut next = *self;
        let result = {
            let mut budget = context.budget.scoped_diagnostic_context(child);
            let scoped = &mut AttemptContext {
                attempt: context.attempt,
                objects: context.objects,
                maps: context.maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut budget,
            };
            (|| {
                validate_directory(&next)?;
                if next.recipe != recipe.fingerprint() {
                    return Err(invalid("Native ANN consolidation recipe mismatch"));
                }
                consolidation::prepare(&mut next, recipe, scoped)
            })()
        };
        if result.is_err() {
            context
                .budget
                .record_failed_stage(quanta_index_core::DiagnosticStageV1::Compaction);
        } else {
            *self = next;
        }
        crate::build_diagnostic::finish_build_stage(span, result.is_ok());
        result
    }

    #[cfg(test)]
    pub(super) const fn empty(recipe: Digest, owners: MapRoot) -> Self {
        Self::empty_with_backend(recipe, owners, AnnBackend::Graph)
    }

    pub(super) const fn empty_with_backend(
        recipe: Digest,
        owners: MapRoot,
        backend: AnnBackend,
    ) -> Self {
        Self {
            format: backend.directory_format(),
            recipe,
            owners,
            regions: MapRoot(None),
            delta: MapRoot(None),
            source_reverse: MapRoot(None),
            next_region: 0,
            region_count: 0,
            maintenance_after: None,
            delta_owners: 0,
            delta_rows: 0,
        }
    }

    pub(super) fn backend(&self) -> Result<AnnBackend, IndexError> {
        AnnBackend::from_directory_format(self.format)
    }

    pub(super) fn inspect_region_seal(
        &self,
        fileset: ObjectRef,
        objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<RegionSealMetadata, IndexError> {
        match self.backend()? {
            AnnBackend::Graph => {
                let seal = native_graph::load_seal(objects, fileset, budget)?;
                Ok(RegionSealMetadata {
                    recipe: seal.recipe,
                    generation: seal.generation,
                    dimension: seal.dimension,
                    rows: seal.rows,
                    index_kind: quanta_index_core::DiagnosticAnnIndexKindV1::Hnsw,
                    files: 1,
                    chunks: 1,
                })
            }
            AnnBackend::LanceDb => {
                let summary = ann_files::inspect_fileset(objects, fileset, budget)?;
                Ok(RegionSealMetadata {
                    recipe: summary.seal.recipe,
                    generation: summary.seal.generation,
                    dimension: summary.seal.dimension,
                    rows: summary.seal.rows,
                    index_kind: summary.index_kind,
                    files: summary.files,
                    chunks: summary.chunks,
                })
            }
        }
    }

    fn inspect_region_metadata(
        &self,
        fileset: ObjectRef,
        objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<RegionSealMetadata, IndexError> {
        match self.backend()? {
            AnnBackend::Graph => {
                let seal = native_graph::load_seal(objects, fileset, budget)?;
                Ok(RegionSealMetadata {
                    recipe: seal.recipe,
                    generation: seal.generation,
                    dimension: seal.dimension,
                    rows: seal.rows,
                    index_kind: quanta_index_core::DiagnosticAnnIndexKindV1::Hnsw,
                    files: 1,
                    chunks: 1,
                })
            }
            AnnBackend::LanceDb => {
                let summary = ann_files::inspect_fileset_manifest(objects, fileset, budget)?;
                Ok(RegionSealMetadata {
                    recipe: summary.seal.recipe,
                    generation: summary.seal.generation,
                    dimension: summary.seal.dimension,
                    rows: summary.seal.rows,
                    index_kind: summary.index_kind,
                    files: summary.files,
                    chunks: summary.chunks,
                })
            }
        }
    }

    pub(super) fn load(
        reference: ObjectRef,
        recipe: Digest,
        read: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        let object = read.objects.get(reference, read.budget)?;
        require_directory_format(&object, reference, read.budget)?;
        let (directory, _memory) =
            read_metadata::<Self>(&object, reference, MAX_DIRECTORY_BYTES, read.budget)?;
        validate_directory(&directory)?;
        if directory.recipe != recipe
            || object.registration.children.as_slice() != directory_refs(&directory).as_slice()
        {
            return Err(corrupt("Native ANN directory recipe or children mismatch"));
        }
        Ok(directory)
    }

    pub(super) fn prepare_searchable(
        &self,
        context: &mut AttemptContext<'_>,
    ) -> Result<quanta_index_core::index::SearchablePreparation, IndexError> {
        validate_directory(self)?;
        let memory = context.budget.reserve_memory(
            u64::try_from(MAX_DIRECTORY_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let payload = encode(self, MAX_DIRECTORY_BYTES, context.budget)?;
        if payload.capacity() > MAX_DIRECTORY_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        quanta_index_core::index::SearchablePreparation::object(
            quanta_index_core::index::StagedPayload {
                bytes: payload,
                memory,
            },
            directory_refs(self).as_slice(),
            context,
        )
    }

    #[cfg(test)]
    pub(super) fn save(&self, context: &mut AttemptContext<'_>) -> Result<ObjectRef, IndexError> {
        self.prepare_searchable(context)?
            .install(context)?
            .ok_or_else(|| {
                IndexError::BackendViolation("ANN directory preparation omitted its root".into())
            })
    }

    #[cfg(test)]
    pub(super) fn delta_member(
        &self,
        owner: OwnerKey,
        read: &mut ReadContext<'_>,
    ) -> Result<Option<(OwnerMemberBinding, ObjectRef)>, IndexError> {
        validate_directory(self)?;
        let Some(reference) =
            read.maps
                .lookup_object(self.delta, &owner_map_key(owner), read.budget)?
        else {
            return Ok(None);
        };
        let binding = load_member(reference, owner, read.objects, read.budget)?;
        Ok(Some((binding, reference)))
    }

    /// A delta scan already returned this reference under the selected key.
    /// Authenticate the member without reading the same immutable map entry again.
    pub(super) fn delta_member_from_page(
        &self,
        owner: OwnerKey,
        reference: ObjectRef,
        read: &mut ReadContext<'_>,
    ) -> Result<OwnerMemberBinding, IndexError> {
        validate_directory(self)?;
        load_member(reference, owner, read.objects, read.budget)
    }

    #[cfg(test)]
    pub(super) fn put_delta(
        &mut self,
        owner: OwnerKey,
        member_ref: ObjectRef,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        validate_directory(self)?;
        if self.recipe != recipe.fingerprint() {
            return Err(invalid("Native ANN delta recipe mismatch"));
        }
        let validated = ValidatedDeltaMember::load(member_ref, owner, recipe, context)?;
        self.put_validated_delta(validated, recipe, context)
    }

    #[cfg(test)]
    pub(super) fn put_validated_delta(
        &mut self,
        validated: ValidatedDeltaMember,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if let Some(prepared) = self.prepare_validated_delta(validated, recipe, context)? {
            let root = context.apply(self.delta, std::slice::from_ref(&prepared.edit))?;
            self.assign_edited_delta(root, prepared.next_owners, prepared.next_rows)?;
        }
        Ok(())
    }

    pub(super) fn assign_edited_delta(
        &mut self,
        root: MapRoot,
        owners: u32,
        rows: u32,
    ) -> Result<(), IndexError> {
        let mut next = *self;
        next.delta = require_changed_map_root(self.delta, root)?;
        next.delta_owners = owners;
        next.delta_rows = rows;
        validate_directory(&next)?;
        *self = next;
        Ok(())
    }

    pub(super) fn prepare_validated_delta(
        &self,
        validated: ValidatedDeltaMember,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<Option<PreparedDeltaEdit>, IndexError> {
        validate_directory(self)?;
        if self.recipe != recipe.fingerprint() || validated.recipe != self.recipe {
            return Err(invalid("Native ANN delta recipe mismatch"));
        }
        let owner = validated.binding.owner;
        let member_ref = validated.reference;
        let new_rows = validated.rows()?;
        if new_rows == 0 {
            return Err(invalid("Native ANN delta member has no searchable rows"));
        }
        // The row count is now bound to the authenticated member. Release its
        // decode lease before reading a replacement or staging a COW map edit.
        drop(validated);
        let old_ref =
            context
                .maps
                .lookup_object(self.delta, &owner_map_key(owner), context.budget)?;
        if old_ref == Some(member_ref) {
            return Ok(None);
        }
        let old_rows = if let Some(old_ref) = old_ref {
            member_rows(old_ref, owner, recipe, context.objects, context.budget)?
        } else {
            0
        };
        let next_owners = self
            .delta_owners
            .checked_add(u32::from(old_ref.is_none()))
            .ok_or(IndexError::ResourceLimited)?;
        let next_rows = self
            .delta_rows
            .checked_sub(old_rows)
            .ok_or_else(|| corrupt("Native ANN delta row count underflow"))?
            .checked_add(new_rows)
            .ok_or(IndexError::ResourceLimited)?;
        if next_owners > MAX_DELTA_OWNERS || next_rows > MAX_DELTA_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        validate_delta_counts(next_owners, next_rows)?;
        let key_memory = context
            .budget
            .reserve_memory(38, MemoryLifetime::Temporary)?;
        let key = owner_map_key(owner).to_vec();
        Ok(Some(PreparedDeltaEdit {
            edit: MapEdit {
                key,
                expected: (old_ref).map(Into::into),
                next: Some((member_ref).into()),
            },
            next_owners,
            next_rows,
            _key_memory: key_memory,
        }))
    }

    /// Empty members stay retained through owners; they contribute no search rows.
    pub(super) fn remove_delta(
        &mut self,
        owner: OwnerKey,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        validate_directory(self)?;
        let key = owner_map_key(owner);
        let Some(reference) = context
            .maps
            .lookup_object(self.delta, &key, context.budget)?
        else {
            return Ok(());
        };
        let rows = member_rows(reference, owner, recipe, context.objects, context.budget)?;
        let next_owners = self
            .delta_owners
            .checked_sub(1)
            .ok_or_else(|| corrupt("ANN delta owner count underflow"))?;
        let next_rows = self
            .delta_rows
            .checked_sub(rows)
            .ok_or_else(|| corrupt("ANN delta row count underflow"))?;
        // Counter corruption must refuse before staging any COW write. Keep
        // the caller's directory intact through both admission and apply errors.
        validate_delta_counts(next_owners, next_rows)?;
        let _memory = context
            .budget
            .reserve_memory(38, MemoryLifetime::Temporary)?;
        let root = context.apply(
            self.delta,
            &[MapEdit {
                key: key.to_vec(),
                expected: Some((reference).into()),
                next: None,
            }],
        )?;
        self.assign_edited_delta(root, next_owners, next_rows)
    }

    pub(super) fn append_region(
        &mut self,
        descriptor: &RegionDescriptor,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        let span = crate::build_diagnostic::begin_build_stage(
            context.budget.request().diagnostic_context(),
            quanta_index_core::DiagnosticStageV1::SemanticSeal,
        );
        let ordinal = self.next_region;
        let result = self.append_region_inner(descriptor, context);
        // Report installation only after the authenticated region append.
        if let (Some(diagnostic), Some(span), Ok((reference, seal))) = (
            context.budget.request().diagnostic_context(),
            span.as_ref(),
            result.as_ref(),
        ) {
            diagnostic.ann_region_observation(
                span,
                quanta_index_core::DiagnosticAnnRegionObservationV1 {
                    phase: quanta_index_core::DiagnosticAnnRegionPhaseV1::Installed,
                    fileset_ref: descriptor.fileset.0.0,
                    descriptor_ref: match seal.index_kind {
                        quanta_index_core::DiagnosticAnnIndexKindV1::Hnsw => Some(reference.0.0),
                        _ => None,
                    },
                    ordinal,
                    rows: descriptor.rows,
                    index_kind: seal.index_kind,
                    files: seal.files,
                    chunks: seal.chunks,
                },
            );
        }
        crate::build_diagnostic::finish_build_stage(span, result.is_ok());
        result.map(|(reference, _seal)| reference)
    }

    fn append_region_inner(
        &mut self,
        descriptor: &RegionDescriptor,
        context: &mut AttemptContext<'_>,
    ) -> Result<(ObjectRef, RegionSealMetadata), IndexError> {
        validate_directory(self)?;
        if u64::from(self.region_count) >= MAX_REGIONS {
            return Err(IndexError::ResourceLimited);
        }
        let generation = self
            .next_region
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let seal = match self.backend()? {
            AnnBackend::Graph => {
                // Preserve the format-four full graph authentication at append.
                let graph =
                    native_graph::load(context.objects, descriptor.fileset, context.budget)?;
                let seal = graph.seal;
                drop(graph);
                RegionSealMetadata {
                    recipe: seal.recipe,
                    generation: seal.generation,
                    dimension: seal.dimension,
                    rows: seal.rows,
                    index_kind: quanta_index_core::DiagnosticAnnIndexKindV1::Hnsw,
                    files: 1,
                    chunks: 1,
                }
            }
            AnnBackend::LanceDb => {
                self.inspect_region_seal(descriptor.fileset, context.objects, context.budget)?
            }
        };
        validate_descriptor(descriptor, seal.dimension)?;
        if seal.recipe != self.recipe
            || seal.rows != descriptor.rows
            || seal.generation != generation
        {
            return Err(corrupt("Native ANN region seal mismatch"));
        }
        if self.delta_owners != 0
            && descriptor.members == self.delta
            && descriptor.rows != self.delta_rows
        {
            return Err(corrupt("Native ANN sealed delta row count mismatch"));
        }
        let _memory = context.budget.reserve_memory(
            u64::try_from(MAX_DESCRIPTOR_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let payload = encode(descriptor, MAX_DESCRIPTOR_BYTES, context.budget)?;
        let children = refs([Some(descriptor.fileset), descriptor.members.0, None]);
        let _key_memory = context
            .budget
            .reserve_memory(8, MemoryLifetime::Temporary)?;
        let mut key = Vec::new();
        key.try_reserve_exact(8)
            .map_err(|_| IndexError::ResourceLimited)?;
        key.extend_from_slice(&self.next_region.to_be_bytes());
        let reference = context.put(&payload, children.as_slice())?;
        let regions = context.apply(
            self.regions,
            &[MapEdit {
                key,
                expected: None,
                next: Some((reference).into()),
            }],
        )?;
        let mut next = *self;
        next.regions = require_changed_map_root(self.regions, regions)?;
        next.next_region = generation;
        next.region_count += 1; // Bounded by MAX_REGIONS above.
        if descriptor.members == next.delta {
            next.delta = MapRoot(None);
            next.delta_owners = 0;
            next.delta_rows = 0;
        }
        validate_directory(&next)?;
        *self = next;
        Ok((reference, seal))
    }

    pub(super) fn retire_obsolete(
        &mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        validate_directory(self)?;
        maintenance::sweep(self, context)
    }

    pub(super) fn retire_delta(
        &mut self,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        validate_directory(self)?;
        maintenance::prune_delta(self, recipe, context)
    }

    pub(super) fn ensure_region_capacity(
        &mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        validate_directory(self)?;
        maintenance::ensure_capacity(self, context)
    }

    #[cfg(test)]
    pub(super) fn ensure_region_capacity_for_test(
        &mut self,
        context: &mut AttemptContext<'_>,
        limit: u64,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        validate_directory(self)?;
        maintenance::ensure_capacity_for_test(self, context, limit)
    }

    pub(super) fn region_page(
        &self,
        after: Option<u64>,
        limit: u32,
        continuation: bool,
        read: &mut ReadContext<'_>,
    ) -> Result<MapPage, IndexError> {
        read.budget.checkpoint()?;
        validate_directory(self)?;
        if limit == 0 || limit > 64 {
            return Err(invalid("Native ANN region page limit is invalid"));
        }
        let cursor = after.map(u64::to_be_bytes);
        let page = scan_ordered_page(
            self.regions,
            cursor.as_ref().map(<[u8; 8]>::as_slice),
            limit,
            continuation,
            read,
        )?;
        let entries = u32::try_from(page.entries.len()).map_err(|_| IndexError::ResourceLimited)?;
        // A scan from the beginning can prove the declared count at EOF.
        // A continuation promises at least one additional active region.
        if entries > self.region_count
            || (page.has_more && entries == self.region_count)
            || (after.is_none() && !page.has_more && entries != self.region_count)
        {
            return Err(corrupt("Native ANN region page differs from active count"));
        }
        let mut last = after;
        for (key, _reference) in &page.entries {
            let id = u64::from_be_bytes(
                key.as_slice()
                    .try_into()
                    .map_err(|_error| corrupt("Native ANN region key width is invalid"))?,
            );
            if id >= self.next_region {
                return Err(corrupt("Native ANN region key identity is invalid"));
            }
            last = Some(id);
        }
        if page.has_more && last == self.next_region.checked_sub(1) {
            return Err(corrupt(
                "Native ANN region page continues past its last ordinal",
            ));
        }
        Ok(page)
    }

    #[cfg(test)]
    pub(super) fn region_at(
        &self,
        ordinal: u64,
        read: &mut ReadContext<'_>,
    ) -> Result<LoadedRegionDescriptor, IndexError> {
        validate_directory(self)?;
        if ordinal >= self.next_region {
            return Err(invalid("Native ANN region ordinal is outside directory"));
        }
        let reference = read
            .maps
            .lookup_object(self.regions, &ordinal.to_be_bytes(), read.budget)?
            .ok_or_else(|| corrupt("Native ANN region descriptor is missing"))?;
        Self::load_region_reference(reference, read)
    }

    /// A region page already supplied this reference under the validated key.
    /// Decode and authenticate the same object without repeating the map read.
    pub(super) fn region_from_page(
        &self,
        ordinal: u64,
        reference: ObjectRef,
        read: &mut ReadContext<'_>,
    ) -> Result<LoadedRegionDescriptor, IndexError> {
        validate_directory(self)?;
        if ordinal >= self.next_region {
            return Err(invalid("Native ANN region ordinal is outside directory"));
        }
        Self::load_region_reference(reference, read)
    }

    /// Repeat the selected descriptor read through the original object port.
    /// The routed object retains its original DATA custody; a backend without
    /// retained reads may perform its ordinary get and is validated identically.
    pub(super) fn region_from_retained(
        &self,
        ordinal: u64,
        object: StoredObject,
        read: &mut ReadContext<'_>,
    ) -> Result<LoadedRegionDescriptor, IndexError> {
        validate_directory(self)?;
        if ordinal >= self.next_region {
            return Err(invalid("Native ANN region ordinal is outside directory"));
        }
        let reference = object.registration.reference;
        let retained = read.objects.read_retained(&object, read.budget)?;
        drop(object);
        Self::decode_region_object(reference, retained, read.budget)
    }

    fn load_region_reference(
        reference: ObjectRef,
        read: &mut ReadContext<'_>,
    ) -> Result<LoadedRegionDescriptor, IndexError> {
        let object = read.objects.get(reference, read.budget)?;
        Self::decode_region_object(reference, object, read.budget)
    }

    fn decode_region_object(
        reference: ObjectRef,
        object: StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<LoadedRegionDescriptor, IndexError> {
        let (descriptor, memory) =
            read_metadata::<RegionDescriptor>(&object, reference, MAX_DESCRIPTOR_BYTES, budget)?;
        let dimension = u32::try_from(descriptor.center.len())
            .map_err(|_error| corrupt("Native ANN region dimension overflow"))?;
        validate_descriptor(&descriptor, dimension)?;
        if object.registration.children.as_slice()
            != refs([Some(descriptor.fileset), descriptor.members.0, None]).as_slice()
        {
            return Err(corrupt("Native ANN region descriptor children mismatch"));
        }
        Ok(LoadedRegionDescriptor {
            descriptor,
            object,
            _memory: memory,
        })
    }
}

pub(super) fn load_member(
    reference: ObjectRef,
    owner: OwnerKey,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<OwnerMemberBinding, IndexError> {
    let object = objects.get(reference, budget)?;
    let (binding, _memory) = read_metadata::<OwnerMemberBinding>(
        &object,
        reference,
        quanta_index_core::index::MAX_BINDING_METADATA_BYTES,
        budget,
    )?;
    if binding.owner != owner
        || object.registration.children.as_slice()
            != refs([
                Some(binding.artifact),
                binding.input.source,
                binding.input.projection,
            ])
            .as_slice()
    {
        return Err(corrupt("Native ANN delta member binding mismatch"));
    }
    Ok(binding)
}

fn member_rows(
    reference: ObjectRef,
    owner: OwnerKey,
    recipe: &EmbeddingRecipe,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<u32, IndexError> {
    let member = load_member(reference, owner, objects, budget)?;
    let artifact = load_query_owner_artifact(objects, budget, &member, recipe)?;
    u32::try_from(artifact.vectors.len()).map_err(|_error| IndexError::ResourceLimited)
}

#[cfg(test)]
mod source_reverse_format_tests {
    use super::*;
    use quanta_index_core::index::{RegisteredObject, WorkLimits};
    use std::time::{Duration, Instant};

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn delta_counter_shape_requires_a_row_for_every_nonempty_owner() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([7; 32]));
        for (owners, rows) in [(1, 0), (2, 1), (64, 63)] {
            let mut directory = AnnDirectory::empty(Digest([3; 32]), MapRoot(Some(reference)));
            directory.delta = MapRoot(Some(reference));
            directory.delta_owners = owners;
            directory.delta_rows = rows;
            if !matches!(
                validate_directory(&directory),
                Err(IndexError::CorruptObject(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "delta counters admitted fewer rows than nonempty owners".into(),
                ));
            }
        }
        for (owners, rows) in [(0, 0), (1, 1), (2, 3), (64, MAX_DELTA_ROWS)] {
            let mut directory = AnnDirectory::empty(Digest([3; 32]), MapRoot(Some(reference)));
            directory.delta = MapRoot((owners != 0).then_some(reference));
            directory.delta_owners = owners;
            directory.delta_rows = rows;
            validate_directory(&directory)?;
        }
        Ok(())
    }

    #[test]
    fn old_directory_refuses_before_current_shape_decode() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([7; 32]));
        let payload = br#"{"format":2}"#.to_vec();
        let stored = StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                Vec::new(),
            ),
            payload,
            None,
        );
        let mut work = budget()?;
        if !matches!(
            require_directory_format(&stored, reference, &mut work),
            Err(IndexError::RebuildRequired)
        ) {
            return Err(IndexError::BackendViolation(
                "old ANN directory format was not refused".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn source_reverse_root_is_a_real_registered_child() {
        let refs = [1, 2, 3, 4].map(|byte| ObjectRef(Digest([byte; 32])));
        let mut directory = AnnDirectory::empty(Digest([9; 32]), MapRoot(Some(refs[0])));
        directory.regions = MapRoot(Some(refs[1]));
        directory.delta = MapRoot(Some(refs[2]));
        directory.source_reverse = MapRoot(Some(refs[3]));
        directory.next_region = 1;
        directory.region_count = 1;
        directory.delta_owners = 1;
        directory.delta_rows = 1;
        assert!(validate_directory(&directory).is_ok());
        assert_eq!(directory_refs(&directory).as_slice(), refs.as_slice());
    }
}
