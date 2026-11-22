//! Bounded, recipe-local reuse of ready canonical token programs.
//!
//! The cache owns only flat programs and exact NFC source keys. Its ordered
//! slot index is fixed size; neither lookup nor eviction allocates backing.

use std::alloc::Layout;
use std::cmp::Ordering;
use std::mem::size_of;
use std::sync::Arc;

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};

use super::TextRecipe;
use super::cold_tokens::{ColdTokenProgram, ColdTokens};

const MAX_ENTRIES: usize = 64;
const LOOKUP_SLOT_BYTES: usize = size_of::<u8>() + size_of::<usize>();
const HIT_TOUCH_BYTES: usize = size_of::<u64>() + size_of::<Arc<ColdTokenProgram>>();
const LRU_SLOT_BYTES: usize = size_of::<u8>() + size_of::<u64>();

fn charge_bytes(budget: &mut WorkBudget, bytes: usize) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

fn shared_bytes<T>() -> Result<u64, IndexError> {
    u64::try_from(MemoryLease::shared_allocation_backing_layout(Layout::new::<T>())?.size())
        .map_err(|_error| IndexError::ResourceLimited)
}

fn ascii_delimiter(byte: u8) -> bool {
    byte.is_ascii() && !quanta_index_lq_text_normalizer::is_token_char(char::from(byte))
}

fn compared(
    a: &[u8],
    b: &[u8],
    known_prefix: usize,
    budget: &mut WorkBudget,
) -> Result<(Ordering, usize, bool), IndexError> {
    let left = a.get(known_prefix..).ok_or(IndexError::ResourceLimited)?;
    let right = b.get(known_prefix..).ok_or(IndexError::ResourceLimited)?;
    let mut inspected = 0usize;
    let mut matched = known_prefix;
    let mut result = Ordering::Equal;
    let mut equivalent_delimiter = false;
    for (left, right) in left.iter().zip(right) {
        inspected = inspected
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        result = left.cmp(right);
        if result != Ordering::Equal {
            equivalent_delimiter = ascii_delimiter(*left) && ascii_delimiter(*right);
            break;
        }
        matched = matched.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    charge_bytes(budget, inspected)?;
    if result == Ordering::Equal {
        Ok((a.len().cmp(&b.len()), matched, false))
    } else {
        Ok((result, matched, equivalent_delimiter))
    }
}

struct CacheEntry {
    key: Vec<u8>,
    program: Arc<ColdTokenProgram>,
    touched: u64,
    _key_memory: MemoryLease,
}

type NeighborBounds = (
    usize,
    Option<(usize, usize, bool)>,
    Option<(usize, usize, Ordering, bool)>,
);

pub(super) struct PrefixHit {
    pub(super) program: Arc<ColdTokenProgram>,
    /// Raw byte LCP only. The caller rounds this down using its current
    /// `TokenCursor` and verifies both sides of the run boundary.
    pub(super) raw_lcp_bytes: usize,
    /// The first differing bytes are both one-byte token delimiters. The
    /// caller may include the run ending exactly at the LCP, but no later run.
    pub(super) equivalent_delimiter: bool,
    /// A whole-body match, including the candidate's end. A shorter current
    /// body that prefixes a longer cached body must not reuse the whole slab.
    pub(super) exact_body: bool,
}

pub(super) struct CacheLookupPlan<'cache> {
    cache: &'cache mut TokenPrefixCache,
    pub(super) hit: Option<PrefixHit>,
    position: usize,
    existing: bool,
}

impl CacheLookupPlan<'_> {
    pub(super) fn try_admit_entry(
        &self,
        normalized: &str,
        budget: &WorkBudget,
    ) -> Result<Option<CacheEntryGrant>, IndexError> {
        self.cache.try_admit_entry(normalized, budget)
    }

    pub(super) fn insert(
        self,
        grant: CacheEntryGrant,
        program: Arc<ColdTokenProgram>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.cache
            .insert_searched(grant, self.position, self.existing, program, budget)
    }
}

/// Admit all optional backing before suffix token production.
///
/// The `Arc` grants move into their values so eviction cannot release memory
/// still used by an active owner.
pub(super) struct CacheEntryGrant {
    key: Vec<u8>,
    key_len: usize,
    key_memory: MemoryLease,
    suffix_arc_memory: Option<MemoryLease>,
    program_arc_memory: Option<MemoryLease>,
}

impl CacheEntryGrant {
    pub(super) fn copy_key(
        &mut self,
        normalized: &str,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.key_len != normalized.len() || !self.key.is_empty() {
            return Err(IndexError::BackendViolation(
                "cold cache key differs from prefill admission".into(),
            ));
        }
        if self.key.capacity() < normalized.len() {
            return Err(IndexError::BackendViolation(
                "cold cache key differs from prefill admission".into(),
            ));
        }
        charge_bytes(budget, normalized.len())?;
        self.key.extend_from_slice(normalized.as_bytes());
        Ok(())
    }

    pub(super) fn wrap_suffix(
        &mut self,
        tokens: ColdTokens,
        budget: &mut WorkBudget,
    ) -> Result<Arc<ColdTokens>, IndexError> {
        let memory = self
            .suffix_arc_memory
            .take()
            .ok_or_else(|| IndexError::BackendViolation("cold suffix Arc grant was used".into()))?;
        charge_bytes(
            budget,
            usize::try_from(shared_bytes::<ColdTokens>()?)
                .map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        tokens.into_shared_with_memory(memory)
    }

    pub(super) fn wrap_program(
        &mut self,
        program: ColdTokenProgram,
        budget: &mut WorkBudget,
    ) -> Result<Arc<ColdTokenProgram>, IndexError> {
        let memory = self.program_arc_memory.take().ok_or_else(|| {
            IndexError::BackendViolation("cold program Arc grant was used".into())
        })?;
        charge_bytes(
            budget,
            usize::try_from(shared_bytes::<ColdTokenProgram>()?)
                .map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        program.into_shared_with_memory(memory)
    }
}

/// Sorted keys plus a fixed slot order. A predecessor or successor of the
/// current key has a maximal byte LCP among all cached keys.
pub(super) struct TokenPrefixCache {
    _recipe: TextRecipe,
    slots: [Option<CacheEntry>; MAX_ENTRIES],
    order: [u8; MAX_ENTRIES],
    len: usize,
    limit: usize,
    tick: u64,
    _box_memory: MemoryLease,
}

impl TokenPrefixCache {
    pub(super) fn try_new(
        recipe: TextRecipe,
        limit: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<Box<Self>>, IndexError> {
        if limit == 0 {
            return Ok(None);
        }
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(size_of::<Self>()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        charge_bytes(budget, size_of::<Self>())?;
        Ok(Some(Box::new(Self {
            _recipe: recipe,
            slots: std::array::from_fn(|_| None),
            order: [0; MAX_ENTRIES],
            len: 0,
            limit: limit.min(MAX_ENTRIES),
            tick: 0,
            _box_memory: memory,
        })))
    }

    fn entry(&self, position: usize) -> Result<&CacheEntry, IndexError> {
        let slot = usize::from(
            *self
                .order
                .get(position)
                .ok_or(IndexError::ResourceLimited)?,
        );
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .ok_or_else(|| IndexError::BackendViolation("cold cache order is incomplete".into()))
    }

    fn lower_bound(
        &self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<NeighborBounds, IndexError> {
        let (mut low, mut predecessor, mut successor): NeighborBounds = (0, None, None);
        let mut high = self.len;
        while low < high {
            budget.checkpoint()?;
            let half = high
                .checked_sub(low)
                .and_then(|distance| distance.checked_div(2))
                .ok_or(IndexError::ResourceLimited)?;
            let mid = low.checked_add(half).ok_or(IndexError::ResourceLimited)?;
            charge_bytes(budget, LOOKUP_SLOT_BYTES)?;
            // Sorted keys between both known neighbors share every byte that
            // each neighbor shares with the query. No other prefix is proved.
            let known_prefix = predecessor
                .zip(successor)
                .map_or(0, |((_, left, _), (_, right, _, _))| left.min(right));
            let (ordering, lcp, equivalent_delimiter) =
                compared(self.entry(mid)?.key.as_slice(), key, known_prefix, budget)?;
            if ordering == Ordering::Less {
                predecessor = Some((mid, lcp, equivalent_delimiter));
                low = mid.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            } else {
                successor = Some((mid, lcp, ordering, equivalent_delimiter));
                high = mid;
            }
        }
        Ok((low, predecessor, successor))
    }

    pub(super) fn lookup_plan(
        &mut self,
        normalized: &str,
        budget: &mut WorkBudget,
    ) -> Result<CacheLookupPlan<'_>, IndexError> {
        if normalized.is_empty() {
            return Ok(CacheLookupPlan {
                cache: self,
                hit: None,
                position: 0,
                existing: false,
            });
        }
        let key = normalized.as_bytes();
        let (at, predecessor, successor) = self.lower_bound(key, budget)?;
        let mut best: Option<(usize, usize, bool)> = None;
        for candidate in [
            predecessor,
            successor.map(|(position, lcp, _ordering, delimiter)| (position, lcp, delimiter)),
        ]
        .into_iter()
        .flatten()
        {
            let (candidate, lcp, delimiter) = candidate;
            if lcp > best.map_or(0, |(_, length, _)| length) {
                best = Some((candidate, lcp, delimiter));
            }
        }
        let hit = if let Some((position, raw_lcp_bytes, equivalent_delimiter)) =
            best.filter(|(_, lcp, _)| *lcp != 0)
        {
            let slot = usize::from(
                *self
                    .order
                    .get(position)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            self.tick = self
                .tick
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            charge_bytes(budget, HIT_TOUCH_BYTES)?;
            let entry = self
                .slots
                .get_mut(slot)
                .and_then(Option::as_mut)
                .ok_or(IndexError::ResourceLimited)?;
            entry.touched = self.tick;
            Some(PrefixHit {
                program: Arc::clone(&entry.program),
                raw_lcp_bytes,
                equivalent_delimiter,
                exact_body: raw_lcp_bytes == key.len() && entry.key.len() == key.len(),
            })
        } else {
            None
        };
        Ok(CacheLookupPlan {
            cache: self,
            hit,
            position: at,
            existing: successor.is_some_and(|(_, _, ordering, _)| ordering == Ordering::Equal),
        })
    }

    #[cfg(test)]
    pub(super) fn lookup(
        &mut self,
        normalized: &str,
        budget: &mut WorkBudget,
    ) -> Result<Option<PrefixHit>, IndexError> {
        Ok(self.lookup_plan(normalized, budget)?.hit)
    }

    pub(super) fn try_admit_entry(
        &self,
        normalized: &str,
        budget: &WorkBudget,
    ) -> Result<Option<CacheEntryGrant>, IndexError> {
        if self.limit == 0 || normalized.is_empty() {
            return Ok(None);
        }
        let Some(key_memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(normalized.len()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut key = Vec::new();
        if key.try_reserve_exact(normalized.len()).is_err()
            || key.capacity()
                > usize::try_from(key_memory.bytes())
                    .map_err(|_error| IndexError::ResourceLimited)?
        {
            return Ok(None);
        }
        let Some(suffix_arc_memory) = budget.try_reserve_memory_for_optimization(
            shared_bytes::<ColdTokens>()?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let Some(program_arc_memory) = budget.try_reserve_memory_for_optimization(
            shared_bytes::<ColdTokenProgram>()?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(CacheEntryGrant {
            key,
            key_len: normalized.len(),
            key_memory,
            suffix_arc_memory: Some(suffix_arc_memory),
            program_arc_memory: Some(program_arc_memory),
        }))
    }

    fn evict_lru(&mut self, budget: &mut WorkBudget) -> Result<usize, IndexError> {
        let mut victim = None;
        for position in 0..self.len {
            budget.checkpoint()?;
            charge_bytes(budget, LRU_SLOT_BYTES)?;
            let touched = self.entry(position)?.touched;
            if victim.is_none_or(|(_, oldest)| touched < oldest) {
                victim = Some((position, touched));
            }
        }
        let (position, _) = victim.ok_or(IndexError::ResourceLimited)?;
        let slot = usize::from(
            *self
                .order
                .get(position)
                .ok_or(IndexError::ResourceLimited)?,
        );
        let end = self.len.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        charge_bytes(
            budget,
            end.checked_sub(position)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let following = position.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.order.copy_within(following..self.len, position);
        self.len = end;
        let removed = self
            .slots
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)?
            .take();
        drop(removed);
        Ok(position)
    }

    fn insert_searched(
        &mut self,
        grant: CacheEntryGrant,
        at: usize,
        existing: bool,
        program: Arc<ColdTokenProgram>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if grant.program_arc_memory.is_some() {
            return Err(IndexError::BackendViolation(
                "cold cache program Arc was not funded".into(),
            ));
        }
        budget.checkpoint()?;
        if grant.key_len != grant.key.len() || grant.key.capacity() < grant.key_len {
            return Err(IndexError::BackendViolation(
                "cold cache key differs from prefill admission".into(),
            ));
        }
        let key = grant.key;

        let mut position = at;
        if existing {
            let slot = usize::from(
                *self
                    .order
                    .get(position)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            self.tick = self
                .tick
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            charge_bytes(budget, size_of::<CacheEntry>())?;
            let replaced = self
                .slots
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?
                .replace(CacheEntry {
                    key,
                    program,
                    touched: self.tick,
                    _key_memory: grant.key_memory,
                });
            drop(replaced);
            return Ok(());
        }
        if self.len == self.limit {
            let evicted = self.evict_lru(budget)?;
            if evicted < position {
                position = position.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(IndexError::ResourceLimited)?;
        charge_bytes(
            budget,
            slot.checked_add(1).ok_or(IndexError::ResourceLimited)?,
        )?;
        charge_bytes(
            budget,
            self.len
                .checked_sub(position)
                .and_then(|count| count.checked_add(size_of::<CacheEntry>()))
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let next = position.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.order.copy_within(position..self.len, next);
        *self
            .order
            .get_mut(position)
            .ok_or(IndexError::ResourceLimited)? =
            u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
        self.tick = self
            .tick
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        *self
            .slots
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)? = Some(CacheEntry {
            key,
            program,
            touched: self.tick,
            _key_memory: grant.key_memory,
        });
        self.len = self.len.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/token_prefix_cache.rs"]
mod tests;
