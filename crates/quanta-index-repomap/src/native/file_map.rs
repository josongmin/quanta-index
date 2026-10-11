//! Native file/path inventory. Runtime graph, symbols and dependency edges are
//! separate producer projections; this adapter never infers them from bytes.

use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapRoot,
    MemberBinding, ObjectBatchInput, ObjectRef, OwnerKey, PlanContext, PlanDecision,
    SourceChangeCursor, SourceDelta, SourceRecord, TicketRecord, WorkBudget, WorkUsage,
    owner_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

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

#[derive(Clone, Debug)]
pub struct FileMapPlan {
    base: Option<MemberBinding>,
    context: PlanContext,
    delta: Option<SourceDelta>,
}

#[derive(Clone, Debug)]
pub struct FileMapCandidate {
    binding: MemberBinding,
}

impl IndexBuildPort for NativeFileMap {
    type Delta = SourceDelta;
    type Plan = FileMapPlan;
    type Candidate = FileMapCandidate;

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<FileMapPlan>, IndexError> {
        budget.checkpoint()?;
        let (base, delta) = match (base, input) {
            (BaseMember::Absent, DeltaInput::Bootstrap)
                if context.source.owners.0.is_none()
                    && context.input.root == context.source.owners =>
            {
                (None, None)
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
                (Some(*member), Some(delta))
            }
            _ => return Ok(PlanDecision::RebuildRequired),
        };
        Ok(PlanDecision::Incremental(FileMapPlan {
            base,
            context: context.clone(),
            delta,
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
            if let Some(delta) = plan.delta {
                let scratch = check_changes(&delta, root, context)?;
                stage_contributions(&scratch, context)?;
                let mut edits = InventoryEditCursor::new(&scratch);
                let roots = context.apply_stream(&[root.owners, root.paths], &mut edits)?;
                root.owners = *roots.first().ok_or_else(|| {
                    IndexError::BackendViolation("file inventory owner root missing".into())
                })?;
                root.paths = *roots.get(1).ok_or_else(|| {
                    IndexError::BackendViolation("file inventory path root missing".into())
                })?;
            }
            root.input = plan.context.input;
            let payload = encode(&root)?;
            let children = root_children(root);
            let artifact = context.put(&payload, &children)?;
            Ok(FileMapCandidate {
                binding: MemberBinding {
                    instance: plan.context.instance.instance,
                    format: plan.context.instance.format,
                    artifact,
                    input: plan.context.input,
                    logical_commitment: commitment(&payload),
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
    delta: &SourceDelta,
    root: FileMapRoot,
    context: &mut AttemptContext<'_>,
) -> Result<InventoryScratch, IndexError> {
    let scratch = InventoryScratch::open(context.budget)?;
    let mut changes = SourceChangeCursor::new(
        delta.changed,
        delta.prior,
        delta.next,
        context.maps,
        context.objects,
    );
    let mut previous = None;
    while let Some(change) = changes.next_change(context.budget)? {
        context.budget.checkpoint()?;
        if previous.is_some_and(|owner| owner >= change.owner) {
            return Err(IndexError::InvalidInput(
                "file map owners must be unique and sorted".into(),
            ));
        }
        previous = Some(change.owner);
        let prior_contribution =
            context
                .maps
                .lookup(root.owners, &owner_map_key(change.owner), context.budget)?;
        let prior = prior_contribution
            .map(|reference| load_contribution(reference, context.objects, context.budget))
            .transpose()?;
        if prior.as_ref().map(|entry| (entry.source, &entry.record))
            != change
                .prior
                .as_ref()
                .map(|(reference, record)| (*reference, record))
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
            Some(path_key(&entry.record.path)?)
        } else {
            None
        };
        if let (Some(path), Some(reference)) = (&prior_path, prior_contribution) {
            if context.maps.lookup(root.paths, path, context.budget)? != Some(reference) {
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
            let path = path_key(&record.path)?;
            scratch.record_next_path(&path, context.budget)?;
        }
        scratch.record_owner(
            change.owner,
            prior_contribution,
            change.next.map(|(source, _record)| source),
            context.budget,
        )?;
    }
    scratch.validate_paths(root.paths, context.maps, context.budget)?;
    Ok(scratch)
}

fn stage_contributions(
    scratch: &InventoryScratch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut after = None;
    let mut inputs = Vec::new();
    let mut bindings = Vec::new();
    let mut batch_bytes = 0usize;
    while let Some(row) = scratch.next_owner(after.as_deref(), context.budget)? {
        context.budget.checkpoint()?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(row.key.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        after = Some(row.key.clone());
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
        let path = path_key(&record.path)?;
        let contribution = FileContribution {
            owner,
            source,
            record,
        };
        let payload = encode(&contribution)?;
        let size = payload
            .len()
            .checked_add(32)
            .ok_or(IndexError::ResourceLimited)?;
        if size > CONTRIBUTION_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        if inputs.len() == CONTRIBUTION_BATCH_COUNT
            || batch_bytes
                .checked_add(size)
                .is_none_or(|total| total > CONTRIBUTION_BATCH_BYTES)
        {
            flush_contributions(scratch, &mut inputs, &mut bindings, context)?;
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes
            .checked_add(size)
            .ok_or(IndexError::ResourceLimited)?;
        inputs.push(ObjectBatchInput {
            payload,
            children: vec![source],
        });
        bindings.push((row.key, path));
    }
    flush_contributions(scratch, &mut inputs, &mut bindings, context)
}

fn flush_contributions(
    scratch: &InventoryScratch,
    inputs: &mut Vec<ObjectBatchInput>,
    bindings: &mut Vec<(Vec<u8>, Vec<u8>)>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if inputs.is_empty() {
        return Ok(());
    }
    let references = context.put_batch(inputs)?;
    if references.len() != bindings.len() {
        return Err(IndexError::BackendViolation(
            "file contributions lost batch ordinal".into(),
        ));
    }
    for ((owner, path), reference) in bindings.iter().zip(references) {
        scratch.set_next(owner, path, reference, context.budget)?;
    }
    inputs.clear();
    bindings.clear();
    Ok(())
}

fn read_source(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecord, IndexError> {
    let stored = objects.get(reference, budget)?;
    let record: SourceRecord = decode(&stored.payload)?;
    validate_path(&record.path)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != [record.bytes]
    {
        return Err(IndexError::CorruptObject(
            "file source record edges mismatch".into(),
        ));
    }
    Ok(record)
}

fn load_contribution(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<FileContribution, IndexError> {
    let stored = objects.get(reference, budget)?;
    let entry: FileContribution = decode(&stored.payload)?;
    validate_path(&entry.record.path)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != [entry.source]
    {
        return Err(IndexError::CorruptObject(
            "file contribution edges mismatch".into(),
        ));
    }
    Ok(entry)
}

fn load_root(
    binding: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<FileMapRoot, IndexError> {
    let stored = objects.get(binding.artifact, budget)?;
    let root: FileMapRoot = decode(&stored.payload)?;
    if stored.registration.reference != binding.artifact
        || stored.registration.children != root_children(root)
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

fn root_children(root: FileMapRoot) -> Vec<ObjectRef> {
    [root.owners.0, root.paths.0, root.input.root.0]
        .into_iter()
        .flatten()
        .collect()
}

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

fn validate_path(path: &str) -> Result<(), IndexError> {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePathEntry {
    pub owner: OwnerKey,
    pub path: String,
    pub source: ObjectRef,
    pub bytes: ObjectRef,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePathPage {
    pub entries: Vec<FilePathEntry>,
    pub has_more: bool,
}

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
        .lookup(
            context.ticket.view.members,
            &member_key(query.member.instance),
            context.budget,
        )?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored_member = context.objects.get(member_ref, context.budget)?;
    let published: MemberBinding = decode(&stored_member.payload)?;
    let mut expected_member_children = vec![query.member.artifact];
    if let Some(source_root) = query.member.input.root.0 {
        expected_member_children.push(source_root);
    }
    if published != *query.member
        || stored_member.registration.reference != member_ref
        || stored_member.registration.children != expected_member_children
        || context.ticket.view.source.owners != query.member.input.root
    {
        return Err(IndexError::CorruptObject(
            "historical file member mismatch".into(),
        ));
    }
    let root = load_root(query.member, context.objects, context.budget)?;
    let prefix_key = path_prefix_key(query.prefix)?;
    let mut entries = Vec::new();
    let mut cursor = if let Some(after) = query.after {
        let key = path_key(after)?;
        let reference = context
            .maps
            .lookup(root.paths, &key, context.budget)?
            .ok_or_else(|| IndexError::InvalidInput("file path cursor is absent".into()))?;
        let _validated_cursor = read_path_entry(reference, after, root.owners, context)?;
        key
    } else {
        if !query.prefix.is_empty()
            && validate_path(query.prefix).is_ok()
            && let Some(reference) = context
                .maps
                .lookup(root.paths, &prefix_key, context.budget)?
        {
            entries.push(read_path_entry(
                reference,
                query.prefix,
                root.owners,
                context,
            )?);
        }
        prefix_key.clone()
    };
    let limit =
        usize::try_from(query.limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
    loop {
        context.budget.checkpoint()?;
        let remaining = limit.saturating_sub(entries.len());
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
            return Ok(FilePathPage {
                entries,
                has_more: false,
            });
        }
        for (key, reference) in &page.entries {
            if !key.starts_with(&prefix_key) {
                return Ok(FilePathPage {
                    entries,
                    has_more: false,
                });
            }
            if entries.len() == limit {
                return Ok(FilePathPage {
                    entries,
                    has_more: true,
                });
            }
            let path =
                std::str::from_utf8(key.get(PATH_PREFIX.len()..).ok_or_else(|| {
                    IndexError::CorruptObject("file path key is truncated".into())
                })?)
                .map_err(|_conversion_error| {
                    IndexError::CorruptObject("file path key is not UTF-8".into())
                })?;
            entries.push(read_path_entry(*reference, path, root.owners, context)?);
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("file map scan omitted continuation".into())
        })?;
        if entries.len() == limit {
            // A later key may be outside this prefix. One bounded lookahead
            // prevents declaring a false continuation at the range boundary.
            let probe = context
                .maps
                .scan(root.paths, Some(&last.0), 1, context.budget)?;
            return Ok(FilePathPage {
                entries,
                has_more: probe
                    .entries
                    .first()
                    .is_some_and(|(key, _)| key.starts_with(&prefix_key)),
            });
        }
        if !page.has_more {
            return Ok(FilePathPage {
                entries,
                has_more: false,
            });
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(last.0.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        cursor.clone_from(&last.0);
    }
}

fn read_path_entry(
    reference: ObjectRef,
    expected_path: &str,
    owners: MapRoot,
    context: &mut FileMapReadContext<'_>,
) -> Result<FilePathEntry, IndexError> {
    let contribution = load_contribution(reference, context.objects, context.budget)?;
    if contribution.record.path != expected_path {
        return Err(IndexError::CorruptObject(
            "file path key/value mismatch".into(),
        ));
    }
    if read_source(contribution.source, context.objects, context.budget)? != contribution.record {
        return Err(IndexError::CorruptObject(
            "file contribution/source mismatch".into(),
        ));
    }
    if context
        .maps
        .lookup(owners, &owner_map_key(contribution.owner), context.budget)?
        != Some(reference)
    {
        return Err(IndexError::CorruptObject(
            "file owner/path reverse mismatch".into(),
        ));
    }
    Ok(FilePathEntry {
        owner: contribution.owner,
        path: contribution.record.path,
        source: contribution.source,
        bytes: contribution.record.bytes,
        byte_len: contribution.record.byte_len,
    })
}

fn member_key(instance: quanta_index_core::index::IndexInstanceId) -> Vec<u8> {
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    key
}

fn validate_prefix(prefix: &str) -> Result<(), IndexError> {
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
        sync::Mutex,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        CatalogView, CollectionId, IndexInstanceId, MapEdit, MapPage, MapWrite, ProducerId,
        RegisteredObject, SchemaId, SlotId, SourceCut, StableOwnerId, StorageFormatId,
        StoredObject, TenantId, TicketId, ViewRef, WorkLimits, WorkspaceKey, WorktreeId,
        ports::ObjectWriteSink,
    };

    use super::*;

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
            let stored = StoredObject {
                registration: RegisteredObject {
                    reference,
                    bytes: u64::try_from(payload.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    children,
                },
                payload,
            };
            let mut values = self
                .0
                .lock()
                .map_err(|_conversion_error| IndexError::Storage("test lock".into()))?;
            drop(values.insert(reference, stored));
            Ok(reference)
        }
    }

    impl ImmutableObjectPort for TestObjects {
        fn install_batch(
            &self,
            _: &[quanta_index_core::index::ObjectBatchInput],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<Vec<RegisteredObject>, IndexError> {
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

    #[derive(Default)]
    struct TestMaps(BTreeMap<ObjectRef, BTreeMap<Vec<u8>, ObjectRef>>);

    impl TestMaps {
        fn add(&mut self, root: ObjectRef, key: Vec<u8>, value: ObjectRef) {
            let _old = self.0.entry(root).or_default().insert(key, value);
        }
    }

    impl PersistentMapPort for TestMaps {
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn quanta_index_core::index::ports::MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<Vec<MapRoot>, IndexError> {
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
        fn lookup(
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
                return Ok(MapPage {
                    entries: vec![],
                    has_more: false,
                });
            };
            let matches: Vec<_> = map
                .iter()
                .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                .map(|(key, value)| (key.clone(), *value))
                .collect();
            let limit =
                usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            Ok(MapPage {
                has_more: matches.len() > limit,
                entries: matches.into_iter().take(limit).collect(),
            })
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
    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 10_000_000,
                mutations: 1000,
            },
            Instant::now() + Duration::from_secs(30),
        )
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
                owner_map_key(owner(id)),
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
        let artifact = objects.add(payload.clone(), root_children(root))?;
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
                id: TicketId([3; 16]),
                view,
            },
            binding,
        ))
    }

    #[test]
    fn exact_prefix_then_exclusive_pages_include_a_ab_and_abc()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, maps, ticket, member) = fixture()?;
        let mut work = budget();
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
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert!(first.has_more);
        let second = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "a",
                after: Some("a"),
                limit: 1,
            },
            &mut context,
        )?;
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["ab"]
        );
        assert!(second.has_more);
        let third = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "a",
                after: Some("ab"),
                limit: 1,
            },
            &mut context,
        )?;
        assert_eq!(
            third
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["abc"]
        );
        assert!(!third.has_more);
        let missing = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "ac",
                after: None,
                limit: 2,
            },
            &mut context,
        )?;
        assert!(missing.entries.is_empty());
        assert!(!missing.has_more);
        let ab = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "ab",
                after: None,
                limit: 2,
            },
            &mut context,
        )?;
        assert_eq!(
            ab.entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["ab", "abc"]
        );
        assert!(!ab.has_more);
        Ok(())
    }

    #[test]
    fn historical_member_and_reverse_owner_edges_are_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, mut maps, ticket, member) = fixture()?;
        let mut work = budget();
        let wrong = MemberBinding {
            artifact: reference(99),
            ..member.clone()
        };
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        assert!(matches!(
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
            let _removed_owner = map.remove(&owner_map_key(owner(1)));
        }
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        assert!(matches!(
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
