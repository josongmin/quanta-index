//! A retained DSL query reads the same Source cut and projection for the
//! entire attempt, including result and cursor construction.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use quanta_index_contract::{
    QueryConstraintSetV1, SearchPlaneErrorCodeV2,
    index::{
        Digest, ObjectRef, OwnerKey, ProjectionBinding, ProjectionOwnerContribution, SourceCut,
        SourceTextAdmission,
    },
    native_protocol::{
        NATIVE_DSL_HEADER_OWNER_V1, NATIVE_DSL_POLICY_V1, NATIVE_DSL_PRODUCER_V1,
        NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1, NATIVE_DSL_SCHEMA_V1,
        NativeDslCorpusV1, NativeDslCursorV1, NativeDslFileV1,
        NativeDslKindV1, NativeDslPageV1, NativeDslQueryV1, NativeDslRowsV1,
        NativeDslRuntimeCatalogPageV1, NativeDslRuntimeCatalogV1, NativeDslScopeV1,
        NativeIndexResponse, NativeReadSessionId, native_dsl_file_owner_v1,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, SourceRecordOwner, StoredObject, WorkBudget,
    WorkUsage, decode_projection_owner, owner_from_map_key, owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
    producer_owner_prefix,
};
use quanta_index_kernel::{ReadTicket, TicketReadAttempt};
use quanta_index_search_plane::{
    native_dsl,
    readiness::{RuntimeMetadataState, StructuralAuthorityState},
};
use serde::{Serialize, Serializer, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};

use super::{NativeDispatcher, NativeReply, RetainedSearchTimer};

const MAX_DSL_OBJECT_BYTES: usize = 16 * 1024 * 1024;

mod publication;
mod structural_chunks;
pub(super) use structural_chunks::structural_chunks_page;
pub(crate) use publication::NativeDslCommitValidator;

fn native_dsl_binding(binding: &ProjectionBinding) -> bool {
    binding.schema == NATIVE_DSL_SCHEMA_V1
        && binding.producer == NATIVE_DSL_PRODUCER_V1
        && binding.producer_policy == NATIVE_DSL_POLICY_V1
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CacheKey {
    Corpus {
        binding: ObjectRef,
        constraints: Option<Digest>,
    },
    Catalog {
        binding: ObjectRef,
        corpus: ObjectRef,
    },
}

#[derive(Clone)]
enum CacheValue {
    Corpus(Arc<AdmittedCorpus>),
    Catalog(Arc<AdmittedCatalog>),
}

impl CacheValue {
    fn retained_bytes(&self) -> u64 {
        match self {
            Self::Corpus(corpus) => corpus.retained_bytes(),
            Self::Catalog(catalog) => catalog._backing.bytes(),
        }
    }
}

/// Cached data remain funded by the leases held in each entry. A read ticket
/// must authenticate the immutable binding before accessing this cache.
pub(crate) struct NativeDslCorpusCache {
    max_bytes: u64,
    entries: Mutex<VecDeque<(CacheKey, CacheValue)>>,
}

impl NativeDslCorpusCache {
    pub(crate) fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    fn get(&self, key: CacheKey) -> Result<Option<CacheValue>, IndexError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| corrupt("Native DSL cache"))?;
        let Some(position) = entries.iter().position(|(cached, _)| *cached == key) else {
            return Ok(None);
        };
        let entry = entries
            .remove(position)
            .ok_or_else(|| corrupt("Native DSL cache position"))?;
        let result = entry.1.clone();
        entries.push_back(entry);
        Ok(Some(result))
    }

    fn retain(&self, key: CacheKey, value: CacheValue) -> Result<(), IndexError> {
        let bytes = value.retained_bytes();
        if bytes > self.max_bytes {
            return Ok(());
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| corrupt("Native DSL cache"))?;
        if entries.iter().any(|(cached, _)| *cached == key) {
            return Ok(());
        }
        let mut held = entries.iter().fold(0u64, |total, (_, value)| {
            total.saturating_add(value.retained_bytes())
        });
        while entries.len() >= 64 || held > self.max_bytes - bytes {
            let Some((_, evicted)) = entries.pop_front() else {
                break;
            };
            held = held.saturating_sub(evicted.retained_bytes());
        }
        entries.push_back((key, value));
        Ok(())
    }
}

pub(crate) struct ScopedNativeDslPage {
    page: NativeDslPageV1,
    _backing: MemoryLease,
}

impl Serialize for ScopedNativeDslPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 64, "DslPageV1", &self.page)
    }
}

pub(super) fn search(
    dispatcher: &NativeDispatcher,
    ticket: &ReadTicket,
    attempt: &TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    query: &NativeDslQueryV1,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
) -> Result<NativeReply, IndexError> {
    timer.mark(super::ReadFailureStage::QueryPreparation);
    let storage = DslStorage {
        objects: dispatcher.objects.as_ref(),
        maps: dispatcher.objects.as_ref(),
    };
    let digest = query
        .digest()
        .map_err(|reason| IndexError::InvalidInput(reason.into()))?;
    let corpus = binding(
        dispatcher,
        ticket,
        attempt,
        &query.scope,
        NativeDslKindV1::Structural,
        work,
    )?;
    let runtime = if query.kind == NativeDslKindV1::RuntimeMetadata {
        Some(binding(
            dispatcher,
            ticket,
            attempt,
            &query.scope,
            NativeDslKindV1::RuntimeMetadata,
            work,
        )?)
    } else {
        None
    };
    let projection = runtime.map_or(corpus.0, |(reference, _)| reference);
    let view = ticket.view().identity;
    if query.after.as_ref().is_some_and(|after| {
        after.view != view
            || after.projection != projection
            || after.query != digest
            || after.candidate_id.is_empty()
    }) {
        return Err(IndexError::InvalidInput(
            "Native DSL cursor binding differs".into(),
        ));
    }
    let constraints = (query.kind == NativeDslKindV1::Structural
        && !query.text.constraints.is_unconstrained())
    .then_some(&query.text.constraints);
    let key = CacheKey::Corpus {
        binding: corpus.0,
        constraints: constraints.map(|_| digest),
    };
    let cache = &dispatcher.dsl_corpus_cache;
    timer.mark(super::ReadFailureStage::QueryExecution);
    let corpus_state = match cache.get(key)? {
        Some(CacheValue::Corpus(corpus)) => corpus,
        Some(CacheValue::Catalog(_)) => return Err(corrupt("Native DSL corpus cache kind")),
        None => {
            let loaded = Arc::new(load_corpus(storage, corpus.1, constraints, work)?);
            work.checkpoint()?;
            cache.retain(key, CacheValue::Corpus(Arc::clone(&loaded)))?;
            loaded
        }
    };
    let header = &corpus_state.header;
    if !query.scope.matches_pin(&header.pin) {
        return Err(corrupt(
            "Native DSL corpus scope differs from its projection",
        ));
    }
    let (rows, window) = if let Some((reference, runtime)) = runtime {
        let key = CacheKey::Catalog {
            binding: reference,
            corpus: corpus.0,
        };
        let catalog = match cache.get(key)? {
            Some(CacheValue::Catalog(catalog)) => catalog,
            Some(CacheValue::Corpus(_)) => return Err(corrupt("Native DSL catalog cache kind")),
            None => {
                let catalog = load_header::<NativeDslRuntimeCatalogV1>(storage, runtime, work)?;
                if catalog.value.corpus_certificate != corpus.1.certificate {
                    return Err(IndexError::SnapshotUnavailable);
                }
                let state = native_dsl::prepare_native_runtime_metadata_v1(
                    &corpus_state.state,
                    &catalog.value,
                    &header.pin,
                    work.request(),
                )
                .map_err(|error| query_error(work, error))?;
                let loaded = Arc::new(AdmittedCatalog {
                    state,
                    _backing: catalog._memory,
                });
                work.checkpoint()?;
                cache.retain(key, CacheValue::Catalog(Arc::clone(&loaded)))?;
                loaded
            }
        };
        let (hits, window) = native_dsl::execute_native_runtime_metadata_v1(
            &corpus_state.state,
            &catalog.state,
            &header.pin,
            query,
            work.request(),
        )
        .map_err(|error| query_error(work, error))?;
        (NativeDslRowsV1::RuntimeMetadata(hits), window)
    } else {
        if !header.structural_ready {
            return Err(IndexError::SnapshotUnavailable);
        }
        let state = &corpus_state.state;
        let producer =
            quanta_index_searchd::app::native_dsl_structural_producer_v1(Arc::clone(state));
        let (hits, window) = native_dsl::execute_native_structural_v1(
            producer,
            state,
            &header.pin,
            query,
            work.request(),
        )
        .map_err(|error| query_error(work, error))?;
        (NativeDslRowsV1::Structural(hits), window)
    };
    let after = if window.has_more() {
        let candidate_id = rows.candidate_ids().last().ok_or_else(|| {
            IndexError::BackendViolation("Native DSL continuation has no boundary".into())
        })?;
        Some(NativeDslCursorV1 {
            view,
            projection,
            query: digest,
            candidate_id: candidate_id.to_owned(),
        })
    } else {
        None
    };
    let page = NativeDslPageV1 {
        session,
        view,
        projection,
        query: digest,
        rows,
        window,
        after,
    };
    let mut size = ReplySize::default();
    ciborium::ser::into_writer(&page, &mut size).map_err(|_error| IndexError::ResourceLimited)?;
    let backing = dispatcher.resources.reserve_response_memory(
        size.bytes
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    work.checkpoint()?;
    Ok(NativeReply::DslPageV1(ScopedNativeDslPage {
        page,
        _backing: backing,
    }))
}

pub(super) fn runtime_catalog_page(
    dispatcher: &NativeDispatcher,
    ticket: &ReadTicket,
    attempt: &TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    scope: &NativeDslScopeV1,
    offset: u32,
    work: &mut WorkBudget,
) -> Result<NativeReply, IndexError> {
    if !offset.is_multiple_of(NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1) {
        return Err(IndexError::InvalidInput("unaligned RuntimeMetadata catalog offset".into()));
    }
    let (_, corpus) = binding(dispatcher, ticket, attempt, scope, NativeDslKindV1::Structural, work)?;
    let (projection, runtime) = binding(
        dispatcher, ticket, attempt, scope, NativeDslKindV1::RuntimeMetadata, work,
    )?;
    let storage = DslStorage {
        objects: dispatcher.objects.as_ref(),
        maps: dispatcher.objects.as_ref(),
    };
    let structural = load_header::<NativeDslCorpusV1>(storage, corpus, work)?;
    // RuntimeMetadata also binds corpora published before parse trees are ready.
    if !scope.matches_pin(&structural.value.pin) {
        return Err(IndexError::SnapshotUnavailable);
    }
    let output = load_header_output(storage, runtime, work)?;
    let catalog = decode::<NativeDslRuntimeCatalogV1>(&output, work)?;
    if catalog.value.corpus_certificate != corpus.certificate {
        return Err(IndexError::SnapshotUnavailable);
    }
    native_dsl::validate_native_runtime_catalog_pin_v1(&catalog.value, &structural.value.pin)
        .map_err(|error| query_error(work, error))?;
    drop(catalog);
    drop(structural);
    let start = usize::try_from(offset).map_err(|_| IndexError::ResourceLimited)?;
    let window = usize::try_from(NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1)
        .map_err(|_| IndexError::ResourceLimited)?;
    let end = start.saturating_add(window).min(output.payload.len());
    if start >= end {
        return Err(IndexError::InvalidInput("RuntimeMetadata catalog offset is past the end".into()));
    }
    let bytes = u64::try_from(end - start).map_err(|_| IndexError::ResourceLimited)?;
    work.charge(WorkUsage { bytes, ..WorkUsage::default() })?;
    let slot = NativeReply::reserve_canonical(|bytes| {
        dispatcher.resources.reserve_response_memory(bytes).map(Some)
    })?;
    let backing = dispatcher.resources.reserve_response_memory(bytes)?;
    let mut page = Vec::new();
    page.try_reserve_exact(end - start).map_err(|_| IndexError::ResourceLimited)?;
    if page.capacity() > end - start {
        return Err(IndexError::ResourceLimited);
    }
    for chunk in output.payload[start..end].chunks(64 * 1024) {
        work.checkpoint()?;
        page.extend_from_slice(chunk);
    }
    work.checkpoint()?;
    Ok(slot.finish_with_extra_memory(NativeIndexResponse::DslRuntimeCatalogPageV1(
        NativeDslRuntimeCatalogPageV1 {
            session,
            view: ticket.view().identity,
            source: ticket.view().source,
            structural_certificate: corpus.certificate,
            projection,
            output: output.registration.reference,
            total_bytes: u32::try_from(output.payload.len()).map_err(|_| IndexError::ResourceLimited)?,
            offset,
            bytes: page,
        },
    ), Some(backing)))
}

fn binding(
    dispatcher: &NativeDispatcher,
    ticket: &ReadTicket,
    attempt: &TicketReadAttempt<'_>,
    scope: &NativeDslScopeV1,
    kind: NativeDslKindV1,
    work: &mut WorkBudget,
) -> Result<(ObjectRef, ProjectionBinding), IndexError> {
    let value = dispatcher
        .kernel
        .projection_for_read_attempt(ticket, attempt, scope.projection(kind), work)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let (_, binding) = value;
    if binding.source != ticket.view().source {
        return Err(IndexError::SnapshotUnavailable);
    }
    if !native_dsl_binding(&binding) {
        return Err(corrupt("Native DSL projection authority differs"));
    }
    Ok(value)
}

#[derive(Clone, Copy)]
struct DslStorage<'a> {
    objects: &'a dyn ImmutableObjectPort,
    maps: &'a dyn PersistentMapPort,
}

struct Decoded<T> {
    value: T,
    _memory: MemoryLease,
}

struct AdmittedCatalog {
    state: RuntimeMetadataState,
    _backing: MemoryLease,
}

struct AdmittedCorpus {
    header: NativeDslCorpusV1,
    state: Arc<StructuralAuthorityState>,
    memory: Vec<MemoryLease>,
}

impl AdmittedCorpus {
    fn retained_bytes(&self) -> u64 {
        self.memory
            .iter()
            .fold(0, |bytes, lease| bytes.saturating_add(lease.bytes()))
    }
}

fn load_header<T: DeserializeOwned>(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    work: &mut WorkBudget,
) -> Result<Decoded<T>, IndexError> {
    let output = load_header_output(storage, binding, work)?;
    decode(&output, work)
}

fn load_header_output(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    work: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    let owner = NATIVE_DSL_HEADER_OWNER_V1;
    let reference = storage
        .maps
        .lookup_object(binding.owners, &owner_map_key(owner), work)?
        .ok_or_else(|| corrupt("Native DSL projection header is missing"))?;
    let record = contribution(storage, binding, owner, reference, work)?;
    if record.source.is_some() || record.source_owner.is_some() {
        return Err(corrupt("Native DSL header carries a file identity"));
    }
    load_output(storage, record, work)
}

fn load_corpus(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    constraints: Option<&QueryConstraintSetV1>,
    work: &mut WorkBudget,
) -> Result<AdmittedCorpus, IndexError> {
    let _sealed_chunk_index = publication::chunk_index(storage, binding, work)?;
    let header = load_header::<NativeDslCorpusV1>(storage, binding, work)?;
    let mut memory = vec![header._memory];
    let mut state = StructuralAuthorityState::default();
    visit_corpus_files(storage, binding, work, |_, _, file, work| {
        native_dsl::add_native_dsl_file_v1(&mut state, file.value, work.request())
            .map_err(|error| map_dsl_error(work, error, IndexError::CorruptObject))?;
        memory.push(file._memory);
        Ok(())
    })?;
    if let Some(constraints) = constraints {
        native_dsl::restrict_native_structural_v1(&mut state, constraints, work.request())
            .map_err(|error| query_error(work, error))?;
    }
    Ok(AdmittedCorpus {
        header: header.value,
        state: Arc::new(state),
        memory,
    })
}

fn visit_corpus_files(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    work: &mut WorkBudget,
    mut visit: impl FnMut(
        ObjectRef,
        ProjectionOwnerContribution,
        Decoded<NativeDslFileV1>,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut after: Option<quanta_index_core::index::MapPageEntry> = None;
    loop {
        let page = storage.maps.scan(
            binding.owners,
            after.as_ref().map(|row| row.key()).transpose()?,
            64,
            work,
        )?;
        let has_more = page.has_more;
        let mut rows = page.cursor();
        while let Some(row) = rows.next() {
            let (key, reference) = row.object_parts()?;
            if key.as_slice() != owner_map_key(NATIVE_DSL_HEADER_OWNER_V1).as_slice()
                && key.as_slice() != publication::CHUNK_INDEX
            {
                let owner = quanta_index_core::index::owner_from_map_key(key)?;
                let record = contribution(storage, binding, owner, *reference, work)?;
                let output = load_output(storage, record, work)?;
                visit(
                    *reference,
                    record,
                    decode::<NativeDslFileV1>(&output, work)?,
                    work,
                )?;
            }
            after = Some(row);
        }
        if !has_more {
            return Ok(());
        }
    }
}

fn require_complete_source_domain(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    header: &NativeDslCorpusV1,
    work: &mut WorkBudget,
) -> Result<(), IndexError> {
    let prefix = producer_owner_prefix(header.source_producer);
    let mut after = None;
    loop {
        let start = after
            .as_ref()
            .map_or(prefix.as_slice(), |key: &[u8; 38]| key.as_slice());
        let page = storage
            .maps
            .scan(binding.source.owners, Some(start), 64, work)?;
        for (key, value) in &page.entries {
            if !key.starts_with(&prefix) {
                return Ok(());
            }
            let owner = owner_from_map_key(key)?;
            let source = storage.objects.get(*value.as_object_ref()?, work)?;
            let record =
                SourceRecordOwner::decode_json(&source.payload, work, MemoryLifetime::Temporary)?;
            if matches!(record.text, SourceTextAdmission::Utf8 { .. })
                && storage
                    .maps
                    .lookup_object(
                        binding.owners,
                        &owner_map_key(native_dsl_file_owner_v1(owner)),
                        work,
                    )?
                    .is_none()
            {
                return Err(IndexError::SnapshotUnavailable);
            }
            after = Some(owner_map_key(owner));
        }
        if !page.has_more {
            return Ok(());
        }
    }
}

fn contribution(
    storage: DslStorage<'_>,
    binding: ProjectionBinding,
    owner: OwnerKey,
    reference: ObjectRef,
    work: &mut WorkBudget,
) -> Result<ProjectionOwnerContribution, IndexError> {
    let stored = storage.objects.get(reference, work)?;
    let record = decode_projection_owner(&stored.payload, work)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != record.object_children().as_slice()
        || record.owner != owner
        || record.producer != binding.producer
        || record.projection != binding.projection
    {
        return Err(corrupt("Native DSL contribution edges or owner differ"));
    }
    Ok(record)
}

fn load_output(
    storage: DslStorage<'_>,
    record: ProjectionOwnerContribution,
    work: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    let reference = record
        .output
        .ok_or_else(|| corrupt("Native DSL output is absent"))?;
    let stored = storage.objects.get(reference, work)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(corrupt("Native DSL output object edges differ"));
    }
    if quanta_index_contract::index::Digest(Sha256::digest(&stored.payload).into())
        != record.dependency_stamp
    {
        return Err(corrupt(
            "Native DSL output digest differs from its contribution",
        ));
    }
    work.checkpoint()?;
    Ok(stored)
}

fn decode<T: DeserializeOwned>(
    stored: &StoredObject,
    work: &WorkBudget,
) -> Result<Decoded<T>, IndexError> {
    let len = stored.payload.len();
    if len == 0 || len > MAX_DSL_OBJECT_BYTES {
        return Err(corrupt("Native DSL object size is outside its bound"));
    }
    let bytes = u64::try_from(len)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(16)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = work
        .reserve_memory(bytes, MemoryLifetime::Retained)?
        .ok_or_else(|| corrupt("Native DSL read has no allocation scope"))?;
    let value = quanta_index_ipc::decode_cbor_payload(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("Native DSL object: {error}")))?;
    work.checkpoint()?;
    Ok(Decoded {
        value,
        _memory: memory,
    })
}

fn validate_file(
    storage: DslStorage<'_>,
    source_cut: SourceCut,
    contribution: ProjectionOwnerContribution,
    header: &NativeDslCorpusV1,
    file: &NativeDslFileV1,
    work: &mut WorkBudget,
) -> Result<(), IndexError> {
    let (Some(owner), Some(reference)) = (contribution.source_owner, contribution.source) else {
        return Err(corrupt("Native DSL file lacks Source custody"));
    };
    if owner.producer != header.source_producer
        || native_dsl_file_owner_v1(owner) != contribution.owner
        || storage
            .maps
            .lookup_object(source_cut.owners, &owner_map_key(owner), work)?
            != Some(reference)
    {
        return Err(corrupt(
            "Native DSL file is stale or belongs to another Source",
        ));
    }
    let stored = storage.objects.get(reference, work)?;
    let record = SourceRecordOwner::decode_json(&stored.payload, work, MemoryLifetime::Temporary)?;
    if stored.registration.reference != reference
        || stored.registration.children.as_slice() != [record.bytes]
        || !matches!(record.text, SourceTextAdmission::Utf8 { .. })
    {
        return Err(corrupt(
            "Native DSL requires an authenticated UTF-8 Source file",
        ));
    }
    let repository = record
        .source_repo_id
        .as_ref()
        .unwrap_or(&header.pin.repo_id);
    storage
        .objects
        .visit_utf8_leaf(record.bytes, record.byte_len, work, &mut |text, work| {
            let bytes = u64::try_from(file.chunks.len())
                .ok()
                .and_then(|count| count.checked_mul(64))
                .ok_or(IndexError::ResourceLimited)?;
            let _line_memory = work.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            let mut line_bounds = Vec::with_capacity(file.chunks.len().saturating_mul(2));
            for chunk in &file.chunks {
                work.charge(WorkUsage {
                    bytes: u64::try_from(chunk.text.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let start = chunk.start_byte as usize;
                let end = chunk.end_byte as usize;
                if chunk.repo_relative_path.as_str() != record.path
                    || chunk.searchable_repo_id(&header.pin.repo_id) != repository
                    || record
                        .language
                        .as_ref()
                        .is_some_and(|language| language != &chunk.language)
                    || text.get(start..end) != Some(chunk.text.as_ref())
                {
                    return Err(corrupt("Native DSL chunk differs from its Source span"));
                }
                line_bounds.push((start, chunk.start_line));
                line_bounds.push((end, chunk.end_line));
            }
            line_bounds.sort_unstable_by_key(|(offset, _)| *offset);
            let mut offset = 0;
            let mut line = 1u32;
            for (next, expected_line) in line_bounds {
                for bytes in text.as_bytes()[offset..next].chunks(32 * 1024) {
                    work.charge(WorkUsage {
                        bytes: bytes.len() as u64,
                        ..WorkUsage::default()
                    })?;
                    let count = bytes.iter().filter(|&&byte| byte == b'\n').count() as u32;
                    line = line.checked_add(count).ok_or(IndexError::ResourceLimited)?;
                }
                if line != expected_line {
                    return Err(corrupt(
                        "Native DSL chunk lines differ from its Source span",
                    ));
                }
                offset = next;
            }
            Ok(())
        })
}

fn corrupt(message: &str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

fn query_error(work: &WorkBudget, error: quanta_index_core::CoreError) -> IndexError {
    map_dsl_error(work, error, IndexError::InvalidInput)
}

fn map_dsl_error(
    work: &WorkBudget,
    error: quanta_index_core::CoreError,
    fallback: fn(String) -> IndexError,
) -> IndexError {
    use quanta_index_core::CoreError;

    if let Err(denial) = work.checkpoint() {
        return denial;
    }
    match error {
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        } => IndexError::Cancelled,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        } => IndexError::ResourceLimited,
        error => fallback(error.to_string()),
    }
}

#[derive(Default)]
struct ReplySize {
    bytes: u64,
}

impl std::io::Write for ReplySize {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(buffer.len()).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("Native DSL reply size"))?;
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
