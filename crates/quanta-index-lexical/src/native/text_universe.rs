//! A finite, ticket-bound admitted-owner universe for unanchored Boolean text.

use std::borrow::Cow;

use super::text::{
    Root, SegmentOwnerCursor, SelectedTextRoot, TextReadContext, TextRecipe, TextStatistics,
    ValidatedUniverseOwner, selected_universe_owner, validate_universe_owner,
};
use super::text_filter::OwnerSelection;
use quanta_index_core::index::{
    IndexError, MapPageCursor, MapPageEntry, MapRoot, MemoryLease, MemoryLifetime, OwnerKey,
    owner_map_key,
};

const OWNER_PAGE: u32 = 64;
pub(super) const MAX_UNANCHORED_DOCS: u64 = 4_096;

/// A path marker certifies an exhaustive and ordered set of source owners.
/// A selected census only visits those owners in the indexed member. Posting
/// cursors must still drain separately to authenticate their own declared DF.
pub(super) enum CandidateUniverse<'a> {
    Full(TextOwnerCursor),
    Selected {
        root: SelectedTextRoot,
        recipe: TextRecipe,
        selection: &'a OwnerSelection,
        next: usize,
        len: usize,
    },
    /// The sorted union of Literal-anchored `file.contains` candidates. Each
    /// is bound to its pinned Source record before it joins the evaluator.
    Anchored {
        root: SelectedTextRoot,
        recipe: TextRecipe,
        owners: Cow<'a, [OwnerKey]>,
        next: usize,
        _memory: Option<MemoryLease>,
    },
}

impl<'a> CandidateUniverse<'a> {
    pub(super) fn new(
        root: SelectedTextRoot,
        recipe: TextRecipe,
        selection: &'a OwnerSelection,
        selected_predicate: bool,
    ) -> Result<Self, IndexError> {
        if selected_predicate {
            if let Some(len) = selection.selected_len() {
                if u64::try_from(len).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_UNANCHORED_DOCS
                {
                    return Err(IndexError::ResourceLimited);
                }
                return Ok(Self::Selected {
                    root,
                    recipe,
                    selection,
                    next: 0,
                    len,
                });
            }
        }
        Ok(Self::Full(TextOwnerCursor::new(root, recipe)?))
    }

    pub(super) fn anchored(
        root: SelectedTextRoot,
        recipe: TextRecipe,
        sets: impl Iterator<Item = &'a [OwnerKey]> + Clone,
        budget: &quanta_index_core::index::WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let mut nonempty = sets.clone().filter(|owners| !owners.is_empty());
        let first = nonempty.next().unwrap_or_default();
        if nonempty.next().is_none() && first.is_sorted_by(|left, right| left < right) {
            return Ok(Self::Anchored {
                root,
                recipe,
                owners: Cow::Borrowed(first),
                next: 0,
                _memory: None,
            });
        }
        let total = sets.clone().try_fold(0usize, |count, set| {
            count
                .checked_add(set.len())
                .ok_or(IndexError::ResourceLimited)
        })?;
        let bytes = total
            .checked_mul(std::mem::size_of::<OwnerKey>())
            .ok_or(IndexError::ResourceLimited)?;
        // A bound memory scope refuses zero-byte reservations.
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut owners: Vec<OwnerKey> = Vec::new();
        owners
            .try_reserve_exact(total)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if owners.capacity() > total {
            return Err(IndexError::ResourceLimited);
        }
        for set in sets {
            owners.extend_from_slice(set);
        }
        owners.sort_unstable();
        owners.dedup();
        Ok(Self::Anchored {
            root,
            recipe,
            owners: Cow::Owned(owners),
            next: 0,
            _memory: memory,
        })
    }

    pub(super) fn is_full(&self) -> bool {
        matches!(self, Self::Full(_))
    }

    pub(super) fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedUniverseOwner>, IndexError> {
        match self {
            Self::Full(cursor) => cursor.next(context),
            Self::Selected {
                root,
                recipe,
                selection,
                next,
                len,
            } => {
                while *next < *len {
                    context.budget.checkpoint()?;
                    let (owner, source) = selection.selected_at(*next).ok_or_else(|| {
                        IndexError::BackendViolation("selected path census lost an owner".into())
                    })?;
                    *next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    if let Some(row) =
                        selected_universe_owner(*root, *recipe, owner, source, context)?
                    {
                        return Ok(Some(row));
                    }
                }
                Ok(None)
            }
            Self::Anchored {
                root,
                recipe,
                owners,
                next,
                ..
            } => {
                while let Some(owner) = owners.get(*next).copied() {
                    context.budget.checkpoint()?;
                    *next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    let source = context
                        .maps
                        .lookup_object(
                            context.ticket.view.source.owners,
                            &owner_map_key(owner),
                            context.budget,
                        )?
                        .ok_or_else(|| {
                            IndexError::CorruptObject(
                                "literal-anchored owner is absent from the pinned source cut"
                                    .into(),
                            )
                        })?;
                    if let Some(row) =
                        selected_universe_owner(*root, *recipe, owner, source, context)?
                    {
                        return Ok(Some(row));
                    }
                }
                Ok(None)
            }
        }
    }
}

/// The pinned member defines the admitted universe. The source owner map is
/// broader: it also contains excluded documents and cannot be a census.
pub(super) struct TextOwnerCursor {
    source: OwnerSource,
    declared: u64,
    visited: u64,
    declared_tokens: u64,
    visited_tokens: u64,
}

enum OwnerSource {
    Tree(TreeOwnerCursor),
    Segment(SegmentOwnerCursor),
}

struct TreeOwnerCursor {
    root: Root,
    recipe: TextRecipe,
    owners: MapRoot,
    after: Option<MapPageEntry>,
    entries: MapPageCursor,
    more: bool,
}

impl TextOwnerCursor {
    /// The census rule: an unbounded program reads the whole admitted
    /// universe, so its declared docs must fit `MAX_UNANCHORED_DOCS`.
    pub(super) fn check_statistics(statistics: TextStatistics) -> Result<(), IndexError> {
        if statistics.docs > MAX_UNANCHORED_DOCS {
            return Err(IndexError::ResourceLimited);
        }
        Ok(())
    }

    /// The structural rule: a program bounded by its positive posting
    /// anchors verifies at most `bound` candidates, which must fit
    /// `MAX_UNANCHORED_DOCS` before any source read.
    pub(super) const fn check_candidate_bound(bound: u64) -> Result<(), IndexError> {
        if bound > MAX_UNANCHORED_DOCS {
            return Err(IndexError::ResourceLimited);
        }
        Ok(())
    }

    pub(super) fn new(root: SelectedTextRoot, recipe: TextRecipe) -> Result<Self, IndexError> {
        let statistics = root.statistics();
        Self::check_statistics(statistics)?;
        let source = match root {
            SelectedTextRoot::Tree(root) | SelectedTextRoot::PreviousTree(root) => {
                let (owners, _, _) = root.owner_universe();
                OwnerSource::Tree(TreeOwnerCursor {
                    root,
                    recipe,
                    owners,
                    after: None,
                    entries: MapPageCursor::default(),
                    more: true,
                })
            }
            SelectedTextRoot::Segment { artifact, .. } => {
                OwnerSource::Segment(SegmentOwnerCursor::new(artifact, recipe))
            }
        };
        Ok(Self {
            source,
            declared: statistics.docs,
            visited: 0,
            declared_tokens: statistics.indexed_tokens,
            visited_tokens: 0,
        })
    }

    pub(super) fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedUniverseOwner>, IndexError> {
        context.budget.checkpoint()?;
        let declared_full = self.visited >= self.declared;
        let owner = match &mut self.source {
            OwnerSource::Tree(cursor) => cursor.next(context, declared_full)?,
            OwnerSource::Segment(cursor) => cursor.next(context)?,
        };
        let Some(owner) = owner else {
            if self.visited != self.declared || self.visited_tokens != self.declared_tokens {
                return Err(IndexError::CorruptObject(
                    "native text owner universe differs from declared statistics".into(),
                ));
            }
            return Ok(None);
        };
        self.visited = self
            .visited
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.visited > self.declared {
            return Err(IndexError::CorruptObject(
                "native text owner universe exceeds declared docs".into(),
            ));
        }
        self.visited_tokens = self
            .visited_tokens
            .checked_add(owner.doc_indexed_tokens)
            .ok_or(IndexError::ResourceLimited)?;
        if self.visited_tokens > self.declared_tokens {
            return Err(IndexError::CorruptObject(
                "native text owner universe exceeds indexed token total".into(),
            ));
        }
        Ok(Some(owner))
    }
}

impl TreeOwnerCursor {
    fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
        declared_full: bool,
    ) -> Result<Option<ValidatedUniverseOwner>, IndexError> {
        loop {
            context.budget.checkpoint()?;
            if let Some(entry) = self.entries.next() {
                let (key, contribution) = entry.get()?;
                if key.len() != 32
                    || self
                        .after
                        .as_ref()
                        .map(MapPageEntry::key)
                        .transpose()?
                        .is_some_and(|prior| key.as_slice() <= prior)
                {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe has invalid order or key".into(),
                    ));
                }
                if declared_full {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe exceeds declared docs".into(),
                    ));
                }
                let owner =
                    validate_universe_owner(key, *contribution, self.root, self.recipe, context)?;
                self.after = Some(entry);
                return Ok(Some(owner));
            }
            if !self.more {
                return Ok(None);
            }
            let page = context.maps.scan(
                self.owners,
                self.after.as_ref().map(MapPageEntry::key).transpose()?,
                OWNER_PAGE,
                context.budget,
            )?;
            if page.entries.len()
                > usize::try_from(OWNER_PAGE).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "native text owner universe page exceeded its bound or did not advance".into(),
                ));
            }
            let mut prior = self.after.as_ref().map(MapPageEntry::key).transpose()?;
            for (key, _) in &page.entries {
                if key.len() != 32 || prior.is_some_and(|previous| key.as_slice() <= previous) {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe page is not strictly ordered".into(),
                    ));
                }
                prior = Some(key);
            }
            self.more = page.has_more;
            self.entries = page.cursor();
        }
    }
}

#[cfg(test)]
mod funding_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        Digest, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, ProducerId,
        StableOwnerId, WorkBudget, WorkLimits,
    };

    use super::*;

    struct ExactMemory(AtomicU64);

    impl MemoryLeaseOwner for ExactMemory {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::Relaxed);
        }
    }

    struct ExactScope(Arc<ExactMemory>);

    impl MemoryScopePort for ExactScope {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let live = self.0.0.load(Ordering::Relaxed);
            if live.checked_add(bytes).is_none_or(|next| next > 4 * 32) {
                return Err(IndexError::ResourceLimited);
            }
            self.0.0.store(live + bytes, Ordering::Relaxed);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    #[test]
    fn anchored_union_uses_one_exact_grant_for_multiple_sets() -> Result<(), IndexError> {
        let memory = Arc::new(ExactMemory(AtomicU64::new(0)));
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(ExactScope(Arc::clone(
            &memory,
        ))))));
        let owner = |id| OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([id; 16]),
        };
        let first = [owner(1), owner(2)];
        let second = [owner(2), owner(3)];
        let universe = CandidateUniverse::anchored(
            SelectedTextRoot::Tree(Root::default()),
            TextRecipe::new(Digest([0; 32])),
            [first.as_slice(), second.as_slice()].into_iter(),
            &budget,
        )?;
        let CandidateUniverse::Anchored { owners, .. } = &universe else {
            return Err(IndexError::BackendViolation(
                "anchored union changed variant".into(),
            ));
        };
        assert_eq!(owners.as_ref(), &[owner(1), owner(2), owner(3)]);
        assert_eq!(memory.0.load(Ordering::Relaxed), 4 * 32);
        drop(universe);
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        let universe = CandidateUniverse::anchored(
            SelectedTextRoot::Tree(Root::default()),
            TextRecipe::new(Digest([0; 32])),
            [first.as_slice(), &[]].into_iter(),
            &budget,
        )?;
        let CandidateUniverse::Anchored {
            owners: Cow::Borrowed(owners),
            ..
        } = universe
        else {
            return Err(IndexError::BackendViolation(
                "single anchor did not reuse its funded owner slice".into(),
            ));
        };
        assert_eq!(owners, first.as_slice());
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::text::TextStatistics;
    use super::super::text_query::{Clause, candidate_bound};
    use super::{MAX_UNANCHORED_DOCS, TextOwnerCursor};
    use quanta_index_core::index::IndexError;

    /// Admit one postfix program under the structural or the census rule.
    ///
    /// A finite structural candidate bound is checked against
    /// `MAX_UNANCHORED_DOCS`, and an unbounded program needs the full
    /// admitted census, checked through its declared statistics. Terms
    /// `t0..t2` have DFs `dfs`; phrase slots remap 0->0, 1->2, 2->1.
    fn admit(program: &[Clause], dfs: [u64; 3], docs: u64) -> Result<(), IndexError> {
        let bound = candidate_bound(
            program,
            &[0, 2, 1],
            |index| dfs.get(index).copied().ok_or(IndexError::ResourceLimited),
            |_| Ok(None),
            |_| Ok(None),
        )?;
        bound.map_or_else(
            || {
                TextOwnerCursor::check_statistics(TextStatistics {
                    docs,
                    indexed_tokens: 0,
                })
            },
            TextOwnerCursor::check_candidate_bound,
        )
    }

    /// `MAX_UNANCHORED_DOCS` admits 4,096 candidates and refuses 4,097.
    ///
    /// This holds for every shape of the bound algebra: a term is bounded by
    /// its DF, a phrase by its rarest term, AND by its smallest finite child
    /// (NOT and regex children only filter), OR by the sum of its children;
    /// NOT or a regex alone, or OR with a regex child, is unbounded and needs
    /// the census.
    #[test]
    fn structural_rule_admits_the_bound_and_refuses_one_more() -> Result<(), IndexError> {
        if MAX_UNANCHORED_DOCS != 4_096 {
            return Err(IndexError::BackendViolation(format!(
                "the structural bound moved to {MAX_UNANCHORED_DOCS}"
            )));
        }
        let phrase = Clause::Phrase {
            first: 0,
            count: 2,
            cap: 1,
        };
        for limit in [4_096_u64, 4_097] {
            let admitted = limit <= MAX_UNANCHORED_DOCS;
            let below = limit.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
            let above = limit.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            // Finite shapes: the census is unbounded, so only the structural
            // bound can admit them.
            let finite: [(&str, Vec<Clause>, [u64; 3]); 6] = [
                ("term", vec![Clause::Term(0)], [limit, 0, 0]),
                ("phrase", vec![phrase], [above, 0, limit]),
                (
                    "and",
                    vec![Clause::Term(0), Clause::Term(1), Clause::All(2)],
                    [limit, above, 0],
                ),
                (
                    "and not",
                    vec![
                        Clause::Term(0),
                        Clause::Term(1),
                        Clause::Not,
                        Clause::All(2),
                    ],
                    [limit, 1, 0],
                ),
                (
                    "and regex",
                    vec![Clause::Term(0), Clause::Regex(0), Clause::All(2)],
                    [limit, 0, 0],
                ),
                (
                    "or",
                    vec![Clause::Term(0), Clause::Term(1), Clause::Any(2)],
                    [below, 1, 0],
                ),
            ];
            // Unbounded shapes: every DF is huge, so only the census decides.
            let unbounded: [(&str, Vec<Clause>, [u64; 3]); 3] = [
                ("not", vec![Clause::Term(1), Clause::Not], [0, 1, 0]),
                ("regex", vec![Clause::Regex(0)], [0, 0, 0]),
                (
                    "or regex",
                    vec![Clause::Term(0), Clause::Regex(0), Clause::Any(2)],
                    [1, 0, 0],
                ),
            ];
            let cases = finite
                .into_iter()
                .map(|(label, program, dfs)| (label, program, dfs, u64::MAX))
                .chain(
                    unbounded
                        .into_iter()
                        .map(|(label, program, _dfs)| (label, program, [u64::MAX; 3], limit)),
                );
            for (label, program, dfs, docs) in cases {
                let outcome = admit(&program, dfs, docs);
                let expected = if admitted {
                    outcome.is_ok()
                } else {
                    matches!(outcome, Err(IndexError::ResourceLimited))
                };
                if !expected {
                    return Err(IndexError::BackendViolation(format!(
                        "{label} at {limit}: {outcome:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}
