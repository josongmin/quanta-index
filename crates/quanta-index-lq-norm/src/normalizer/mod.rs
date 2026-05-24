//! LQ canonical normalizer.
//!
//! Idempotent: `normalize(normalize(x)) == normalize(x)` (byte-identical).
//!
//! Transforms per dsl.md §10:
//! 1. Apply `LqOptions.case` to pattern leaves (case-fold the `Keyword` /
//!    `Phrase` bodies when `case:no`; raw / regex / structural preserved).
//! 2. Flatten nested `LqExpr::All`/`LqExpr::Any` of the same kind (one level
//!    deep — fixed-point after the recursive `normalize_expr` returns).
//! 3. Sort commutative children (`All`, `Any`) by canonical key.
//! 4. Dedup adjacent identical children.
//! 5. Sort filters and directives by canonical key.
//! 6. Collapse trivial single-child `All`/`Any` to the child.
//!
//! The canonical key used for ordering is a stable string derived from the
//! structural form of each sub-tree. It is not the hash output; the hash
//! is computed by [`crate::hasher`] after normalize completes.

use crate::ast::{LqCase, LqDirective, LqExpr, LqFilter, LqLeaf, LqNormalizedQuery, LqOptions};
use crate::errors::LqParseError;
use crate::limits::MAX_FANOUT_PER_NODE;

/// Normalize a parsed query into canonical form.
///
/// Returns an error only if the normalize pass surfaces a deferred limit
/// violation (e.g. fan-out after flattening). The `LqParseError` shape is
/// shared with parse so callers see one typed surface.
pub fn normalize(mut q: LqNormalizedQuery) -> Result<LqNormalizedQuery, LqParseError> {
    q.expr = normalize_expr(q.expr, q.options.case)?;
    sort_filters(&mut q.filters);
    sort_directives(&mut q.directives);
    Ok(q)
}

fn normalize_expr(expr: LqExpr, case: Option<LqCase>) -> Result<LqExpr, LqParseError> {
    match expr {
        LqExpr::Empty => Ok(LqExpr::Empty),
        LqExpr::Leaf(leaf) => Ok(LqExpr::Leaf(apply_case_to_leaf(leaf, case))),
        LqExpr::Not(inner) => {
            let inner = normalize_expr(*inner, case)?;
            // Constant fold: NOT NOT x → x.
            if let LqExpr::Not(grandchild) = inner {
                return Ok(*grandchild);
            }
            Ok(LqExpr::Not(Box::new(inner)))
        }
        LqExpr::All(children) => normalize_n_ary(children, case, /*is_all=*/ true),
        LqExpr::Any(children) => normalize_n_ary(children, case, /*is_all=*/ false),
    }
}

fn normalize_n_ary(
    children: Vec<LqExpr>,
    case: Option<LqCase>,
    is_all: bool,
) -> Result<LqExpr, LqParseError> {
    // 1. Recursively normalize each child.
    let mut normalized: Vec<LqExpr> = Vec::with_capacity(children.len());
    for c in children {
        let nc = normalize_expr(c, case)?;
        normalized.push(nc);
    }
    // 2. Flatten: pull up same-kind nested vectors.
    let mut flat: Vec<LqExpr> = Vec::with_capacity(normalized.len());
    for c in normalized {
        match c {
            LqExpr::All(inner) if is_all => flat.extend(inner),
            LqExpr::Any(inner) if !is_all => flat.extend(inner),
            kept @ (LqExpr::All(_)
            | LqExpr::Any(_)
            | LqExpr::Empty
            | LqExpr::Leaf(_)
            | LqExpr::Not(_)) => flat.push(kept),
        }
    }
    // 3. Drop Empty children (commutative identity).
    flat.retain(|c| !matches!(c, LqExpr::Empty));
    // 4. Sort by canonical key.
    flat.sort_by_cached_key(canonical_key);
    // 5. Dedup adjacent identical children.
    flat.dedup();
    // 6. Collapse trivial cases.
    if flat.is_empty() {
        return Ok(LqExpr::Empty);
    }
    if flat.len() == 1 {
        let Some(single) = flat.into_iter().next() else {
            return Ok(LqExpr::Empty);
        };
        return Ok(single);
    }
    if flat.len() > MAX_FANOUT_PER_NODE {
        return Err(LqParseError::new(
            crate::errors::LqParseErrorCode::LimitExceededFanout,
            crate::errors::LqSpan::new(0, 0),
            "fan-out exceeds 64 after normalize",
        ));
    }
    if is_all {
        Ok(LqExpr::All(flat))
    } else {
        Ok(LqExpr::Any(flat))
    }
}

fn apply_case_to_leaf(leaf: LqLeaf, case: Option<LqCase>) -> LqLeaf {
    if !matches!(case, Some(LqCase::Insensitive)) {
        return leaf;
    }
    match leaf {
        LqLeaf::Keyword(s) => LqLeaf::Keyword(s.to_lowercase()),
        LqLeaf::Phrase(s) => LqLeaf::Phrase(s.to_lowercase()),
        // Raw strings and regex bodies are case-preserved: their
        // case-handling is delegated to the regex engine / literal matcher.
        // Structural patterns are not case-folded either (per dsl.md §8).
        LqLeaf::RawString(s) => LqLeaf::RawString(s),
        LqLeaf::Regex(s) => LqLeaf::Regex(s),
        LqLeaf::StructuralBlock(s) => LqLeaf::StructuralBlock(s),
    }
}

fn sort_filters(filters: &mut Vec<LqFilter>) {
    filters.sort_by_cached_key(filter_canonical_key);
    filters.dedup();
}

fn sort_directives(directives: &mut Vec<LqDirective>) {
    directives.sort_by_cached_key(directive_canonical_key);
    directives.dedup();
}

/// Stable, total ordering key for an `LqExpr`.
///
/// The format is engineering shorthand — never on the wire and never
/// participates in the canonical hash; it only needs to be a total order
/// under structural equality.
fn canonical_key(e: &LqExpr) -> String {
    let mut out = String::new();
    write_expr_key(&mut out, e);
    out
}

fn write_expr_key(out: &mut String, e: &LqExpr) {
    match e {
        LqExpr::Empty => out.push('E'),
        LqExpr::Leaf(leaf) => {
            out.push_str("L:");
            write_leaf_key(out, leaf);
        }
        LqExpr::Not(inner) => {
            out.push_str("N(");
            write_expr_key(out, inner);
            out.push(')');
        }
        LqExpr::All(children) => {
            out.push_str("A(");
            for (i, c) in children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_expr_key(out, c);
            }
            out.push(')');
        }
        LqExpr::Any(children) => {
            out.push_str("O(");
            for (i, c) in children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_expr_key(out, c);
            }
            out.push(')');
        }
    }
}

fn write_leaf_key(out: &mut String, leaf: &LqLeaf) {
    match leaf {
        LqLeaf::Keyword(s) => {
            out.push_str("K:");
            out.push_str(s);
        }
        LqLeaf::Phrase(s) => {
            out.push_str("P:");
            out.push_str(s);
        }
        LqLeaf::RawString(s) => {
            out.push_str("R:");
            out.push_str(s);
        }
        LqLeaf::Regex(s) => {
            out.push_str("X:");
            out.push_str(s);
        }
        LqLeaf::StructuralBlock(s) => {
            out.push_str("S:");
            out.push_str(s);
        }
    }
}

fn filter_canonical_key(f: &LqFilter) -> String {
    match f {
        LqFilter::Repo { pattern, revs } => format!("repo:{pattern}@{}", revs.join(",")),
        LqFilter::File { pattern, scope } => format!("file:{}:{pattern}", scope.as_str()),
        LqFilter::Lang { id } => format!("lang:{id}"),
        LqFilter::Rev { spec } => format!("rev:{spec}"),
        LqFilter::Type { kind } => format!("type:{}", kind.as_str()),
        LqFilter::Select { dim } => format!("select:{}", dim.as_str()),
        LqFilter::Fork { mode } => format!("fork:{}", mode.as_str()),
        LqFilter::Archived { mode } => format!("archived:{}", mode.as_str()),
        LqFilter::Visibility { mode } => format!("visibility:{}", mode.as_str()),
        LqFilter::Context { name } => format!("context:{name}"),
        LqFilter::Content { leaf } => {
            let mut s = String::from("content:");
            write_leaf_key(&mut s, leaf);
            s
        }
    }
}

fn directive_canonical_key(d: &LqDirective) -> String {
    match d {
        LqDirective::IntoCodeQl => "into:codeql".to_owned(),
        LqDirective::ScopeResults => "scope:results".to_owned(),
        LqDirective::WithLexical => "with:lexical".to_owned(),
    }
}

/// Type-state alias of [`LqOptions`] kept here so future extensions can
/// hook normalize-time option mutations without re-exporting the AST.
pub type NormalizedOptions = LqOptions;

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test fixtures only assert one happy variant; the wildcard catches the fail-loudly path and is not a production-code blind spot"
)]
mod tests {
    use super::normalize;
    use crate::ast::{LqCase, LqExpr, LqLeaf, LqNormalizedQuery, LqOptions};
    use crate::parser::parse;
    use crate::tokenizer::tokenize;

    fn run(s: &str) -> LqNormalizedQuery {
        let toks = match tokenize(s) {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "tokenize failed: {e}");
                return LqNormalizedQuery::empty(crate::errors::LqSpan::new(0, 0));
            }
        };
        let q = match parse(&toks, s) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed: {e}");
                return LqNormalizedQuery::empty(crate::errors::LqSpan::new(0, 0));
            }
        };
        match normalize(q) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "normalize failed: {e}");
                LqNormalizedQuery::empty(crate::errors::LqSpan::new(0, 0))
            }
        }
    }

    #[test]
    fn idempotent_on_single_keyword() {
        let q1 = run("foo");
        let q2 = match normalize(q1.clone()) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "second normalize failed: {e}");
                return;
            }
        };
        assert_eq!(q1, q2);
    }

    #[test]
    fn or_children_are_sorted_canonically() {
        let q = run("zeta OR alpha OR mu");
        match q.expr {
            LqExpr::Any(children) => {
                let leaves: Vec<&str> = children
                    .iter()
                    .filter_map(|c| match c {
                        LqExpr::Leaf(LqLeaf::Keyword(s)) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(leaves, vec!["alpha", "mu", "zeta"]);
            }
            other => {
                assert!(false, "expected Any, got {other:?}");
            }
        }
    }

    #[test]
    fn case_insensitive_lowercases_keyword_leaves() {
        let q = run("Foo case:no");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Keyword(s)) => assert_eq!(s, "foo"),
            other => {
                assert!(false, "expected lowercased Keyword, got {other:?}");
            }
        }
        assert_eq!(q.options.case, Some(LqCase::Insensitive));
    }

    #[test]
    fn not_not_collapses() {
        // We can't write `NOT NOT x` directly through the parser (parser
        // rejects double NOT atom-position), so synthesize the tree.
        let inner = LqExpr::Leaf(LqLeaf::Keyword("x".to_owned()));
        let double_not = LqExpr::Not(Box::new(LqExpr::Not(Box::new(inner.clone()))));
        let q = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: double_not,
            filters: Vec::new(),
            directives: Vec::new(),
            options: LqOptions::defaults(),
            source_span: crate::errors::LqSpan::new(0, 0),
        };
        let out = match normalize(q) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "normalize failed: {e}");
                return;
            }
        };
        assert_eq!(out.expr, inner);
    }

    #[test]
    fn dedup_identical_or_children() {
        let q = run("foo OR foo OR bar");
        match q.expr {
            LqExpr::Any(children) => {
                assert_eq!(children.len(), 2);
            }
            other => {
                assert!(false, "expected Any, got {other:?}");
            }
        }
    }

    #[test]
    fn double_normalize_is_byte_identical() {
        let cases = [
            "foo",
            "foo bar",
            "foo OR bar",
            "(panic OR unwrap) lang:rust",
            "repo:foo@main file:lib path:src tokio",
            "Iterator -dyn",
            "Foo case:no",
        ];
        for s in cases {
            let q1 = run(s);
            let q2 = match normalize(q1.clone()) {
                Ok(q) => q,
                Err(e) => {
                    assert!(false, "second normalize failed on {s:?}: {e}");
                    continue;
                }
            };
            assert_eq!(q1, q2, "idempotency failed on {s:?}");
        }
    }
}
