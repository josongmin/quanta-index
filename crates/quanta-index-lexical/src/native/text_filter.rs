//! Ticket-bound exact source-path selection for native text candidates.

use super::{NativeTextIndex, TextReadContext};
use quanta_index_contract::QueryConstraintSetV1;
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey,
    SourcePathOwners, SourceRecordOwner, WorkBudget, WorkUsage, owner_map_key,
    visit_repository_path_sources,
};

const PATH_KEY_PREFIX: &[u8] = b"path\0";
const MAX_PATH_KEY_BYTES: usize = 4_096;

/// A resolved source identity; callers apply it while generating candidates,
/// before the bounded top-k heap can discard a matching result.
#[derive(Debug)]
pub(super) enum OwnerSelection {
    Any,
    Absent,
    Exact { owner: OwnerKey, source: ObjectRef },
    Repositories(RepositoryOwners),
}

impl OwnerSelection {
    pub(super) fn matches(&self, owner: OwnerKey, source: ObjectRef) -> Result<bool, IndexError> {
        match self {
            Self::Any => Ok(true),
            Self::Exact {
                owner: selected_owner,
                source: selected_source,
            } if owner == *selected_owner => {
                if source != *selected_source {
                    return Err(IndexError::CorruptObject(
                        "native text candidate differs from selected source owner".into(),
                    ));
                }
                Ok(true)
            }
            Self::Absent | Self::Exact { .. } => Ok(false),
            Self::Repositories(selected) => {
                let Ok(position) = selected
                    .rows
                    .binary_search_by_key(&owner, |(owner, _)| *owner)
                else {
                    return Ok(false);
                };
                let (_, expected) = selected.rows.get(position).ok_or_else(|| {
                    IndexError::BackendViolation("repository owner selection lost its row".into())
                })?;
                if source != *expected {
                    return Err(IndexError::CorruptObject(
                        "native text candidate differs from selected source owner".into(),
                    ));
                }
                Ok(true)
            }
        }
    }
}

/// Owner identities are funded before growth. Sorting reuses the existing
/// buffer, and the retained grant outlives every candidate comparison.
#[derive(Debug, Default)]
pub(super) struct RepositoryOwners {
    rows: Vec<(OwnerKey, ObjectRef)>,
    memory: Option<MemoryLease>,
}

impl RepositoryOwners {
    fn push(
        &mut self,
        owner: OwnerKey,
        source: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.rows.len() == self.rows.capacity() {
            let capacity = self
                .rows
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = capacity
                .checked_mul(std::mem::size_of::<(OwnerKey, ObjectRef)>())
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            let mut rows = Vec::new();
            rows.try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if rows.capacity() > capacity {
                return Err(IndexError::ResourceLimited);
            }
            rows.append(&mut self.rows);
            self.rows = rows;
            self.memory = memory;
        }
        self.rows.push((owner, source));
        Ok(())
    }

    fn complete(mut self) -> Result<OwnerSelection, IndexError> {
        self.rows.sort_unstable_by_key(|(owner, _)| *owner);
        if self.rows.windows(2).any(|pair| {
            pair.first()
                .zip(pair.last())
                .is_some_and(|(first, last)| first.0 == last.0)
        }) {
            return Err(IndexError::CorruptObject(
                "repository path repeats the selected source owner".into(),
            ));
        }
        if self.rows.is_empty() {
            return Ok(OwnerSelection::Absent);
        }
        Ok(OwnerSelection::Repositories(self))
    }
}

/// Resolve an exact path through the selected ticket's reverse path map.
///
/// Prove its forward owner/source binding. A missing path is an
/// ordinary empty result; a dangling or inconsistent binding is corruption.
pub(super) fn resolve(
    index: &NativeTextIndex,
    constraints: &QueryConstraintSetV1,
    context: &mut TextReadContext<'_>,
) -> Result<OwnerSelection, IndexError> {
    context.budget.checkpoint()?;
    if !constraints.language_any_of.is_empty() {
        return Err(IndexError::InvalidInput(
            "native text language constraint requires producer language metadata".into(),
        ));
    }
    let Some(path) = constraints.repo_relative_path_exact.as_ref() else {
        return Ok(OwnerSelection::Any);
    };
    let path = path.as_str();
    // The public exact-path scalar permits 4096 bytes, while the native path
    // map can contain at most 4091 path bytes after its canonical prefix.
    // Longer valid queries therefore have no possible native match.
    if path.len() > MAX_SOURCE_PATH_BYTES {
        return Ok(OwnerSelection::Absent);
    }
    let key_len = path
        .len()
        .checked_add(PATH_KEY_PREFIX.len())
        .filter(|len| *len <= MAX_PATH_KEY_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let key_bytes = u64::try_from(key_len).map_err(|_error| IndexError::ResourceLimited)?;
    context.budget.charge(WorkUsage {
        bytes: key_bytes,
        ..WorkUsage::default()
    })?;
    let _key_memory = context
        .budget
        .reserve_memory(key_bytes, MemoryLifetime::Temporary)?;
    let mut key = Vec::new();
    key.try_reserve_exact(key_len)
        .map_err(|_error| IndexError::ResourceLimited)?;
    key.extend_from_slice(PATH_KEY_PREFIX);
    key.extend_from_slice(path.as_bytes());
    context.budget.checkpoint()?;

    let Some(marker_ref) =
        context
            .maps
            .lookup(context.ticket.view.source.paths, &key, context.budget)?
    else {
        context.budget.checkpoint()?;
        return Ok(OwnerSelection::Absent);
    };
    let owner = match SourcePathOwners::decode(marker_ref, context.budget)? {
        SourcePathOwners::LegacyOwner(owner) => owner,
        SourcePathOwners::Repositories(root) => {
            let mut selected = RepositoryOwners::default();
            visit_repository_path_sources(
                root,
                path,
                context.ticket.view.source.owners,
                context.maps,
                context.objects,
                context.budget,
                &mut |owner, source, record, budget| {
                    if index.validate_selected_source(&record)? {
                        selected.push(owner, source, budget)?;
                    }
                    Ok(())
                },
            )?;
            return selected.complete();
        }
    };
    context.budget.checkpoint()?;

    let source = context
        .maps
        .lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )?
        .ok_or_else(|| {
            IndexError::CorruptObject("native text path marker lacks source owner".into())
        })?;
    let stored = context.objects.get(source, context.budget)?;
    if stored.registration.reference != source || stored.registration.children.len() != 1 {
        return Err(IndexError::CorruptObject(
            "native text exact path source has invalid registration".into(),
        ));
    }
    let record =
        SourceRecordOwner::decode_json(&stored.payload, context.budget, MemoryLifetime::Temporary)?;
    context.budget.charge(WorkUsage {
        bytes: u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    if stored.registration.children.as_slice() != [record.bytes] || record.path != path {
        return Err(IndexError::CorruptObject(
            "native text exact path differs from selected source record".into(),
        ));
    }
    context.budget.checkpoint()?;
    if !index.validate_selected_source(&record)? {
        return Ok(OwnerSelection::Absent);
    }
    Ok(OwnerSelection::Exact { owner, source })
}
