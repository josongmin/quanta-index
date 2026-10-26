//! Size-tiered coalescing reuses owner artifacts and the original COW attempt.
//! No intermediate Lance file-set is built and no selected view is mutated.

use quanta_index_core::index::{
    AttemptContext, IndexError, MapEdit, MapRoot, MemoryLease, MemoryLifetime, ObjectRef,
    OwnerMemberBinding, WorkBudget, WorkUsage, owner_from_map_key,
};

use super::super::{LoadedOwnerArtifact, load_query_owner_artifact};
use super::{
    AnnDirectory, EmbeddingRecipe, MAX_DELTA_OWNERS, MAX_REGIONS, ReadContext, ann_files,
    ann_lance, corrupt, load_member, maintenance,
};

const MAX_MERGED_REGIONS: u32 = 8;
const MAX_INSPECTED_REGIONS: u32 = 64;
// Bound foreground carry by source windows, not only fan-in: eight binary
// levels can otherwise pull thousands of owners into one fixed-lifetime job.
const MAX_CARRY_ROWS: u32 = MAX_DELTA_OWNERS * MAX_MERGED_REGIONS;

pub(in super::super) struct RegionMembers {
    pub root: MapRoot,
    pub owners: u32,
    pub rows: u32,
    pub artifacts: Vec<SelectedRegionMember>,
    artifacts_memory: Option<MemoryLease>,
}

pub(in super::super) struct SelectedRegionMember {
    pub reference: ObjectRef,
    pub binding: OwnerMemberBinding,
    pub artifact: LoadedOwnerArtifact,
}

impl RegionMembers {
    fn reserve_artifacts(
        &mut self,
        additional: usize,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let needed = self
            .artifacts
            .len()
            .checked_add(additional)
            .ok_or(IndexError::ResourceLimited)?;
        if needed <= self.artifacts.capacity() {
            return Ok(());
        }
        let bytes = needed
            .checked_mul(std::mem::size_of::<SelectedRegionMember>())
            .ok_or(IndexError::ResourceLimited)?;
        // A reallocation temporarily owns both buffers. Keep the previous
        // grant until the new reservation and allocation have succeeded.
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        self.artifacts
            .try_reserve_exact(additional)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if self.artifacts.capacity() > needed {
            return Err(IndexError::ResourceLimited);
        }
        self.artifacts_memory = memory;
        Ok(())
    }
}

fn row_limit(recipe: &EmbeddingRecipe) -> Result<u32, IndexError> {
    let cells =
        u32::try_from(ann_lance::MAX_REGION_CELLS).map_err(|_error| IndexError::ResourceLimited)?;
    let rows =
        u32::try_from(ann_lance::MAX_REGION_ROWS).map_err(|_error| IndexError::ResourceLimited)?;
    Ok(cells
        .checked_div(recipe.dimension)
        .ok_or(IndexError::ResourceLimited)?
        .min(rows))
}

/// Copy only still-selected members, including pending coverage that retains
/// the same searchable member. Duplicate references contribute rows only once.
fn include(
    result: &mut RegionMembers,
    source: MapRoot,
    maximum_rows: u32,
    directory: &AnnDirectory,
    recipe: &EmbeddingRecipe,
    context: &mut AttemptContext<'_>,
) -> Result<SourceCounts, IndexError> {
    // Prior sources are sorted at the end of each include. A source map itself
    // has unique, ordered owner keys, so only the prior prefix can duplicate.
    let prior_owners = result.artifacts.len();
    result.reserve_artifacts(
        usize::try_from(maximum_rows).map_err(|_error| IndexError::ResourceLimited)?,
        context.budget,
    )?;
    let mut after: Option<[u8; 38]> = None;
    let mut inspected = 0u32;
    let mut source_rows = 0u32;
    let mut exact_matches = 0u32;
    loop {
        let page = context.maps.scan(
            source,
            after.as_ref().map(<[u8; 38]>::as_slice),
            MAX_DELTA_OWNERS,
            context.budget,
        )?;
        if page.entries.len()
            > usize::try_from(MAX_DELTA_OWNERS).map_err(|_error| IndexError::ResourceLimited)?
            || (page.entries.is_empty() && page.has_more)
        {
            return Err(IndexError::BackendViolation(
                "ANN consolidation page made no bounded progress".into(),
            ));
        }
        let selected = maintenance::selected_page(
            directory,
            &page,
            after.as_ref().map(<[u8; 38]>::as_slice),
            context,
        )?;
        for (index, (key, reference)) in page.entries.iter().enumerate() {
            let reference = reference.as_object_ref()?;
            context.budget.checkpoint()?;
            let cursor: [u8; 38] = key
                .as_slice()
                .try_into()
                .map_err(|_error| corrupt("ANN consolidation member key width differs"))?;
            if after.is_some_and(|previous| cursor <= previous) {
                return Err(corrupt("ANN consolidation members are not ordered"));
            }
            after = Some(cursor);
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if inspected > maximum_rows {
                return Err(corrupt(
                    "ANN consolidation member count exceeds source rows",
                ));
            }
            let owner = owner_from_map_key(key)?;
            let binding = load_member(*reference, owner, context.objects, context.budget)?;
            let artifact =
                load_query_owner_artifact(context.objects, context.budget, &binding, recipe)?;
            let rows = u32::try_from(artifact.vectors.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            source_rows = source_rows
                .checked_add(rows)
                .ok_or(IndexError::ResourceLimited)?;
            if source_rows > maximum_rows {
                return Err(corrupt("ANN consolidation source exceeds declared rows"));
            }
            if rows == 0 {
                return Err(corrupt("ANN sealed member has no rows"));
            }
            if maintenance::selected_member_value(
                directory,
                owner,
                key,
                selected.value(index)?,
                context,
            )? != Some(*reference)
            {
                continue;
            }
            exact_matches = exact_matches
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            let prior_members = result.artifacts.get(..prior_owners).ok_or_else(|| {
                IndexError::BackendViolation("ANN consolidation prior owners differ".into())
            })?;
            if let Ok(index) =
                prior_members.binary_search_by_key(&owner, |entry| entry.binding.owner)
            {
                let prior = prior_members.get(index).ok_or_else(|| {
                    IndexError::BackendViolation("ANN consolidation prior member absent".into())
                })?;
                if prior.reference != *reference {
                    return Err(corrupt("ANN consolidation selected member conflicts"));
                }
                continue;
            }
            let next_rows = result
                .rows
                .checked_add(rows)
                .ok_or(IndexError::ResourceLimited)?;
            if next_rows > row_limit(recipe)? {
                return Err(IndexError::ResourceLimited);
            }
            result.rows = next_rows;
            result.owners = result
                .owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            result.artifacts.push(SelectedRegionMember {
                reference: *reference,
                binding,
                artifact,
            });
        }
        if !page.has_more {
            break;
        }
    }
    if inspected == 0 || source_rows != maximum_rows {
        return Err(corrupt("ANN consolidation source row count differs"));
    }
    context.budget.checkpoint()?;
    result
        .artifacts
        .sort_unstable_by_key(|entry| entry.binding.owner);
    context.budget.checkpoint()?;
    Ok(SourceCounts {
        owners: inspected,
        exact_matches,
    })
}

#[derive(Clone, Copy)]
struct SourceCounts {
    owners: u32,
    exact_matches: u32,
}

fn install_selected_members(
    result: &mut RegionMembers,
    base: MemberBase,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let owner_count = result.artifacts.len();
    if owner_count
        != usize::try_from(result.owners).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(corrupt("ANN consolidation selected owner count differs"));
    }
    if owner_count == 0 {
        return Ok(());
    }
    if (base.root.0.is_none() != (base.owners == 0))
        || base.owners
            > u32::try_from(ann_lance::MAX_REGION_ROWS)
                .map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(corrupt("ANN consolidation base member count differs"));
    }
    let maximum_edits = owner_count
        .checked_add(usize::try_from(base.owners).map_err(|_error| IndexError::ResourceLimited)?)
        .ok_or(IndexError::ResourceLimited)?;
    let mut edits = MemberDiffEdits::new(maximum_edits);
    let mut after: Option<[u8; 38]> = None;
    let mut scanned = 0u32;
    let mut selected_at = 0usize;
    while base.root.0.is_some() {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            base.root,
            after.as_ref().map(<[u8; 38]>::as_slice),
            MAX_DELTA_OWNERS,
            context.budget,
        )?;
        if page.entries.len()
            > usize::try_from(MAX_DELTA_OWNERS).map_err(|_error| IndexError::ResourceLimited)?
            || (page.entries.is_empty() && page.has_more)
        {
            return Err(IndexError::BackendViolation(
                "ANN consolidation base scan made no bounded progress".into(),
            ));
        }
        for (key, old_value) in &page.entries {
            context.budget.checkpoint()?;
            let cursor: [u8; 38] = key
                .as_slice()
                .try_into()
                .map_err(|_error| corrupt("ANN consolidation base key width differs"))?;
            if after.is_some_and(|previous| cursor <= previous) {
                return Err(corrupt("ANN consolidation base members are not ordered"));
            }
            after = Some(cursor);
            scanned = scanned.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if scanned > base.owners {
                return Err(corrupt(
                    "ANN consolidation base member count exceeds source",
                ));
            }
            let old = *old_value.as_object_ref()?;
            let _owner = owner_from_map_key(key)?;
            loop {
                let Some(selected) = result.artifacts.get(selected_at) else {
                    edits.push(key, Some(old), None, context.budget)?;
                    break;
                };
                let selected_key = quanta_index_core::index::owner_map_key(selected.binding.owner);
                match selected_key.as_slice().cmp(key.as_slice()) {
                    std::cmp::Ordering::Less => {
                        edits.push(
                            &selected_key,
                            None,
                            Some(selected.reference),
                            context.budget,
                        )?;
                        selected_at = selected_at
                            .checked_add(1)
                            .ok_or(IndexError::ResourceLimited)?;
                    }
                    std::cmp::Ordering::Equal => {
                        if old != selected.reference {
                            edits.push(key, Some(old), Some(selected.reference), context.budget)?;
                        }
                        selected_at = selected_at
                            .checked_add(1)
                            .ok_or(IndexError::ResourceLimited)?;
                        break;
                    }
                    std::cmp::Ordering::Greater => {
                        edits.push(key, Some(old), None, context.budget)?;
                        break;
                    }
                }
            }
        }
        if !page.has_more {
            break;
        }
    }
    if scanned != base.owners {
        return Err(corrupt("ANN consolidation base member count differs"));
    }
    for selected in result
        .artifacts
        .get(selected_at..)
        .ok_or(IndexError::ResourceLimited)?
    {
        context.budget.checkpoint()?;
        let key = quanta_index_core::index::owner_map_key(selected.binding.owner);
        edits.push(&key, None, Some(selected.reference), context.budget)?;
    }
    context.budget.checkpoint()?;
    result.root = if edits.values.is_empty() {
        base.root
    } else {
        context.apply(base.root, &edits.values)?
    };
    Ok(())
}

#[derive(Clone, Copy)]
struct MemberBase {
    root: MapRoot,
    owners: u32,
    // A positive score proves a smaller diff than an empty-root rebuild.
    score: i64,
}

impl MemberBase {
    fn consider(&mut self, root: MapRoot, source: SourceCounts) -> Result<(), IndexError> {
        if root.0.is_none() || source.owners == 0 || source.exact_matches > source.owners {
            return Err(corrupt("ANN consolidation source base differs"));
        }
        let score = i64::from(source.exact_matches)
            .checked_mul(2)
            .and_then(|matched| matched.checked_sub(i64::from(source.owners)))
            .ok_or(IndexError::ResourceLimited)?;
        // Earlier include order wins ties. Declared rows count vectors, not map
        // entries, and a mostly stale large region is a poor COW base.
        if score > self.score {
            *self = Self {
                root,
                owners: source.owners,
                score,
            };
        }
        Ok(())
    }
}

/// Each live key buffer is covered by the same grant as its Vec slot. Growth
/// admits both old and new capacities before moving existing edits.
struct MemberDiffEdits {
    values: Vec<MapEdit>,
    maximum: usize,
    memory: Option<MemoryLease>,
}

impl MemberDiffEdits {
    fn new(maximum: usize) -> Self {
        Self {
            values: Vec::new(),
            maximum,
            memory: None,
        }
    }

    fn push(
        &mut self,
        key: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if key.len() != 38
            || self
                .values
                .last()
                .is_some_and(|prior| prior.key.as_slice() >= key)
        {
            return Err(corrupt("ANN consolidation diff is not ordered and unique"));
        }
        if self.values.len() == self.maximum {
            return Err(corrupt("ANN consolidation diff exceeds member bound"));
        }
        budget.charge(WorkUsage {
            bytes: 38,
            ..WorkUsage::default()
        })?;
        if self.values.len() == self.values.capacity() {
            let target = self
                .values
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .min(self.maximum);
            let stride = std::mem::size_of::<MapEdit>()
                .checked_add(38)
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    target
                        .checked_add(self.values.len())
                        .and_then(|slots| slots.checked_mul(std::mem::size_of::<MapEdit>()))
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let memory = budget.reserve_memory(
                u64::try_from(
                    target
                        .checked_mul(stride)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            let mut expanded = Vec::new();
            expanded
                .try_reserve_exact(target)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if expanded.capacity() > target {
                return Err(IndexError::ResourceLimited);
            }
            expanded.append(&mut self.values);
            let old_values = std::mem::replace(&mut self.values, expanded);
            let old_memory = std::mem::replace(&mut self.memory, memory);
            drop(old_values);
            drop(old_memory);
        }
        let mut owned_key = Vec::new();
        owned_key
            .try_reserve_exact(38)
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned_key.extend_from_slice(key);
        self.values.push(MapEdit {
            key: owned_key,
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        });
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct MergeCandidate {
    ordinal: u64,
    reference: ObjectRef,
    members: MapRoot,
    rows: u32,
    fileset: ObjectRef,
    dimension: usize,
}

/// Scan once, starting at the existing persistent maintenance cursor. Keep
/// candidate metadata for all carry rounds instead of repeatedly reading it.
fn load_candidates(
    directory: &AnnDirectory,
    candidates: &mut Vec<MergeCandidate>,
    limit: u32,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let start = directory.maintenance_after;
    let mut after = start;
    let mut wrapped = false;
    let mut inspected = 0u32;
    while inspected < limit {
        let page = directory.region_page(
            after,
            limit
                .checked_sub(inspected)
                .ok_or(IndexError::ResourceLimited)?
                .min(64),
            &mut ReadContext {
                objects: context.objects,
                maps: context.maps,
                budget: context.budget,
            },
        )?;
        for (key, reference) in &page.entries {
            let reference = reference.as_object_ref()?;
            let id = u64::from_be_bytes(
                key.as_slice()
                    .try_into()
                    .map_err(|_error| corrupt("ANN consolidation region key width differs"))?,
            );
            if wrapped && start.is_some_and(|first| id > first) {
                if inspected != directory.region_count {
                    return Err(corrupt("ANN consolidation region count differs"));
                }
                return Ok(());
            }
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if inspected > directory.region_count {
                return Err(corrupt("ANN consolidation region count exceeds directory"));
            }
            let region = directory.region_from_page(
                id,
                *reference,
                &mut ReadContext {
                    objects: context.objects,
                    maps: context.maps,
                    budget: context.budget,
                },
            )?;
            let descriptor = &region.descriptor;
            candidates.push(MergeCandidate {
                ordinal: id,
                reference: *reference,
                members: descriptor.members,
                rows: descriptor.rows,
                fileset: descriptor.fileset,
                dimension: descriptor.center.len(),
            });
            after = Some(id);
        }
        if !page.has_more {
            if start.is_some() && !wrapped {
                after = None;
                wrapped = true;
            } else {
                if inspected != directory.region_count {
                    return Err(corrupt("ANN consolidation region count differs"));
                }
                break;
            }
        }
    }
    Ok(())
}

pub(super) fn prepare(
    directory: &mut AnnDirectory,
    recipe: &EmbeddingRecipe,
    context: &mut AttemptContext<'_>,
) -> Result<RegionMembers, IndexError> {
    let maximum = row_limit(recipe)?;
    let mut result = RegionMembers {
        root: MapRoot(None),
        owners: 0,
        rows: 0,
        artifacts: Vec::new(),
        artifacts_memory: None,
    };
    let delta_counts = include(
        &mut result,
        directory.delta,
        directory.delta_rows,
        directory,
        recipe,
        context,
    )?;
    if delta_counts.owners != directory.delta_owners {
        return Err(corrupt("ANN consolidation delta owner count differs"));
    }
    // An entirely superseded delta has no new search rows to publish. Leave
    // unrelated live runs alone, especially at the active-region cap.
    if result.rows == 0 {
        return Ok(result);
    }
    let at_capacity = u64::from(directory.region_count) == MAX_REGIONS;
    // Ordinary carry cannot pull an arbitrarily large run into a small job.
    // At the original region cap, preserve the existing pressure path's cell
    // bound so a fitting larger run can still release a slot. Original work,
    // memory and lifetime admission remain authoritative in either case.
    let carry_rows = if at_capacity {
        maximum
    } else {
        maximum.min(MAX_CARRY_ROWS.max(result.rows))
    };
    let mut base = MemberBase {
        root: MapRoot(None),
        owners: 0,
        score: 0,
    };
    base.consider(directory.delta, delta_counts)?;
    // Binary carry: compare against the growing run, not the original delta.
    // The region count, source membership and cells remain explicitly bounded.
    // At the original active cap inspect its bounded full set, allowing one
    // fitting larger run to release a slot. Ordinary seals rotate a small window.
    let candidate_limit = if at_capacity {
        u32::try_from(MAX_REGIONS).map_err(|_error| IndexError::ResourceLimited)?
    } else {
        MAX_INSPECTED_REGIONS.min(directory.region_count)
    };
    let candidate_bytes = usize::try_from(candidate_limit)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(std::mem::size_of::<MergeCandidate>())
        .ok_or(IndexError::ResourceLimited)?;
    let _candidate_memory = if candidate_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(candidate_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(
            usize::try_from(candidate_limit).map_err(|_error| IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
    load_candidates(directory, &mut candidates, candidate_limit, context)?;
    candidates.sort_unstable_by_key(|entry| (entry.rows, entry.ordinal));
    let mut merged = 0u32;
    for selected in candidates {
        if merged == MAX_MERGED_REGIONS {
            break;
        }
        if selected.rows > result.rows && !(at_capacity && merged == 0) {
            break;
        }
        if result
            .rows
            .checked_add(selected.rows)
            .is_none_or(|rows| rows > carry_rows)
        {
            break;
        }
        let seal = ann_files::load_fileset_seal(context.objects, selected.fileset, context.budget)?;
        if seal.seal.recipe != directory.recipe
            || seal.seal.generation
                != selected
                    .ordinal
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?
            || seal.seal.rows != selected.rows
            || seal.seal.dimension != recipe.dimension
            || selected.dimension
                != usize::try_from(recipe.dimension)
                    .map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(corrupt("ANN consolidation region seal differs"));
        }
        let source_counts = include(
            &mut result,
            selected.members,
            selected.rows,
            directory,
            recipe,
            context,
        )?;
        base.consider(selected.members, source_counts)?;
        maintenance::unlink(directory, selected.ordinal, selected.reference, context)?;
        merged = merged.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    install_selected_members(&mut result, base, context)?;
    Ok(result)
}
