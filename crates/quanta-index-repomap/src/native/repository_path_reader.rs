//! Repository-aware, ticket-bound pagination of the selected file inventory.

use std::ops::Deref;

use quanta_index_contract::native_protocol::{
    NativeFilePathEntry, NativeRepositoryPathCursor, NativeRepositoryPathEntry,
    NativeRepositoryPathPage,
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MapPageEntry, MapRoot, MapValue, MemoryLease, MemoryLifetime,
    QueryOutcome, QueryReadContext, SourceCut, SourcePathKey, SourcePathOwners,
    SourcePathRepositoryKey, SourceRecordOwner, WorkBudget, WorkUsage, effort::ResourceNeed,
    owner_from_path_map_value, owner_map_key, ports::IndexBuildPort,
    validate_source_path_repository_key,
};

use super::{
    FileMapReadContext, NativeFileMap, NativePathReader, PathReadRequest,
    file_map::{selected_source_cut, validate_path, validate_prefix},
};

#[derive(Debug)]
pub struct RepositoryPathReadRequest {
    pub prefix: String,
    pub after: Option<NativeRepositoryPathCursor>,
    pub limit: u32,
}

impl RepositoryPathReadRequest {
    pub fn validate(&self) -> Result<(), IndexError> {
        validate_prefix(&self.prefix)?;
        if !(1..=128).contains(&self.limit) {
            return Err(IndexError::InvalidInput(
                "file path page limit out of bounds".into(),
            ));
        }
        if let Some(after) = &self.after {
            validate_path(&after.path)?;
            if after.prefix != self.prefix || !after.path.starts_with(&self.prefix) {
                return Err(IndexError::InvalidInput(
                    "file path cursor differs from prefix".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Canonical response data and every grant for its row and cursor backing.
pub struct RepositoryFilePathPage {
    result: NativeRepositoryPathPage,
    row_memory: Vec<Option<MemoryLease>>,
    _rows_memory: Option<MemoryLease>,
    _cursor_memory: Option<MemoryLease>,
}

impl Deref for RepositoryFilePathPage {
    type Target = NativeRepositoryPathPage;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl RepositoryFilePathPage {
    fn new(
        view: quanta_index_core::index::ViewRef,
        limit: usize,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let bytes = limit
            .checked_mul(
                std::mem::size_of::<NativeRepositoryPathEntry>()
                    .checked_add(std::mem::size_of::<Option<MemoryLease>>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
        let mut entries = Vec::new();
        let mut row_memory = Vec::new();
        entries
            .try_reserve_exact(limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        row_memory
            .try_reserve_exact(limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if entries.capacity() > limit || row_memory.capacity() > limit {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            result: NativeRepositoryPathPage {
                view,
                entries,
                has_more: false,
                next_after: None,
            },
            row_memory,
            _rows_memory: memory,
            _cursor_memory: None,
        })
    }

    fn append(
        &mut self,
        marker: MapValue,
        expected_repo: Option<&[u8]>,
        path: &str,
        source: SourceCut,
        context: &mut QueryReadContext<'_>,
        limit: usize,
    ) -> Result<bool, IndexError> {
        let owner = owner_from_path_map_value(marker, context.budget)?;
        let reference = context
            .maps
            .lookup_object(source.owners, &owner_map_key(owner), context.budget)?
            .ok_or_else(|| IndexError::CorruptObject("file path owner lacks source".into()))?;
        let stored = context.objects.get(reference, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        let repo_key = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
        let actual_repo_key = repo_key.as_slice()?;
        if stored.registration.reference != reference
            || stored.registration.children.as_slice() != [record.bytes]
            || record.path != path
            || expected_repo.map_or(record.source_repo_id.is_some(), |key| {
                actual_repo_key != key
            })
        {
            return Err(IndexError::CorruptObject(
                "file path key/value mismatch".into(),
            ));
        }
        if self.result.entries.len() == limit {
            self.result.has_more = true;
            return Ok(false);
        }
        let (record, memory) = record.into_retained_parts(context.budget)?;
        self.result.entries.push(NativeRepositoryPathEntry {
            repository: record.source_repo_id,
            file: NativeFilePathEntry {
                owner,
                path: record.path,
                source: reference,
                bytes: record.bytes,
                byte_len: record.byte_len,
            },
        });
        self.row_memory.push(memory);
        Ok(true)
    }

    fn finish(
        mut self,
        request: &RepositoryPathReadRequest,
        instance: quanta_index_core::index::IndexInstanceId,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        if self.result.has_more {
            let last = self.result.entries.last().ok_or_else(|| {
                IndexError::BackendViolation("repository path page has no continuation row".into())
            })?;
            let bytes = last
                .file
                .path
                .len()
                .checked_add(request.prefix.len())
                .and_then(|bytes| {
                    bytes.checked_add(
                        last.repository
                            .as_ref()
                            .map_or(0, |repo| repo.as_str().len()),
                    )
                })
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or(IndexError::ResourceLimited)?;
            self._cursor_memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            self.result.next_after = Some(NativeRepositoryPathCursor {
                view: self.result.view,
                instance,
                prefix: request.prefix.clone(),
                path: last.file.path.clone(),
                repository: last.repository.clone(),
            });
        }
        Ok(self)
    }
}

pub struct NativeRepositoryPathReader {
    path_reader: NativePathReader,
}

impl NativeRepositoryPathReader {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        Ok(Self {
            path_reader: NativePathReader::new(need)?,
        })
    }
}

impl IndexQueryPort<RepositoryPathReadRequest, RepositoryFilePathPage>
    for NativeRepositoryPathReader
{
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        NativeFileMap.validate_instance(instance)
    }

    fn reservation(&self, request: &RepositoryPathReadRequest) -> Result<ResourceNeed, IndexError> {
        request.validate()?;
        // Same server-owned reservation as the existing FileMap query reader.
        self.path_reader.reservation(&PathReadRequest {
            prefix: String::new(),
            after: None,
            limit: request.limit,
        })
    }

    fn query(
        &self,
        request: &RepositoryPathReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<RepositoryFilePathPage>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        context.budget.checkpoint()?;
        let cursor_bytes = request
            .after
            .as_ref()
            .map_or(Some(0), |cursor| {
                cursor.path.len().checked_add(
                    cursor
                        .repository
                        .as_ref()
                        .map_or(0, |repository| repository.as_str().len()),
                )
            })
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = b"path\0"
            .len()
            .checked_add(request.prefix.len())
            .and_then(|bytes| bytes.checked_add(cursor_bytes))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        context.budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        if request.after.as_ref().is_some_and(|cursor| {
            cursor.view != context.ticket.view.identity
                || cursor.instance != context.instance.instance
        }) {
            return Err(IndexError::InvalidInput(
                "repository path cursor differs from selected view".into(),
            ));
        }
        let member = context.source_member_binding()?;
        let source = selected_source_cut(
            &member,
            &mut FileMapReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        let limit = usize::try_from(request.limit).map_err(|_error| IndexError::ResourceLimited)?;
        let mut output =
            RepositoryFilePathPage::new(context.ticket.view.identity, limit, context.budget)?;
        let initial_path = request
            .after
            .as_ref()
            .map_or(request.prefix.as_str(), |cursor| cursor.path.as_str());
        let initial = if initial_path.is_empty() || validate_path(initial_path).is_err() {
            None
        } else {
            let key = SourcePathKey::copy(initial_path, context.budget)?;
            context
                .maps
                .lookup(source.paths, key.as_slice(), context.budget)?
        };
        if request.after.is_some() && initial.is_none() {
            return Err(IndexError::InvalidInput(
                "file path cursor is absent".into(),
            ));
        }
        if let Some(marker) = initial {
            visit_path(
                &mut output,
                marker,
                initial_path,
                request.after.as_ref(),
                source,
                context,
                limit,
            )?;
        }
        let mut prior = SourcePathKey::copy(initial_path, context.budget)?;
        while !output.result.has_more {
            let page =
                context
                    .maps
                    .scan(source.paths, Some(prior.as_slice()), 1, context.budget)?;
            if page.entries.len() > 1 || page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "file path scan did not advance".into(),
                ));
            }
            let has_more = page.has_more;
            let Some(entry) = page.cursor().next() else {
                break;
            };
            let key = entry.key()?;
            if key <= prior.as_slice() {
                return Err(IndexError::BackendViolation(
                    "file path scan reversed its key".into(),
                ));
            }
            let path = key
                .strip_prefix(b"path\0")
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .ok_or_else(|| {
                    IndexError::CorruptObject("file path map has an invalid key".into())
                })?;
            validate_path(path)?;
            if !path.starts_with(&request.prefix) {
                break;
            }
            visit_path(
                &mut output,
                entry.get()?.1,
                path,
                None,
                source,
                context,
                limit,
            )?;
            prior = SourcePathKey::copy(path, context.budget)?;
            if !has_more {
                break;
            }
        }
        let output = output.finish(request, context.instance.instance, context.budget)?;
        QueryOutcome::core_scanned(context.ticket, context.instance, !output.has_more, output)
    }
}

fn visit_path(
    output: &mut RepositoryFilePathPage,
    marker: MapValue,
    path: &str,
    after: Option<&NativeRepositoryPathCursor>,
    source: SourceCut,
    context: &mut QueryReadContext<'_>,
    limit: usize,
) -> Result<(), IndexError> {
    match SourcePathOwners::decode(marker, context.budget)? {
        SourcePathOwners::LegacyOwner(_) => {
            if let Some(cursor) = after {
                // Read and authenticate the cursor before skipping this row.
                let owner = owner_from_path_map_value(marker, context.budget)?;
                let reference = context
                    .maps
                    .lookup_object(source.owners, &owner_map_key(owner), context.budget)?
                    .ok_or_else(|| {
                        IndexError::CorruptObject("file path cursor lacks source".into())
                    })?;
                let stored = context.objects.get(reference, context.budget)?;
                let record = SourceRecordOwner::decode_json(
                    &stored.payload,
                    context.budget,
                    MemoryLifetime::Temporary,
                )?;
                if record.path != path
                    || record.source_repo_id.is_some()
                    || stored.registration.reference != reference
                    || stored.registration.children.as_slice() != [record.bytes]
                {
                    return Err(IndexError::CorruptObject(
                        "file path key/value mismatch".into(),
                    ));
                }
                if cursor.repository.is_some() {
                    return Err(IndexError::InvalidInput(
                        "repository path cursor is absent".into(),
                    ));
                }
            } else {
                let _continue = output.append(marker, None, path, source, context, limit)?;
            }
        }
        SourcePathOwners::Repositories(root) => {
            let key = after
                .map(|cursor| SourcePathRepositoryKey::new(cursor.repository.as_ref()))
                .transpose()?;
            if let Some(key) = &key {
                let marker = context
                    .maps
                    .lookup(MapRoot(Some(root)), key.as_slice()?, context.budget)?
                    .ok_or_else(|| {
                        IndexError::InvalidInput("repository path cursor is absent".into())
                    })?;
                // Cursor authentication follows the same forward record checks.
                let mut cursor_page =
                    RepositoryFilePathPage::new(context.ticket.view.identity, 1, context.budget)?;
                let _continue =
                    cursor_page.append(marker, Some(key.as_slice()?), path, source, context, 1)?;
            }
            let mut previous: Option<MapPageEntry> = None;
            loop {
                let after_key = previous.as_ref().map(MapPageEntry::key).transpose()?.or(key
                    .as_ref()
                    .map(SourcePathRepositoryKey::as_slice)
                    .transpose()?);
                let page =
                    context
                        .maps
                        .scan(MapRoot(Some(root)), after_key, 128, context.budget)?;
                if page.entries.is_empty() {
                    if page.has_more || key.is_none() && previous.is_none() {
                        return Err(IndexError::CorruptObject(
                            "source path repository bucket is empty".into(),
                        ));
                    }
                    break;
                }
                if page.entries.len() > 128 {
                    return Err(IndexError::BackendViolation(
                        "repository path scan exceeds page".into(),
                    ));
                }
                let has_more = page.has_more;
                for entry in page.cursor() {
                    let repo = entry.key()?;
                    let prior_key = previous.as_ref().map(MapPageEntry::key).transpose()?.or(key
                        .as_ref()
                        .map(SourcePathRepositoryKey::as_slice)
                        .transpose()?);
                    if prior_key.is_some_and(|prior| repo <= prior) {
                        return Err(IndexError::BackendViolation(
                            "repository path scan reversed its key".into(),
                        ));
                    }
                    validate_source_path_repository_key(repo, context.budget)?;
                    if !output.append(entry.get()?.1, Some(repo), path, source, context, limit)? {
                        return Ok(());
                    }
                    previous = Some(entry);
                }
                if !has_more {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use quanta_index_contract::{
        RepoId,
        index::{Digest, OwnerKey, ProducerId, SourceRecord, SourceTextAdmission, StableOwnerId},
    };
    use quanta_index_core::index::{
        CatalogView, CollectionId, GroupActivation, IndexInstanceId, IndexKind, InputDomain,
        InstanceSpec, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite, ObjectBatchInput,
        ObjectRef, PublicationGroupId, ReadSessionRef, ReadTicketLease, RegisteredObject, SchemaId,
        SlotId, StorageFormatId, StoredObject, TenantId, TicketId, TicketRecord, ViewRef,
        WorkLimits, WorkspaceKey, WorktreeId,
        ports::{ImmutableObjectPort, MapEditCursor, ObjectWriteSink, PersistentMapPort},
    };

    use super::*;

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    fn owner() -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        }
    }

    fn selected() -> Result<(TicketRecord, InstanceSpec), IndexError> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let mut view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile: reference(5),
            source: SourceCut {
                owners: MapRoot(Some(reference(6))),
                paths: MapRoot(Some(reference(7))),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        view.identity.digest = view
            .identity_digest()
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let ticket = TicketRecord {
            lease: ReadTicketLease {
                id: TicketId([8; 16]),
                session: ReadSessionRef { id: 1, fence: 1 },
                fence: 1,
                expires_at_ms: 10_000,
            },
            view,
        };
        let instance = InstanceSpec {
            instance: IndexInstanceId([9; 16]),
            kind: IndexKind([10; 16]),
            group: PublicationGroupId([11; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([12; 16]),
            policy: Digest([13; 32]),
            format: StorageFormatId([14; 16]),
        };
        Ok((ticket, instance))
    }

    fn budget(bytes: u64) -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 0,
                bytes,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    struct SourcePorts {
        reference: ObjectRef,
        stored: StoredObject,
    }

    impl SourcePorts {
        fn new(repository: Option<RepoId>, valid_edge: bool) -> Result<Self, IndexError> {
            let bytes = reference(16);
            let reference = reference(15);
            let record = SourceRecord {
                path: "src/lib.rs".into(),
                bytes,
                byte_len: 0,
                text: SourceTextAdmission::Excluded,
                source_repo_id: repository,
                language: None,
            };
            let payload = serde_json::to_vec(&record)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            let stored = StoredObject::new(
                RegisteredObject::unscoped(
                    reference,
                    u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    if valid_edge { vec![bytes] } else { vec![] },
                ),
                payload,
                None,
            );
            Ok(Self { reference, stored })
        }
    }

    impl ImmutableObjectPort for SourcePorts {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected identify".into()))
        }
        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected put".into()))
        }
        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected install".into()))
        }
        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected batch install".into(),
            ))
        }
        fn get(
            &self,
            reference: ObjectRef,
            _: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            if reference != self.reference {
                return Err(IndexError::BackendViolation(
                    "unexpected object read".into(),
                ));
            }
            Ok(self.stored.clone())
        }
    }

    impl PersistentMapPort for SourcePorts {
        fn lookup(
            &self,
            _: MapRoot,
            key: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<MapValue>, IndexError> {
            Ok((key == owner_map_key(owner()).as_slice())
                .then_some(MapValue::Object(self.reference)))
        }
        fn scan(
            &self,
            _: MapRoot,
            _: Option<&[u8]>,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            Err(IndexError::BackendViolation("unexpected scan".into()))
        }
        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("unexpected apply".into()))
        }
        fn apply_batch(
            &self,
            _: &[MapBatchRequest],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected batch apply".into(),
            ))
        }
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected stream apply".into(),
            ))
        }
    }

    #[test]
    fn scalar_path_rejects_repository_record_and_corrupt_cursor_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ticket, instance) = selected()?;
        let marker = SourcePathOwners::LegacyOwner(owner()).encode()?;
        let source = ticket.view.source;
        let scoped = SourcePorts::new(Some(RepoId::new("repo-a")?), true)?;
        let mut work = budget(100_000)?;
        let mut context = QueryReadContext {
            ticket: &ticket,
            instance: &instance,
            objects: &scoped,
            maps: &scoped,
            budget: &mut work,
        };
        let mut page = RepositoryFilePathPage::new(ticket.view.identity, 1, context.budget)?;
        if !matches!(
            visit_path(
                &mut page,
                marker,
                "src/lib.rs",
                None,
                source,
                &mut context,
                1
            ),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err("scalar path accepted a repository record".into());
        }
        let corrupt = SourcePorts::new(None, false)?;
        let mut context = QueryReadContext {
            ticket: &ticket,
            instance: &instance,
            objects: &corrupt,
            maps: &corrupt,
            budget: &mut work,
        };
        let cursor = NativeRepositoryPathCursor {
            view: ticket.view.identity,
            instance: instance.instance,
            prefix: "src/".into(),
            path: "src/lib.rs".into(),
            repository: None,
        };
        let mut page = RepositoryFilePathPage::new(ticket.view.identity, 1, context.budget)?;
        if !matches!(
            visit_path(
                &mut page,
                marker,
                "src/lib.rs",
                Some(&cursor),
                source,
                &mut context,
                1,
            ),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err("corrupt cursor source edge was not reported as corruption".into());
        }
        Ok(())
    }

    #[test]
    fn cursor_request_bytes_are_charged_before_storage() -> Result<(), Box<dyn std::error::Error>> {
        let (ticket, instance) = selected()?;
        let ports = SourcePorts::new(None, true)?;
        let prefix = "a".repeat(quanta_index_core::index::MAX_SOURCE_PATH_BYTES);
        let repository = RepoId::new("repo-a")?;
        let expected_bytes = b"path\0"
            .len()
            .checked_add(prefix.len())
            .and_then(|bytes| bytes.checked_add(prefix.len()))
            .and_then(|bytes| bytes.checked_add(repository.as_str().len()))
            .ok_or("cursor charge overflow")?;
        let mut work = budget(
            u64::try_from(expected_bytes)?
                .checked_sub(1)
                .ok_or("empty cursor charge")?,
        )?;
        let request = RepositoryPathReadRequest {
            prefix: prefix.clone(),
            after: Some(NativeRepositoryPathCursor {
                view: ticket.view.identity,
                instance: instance.instance,
                prefix: prefix.clone(),
                path: prefix,
                repository: Some(repository),
            }),
            limit: 1,
        };
        let reader = NativeRepositoryPathReader::new(ResourceNeed {
            cpu_units: 1,
            io_bytes: 4096,
            memory_bytes: 4096,
            provider_units: 0,
        })?;
        let result = reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &instance,
                objects: &ports,
                maps: &ports,
                budget: &mut work,
            },
        );
        if !matches!(result, Err(IndexError::ResourceLimited)) {
            return Err("cursor bytes escaped work limit".into());
        }
        Ok(())
    }
}
