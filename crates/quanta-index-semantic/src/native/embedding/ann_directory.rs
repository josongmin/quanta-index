//! Immutable ANN region directory and bounded delta ownership.
//!
//! The directory is one Catalog child. Its map roots retain every selected
//! region descriptor and delta member until the containing view is reclaimed.

use std::io;

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, InstanceSpec, MapEdit, MapPage, MapRoot, MemoryLease,
    MemoryLifetime, ObjectChildren, ObjectRef, OwnerKey, OwnerMemberBinding, StoredObject,
    WorkBudget, WorkUsage, owner_map_key,
};

use crate::codec::cbor_serde;

use super::{EmbeddingRecipe, LoadedOwnerArtifact, ReadContext, load_query_owner_artifact};
use crate::native::{ann_files, ann_lance};

mod consolidation;
mod maintenance;
mod source_reverse;

const DIRECTORY_FORMAT: u32 = 3;
const MAX_DIRECTORY_BYTES: usize = 4096;
const MAX_DESCRIPTOR_BYTES: usize = 256 * 1024;
pub(super) const MAX_REGIONS: u64 = 4096;
pub(super) const MAX_DELTA_OWNERS: u32 = 64;
pub(super) const MAX_DELTA_ROWS: u32 = 8_192;

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

fn validate_directory(directory: &AnnDirectory) -> Result<(), IndexError> {
    if directory.format != DIRECTORY_FORMAT
        || u64::from(directory.region_count) > MAX_REGIONS
        || u64::from(directory.region_count) > directory.next_region
        || directory.delta_owners > MAX_DELTA_OWNERS
        || directory.delta_rows > MAX_DELTA_ROWS
        || (directory.region_count == 0) != directory.regions.0.is_none()
        || directory
            .maintenance_after
            .is_some_and(|id| id >= directory.next_region)
        || (directory.region_count == 0 && directory.maintenance_after.is_some())
        || (directory.delta_owners == 0) != directory.delta.0.is_none()
        || (directory.delta_owners == 0 && directory.delta_rows != 0)
        || (directory.owners.0.is_none() && directory.source_reverse.0.is_some())
    {
        return Err(corrupt("Native ANN directory shape is invalid"));
    }
    Ok(())
}

fn validate_descriptor(descriptor: &RegionDescriptor, dimension: u32) -> Result<(), IndexError> {
    let rows = usize::try_from(descriptor.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let dimension = usize::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?;
    if descriptor.members.0.is_none()
        || rows == 0
        || rows > ann_lance::MAX_REGION_ROWS
        || dimension == 0
        || dimension > ann_lance::MAX_REGION_DIMENSION
        || descriptor.center.len() != dimension
        || rows
            .checked_mul(dimension)
            .is_none_or(|cells| cells > ann_lance::MAX_REGION_CELLS)
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
    if header.format != DIRECTORY_FORMAT {
        return Err(IndexError::RebuildRequired);
    }
    Ok(())
}

impl AnnDirectory {
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
    pub(super) fn consolidated_members(
        &mut self,
        recipe: &EmbeddingRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<consolidation::RegionMembers, IndexError> {
        // Separate carry work from Lance build and managed file installation.
        // The existing optional producer starts no clock when tracing is Off.
        let span = crate::build::begin_build_stage(
            context.budget.request().diagnostic_context(),
            quanta_index_core::DiagnosticStageV1::Compaction,
        );
        let result = (|| {
            validate_directory(self)?;
            if self.recipe != recipe.fingerprint() {
                return Err(invalid("Native ANN consolidation recipe mismatch"));
            }
            consolidation::prepare(self, recipe, context)
        })();
        if result.is_err() {
            context
                .budget
                .record_failed_stage(quanta_index_core::DiagnosticStageV1::Compaction);
        }
        crate::build::finish_build_stage(span, result.is_ok());
        result
    }

    pub(super) const fn empty(recipe: Digest, owners: MapRoot) -> Self {
        Self {
            format: DIRECTORY_FORMAT,
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
            self.delta = root;
            self.delta_owners = prepared.next_owners;
            self.delta_rows = prepared.next_rows;
        }
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
        self.delta = root;
        self.delta_owners = self
            .delta_owners
            .checked_sub(1)
            .ok_or_else(|| corrupt("ANN delta owner count underflow"))?;
        self.delta_rows = self
            .delta_rows
            .checked_sub(rows)
            .ok_or_else(|| corrupt("ANN delta row count underflow"))?;
        validate_directory(self)
    }

    pub(super) fn append_region(
        &mut self,
        descriptor: &RegionDescriptor,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        validate_directory(self)?;
        if u64::from(self.region_count) >= MAX_REGIONS || self.next_region.checked_add(1).is_none()
        {
            return Err(IndexError::ResourceLimited);
        }
        let fileset = ann_files::load_fileset(context.objects, descriptor.fileset, context.budget)?;
        validate_descriptor(descriptor, fileset.seal.dimension)?;
        if fileset.seal.recipe != self.recipe || fileset.seal.rows != descriptor.rows {
            return Err(corrupt("Native ANN region file-set seal mismatch"));
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
        let reference = context.put(&payload, children.as_slice())?;
        let key_memory = context
            .budget
            .reserve_memory(8, MemoryLifetime::Temporary)?;
        let key = self.next_region.to_be_bytes().to_vec();
        let regions = context.apply(
            self.regions,
            &[MapEdit {
                key,
                expected: None,
                next: Some((reference).into()),
            }],
        )?;
        drop(key_memory);
        self.regions = regions;
        self.next_region = self
            .next_region
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.region_count = self
            .region_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.delta_owners != 0 && descriptor.members == self.delta {
            self.delta = MapRoot(None);
            self.delta_owners = 0;
            self.delta_rows = 0;
        }
        Ok(reference)
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
        read: &mut ReadContext<'_>,
    ) -> Result<MapPage, IndexError> {
        read.budget.checkpoint()?;
        validate_directory(self)?;
        if limit == 0 || limit > 64 {
            return Err(invalid("Native ANN region page limit is invalid"));
        }
        let cursor = after.map(u64::to_be_bytes);
        let page = read.maps.scan(
            self.regions,
            cursor.as_ref().map(<[u8; 8]>::as_slice),
            limit,
            read.budget,
        )?;
        if page.entries.len()
            > usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
            || (page.entries.is_empty() && page.has_more)
        {
            return Err(IndexError::BackendViolation(
                "Native ANN region scan exceeded its page or made no progress".into(),
            ));
        }
        let mut prior = after;
        for (key, _reference) in &page.entries {
            let id = u64::from_be_bytes(
                key.as_slice()
                    .try_into()
                    .map_err(|_error| corrupt("Native ANN region key width is invalid"))?,
            );
            if id >= self.next_region || prior.is_some_and(|previous| id <= previous) {
                return Err(corrupt(
                    "Native ANN region key order or identity is invalid",
                ));
            }
            prior = Some(id);
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
        self.load_region_reference(reference, read)
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
        self.load_region_reference(reference, read)
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
        &self,
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

fn load_member(
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
