//! Sound necessary byte-gram conditions from the validated regex HIR.
//!
//! The postings and final matcher must observe the same raw byte stream.
//! Structural classes and optional branches cannot establish one exact byte
//! gram. Finite case variants may still yield a bounded prefix condition.

use quanta_index_lq_trigram::{NecessaryGramExpr, trigrams_of};
use regex_syntax::hir::literal::Extractor;
use regex_syntax::hir::{Hir, HirKind};

const MAX_EXPR_NODES: usize = 256;
const MAX_EXPR_DEPTH: usize = 64;
const MAX_CHILDREN: usize = 64;
const MAX_LITERAL_GRAMS: usize = 32;
const MAX_PREFIX_BYTES: usize = 4_096;
const MAX_FINAL_EXPR_NODES: usize = 512;

/// Extract a bounded necessary condition. Resource exhaustion weakens the
/// condition; it never excludes an exact match.
pub(crate) fn from_hir(hir: &Hir) -> NecessaryGramExpr {
    let mut remaining = MAX_EXPR_NODES;
    let structural = visit(hir, &mut remaining, 0);
    // The extractor finds a bounded set of literal prefixes, including
    // finite case-fold variants. Its alternatives are OR, while each
    // literal's trigrams are AND. This is another necessary condition and
    // can safely strengthen independent structural conditions.
    let prefix = prefix_condition(hir);
    let combined = simplify_and(vec![structural.clone(), prefix.clone()]);
    if expr_node_count(&combined) <= MAX_FINAL_EXPR_NODES {
        combined
    } else if expr_node_count(&structural) <= MAX_FINAL_EXPR_NODES {
        structural
    } else if expr_node_count(&prefix) <= MAX_FINAL_EXPR_NODES {
        prefix
    } else {
        NecessaryGramExpr::AllDocs
    }
}

fn expr_node_count(expr: &NecessaryGramExpr) -> usize {
    match expr {
        NecessaryGramExpr::AllDocs | NecessaryGramExpr::Gram(_) => 1,
        NecessaryGramExpr::And(children) | NecessaryGramExpr::Or(children) => {
            children.iter().fold(1usize, |sum, child| {
                sum.saturating_add(expr_node_count(child))
            })
        }
    }
}

fn prefix_condition(hir: &Hir) -> NecessaryGramExpr {
    let seq = Extractor::new().extract(hir);
    let Some(literals) = seq.literals() else {
        return NecessaryGramExpr::AllDocs;
    };
    if literals.is_empty() || literals.len() > MAX_CHILDREN {
        return NecessaryGramExpr::AllDocs;
    }
    let mut total_bytes = 0usize;
    let mut total_grams = 0usize;
    let mut alternatives = Vec::new();
    if alternatives.try_reserve(literals.len()).is_err() {
        return NecessaryGramExpr::AllDocs;
    }
    for literal in literals {
        let bytes = literal.as_bytes();
        total_bytes = total_bytes.saturating_add(bytes.len());
        if bytes.len() < 3 || total_bytes > MAX_PREFIX_BYTES {
            return NecessaryGramExpr::AllDocs;
        }
        let mut grams: Vec<_> = trigrams_of(bytes).take(MAX_LITERAL_GRAMS).collect();
        grams.sort_unstable();
        grams.dedup();
        total_grams = total_grams.saturating_add(grams.len());
        if total_grams > MAX_EXPR_NODES {
            return NecessaryGramExpr::AllDocs;
        }
        alternatives.push(simplify_and(
            grams.into_iter().map(NecessaryGramExpr::Gram).collect(),
        ));
    }
    simplify_or(alternatives)
}

fn visit(hir: &Hir, remaining: &mut usize, depth: usize) -> NecessaryGramExpr {
    if depth >= MAX_EXPR_DEPTH || *remaining == 0 {
        return NecessaryGramExpr::AllDocs;
    }
    *remaining = remaining.saturating_sub(1);
    match hir.kind() {
        HirKind::Literal(lit) => {
            let mut grams: Vec<_> = trigrams_of(&lit.0)
                .take(MAX_LITERAL_GRAMS.min(*remaining))
                .collect();
            grams.sort_unstable();
            grams.dedup();
            *remaining = remaining.saturating_sub(grams.len());
            simplify_and(grams.into_iter().map(NecessaryGramExpr::Gram).collect())
        }
        HirKind::Capture(capture) => visit(&capture.sub, remaining, depth.saturating_add(1)),
        HirKind::Repetition(repetition) if repetition.min > 0 => {
            visit(&repetition.sub, remaining, depth.saturating_add(1))
        }
        HirKind::Concat(children) => {
            let mut conditions = Vec::new();
            for child in children.iter().take(MAX_CHILDREN) {
                let expr = visit(child, remaining, depth.saturating_add(1));
                match expr {
                    NecessaryGramExpr::AllDocs => {}
                    NecessaryGramExpr::And(parts) => conditions.extend(parts),
                    other @ (NecessaryGramExpr::Gram(_) | NecessaryGramExpr::Or(_)) => {
                        conditions.push(other);
                    }
                }
            }
            simplify_and(conditions)
        }
        HirKind::Alternation(children) => {
            if children.len() > MAX_CHILDREN {
                return NecessaryGramExpr::AllDocs;
            }
            let mut conditions = Vec::new();
            for child in children {
                let expr = visit(child, remaining, depth.saturating_add(1));
                if expr == NecessaryGramExpr::AllDocs {
                    return NecessaryGramExpr::AllDocs;
                }
                conditions.push(expr);
            }
            simplify_or(conditions)
        }
        HirKind::Empty | HirKind::Class(_) | HirKind::Look(_) | HirKind::Repetition(_) => {
            NecessaryGramExpr::AllDocs
        }
    }
}

fn simplify_and(mut conditions: Vec<NecessaryGramExpr>) -> NecessaryGramExpr {
    conditions.retain(|condition| *condition != NecessaryGramExpr::AllDocs);
    conditions.sort_unstable();
    conditions.dedup();
    match conditions.len() {
        0 => NecessaryGramExpr::AllDocs,
        1 => conditions.remove(0),
        _ => NecessaryGramExpr::And(conditions),
    }
}

fn simplify_or(mut conditions: Vec<NecessaryGramExpr>) -> NecessaryGramExpr {
    conditions.sort_unstable();
    conditions.dedup();
    match conditions.len() {
        0 => NecessaryGramExpr::AllDocs,
        1 => conditions.remove(0),
        _ => NecessaryGramExpr::Or(conditions),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed regex implication oracles assert exact behavior"
)]
mod tests {
    use quanta_index_lq_trigram::NecessaryGramExpr;

    use super::MAX_FINAL_EXPR_NODES;
    use crate::RegexExecutor;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    // Independent truth oracle: actual engine matching implies each selected
    // raw gram occurs in the same document. No planner logic is reused here.
    fn satisfied(expr: &NecessaryGramExpr, bytes: &[u8]) -> bool {
        match expr {
            NecessaryGramExpr::AllDocs => true,
            NecessaryGramExpr::Gram(gram) => bytes.windows(3).any(|window| window == gram),
            NecessaryGramExpr::And(children) => {
                children.iter().all(|child| satisfied(child, bytes))
            }
            NecessaryGramExpr::Or(children) => children.iter().any(|child| satisfied(child, bytes)),
        }
    }

    #[test]
    fn exact_matches_imply_the_necessary_expression() -> TestResult {
        let corpus: &[&[u8]] = &[
            b"foo then bar",
            b"FOO then BAR",
            b"foobar",
            b"abcxyz",
            b"xyz",
            b"a",
            "fresh freſh".as_bytes(),
            "日本語と日本".as_bytes(),
            "é decomposed e\u{301}".as_bytes(),
        ];
        for pattern in [
            "foo",
            "foo.*bar",
            "(?i)foo.*bar",
            "(?i)fresh",
            "a|foobar",
            "é|foobar",
            "(?:abc)?xyz",
            "[abc]foo",
            ".*foo.*bar.*",
            "日本",
            "é",
            ".*",
        ] {
            let executor = RegexExecutor::compile(pattern)?;
            let expr = executor.necessary_gram_expr();
            for bytes in corpus {
                if executor.verify(bytes) {
                    assert!(
                        satisfied(&expr, bytes),
                        "false negative: {pattern:?}, {bytes:?}, {expr:?}"
                    );
                }
            }
            let prepared = RegexExecutor::prepare(pattern)?;
            assert_eq!(prepared.necessary_gram_expr(), expr);
        }
        Ok(())
    }

    #[test]
    fn short_or_optional_branch_never_filters_matching_documents() -> TestResult {
        let short = RegexExecutor::compile("a|foobar")?;
        assert_eq!(short.necessary_gram_expr(), NecessaryGramExpr::AllDocs);
        let optional = RegexExecutor::compile("(?:abc)?xyz")?;
        assert!(satisfied(&optional.necessary_gram_expr(), b"xyz"));
        assert!(satisfied(&optional.necessary_gram_expr(), b"abcxyz"));
        Ok(())
    }

    #[test]
    fn case_variants_keep_a_useful_bounded_prefix() -> TestResult {
        let executor = RegexExecutor::compile("(?i)foo.*bar")?;
        let expr = executor.necessary_gram_expr();
        assert_ne!(expr, NecessaryGramExpr::AllDocs);
        assert!(satisfied(&expr, b"FOO and BAR"));
        assert!(satisfied(&expr, b"foo and bar"));
        Ok(())
    }

    #[test]
    fn large_literal_is_bounded_and_stays_sound() -> TestResult {
        let literal = "abc".repeat(500);
        let executor = RegexExecutor::compile(&literal)?;
        let expr = executor.necessary_gram_expr();
        assert!(satisfied(&expr, literal.as_bytes()));
        assert!(count_nodes(&expr) <= MAX_FINAL_EXPR_NODES);
        Ok(())
    }

    fn count_nodes(expr: &NecessaryGramExpr) -> usize {
        match expr {
            NecessaryGramExpr::AllDocs | NecessaryGramExpr::Gram(_) => 1,
            NecessaryGramExpr::And(children) | NecessaryGramExpr::Or(children) => {
                1_usize.saturating_add(children.iter().map(count_nodes).sum::<usize>())
            }
        }
    }

    #[test]
    fn many_distinct_branches_stay_within_the_final_node_cap() -> TestResult {
        let branches: Vec<String> = (0..80).map(|n| format!("a{n:03}.*b{n:03}")).collect();
        let pattern = branches.join("|");
        let prepared = RegexExecutor::prepare(&pattern)?;
        let expr = prepared.necessary_gram_expr();
        assert!(count_nodes(&expr) <= MAX_FINAL_EXPR_NODES);
        for n in [0, 17, 79] {
            let text = format!("a{n:03} middle b{n:03}");
            assert!(satisfied(&expr, text.as_bytes()));
        }
        Ok(())
    }
}
