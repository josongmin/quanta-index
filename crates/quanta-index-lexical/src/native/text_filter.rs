//! Ticket-bound exact source-path selection for native text candidates.

use super::{NativeTextIndex, TextReadContext};
use quanta_index_contract::QueryConstraintSetV1;
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemoryLifetime, ObjectRef, OwnerKey, SourceRecordOwner,
    WorkUsage, owner_map_key,
};

const PATH_KEY_PREFIX: &[u8] = b"path\0";
const MAX_PATH_KEY_BYTES: usize = 4_096;

/// A resolved source identity; callers apply it while generating candidates,
/// before the bounded top-k heap can discard a matching result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnerSelection {
    Any,
    Absent,
    Exact { owner: OwnerKey, source: ObjectRef },
}

impl OwnerSelection {
    pub(super) fn matches(self, owner: OwnerKey, source: ObjectRef) -> Result<bool, IndexError> {
        match self {
            Self::Any => Ok(true),
            Self::Exact {
                owner: selected_owner,
                source: selected_source,
            } if owner == selected_owner => {
                if source != selected_source {
                    return Err(IndexError::CorruptObject(
                        "native text candidate differs from selected source owner".into(),
                    ));
                }
                Ok(true)
            }
            Self::Absent | Self::Exact { .. } => Ok(false),
        }
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
    let owner = quanta_index_core::index::owner_from_path_map_value(marker_ref, context.budget)?;
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
