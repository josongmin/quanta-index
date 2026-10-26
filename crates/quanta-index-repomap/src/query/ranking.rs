//! One bounded `RepoMap` selector for both materialized and paged candidates.
//!
//! A scanner may be called again when token skips require a wider rank prefix.
//! It must yield the same candidate universe and source ordinals on each pass.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};

use quanta_index_contract::RepoMapDocType;

const DIAGNOSTIC_ALLOWANCE_BYTES: usize = 2048;

/// Fields certified by the candidate source. The candidate may borrow a
/// materialized row or own a funded row/handle from a paged selected view.
pub(crate) trait RankCandidate {
    fn identity(&self) -> &str;
    fn doc_type(&self) -> RepoMapDocType;
    fn owner_path(&self) -> &str;
    fn folded_search_text(&self) -> &str;
    fn final_score_millis(&self) -> u32;
    fn token_budget_hint(&self) -> u32;
    /// Stable scan position. This is only the heap tie after the rank key;
    /// final ordering deliberately retains the legacy key-only stable sort.
    fn source_ordinal(&self) -> usize;
}

/// A repeatable, fallible scan.
///
/// `reserve_prefix` grants the memory for the heap, sorted prefix, sort
/// scratch and admitted rows before allocation.
/// Its grant remains alive in the returned selection while rows are consumed.
pub(crate) trait RankScanner {
    type Candidate: RankCandidate;
    type Error;
    type Grant;

    fn resource_limited() -> Self::Error;
    fn reserve_prefix(&mut self, bytes: usize) -> Result<Self::Grant, Self::Error>;
    fn scan(
        &mut self,
        sink: &mut dyn FnMut(Self::Candidate) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error>;
}

pub(crate) struct RankPlan<'a> {
    pub focus_keys: &'a BTreeSet<(&'a str, RepoMapDocType)>,
    pub focus_owner_paths: &'a BTreeSet<&'a str>,
    pub query_terms: &'a [String],
    pub top_k_limit: usize,
    /// The first prefix may use a known exact universe cap. A scanner without
    /// one starts at `top_k_limit`; this value never asserts completeness.
    pub initial_prefix: usize,
    pub token_budget: u32,
}

pub(crate) struct RankSelection<C, G> {
    pub included: Vec<(C, u32)>,
    pub dropped_entries_count: u32,
    pub drop_reason_codes: Vec<String>,
    pub degraded_reason_codes: Vec<String>,
    /// Keeps the admitted candidates' allocation grant alive through emission.
    pub(crate) grant: G,
}

struct Ranked<C> {
    candidate: C,
    exact_focus: bool,
    owner_path_focus: bool,
    query_match_score: u32,
    final_score_millis: u32,
}

impl<C: RankCandidate> Ranked<C> {
    fn cmp_rank(&self, other: &Self) -> Ordering {
        self.exact_focus
            .cmp(&other.exact_focus)
            .then(self.owner_path_focus.cmp(&other.owner_path_focus))
            .then(self.query_match_score.cmp(&other.query_match_score))
            .then(self.final_score_millis.cmp(&other.final_score_millis))
            .then(other.candidate.identity().cmp(self.candidate.identity()))
    }
}

impl<C: RankCandidate> PartialEq for Ranked<C> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<C: RankCandidate> Eq for Ranked<C> {}

impl<C: RankCandidate> PartialOrd for Ranked<C> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<C: RankCandidate> Ord for Ranked<C> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_rank(other).then(
            self.candidate
                .source_ordinal()
                .cmp(&other.candidate.source_ordinal()),
        )
    }
}

struct RankedPrefix<C, G> {
    entries: Vec<Ranked<C>>,
    universe: usize,
    any_query_match: bool,
    minimum_token_hint: u32,
    grant: G,
}

struct InclusionWalk<C> {
    included: Vec<(C, u32)>,
    walked: usize,
    budget_skips: bool,
    forced_budget_floor: bool,
    remaining_tokens: u32,
}

pub(crate) fn select<S: RankScanner>(
    scanner: &mut S,
    plan: &RankPlan<'_>,
) -> Result<RankSelection<S::Candidate, S::Grant>, S::Error> {
    let mut prefix = plan.initial_prefix.max(1).min(plan.top_k_limit.max(1));
    loop {
        let ranked = rank_prefix(scanner, plan, prefix)?;
        let exhausted_prefix = ranked.entries.len() >= ranked.universe;
        let walk = walk_inclusion::<S>(ranked.entries, plan.top_k_limit, plan.token_budget)?;
        if walk.included.len() >= plan.top_k_limit
            || exhausted_prefix
            || (prefix >= plan.top_k_limit && !walk.budget_skips)
            // The scan covered the entire candidate universe, including
            // rows outside the retained prefix. No later row can fit below
            // its minimum normalized cost. Require an observed skip so the
            // existing drop diagnostics remain unchanged for short prefixes.
            || (walk.budget_skips && walk.remaining_tokens < ranked.minimum_token_hint)
        {
            let mut degraded = BTreeSet::new();
            if !plan.query_terms.is_empty() && !ranked.any_query_match {
                let _inserted = degraded.insert("query_terms_unmatched".to_string());
            }
            if walk.forced_budget_floor {
                let _inserted = degraded.insert("token_budget_floor_applied".to_string());
            }
            let mut drops = BTreeSet::new();
            if walk.budget_skips {
                let _inserted = drops.insert("token_budget_exhausted".to_string());
            }
            if walk.included.len() >= plan.top_k_limit
                && ranked.universe.saturating_sub(walk.walked) > 0
            {
                let _inserted = drops.insert("top_k_exhausted".to_string());
            }
            return Ok(RankSelection {
                dropped_entries_count: saturating_u32_from_usize(
                    ranked.universe.saturating_sub(walk.included.len()),
                ),
                included: walk.included,
                drop_reason_codes: drops.into_iter().collect(),
                degraded_reason_codes: degraded.into_iter().collect(),
                grant: ranked.grant,
            });
        }
        // At this point there are candidates beyond the retained prefix. A
        // skipped row may be followed by a smaller row that fits the budget.
        prefix = prefix.saturating_mul(2).min(ranked.universe);
    }
}

fn rank_prefix<S: RankScanner>(
    scanner: &mut S,
    plan: &RankPlan<'_>,
    prefix: usize,
) -> Result<RankedPrefix<S::Candidate, S::Grant>, S::Error> {
    let slots = prefix.checked_add(1).ok_or_else(S::resource_limited)?;
    let row_bytes =
        std::mem::size_of::<Ranked<S::Candidate>>().max(std::mem::size_of::<(S::Candidate, u32)>());
    let bytes = slots
        .checked_mul(row_bytes)
        .and_then(|value| value.checked_mul(4))
        // Four fixed reason codes and their set/vector nodes still allocate
        // when the candidate type is tiny or has zero size.
        .and_then(|value| value.checked_add(DIAGNOSTIC_ALLOWANCE_BYTES))
        .ok_or_else(S::resource_limited)?;
    let grant = scanner.reserve_prefix(bytes)?;
    let mut retained = BinaryHeap::<Reverse<Ranked<S::Candidate>>>::new();
    retained
        .try_reserve(slots)
        .map_err(|_error| S::resource_limited())?;
    let mut any_query_match = false;
    let mut universe = 0_usize;
    let mut minimum_token_hint = u32::MAX;
    scanner.scan(&mut |candidate| {
        universe = universe.checked_add(1).ok_or_else(S::resource_limited)?;
        minimum_token_hint = minimum_token_hint.min(candidate.token_budget_hint().max(1));
        let score = query_match_score(candidate.folded_search_text(), plan.query_terms);
        any_query_match |= score > 0;
        let ranked = Ranked {
            exact_focus: plan
                .focus_keys
                .contains(&(candidate.identity(), candidate.doc_type())),
            owner_path_focus: plan.focus_owner_paths.contains(candidate.owner_path()),
            query_match_score: score,
            final_score_millis: candidate.final_score_millis(),
            candidate,
        };
        if retained.len() < prefix {
            retained.push(Reverse(ranked));
        } else if retained
            .peek()
            .is_some_and(|Reverse(weakest)| ranked.cmp_rank(weakest) == Ordering::Greater)
        {
            let _evicted = retained.pop();
            retained.push(Reverse(ranked));
        }
        Ok(())
    })?;
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(retained.len())
        .map_err(|_error| S::resource_limited())?;
    ordered.extend(retained.into_iter().map(|Reverse(candidate)| candidate));
    ordered.sort_by(|left, right| right.cmp_rank(left));
    Ok(RankedPrefix {
        entries: ordered,
        universe,
        any_query_match,
        minimum_token_hint,
        grant,
    })
}

fn walk_inclusion<S: RankScanner>(
    ordered: Vec<Ranked<S::Candidate>>,
    top_k_limit: usize,
    token_budget: u32,
) -> Result<InclusionWalk<S::Candidate>, S::Error> {
    let mut included = Vec::new();
    included
        .try_reserve_exact(top_k_limit.min(ordered.len()))
        .map_err(|_error| S::resource_limited())?;
    let mut consumed_tokens = 0_u32;
    let mut budget_skips = false;
    let mut forced_budget_floor = false;
    let mut walked = 0_usize;
    for ranked in ordered {
        if included.len() >= top_k_limit {
            break;
        }
        walked = walked.saturating_add(1);
        let token_hint = ranked.candidate.token_budget_hint().max(1);
        let within_budget = consumed_tokens.saturating_add(token_hint) <= token_budget;
        let apply_budget_floor = included.is_empty() && !within_budget;
        if within_budget || apply_budget_floor {
            forced_budget_floor |= apply_budget_floor;
            consumed_tokens = consumed_tokens.saturating_add(token_hint);
            included.push((
                ranked.candidate,
                saturating_u32_from_usize(included.len().saturating_add(1)),
            ));
        } else {
            budget_skips = true;
        }
    }
    Ok(InclusionWalk {
        included,
        walked,
        budget_skips,
        forced_budget_floor,
        remaining_tokens: token_budget.saturating_sub(consumed_tokens),
    })
}

pub(crate) fn tokenize(query_text: &str) -> Vec<String> {
    quanta_index_lq_text_normalizer::tokenize(
        query_text,
        quanta_index_lq_text_normalizer::CaseMode::Folded,
    )
    .indexable()
    .map(|token| token.text.clone())
    .collect()
}

fn query_match_score(folded_search_text: &str, query_terms: &[String]) -> u32 {
    query_terms
        .iter()
        .filter(|term| folded_search_text.contains(term.as_str()))
        .count()
        .try_into()
        .map_or(u32::MAX, |count| count)
}

fn saturating_u32_from_usize(value: usize) -> u32 {
    u32::try_from(value).map_or(u32::MAX, std::convert::identity)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use quanta_index_contract::RepoMapDocType;

    use super::{RankCandidate, RankPlan, RankScanner, select};

    struct Candidate {
        identity: &'static str,
        ordinal: usize,
        token_hint: u32,
        drops: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Drop for Candidate {
        fn drop(&mut self) {
            self.drops.borrow_mut().push("candidate");
        }
    }

    impl RankCandidate for Candidate {
        fn identity(&self) -> &str {
            self.identity
        }

        fn doc_type(&self) -> RepoMapDocType {
            RepoMapDocType::File
        }

        fn owner_path(&self) -> &'static str {
            "src/file.rs"
        }

        fn folded_search_text(&self) -> &'static str {
            "needle"
        }

        fn final_score_millis(&self) -> u32 {
            1
        }

        fn token_budget_hint(&self) -> u32 {
            self.token_hint
        }

        fn source_ordinal(&self) -> usize {
            self.ordinal
        }
    }

    struct Grant(Rc<RefCell<Vec<&'static str>>>);

    impl Drop for Grant {
        fn drop(&mut self) {
            self.0.borrow_mut().push("grant");
        }
    }

    struct FakeScanner {
        deny_grant: bool,
        fail_scan: bool,
        rows: usize,
        tokens: [u32; 4],
        freeze_first_prefix: bool,
        admitted_prefix_bytes: Option<usize>,
        scans: Rc<Cell<usize>>,
        drops: Rc<RefCell<Vec<&'static str>>>,
    }

    impl RankScanner for FakeScanner {
        type Candidate = Candidate;
        type Error = &'static str;
        type Grant = Grant;

        fn resource_limited() -> Self::Error {
            "resource-limited"
        }

        fn reserve_prefix(&mut self, bytes: usize) -> Result<Self::Grant, Self::Error> {
            if self.deny_grant
                || (self.freeze_first_prefix
                    && self
                        .admitted_prefix_bytes
                        .is_some_and(|limit| bytes > limit))
            {
                Err("grant-denied")
            } else {
                self.admitted_prefix_bytes = Some(bytes);
                Ok(Grant(Rc::clone(&self.drops)))
            }
        }

        fn scan(
            &mut self,
            sink: &mut dyn FnMut(Self::Candidate) -> Result<(), Self::Error>,
        ) -> Result<(), Self::Error> {
            self.scans.set(self.scans.get().saturating_add(1));
            for (ordinal, (identity, token_hint)) in
                ["subject-a", "subject-b", "subject-c", "subject-d"]
                    .into_iter()
                    .zip(self.tokens)
                    .enumerate()
            {
                if ordinal >= self.rows {
                    break;
                }
                sink(Candidate {
                    identity,
                    ordinal,
                    token_hint,
                    drops: Rc::clone(&self.drops),
                })?;
            }
            if self.fail_scan {
                Err("scan-failed")
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn grant_denial_does_not_begin_scan() {
        let scans = Rc::new(Cell::new(0));
        let drops = Rc::new(RefCell::new(Vec::new()));
        let mut scanner = FakeScanner {
            deny_grant: true,
            fail_scan: false,
            rows: 1,
            tokens: [1; 4],
            freeze_first_prefix: false,
            admitted_prefix_bytes: None,
            scans: Rc::clone(&scans),
            drops: Rc::clone(&drops),
        };
        let focus = BTreeSet::new();
        let paths = BTreeSet::new();
        let terms = vec!["needle".to_string()];
        let plan = RankPlan {
            focus_keys: &focus,
            focus_owner_paths: &paths,
            query_terms: &terms,
            top_k_limit: 1,
            initial_prefix: 1,
            token_budget: 1,
        };
        assert!(matches!(select(&mut scanner, &plan), Err("grant-denied")));
        assert_eq!(scans.get(), 0);
        assert!(drops.borrow().is_empty());
    }

    #[test]
    fn scan_error_releases_retained_candidate_before_grant() {
        let scans = Rc::new(Cell::new(0));
        let drops = Rc::new(RefCell::new(Vec::new()));
        let mut scanner = FakeScanner {
            deny_grant: false,
            fail_scan: true,
            rows: 1,
            tokens: [1; 4],
            freeze_first_prefix: false,
            admitted_prefix_bytes: None,
            scans: Rc::clone(&scans),
            drops: Rc::clone(&drops),
        };
        let focus = BTreeSet::new();
        let paths = BTreeSet::new();
        let terms = vec!["needle".to_string()];
        let plan = RankPlan {
            focus_keys: &focus,
            focus_owner_paths: &paths,
            query_terms: &terms,
            top_k_limit: 1,
            initial_prefix: 1,
            token_budget: 1,
        };
        assert!(matches!(select(&mut scanner, &plan), Err("scan-failed")));
        assert_eq!(scans.get(), 1);
        assert_eq!(&*drops.borrow(), &["candidate", "grant"]);
    }

    #[test]
    fn first_prefix_below_top_k_does_not_claim_a_complete_universe() -> Result<(), &'static str> {
        let scans = Rc::new(Cell::new(0));
        let drops = Rc::new(RefCell::new(Vec::new()));
        let mut scanner = FakeScanner {
            deny_grant: false,
            fail_scan: false,
            rows: 2,
            tokens: [1; 4],
            freeze_first_prefix: false,
            admitted_prefix_bytes: None,
            scans: Rc::clone(&scans),
            drops,
        };
        let focus = BTreeSet::new();
        let paths = BTreeSet::new();
        let terms = vec!["needle".to_string()];
        let plan = RankPlan {
            focus_keys: &focus,
            focus_owner_paths: &paths,
            query_terms: &terms,
            top_k_limit: 2,
            initial_prefix: 1,
            token_budget: 2,
        };
        let selection = select(&mut scanner, &plan)?;
        if selection.included.len() != 2 || scans.get() != 2 {
            return Err("first prefix claimed completeness before the second scan");
        }
        Ok(())
    }

    #[test]
    fn exhausted_token_budget_does_not_expand_retained_prefix() -> Result<(), &'static str> {
        for (token_hint, token_budget, expected_count) in [
            (64, 1, 1),
            (64, 64, 1),
            (64, 100, 1),
            (64, 128, 2),
            (0, 1, 1),
            (u32::MAX, 1, 1),
        ] {
            let scans = Rc::new(Cell::new(0));
            let mut scanner = FakeScanner {
                deny_grant: false,
                fail_scan: false,
                rows: 4,
                tokens: [token_hint; 4],
                // One initial prefix fits; widening would exceed admission.
                freeze_first_prefix: true,
                admitted_prefix_bytes: None,
                scans: Rc::clone(&scans),
                drops: Rc::new(RefCell::new(Vec::new())),
            };
            let focus = BTreeSet::new();
            let paths = BTreeSet::new();
            let terms = vec!["needle".to_string()];
            let plan = RankPlan {
                focus_keys: &focus,
                focus_owner_paths: &paths,
                query_terms: &terms,
                top_k_limit: 3,
                initial_prefix: 3,
                token_budget,
            };
            let selection = select(&mut scanner, &plan)?;
            if selection.included.len() != expected_count
                || usize::try_from(selection.dropped_entries_count)
                    .map_err(|_error| "drop count exceeds platform size")?
                    != 4 - expected_count
                || selection.drop_reason_codes != ["token_budget_exhausted"]
                || selection.degraded_reason_codes
                    != if token_budget < token_hint.max(1) {
                        vec!["token_budget_floor_applied"]
                    } else {
                        Vec::new()
                    }
                || scans.get() != 1
            {
                return Err("fixed-cost token exhaustion widened the prefix or changed rank facts");
            }
        }
        Ok(())
    }

    #[test]
    fn smaller_unseen_row_still_extends_token_prefix() -> Result<(), &'static str> {
        let scans = Rc::new(Cell::new(0));
        let mut scanner = FakeScanner {
            deny_grant: false,
            fail_scan: false,
            rows: 4,
            tokens: [5, 4, 3, 1],
            freeze_first_prefix: false,
            admitted_prefix_bytes: None,
            scans: Rc::clone(&scans),
            drops: Rc::new(RefCell::new(Vec::new())),
        };
        let focus = BTreeSet::new();
        let paths = BTreeSet::new();
        let terms = vec!["needle".to_string()];
        let selection = select(
            &mut scanner,
            &RankPlan {
                focus_keys: &focus,
                focus_owner_paths: &paths,
                query_terms: &terms,
                top_k_limit: 3,
                initial_prefix: 3,
                token_budget: 6,
            },
        )?;
        if selection
            .included
            .iter()
            .map(|(candidate, rank)| (candidate.identity, *rank))
            .collect::<Vec<_>>()
            != [("subject-a", 1), ("subject-d", 2)]
            || selection.dropped_entries_count != 2
            || selection.drop_reason_codes != ["token_budget_exhausted"]
            || scans.get() != 2
        {
            return Err("a smaller row beyond the first prefix was not considered");
        }
        Ok(())
    }
}
