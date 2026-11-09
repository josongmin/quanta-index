//! Exact raw-byte trigram membership delta for one source owner.
//!
//! The 24-bit masks are reusable scratch, not a persisted representation.
//! The caller must abort the enclosing attempt if a chunk callback fails;
//! no emitted prefix may be published on its own.

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};

use super::literal_grams::MAX_GRAMS;

const GRAM_BITS: usize = 1 << 24;
const WORD_BITS: usize = u64::BITS as usize;
const WORDS: usize = GRAM_BITS / WORD_BITS;
pub(super) const ENTRY_CHUNK_CAP: usize = 65_536;
pub(super) const MAX_ADMITTED_BYTES: u64 = (2 * WORDS * std::mem::size_of::<u64>()
    + WORDS * std::mem::size_of::<u32>()
    + ENTRY_CHUNK_CAP * std::mem::size_of::<MembershipEdit>())
    as u64;

/// One transient edit. The ordinal is resolved to the stable 32-byte owner key
/// by the caller before persistence; it is never a catalog identity.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MembershipEdit {
    pub(super) ordinal: u32,
    pub(super) gram: [u8; 3],
    pub(super) add: bool,
}

const _: [(); 8] = [(); std::mem::size_of::<MembershipEdit>()];

pub(super) struct OwnerMemberships {
    old: Vec<u64>,
    new: Vec<u64>,
    touched: Vec<u32>,
    entries: Vec<MembershipEdit>,
    _old_memory: Option<MemoryLease>,
    _new_memory: Option<MemoryLease>,
    _touched_memory: Option<MemoryLease>,
    _entries_memory: Option<MemoryLease>,
    poisoned: bool,
}

impl OwnerMemberships {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let mask_bytes = u64::try_from(
            WORDS
                .checked_mul(std::mem::size_of::<u64>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        let touched_bytes = u64::try_from(
            WORDS
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        let entry_bytes = u64::try_from(
            ENTRY_CHUNK_CAP
                .checked_mul(std::mem::size_of::<MembershipEdit>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        if mask_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(touched_bytes))
            .and_then(|bytes| bytes.checked_add(entry_bytes))
            != Some(MAX_ADMITTED_BYTES)
        {
            return Err(IndexError::BackendViolation(
                "literal membership backing size changed".into(),
            ));
        }
        let old_memory = budget.reserve_memory(mask_bytes, MemoryLifetime::Temporary)?;
        let new_memory = budget.reserve_memory(mask_bytes, MemoryLifetime::Temporary)?;
        let touched_memory = budget.reserve_memory(touched_bytes, MemoryLifetime::Temporary)?;
        let entries_memory = budget.reserve_memory(entry_bytes, MemoryLifetime::Temporary)?;
        let mut old = Vec::new();
        old.try_reserve_exact(WORDS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        old.resize(WORDS, 0);
        let mut new = Vec::new();
        new.try_reserve_exact(WORDS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        new.resize(WORDS, 0);
        let mut touched = Vec::new();
        touched
            .try_reserve_exact(WORDS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(ENTRY_CHUNK_CAP)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            old,
            new,
            touched,
            entries,
            _old_memory: old_memory,
            _new_memory: new_memory,
            _touched_memory: touched_memory,
            _entries_memory: entries_memory,
            poisoned: false,
        })
    }

    /// Emit each sorted chunk. The callback may consume the slice only during
    /// this call and must account for its own spill or bulk write work.
    pub(super) fn diff_owner(
        &mut self,
        ordinal: u32,
        old: Option<&[u8]>,
        new: Option<&[u8]>,
        budget: &mut WorkBudget,
        mut emit: impl FnMut(&[MembershipEdit], &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if self.poisoned || !self.touched.is_empty() || !self.entries.is_empty() {
            return Err(IndexError::BackendViolation(
                "literal membership scratch is not reusable".into(),
            ));
        }
        let result = (|| {
            self.mark_bytes(old.unwrap_or_default(), true, budget)?;
            self.mark_bytes(new.unwrap_or_default(), false, budget)?;
            self.touched.sort_unstable();
            for slot in 0..self.touched.len() {
                if slot % 4096 == 0 {
                    budget.checkpoint()?;
                }
                let word =
                    usize::try_from(*self.touched.get(slot).ok_or(IndexError::ResourceLimited)?)
                        .map_err(|_error| IndexError::ResourceLimited)?;
                let old_bits = *self.old.get(word).ok_or(IndexError::ResourceLimited)?;
                let new_bits = *self.new.get(word).ok_or(IndexError::ResourceLimited)?;
                let mut changed = old_bits ^ new_bits;
                while changed != 0 {
                    let bit = usize::try_from(changed.trailing_zeros())
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    changed &= changed.wrapping_sub(1);
                    let value = word
                        .checked_mul(WORD_BITS)
                        .and_then(|value| value.checked_add(bit))
                        .ok_or(IndexError::ResourceLimited)?;
                    let [_, first, second, third] = u32::try_from(value)
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes();
                    self.entries.push(MembershipEdit {
                        ordinal,
                        gram: [first, second, third],
                        add: new_bits & (1_u64 << bit) != 0,
                    });
                    if self.entries.len() == ENTRY_CHUNK_CAP {
                        self.flush(budget, &mut emit)?;
                    }
                }
                *self.old.get_mut(word).ok_or(IndexError::ResourceLimited)? = 0;
                *self.new.get_mut(word).ok_or(IndexError::ResourceLimited)? = 0;
            }
            self.flush(budget, &mut emit)?;
            self.touched.clear();
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn mark_bytes(
        &mut self,
        bytes: &[u8],
        is_old: bool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let windows = bytes.len().saturating_sub(2);
        budget.charge(WorkUsage {
            bytes: u64::try_from(windows)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(3)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: u64::from(windows != 0),
            ..WorkUsage::default()
        })?;
        let mut unique = 0_usize;
        for (slot, window) in bytes.windows(3).enumerate() {
            if slot % 4096 == 0 {
                budget.checkpoint()?;
            }
            let [first, second, third] = window else {
                return Err(IndexError::BackendViolation("literal gram width".into()));
            };
            let gram =
                (usize::from(*first) << 16) | (usize::from(*second) << 8) | usize::from(*third);
            let word = gram / WORD_BITS;
            let bit = 1_u64 << (gram % WORD_BITS);
            let old_word = self.old.get_mut(word).ok_or(IndexError::ResourceLimited)?;
            let new_word = self.new.get_mut(word).ok_or(IndexError::ResourceLimited)?;
            if *old_word == 0 && *new_word == 0 {
                if self.touched.len() == WORDS {
                    return Err(IndexError::ResourceLimited);
                }
                self.touched
                    .push(u32::try_from(word).map_err(|_error| IndexError::ResourceLimited)?);
            }
            let target = if is_old { old_word } else { new_word };
            if *target & bit == 0 {
                if unique == MAX_GRAMS {
                    return Err(IndexError::ResourceLimited);
                }
                *target |= bit;
                unique += 1;
            }
        }
        Ok(())
    }

    fn flush(
        &mut self,
        budget: &mut WorkBudget,
        emit: &mut impl FnMut(&[MembershipEdit], &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if self.entries.is_empty() {
            return Ok(());
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(self.entries.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(8)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        emit(&self.entries, budget)?;
        self.entries.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use quanta_index_core::index::WorkLimits;

    use super::*;

    fn work() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 100_000_000,
                mutations: 10_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn raw_replacement_delete_and_add_are_exact_and_reusable() -> Result<(), IndexError> {
        let mut budget = work()?;
        let mut scratch = OwnerMemberships::new(&budget)?;
        let mut observed = Vec::new();
        scratch.diff_owner(
            7,
            Some(b"ababa"),
            Some(b"abcca"),
            &mut budget,
            |chunk, _budget| {
                observed.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        let expected = [
            (b"aba", false),
            (b"abc", true),
            (b"bab", false),
            (b"bcc", true),
            (b"cca", true),
        ];
        if observed.len() != expected.len()
            || observed.iter().zip(expected).any(|(actual, (gram, add))| {
                actual.ordinal != 7 || &actual.gram != gram || actual.add != add
            })
        {
            return Err(IndexError::BackendViolation(
                "raw replacement membership mismatch".into(),
            ));
        }
        observed.clear();
        scratch.diff_owner(8, Some(b"abcca"), None, &mut budget, |chunk, _budget| {
            observed.extend_from_slice(chunk);
            Ok(())
        })?;
        if observed
            .iter()
            .map(|edit| (edit.gram, edit.add))
            .collect::<Vec<_>>()
            != vec![(*b"abc", false), (*b"bcc", false), (*b"cca", false)]
        {
            return Err(IndexError::BackendViolation(
                "raw delete membership mismatch".into(),
            ));
        }
        observed.clear();
        scratch.diff_owner(
            9,
            None,
            Some(&[0, 255, 0]),
            &mut budget,
            |chunk, _budget| {
                observed.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        if observed
            != [MembershipEdit {
                ordinal: 9,
                gram: [0, 255, 0],
                add: true,
            }]
        {
            return Err(IndexError::BackendViolation(
                "raw binary membership mismatch".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn failed_emit_poisons_scratch() -> Result<(), IndexError> {
        let mut budget = work()?;
        let mut scratch = OwnerMemberships::new(&budget)?;
        if scratch
            .diff_owner(1, None, Some(b"abc"), &mut budget, |_chunk, _budget| {
                Err(IndexError::BackendViolation("spill refused".into()))
            })
            .is_ok()
        {
            return Err(IndexError::BackendViolation(
                "failed emission accepted".into(),
            ));
        }
        if !matches!(
            scratch.diff_owner(2, None, Some(b"def"), &mut budget, |_chunk, _budget| Ok(())),
            Err(IndexError::BackendViolation(_))
        ) {
            return Err(IndexError::BackendViolation(
                "failed emission reused scratch".into(),
            ));
        }
        Ok(())
    }
}
