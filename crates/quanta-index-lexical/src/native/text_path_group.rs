//! Exact Native `select:path` grouping before the result limit.
//! Repository and path are selected-source values with their original grants.

use quanta_index_contract::{RepoId, native_protocol::compare_native_text_path_rank_v1};
use quanta_index_core::index::{
    FiniteScore, IndexError, MAX_SOURCE_PATH_BYTES, MemoryLease, MemoryLifetime, OwnerKey,
    WorkBudget, WorkUsage,
};
#[expect(
    clippy::disallowed_types,
    reason = "ephemeral hash routing checks full repository and path equality"
)]
use std::hash::DefaultHasher;
use std::{
    cell::Cell,
    cmp::Ordering,
    hash::{Hash, Hasher},
    mem::size_of,
};

struct Group<T> {
    hash: u64,
    score: FiniteScore,
    repository: RepoId,
    path: String,
    payload: T,
    // The backing dies before its retained allocation grant.
    _source_memory: Option<MemoryLease>,
}

pub(super) struct PathGroupRow<T> {
    pub payload: T,
}

pub(super) struct PathGroupRanking<T> {
    pub matched: u64,
    pub remaining: u64,
    pub rows: Vec<PathGroupRow<T>>,
    // The row vector dies before its retained capacity grant.
    _rows_memory: Option<MemoryLease>,
}

pub(super) struct PathGroupAccumulator<T> {
    buckets: Vec<Option<Group<T>>>,
    _buckets_memory: Option<MemoryLease>,
    len: usize,
    maximum: usize,
    rank: fn(&T) -> (OwnerKey, f64),
}

fn width(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)
}

fn bytes<T>(count: usize) -> Result<u64, IndexError> {
    width(
        count
            .checked_mul(size_of::<T>())
            .ok_or(IndexError::ResourceLimited)?,
    )
}

fn allocate_buckets<T>(
    capacity: usize,
    budget: &WorkBudget,
) -> Result<(Vec<Option<Group<T>>>, Option<MemoryLease>), IndexError> {
    let memory = budget.reserve_memory(
        bytes::<Option<Group<T>>>(capacity)?,
        MemoryLifetime::Temporary,
    )?;
    let mut buckets = Vec::new();
    buckets
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if buckets.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    buckets.resize_with(capacity, || None);
    Ok((buckets, memory))
}

fn hash_key(repository: &RepoId, path: &str, budget: &mut WorkBudget) -> Result<u64, IndexError> {
    budget.charge(WorkUsage {
        bytes: width(
            repository
                .as_str()
                .len()
                .checked_add(path.len())
                .ok_or(IndexError::ResourceLimited)?,
        )?,
        ..WorkUsage::default()
    })?;
    #[expect(
        clippy::disallowed_types,
        reason = "ephemeral hash routing checks full repository and path equality"
    )]
    let mut hasher = DefaultHasher::new();
    repository.hash(&mut hasher);
    path.hash(&mut hasher);
    Ok(hasher.finish())
}

fn slot(hash: u64, capacity: usize) -> Result<usize, IndexError> {
    usize::try_from(hash % width(capacity)?).map_err(|_error| IndexError::ResourceLimited)
}

impl<T> PathGroupAccumulator<T> {
    pub(super) fn new(maximum: u64, rank: fn(&T) -> (OwnerKey, f64)) -> Result<Self, IndexError> {
        Ok(Self {
            buckets: Vec::new(),
            _buckets_memory: None,
            len: 0,
            maximum: usize::try_from(maximum).map_err(|_error| IndexError::ResourceLimited)?,
            rank,
        })
    }

    fn grow(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let capacity = self
            .buckets
            .len()
            .max(4)
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let (mut next, memory) = allocate_buckets(capacity, budget)?;
        budget.charge(WorkUsage {
            bytes: bytes::<Option<Group<T>>>(capacity)?
                .checked_add(bytes::<Option<Group<T>>>(self.len)?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        for old in &mut self.buckets {
            let Some(row) = old.take() else { continue };
            let mut index = slot(row.hash, capacity)?;
            while next
                .get(index)
                .ok_or(IndexError::ResourceLimited)?
                .is_some()
            {
                index = index.checked_add(1).ok_or(IndexError::ResourceLimited)? % capacity;
            }
            *next.get_mut(index).ok_or(IndexError::ResourceLimited)? = Some(row);
        }
        self.buckets = next;
        self._buckets_memory = memory;
        Ok(())
    }

    /// The caller has authenticated one selected Source record and transfers its
    /// existing path/repository grant. Every distinct group retains one owner.
    pub(super) fn consider(
        &mut self,
        payload: T,
        repository: RepoId,
        path: String,
        source_memory: Option<MemoryLease>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if path.is_empty()
            || path.len() > MAX_SOURCE_PATH_BYTES
            || path.starts_with('/')
            || path.contains('\0')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(IndexError::CorruptObject(
                "native text path group has invalid source path".into(),
            ));
        }
        let (owner, raw_score) = (self.rank)(&payload);
        let score = FiniteScore::new(raw_score).map_err(|_error| {
            IndexError::CorruptObject("native path group has nonfinite score".into())
        })?;
        let hash = hash_key(&repository, &path, budget)?;
        if self.buckets.is_empty() {
            if self.maximum == 0 {
                return Err(IndexError::ResourceLimited);
            }
            self.grow(budget)?;
        }
        let mut index = slot(hash, self.buckets.len())?;
        let rank = self.rank;
        loop {
            budget.checkpoint()?;
            let existing = self
                .buckets
                .get_mut(index)
                .ok_or(IndexError::ResourceLimited)?;
            match existing {
                Some(group) => {
                    budget.charge(WorkUsage {
                        bytes: width(
                            repository
                                .as_str()
                                .len()
                                .checked_add(path.len())
                                .ok_or(IndexError::ResourceLimited)?,
                        )?,
                        ..WorkUsage::default()
                    })?;
                    if group.hash == hash && group.repository == repository && group.path == path {
                        if score.get().total_cmp(&group.score.get()) == Ordering::Greater
                            || (score == group.score && owner < rank(&group.payload).0)
                        {
                            *group = Group {
                                hash,
                                score,
                                repository,
                                path,
                                payload,
                                _source_memory: source_memory,
                            };
                        }
                        return Ok(());
                    }
                }
                None => break,
            }
            index = index.checked_add(1).ok_or(IndexError::ResourceLimited)? % self.buckets.len();
        }
        if self.len == self.maximum {
            return Err(IndexError::ResourceLimited);
        }
        if self
            .len
            .checked_add(1)
            .and_then(|value| value.checked_mul(4))
            .ok_or(IndexError::ResourceLimited)?
            >= self
                .buckets
                .len()
                .checked_mul(3)
                .ok_or(IndexError::ResourceLimited)?
        {
            self.grow(budget)?;
            index = slot(hash, self.buckets.len())?;
            while self
                .buckets
                .get(index)
                .ok_or(IndexError::ResourceLimited)?
                .is_some()
            {
                index =
                    index.checked_add(1).ok_or(IndexError::ResourceLimited)? % self.buckets.len();
            }
        }
        budget.charge(WorkUsage {
            bytes: bytes::<Group<T>>(1)?,
            ..WorkUsage::default()
        })?;
        *self
            .buckets
            .get_mut(index)
            .ok_or(IndexError::ResourceLimited)? = Some(Group {
            hash,
            score,
            repository,
            path,
            payload,
            _source_memory: source_memory,
        });
        self.len = self.len.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        top_k: usize,
        after: Option<(FiniteScore, &RepoId, &str)>,
        budget: &mut WorkBudget,
    ) -> Result<PathGroupRanking<T>, IndexError> {
        budget.checkpoint()?;
        let order_bytes = bytes::<usize>(self.len)?;
        let order_memory = if order_bytes == 0 {
            None
        } else {
            budget.reserve_memory(order_bytes, MemoryLifetime::Temporary)?
        };
        let mut order = Vec::new();
        order
            .try_reserve_exact(self.len)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if order.capacity() > self.len {
            return Err(IndexError::ResourceLimited);
        }
        for (index, row) in self.buckets.iter().enumerate() {
            if row.is_some() {
                order.push(index);
            }
        }
        if order.len() != self.len {
            return Err(IndexError::BackendViolation(
                "path group table count changed".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: order_bytes,
            ..WorkUsage::default()
        })?;
        let compared = Cell::new(Some(0u64));
        order.sort_unstable_by(|&left, &right| {
            let a = self.buckets.get(left).and_then(Option::as_ref);
            let b = self.buckets.get(right).and_then(Option::as_ref);
            match (a, b) {
                (Some(a), Some(b)) => {
                    let compared_width = a
                        .repository
                        .as_str()
                        .len()
                        .checked_add(b.repository.as_str().len())
                        .and_then(|value| value.checked_add(a.path.len()))
                        .and_then(|value| value.checked_add(b.path.len()))
                        .and_then(|value| width(value).ok());
                    compared.set(
                        compared
                            .get()
                            .and_then(|used| used.checked_add(32))
                            .and_then(|used| used.checked_add(compared_width?)),
                    );
                    compare_native_text_path_rank_v1(
                        (a.score, &a.repository, &a.path),
                        (b.score, &b.repository, &b.path),
                    )
                }
                _ => Ordering::Equal,
            }
        });
        budget.charge(WorkUsage {
            bytes: compared.get().ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        let mut anchor = 0;
        if let Some((score, repository, path)) = after {
            let mut found = false;
            for (position, index) in order.iter().enumerate() {
                budget.checkpoint()?;
                let row = self
                    .buckets
                    .get(*index)
                    .and_then(Option::as_ref)
                    .ok_or(IndexError::ResourceLimited)?;
                let comparison_bytes = row
                    .repository
                    .as_str()
                    .len()
                    .checked_add(repository.as_str().len())
                    .and_then(|value| value.checked_add(row.path.len()))
                    .and_then(|value| value.checked_add(path.len()))
                    .and_then(|value| value.checked_add(size_of::<FiniteScore>()))
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes: width(comparison_bytes)?,
                    ..WorkUsage::default()
                })?;
                if compare_native_text_path_rank_v1(
                    (row.score, &row.repository, &row.path),
                    (score, repository, path),
                ) == Ordering::Equal
                {
                    anchor = position.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(IndexError::InvalidInput(
                    "native path rank-after anchor is absent from eligible groups".into(),
                ));
            }
        }
        let remaining = self
            .len
            .checked_sub(anchor)
            .ok_or(IndexError::ResourceLimited)?;
        let count = remaining.min(top_k);
        let row_bytes = bytes::<PathGroupRow<T>>(count)?;
        let rows_memory = if row_bytes == 0 {
            None
        } else {
            budget.reserve_memory(row_bytes, MemoryLifetime::Retained)?
        };
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if rows.capacity() > count {
            return Err(IndexError::ResourceLimited);
        }
        budget.charge(WorkUsage {
            bytes: row_bytes,
            ..WorkUsage::default()
        })?;
        for index in order.into_iter().skip(anchor).take(count) {
            let row = self
                .buckets
                .get_mut(index)
                .and_then(Option::take)
                .ok_or(IndexError::ResourceLimited)?;
            rows.push(PathGroupRow {
                payload: row.payload,
            });
        }
        drop(order_memory);
        Ok(PathGroupRanking {
            matched: width(self.len)?,
            remaining: width(remaining)?,
            rows,
            _rows_memory: rows_memory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::text_query::NativeTextRankedHit;
    use quanta_index_core::index::{
        MemoryLeaseOwner, MemoryScope, MemoryScopePort, NativeTextHitSource, ObjectRef, OwnerKey,
        ProducerId, StableOwnerId, WorkLimits,
    };
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    struct TestMemory;

    impl MemoryScopePort for TestMemory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            Ok(MemoryLease::new(bytes, Arc::new(Self)))
        }
    }

    impl MemoryLeaseOwner for TestMemory {
        fn release(&self, _: u64) {}
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 1_000_000,
                mutations: 0,
            },
            deadline,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(TestMemory))));
        Ok(budget)
    }

    fn hit(owner: u8, score: f64) -> NativeTextRankedHit {
        NativeTextRankedHit {
            owner: OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([owner; 16]),
            },
            source: NativeTextHitSource::Durable(ObjectRef(quanta_index_core::index::Digest(
                [owner; 32],
            ))),
            score,
        }
    }

    fn consider(
        groups: &mut PathGroupAccumulator<(NativeTextRankedHit, u8)>,
        budget: &mut WorkBudget,
        repo: &str,
        path: &str,
        owner: u8,
        score: f64,
    ) -> Result<(), IndexError> {
        groups.consider(
            (hit(owner, score), owner),
            RepoId::new(repo).map_err(|error| IndexError::InvalidInput(error.to_string()))?,
            path.to_owned(),
            None,
            budget,
        )
    }

    #[test]
    fn exact_grouping_selects_score_then_owner_before_global_cut() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut groups = PathGroupAccumulator::new(4, |row: &(NativeTextRankedHit, u8)| {
            (row.0.owner, row.0.score)
        })?;
        consider(&mut groups, &mut budget, "repo-b", "same.rs", 8, 0.7)?;
        consider(&mut groups, &mut budget, "repo-a", "a.rs", 5, 0.8)?;
        consider(&mut groups, &mut budget, "repo-b", "same.rs", 9, 0.9)?;
        consider(&mut groups, &mut budget, "repo-b", "same.rs", 3, 0.9)?;
        consider(&mut groups, &mut budget, "repo-a", "z.rs", 4, 0.8)?;
        let ranked = groups.finish(2, None, &mut budget)?;
        assert_eq!(ranked.matched, 3);
        assert_eq!(ranked.remaining, 3);
        assert_eq!(ranked.rows.len(), 2);
        let first = ranked.rows.first().ok_or(IndexError::Conflict)?;
        let second = ranked.rows.get(1).ok_or(IndexError::Conflict)?;
        assert_eq!(first.payload.0.owner.stable_id, StableOwnerId([3; 16]));
        assert_eq!(first.payload.1, 3);
        assert_eq!(second.payload.0.owner.stable_id, StableOwnerId([5; 16]));
        Ok(())
    }

    #[test]
    fn cursor_counts_all_groups_after_the_exact_anchor() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut groups = PathGroupAccumulator::new(4, |row: &(NativeTextRankedHit, u8)| {
            (row.0.owner, row.0.score)
        })?;
        consider(&mut groups, &mut budget, "repo-a", "b.rs", 1, 0.7)?;
        consider(&mut groups, &mut budget, "repo-b", "a.rs", 2, 0.7)?;
        consider(&mut groups, &mut budget, "repo-a", "a.rs", 3, 0.7)?;
        let repo =
            RepoId::new("repo-a").map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let score =
            FiniteScore::new(0.7).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let ranked = groups.finish(1, Some((score, &repo, "a.rs")), &mut budget)?;
        assert_eq!(ranked.matched, 3);
        assert_eq!(ranked.remaining, 2);
        assert_eq!(ranked.rows.len(), 1);
        let first = ranked.rows.first().ok_or(IndexError::Conflict)?;
        assert_eq!(first.payload.0.owner.stable_id, StableOwnerId([1; 16]));
        Ok(())
    }

    #[test]
    fn no_eligible_groups_return_empty_ranked_rows_with_live_memory() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let groups =
            PathGroupAccumulator::<NativeTextRankedHit>::new(4, |hit| (hit.owner, hit.score))?;
        let ranked = groups.finish(2, None, &mut budget)?;
        assert_eq!((ranked.matched, ranked.remaining), (0, 0));
        assert!(ranked.rows.is_empty());
        Ok(())
    }

    #[test]
    fn last_cursor_returns_an_empty_page_with_live_memory() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut groups = PathGroupAccumulator::new(4, |row: &(NativeTextRankedHit, u8)| {
            (row.0.owner, row.0.score)
        })?;
        consider(&mut groups, &mut budget, "repo-a", "a.rs", 1, 0.7)?;
        let repo =
            RepoId::new("repo-a").map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let score =
            FiniteScore::new(0.7).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let ranked = groups.finish(2, Some((score, &repo, "a.rs")), &mut budget)?;
        assert_eq!((ranked.matched, ranked.remaining), (1, 0));
        assert!(ranked.rows.is_empty());
        Ok(())
    }
}
