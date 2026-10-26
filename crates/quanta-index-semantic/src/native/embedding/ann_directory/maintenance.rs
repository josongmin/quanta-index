//! Bounded retirement of regions with no member in the newly selected owner map.
//!
//! The directory root is staged by the original publication attempt. An old
//! Catalog view still owns its old region map and all of its physical children.

use quanta_index_core::index::{
    AttemptContext, IndexError, MapEdit, MapPage, MapValue, MemoryLease, MemoryLifetime, ObjectRef,
    OwnerKey, WorkUsage, owner_from_map_key,
};

use super::{
    AnnDirectory, EmbeddingRecipe, MAX_DELTA_OWNERS, MAX_REGIONS, ReadContext, ann_files,
    load_member,
};

const REGION_SCAN_PAGE: u32 = 64;

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

fn region_page(
    directory: &AnnDirectory,
    after: Option<u64>,
    limit: u32,
    context: &mut AttemptContext<'_>,
) -> Result<MapPage, IndexError> {
    directory.region_page(
        after,
        limit,
        &mut ReadContext {
            objects: context.objects,
            maps: context.maps,
            budget: context.budget,
        },
    )
}

fn ordinal(key: &[u8]) -> Result<u64, IndexError> {
    let bytes: [u8; 8] = key
        .try_into()
        .map_err(|_error| corrupt("ANN maintenance region key width differs"))?;
    Ok(u64::from_be_bytes(bytes))
}

/// Authenticate the new selected owner before using its member reference as
/// retirement authority. Pending coverage with the same reference stays live.
pub(super) fn selected_member(
    directory: &AnnDirectory,
    owner: OwnerKey,
    key: &[u8],
    context: &mut AttemptContext<'_>,
) -> Result<Option<ObjectRef>, IndexError> {
    let value = context.maps.lookup(directory.owners, key, context.budget)?;
    selected_member_value(directory, owner, key, value, context)
}

pub(super) fn selected_member_value(
    directory: &AnnDirectory,
    owner: OwnerKey,
    key: &[u8],
    value: Option<MapValue>,
    context: &mut AttemptContext<'_>,
) -> Result<Option<ObjectRef>, IndexError> {
    let Some(selected_ref) = value else {
        return Ok(None);
    };
    let selected = quanta_index_core::index::read_owner_coverage(
        selected_ref,
        key,
        context.objects,
        context.budget,
    )?;
    if selected.owner != owner || selected.desired.derivation_policy != directory.recipe {
        return Err(corrupt("ANN maintenance selected owner differs"));
    }
    Ok(selected.searchable)
}

pub(super) struct SelectedPage {
    values: Vec<Option<MapValue>>,
    _memory: Option<MemoryLease>,
}

impl SelectedPage {
    pub(super) fn value(&self, index: usize) -> Result<Option<MapValue>, IndexError> {
        self.values.get(index).copied().ok_or_else(|| {
            IndexError::BackendViolation("ANN selected lookup page lost a row".into())
        })
    }
}

/// Own the bounded key/value backing before allocation.
///
/// These are immutable
/// map values only; each coverage record is authenticated after member/artifact
/// authentication by the same `selected_member_value` used by scalar retirement.
pub(super) fn selected_page(
    directory: &AnnDirectory,
    page: &MapPage,
    after: Option<&[u8]>,
    context: &mut AttemptContext<'_>,
) -> Result<SelectedPage, IndexError> {
    let count = page.entries.len();
    if count > quanta_index_core::index::ports::MAX_MAP_LOOKUP_BATCH {
        return Err(IndexError::BackendViolation(
            "ANN lookup page exceeds bounded source page".into(),
        ));
    }
    let mut previous: Option<&[u8]> = after;
    for (key, _) in &page.entries {
        let _owner = owner_from_map_key(key)?;
        if previous.is_some_and(|prior| prior >= key.as_slice()) {
            return Err(corrupt("ANN selected lookup keys are not ordered"));
        }
        previous = Some(key);
    }
    let bytes = std::mem::size_of::<Option<MapValue>>()
        .checked_add(std::mem::size_of::<&[u8]>())
        .and_then(|stride| count.checked_mul(stride))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
    context.budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    let memory = context
        .budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    values.resize(count, None);
    let mut keys = Vec::new();
    keys.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    keys.extend(page.entries.iter().map(|(key, _)| key.as_slice()));
    if values.capacity() > count || keys.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    context
        .maps
        .lookup_batch(directory.owners, &keys, &mut values, context.budget)?;
    drop(keys);
    Ok(SelectedPage {
        values,
        _memory: memory,
    })
}

fn fully_obsolete(
    directory: &AnnDirectory,
    id: u64,
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<bool, IndexError> {
    context.budget.checkpoint()?;
    let loaded = directory.region_from_page(
        id,
        reference,
        &mut ReadContext {
            objects: context.objects,
            maps: context.maps,
            budget: context.budget,
        },
    )?;
    if loaded.object.registration.reference != reference {
        return Err(corrupt("ANN maintenance region map changed its reference"));
    }
    let descriptor = &loaded.descriptor;
    // Retirement needs the authenticated descriptor and seal, but must not
    // hydrate and hash every Lance data chunk merely to unlink a dead root.
    let fileset =
        ann_files::load_fileset_seal(context.objects, descriptor.fileset, context.budget)?;
    if fileset.seal.recipe != directory.recipe
        || fileset.seal.generation != id.checked_add(1).ok_or(IndexError::ResourceLimited)?
        || fileset.seal.rows != descriptor.rows
        || usize::try_from(fileset.seal.dimension).map_err(|_error| IndexError::ResourceLimited)?
            != descriptor.center.len()
    {
        return Err(corrupt(
            "ANN maintenance region seal differs from directory",
        ));
    }
    let mut after: Option<[u8; 38]> = None;
    let mut inspected = 0u32;
    // A live region usually proves itself with its first selected member.
    // Only a dead first member needs the original bounded page traversal.
    let mut scan_limit = 1;
    loop {
        let members = context.maps.scan(
            descriptor.members,
            after.as_ref().map(<[u8; 38]>::as_slice),
            scan_limit,
            context.budget,
        )?;
        if members.entries.len()
            > usize::try_from(scan_limit).map_err(|_error| IndexError::ResourceLimited)?
            || (members.entries.is_empty() && members.has_more)
        {
            return Err(IndexError::BackendViolation(
                "ANN maintenance member page made no bounded progress".into(),
            ));
        }
        for (key, member_ref) in &members.entries {
            let member_ref = member_ref.as_object_ref()?;
            context.budget.checkpoint()?;
            let cursor: [u8; 38] = key
                .as_slice()
                .try_into()
                .map_err(|_error| corrupt("ANN maintenance member key width differs"))?;
            if after.is_some_and(|prior| cursor <= prior) {
                return Err(corrupt("ANN maintenance region members are not ordered"));
            }
            after = Some(cursor);
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if inspected > descriptor.rows {
                return Err(corrupt("ANN maintenance region member bound differs"));
            }
            let owner = owner_from_map_key(key)?;
            let member = load_member(*member_ref, owner, context.objects, context.budget)?;
            if member.input.derivation_policy != directory.recipe {
                return Err(corrupt("ANN maintenance member recipe differs"));
            }
            // Pending coverage can become current with this same member.
            if selected_member(directory, owner, key, context)? == Some(*member_ref) {
                return Ok(false);
            }
        }
        if !members.has_more {
            break;
        }
        scan_limit = MAX_DELTA_OWNERS;
    }
    if inspected == 0 {
        return Err(corrupt("ANN maintenance region has no members"));
    }
    Ok(true)
}

/// Remove selected delta entries that the new owner map no longer points to.
/// The old page is immutable; each removal is a COW edit in this same attempt.
pub(super) fn prune_delta(
    directory: &mut AnnDirectory,
    recipe: &EmbeddingRecipe,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if directory.recipe != recipe.fingerprint() {
        return Err(IndexError::InvalidInput(
            "ANN maintenance delta recipe differs".into(),
        ));
    }
    if directory.delta_owners == 0 {
        return Ok(());
    }
    let page = context
        .maps
        .scan(directory.delta, None, MAX_DELTA_OWNERS, context.budget)?;
    if page.has_more
        || page.entries.len()
            != usize::try_from(directory.delta_owners)
                .map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(corrupt("ANN maintenance delta count differs from map"));
    }
    let mut previous: Option<&[u8]> = None;
    for (key, _reference) in &page.entries {
        context.budget.checkpoint()?;
        if previous.is_some_and(|prior| prior >= key.as_slice()) {
            return Err(corrupt("ANN maintenance delta entries are not ordered"));
        }
        previous = Some(key);
        let _owner = owner_from_map_key(key)?;
    }
    for (key, member_ref) in &page.entries {
        let member_ref = member_ref.as_object_ref()?;
        context.budget.checkpoint()?;
        let owner = owner_from_map_key(key)?;
        let member = load_member(*member_ref, owner, context.objects, context.budget)?;
        if member.input.derivation_policy != directory.recipe {
            return Err(corrupt("ANN maintenance delta member recipe differs"));
        }
        if selected_member(directory, owner, key, context)? != Some(*member_ref) {
            directory.remove_delta(owner, recipe, context)?;
        }
    }
    Ok(())
}

pub(super) fn unlink(
    directory: &mut AnnDirectory,
    id: u64,
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    context.budget.checkpoint()?;
    let _key_memory = context
        .budget
        .reserve_memory(8, MemoryLifetime::Temporary)?;
    let key = id.to_be_bytes().to_vec();
    let regions = context.apply(
        directory.regions,
        &[MapEdit {
            key,
            expected: Some((reference).into()),
            next: None,
        }],
    )?;
    let count = directory
        .region_count
        .checked_sub(1)
        .ok_or_else(|| corrupt("ANN maintenance region count underflow"))?;
    if (count == 0) != regions.0.is_none() {
        return Err(corrupt("ANN maintenance region map/count mismatch"));
    }
    directory.regions = regions;
    directory.region_count = count;
    if count == 0 {
        directory.maintenance_after = None;
    }
    Ok(())
}

/// Inspect one region per ordinary publication, wrapping the persistent cursor.
/// A refusal leaves the caller's directory unchanged; the attempt is not
/// published if later staging fails.
pub(super) fn sweep(
    directory: &mut AnnDirectory,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if directory.region_count == 0 {
        directory.maintenance_after = None;
        return Ok(());
    }
    let mut page = region_page(directory, directory.maintenance_after, 1, context)?;
    if page.entries.is_empty() && directory.maintenance_after.is_some() && !page.has_more {
        page = region_page(directory, None, 1, context)?;
    }
    let (key, reference) = page
        .entries
        .first()
        .ok_or_else(|| corrupt("ANN maintenance region cursor lost its continuation"))?;
    if page.entries.len() != 1 {
        return Err(corrupt(
            "ANN maintenance single-region page exceeded its limit",
        ));
    }
    let id = ordinal(key)?;
    let reference = *reference;
    let reference = reference.object_ref()?;
    let obsolete = fully_obsolete(directory, id, reference, context)?;
    if obsolete {
        unlink(directory, id, reference, context)?;
    }
    directory.maintenance_after = if page.has_more { Some(id) } else { None };
    Ok(())
}

fn ensure_capacity_at(
    directory: &mut AnnDirectory,
    context: &mut AttemptContext<'_>,
    limit: u64,
) -> Result<(), IndexError> {
    if limit == 0 || limit > MAX_REGIONS {
        return Err(IndexError::InvalidInput(
            "ANN region capacity test limit differs from policy".into(),
        ));
    }
    // The next seal generation is id + 1. Refuse overflow before opening Lance.
    let _generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    if u64::from(directory.region_count) < limit {
        return Ok(());
    }
    // A full directory must find an obsolete slot before the expensive build.
    // Scan the entire bounded active keyset so a dead high ordinal cannot be
    // hidden behind live low ordinals. Pages remain <=64 entries.
    let mut after = None;
    let mut inspected = 0u64;
    let mut retired = None;
    loop {
        context.budget.checkpoint()?;
        let page = region_page(directory, after, REGION_SCAN_PAGE, context)?;
        if page.entries.is_empty() && page.has_more {
            return Err(corrupt("ANN maintenance full-cap scan made no progress"));
        }
        for (key, reference) in &page.entries {
            let reference = reference.as_object_ref()?;
            let id = ordinal(key)?;
            if after.is_some_and(|previous| id <= previous) {
                return Err(corrupt("ANN maintenance region scan did not advance"));
            }
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if inspected > MAX_REGIONS {
                return Err(corrupt("ANN maintenance region map exceeds active cap"));
            }
            if retired.is_none() && fully_obsolete(directory, id, *reference, context)? {
                retired = Some((id, *reference));
            }
            after = Some(id);
        }
        if !page.has_more {
            break;
        }
    }
    if inspected != u64::from(directory.region_count) {
        return Err(corrupt("ANN maintenance region count differs from map"));
    }
    if inspected > limit {
        return Err(IndexError::ResourceLimited);
    }
    let (id, reference) = retired.ok_or(IndexError::ResourceLimited)?;
    unlink(directory, id, reference, context)
}

/// Prove capacity before a region build; the original attempt stages the COW
/// unlink and the next append under one Catalog compare-and-swap.
pub(super) fn ensure_capacity(
    directory: &mut AnnDirectory,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    ensure_capacity_at(directory, context, MAX_REGIONS)
}

#[cfg(test)]
pub(super) fn ensure_capacity_for_test(
    directory: &mut AnnDirectory,
    context: &mut AttemptContext<'_>,
    limit: u64,
) -> Result<(), IndexError> {
    ensure_capacity_at(directory, context, limit)
}
