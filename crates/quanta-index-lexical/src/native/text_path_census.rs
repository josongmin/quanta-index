//! Exact census of Source paths for path leaves and path-only queries.
//!
//! The pinned Source path map lists every published repository-relative path
//! and its owners. One bounded scan decides, per path, each path leaf (a
//! `path.contains` needle under the `file.contains` NFC + case policy, or a
//! `path.regex` over the NFC path) and the query's `file:` filters. No Source body is read. The owners collected for
//! a leaf are exactly the owners whose path satisfies it, so their count is
//! the leaf's structural candidate bound.

use quanta_index_core::index::{
    IndexError, MapPageEntry, MemoryLease, MemoryLifetime, OwnerKey, SourcePathOwners, WorkBudget,
    visit_repository_path_sources,
};

use super::text::{CaseMode, TextReadContext};
use super::text_contains::file_contains_mask;
use super::text_regex::{NativeRegexProgram, PathLeafRegex};

const PATH_KEY_PREFIX: &[u8] = b"path\0";
const PATH_PAGE: u32 = 64;

/// Owners whose path satisfies one path leaf, sorted and deduplicated.
#[derive(Default)]
pub(super) struct PathOwners {
    owners: Vec<OwnerKey>,
    memory: Vec<MemoryLease>,
}

impl PathOwners {
    fn push(&mut self, owner: OwnerKey, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.owners.len() == self.owners.capacity() {
            let capacity = self
                .owners
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?;
            let added = capacity
                .checked_sub(self.owners.capacity())
                .and_then(|rows| rows.checked_mul(std::mem::size_of::<OwnerKey>()))
                .ok_or(IndexError::ResourceLimited)?;
            if let Some(lease) = budget.reserve_memory(
                u64::try_from(added).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )? {
                self.memory.push(lease);
            }
            let additional = capacity
                .checked_sub(self.owners.len())
                .ok_or(IndexError::ResourceLimited)?;
            self.owners
                .try_reserve_exact(additional)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if self.owners.capacity() > capacity {
                return Err(IndexError::ResourceLimited);
            }
        }
        self.owners.push(owner);
        Ok(())
    }

    fn finish(&mut self) {
        self.owners.sort_unstable();
        self.owners.dedup();
    }

    pub(super) fn len(&self) -> Result<u64, IndexError> {
        u64::try_from(self.owners.len()).map_err(|_width| IndexError::ResourceLimited)
    }

    pub(super) fn contains(&self, owner: OwnerKey) -> bool {
        self.owners.binary_search(&owner).is_ok()
    }

    pub(super) fn owners(&self) -> &[OwnerKey] {
        &self.owners
    }
}

/// One path leaf of the query, in leaf order.
#[derive(Clone, Copy)]
pub(super) enum PathLeaf<'a> {
    /// `path.contains`: a substring under the `file.contains` policy.
    Contains(&'a str),
    /// `path.regex`: a match anywhere in the NFC path.
    Regex(&'a PathLeafRegex),
}

/// Which path leaves `path` satisfies, one bit per leaf ordinal.
pub(super) fn path_leaf_mask(
    path: &str,
    leaves: &[PathLeaf<'_>],
    case: CaseMode,
    budget: &mut WorkBudget,
) -> Result<u8, IndexError> {
    let bit = |ordinal: usize| -> Result<u8, IndexError> {
        1_u8.checked_shl(u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)
    };
    let mut needles = [""; 8];
    let mut slots = [0_usize; 8];
    let mut literals = 0_usize;
    let mut mask = 0_u8;
    for (ordinal, leaf) in leaves.iter().enumerate() {
        match leaf {
            PathLeaf::Contains(needle) => {
                *needles
                    .get_mut(literals)
                    .ok_or(IndexError::ResourceLimited)? = needle;
                *slots.get_mut(literals).ok_or(IndexError::ResourceLimited)? = ordinal;
                literals = literals.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            PathLeaf::Regex(regex) => {
                if regex.is_match(path, budget)? {
                    mask |= bit(ordinal)?;
                }
            }
        }
    }
    if literals != 0 {
        let needles = needles.get(..literals).ok_or(IndexError::ResourceLimited)?;
        let found = file_contains_mask(path, needles, case, budget)?;
        for (index, ordinal) in slots.iter().take(literals).enumerate() {
            if found & bit(index)? != 0 {
                mask |= bit(*ordinal)?;
            }
        }
    }
    Ok(mask)
}

/// One owner set per path leaf, plus the owners passing every `file:` filter.
///
/// The filtered set exists when `filters` is given (a path-only query). A
/// leaf whose owners exceed the census cap has no set (`None`): it is decided
/// per candidate.
pub(super) struct PathCensus {
    pub(super) needles: Vec<Option<PathOwners>>,
    pub(super) filtered: Option<PathOwners>,
}

/// Leaves in `skip` get no set. With `cap`, a leaf stops collecting once it
/// holds more than `cap` owners. The scan stops once no leaf collects and
/// `filters` needs no census.
pub(super) fn path_census(
    leaves: &[PathLeaf<'_>],
    filters: Option<&NativeRegexProgram>,
    case: CaseMode,
    cap: Option<usize>,
    skip: u8,
    context: &mut TextReadContext<'_>,
) -> Result<PathCensus, IndexError> {
    let skipped = |ordinal: usize| {
        u32::try_from(ordinal)
            .is_ok_and(|shift| skip.checked_shr(shift).is_some_and(|rest| rest & 1 != 0))
    };
    let mut census = PathCensus {
        needles: (0..leaves.len())
            .map(|ordinal| (!skipped(ordinal)).then(PathOwners::default))
            .collect(),
        filtered: filters.map(|_program| PathOwners::default()),
    };
    let root = context.ticket.view.source.paths;
    let owners_root = context.ticket.view.source.owners;
    let mut after: Option<MapPageEntry> = None;
    let mut matched: Vec<OwnerKey> = Vec::new();
    loop {
        if filters.is_none() && census.needles.iter().all(Option::is_none) {
            break;
        }
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            root,
            after.as_ref().map(MapPageEntry::key).transpose()?,
            PATH_PAGE,
            context.budget,
        )?;
        let has_more = page.has_more;
        let mut last = None;
        for entry in page.cursor() {
            context.budget.checkpoint()?;
            let (key, value) = entry.get()?;
            let Some(raw) = key.strip_prefix(PATH_KEY_PREFIX) else {
                last = Some(entry);
                continue;
            };
            let path = std::str::from_utf8(raw).map_err(|_error| {
                IndexError::CorruptObject("source path key is not UTF-8".into())
            })?;
            let mask = if leaves.is_empty() {
                0
            } else {
                path_leaf_mask(path, leaves, case, context.budget)?
            };
            let filtered = match filters {
                Some(program) => program.matches_path_filters(path, context.budget)?,
                None => false,
            };
            if mask != 0 || filtered {
                matched.clear();
                match SourcePathOwners::decode(*value, context.budget)? {
                    SourcePathOwners::LegacyOwner(owner) => matched.push(owner),
                    SourcePathOwners::Repositories(repositories) => {
                        visit_repository_path_sources(
                            repositories,
                            path,
                            owners_root,
                            context.maps,
                            context.objects,
                            context.budget,
                            &mut |owner, _source, _record, _budget| {
                                matched.push(owner);
                                Ok(())
                            },
                        )?;
                    }
                }
                for (ordinal, slot) in census.needles.iter_mut().enumerate() {
                    let bit = 1_u8
                        .checked_shl(
                            u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                    if mask & bit != 0
                        && let Some(set) = slot.as_mut()
                    {
                        for owner in &matched {
                            set.push(*owner, context.budget)?;
                        }
                        if cap.is_some_and(|cap| set.owners().len() > cap) {
                            *slot = None;
                        }
                    }
                }
                if filtered && let Some(set) = census.filtered.as_mut() {
                    for owner in &matched {
                        set.push(*owner, context.budget)?;
                    }
                }
            }
            last = Some(entry);
        }
        if !has_more {
            break;
        }
        let Some(last) = last else {
            return Err(IndexError::BackendViolation(
                "nonprogressing source path map cursor".into(),
            ));
        };
        after = Some(last);
    }
    for set in census
        .needles
        .iter_mut()
        .flatten()
        .chain(census.filtered.as_mut())
    {
        set.finish();
    }
    Ok(census)
}
