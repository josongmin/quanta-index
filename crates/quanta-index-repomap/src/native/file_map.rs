//! Native file/path inventory. Runtime graph, symbols and dependency edges are
//! separate producer projections; this adapter never infers them from bytes.

use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapRoot,
    MemberBinding, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef, OrdinalMapEdit,
    OwnerKey, PlanContext, PlanDecision, SourceChange, SourceDelta, SourceInputCursor,
    SourceRecord, SourceRecordOwner, StoredObject, TicketRecord, WorkBudget, WorkUsage,
    member_map_key as member_key, owner_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use serde::{Deserialize, Serialize, ser::SerializeStruct as _};
use sha2::{Digest as _, Sha256};
use std::{
    io::{self, Write},
    ops::Deref,
};

use super::scratch::{InventoryEditCursor, InventoryScratch};

const COMMIT_DOMAIN: &[u8] = b"quanta-index-native-filemap-v1\0";
const PATH_PREFIX: &[u8] = b"path\0";
const MAX_PATH_BYTES: usize = quanta_index_core::index::MAX_SOURCE_PATH_BYTES;
const MAX_PAGE: u32 = 4096;
const CONTRIBUTION_BATCH_COUNT: usize = 64;
const CONTRIBUTION_BATCH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFileMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMapCapability {
    FilePathInventoryV1,
}
quanta_index_contract::native_serde_enum!(FileMapCapability {
    units { FilePathInventoryV1 = 0 }
    newtypes {}
    records {}
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileMapRoot {
    capability: FileMapCapability,
    input: InputBinding,
    owners: MapRoot,
    paths: MapRoot,
}
quanta_index_contract::native_serde_struct!(FileMapRoot {
    capability: FileMapCapability,
    input: InputBinding,
    owners: MapRoot,
    paths: MapRoot,
});

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileContribution {
    owner: OwnerKey,
    source: ObjectRef,
    record: SourceRecord,
}
quanta_index_contract::native_serde_struct!(FileContribution {
    owner: OwnerKey,
    source: ObjectRef,
    record: SourceRecord,
});

struct LoadedContribution {
    entry: FileContribution,
    _memory: Option<MemoryLease>,
}

impl Deref for LoadedContribution {
    type Target = FileContribution;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

// Borrowed serialization view of the existing wire record, not another index IR.
struct ContributionWire<'a> {
    owner: OwnerKey,
    source: ObjectRef,
    record: &'a SourceRecord,
}

impl Serialize for ContributionWire<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("FileContribution", 3)?;
        fields.serialize_field("owner", &self.owner)?;
        fields.serialize_field("source", &self.source)?;
        fields.serialize_field("record", self.record)?;
        fields.end()
    }
}

struct PathKey {
    bytes: Vec<u8>,
    _memory: Option<MemoryLease>,
}

impl PathKey {
    fn new(path: &str, budget: &WorkBudget) -> Result<Self, IndexError> {
        validate_path(path)?;
        let len = PATH_PREFIX
            .len()
            .checked_add(path.len())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = query_memory(len, MemoryLifetime::Temporary, budget)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bytes.extend_from_slice(PATH_PREFIX);
        bytes.extend_from_slice(path.as_bytes());
        budget.checkpoint()?;
        Ok(Self {
            bytes,
            _memory: memory,
        })
    }
}

impl Deref for PathKey {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

struct FundedPayload {
    bytes: Vec<u8>,
    memory: Option<MemoryLease>,
}

struct ContributionWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    budget: &'a WorkBudget,
    error: Option<IndexError>,
}

impl Write for ContributionWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let checked = self.budget.checkpoint().and_then(|()| {
            self.bytes
                .len()
                .checked_add(bytes.len())
                .filter(|next| *next <= self.limit)
                .ok_or(IndexError::ResourceLimited)
        });
        if let Err(error) = checked {
            self.error = Some(error);
            return Err(io::Error::other(
                "file contribution exceeds admitted output",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_contribution(
    owner: OwnerKey,
    source: ObjectRef,
    record: &SourceRecord,
    bound: usize,
    budget: &WorkBudget,
) -> Result<FundedPayload, IndexError> {
    encode_bounded(
        &ContributionWire {
            owner,
            source,
            record,
        },
        bound,
        budget,
    )
}

fn encode_bounded<T: Serialize>(
    value: &T,
    bound: usize,
    budget: &WorkBudget,
) -> Result<FundedPayload, IndexError> {
    let memory = query_memory(bound, MemoryLifetime::Temporary, budget)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(bound)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut writer = ContributionWriter {
        bytes,
        limit: bound,
        budget,
        error: None,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return Err(writer.error.unwrap_or_else(|| {
            IndexError::BackendViolation(format!("file contribution JSON: {error}"))
        }));
    }
    budget.checkpoint()?;
    Ok(FundedPayload {
        bytes: writer.bytes,
        memory,
    })
}

struct ContributionBinding {
    owner: OrdinalMapEdit,
    path: PathKey,
    _payload_memory: Option<MemoryLease>,
    _children_memory: Option<MemoryLease>,
}

struct ContributionBatch {
    // Payload/children die before the guards retained by bindings.
    inputs: Vec<ObjectBatchInput>,
    bindings: Vec<ContributionBinding>,
    _memory: Option<MemoryLease>,
}

impl ContributionBatch {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = std::mem::size_of::<ObjectBatchInput>()
            .checked_add(std::mem::size_of::<ContributionBinding>())
            .and_then(|width| width.checked_mul(CONTRIBUTION_BATCH_COUNT))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = query_memory(bytes, MemoryLifetime::Temporary, budget)?;
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(CONTRIBUTION_BATCH_COUNT)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(CONTRIBUTION_BATCH_COUNT)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            inputs,
            bindings,
            _memory: memory,
        })
    }

    fn push(
        &mut self,
        owner: OrdinalMapEdit,
        path: PathKey,
        payload: FundedPayload,
        source: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if self.inputs.len() == CONTRIBUTION_BATCH_COUNT {
            return Err(IndexError::ResourceLimited);
        }
        let memory = query_memory(
            std::mem::size_of::<ObjectRef>(),
            MemoryLifetime::Temporary,
            budget,
        )?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        children.push(source);
        budget.checkpoint()?;
        self.inputs.push(ObjectBatchInput {
            payload: payload.bytes,
            children,
        });
        self.bindings.push(ContributionBinding {
            owner,
            path,
            _payload_memory: payload.memory,
            _children_memory: memory,
        });
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FileMapPlan {
    base: Option<MemberBinding>,
    context: PlanContext,
    input: DeltaInput<SourceDelta>,
}

#[derive(Clone, Debug)]
pub struct FileMapCandidate {
    binding: MemberBinding,
}

impl IndexBuildPort for NativeFileMap {
    type Delta = SourceDelta;
    type Plan = FileMapPlan;
    type Candidate = FileMapCandidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        quanta_index_core::index::effort::ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        if instance.activation != quanta_index_core::index::GroupActivation::CoreRequired
            || instance.input_domain != quanta_index_core::index::InputDomain::SourceLocal
        {
            return Err(IndexError::InvalidInput(
                "adapter requires a core source-local instance".into(),
            ));
        }
        Ok(())
    }

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<FileMapPlan>, IndexError> {
        budget.checkpoint()?;
        let (base, input) = match (base, input) {
            (BaseMember::Absent, DeltaInput::Bootstrap)
                if context.input.root == context.source.owners =>
            {
                (None, DeltaInput::Bootstrap)
            }
            (BaseMember::Present(member), DeltaInput::Patch(delta)) => {
                if member.instance != context.instance.instance
                    || member.format != context.instance.format
                    || member.input.schema != context.input.schema
                    || member.input.policy != context.input.policy
                    || member.input.root != delta.prior.owners
                    || delta.next != context.source
                    || context.input.root != delta.next.owners
                {
                    return Err(IndexError::Conflict);
                }
                (Some(*member), DeltaInput::Patch(delta))
            }
            _ => return Ok(PlanDecision::RebuildRequired),
        };
        Ok(PlanDecision::Incremental(FileMapPlan {
            base,
            context: context.clone(),
            input,
        }))
    }

    fn prepare<'a>(
        &'a self,
        plan: FileMapPlan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, FileMapCandidate> {
        Box::pin(async move {
            let mut root = match &plan.base {
                Some(base) => load_root(base, context.objects, context.budget)?,
                None => FileMapRoot {
                    capability: FileMapCapability::FilePathInventoryV1,
                    input: plan.context.input,
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                },
            };
            let patch_owner_roots = match &plan.input {
                DeltaInput::Patch(delta) => Some((delta.prior.owners, delta.next.owners)),
                DeltaInput::Bootstrap => None,
            };
            let mut changes = SourceInputCursor::new(
                plan.context.source,
                plan.input,
                context.maps,
                context.objects,
            )?;
            let first = changes.next_change(context.budget)?;
            if let Some(first) = first {
                let scratch = check_changes(first, &mut changes, root, context)?;
                stage_contributions(&scratch, context)?;
                let mut edits = InventoryEditCursor::new(&scratch);
                let roots = context.apply_stream(&[root.owners, root.paths], &mut edits)?;
                root.owners = *roots.first().ok_or_else(|| {
                    IndexError::BackendViolation("file inventory owner root missing".into())
                })?;
                root.paths = *roots.get(1).ok_or_else(|| {
                    IndexError::BackendViolation("file inventory path root missing".into())
                })?;
            } else {
                if patch_owner_roots.is_some_and(|(prior, next)| prior != next) {
                    return Err(IndexError::CorruptObject(
                        "empty file map patch changed owner root".into(),
                    ));
                }
                if let Some(base) = &plan.base
                    && root.input == plan.context.input
                {
                    return Ok(FileMapCandidate {
                        binding: base.clone(),
                    });
                }
            }
            root.input = plan.context.input;
            // This fixed-field schema has no strings or variable collections.
            let payload = encode_bounded(&root, 2048, context.budget)?;
            let children = root_children(root);
            let artifact = context.put(&payload.bytes, children.as_slice())?;
            Ok(FileMapCandidate {
                binding: MemberBinding {
                    instance: plan.context.instance.instance,
                    format: plan.context.instance.format,
                    artifact,
                    input: plan.context.input,
                    logical_commitment: commitment(&payload.bytes),
                },
            })
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: FileMapCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            let root = load_root(&candidate.binding, context.objects, context.budget)?;
            if root.input != candidate.binding.input {
                return Err(IndexError::BackendViolation(
                    "file map input changed after prepare".into(),
                ));
            }
            Ok(candidate.binding)
        })
    }
}

fn check_changes(
    first: SourceChange,
    changes: &mut SourceInputCursor<'_>,
    root: FileMapRoot,
    context: &mut AttemptContext<'_>,
) -> Result<InventoryScratch, IndexError> {
    let scratch = InventoryScratch::open(context.scratch, context.budget)?;
    let mut previous = None;
    let mut next_change = Some(first);
    while let Some(change) = next_change {
        context.budget.checkpoint()?;
        if previous.is_some_and(|owner| owner >= change.owner) {
            return Err(IndexError::InvalidInput(
                "file map owners must be unique and sorted".into(),
            ));
        }
        previous = Some(change.owner);
        let prior_contribution = context.maps.lookup_object(
            root.owners,
            &owner_map_key(change.owner),
            context.budget,
        )?;
        let prior = prior_contribution
            .map(|reference| load_contribution(reference, context.objects, context.budget))
            .transpose()?;
        if prior.as_ref().map(|entry| (entry.source, &entry.record))
            != change
                .prior
                .as_ref()
                .map(|(reference, record)| (*reference, &**record))
            || prior
                .as_ref()
                .is_some_and(|entry| entry.owner != change.owner)
        {
            return Err(IndexError::Conflict);
        }
        let prior_path = if let Some(entry) = &prior {
            context.budget.charge(WorkUsage {
                bytes: u64::try_from(
                    entry
                        .record
                        .path
                        .len()
                        .checked_add(PATH_PREFIX.len())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            Some(PathKey::new(&entry.record.path, context.budget)?)
        } else {
            None
        };
        if let (Some(path), Some(reference)) = (&prior_path, prior_contribution) {
            if context
                .maps
                .lookup_object(root.paths, path, context.budget)?
                != Some(reference)
            {
                return Err(IndexError::CorruptObject(
                    "file owner/path reverse entry mismatch".into(),
                ));
            }
            scratch.record_prior_path(path, reference, context.budget)?;
        }
        // The cursor authenticated each source record against the sealed
        // mutation manifest and both source roots before yielding it.
        if let Some((_, record)) = &change.next {
            context.budget.charge(WorkUsage {
                bytes: u64::try_from(
                    record
                        .path
                        .len()
                        .checked_add(PATH_PREFIX.len())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let path = PathKey::new(&record.path, context.budget)?;
            scratch.record_next_path(&path, context.budget)?;
        }
        scratch.record_owner(
            change.owner,
            prior_contribution,
            change.next.map(|(source, _record)| source),
            context.budget,
        )?;
        next_change = changes.next_change(context.budget)?;
    }
    scratch.validate_paths(root.paths, context.maps, context.budget)?;
    Ok(scratch)
}

fn stage_contributions(
    scratch: &InventoryScratch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut after: Option<OrdinalMapEdit> = None;
    let mut batch = ContributionBatch::new(context.budget)?;
    let mut batch_bytes = 0usize;
    while let Some(row) = scratch.next_owner(
        after
            .as_ref()
            .map(|position: &OrdinalMapEdit| position.edit.key.as_slice()),
        context.budget,
    )? {
        context.budget.checkpoint()?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(row.key.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        after = Some(row.position.clone());
        let Some(source) = row.next_source else {
            continue;
        };
        let owner = InventoryScratch::owner_from_key(&row.key)?;
        let record = read_source(source, context.objects, context.budget)?;
        let bound = record
            .path
            .len()
            .checked_mul(6)
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or(IndexError::ResourceLimited)?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(bound).map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let size = bound.checked_add(32).ok_or(IndexError::ResourceLimited)?;
        if size > CONTRIBUTION_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        if batch.inputs.len() == CONTRIBUTION_BATCH_COUNT
            || batch_bytes
                .checked_add(size)
                .is_none_or(|total| total > CONTRIBUTION_BATCH_BYTES)
        {
            flush_contributions(scratch, &mut batch, context)?;
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes
            .checked_add(size)
            .ok_or(IndexError::ResourceLimited)?;
        let path = PathKey::new(&record.path, context.budget)?;
        let payload = encode_contribution(owner, source, &record, bound, context.budget)?;
        batch.push(row.position, path, payload, source, context.budget)?;
    }
    flush_contributions(scratch, &mut batch, context)
}

fn flush_contributions(
    scratch: &InventoryScratch,
    batch: &mut ContributionBatch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if batch.inputs.is_empty() {
        return Ok(());
    }
    let references = context.put_batch(&batch.inputs)?;
    if references.len() != batch.bindings.len() {
        return Err(IndexError::BackendViolation(
            "file contributions lost batch ordinal".into(),
        ));
    }
    for (binding, reference) in batch.bindings.iter().zip(references) {
        scratch.set_next(
            &binding.owner.edit.key,
            &binding.path,
            reference,
            context.budget,
        )?;
    }
    batch.inputs.clear();
    batch.bindings.clear();
    Ok(())
}

fn read_source(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecordOwner, IndexError> {
    let stored = objects.get(reference, budget)?;
    let record =
        SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Temporary)?;
    validate_source(&record, reference, &stored)?;
    Ok(record)
}

fn load_contribution(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedContribution, IndexError> {
    let stored = objects.get(reference, budget)?;
    let memory = decoded_memory(&stored.payload, budget)?;
    let entry: FileContribution = decode(&stored.payload)?;
    validate_contribution(&entry, reference, &stored)?;
    Ok(LoadedContribution {
        entry,
        _memory: memory,
    })
}

fn validate_source(
    record: &SourceRecord,
    reference: ObjectRef,
    stored: &StoredObject,
) -> Result<(), IndexError> {
    validate_path(&record.path)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != [record.bytes]
    {
        return Err(IndexError::CorruptObject(
            "file source record edges mismatch".into(),
        ));
    }
    Ok(())
}

fn validate_contribution(
    entry: &FileContribution,
    reference: ObjectRef,
    stored: &StoredObject,
) -> Result<(), IndexError> {
    validate_path(&entry.record.path)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != [entry.source]
    {
        return Err(IndexError::CorruptObject(
            "file contribution edges mismatch".into(),
        ));
    }
    Ok(())
}

fn load_root(
    binding: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<FileMapRoot, IndexError> {
    let stored = objects.get(binding.artifact, budget)?;
    let _memory = decoded_memory(&stored.payload, budget)?;
    let root: FileMapRoot = decode(&stored.payload)?;
    if stored.registration.reference != binding.artifact
        || stored.registration.children.as_slice() != root_children(root).as_slice()
        || root.input != binding.input
        || root.capability != FileMapCapability::FilePathInventoryV1
        || commitment(&stored.payload) != binding.logical_commitment
    {
        return Err(IndexError::CorruptObject(
            "file map root binding mismatch".into(),
        ));
    }
    Ok(root)
}

enum RootChildren {
    Empty,
    One([ObjectRef; 1]),
    Two([ObjectRef; 2]),
    Three([ObjectRef; 3]),
}

impl RootChildren {
    fn as_slice(&self) -> &[ObjectRef] {
        match self {
            Self::Empty => &[],
            Self::One(values) => values.as_slice(),
            Self::Two(values) => values.as_slice(),
            Self::Three(values) => values.as_slice(),
        }
    }
}

fn root_children(root: FileMapRoot) -> RootChildren {
    // Preserve the existing edge order and aliases in immutable object identity.
    match [root.owners.0, root.paths.0, root.input.root.0] {
        [None, None, None] => RootChildren::Empty,
        [Some(a), None, None] | [None, Some(a), None] | [None, None, Some(a)] => {
            RootChildren::One([a])
        }
        [Some(a), Some(b), None] | [Some(a), None, Some(b)] | [None, Some(a), Some(b)] => {
            RootChildren::Two([a, b])
        }
        [Some(a), Some(b), Some(c)] => RootChildren::Three([a, b, c]),
    }
}

#[cfg(test)]
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, IndexError> {
    serde_json::from_slice(payload).map_err(|error| IndexError::CorruptObject(error.to_string()))
}

fn commitment(payload: &[u8]) -> Digest {
    let mut hash = Sha256::new();
    hash.update(COMMIT_DOMAIN);
    hash.update(payload);
    Digest(hash.finalize().into())
}

pub(super) fn validate_path(path: &str) -> Result<(), IndexError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(IndexError::InvalidInput("noncanonical file path".into()));
    }
    Ok(())
}

#[cfg(test)]
fn path_key(path: &str) -> Result<Vec<u8>, IndexError> {
    validate_path(path)?;
    let mut key = Vec::with_capacity(PATH_PREFIX.len().saturating_add(path.len()));
    key.extend_from_slice(PATH_PREFIX);
    key.extend_from_slice(path.as_bytes());
    Ok(key)
}

pub struct FileMapReadContext<'a> {
    pub ticket: &'a TicketRecord,
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub budget: &'a mut WorkBudget,
}

pub struct FilePathQuery<'a> {
    pub member: &'a MemberBinding,
    pub prefix: &'a str,
    /// Exclusive path cursor returned by the preceding page.
    pub after: Option<&'a str>,
    pub limit: u32,
}

#[derive(Debug)]
pub struct FilePathEntry {
    pub owner: OwnerKey,
    pub path: String,
    pub source: ObjectRef,
    pub bytes: ObjectRef,
    pub byte_len: u64,
    path_memory: Option<MemoryLease>,
}

impl FilePathEntry {
    /// The producing owner admits the path before transferring its `String`.
    #[must_use]
    pub fn from_parts(
        owner: OwnerKey,
        path: String,
        source: ObjectRef,
        bytes: ObjectRef,
        byte_len: u64,
        memory: Option<MemoryLease>,
    ) -> Self {
        Self {
            owner,
            path,
            source,
            bytes,
            byte_len,
            path_memory: memory,
        }
    }
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        OwnerKey,
        String,
        ObjectRef,
        ObjectRef,
        u64,
        Option<MemoryLease>,
    ) {
        (
            self.owner,
            self.path,
            self.source,
            self.bytes,
            self.byte_len,
            self.path_memory,
        )
    }
}
impl PartialEq for FilePathEntry {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.path == other.path
            && self.source == other.source
            && self.bytes == other.bytes
            && self.byte_len == other.byte_len
    }
}
impl Eq for FilePathEntry {}

#[derive(Debug)]
pub struct FilePathPage {
    pub entries: Vec<FilePathEntry>,
    pub has_more: bool,
    entry_memory: Option<MemoryLease>,
}
impl FilePathPage {
    /// The producer admits the backing before constructing an escaping page.
    #[must_use]
    pub fn from_parts(
        entries: Vec<FilePathEntry>,
        has_more: bool,
        memory: Option<MemoryLease>,
    ) -> Self {
        Self {
            entries,
            has_more,
            entry_memory: memory,
        }
    }
    #[must_use]
    pub fn into_parts(self) -> (Vec<FilePathEntry>, bool, Option<MemoryLease>) {
        (self.entries, self.has_more, self.entry_memory)
    }
}
impl PartialEq for FilePathPage {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries && self.has_more == other.has_more
    }
}
impl Eq for FilePathPage {}

/// Reads a pinned historical file/path inventory. Prefix inclusion uses one
/// exact lookup, then the map's exclusive lower bound; no predecessor walk.
pub fn query_paths(
    query: &FilePathQuery<'_>,
    context: &mut FileMapReadContext<'_>,
) -> Result<FilePathPage, IndexError> {
    if query.limit == 0 || query.limit > MAX_PAGE {
        return Err(IndexError::InvalidInput(
            "file path page limit out of bounds".into(),
        ));
    }
    validate_prefix(query.prefix)?;
    if let Some(after) = query.after {
        validate_path(after)?;
        if !after.starts_with(query.prefix) {
            return Err(IndexError::InvalidInput(
                "file path cursor differs from prefix".into(),
            ));
        }
    }
    context.budget.checkpoint()?;
    let cursor_bytes = query.after.map_or(0usize, str::len);
    context.budget.charge(WorkUsage {
        bytes: u64::try_from(
            PATH_PREFIX
                .len()
                .saturating_add(query.prefix.len())
                .saturating_add(cursor_bytes),
        )
        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let member_ref = context
        .maps
        .lookup_object(
            context.ticket.view.members,
            &member_key(query.member.instance),
            context.budget,
        )?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored_member = context.objects.get(member_ref, context.budget)?;
    let published =
        quanta_index_core::index::decode_member_binding(&stored_member.payload, context.budget)?;
    let expected_member_children = [Some(query.member.artifact), query.member.input.root.0];
    if published != *query.member
        || stored_member.registration.reference != member_ref
        || !stored_member
            .registration
            .children
            .iter()
            .copied()
            .eq(expected_member_children.into_iter().flatten())
        || context.ticket.view.source.owners != query.member.input.root
    {
        return Err(IndexError::CorruptObject(
            "historical file member mismatch".into(),
        ));
    }
    let root = load_root(query.member, context.objects, context.budget)?;
    // Prefix and reusable continuation backing are bounded by the persisted
    // map-key envelope. Fund both before either owned key is born.
    let key_capacity = PATH_PREFIX
        .len()
        .checked_add(MAX_PATH_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let _key_memory = query_memory(
        key_capacity
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
        context.budget,
    )?;
    let prefix_key = path_prefix_key(query.prefix)?;
    let limit = usize::try_from(query.limit).map_err(|_error| IndexError::ResourceLimited)?;
    let entry_memory = query_memory(
        limit
            .checked_mul(std::mem::size_of::<FilePathEntry>())
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
        context.budget,
    )?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(limit)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut result = FilePathPage::from_parts(entries, false, entry_memory);
    let mut cursor = Vec::new();
    cursor
        .try_reserve_exact(key_capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if let Some(after) = query.after {
        cursor.extend_from_slice(PATH_PREFIX);
        cursor.extend_from_slice(after.as_bytes());
        let reference = context
            .maps
            .lookup_object(root.paths, &cursor, context.budget)?
            .ok_or_else(|| IndexError::InvalidInput("file path cursor is absent".into()))?;
        let _validated_cursor = read_path_entry(reference, after, root.owners, context)?;
    } else {
        if !query.prefix.is_empty()
            && validate_path(query.prefix).is_ok()
            && let Some(reference) =
                context
                    .maps
                    .lookup_object(root.paths, &prefix_key, context.budget)?
        {
            result.entries.push(read_path_entry(
                reference,
                query.prefix,
                root.owners,
                context,
            )?);
        }
        cursor.extend_from_slice(&prefix_key);
    }
    loop {
        context.budget.checkpoint()?;
        let remaining = limit.saturating_sub(result.entries.len());
        let scan_limit = u32::try_from(remaining.max(1))
            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
        let page = context
            .maps
            .scan(root.paths, Some(&cursor), scan_limit, context.budget)?;
        if page.entries.is_empty() {
            if page.has_more {
                return Err(IndexError::BackendViolation(
                    "file map scan made no progress".into(),
                ));
            }
            return Ok(result);
        }
        for (key, reference) in &page.entries {
            if !key.starts_with(&prefix_key) {
                return Ok(result);
            }
            if result.entries.len() == limit {
                result.has_more = true;
                return Ok(result);
            }
            let path =
                std::str::from_utf8(key.get(PATH_PREFIX.len()..).ok_or_else(|| {
                    IndexError::CorruptObject("file path key is truncated".into())
                })?)
                .map_err(|_conversion_error| {
                    IndexError::CorruptObject("file path key is not UTF-8".into())
                })?;
            result.entries.push(read_path_entry(
                reference.object_ref()?,
                path,
                root.owners,
                context,
            )?);
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("file map scan omitted continuation".into())
        })?;
        if result.entries.len() == limit {
            // A later key may be outside this prefix. One bounded lookahead
            // prevents declaring a false continuation at the range boundary.
            let probe = context
                .maps
                .scan(root.paths, Some(&last.0), 1, context.budget)?;
            result.has_more = probe
                .entries
                .first()
                .is_some_and(|(key, _)| key.starts_with(&prefix_key));
            return Ok(result);
        }
        if !page.has_more {
            return Ok(result);
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(last.0.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if last.0.len() > key_capacity {
            return Err(IndexError::CorruptObject(
                "file path cursor exceeds map-key envelope".into(),
            ));
        }
        cursor.clear();
        cursor.extend_from_slice(&last.0);
    }
}

fn read_path_entry(
    reference: ObjectRef,
    expected_path: &str,
    owners: MapRoot,
    context: &mut FileMapReadContext<'_>,
) -> Result<FilePathEntry, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    let _contribution_memory = decoded_memory(&stored.payload, context.budget)?;
    let contribution: FileContribution = decode(&stored.payload)?;
    validate_contribution(&contribution, reference, &stored)?;
    if contribution.record.path != expected_path {
        return Err(IndexError::CorruptObject(
            "file path key/value mismatch".into(),
        ));
    }
    let stored_source = context.objects.get(contribution.source, context.budget)?;
    let _source_memory = decoded_memory(&stored_source.payload, context.budget)?;
    let source: SourceRecord = decode(&stored_source.payload)?;
    validate_source(&source, contribution.source, &stored_source)?;
    if source != contribution.record {
        return Err(IndexError::CorruptObject(
            "file contribution/source mismatch".into(),
        ));
    }
    if context
        .maps
        .lookup_object(owners, &owner_map_key(contribution.owner), context.budget)?
        != Some(reference)
    {
        return Err(IndexError::CorruptObject(
            "file owner/path reverse mismatch".into(),
        ));
    }
    let memory = query_memory(
        contribution.record.path.capacity(),
        MemoryLifetime::Retained,
        context.budget,
    )?;
    Ok(FilePathEntry::from_parts(
        contribution.owner,
        contribution.record.path,
        contribution.source,
        contribution.record.bytes,
        contribution.record.byte_len,
        memory,
    ))
}

fn query_memory(
    bytes: usize,
    lifetime: MemoryLifetime,
    budget: &WorkBudget,
) -> Result<Option<MemoryLease>, IndexError> {
    if bytes == 0 {
        return Ok(None);
    }
    budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        lifetime,
    )
}

fn decoded_memory(bytes: &[u8], budget: &WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    query_memory(
        bytes
            .len()
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
        budget,
    )
}

pub(super) fn validate_prefix(prefix: &str) -> Result<(), IndexError> {
    if prefix.len() > MAX_PATH_BYTES || prefix.starts_with('/') || prefix.contains('\0') {
        return Err(IndexError::InvalidInput("invalid file path prefix".into()));
    }
    Ok(())
}

fn path_prefix_key(prefix: &str) -> Result<Vec<u8>, IndexError> {
    validate_prefix(prefix)?;
    let mut key = Vec::with_capacity(PATH_PREFIX.len().saturating_add(prefix.len()));
    key.extend_from_slice(PATH_PREFIX);
    key.extend_from_slice(prefix.as_bytes());
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        future::Future,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
        task::{Context, Poll, Waker},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        AttemptId, AttemptRef, CatalogView, CollectionId, GroupActivation, IndexInstanceId,
        IndexKind, InputDomain, InstanceSpec, MapEdit, MapPage, MapWrite, OperationKey,
        OwnerMutationDigest, OwnerMutationSet, ProducerId, PublicationGroupId, RegisteredObject,
        SchemaId, ScratchAllocation, ScratchRequest, ScratchSpacePort, SlotId, SourceCut,
        StableOwnerId, StorageFormatId, StoredObject, StreamId, TenantId, TicketId, ViewRef,
        WorkLimits, WorkspaceKey, WorktreeId,
        ports::{ObjectCustodyPort, ObjectWriteSink},
    };

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(format!($($message)+).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}").into());
                }
                _ => {}
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}: {}", format_args!($($message)+)).into());
                }
                _ => {}
            }
        }};
    }
    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(format!("assertion `left != right` failed\n left: {left:?}\n right: {right:?}").into());
                }
                _ => {}
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(format!("assertion `left != right` failed\n left: {left:?}\n right: {right:?}: {}", format_args!($($message)+)).into());
                }
                _ => {}
            }
        }};
    }

    #[derive(Default)]
    struct TestObjects(Mutex<BTreeMap<ObjectRef, quanta_index_core::index::StoredObject>>);

    impl TestObjects {
        fn add(&self, payload: Vec<u8>, children: Vec<ObjectRef>) -> Result<ObjectRef, IndexError> {
            let mut hash = Sha256::new();
            hash.update(b"filemap-test-object\0");
            hash.update(&payload);
            for child in &children {
                hash.update((child.0).0);
            }
            let reference = ObjectRef(Digest(hash.finalize().into()));
            let stored = StoredObject::new(
                RegisteredObject::unscoped(
                    reference,
                    u64::try_from(payload.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    children,
                ),
                payload,
                None,
            );
            let mut values = self
                .0
                .lock()
                .map_err(|_conversion_error| IndexError::Storage("test lock".into()))?;
            drop(values.insert(reference, stored));
            drop(values);
            Ok(reference)
        }
    }

    impl ImmutableObjectPort for TestObjects {
        fn install_batch(
            &self,
            _: &[quanta_index_core::index::ObjectBatchInput],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test batch install".into(),
            ))
        }
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test identification".into(),
            ))
        }
        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected test write".into()))
        }
        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test install".into(),
            ))
        }
        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let values = self
                .0
                .lock()
                .map_err(|_conversion_error| IndexError::Storage("test lock".into()))?;
            values
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("missing test object".into()))
        }
    }

    struct PatchObjects {
        base: TestObjects,
        installs: AtomicUsize,
    }

    impl ImmutableObjectPort for PatchObjects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test identification".into(),
            ))
        }

        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected test write".into()))
        }

        fn install(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            sink: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let _prior = self.installs.fetch_add(1, Ordering::Relaxed);
            let mut hash = Sha256::new();
            hash.update(b"filemap-test-object\0");
            hash.update(payload);
            for child in children {
                hash.update((child.0).0);
            }
            let registration = RegisteredObject::unscoped(
                ObjectRef(Digest(hash.finalize().into())),
                u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                children.to_vec(),
            );
            sink.reserve(&registration)?;
            let installed = self.base.add(payload.to_vec(), children.to_vec())?;
            if installed != registration.reference {
                return Err(IndexError::BackendViolation(
                    "test object identity drift".into(),
                ));
            }
            sink.installed(&registration)?;
            Ok(registration)
        }

        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test batch install".into(),
            ))
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            self.base.get(reference, budget)
        }
    }

    #[derive(Default)]
    struct NoScratch {
        allocations: AtomicUsize,
    }

    impl ScratchSpacePort for NoScratch {
        fn allocate(
            &self,
            _: ScratchRequest,
            _: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            let _prior = self.allocations.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected scratch allocation".into(),
            ))
        }
    }

    struct NoCustody;

    impl ObjectCustodyPort for NoCustody {
        fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }

        fn register_objects(
            &self,
            _: AttemptRef,
            _: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn ready<T>(mut future: IndexFuture<'_, T>) -> Result<T, IndexError> {
        let mut context = Context::from_waker(Waker::noop());
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(IndexError::BackendViolation(
                "file map test future unexpectedly pending".into(),
            )),
        }
    }

    #[derive(Default)]
    struct TestMaps(
        BTreeMap<ObjectRef, BTreeMap<Vec<u8>, ObjectRef>>,
        Option<(MapRoot, MapRoot)>,
    );

    impl TestMaps {
        fn add(&mut self, root: ObjectRef, key: Vec<u8>, value: ObjectRef) {
            let _old = self.0.entry(root).or_default().insert(key, value);
        }
    }

    impl PersistentMapPort for TestMaps {
        fn visit_diff(
            &self,
            before: MapRoot,
            after: MapRoot,
            range: quanta_index_core::index::ports::MapDiffRange<'_>,
            _: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
            budget: &mut WorkBudget,
        ) -> Result<bool, IndexError> {
            budget.checkpoint()?;
            if self.1 == Some((before, after))
                && range.prefix.is_empty()
                && range.after.is_none()
                && range.limit == 64
            {
                return Ok(false);
            }
            Err(IndexError::BackendViolation(
                "unexpected test structural diff".into(),
            ))
        }

        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn quanta_index_core::index::ports::MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test stream map write".into(),
            ))
        }
        fn apply_batch(
            &self,
            _: &[quanta_index_core::index::MapBatchRequest],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test batch map write".into(),
            ))
        }
        fn lookup_object(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            Ok(root
                .0
                .and_then(|reference| self.0.get(&reference))
                .and_then(|map| map.get(key))
                .copied())
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let Some(map) = root.0.and_then(|reference| self.0.get(&reference)) else {
                return Ok(MapPage::new(vec![], false, Vec::new(), None));
            };
            let matches: Vec<_> = map
                .iter()
                .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                .map(|(key, value)| (key.clone(), *value))
                .collect();
            let limit =
                usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let has_more = matches.len() > limit;
            Ok(MapPage::new(
                matches
                    .into_iter()
                    .take(limit)
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
                has_more,
                Vec::new(),
                None,
            ))
        }
        fn apply(
            &self,
            _base: MapRoot,
            _edits: &[MapEdit],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("unexpected test apply".into()))
        }
    }

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }
    fn owner(byte: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([byte; 16]),
        }
    }
    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 10_000_000,
                mutations: 1000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    struct BuildMemory {
        cap: AtomicU64,
        live: AtomicU64,
        invalid: AtomicBool,
        cancel: Option<quanta_index_core::RequestBudgetV1>,
    }

    impl quanta_index_core::index::MemoryLeaseOwner for BuildMemory {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid.store(true, Ordering::SeqCst);
            }
        }
    }

    struct BuildScope(Arc<BuildMemory>);

    impl quanta_index_core::index::MemoryScopePort for BuildScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "build-local allocation escaped its stage".into(),
                ));
            }
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.cap.load(Ordering::SeqCst))
                })
                .map_err(|_live| IndexError::ResourceLimited)?;
            if let Some(request) = &self.0.cancel {
                request.cancel_handle().cancel();
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn build_budget(cap: u64, cancel: bool) -> Result<(WorkBudget, Arc<BuildMemory>), IndexError> {
        let mut work = budget()?;
        let memory = Arc::new(BuildMemory {
            cap: AtomicU64::new(cap),
            live: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
            cancel: cancel.then(|| work.request().clone()),
        });
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            BuildScope(memory.clone()),
        ))));
        Ok((work, memory))
    }

    fn contribution_record(path: &str) -> SourceRecord {
        SourceRecord {
            path: path.into(),
            bytes: reference(255),
            byte_len: u64::MAX,
            text: quanta_index_core::index::SourceTextAdmission::Utf8 {
                policy: Digest([255; 32]),
            },
        }
    }

    #[test]
    fn build_record_denial_precedes_parse_and_cancel_or_edges_refund()
    -> Result<(), Box<dyn std::error::Error>> {
        let objects = TestObjects::default();
        let malformed = objects.add(b"{".to_vec(), vec![])?;
        for reader in [false, true] {
            for (cap, cancel) in [(0, false), (8, false), (8, true)] {
                let (mut work, memory) = build_budget(cap, cancel)?;
                let result = if reader {
                    load_contribution(malformed, &objects, &mut work).map(|_record| ())
                } else {
                    read_source(malformed, &objects, &mut work).map(|_record| ())
                };
                if cap == 0 {
                    check!(matches!(result, Err(IndexError::ResourceLimited)));
                } else if cancel {
                    check!(matches!(result, Err(IndexError::Cancelled)));
                } else {
                    check!(matches!(result, Err(IndexError::CorruptObject(_))));
                }
                check_eq!(memory.live.load(Ordering::SeqCst), 0);
                check!(!memory.invalid.load(Ordering::SeqCst));
            }
        }
        let record = contribution_record("src/a.rs");
        let source = objects.add(encode(&record)?, vec![])?;
        let contribution = objects.add(
            encode(&FileContribution {
                owner: owner(1),
                source,
                record,
            })?,
            vec![],
        )?;
        let (mut work, memory) = build_budget(u64::MAX, false)?;
        check!(matches!(
            read_source(source, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            load_contribution(contribution, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn build_records_hold_both_path_charges_until_each_last_use()
    -> Result<(), Box<dyn std::error::Error>> {
        let objects = TestObjects::default();
        let record = contribution_record("src/\"源.rs");
        let source_wire = encode(&record)?;
        let source = objects.add(source_wire.clone(), vec![record.bytes])?;
        let contribution_wire = encode(&FileContribution {
            owner: owner(1),
            source,
            record,
        })?;
        let contribution = objects.add(contribution_wire.clone(), vec![source])?;
        let (mut work, memory) = build_budget(u64::MAX, false)?;
        let old = read_source(source, &objects, &mut work)?;
        let next = load_contribution(contribution, &objects, &mut work)?;
        check_eq!(old.path.as_str(), "src/\"源.rs");
        check_eq!(next.record.path, old.path);
        check_eq!(
            memory.live.load(Ordering::SeqCst),
            u64::try_from(
                source_wire
                    .len()
                    .checked_add(contribution_wire.len())
                    .ok_or("wire length")?
            )?
            .checked_mul(8)
            .ok_or("decode charge")?
        );
        drop(old);
        check_eq!(
            memory.live.load(Ordering::SeqCst),
            u64::try_from(contribution_wire.len())?
                .checked_mul(8)
                .ok_or("decode charge")?
        );
        drop(next);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn borrowed_build_encoding_preserves_wire_and_refuses_bounded_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let record = contribution_record(&"\u{1}".repeat(MAX_PATH_BYTES));
        let expected = encode(&FileContribution {
            owner: owner(255),
            source: reference(255),
            record: record.clone(),
        })?;
        let bound = record
            .path
            .len()
            .checked_mul(6)
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or("bound")?;
        let (work, memory) = build_budget(u64::MAX, false)?;
        let payload = encode_contribution(owner(255), reference(255), &record, bound, &work)?;
        check_eq!(payload.bytes, expected);
        check_eq!(memory.live.load(Ordering::SeqCst), u64::try_from(bound)?);
        drop(payload);
        check!(matches!(
            encode_contribution(
                owner(255),
                reference(255),
                &record,
                expected.len().checked_sub(1).ok_or("wire")?,
                &work
            ),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        let (denied, memory) = build_budget(0, false)?;
        check!(matches!(
            PathKey::new("src/a", &denied),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            ContributionBatch::new(&denied),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            encode_contribution(owner(1), reference(1), &record, bound, &denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        Ok(())
    }

    fn batch_position() -> OrdinalMapEdit {
        OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: owner_map_key(owner(1)).to_vec(),
                expected: None,
                next: None,
            },
        )
    }

    #[test]
    fn batch_child_refusal_keeps_old_payload_and_refunds_new_preparation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (work, memory) = build_budget(u64::MAX, false)?;
        let mut batch = ContributionBatch::new(&work)?;
        let record = contribution_record("a.rs");
        let first = encode_contribution(owner(1), reference(1), &record, 2048, &work)?;
        let expected = first.bytes.clone();
        let path = PathKey::new(&record.path, &work)?;
        batch.push(batch_position(), path, first, reference(1), &work)?;
        let retained = memory.live.load(Ordering::SeqCst);
        let next = encode_contribution(owner(1), reference(1), &record, 2048, &work)?;
        let path = PathKey::new(&record.path, &work)?;
        memory
            .cap
            .store(memory.live.load(Ordering::SeqCst), Ordering::SeqCst);
        check!(matches!(
            batch.push(batch_position(), path, next, reference(1), &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(batch.inputs.len(), 1);
        check_eq!(batch.inputs.first().ok_or("first input")?.payload, expected);
        check_eq!(memory.live.load(Ordering::SeqCst), retained);
        drop(batch);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn fixed_root_encoding_preserves_edge_order_and_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_objects, _maps, _ticket, binding) = fixture()?;
        let root = FileMapRoot {
            capability: FileMapCapability::FilePathInventoryV1,
            input: InputBinding {
                root: MapRoot(Some(reference(2))),
                ..binding.input
            },
            owners: MapRoot(Some(reference(2))),
            paths: MapRoot(Some(reference(1))),
        };
        check_eq!(
            root_children(root).as_slice(),
            &[reference(2), reference(1), reference(2)]
        );
        let (work, memory) = build_budget(u64::MAX, false)?;
        let payload = encode_bounded(&root, 2048, &work)?;
        check_eq!(payload.bytes, encode(&root)?);
        drop(payload);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        Ok(())
    }

    fn fixture()
    -> Result<(TestObjects, TestMaps, TicketRecord, MemberBinding), Box<dyn std::error::Error>>
    {
        let objects = TestObjects::default();
        let mut maps = TestMaps::default();
        let owners = MapRoot(Some(reference(1)));
        let paths = MapRoot(Some(reference(2)));
        let source_root = MapRoot(Some(reference(3)));
        let members = MapRoot(Some(reference(4)));
        for (id, path) in [(1u8, "a"), (2, "ab"), (3, "abc"), (4, "b")] {
            let bytes = objects.add(vec![id], vec![])?;
            let record = SourceRecord {
                path: path.into(),
                bytes,
                byte_len: 1,
                text: quanta_index_core::index::SourceTextAdmission::Excluded,
            };
            let source = objects.add(encode(&record)?, vec![bytes])?;
            let contribution = FileContribution {
                owner: owner(id),
                source,
                record,
            };
            let contribution_ref = objects.add(encode(&contribution)?, vec![source])?;
            maps.add(
                owners.0.ok_or("missing owners root")?,
                owner_map_key(owner(id)).to_vec(),
                contribution_ref,
            );
            maps.add(
                paths.0.ok_or("missing paths root")?,
                path_key(path)?,
                contribution_ref,
            );
        }
        let input = InputBinding {
            root: source_root,
            schema: SchemaId([5; 16]),
            policy: Digest([6; 32]),
            context: Digest([7; 32]),
        };
        let root = FileMapRoot {
            capability: FileMapCapability::FilePathInventoryV1,
            input,
            owners,
            paths,
        };
        let payload = encode(&root)?;
        let artifact = objects.add(payload.clone(), root_children(root).as_slice().to_vec())?;
        let binding = MemberBinding {
            instance: IndexInstanceId([8; 16]),
            format: StorageFormatId([9; 16]),
            artifact,
            input,
            logical_commitment: commitment(&payload),
        };
        let member_ref = objects.add(
            encode(&binding)?,
            vec![artifact, source_root.0.ok_or("missing source root")?],
        )?;
        let mut member_key = b"member\0".to_vec();
        member_key.extend_from_slice(&binding.instance.0);
        maps.add(
            members.0.ok_or("missing members root")?,
            member_key,
            member_ref,
        );
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([1; 16]),
            worktree: WorktreeId([1; 16]),
            slot: SlotId([1; 16]),
        };
        let view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([2; 32]),
            },
            profile: reference(5),
            source: SourceCut {
                owners: source_root,
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            members,
            coverage: MapRoot(None),
            projections: MapRoot(None),
        };
        Ok((
            objects,
            maps,
            TicketRecord {
                // Direct adapter fixture; catalog lease validation is covered
                // by the acquired consumer path.
                lease: quanta_index_core::index::ReadTicketLease {
                    id: TicketId([3; 16]),
                    session: quanta_index_core::index::ReadSessionRef { id: 1, fence: 2 },
                    fence: 3,
                    expires_at_ms: 4,
                },
                view,
            },
            binding,
        ))
    }

    #[test]
    fn pinned_member_denial_precedes_parse_and_original_cancel_refunds()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, mut maps, ticket, member) = fixture()?;
        let malformed = objects.add(b"{".to_vec(), Vec::new())?;
        maps.add(
            ticket.view.members.0.ok_or("missing members root")?,
            member_key(member.instance).to_vec(),
            malformed,
        );
        for (cap, cancel) in [(0, false), (u64::MAX, false), (u64::MAX, true)] {
            let (mut work, memory) = build_budget(cap, cancel)?;
            let result = query_paths(
                &FilePathQuery {
                    member: &member,
                    prefix: "a",
                    after: None,
                    limit: 1,
                },
                &mut FileMapReadContext {
                    ticket: &ticket,
                    objects: &objects,
                    maps: &maps,
                    budget: &mut work,
                },
            );
            if cap == 0 {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
            } else if cancel {
                check!(matches!(result, Err(IndexError::Cancelled)));
            } else {
                check!(matches!(result, Err(IndexError::CorruptObject(_))));
            }
            check_eq!(memory.live.load(Ordering::SeqCst), 0);
            check!(!memory.invalid.load(Ordering::SeqCst));
        }
        Ok(())
    }

    struct EmptyPatchFixture {
        objects: PatchObjects,
        maps: TestMaps,
        ticket: TicketRecord,
        member: MemberBinding,
        scratch: NoScratch,
        custody: NoCustody,
        work: WorkBudget,
    }

    impl EmptyPatchFixture {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let (objects, maps, ticket, member) = fixture()?;
            Ok(Self {
                objects: PatchObjects {
                    base: objects,
                    installs: AtomicUsize::new(0),
                },
                maps,
                ticket,
                member,
                scratch: NoScratch::default(),
                custody: NoCustody,
                work: budget()?,
            })
        }

        fn plan(
            &mut self,
            next: SourceCut,
            input: InputBinding,
            digest: Digest,
        ) -> Result<FileMapPlan, IndexError> {
            let context = PlanContext {
                instance: InstanceSpec {
                    instance: self.member.instance,
                    kind: IndexKind([11; 16]),
                    group: PublicationGroupId([12; 16]),
                    activation: GroupActivation::CoreRequired,
                    input_domain: InputDomain::SourceLocal,
                    schema: self.member.input.schema,
                    policy: self.member.input.policy,
                    format: self.member.format,
                },
                source: next,
                input,
            };
            match NativeFileMap.plan(
                BaseMember::Present(Box::new(self.member.clone())),
                DeltaInput::Patch(SourceDelta {
                    prior: self.ticket.view.source,
                    next,
                    changed: OwnerMutationSet {
                        root: MapRoot(None),
                        count: 0,
                        digest,
                    },
                }),
                &context,
                &mut self.work,
            )? {
                PlanDecision::Incremental(plan) => Ok(plan),
                PlanDecision::Reuse(_)
                | PlanDecision::RebuildRequired
                | PlanDecision::MaintenanceRequired => Err(IndexError::BackendViolation(
                    "empty patch did not produce incremental file map plan".into(),
                )),
            }
        }

        fn prepare(&mut self, plan: FileMapPlan) -> Result<FileMapCandidate, IndexError> {
            let mut attempt = AttemptContext {
                attempt: AttemptRef {
                    operation: OperationKey {
                        workspace: self.ticket.view.identity.workspace,
                        epoch: self.ticket.view.identity.epoch,
                        producer: ProducerId([13; 16]),
                        stream: StreamId([14; 16]),
                        stream_epoch: 1,
                        sequence: 1,
                    },
                    attempt: AttemptId([15; 16]),
                    fence: 1,
                },
                objects: &self.objects,
                maps: &self.maps,
                custody: &self.custody,
                scratch: &self.scratch,
                budget: &mut self.work,
            };
            ready(NativeFileMap.prepare(plan, &mut attempt))
        }

        fn verify(&mut self, candidate: FileMapCandidate) -> Result<MemberBinding, IndexError> {
            let mut attempt = AttemptContext {
                attempt: AttemptRef {
                    operation: OperationKey {
                        workspace: self.ticket.view.identity.workspace,
                        epoch: self.ticket.view.identity.epoch,
                        producer: ProducerId([13; 16]),
                        stream: StreamId([14; 16]),
                        stream_epoch: 1,
                        sequence: 1,
                    },
                    attempt: AttemptId([15; 16]),
                    fence: 1,
                },
                objects: &self.objects,
                maps: &self.maps,
                custody: &self.custody,
                scratch: &self.scratch,
                budget: &mut self.work,
            };
            ready(NativeFileMap.verify_and_seal(candidate, &mut attempt))
        }
    }

    #[test]
    fn empty_patch_reuses_file_map_artifact_without_scratch_or_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let plan = case.plan(
            case.ticket.view.source,
            case.member.input,
            OwnerMutationDigest::default().finish(),
        )?;
        let candidate = case.prepare(plan)?;
        check_eq!(case.verify(candidate)?, case.member);
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn frontier_only_empty_patch_rebinds_file_map_with_one_root_object()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let mut next = case.ticket.view.source;
        next.frontiers = MapRoot(Some(reference(41)));
        let input = InputBinding {
            context: Digest([42; 32]),
            ..case.member.input
        };
        let plan = case.plan(next, input, OwnerMutationDigest::default().finish())?;
        let candidate = case.prepare(plan)?;
        let binding = case.verify(candidate)?;
        check_ne!(binding.artifact, case.member.artifact);
        check_eq!(binding.input, input);
        let old = load_root(&case.member, &case.objects, &mut case.work)?;
        let updated = load_root(&binding, &case.objects, &mut case.work)?;
        check_eq!(updated.owners, old.owners);
        check_eq!(updated.paths, old.paths);
        check_eq!(updated.input, input);
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn forged_empty_patch_digest_fails_before_file_map_scratch()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let plan = case.plan(
            case.ticket.view.source,
            case.member.input,
            Digest([0xff; 32]),
        )?;
        check!(matches!(
            case.prepare(plan),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn empty_patch_with_changed_owner_root_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let mut next = case.ticket.view.source;
        next.owners = MapRoot(Some(reference(43)));
        // Both fixture source roots have no owner entries. The changed identity
        // has an empty structural difference before the patch-root guard fires.
        case.maps.1 = Some((case.ticket.view.source.owners, next.owners));
        let input = InputBinding {
            root: next.owners,
            context: Digest([44; 32]),
            ..case.member.input
        };
        let plan = case.plan(next, input, OwnerMutationDigest::default().finish())?;
        check!(matches!(
            case.prepare(plan),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn nonempty_source_bootstrap_requires_exact_input_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_objects, _maps, ticket, member) = fixture()?;
        check!(ticket.view.source.owners.0.is_some());
        let mut context = PlanContext {
            instance: InstanceSpec {
                instance: member.instance,
                kind: IndexKind([11; 16]),
                group: PublicationGroupId([12; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: member.input.schema,
                policy: member.input.policy,
                format: member.format,
            },
            source: ticket.view.source,
            input: member.input,
        };
        check!(matches!(
            NativeFileMap.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &context,
                &mut budget()?
            )?,
            PlanDecision::Incremental(_)
        ));
        context.input.root = MapRoot(None);
        check!(matches!(
            NativeFileMap.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &context,
                &mut budget()?
            )?,
            PlanDecision::RebuildRequired
        ));
        Ok(())
    }

    #[test]
    fn exact_prefix_then_exclusive_pages_include_a_ab_and_abc()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, maps, ticket, member) = fixture()?;
        let mut work = budget()?;
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        let first = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "a",
                after: None,
                limit: 1,
            },
            &mut context,
        )?;
        check_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        check!(first.has_more);
        let second = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "a",
                after: Some("a"),
                limit: 1,
            },
            &mut context,
        )?;
        check_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["ab"]
        );
        check!(second.has_more);
        let third = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "a",
                after: Some("ab"),
                limit: 1,
            },
            &mut context,
        )?;
        check_eq!(
            third
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["abc"]
        );
        check!(!third.has_more);
        let missing = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "ac",
                after: None,
                limit: 2,
            },
            &mut context,
        )?;
        check!(missing.entries.is_empty());
        check!(!missing.has_more);
        let ab = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "ab",
                after: None,
                limit: 2,
            },
            &mut context,
        )?;
        check_eq!(
            ab.entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["ab", "abc"]
        );
        check!(!ab.has_more);
        Ok(())
    }

    #[test]
    fn historical_member_and_reverse_owner_edges_are_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, mut maps, ticket, member) = fixture()?;
        let mut work = budget()?;
        let wrong = MemberBinding {
            artifact: reference(99),
            ..member
        };
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        check!(matches!(
            query_paths(
                &FilePathQuery {
                    member: &wrong,
                    prefix: "a",
                    after: None,
                    limit: 1
                },
                &mut context
            ),
            Err(IndexError::CorruptObject(_))
        ));
        let actual_owner_root = reference(1);
        if let Some(map) = maps.0.get_mut(&actual_owner_root) {
            let _removed_owner = map.remove(owner_map_key(owner(1)).as_slice());
        }
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        check!(matches!(
            query_paths(
                &FilePathQuery {
                    member: &member,
                    prefix: "a",
                    after: None,
                    limit: 1
                },
                &mut context
            ),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }
}
