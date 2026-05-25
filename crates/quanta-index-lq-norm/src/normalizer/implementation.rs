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
    // First pass: scan Regex leaves for `(?i)` prefix canonicalization and
    // mid-pattern flag rejection. Walks expr + filter-embedded leaves.
    let mut saw_leading_i = false;
    canonicalize_regex_inline_flags_expr(&mut q.expr, &mut saw_leading_i)?;
    for f in &mut q.filters {
        if let LqFilter::Content { leaf } = f {
            canonicalize_regex_inline_flags_leaf(leaf, &mut saw_leading_i)?;
        }
    }
    if saw_leading_i {
        q.options.case = Some(LqCase::Insensitive);
    }
    q.expr = normalize_expr(q.expr, q.options.case)?;
    sort_filters(&mut q.filters);
    sort_directives(&mut q.directives);
    Ok(q)
}

/// Walk `expr` and canonicalize regex inline-flag prefixes per dsl.md §6.2.
///
/// - `(?i)BODY` → `BODY`, and `saw_leading_i` set to `true` for the whole
///   query (the option carrier is query-scoped, not per-leaf).
/// - any inline flag group not at byte position 0 → `ForbiddenSyntax`
/// - other prefix flags `(?m)`, `(?s)`, `(?x)` are preserved verbatim
///   (deferred to v2 per ticket §12).
///
/// Idempotent: stripping a regex twice produces the same result.
fn canonicalize_regex_inline_flags_expr(
    expr: &mut LqExpr,
    saw_leading_i: &mut bool,
) -> Result<(), LqParseError> {
    match expr {
        LqExpr::Empty | LqExpr::SemanticVector { .. } => Ok(()),
        LqExpr::Leaf(l) => canonicalize_regex_inline_flags_leaf(l, saw_leading_i),
        LqExpr::Not(inner) => canonicalize_regex_inline_flags_expr(inner, saw_leading_i),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for c in children {
                canonicalize_regex_inline_flags_expr(c, saw_leading_i)?;
            }
            Ok(())
        }
    }
}

fn canonicalize_regex_inline_flags_leaf(
    leaf: &mut LqLeaf,
    saw_leading_i: &mut bool,
) -> Result<(), LqParseError> {
    let s = match leaf {
        LqLeaf::Regex(s) => s,
        LqLeaf::Keyword(_)
        | LqLeaf::Phrase(_)
        | LqLeaf::RawString(_)
        | LqLeaf::StructuralBlock(_)
        | LqLeaf::Predicate { .. } => return Ok(()),
    };
    let stripped = strip_leading_case_insensitive(s, saw_leading_i);
    reject_mid_pattern_inline_flags(&stripped)?;
    *s = stripped;
    Ok(())
}

/// Try to strip a leading `(?i)` from the regex source.
///
/// Returns the (possibly identical) stripped string. Sets `saw_leading_i`
/// to true if a `(?i)` prefix was found and removed. Idempotent: the
/// post-strip string never starts with `(?i)` again.
///
/// Only the pure case-insensitive form `(?i)` is stripped. Combined or
/// alternative-character flag groups like `(?im)`, `(?is)`, or `(?-i)`
/// are not stripped here; they fall through to
/// `reject_mid_pattern_inline_flags` if they appear elsewhere and to
/// the regex compiler otherwise. PRE-NORM v1 only canonicalizes the
/// most common form per dsl.md §6.2.
fn strip_leading_case_insensitive(src: &str, saw_leading_i: &mut bool) -> String {
    src.strip_prefix("(?i)").map_or_else(
        || src.to_owned(),
        |rest| {
            *saw_leading_i = true;
            rest.to_owned()
        },
    )
}

/// Scan the regex body and surface `ForbiddenSyntax` on any inline-flag
/// group `(?<flags>)` or `(?<flags>:` occurrence.
///
/// The caller has already stripped a leading `(?i)`; any remaining inline
/// flag group is by definition mid-pattern.
fn reject_mid_pattern_inline_flags(src: &str) -> Result<(), LqParseError> {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(&b) = bytes.get(i) else { break };
        if b == b'\\' {
            // Skip the next byte (escape).
            let Some(next) = i.checked_add(2) else { break };
            i = next;
            continue;
        }
        if b == b'(' {
            let Some(after) = i.checked_add(1) else { break };
            if bytes.get(after) == Some(&b'?') {
                // Scan flag characters [imsxU-]+ and look for terminating `)` or `:`.
                let Some(mut j) = after.checked_add(1) else {
                    break;
                };
                let flag_start = j;
                while let Some(&c) = bytes.get(j) {
                    if matches!(c, b'i' | b'm' | b's' | b'x' | b'U' | b'R' | b'-') {
                        let Some(next) = j.checked_add(1) else { break };
                        j = next;
                    } else {
                        break;
                    }
                }
                if j > flag_start {
                    let term = bytes.get(j).copied();
                    if matches!(term, Some(b')' | b':')) {
                        return Err(LqParseError::new(
                            crate::errors::LqParseErrorCode::ForbiddenSyntax,
                            crate::errors::LqSpan::synthetic(0),
                            "mid-pattern inline regex flag is forbidden",
                        ));
                    }
                }
            }
        }
        let Some(next) = i.checked_add(1) else { break };
        i = next;
    }
    Ok(())
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
        // SemanticVector is a programmatic-only leaf — no DSL string form in
        // v1 (dsl.md), case-folding is meaningless on raw embeddings, and the
        // vector content is hash-bearing. Pass through unchanged.
        sv @ LqExpr::SemanticVector { .. } => Ok(sv),
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
            | LqExpr::Not(_)
            | LqExpr::SemanticVector { .. }) => flat.push(kept),
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
        // Structural patterns and predicates are not case-folded either
        // (per dsl.md §8 / predicate semantics are planner-scope).
        LqLeaf::RawString(s) => LqLeaf::RawString(s),
        LqLeaf::Regex(s) => LqLeaf::Regex(s),
        LqLeaf::StructuralBlock(b) => LqLeaf::StructuralBlock(b),
        LqLeaf::Predicate { name, args } => LqLeaf::Predicate { name, args },
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
        LqExpr::SemanticVector { vector_ref, top_k } => {
            out.push_str("SV(");
            write_semantic_vector_ref_key(out, vector_ref);
            // Display-stable form; canonical_key is engineering-only.
            let s = format!(",k={top_k}");
            out.push_str(&s);
            out.push(')');
        }
    }
}

fn write_semantic_vector_ref_key(out: &mut String, vr: &crate::ast::SemanticVectorRef) {
    match vr {
        crate::ast::SemanticVectorRef::Inline(vector) => {
            out.push_str("IN[");
            let len = format!("{}", vector.len());
            out.push_str(&len);
            out.push(';');
            for (i, x) in vector.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                // Use bit-pattern hex for byte-stable ordering; canonical_key
                // is engineering-only (not on the wire or in the hash).
                let bits = format!("{:08x}", x.to_bits());
                out.push_str(&bits);
            }
            out.push(']');
        }
        crate::ast::SemanticVectorRef::Handle(h) => {
            out.push_str("HA:");
            out.push_str(h);
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
        LqLeaf::StructuralBlock(block) => {
            out.push_str("S:");
            write_structural_block_key(out, block);
        }
        LqLeaf::Predicate { name, args } => {
            out.push_str("PRED:");
            out.push_str(name);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_predicate_arg_key(out, a);
            }
            out.push(')');
        }
    }
}

fn write_structural_block_key(out: &mut String, block: &crate::ast::LqStructuralBlock) {
    out.push('[');
    match &block.lang {
        Some(l) => {
            out.push_str("lang=");
            out.push_str(l);
        }
        None => out.push_str("lang=_"),
    }
    out.push(';');
    for (i, node) in block.nodes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_structural_node_key(out, node);
    }
    out.push(']');
}

fn write_structural_node_key(out: &mut String, node: &crate::ast::LqStructuralNode) {
    match node {
        crate::ast::LqStructuralNode::Literal(s) => {
            out.push_str("LIT:");
            out.push_str(s);
        }
        crate::ast::LqStructuralNode::MetaVar(mv) => {
            out.push_str("MV:");
            out.push_str(mv.as_str());
        }
        crate::ast::LqStructuralNode::Group(children) => {
            out.push_str("G(");
            for (i, c) in children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_structural_node_key(out, c);
            }
            out.push(')');
        }
    }
}

fn write_predicate_arg_key(out: &mut String, arg: &crate::ast::LqPredicateArg) {
    match arg {
        crate::ast::LqPredicateArg::Keyword(s) => {
            out.push_str("K:");
            out.push_str(s);
        }
        crate::ast::LqPredicateArg::Phrase(s) => {
            out.push_str("P:");
            out.push_str(s);
        }
        crate::ast::LqPredicateArg::RawString(s) => {
            out.push_str("R:");
            out.push_str(s);
        }
        crate::ast::LqPredicateArg::Number(n) => {
            out.push_str("N:");
            // Use a Display-stable form; canonical_key is engineering-only.
            let s = format!("{n}");
            out.push_str(&s);
        }
        crate::ast::LqPredicateArg::Filter { name, value } => {
            out.push_str("F:");
            out.push_str(name);
            out.push(':');
            out.push_str(value);
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
            "/(?i)hello/",
            "/foo/",
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

    // ---- Step 4: `(?i)` inline-flag stripping tests ----

    #[test]
    fn regex_leading_case_insensitive_flag_is_stripped() {
        let q = run("/(?i)hello/");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Regex(s)) => assert_eq!(s, "hello"),
            other => {
                assert!(false, "expected Regex leaf, got {other:?}");
            }
        }
        assert_eq!(q.options.case, Some(LqCase::Insensitive));
    }

    #[test]
    fn regex_without_inline_flag_is_unchanged() {
        let q = run("/hello/");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Regex(s)) => assert_eq!(s, "hello"),
            other => {
                assert!(false, "expected Regex leaf, got {other:?}");
            }
        }
        assert_eq!(q.options.case, None);
    }

    #[test]
    fn regex_inline_flag_strip_is_idempotent() {
        let q1 = run("/(?i)hello/");
        let q2 = match normalize(q1.clone()) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "second normalize failed: {e}");
                return;
            }
        };
        assert_eq!(q1, q2);
        // Triple-normalize for extra safety.
        let q3 = match normalize(q2.clone()) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "third normalize failed: {e}");
                return;
            }
        };
        assert_eq!(q2, q3);
    }

    #[test]
    fn regex_mid_pattern_inline_flag_is_forbidden() {
        let toks = match tokenize("/foo(?i)bar/") {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "tokenize failed: {e}");
                return;
            }
        };
        let q = match parse(&toks, "/foo(?i)bar/") {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed: {e}");
                return;
            }
        };
        match normalize(q) {
            Ok(_) => assert!(false, "expected ForbiddenSyntax"),
            Err(e) => assert_eq!(
                e.code,
                crate::errors::LqParseErrorCode::ForbiddenSyntax,
                "got {e}"
            ),
        }
    }

    // ---- Step (round-7): SemanticVector pass-through tests ----

    #[test]
    fn semantic_vector_inline_passes_through_normalizer_unchanged() {
        let sv = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Inline(vec![0.1, 0.2, 0.3]),
            top_k: 16,
        };
        let q = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: sv.clone(),
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
        assert_eq!(out.expr, sv);
    }

    #[test]
    fn semantic_vector_handle_passes_through_normalizer_unchanged() {
        let sv = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Handle("vec-h-1".into()),
            top_k: 5,
        };
        let q = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: sv.clone(),
            filters: Vec::new(),
            directives: Vec::new(),
            options: LqOptions {
                pattern_type: crate::ast::LqPatternType::Standard,
                case: Some(LqCase::Insensitive),
                count: None,
            },
            source_span: crate::errors::LqSpan::new(0, 0),
        };
        // Even with case:no, the semantic-vector leaf must not be folded.
        let out = match normalize(q) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "normalize failed: {e}");
                return;
            }
        };
        assert_eq!(out.expr, sv);
    }

    #[test]
    fn semantic_vector_inside_all_is_preserved_and_flattening_skips_it() {
        let sv = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Handle("h2".into()),
            top_k: 8,
        };
        // AND[ All[Leaf(foo), Leaf(bar)], SV ] should flatten the inner All but
        // leave SV as a sibling.
        let inner_all = LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Keyword("foo".to_owned())),
            LqExpr::Leaf(LqLeaf::Keyword("bar".to_owned())),
        ]);
        let q = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: LqExpr::All(vec![inner_all, sv.clone()]),
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
        match out.expr {
            LqExpr::All(children) => {
                assert_eq!(children.len(), 3, "expected 3 children, got {children:?}");
                assert!(
                    children.contains(&sv),
                    "SV leaf must survive normalize: {children:?}"
                );
            }
            other => assert!(false, "expected All, got {other:?}"),
        }
    }

    #[test]
    fn semantic_vector_handle_cbor_canonical_hash_is_deterministic() {
        let sv1 = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Handle("h-stable".into()),
            top_k: 4,
        };
        let q1 = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: sv1,
            filters: Vec::new(),
            directives: Vec::new(),
            options: LqOptions::defaults(),
            source_span: crate::errors::LqSpan::new(0, 0),
        };
        let h1 = match crate::hasher::canonical_hash(&q1) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "hash1 failed: {e}");
                return;
            }
        };
        let h2 = match crate::hasher::canonical_hash(&q1) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "hash2 failed: {e}");
                return;
            }
        };
        assert_eq!(h1, h2);

        // Distinct handle ⇒ distinct hash.
        let sv2 = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Handle("h-other".into()),
            top_k: 4,
        };
        let q2 = LqNormalizedQuery {
            lq_version: crate::ast::LQ_VERSION_TAG,
            expr: sv2,
            ..q1
        };
        let h3 = match crate::hasher::canonical_hash(&q2) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "hash3 failed: {e}");
                return;
            }
        };
        assert_ne!(h1, h3);
    }

    #[test]
    fn semantic_vector_cbor_roundtrip_inline() {
        let sv = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Inline(vec![1.0_f32, 2.0, -3.5]),
            top_k: 5,
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&sv, &mut buf) {
            assert!(false, "encode failed: {e}");
            return;
        }
        let back: LqExpr = match ciborium::de::from_reader(buf.as_slice()) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "decode failed: {e}");
                return;
            }
        };
        assert_eq!(back, sv);
    }

    #[test]
    fn semantic_vector_cbor_roundtrip_handle() {
        let sv = LqExpr::SemanticVector {
            vector_ref: crate::ast::SemanticVectorRef::Handle("h1".into()),
            top_k: 10,
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&sv, &mut buf) {
            assert!(false, "encode failed: {e}");
            return;
        }
        let back: LqExpr = match ciborium::de::from_reader(buf.as_slice()) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "decode failed: {e}");
                return;
            }
        };
        assert_eq!(back, sv);
    }
}
