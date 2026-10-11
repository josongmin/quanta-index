//! A sound, bounded necessary condition for exact byte matching.
//!
//! This expression only selects candidates. A caller must verify every
//! candidate against the original matcher. `AllDocs` means no gram filter is
//! available; it never means an empty result. The grams refer to the raw
//! indexed byte stream. A caller using a transformed stream must derive a
//! separate condition proved sound for that transformation.

use crate::errors::{LimitDimension, TrigramError};
use crate::source::TrigramPostingSource;
use crate::types::Trigram;
use crate::types::{DocId, MAX_CANDIDATE_PRE_VERIFY, MAX_TRIGRAMS_PER_QUERY};

/// A Boolean necessary condition over byte-trigram membership.
///
/// For any exact match, this expression must evaluate to true for the same
/// document. `And` and `Or` have their ordinary Boolean meanings. The planner
/// may weaken a condition to `AllDocs` when it cannot prove a useful gram or
/// when its construction budget is exhausted. Negation is intentionally absent:
/// a trigram posting can include false positives for the exact regex.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NecessaryGramExpr {
    /// Every document is a candidate; exact verification is still required.
    AllDocs,
    /// The document must contain this byte trigram.
    Gram(Trigram),
    /// Every child condition is necessary.
    And(Vec<Self>),
    /// At least one child condition is necessary.
    Or(Vec<Self>),
}

/// A filter result. `AllDocs` requires the caller to enumerate its admitted
/// document universe; an empty `Docs` is a proven empty posting result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NecessaryGramCandidates {
    AllDocs,
    Docs(Vec<DocId>),
}

/// Evaluate a necessary condition against the same raw-byte stream used by
/// exact verification.
///
/// All returned IDs are candidates, never regex matches. Candidate caps are
/// refusals, never a reason to silently drop a branch.
pub fn evaluate_necessary_gram_expr<S: TrigramPostingSource + ?Sized>(
    source: &S,
    expr: &NecessaryGramExpr,
) -> Result<NecessaryGramCandidates, TrigramError> {
    evaluate_with_context(source, expr, &[], 0)
}

// The regex planner caps HIR descent at 64 and may add AND/OR wrappers.
const MAX_EVAL_DEPTH: usize = 128;

fn evaluate_with_context<S: TrigramPostingSource + ?Sized>(
    source: &S,
    expr: &NecessaryGramExpr,
    context: &[Trigram],
    depth: usize,
) -> Result<NecessaryGramCandidates, TrigramError> {
    if depth >= MAX_EVAL_DEPTH {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            "necessary gram expression nesting exceeds evaluation bound",
        ));
    }
    match expr {
        NecessaryGramExpr::AllDocs => lookup_context(source, context),
        NecessaryGramExpr::Gram(gram) => {
            let mut grams = clone_grams(context, 1)?;
            grams.push(*gram);
            lookup_context(source, &grams)
        }
        NecessaryGramExpr::And(children) => {
            let mut grams = clone_grams(context, 0)?;
            let mut remaining = Vec::new();
            for child in children {
                collect_conjuncts(child, &mut grams, &mut remaining, depth.saturating_add(1))?;
            }
            grams.sort_unstable();
            grams.dedup();
            if grams.len() > MAX_TRIGRAMS_PER_QUERY {
                return Err(TrigramError::plan_limit(
                    LimitDimension::Trigrams,
                    format!("necessary gram conjunction has {} grams", grams.len()),
                ));
            }
            if remaining.is_empty() {
                return lookup_context(source, &grams);
            }
            let mut result = NecessaryGramCandidates::AllDocs;
            for child in remaining {
                if matches!(&result, NecessaryGramCandidates::Docs(ids) if ids.is_empty()) {
                    break;
                }
                let next = evaluate_with_context(source, child, &grams, depth.saturating_add(1))?;
                result = intersect_candidates(result, next)?;
            }
            Ok(result)
        }
        NecessaryGramExpr::Or(children) => {
            if children
                .iter()
                .any(|child| is_universal(child, depth.saturating_add(1)))
            {
                return lookup_context(source, context);
            }
            let mut result = NecessaryGramCandidates::Docs(Vec::new());
            for child in children {
                let next = evaluate_with_context(source, child, context, depth.saturating_add(1))?;
                result = union_candidates(result, next)?;
                if matches!(result, NecessaryGramCandidates::AllDocs) {
                    break;
                }
            }
            Ok(result)
        }
    }
}

fn clone_grams(context: &[Trigram], extra: usize) -> Result<Vec<Trigram>, TrigramError> {
    let mut grams = Vec::new();
    grams
        .try_reserve(context.len().saturating_add(extra))
        .map_err(|error| {
            TrigramError::plan_limit(
                LimitDimension::Trigrams,
                format!("necessary gram allocation refused: {error}"),
            )
        })?;
    grams.extend_from_slice(context);
    Ok(grams)
}

fn collect_conjuncts<'a>(
    expr: &'a NecessaryGramExpr,
    grams: &mut Vec<Trigram>,
    remaining: &mut Vec<&'a NecessaryGramExpr>,
    depth: usize,
) -> Result<(), TrigramError> {
    if depth >= MAX_EVAL_DEPTH {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            "necessary gram conjunction nesting exceeds evaluation bound",
        ));
    }
    match expr {
        NecessaryGramExpr::AllDocs => {}
        NecessaryGramExpr::Gram(gram) => {
            if grams.len() >= MAX_TRIGRAMS_PER_QUERY {
                return Err(TrigramError::plan_limit(
                    LimitDimension::Trigrams,
                    "necessary gram conjunction exceeds trigram cap",
                ));
            }
            grams.try_reserve(1).map_err(|error| {
                TrigramError::plan_limit(
                    LimitDimension::Trigrams,
                    format!("necessary gram allocation refused: {error}"),
                )
            })?;
            grams.push(*gram);
        }
        NecessaryGramExpr::And(children) => {
            for child in children {
                collect_conjuncts(child, grams, remaining, depth.saturating_add(1))?;
            }
        }
        NecessaryGramExpr::Or(_) => {
            if !is_universal(expr, depth) {
                remaining.try_reserve(1).map_err(|error| {
                    TrigramError::plan_limit(
                        LimitDimension::Trigrams,
                        format!("necessary gram branch allocation refused: {error}"),
                    )
                })?;
                remaining.push(expr);
            }
        }
    }
    Ok(())
}

fn is_universal(expr: &NecessaryGramExpr, depth: usize) -> bool {
    if depth >= MAX_EVAL_DEPTH {
        return false;
    }
    match expr {
        NecessaryGramExpr::AllDocs => true,
        NecessaryGramExpr::Gram(_) => false,
        NecessaryGramExpr::And(children) => children
            .iter()
            .all(|child| is_universal(child, depth.saturating_add(1))),
        NecessaryGramExpr::Or(children) => children
            .iter()
            .any(|child| is_universal(child, depth.saturating_add(1))),
    }
}

fn lookup_context<S: TrigramPostingSource + ?Sized>(
    source: &S,
    context: &[Trigram],
) -> Result<NecessaryGramCandidates, TrigramError> {
    if context.is_empty() {
        Ok(NecessaryGramCandidates::AllDocs)
    } else {
        if context.len() > MAX_TRIGRAMS_PER_QUERY {
            return Err(TrigramError::plan_limit(
                LimitDimension::Trigrams,
                "necessary gram context exceeds trigram cap",
            ));
        }
        source
            .intersect_trigrams(context)
            .map(NecessaryGramCandidates::Docs)
    }
}

fn intersect_candidates(
    lhs: NecessaryGramCandidates,
    rhs: NecessaryGramCandidates,
) -> Result<NecessaryGramCandidates, TrigramError> {
    match (lhs, rhs) {
        (NecessaryGramCandidates::AllDocs, other) | (other, NecessaryGramCandidates::AllDocs) => {
            Ok(other)
        }
        (NecessaryGramCandidates::Docs(a), NecessaryGramCandidates::Docs(b)) => {
            let mut out = Vec::new();
            out.try_reserve(a.len().min(b.len())).map_err(|error| {
                TrigramError::plan_limit(
                    LimitDimension::CandidateSet,
                    format!("necessary gram intersection allocation refused: {error}"),
                )
            })?;
            let mut left = a.into_iter().peekable();
            let mut right = b.into_iter().peekable();
            while let (Some(&left_id), Some(&right_id)) = (left.peek(), right.peek()) {
                match left_id.cmp(&right_id) {
                    core::cmp::Ordering::Less => {
                        let _advanced = left.next();
                    }
                    core::cmp::Ordering::Greater => {
                        let _advanced = right.next();
                    }
                    core::cmp::Ordering::Equal => {
                        out.push(left_id);
                        let _left = left.next();
                        let _right = right.next();
                    }
                }
            }
            Ok(NecessaryGramCandidates::Docs(out))
        }
    }
}

fn union_candidates(
    lhs: NecessaryGramCandidates,
    rhs: NecessaryGramCandidates,
) -> Result<NecessaryGramCandidates, TrigramError> {
    match (lhs, rhs) {
        (NecessaryGramCandidates::AllDocs, _) | (_, NecessaryGramCandidates::AllDocs) => {
            Ok(NecessaryGramCandidates::AllDocs)
        }
        (NecessaryGramCandidates::Docs(a), NecessaryGramCandidates::Docs(b)) => {
            let mut out = Vec::new();
            out.try_reserve(
                a.len()
                    .saturating_add(b.len())
                    .min(MAX_CANDIDATE_PRE_VERIFY),
            )
            .map_err(|error| {
                TrigramError::plan_limit(
                    LimitDimension::CandidateSet,
                    format!("necessary gram union allocation refused: {error}"),
                )
            })?;
            let mut left = a.into_iter().peekable();
            let mut right = b.into_iter().peekable();
            loop {
                let next = match (left.peek().copied(), right.peek().copied()) {
                    (Some(left_id), Some(right_id)) if left_id < right_id => left.next(),
                    (Some(left_id), Some(right_id)) if right_id < left_id => right.next(),
                    (Some(_), Some(_)) => {
                        let _left = left.next();
                        right.next()
                    }
                    (Some(_), None) => left.next(),
                    (None, Some(_)) => right.next(),
                    (None, None) => break,
                };
                let Some(next) = next else { break };
                if out.last() != Some(&next) {
                    if out.len() >= MAX_CANDIDATE_PRE_VERIFY {
                        return Err(TrigramError::plan_limit(
                            LimitDimension::CandidateSet,
                            format!(
                                "necessary gram union exceeds {MAX_CANDIDATE_PRE_VERIFY} candidates"
                            ),
                        ));
                    }
                    out.push(next);
                }
            }
            Ok(NecessaryGramCandidates::Docs(out))
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed candidate oracles assert exact set semantics"
)]
mod tests {
    use super::{NecessaryGramCandidates, NecessaryGramExpr, evaluate_necessary_gram_expr};
    use crate::{DocId, TrigramIndexBuilder};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn all_docs_is_distinct_from_an_empty_posting_result() -> TestResult {
        let mut builder = TrigramIndexBuilder::new(1)?;
        builder.add_doc(DocId(1), b"abc");
        let index = builder.finish();
        assert_eq!(
            evaluate_necessary_gram_expr(&index, &NecessaryGramExpr::AllDocs)?,
            NecessaryGramCandidates::AllDocs,
        );
        assert_eq!(
            evaluate_necessary_gram_expr(&index, &NecessaryGramExpr::Gram(*b"xyz"))?,
            NecessaryGramCandidates::Docs(Vec::new()),
        );
        Ok(())
    }

    #[test]
    fn conjunction_and_alternation_retain_exact_candidate_sets() -> TestResult {
        let mut builder = TrigramIndexBuilder::new(1)?;
        for (id, bytes) in [(1, b"abc-def".as_slice()), (2, b"abc-xyz"), (3, b"xyz-def")] {
            builder.add_doc(DocId(id), bytes);
        }
        let index = builder.finish();
        let expr = NecessaryGramExpr::Or(vec![
            NecessaryGramExpr::And(vec![
                NecessaryGramExpr::Gram(*b"abc"),
                NecessaryGramExpr::Gram(*b"def"),
            ]),
            NecessaryGramExpr::Gram(*b"xyz"),
        ]);
        assert_eq!(
            evaluate_necessary_gram_expr(&index, &expr)?,
            NecessaryGramCandidates::Docs(vec![DocId(1), DocId(2), DocId(3)]),
        );
        assert_eq!(
            evaluate_necessary_gram_expr(
                &index,
                &NecessaryGramExpr::Or(vec![expr, NecessaryGramExpr::AllDocs])
            )?,
            NecessaryGramCandidates::AllDocs,
        );
        Ok(())
    }

    #[test]
    fn direct_gram_conjunction_avoids_intermediate_broad_leaf_cap() -> TestResult {
        struct BroadLeaf;
        impl crate::TrigramPostingSource for BroadLeaf {
            fn intersect_trigrams(
                &self,
                grams: &[crate::Trigram],
            ) -> Result<Vec<DocId>, crate::TrigramError> {
                if grams.len() == 1 {
                    return Err(crate::TrigramError::plan_limit(
                        crate::LimitDimension::CandidateSet,
                        "single gram posting is broader than the candidate cap",
                    ));
                }
                if grams != [*b"abc", *b"xyz"] {
                    return Err(crate::TrigramError::new(
                        crate::TrigramErrorCode::IndexCorrupted,
                        "mock received the wrong gram conjunction",
                    ));
                }
                Ok(vec![DocId(7)])
            }
        }
        let expr = NecessaryGramExpr::And(vec![
            NecessaryGramExpr::Gram(*b"abc"),
            NecessaryGramExpr::Gram(*b"xyz"),
        ]);
        assert_eq!(
            evaluate_necessary_gram_expr(&BroadLeaf, &expr)?,
            NecessaryGramCandidates::Docs(vec![DocId(7)]),
        );
        Ok(())
    }

    #[test]
    fn rare_and_context_is_applied_inside_broad_or_before_posting_cap() -> TestResult {
        use std::cell::RefCell;

        struct BroadOr(RefCell<Vec<Vec<crate::Trigram>>>);
        impl crate::TrigramPostingSource for BroadOr {
            fn intersect_trigrams(
                &self,
                grams: &[crate::Trigram],
            ) -> Result<Vec<DocId>, crate::TrigramError> {
                self.0.borrow_mut().push(grams.to_vec());
                if !grams.contains(b"rar") || grams.len() != 2 {
                    return Err(crate::TrigramError::plan_limit(
                        crate::LimitDimension::CandidateSet,
                        "unrestricted OR branch exceeds candidate cap",
                    ));
                }
                if grams.contains(b"abc") {
                    Ok(vec![DocId(7)])
                } else {
                    Ok(Vec::new())
                }
            }
        }
        let source = BroadOr(RefCell::new(Vec::new()));
        let expr = NecessaryGramExpr::And(vec![
            NecessaryGramExpr::Or(vec![
                NecessaryGramExpr::Gram(*b"abc"),
                NecessaryGramExpr::Gram(*b"xyz"),
            ]),
            NecessaryGramExpr::And(vec![NecessaryGramExpr::Gram(*b"rar")]),
        ]);
        assert_eq!(
            evaluate_necessary_gram_expr(&source, &expr)?,
            NecessaryGramCandidates::Docs(vec![DocId(7)]),
        );
        assert_eq!(source.0.borrow().len(), 2);
        Ok(())
    }

    #[test]
    fn nested_universal_or_preflights_without_reading_postings() -> TestResult {
        struct NoRead(std::cell::Cell<bool>);
        impl crate::TrigramPostingSource for NoRead {
            fn intersect_trigrams(
                &self,
                _: &[crate::Trigram],
            ) -> Result<Vec<DocId>, crate::TrigramError> {
                self.0.set(true);
                Err(crate::TrigramError::new(
                    crate::TrigramErrorCode::IndexCorrupted,
                    "universal expression read a posting",
                ))
            }
        }
        let source = NoRead(std::cell::Cell::new(false));
        let expr = NecessaryGramExpr::Or(vec![
            NecessaryGramExpr::Gram(*b"abc"),
            NecessaryGramExpr::And(Vec::new()),
        ]);
        assert_eq!(
            evaluate_necessary_gram_expr(&source, &expr)?,
            NecessaryGramCandidates::AllDocs,
        );
        assert!(!source.0.get());
        Ok(())
    }
}
