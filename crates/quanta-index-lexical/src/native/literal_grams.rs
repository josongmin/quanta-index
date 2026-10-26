//! Bounded custody for byte-gram generation and unchanged-body reuse.

use std::collections::BTreeSet;

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};

pub(super) const MAX_GRAMS: usize = 262_144;
const FIRST_CAPACITY: usize = 8;
// Existing logical tree allowance; not an allocator or RSS guarantee.
const TREE_BYTES_PER_GRAM: u64 = 128;
// Capacities 2^3 through 2^18 need sixteen incremental credits, held inline.
const CREDIT_LEVELS: usize = 16;

pub(super) struct PreparedGrams {
    grams: Vec<[u8; 3]>,
    memory: Option<MemoryLease>,
    maximum: usize,
}

impl PreparedGrams {
    pub(super) fn copy(grams: &[[u8; 3]], budget: &WorkBudget) -> Result<Self, IndexError> {
        let mut prepared = Self::allocate(grams.len(), budget)?;
        prepared.grams.extend_from_slice(grams);
        budget.checkpoint()?;
        Ok(prepared)
    }

    pub(super) fn allocate(count: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if count > MAX_GRAMS {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = count.checked_mul(3).ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut grams = Vec::new();
        grams
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            grams,
            memory,
            maximum: count,
        })
    }

    /// The preflight count funds all backing before birth. Refuse growth even
    /// if the allocator happened to supply capacity beyond the requested count.
    pub(super) fn push(&mut self, gram: [u8; 3]) -> Result<(), IndexError> {
        if self.grams.len() == self.maximum || self.grams.len() == self.grams.capacity() {
            return Err(IndexError::BackendViolation(
                "literal gram materialization exceeds preflight".into(),
            ));
        }
        self.grams.push(gram);
        Ok(())
    }

    pub(super) fn into_parts(self) -> (Vec<[u8; 3]>, Option<MemoryLease>) {
        (self.grams, self.memory)
    }
}

struct GramTree {
    grams: BTreeSet<[u8; 3]>,
    credits: [Option<MemoryLease>; CREDIT_LEVELS],
    capacity: usize,
    next_credit: usize,
}

impl GramTree {
    fn new() -> Self {
        Self {
            grams: BTreeSet::new(),
            credits: std::array::from_fn(|_| None),
            capacity: 0,
            next_credit: 0,
        }
    }

    fn insert(&mut self, gram: [u8; 3], budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.grams.contains(&gram) {
            return Ok(());
        }
        if self.grams.len() == MAX_GRAMS {
            return Err(IndexError::ResourceLimited);
        }
        if self.grams.len() == self.capacity {
            let next = self
                .capacity
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .clamp(FIRST_CAPACITY, MAX_GRAMS);
            let increment = next
                .checked_sub(self.capacity)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = u64::try_from(increment)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(TREE_BYTES_PER_GRAM)
                .ok_or(IndexError::ResourceLimited)?;
            let slot = self
                .credits
                .get_mut(self.next_credit)
                .ok_or(IndexError::ResourceLimited)?;
            *slot = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            self.capacity = next;
            self.next_credit = self
                .next_credit
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: TREE_BYTES_PER_GRAM,
            ..WorkUsage::default()
        })?;
        let _inserted = self.grams.insert(gram);
        Ok(())
    }
}

pub(super) fn build(bytes: &[u8], budget: &mut WorkBudget) -> Result<PreparedGrams, IndexError> {
    budget.checkpoint()?;
    let mut tree = GramTree::new();
    for window in bytes.windows(3) {
        budget.checkpoint()?;
        let gram = window
            .try_into()
            .map_err(|_error| IndexError::BackendViolation("invalid byte window".into()))?;
        tree.insert(gram, budget)?;
    }
    let mut output = PreparedGrams::allocate(tree.grams.len(), budget)?;
    // The bounded array and tree overlap under separate credits. Destroy tree
    // nodes before their inline credits are refunded, including unwind/error.
    output.grams.extend(std::mem::take(&mut tree.grams));
    budget.checkpoint()?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct Ledger {
        cap: u64,
        live: AtomicU64,
        peak: AtomicU64,
        calls: AtomicU64,
        invalid: AtomicBool,
    }

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid.store(true, Ordering::Relaxed);
            }
        }
    }

    struct Scope(Arc<Ledger>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            require(lifetime == MemoryLifetime::Temporary)?;
            let _prior = self.0.calls.fetch_add(1, Ordering::Relaxed);
            let old = self
                .0
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes).filter(|next| *next <= self.0.cap)
                })
                .map_err(|_error| IndexError::ResourceLimited)?;
            let live = old.checked_add(bytes).ok_or(IndexError::ResourceLimited)?;
            let _peak = self.0.peak.fetch_max(live, Ordering::Relaxed);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn work(cap: u64) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 1_000_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let ledger = Arc::new(Ledger {
            cap,
            ..Ledger::default()
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(&ledger))))));
        Ok((budget, ledger))
    }

    fn require(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "literal gram custody law failed".into(),
            ))
        }
    }

    #[test]
    fn sorted_byte_gold_reuse_and_tree_array_overlap_keep_custody() -> Result<(), IndexError> {
        type GramGold<'a> = (&'a [u8], &'a [[u8; 3]], u64);
        let cases: &[GramGold<'_>] = &[
            (b"ababa", &[*b"aba", *b"bab"], 1030),
            (
                b"abcdefghijk",
                &[
                    *b"abc", *b"bcd", *b"cde", *b"def", *b"efg", *b"fgh", *b"ghi", *b"hij", *b"ijk",
                ],
                2075,
            ),
            (b"AbAb", &[*b"AbA", *b"bAb"], 1030),
            ("éA".as_bytes(), &[[0xc3, 0xa9, b'A']], 1027),
        ];
        for (bytes, gold, expected_peak) in cases {
            let (mut budget, ledger) = work(*expected_peak)?;
            let generated = build(bytes, &mut budget)?;
            require(generated.grams.as_slice() == *gold)?;
            require(ledger.peak.load(Ordering::Relaxed) == *expected_peak)?;
            let data_bytes = u64::try_from(gold.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(3)
                .ok_or(IndexError::ResourceLimited)?;
            require(ledger.live.load(Ordering::Relaxed) == data_bytes)?;
            let copied = PreparedGrams::copy(&generated.grams, &budget)?;
            require(copied.grams.as_slice() == *gold)?;
            require(
                ledger.live.load(Ordering::Relaxed)
                    == data_bytes
                        .checked_mul(2)
                        .ok_or(IndexError::ResourceLimited)?,
            )?;
            drop(copied);
            let (grams, memory) = generated.into_parts();
            require(
                grams.as_slice() == *gold && ledger.live.load(Ordering::Relaxed) == data_bytes,
            )?;
            drop(grams);
            drop(memory);
            require(
                ledger.live.load(Ordering::Relaxed) == 0 && !ledger.invalid.load(Ordering::Relaxed),
            )?;
        }
        Ok(())
    }

    #[test]
    fn tree_birth_growth_and_output_refusal_release_all_credit() -> Result<(), IndexError> {
        for (bytes, cap, expected_mutations, expected_peak) in [
            (b"abc".as_slice(), 1023, 0, 0),
            (b"abc".as_slice(), 1026, 1, 1024),
            (b"abcdefghijk".as_slice(), 2047, 8, 1024),
            (b"abcdefghijk".as_slice(), 2074, 9, 2048),
        ] {
            let (mut budget, ledger) = work(cap)?;
            require(matches!(
                build(bytes, &mut budget),
                Err(IndexError::ResourceLimited)
            ))?;
            require(budget.usage().mutations == expected_mutations)?;
            require(ledger.peak.load(Ordering::Relaxed) == expected_peak)?;
            require(
                ledger.live.load(Ordering::Relaxed) == 0 && !ledger.invalid.load(Ordering::Relaxed),
            )?;
        }
        Ok(())
    }

    #[test]
    fn copy_floor_count_limit_and_empty_original_cancellation_are_exact() -> Result<(), IndexError>
    {
        let (mut budget, ledger) = work(0)?;
        for bytes in [b"".as_slice(), b"a", b"ab"] {
            require(build(bytes, &mut budget)?.grams.is_empty())?;
        }
        require(ledger.calls.load(Ordering::Relaxed) == 0)?;
        let over_limit = vec![[0; 3]; MAX_GRAMS + 1];
        require(matches!(
            PreparedGrams::copy(&over_limit, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.calls.load(Ordering::Relaxed) == 0)?;
        for cap in [2, 3] {
            let (credit, tracking) = work(cap)?;
            let result = PreparedGrams::copy(&[*b"abc"], &credit);
            if cap == 2 {
                require(matches!(result, Err(IndexError::ResourceLimited)))?;
            } else {
                let copied = result?;
                require(copied.grams.as_slice() == [*b"abc"])?;
                drop(copied);
            }
            require(
                tracking.live.load(Ordering::Relaxed) == 0
                    && !tracking.invalid.load(Ordering::Relaxed),
            )?;
        }
        budget.request().cancel_handle().cancel();
        require(matches!(
            build(b"", &mut budget),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            PreparedGrams::copy(&[], &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.calls.load(Ordering::Relaxed) == 0)
    }
}
