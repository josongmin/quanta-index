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
