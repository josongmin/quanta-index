//! Native file/path inventory. Runtime graph, symbols and dependency edges are
//! separate producer projections; this adapter never infers them from bytes.

use quanta_index_contract::RepoId;
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapRoot,
    MemberBinding, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, PlanContext, PlanDecision,
    SchemaId, SourceCut, SourceDelta, SourceInputCursor, SourcePathOwners, SourcePathRepositoryKey,
    SourceRecord, SourceRecordOwner, StoredObject, TicketRecord, WorkBudget, WorkUsage,
    member_map_key as member_key, owner_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use quanta_index_core::{DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    io::{self, Write},
    ops::Deref,
};

use super::scratch::InventoryScratch;

// The live commitment domain. Its bytes are part of every stored root's
// commitment, so the `-v3` in the wire string stays even though the live type
// carries no version.
const COMMIT_DOMAIN: &[u8] = b"quanta-index-native-filemap-v3\0";
// Retired wire formats keep their version: each name identifies one retired
// commitment domain that readers only authenticate before refusing it.
const LEGACY_V2_COMMIT_DOMAIN: &[u8] = b"quanta-index-native-filemap-v2\0";
const LEGACY_COMMIT_DOMAIN: &[u8] = b"quanta-index-native-filemap-v1\0";
const PATH_PREFIX: &[u8] = b"path\0";
const MAX_PATH_BYTES: usize = quanta_index_core::index::MAX_SOURCE_PATH_BYTES;
const MAX_PAGE: u32 = 4096;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFileMap;

fn observe_member_work<T>(
    context: &mut AttemptContext<'_>,
    lane: u16,
    work: impl FnOnce(&mut AttemptContext<'_>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let parent = context.budget.request().diagnostic_context().cloned();
    let span = parent.as_ref().and_then(|parent| {
        parent
            .enter(DiagnosticStageV1::BuildPrepare, DiagnosticLaneV1(lane))
            .ok()
    });
    let before = span.as_ref().map(|_| context.budget.usage());
    let result = {
        let child = parent
            .as_ref()
            .zip(span.as_ref())
            .map(|(parent, span)| parent.child_context(span));
        let mut budget = context.budget.scoped_diagnostic_context(child);
        let mut phase = AttemptContext {
            attempt: context.attempt,
            objects: context.objects,
            maps: context.maps,
            custody: context.custody,
            scratch: context.scratch,
            budget: &mut budget,
        };
        work(&mut phase)
    };
    if let (Some(span), Some(before)) = (span.as_ref(), before) {
        context.budget.record_diagnostic_usage_delta(before, span);
    }
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMapCapability {
    /// Retired wire format; authenticated, then refused as `RebuildRequired`.
    FilePathInventoryV1,
    /// Retired wire format; authenticated, then refused as `RebuildRequired`.
    FilePathInventoryV2,
    /// The live root format. Its wire code stays 2.
    FilePathInventory,
}
quanta_index_contract::native_serde_enum!(FileMapCapability {
    units { FilePathInventoryV1 = 0, FilePathInventoryV2 = 1, FilePathInventory = 2 }
    newtypes {}
    records {}
});

/// The file map root binds exactly what its inventory is a function of.
///
/// That is the member schema/policy and the Source owners and paths maps.
/// Source frontiers are producer stream positions and the member input
/// context fingerprints them, so neither is part of the root: a frontier-only
/// Source advance keeps the exact root identity under a new input binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileMapRoot {
    capability: FileMapCapability,
    schema: SchemaId,
    policy: Digest,
    owners: MapRoot,
    paths: MapRoot,
}
quanta_index_contract::native_serde_struct!(FileMapRoot {
    capability: FileMapCapability,
    schema: SchemaId,
    policy: Digest,
    owners: MapRoot,
    paths: MapRoot,
});

fn file_map_root(input: InputBinding, source: SourceCut) -> FileMapRoot {
    FileMapRoot {
        capability: FileMapCapability::FilePathInventory,
        schema: input.schema,
        policy: input.policy,
        owners: source.owners,
        paths: source.paths,
    }
}

impl FileMapRoot {
    fn binds(self, input: InputBinding, source: SourceCut) -> bool {
        self == file_map_root(input, source)
    }
}

struct RootHeader {
    capability: FileMapCapability,
}

impl<'de> Deserialize<'de> for RootHeader {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

        struct CapabilityKey;

        impl<'de> DeserializeSeed<'de> for CapabilityKey {
            type Value = bool;

            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                deserializer.deserialize_identifier(self)
            }
        }

        impl Visitor<'_> for CapabilityKey {
            type Value = bool;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("field identifier")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(value == "capability")
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(value == b"capability")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                if value == 0 {
                    Ok(true)
                } else {
                    Err(E::invalid_value(
                        de::Unexpected::Unsigned(value),
                        &"field index 0 <= i < 1",
                    ))
                }
            }
        }

        struct Header;

        impl<'de> Visitor<'de> for Header {
            type Value = RootHeader;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct RootHeader")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut capability = None;
                while let Some(is_capability) = map.next_key_seed(CapabilityKey)? {
                    if is_capability {
                        if capability.is_some() {
                            return Err(de::Error::duplicate_field("capability"));
                        }
                        capability = Some(map.next_value()?);
                    } else {
                        let _ignored = map.next_value::<IgnoredAny>()?;
                    }
                }
                let capability =
                    capability.ok_or_else(|| de::Error::missing_field("capability"))?;
                Ok(RootHeader { capability })
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let capability = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(0, &"struct RootHeader with 1 element")
                })?;
                Ok(RootHeader { capability })
            }
        }

        deserializer.deserialize_struct("RootHeader", &["capability"], Header)
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
    _memory: Option<MemoryLease>,
}

struct BoundedWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    budget: &'a WorkBudget,
    error: Option<IndexError>,
}

impl Write for BoundedWriter<'_> {
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
            return Err(io::Error::other("file map root exceeds admitted output"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
    let mut writer = BoundedWriter {
        bytes,
        limit: bound,
        budget,
        error: None,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return Err(writer.error.unwrap_or_else(|| {
            IndexError::BackendViolation(format!("file map root JSON: {error}"))
        }));
    }
    budget.checkpoint()?;
    Ok(FundedPayload {
        bytes: writer.bytes,
        _memory: memory,
    })
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
        if context.input.root != context.source.owners {
            return Ok(PlanDecision::RebuildRequired);
        }
        if context.input.context
            != context.source.input_context_digest().map_err(|error| {
                IndexError::InvalidInput(format!("file map source context: {error}"))
            })?
        {
            return Err(IndexError::Conflict);
        }
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
                    || member.input.context
                        != delta.prior.input_context_digest().map_err(|error| {
                            IndexError::InvalidInput(format!(
                                "file map prior source context: {error}"
                            ))
                        })?
                    || delta.next != context.source
                    || context.input.root != delta.next.owners
                {
                    return Err(IndexError::Conflict);
                }
                // The root binds only schema/policy and the owners and paths
                // maps. A frontier-only advance (canonical empty mutation set,
                // unchanged owners and paths) keeps the published root without
                // invoking prepare or opening a file map object.
                if quanta_index_core::index::is_canonical_empty_mutation_set(delta.changed)
                    && delta.prior.owners == delta.next.owners
                    && delta.prior.paths == delta.next.paths
                {
                    return Ok(PlanDecision::Reuse(Box::new(MemberBinding {
                        input: context.input,
                        ..*member
                    })));
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
            observe_member_work(context, 41, |context| {
                let base_root = match &plan.base {
                    Some(base) => load_root(base, context.objects, context.budget)?,
                    None => file_map_root(
                        plan.context.input,
                        SourceCut {
                            owners: MapRoot(None),
                            paths: MapRoot(None),
                            frontiers: MapRoot(None),
                        },
                    ),
                };
                if let DeltaInput::Patch(delta) = &plan.input
                    && !base_root.binds(plan.context.input, delta.prior)
                {
                    return Err(IndexError::Conflict);
                }
                let mut changes = SourceInputCursor::new(
                    plan.context.source,
                    plan.input.clone(),
                    context.maps,
                    context.objects,
                )?;
                let changes_seen = observe_member_work(context, 49, |context| match &plan.input {
                    DeltaInput::Bootstrap => {
                        validate_bootstrap_paths(&mut changes, plan.context.source, context)
                    }
                    DeltaInput::Patch(delta) => {
                        validate_delta_paths(&mut changes, delta.prior, delta.next, context)
                    }
                })?;
                if changes_seen == 0
                    && matches!(&plan.input, DeltaInput::Patch(delta) if delta.prior.owners != delta.next.owners)
                {
                    return Err(IndexError::CorruptObject(
                        "empty file map patch changed owner root".into(),
                    ));
                }
                if let Some(base) = &plan.base
                    && base_root.binds(plan.context.input, plan.context.source)
                {
                    return Ok(FileMapCandidate {
                        binding: MemberBinding {
                            input: plan.context.input,
                            ..base.clone()
                        },
                    });
                }
                let root = file_map_root(plan.context.input, plan.context.source);
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
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: FileMapCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            observe_member_work(context, 43, |context| {
                context.budget.checkpoint()?;
                let _root = load_root(&candidate.binding, context.objects, context.budget)?;
                Ok(candidate.binding)
            })
        })
    }
}

fn validate_bootstrap_paths(
    changes: &mut SourceInputCursor<'_>,
    source: SourceCut,
    context: &mut AttemptContext<'_>,
) -> Result<u64, IndexError> {
    let mut owner_count = 0_u64;
    let mut expected_paths = None;
    while let Some(change) = changes.next_change(context.budget)? {
        context.budget.checkpoint()?;
        let Some((_, record)) = change.next else {
            return Err(IndexError::CorruptObject(
                "bootstrap source owner has no record".into(),
            ));
        };
        if change.prior.is_some() {
            return Err(IndexError::CorruptObject(
                "bootstrap source contains a prior record".into(),
            ));
        }
        if expected_paths.is_none() {
            expected_paths = Some(InventoryScratch::open(context.scratch, context.budget)?);
        }
        let path = PathKey::new(&record.path, context.budget)?;
        let repo = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
        expected_paths
            .as_ref()
            .ok_or(IndexError::ResourceLimited)?
            .record_next_repository_path(&path, repo.as_slice()?, change.owner, context.budget)?;
        owner_count = owner_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
    }
    // The sorted scratch expectation is disposable. The complete source path
    // scan supplies the authority and excludes missing or extra map entries.
    let mut path_count = 0_u64;
    let mut after: Option<quanta_index_core::index::MapPageEntry> = None;
    loop {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            source.paths,
            after
                .as_ref()
                .map(quanta_index_core::index::MapPageEntry::key)
                .transpose()?,
            128,
            context.budget,
        )?;
        if page.entries.is_empty() {
            if page.has_more {
                return Err(IndexError::BackendViolation(
                    "source path scan did not advance".into(),
                ));
            }
            break;
        }
        if page.entries.len() > 128
            || page.entries.first().is_some_and(|(key, _)| {
                after
                    .as_ref()
                    .and_then(|entry| entry.key().ok())
                    .is_some_and(|last| key.as_slice() <= last)
            })
        {
            return Err(IndexError::BackendViolation(
                "source path scan escaped its ordered page".into(),
            ));
        }
        for (index, (key, value)) in page.entries.iter().enumerate() {
            if index > 0 && page.entries[index - 1].0.as_slice() >= key.as_slice() {
                return Err(IndexError::BackendViolation(
                    "source path scan repeated or reversed a key".into(),
                ));
            }
            let path = key.strip_prefix(PATH_PREFIX).ok_or_else(|| {
                IndexError::CorruptObject("source path map has a foreign key".into())
            })?;
            let path = std::str::from_utf8(path).map_err(|_error| {
                IndexError::CorruptObject("source path map key is not UTF-8".into())
            })?;
            validate_path(path).map_err(|_error| {
                IndexError::CorruptObject("source path map key is not canonical".into())
            })?;
            match SourcePathOwners::decode(*value, context.budget)? {
                SourcePathOwners::LegacyOwner(_) => {
                    validate_bootstrap_repository(
                        key,
                        b"repo\0",
                        *value,
                        expected_paths.as_ref(),
                        context.budget,
                    )?;
                    path_count = path_count
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                SourcePathOwners::Repositories(root) => {
                    let before = path_count;
                    scan_repository_bucket(
                        MapRoot(Some(root)),
                        context.maps,
                        context.budget,
                        |repo, owner, budget| {
                            validate_bootstrap_repository(
                                key,
                                repo,
                                owner,
                                expected_paths.as_ref(),
                                budget,
                            )?;
                            path_count = path_count
                                .checked_add(1)
                                .ok_or(IndexError::ResourceLimited)?;
                            Ok(())
                        },
                    )?;
                    if before == path_count {
                        return Err(IndexError::CorruptObject(
                            "source path repository bucket is empty".into(),
                        ));
                    }
                }
            }
        }
        let more = page.has_more;
        after = page.cursor().last();
        if !more {
            break;
        }
    }
    if path_count != owner_count {
        return Err(IndexError::CorruptObject(
            "source path map is not a complete owner projection".into(),
        ));
    }
    Ok(owner_count)
}

fn validate_bootstrap_repository(
    path: &[u8],
    repo: &[u8],
    owner: quanta_index_core::index::MapValue,
    expected: Option<&InventoryScratch>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if !matches!(
        SourcePathOwners::decode(owner, budget)?,
        SourcePathOwners::LegacyOwner(_)
    ) {
        return Err(IndexError::CorruptObject(
            "repository path owner has a child map".into(),
        ));
    }
    if expected
        .map(|scratch| scratch.expected_repository_at(path, repo, budget))
        .transpose()?
        .flatten()
        != Some((None, Some(owner)))
    {
        return Err(IndexError::CorruptObject(
            "source path map differs from authenticated owners".into(),
        ));
    }
    Ok(())
}

fn scan_repository_bucket(
    root: MapRoot,
    maps: &dyn PersistentMapPort,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(
        &[u8],
        quanta_index_core::index::MapValue,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut after: Option<quanta_index_core::index::MapPageEntry> = None;
    loop {
        budget.checkpoint()?;
        let page = maps.scan(
            root,
            after
                .as_ref()
                .map(quanta_index_core::index::MapPageEntry::key)
                .transpose()?,
            128,
            budget,
        )?;
        if page.entries.is_empty() {
            if page.has_more {
                return Err(IndexError::BackendViolation(
                    "repository path scan did not advance".into(),
                ));
            }
            break;
        }
        if page.entries.len() > 128
            || page.entries.first().is_some_and(|(key, _)| {
                after
                    .as_ref()
                    .and_then(|entry| entry.key().ok())
                    .is_some_and(|last| key.as_slice() <= last)
            })
        {
            return Err(IndexError::BackendViolation(
                "repository path scan escaped its ordered page".into(),
            ));
        }
        for (index, (key, value)) in page.entries.iter().enumerate() {
            if index > 0 && page.entries[index - 1].0.as_slice() >= key.as_slice() {
                return Err(IndexError::BackendViolation(
                    "repository path scan repeated or reversed a key".into(),
                ));
            }
            validate_repository_key(key)?;
            visit(key, *value, budget)?;
        }
        let more = page.has_more;
        after = page.cursor().last();
        if !more {
            break;
        }
    }
    Ok(())
}

fn validate_repository_key(key: &[u8]) -> Result<(), IndexError> {
    let suffix = key
        .strip_prefix(b"repo\0")
        .ok_or_else(|| IndexError::CorruptObject("repository path map has a foreign key".into()))?;
    if suffix.is_empty() {
        return Ok(());
    }
    let name = std::str::from_utf8(suffix)
        .map_err(|_error| IndexError::CorruptObject("repository path key is not UTF-8".into()))?;
    RepoId::validate(name)
        .map_err(|error| IndexError::CorruptObject(format!("repository path key: {error}")))?;
    Ok(())
}

fn validate_delta_paths(
    changes: &mut SourceInputCursor<'_>,
    prior: SourceCut,
    next: SourceCut,
    context: &mut AttemptContext<'_>,
) -> Result<u64, IndexError> {
    let mut scratch = None;
    let mut changed_count = 0_u64;
    while let Some(change) = changes.next_change(context.budget)? {
        changed_count = changed_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        context.budget.checkpoint()?;
        let old_path = change
            .prior
            .as_ref()
            .map(|(_, record)| record.path.as_str());
        let new_path = change.next.as_ref().map(|(_, record)| record.path.as_str());
        let old_repo = change
            .prior
            .as_ref()
            .map(|(_, record)| record.source_repo_id.as_ref());
        let new_repo = change
            .next
            .as_ref()
            .map(|(_, record)| record.source_repo_id.as_ref());
        // The pinned V2 base proved prior.paths. Changed owner records and the
        // outer and repository-map diffs prove the next cut.
        if old_path != new_path || old_repo != new_repo {
            if scratch.is_none() {
                scratch = Some(InventoryScratch::open(context.scratch, context.budget)?);
            }
            let expected = scratch.as_ref().ok_or(IndexError::ResourceLimited)?;
            if let Some((_, record)) = change.prior.as_ref() {
                let key = PathKey::new(&record.path, context.budget)?;
                let repo = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
                expected.record_prior_repository_path(
                    &key,
                    repo.as_slice()?,
                    change.owner,
                    context.budget,
                )?;
            }
            if let Some((_, record)) = change.next.as_ref() {
                let key = PathKey::new(&record.path, context.budget)?;
                let repo = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
                expected.record_next_repository_path(
                    &key,
                    repo.as_slice()?,
                    change.owner,
                    context.budget,
                )?;
            }
        }
    }
    verify_path_diff(prior.paths, next.paths, scratch.as_ref(), context)?;
    Ok(changed_count)
}

struct PathDiffCheck<'a> {
    expected: Option<&'a InventoryScratch>,
    maps: &'a dyn PersistentMapPort,
    seen: &'a mut u64,
    after: Option<&'a [u8]>,
    page_last: &'a mut Vec<u8>,
}

impl quanta_index_core::index::ports::MapDiffVisitor for PathDiffCheck<'_> {
    fn changed(
        &mut self,
        key: &[u8],
        before: Option<quanta_index_core::index::MapValue>,
        after: Option<quanta_index_core::index::MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let lower = if self.page_last.is_empty() {
            self.after
        } else {
            Some(self.page_last.as_slice())
        };
        if lower.is_some_and(|lower| key <= lower) {
            return Err(IndexError::BackendViolation(
                "source path diff is not strictly increasing".into(),
            ));
        }
        let path = key.strip_prefix(PATH_PREFIX).ok_or_else(|| {
            IndexError::CorruptObject("source path diff contains a foreign key".into())
        })?;
        let path = std::str::from_utf8(path).map_err(|_error| {
            IndexError::CorruptObject("source path diff key is not UTF-8".into())
        })?;
        validate_path(path).map_err(|_error| {
            IndexError::CorruptObject("source path diff key is not canonical".into())
        })?;
        if before == after {
            return Err(IndexError::BackendViolation(
                "source path diff contains an unchanged value".into(),
            ));
        }
        let before = before
            .map(|value| SourcePathOwners::decode(value, budget))
            .transpose()?;
        let after = after
            .map(|value| SourcePathOwners::decode(value, budget))
            .transpose()?;
        if key.len() > PATH_PREFIX.len() + MAX_PATH_BYTES {
            return Err(IndexError::CorruptObject(
                "source path diff key exceeds bound".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let prior_seen = *self.seen;
        verify_repository_diff(
            key,
            before,
            after,
            self.maps,
            self.expected,
            self.seen,
            budget,
        )?;
        if *self.seen == prior_seen {
            return Err(IndexError::CorruptObject(
                "source path diff has no owner transition".into(),
            ));
        }
        self.page_last.clear();
        self.page_last.extend_from_slice(key);
        Ok(())
    }
}

struct RepositoryDiffCheck<'a> {
    path: &'a [u8],
    expected: Option<&'a InventoryScratch>,
    seen: &'a mut u64,
    legacy_before: Option<quanta_index_core::index::MapValue>,
    legacy_after: Option<quanta_index_core::index::MapValue>,
    saw_legacy: &'a mut bool,
    after: Option<&'a [u8]>,
    page_last: &'a mut Vec<u8>,
}

impl quanta_index_core::index::ports::MapDiffVisitor for RepositoryDiffCheck<'_> {
    fn changed(
        &mut self,
        key: &[u8],
        before: Option<quanta_index_core::index::MapValue>,
        after: Option<quanta_index_core::index::MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let lower = if self.page_last.is_empty() {
            self.after
        } else {
            Some(self.page_last.as_slice())
        };
        if lower.is_some_and(|lower| key <= lower) {
            return Err(IndexError::BackendViolation(
                "repository path diff is not strictly increasing".into(),
            ));
        }
        validate_repository_key(key)?;
        let (before, after) = if key == b"repo\0" {
            *self.saw_legacy = true;
            (self.legacy_before.or(before), self.legacy_after.or(after))
        } else {
            (before, after)
        };
        verify_repository_transition(
            self.path,
            key,
            before,
            after,
            self.expected,
            self.seen,
            budget,
        )?;
        self.page_last.clear();
        self.page_last.extend_from_slice(key);
        Ok(())
    }
}

fn verify_repository_transition(
    path: &[u8],
    repo: &[u8],
    before: Option<quanta_index_core::index::MapValue>,
    after: Option<quanta_index_core::index::MapValue>,
    expected: Option<&InventoryScratch>,
    seen: &mut u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    for value in [before, after].into_iter().flatten() {
        if !matches!(
            SourcePathOwners::decode(value, budget)?,
            SourcePathOwners::LegacyOwner(_)
        ) {
            return Err(IndexError::CorruptObject(
                "repository path owner has a child map".into(),
            ));
        }
    }
    if before == after {
        return Ok(());
    }
    if expected
        .map(|scratch| scratch.expected_repository_at(path, repo, budget))
        .transpose()?
        .flatten()
        != Some((before, after))
    {
        return Err(IndexError::CorruptObject(
            "source path diff differs from owner changes".into(),
        ));
    }
    *seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    Ok(())
}

fn verify_repository_diff(
    path: &[u8],
    before: Option<SourcePathOwners>,
    after: Option<SourcePathOwners>,
    maps: &dyn PersistentMapPort,
    expected: Option<&InventoryScratch>,
    seen: &mut u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let child_before = match before {
        Some(SourcePathOwners::Repositories(root)) => MapRoot(Some(root)),
        Some(SourcePathOwners::LegacyOwner(_)) | None => MapRoot(None),
    };
    let child_after = match after {
        Some(SourcePathOwners::Repositories(root)) => MapRoot(Some(root)),
        Some(SourcePathOwners::LegacyOwner(_)) | None => MapRoot(None),
    };
    let legacy_before = match before {
        Some(SourcePathOwners::LegacyOwner(owner)) => {
            Some(SourcePathOwners::LegacyOwner(owner).encode()?)
        }
        Some(SourcePathOwners::Repositories(_)) | None => None,
    };
    let legacy_after = match after {
        Some(SourcePathOwners::LegacyOwner(owner)) => {
            Some(SourcePathOwners::LegacyOwner(owner).encode()?)
        }
        Some(SourcePathOwners::Repositories(_)) | None => None,
    };
    if child_before.0.is_none() && child_after.0.is_none() {
        return verify_repository_transition(
            path,
            b"repo\0",
            legacy_before,
            legacy_after,
            expected,
            seen,
            budget,
        );
    }
    let capacity = 5 + RepoId::MAX_UTF8_BYTES_V1;
    let _memory = budget.reserve_memory(
        u64::try_from(capacity.checked_mul(2).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut previous = Vec::new();
    previous
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if previous.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut page_last = Vec::new();
    page_last
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if page_last.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut saw_legacy = false;
    loop {
        page_last.clear();
        let mut visitor = RepositoryDiffCheck {
            path,
            expected,
            seen,
            legacy_before,
            legacy_after,
            saw_legacy: &mut saw_legacy,
            after: if previous.is_empty() {
                None
            } else {
                Some(previous.as_slice())
            },
            page_last: &mut page_last,
        };
        let more = maps.visit_diff(
            child_before,
            child_after,
            quanta_index_core::index::ports::MapDiffRange {
                prefix: b"",
                after: visitor.after,
                limit: 128,
            },
            &mut visitor,
            budget,
        )?;
        if !more {
            break;
        }
        if page_last.is_empty() {
            return Err(IndexError::BackendViolation(
                "repository path diff returned an empty continuation page".into(),
            ));
        }
        std::mem::swap(&mut previous, &mut page_last);
    }
    if (legacy_before.is_some() || legacy_after.is_some()) && !saw_legacy {
        let child_prior = maps.lookup(child_before, b"repo\0", budget)?;
        let child_next = maps.lookup(child_after, b"repo\0", budget)?;
        verify_repository_transition(
            path,
            b"repo\0",
            legacy_before.or(child_prior),
            legacy_after.or(child_next),
            expected,
            seen,
            budget,
        )?;
    }
    Ok(())
}

fn verify_path_diff(
    prior: MapRoot,
    next: MapRoot,
    scratch: Option<&InventoryScratch>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let capacity = PATH_PREFIX.len() + MAX_PATH_BYTES;
    let _memory = context.budget.reserve_memory(
        u64::try_from(capacity.checked_mul(2).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut previous = Vec::new();
    previous
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if previous.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut page_last = Vec::new();
    page_last
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if page_last.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut seen = 0_u64;
    loop {
        let before = seen;
        page_last.clear();
        let mut visitor = PathDiffCheck {
            expected: scratch,
            maps: context.maps,
            seen: &mut seen,
            after: if before == 0 {
                None
            } else {
                Some(previous.as_slice())
            },
            page_last: &mut page_last,
        };
        let more = context.maps.visit_diff(
            prior,
            next,
            quanta_index_core::index::ports::MapDiffRange {
                prefix: b"",
                after: visitor.after,
                limit: 128,
            },
            &mut visitor,
            context.budget,
        )?;
        if !more {
            break;
        }
        if seen == before {
            return Err(IndexError::BackendViolation(
                "source path diff returned an empty continuation page".into(),
            ));
        }
        std::mem::swap(&mut previous, &mut page_last);
    }
    let expected = scratch
        .map(|scratch| scratch.count(context.budget))
        .transpose()?
        .unwrap_or(0);
    if seen != expected {
        return Err(IndexError::CorruptObject(
            "source path diff omitted an owner path transition".into(),
        ));
    }
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

fn load_root(
    binding: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<FileMapRoot, IndexError> {
    let stored = objects.get(binding.artifact, budget)?;
    let _memory = decoded_memory(&stored.payload, budget)?;
    if stored.registration.reference != binding.artifact {
        return Err(IndexError::CorruptObject(
            "file map root reference mismatch".into(),
        ));
    }
    // This header only distinguishes persisted formats; it never reads V1 data.
    let header: RootHeader = decode(&stored.payload)?;
    let legacy_domain = match header.capability {
        FileMapCapability::FilePathInventoryV1 => Some(LEGACY_COMMIT_DOMAIN),
        FileMapCapability::FilePathInventoryV2 => Some(LEGACY_V2_COMMIT_DOMAIN),
        FileMapCapability::FilePathInventory => None,
    };
    if let Some(domain) = legacy_domain {
        if commitment_in_domain(domain, &stored.payload) != binding.logical_commitment {
            return Err(IndexError::CorruptObject(
                "legacy file map commitment mismatch".into(),
            ));
        }
        return Err(IndexError::RebuildRequired);
    }
    let root: FileMapRoot = decode(&stored.payload)?;
    if stored.registration.children.as_slice() != root_children(root).as_slice()
        || root.capability != FileMapCapability::FilePathInventory
        || root.schema != binding.input.schema
        || root.policy != binding.input.policy
        || root.owners != binding.input.root
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
}

impl RootChildren {
    fn as_slice(&self) -> &[ObjectRef] {
        match self {
            Self::Empty => &[],
            Self::One(values) => values.as_slice(),
            Self::Two(values) => values.as_slice(),
        }
    }
}

fn root_children(root: FileMapRoot) -> RootChildren {
    // Owners then paths, preserving aliases in immutable object identity.
    match [root.owners.0, root.paths.0] {
        [None, None] => RootChildren::Empty,
        [Some(a), None] | [None, Some(a)] => RootChildren::One([a]),
        [Some(a), Some(b)] => RootChildren::Two([a, b]),
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
    commitment_in_domain(COMMIT_DOMAIN, payload)
}

fn commitment_in_domain(domain: &[u8], payload: &[u8]) -> Digest {
    let mut hash = Sha256::new();
    hash.update(domain);
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

pub(super) fn selected_source_cut(
    member: &MemberBinding,
    context: &mut FileMapReadContext<'_>,
) -> Result<SourceCut, IndexError> {
    let member_ref = context
        .maps
        .lookup_object(
            context.ticket.view.members,
            &member_key(member.instance),
            context.budget,
        )?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored_member = context.objects.get(member_ref, context.budget)?;
    let published =
        quanta_index_core::index::decode_member_binding(&stored_member.payload, context.budget)?;
    let expected_member_children = [Some(member.artifact), member.input.root.0];
    if published != *member
        || stored_member.registration.reference != member_ref
        || !stored_member
            .registration
            .children
            .iter()
            .copied()
            .eq(expected_member_children.into_iter().flatten())
        || context.ticket.view.source.owners != member.input.root
    {
        return Err(IndexError::CorruptObject(
            "historical file member mismatch".into(),
        ));
    }
    let root = load_root(member, context.objects, context.budget)?;
    let source = context.ticket.view.source;
    let source_context = source.input_context_digest().map_err(|error| {
        IndexError::CorruptObject(format!("file source context digest: {error}"))
    })?;
    if !root.binds(member.input, source) || member.input.context != source_context {
        return Err(IndexError::CorruptObject(
            "historical file source binding mismatch".into(),
        ));
    }
    Ok(source)
}

/// Reads the unscoped paths of a pinned historical file inventory.
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
    let source = selected_source_cut(query.member, context)?;
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
        let marker = context
            .maps
            .lookup(source.paths, &cursor, context.budget)?
            .ok_or_else(|| IndexError::InvalidInput("file path cursor is absent".into()))?;
        let owner = unscoped_path_owner(marker, context)?
            .ok_or_else(|| IndexError::InvalidInput("file path cursor is absent".into()))?;
        let _validated_cursor = read_path_entry(owner, after, source.owners, context)?;
    } else {
        if !query.prefix.is_empty()
            && validate_path(query.prefix).is_ok()
            && let Some(marker) = context
                .maps
                .lookup(source.paths, &prefix_key, context.budget)?
            && let Some(owner) = unscoped_path_owner(marker, context)?
        {
            result.entries.push(read_path_entry(
                owner,
                query.prefix,
                source.owners,
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
            .scan(source.paths, Some(&cursor), scan_limit, context.budget)?;
        if page.entries.is_empty() {
            if page.has_more {
                return Err(IndexError::BackendViolation(
                    "file map scan made no progress".into(),
                ));
            }
            return Ok(result);
        }
        for (key, marker) in &page.entries {
            if !key.starts_with(&prefix_key) {
                return Ok(result);
            }
            let Some(owner) = unscoped_path_owner(*marker, context)? else {
                continue;
            };
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
            result
                .entries
                .push(read_path_entry(owner, path, source.owners, context)?);
        }
        if !page.has_more {
            return Ok(result);
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("file map scan omitted continuation".into())
        })?;
        if last.0.as_slice() <= cursor.as_slice() {
            return Err(IndexError::BackendViolation(
                "file map scan did not advance its continuation".into(),
            ));
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

fn unscoped_path_owner(
    marker: quanta_index_core::index::MapValue,
    context: &mut FileMapReadContext<'_>,
) -> Result<Option<OwnerKey>, IndexError> {
    match SourcePathOwners::decode(marker, context.budget)? {
        SourcePathOwners::LegacyOwner(owner) => Ok(Some(owner)),
        SourcePathOwners::Repositories(root) => {
            let key = SourcePathRepositoryKey::new(None)?;
            SourcePathOwners::repository_owner(root, key.as_slice()?, context.maps, context.budget)
        }
    }
}

fn read_path_entry(
    owner: OwnerKey,
    expected_path: &str,
    owners: MapRoot,
    context: &mut FileMapReadContext<'_>,
) -> Result<FilePathEntry, IndexError> {
    let source_ref = context
        .maps
        .lookup_object(owners, &owner_map_key(owner), context.budget)?
        .ok_or_else(|| IndexError::CorruptObject("file path owner lacks source".into()))?;
    let source = read_source(source_ref, context.objects, context.budget)?;
    if source.path != expected_path || source.source_repo_id.is_some() {
        return Err(IndexError::CorruptObject(
            "file path key/value mismatch".into(),
        ));
    }
    let bytes = source.bytes;
    let byte_len = source.byte_len;
    let (path, memory) = source.into_retained_path(context.budget)?;
    Ok(FilePathEntry::from_parts(
        owner, path, source_ref, bytes, byte_len, memory,
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
        collections::{BTreeMap, BTreeSet},
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
        IndexKind, InputDomain, InstanceSpec, MapEdit, MapPage, MapWrite, ObjectBatchInput,
        OperationKey, OwnerMutationDigest, OwnerMutationSet, ProducerId, PublicationGroupId,
        RegisteredObject, SchemaId, ScratchAllocation, ScratchRequest, ScratchSpacePort, SlotId,
        SourceCut, StableOwnerId, StorageFormatId, StoredObject, StreamId, TenantId, TicketId,
        ViewRef, WorkLimits, WorkspaceKey, WorktreeId,
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

    struct DiskScratch(AtomicUsize);
    struct DiskAllocation(tempfile::TempDir, AtomicU64);

    impl ScratchAllocation for DiskAllocation {
        fn path(&self) -> &std::path::Path {
            self.0.path()
        }

        fn max_bytes(&self) -> u64 {
            4_294_967_296
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(self.1.load(Ordering::Acquire))
        }

        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > self.max_bytes() {
                return Err(IndexError::ResourceLimited);
            }
            let _previous = self.1.fetch_max(bytes, Ordering::AcqRel);
            Ok(())
        }
    }

    impl ScratchSpacePort for DiskScratch {
        fn allocate(
            &self,
            request: ScratchRequest,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            budget.checkpoint()?;
            if request.max_bytes != 4_294_967_296 {
                return Err(IndexError::BackendViolation(
                    "unexpected test scratch envelope".into(),
                ));
            }
            let _previous = self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(DiskAllocation(
                tempfile::tempdir()
                    .map_err(|error| IndexError::Storage(format!("test scratch: {error}")))?,
                AtomicU64::new(request.initial_bytes),
            )))
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
        BTreeMap<ObjectRef, BTreeMap<Vec<u8>, quanta_index_core::index::MapValue>>,
        Option<MapRoot>,
        Option<MapRoot>,
        AtomicUsize,
    );

    impl TestMaps {
        fn add(&mut self, root: ObjectRef, key: Vec<u8>, value: ObjectRef) {
            let _old = self.0.entry(root).or_default().insert(key, value.into());
        }

        fn add_value(
            &mut self,
            root: ObjectRef,
            key: Vec<u8>,
            value: quanta_index_core::index::MapValue,
        ) {
            let _old = self.0.entry(root).or_default().insert(key, value);
        }
    }

    impl PersistentMapPort for TestMaps {
        fn visit_diff(
            &self,
            before: MapRoot,
            after: MapRoot,
            range: quanta_index_core::index::ports::MapDiffRange<'_>,
            visitor: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
            budget: &mut WorkBudget,
        ) -> Result<bool, IndexError> {
            budget.checkpoint()?;
            if before == after {
                return Ok(false);
            }
            let left = before.0.and_then(|root| self.0.get(&root));
            let right = after.0.and_then(|root| self.0.get(&root));
            let keys: BTreeSet<Vec<u8>> = left
                .into_iter()
                .flat_map(|map| map.keys())
                .chain(right.into_iter().flat_map(|map| map.keys()))
                .cloned()
                .collect();
            let mut count = 0_u32;
            for key in keys {
                if !key.starts_with(range.prefix)
                    || range.after.is_some_and(|after| key.as_slice() <= after)
                {
                    continue;
                }
                let expected = left.and_then(|map| map.get(&key)).copied();
                let next = right.and_then(|map| map.get(&key)).copied();
                if expected == next {
                    continue;
                }
                if count == range.limit {
                    return Ok(true);
                }
                visitor.changed(&key, expected, next, budget)?;
                count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            Ok(false)
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
            root.0
                .and_then(|reference| self.0.get(&reference))
                .and_then(|map| map.get(key))
                .copied()
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            if self.2 == Some(root) {
                let _previous = self.3.fetch_add(1, Ordering::Relaxed);
            }
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
                return Ok(MapPage::new(vec![], false, Vec::new(), None));
            };
            let mut matches: Vec<_> = map
                .iter()
                .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                .map(|(key, value)| (key.clone(), *value))
                .collect();
            if self.1 == Some(root) && after.is_none() {
                matches.reverse();
            }
            let limit =
                usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let has_more = matches.len() > limit;
            Ok(MapPage::new(
                matches.into_iter().take(limit).collect(),
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

    #[test]
    fn fixed_root_encoding_preserves_edge_order_and_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_objects, _maps, _ticket, binding) = fixture()?;
        let source = SourceCut {
            owners: MapRoot(Some(reference(2))),
            paths: MapRoot(Some(reference(1))),
            frontiers: MapRoot(Some(reference(2))),
        };
        let root = file_map_root(
            InputBinding {
                root: source.owners,
                context: source.input_context_digest()?,
                ..binding.input
            },
            source,
        );
        check_eq!(
            root_children(root).as_slice(),
            &[reference(2), reference(1)]
        );
        let aliased = FileMapRoot {
            paths: source.owners,
            ..root
        };
        check_eq!(
            root_children(aliased).as_slice(),
            &[reference(2), reference(2)]
        );
        let (work, memory) = build_budget(u64::MAX, false)?;
        let payload = encode_bounded(&root, 2048, &work)?;
        check_eq!(payload.bytes, encode(&root)?);
        drop(payload);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn fixed_v1_root_wire_refuses_with_typed_rebuild() -> Result<(), Box<dyn std::error::Error>> {
        // Fixed legacy JSON and its V1 domain digest, independent of the V2 writer.
        const WIRE: &[u8] = br#"{"capability":"FilePathInventoryV1","input":{"root":null,"schema":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"policy":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"context":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]},"owners":null,"paths":null}"#;
        const LEGACY_DIGEST: Digest = Digest([
            209, 24, 233, 104, 66, 248, 120, 118, 158, 93, 55, 5, 92, 149, 34, 253, 76, 10, 169,
            108, 46, 15, 0, 44, 64, 80, 188, 110, 209, 26, 35, 231,
        ]);
        let objects = TestObjects::default();
        let artifact = objects.add(WIRE.to_vec(), vec![])?;
        let binding = MemberBinding {
            instance: IndexInstanceId([8; 16]),
            format: StorageFormatId([9; 16]),
            artifact,
            input: InputBinding {
                root: MapRoot(None),
                schema: SchemaId([0; 16]),
                policy: Digest([0; 32]),
                context: Digest([0; 32]),
            },
            logical_commitment: LEGACY_DIGEST,
        };
        let mut work = budget()?;
        check!(matches!(
            load_root(&binding, &objects, &mut work),
            Err(IndexError::RebuildRequired)
        ));
        let forged = MemberBinding {
            logical_commitment: commitment(WIRE),
            ..binding
        };
        check!(matches!(
            load_root(&forged, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn canonical_source_bootstrap_publishes_only_metadata_root()
    -> Result<(), Box<dyn std::error::Error>> {
        // The source map and its path marker are the fixed independent golden.
        // No FileMap contribution or private map is available to the adapter.
        let objects = PatchObjects {
            base: TestObjects::default(),
            installs: AtomicUsize::new(0),
        };
        let mut maps = TestMaps::default();
        let source = SourceCut {
            owners: MapRoot(Some(reference(31))),
            paths: MapRoot(Some(reference(32))),
            frontiers: MapRoot(None),
        };
        let bytes = objects.base.add(b"x".to_vec(), vec![])?;
        let record = SourceRecord {
            path: "src/main.rs".into(),
            bytes,
            byte_len: 1,
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
            source_repo_id: None,
            language: None,
        };
        let record_ref = objects.base.add(encode(&record)?, vec![bytes])?;
        maps.add(
            source.owners.0.ok_or("missing source owners")?,
            owner_map_key(owner(1)).to_vec(),
            record_ref,
        );
        maps.add_value(
            source.paths.0.ok_or("missing source paths")?,
            path_key(&record.path)?,
            quanta_index_core::index::path_owner_map_value(owner(1))?,
        );
        maps.2 = Some(source.paths);
        let source_bytes = serde_json::to_vec(&source)?;
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-index-vnext\0");
        hasher.update(14_u64.to_be_bytes());
        hasher.update(b"source-context");
        hasher.update(u64::try_from(source_bytes.len())?.to_be_bytes());
        hasher.update(&source_bytes);
        let input = InputBinding {
            root: source.owners,
            schema: SchemaId([5; 16]),
            policy: Digest([6; 32]),
            context: Digest(hasher.finalize().into()),
        };
        let plan_context = PlanContext {
            instance: InstanceSpec {
                instance: IndexInstanceId([8; 16]),
                kind: IndexKind([11; 16]),
                group: PublicationGroupId([12; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: input.schema,
                policy: input.policy,
                format: StorageFormatId([9; 16]),
            },
            source,
            input,
        };
        let mut work = budget()?;
        let plan = match NativeFileMap.plan(
            BaseMember::Absent,
            DeltaInput::Bootstrap,
            &plan_context,
            &mut work,
        )? {
            PlanDecision::Incremental(plan) => plan,
            decision => return Err(format!("unexpected bootstrap decision: {decision:?}").into()),
        };
        let scratch = DiskScratch(AtomicUsize::new(0));
        let custody = NoCustody;
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: TenantId([1; 16]),
                        collection: CollectionId([1; 16]),
                        worktree: WorktreeId([1; 16]),
                        slot: SlotId([1; 16]),
                    },
                    epoch: 1,
                    producer: ProducerId([13; 16]),
                    stream: StreamId([14; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([15; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let candidate = ready(NativeFileMap.prepare(plan, &mut attempt))?;
        check_eq!(candidate.binding.input, input);
        check_eq!(objects.installs.load(Ordering::Relaxed), 1);
        check_eq!(maps.3.load(Ordering::Relaxed), 0);
        check!(scratch.0.load(Ordering::Relaxed) <= 1);
        Ok(())
    }

    fn fixture()
    -> Result<(TestObjects, TestMaps, TicketRecord, MemberBinding), Box<dyn std::error::Error>>
    {
        let objects = TestObjects::default();
        let mut maps = TestMaps::default();
        let owners = MapRoot(Some(reference(3)));
        let paths = MapRoot(Some(reference(2)));
        let members = MapRoot(Some(reference(4)));
        for (id, path) in [(1u8, "a"), (2, "ab"), (3, "abc"), (4, "b")] {
            let bytes = objects.add(vec![id], vec![])?;
            let record = SourceRecord {
                path: path.into(),
                bytes,
                byte_len: 1,
                text: quanta_index_core::index::SourceTextAdmission::Excluded,
                source_repo_id: None,
                language: None,
            };
            let source = objects.add(encode(&record)?, vec![bytes])?;
            maps.add(
                owners.0.ok_or("missing owners root")?,
                owner_map_key(owner(id)).to_vec(),
                source,
            );
            maps.add_value(
                paths.0.ok_or("missing paths root")?,
                path_key(path)?,
                quanta_index_core::index::path_owner_map_value(owner(id))?,
            );
        }
        let source = SourceCut {
            owners,
            paths,
            frontiers: MapRoot(None),
        };
        let input = InputBinding {
            root: owners,
            schema: SchemaId([5; 16]),
            policy: Digest([6; 32]),
            context: source.input_context_digest()?,
        };
        let root = file_map_root(input, source);
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
            vec![artifact, owners.0.ok_or("missing source root")?],
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
            source,
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

    fn run_fixture_bootstrap(
        edit: impl FnOnce(&mut TestMaps, SourceCut) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(Result<FileMapCandidate, IndexError>, usize, usize), Box<dyn std::error::Error>>
    {
        let (base, mut maps, ticket, member) = fixture()?;
        edit(&mut maps, ticket.view.source)?;
        let objects = PatchObjects {
            base,
            installs: AtomicUsize::new(0),
        };
        let scratch = DiskScratch(AtomicUsize::new(0));
        let custody = NoCustody;
        let context = PlanContext {
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
        let mut work = budget()?;
        let plan = match NativeFileMap.plan(
            BaseMember::Absent,
            DeltaInput::Bootstrap,
            &context,
            &mut work,
        )? {
            PlanDecision::Incremental(plan) => plan,
            decision => return Err(format!("unexpected bootstrap decision: {decision:?}").into()),
        };
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: ticket.view.identity.workspace,
                    epoch: ticket.view.identity.epoch,
                    producer: ProducerId([13; 16]),
                    stream: StreamId([14; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([15; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = ready(NativeFileMap.prepare(plan, &mut attempt));
        Ok((
            result,
            objects.installs.load(Ordering::Relaxed),
            scratch.0.load(Ordering::Relaxed),
        ))
    }

    #[test]
    fn bootstrap_rejects_missing_extra_malformed_and_out_of_order_paths_before_put()
    -> Result<(), Box<dyn std::error::Error>> {
        for fault in 0..7 {
            let (result, installs, scratch) = run_fixture_bootstrap(|maps, source| {
                let root = source.paths.0.ok_or("missing paths root")?;
                match fault {
                    0 => {
                        let _removed = maps
                            .0
                            .get_mut(&root)
                            .ok_or("missing map")?
                            .remove(&path_key("a")?);
                    }
                    1 => maps.add_value(
                        root,
                        path_key("orphan")?,
                        quanta_index_core::index::path_owner_map_value(owner(7))?,
                    ),
                    2 => maps.1 = Some(source.paths),
                    3 => {
                        for ordinal in 0..128 {
                            maps.add_value(
                                root,
                                path_key(&format!("z{ordinal:03}"))?,
                                quanta_index_core::index::path_owner_map_value(owner(7))?,
                            );
                        }
                    }
                    4 => maps.add_value(
                        root,
                        path_key("a")?,
                        quanta_index_core::index::path_owner_map_value(owner(7))?,
                    ),
                    5 => maps.add_value(root, path_key("a")?, reference(70).into()),
                    6 => {
                        let canonical = quanta_index_core::index::path_owner_map_value(owner(1))?;
                        let quanta_index_core::index::MapValue::Inline(canonical) = canonical
                        else {
                            return Err("canonical path owner is not inline".into());
                        };
                        maps.add_value(
                            root,
                            path_key("a")?,
                            quanta_index_core::index::MapValue::inline(
                                canonical.data()?,
                                [reference(70)],
                            )?,
                        );
                    }
                    _ => return Err("unexpected fault".into()),
                }
                Ok(())
            })?;
            check!(matches!(
                result,
                Err(IndexError::CorruptObject(_) | IndexError::BackendViolation(_))
            ));
            check_eq!(installs, 0);
            check!(scratch <= 1);
        }
        Ok(())
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
            match self.decide(next, input, digest)? {
                PlanDecision::Incremental(plan) => Ok(plan),
                PlanDecision::Reuse(_)
                | PlanDecision::RebuildRequired
                | PlanDecision::MaintenanceRequired => Err(IndexError::BackendViolation(
                    "empty patch did not produce incremental file map plan".into(),
                )),
            }
        }

        fn decide(
            &mut self,
            next: SourceCut,
            input: InputBinding,
            digest: Digest,
        ) -> Result<PlanDecision<FileMapPlan>, IndexError> {
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
            NativeFileMap.plan(
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
            )
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
    }

    #[test]
    fn empty_patch_reuses_file_map_artifact_without_scratch_or_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let PlanDecision::Reuse(reused) = case.decide(
            case.ticket.view.source,
            case.member.input,
            OwnerMutationDigest::default().finish(),
        )?
        else {
            return Err("canonical empty patch did not reuse the file map root".into());
        };
        check_eq!(*reused, case.member);
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
        Ok(())
    }

    /// Planning has no object port: `Reuse` proves zero file map I/O.
    #[test]
    fn frontier_only_empty_patch_reuses_file_map_root_without_io()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let mut next = case.ticket.view.source;
        next.frontiers = MapRoot(Some(reference(41)));
        let input = InputBinding {
            context: next.input_context_digest()?,
            ..case.member.input
        };
        let PlanDecision::Reuse(binding) =
            case.decide(next, input, OwnerMutationDigest::default().finish())?
        else {
            return Err("frontier-only patch did not reuse the file map root".into());
        };
        check_eq!(binding.artifact, case.member.artifact);
        check_eq!(binding.logical_commitment, case.member.logical_commitment);
        check_eq!(binding.input, input);
        check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
        // The retained root still authenticates under the new input binding.
        let root = load_root(&binding, &case.objects, &mut case.work)?;
        check!(root.binds(input, next));
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
    fn empty_owner_patch_rejects_unlisted_and_foreign_path_diff_before_put()
    -> Result<(), Box<dyn std::error::Error>> {
        for key in [path_key("orphan")?, b"foreign\0entry".to_vec()] {
            let mut case = EmptyPatchFixture::new()?;
            let mut next = case.ticket.view.source;
            next.paths = MapRoot(Some(reference(46)));
            let old_root = case
                .ticket
                .view
                .source
                .paths
                .0
                .ok_or("missing prior paths")?;
            let next_root = next.paths.0.ok_or("missing next paths")?;
            let prior = case
                .maps
                .0
                .get(&old_root)
                .ok_or("missing path map")?
                .clone();
            let _previous = case.maps.0.insert(next_root, prior);
            case.maps.add_value(
                next_root,
                key,
                quanta_index_core::index::path_owner_map_value(owner(7))?,
            );
            let input = InputBinding {
                context: next.input_context_digest()?,
                ..case.member.input
            };
            let plan = case.plan(next, input, OwnerMutationDigest::default().finish())?;
            check!(matches!(
                case.prepare(plan),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(case.objects.installs.load(Ordering::Relaxed), 0);
            check_eq!(case.scratch.allocations.load(Ordering::Relaxed), 0);
        }
        Ok(())
    }

    fn run_rename_with_extra_path(
        extra: Option<Vec<u8>>,
    ) -> Result<
        (Result<FileMapCandidate, IndexError>, usize, usize, usize),
        Box<dyn std::error::Error>,
    > {
        let (base, mut maps, ticket, member) = fixture()?;
        let objects = PatchObjects {
            base,
            installs: AtomicUsize::new(0),
        };
        let old_owner_root = ticket.view.source.owners.0.ok_or("missing owners")?;
        let old_path_root = ticket.view.source.paths.0.ok_or("missing paths")?;
        let old_record = maps
            .0
            .get(&old_owner_root)
            .and_then(|map| map.get(owner_map_key(owner(1)).as_slice()))
            .copied()
            .ok_or("missing prior owner")?
            .object_ref()?;
        let bytes = objects.base.add(b"renamed".to_vec(), vec![])?;
        let record = SourceRecord {
            path: "z".into(),
            bytes,
            byte_len: 7,
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
            source_repo_id: None,
            language: None,
        };
        let next_record = objects.base.add(encode(&record)?, vec![bytes])?;
        let mut next = ticket.view.source;
        next.owners = MapRoot(Some(reference(60)));
        next.paths = MapRoot(Some(reference(61)));
        let mut owner_map = maps
            .0
            .get(&old_owner_root)
            .ok_or("missing owner map")?
            .clone();
        let _previous = owner_map.insert(owner_map_key(owner(1)).to_vec(), next_record.into());
        let _previous = maps
            .0
            .insert(next.owners.0.ok_or("missing next owners")?, owner_map);
        let mut path_map = maps
            .0
            .get(&old_path_root)
            .ok_or("missing path map")?
            .clone();
        let _removed = path_map.remove(&path_key("a")?);
        let _previous = path_map.insert(
            path_key("z")?,
            quanta_index_core::index::path_owner_map_value(owner(1))?,
        );
        if let Some(extra) = extra {
            let _previous = path_map.insert(
                extra,
                quanta_index_core::index::path_owner_map_value(owner(7))?,
            );
        }
        let _previous = maps
            .0
            .insert(next.paths.0.ok_or("missing next paths")?, path_map);
        maps.2 = Some(next.paths);
        let mutation = quanta_index_core::index::OwnerMutation {
            owner: owner(1),
            expected: Some(old_record),
            next: Some(next_record),
        };
        let mut digest = OwnerMutationDigest::default();
        digest.push(&mutation)?;
        let changed = OwnerMutationSet {
            root: MapRoot(Some(reference(62))),
            count: 1,
            digest: digest.finish(),
        };
        maps.add_value(
            changed.root.0.ok_or("missing manifest")?,
            owner_map_key(owner(1)).to_vec(),
            quanta_index_core::index::owner_mutation_value(&mutation)?,
        );
        let input = InputBinding {
            root: next.owners,
            context: next.input_context_digest()?,
            ..member.input
        };
        let plan_context = PlanContext {
            instance: InstanceSpec {
                instance: member.instance,
                kind: IndexKind([11; 16]),
                group: PublicationGroupId([12; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: input.schema,
                policy: input.policy,
                format: member.format,
            },
            source: next,
            input,
        };
        let mut work = budget()?;
        let plan = match NativeFileMap.plan(
            BaseMember::Present(Box::new(member)),
            DeltaInput::Patch(SourceDelta {
                prior: ticket.view.source,
                next,
                changed,
            }),
            &plan_context,
            &mut work,
        )? {
            PlanDecision::Incremental(plan) => plan,
            decision => return Err(format!("unexpected rename decision: {decision:?}").into()),
        };
        let scratch = DiskScratch(AtomicUsize::new(0));
        let custody = NoCustody;
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: ticket.view.identity.workspace,
                    epoch: ticket.view.identity.epoch,
                    producer: ProducerId([13; 16]),
                    stream: StreamId([14; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([15; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = ready(NativeFileMap.prepare(plan, &mut attempt));
        Ok((
            result,
            objects.installs.load(Ordering::Relaxed),
            scratch.0.load(Ordering::Relaxed),
            maps.3.load(Ordering::Relaxed),
        ))
    }

    #[test]
    fn renamed_source_path_requires_exact_structural_diff_before_put()
    -> Result<(), Box<dyn std::error::Error>> {
        let (valid, installs, scratch, _lookups) = run_rename_with_extra_path(None)?;
        check!(valid.is_ok());
        check_eq!(installs, 1);
        check_eq!(scratch, 1);
        for extra in [path_key("orphan")?, b"foreign\0entry".to_vec()] {
            let (result, installs, scratch, _lookups) = run_rename_with_extra_path(Some(extra))?;
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
            check_eq!(installs, 0);
            check_eq!(scratch, 1);
        }
        Ok(())
    }

    #[test]
    fn patch_path_validation_uses_diff_without_random_path_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let (result, installs, scratch, path_lookups) = run_rename_with_extra_path(None)?;
        check!(result.is_ok());
        check_eq!(path_lookups, 0);
        check_eq!(installs, 1);
        check_eq!(scratch, 1);
        Ok(())
    }

    #[test]
    fn first_full_patch_from_configured_empty_source_uses_no_path_lookups()
    -> Result<(), Box<dyn std::error::Error>> {
        let (base, mut maps, ticket, member) = fixture()?;
        let objects = PatchObjects {
            base,
            installs: AtomicUsize::new(0),
        };
        let next = ticket.view.source;
        let prior = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let old_input = InputBinding {
            root: prior.owners,
            context: prior.input_context_digest()?,
            ..member.input
        };
        let old_root = file_map_root(old_input, prior);
        let old_payload = encode(&old_root)?;
        let old_artifact = objects.base.add(old_payload.clone(), vec![])?;
        let old_member = MemberBinding {
            instance: member.instance,
            format: member.format,
            artifact: old_artifact,
            input: old_input,
            logical_commitment: commitment(&old_payload),
        };
        let mut digest = OwnerMutationDigest::default();
        let changed_root = MapRoot(Some(reference(82)));
        for id in 1..=4 {
            let source_ref = maps
                .0
                .get(&next.owners.0.ok_or("missing source owner root")?)
                .and_then(|map| map.get(owner_map_key(owner(id)).as_slice()))
                .copied()
                .ok_or("missing source owner")?
                .object_ref()?;
            let mutation = quanta_index_core::index::OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(source_ref),
            };
            digest.push(&mutation)?;
            maps.add_value(
                changed_root.0.ok_or("missing mutation root")?,
                owner_map_key(owner(id)).to_vec(),
                quanta_index_core::index::owner_mutation_value(&mutation)?,
            );
        }
        maps.2 = Some(next.paths);
        let input = InputBinding {
            root: next.owners,
            context: next.input_context_digest()?,
            ..old_input
        };
        let plan_context = PlanContext {
            instance: InstanceSpec {
                instance: member.instance,
                kind: IndexKind([11; 16]),
                group: PublicationGroupId([12; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: input.schema,
                policy: input.policy,
                format: member.format,
            },
            source: next,
            input,
        };
        let mut work = budget()?;
        let plan = match NativeFileMap.plan(
            BaseMember::Present(Box::new(old_member)),
            DeltaInput::Patch(SourceDelta {
                prior,
                next,
                changed: OwnerMutationSet {
                    root: changed_root,
                    count: 4,
                    digest: digest.finish(),
                },
            }),
            &plan_context,
            &mut work,
        )? {
            PlanDecision::Incremental(plan) => plan,
            decision => return Err(format!("unexpected first patch: {decision:?}").into()),
        };
        let scratch = DiskScratch(AtomicUsize::new(0));
        let custody = NoCustody;
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: ticket.view.identity.workspace,
                    epoch: ticket.view.identity.epoch,
                    producer: ProducerId([13; 16]),
                    stream: StreamId([14; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([15; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let candidate = ready(NativeFileMap.prepare(plan, &mut attempt))?;
        check_eq!(candidate.binding.input, input);
        check_eq!(maps.3.load(Ordering::Relaxed), 0);
        check_eq!(objects.installs.load(Ordering::Relaxed), 1);
        check_eq!(scratch.0.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn empty_patch_with_changed_owner_root_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let mut case = EmptyPatchFixture::new()?;
        let mut next = case.ticket.view.source;
        next.owners = MapRoot(Some(reference(43)));
        // A zero-entry manifest cannot explain removal of the pinned source owners.
        let input = InputBinding {
            root: next.owners,
            context: next.input_context_digest()?,
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
    fn scalar_path_pages_keep_unscoped_entries_in_repository_buckets()
    -> Result<(), Box<dyn std::error::Error>> {
        let (objects, mut maps, ticket, member) = fixture()?;
        let owners = ticket.view.source.owners.0.ok_or("missing owners root")?;
        let paths = ticket.view.source.paths.0.ok_or("missing paths root")?;
        let repository = RepoId::new("repo-a")?;
        let repository_key = SourcePathRepositoryKey::new(Some(&repository))?;
        let legacy_key = SourcePathRepositoryKey::new(None)?;
        for (path, id, bucket) in [("a", 5, 60), ("ab", 2, 61), ("abd", 6, 62)] {
            let bytes = objects.add(vec![id], vec![])?;
            let record = SourceRecord {
                path: path.into(),
                bytes,
                byte_len: 1,
                text: quanta_index_core::index::SourceTextAdmission::Excluded,
                source_repo_id: Some(repository.clone()),
                language: None,
            };
            let source = objects.add(encode(&record)?, vec![bytes])?;
            maps.add(owners, owner_map_key(owner(id)).to_vec(), source);
            let bucket = reference(bucket);
            maps.add_value(
                bucket,
                repository_key.as_slice()?.to_vec(),
                quanta_index_core::index::path_owner_map_value(owner(id))?,
            );
            maps.add_value(
                paths,
                path_key(path)?,
                SourcePathOwners::Repositories(bucket).encode()?,
            );
        }
        maps.add_value(
            reference(60),
            legacy_key.as_slice()?.to_vec(),
            quanta_index_core::index::path_owner_map_value(owner(1))?,
        );
        let mut work = budget()?;
        let mut context = FileMapReadContext {
            ticket: &ticket,
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        let mut page = |after| {
            query_paths(
                &FilePathQuery {
                    member: &member,
                    prefix: "a",
                    after,
                    limit: 1,
                },
                &mut context,
            )
        };
        let first = page(None)?;
        check_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.owner)
                .collect::<Vec<_>>(),
            vec![owner(1)]
        );
        check!(first.has_more);
        let last = page(Some("a"))?;
        check_eq!(
            last.entries
                .iter()
                .map(|entry| entry.owner)
                .collect::<Vec<_>>(),
            vec![owner(3)]
        );
        check!(!last.has_more);
        check!(matches!(page(Some("ab")), Err(IndexError::InvalidInput(_))));
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
        let actual_owner_root = reference(3);
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
