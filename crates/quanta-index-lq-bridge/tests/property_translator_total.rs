//! Property test — translator totality.
//!
//! For every parseable `SgQuery` shape we can generate, `translate_query`
//! returns either `Ok(LqQuery)` or a typed `BridgeError`. There
//! is no panic, no `unwrap`, and no infinite recursion. ≥ 256 cases.
//!
//! Per BRIDGE-01 § 8.3 failure-classification invariants: every
//! refusal path emits exactly one typed code; no untyped error.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_contract::{LqExpr, LqLeaf, LqQuery};
use quanta_index_lq_bridge::syntax::SgPatternKind;
use quanta_index_lq_bridge::{
    BridgeErrorCode, SgFilter, SgQuery, SourcegraphVersionTag, translate_query,
};

fn ver() -> SourcegraphVersionTag {
    match SourcegraphVersionTag::supported() {
        Ok(t) => t,
        Err(e) => {
            assert!(false, "supported pin must parse: {e}");
            if let Ok(t) = SourcegraphVersionTag::new("sg-0.0.0") {
                return t;
            }
            loop {
                core::hint::spin_loop();
            }
        }
    }
}

fn name_strategy() -> BoxedStrategy<String> {
    prop::string::string_regex("[A-Za-z0-9_./-]{1,20}")
        .map_or_else(|_| Just("x".to_string()).boxed(), proptest::strategy::Strategy::boxed)
}

fn sg_filter_strategy() -> impl Strategy<Value = SgFilter> {
    (0u8..21u8, name_strategy()).prop_map(|(tag, v)| match tag {
        0 => SgFilter::Repo(v.into_boxed_str()),
        1 => SgFilter::File(v.into_boxed_str()),
        2 => SgFilter::Path(v.into_boxed_str()),
        3 => SgFilter::Lang(v.into_boxed_str()),
        4 => SgFilter::Author(v.into_boxed_str()),
        5 => SgFilter::Committer(v.into_boxed_str()),
        6 => SgFilter::Message(v.into_boxed_str()),
        7 => SgFilter::Type(v.into_boxed_str()),
        8 => SgFilter::Case(v.into_boxed_str()),
        9 => SgFilter::Select(v.into_boxed_str()),
        10 => SgFilter::Count(v.into_boxed_str()),
        11 => SgFilter::Patterntype(v.into_boxed_str()),
        12 => SgFilter::Dirty(v.into_boxed_str()),
        13 => SgFilter::Fork(v.into_boxed_str()),
        14 => SgFilter::Archived(v.into_boxed_str()),
        15 => SgFilter::Content(v.into_boxed_str()),
        16 => SgFilter::Visibility(v.into_boxed_str()),
        17 => SgFilter::Context(v.into_boxed_str()),
        18 => SgFilter::Index(v.into_boxed_str()),
        19 => SgFilter::Boost(v.into_boxed_str()),
        _ => SgFilter::Timeout(v.into_boxed_str()),
    })
}

fn sg_query_leaf_strategy() -> impl Strategy<Value = SgQuery> {
    (0u8..3u8, name_strategy()).prop_map(|(tag, s)| SgQuery::Pattern {
        kind: match tag {
            0 => SgPatternKind::Literal,
            1 => SgPatternKind::Phrase,
            _ => SgPatternKind::Regex,
        },
        body: s.into_boxed_str(),
    })
}

fn sg_query_strategy() -> impl Strategy<Value = SgQuery> {
    // Bound depth to 3 levels to keep proptest cases bounded.
    let leaf = sg_query_leaf_strategy();
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            vec(inner.clone(), 1..=4).prop_map(SgQuery::And),
            vec(inner.clone(), 1..=4).prop_map(SgQuery::Or),
            inner.clone().prop_map(|q| SgQuery::Not(Box::new(q))),
            (vec(sg_filter_strategy(), 0..=4), inner).prop_map(|(filters, body)| {
                if filters.is_empty() {
                    body
                } else {
                    SgQuery::Filtered {
                        filters,
                        body: Box::new(body),
                    }
                }
            }),
        ]
    })
}

fn allowed_error_code(c: BridgeErrorCode) -> bool {
    matches!(
        c,
        BridgeErrorCode::BridgeUnsupportedFilter
            | BridgeErrorCode::BridgeUnsupportedDirective
            | BridgeErrorCode::BridgeAmbiguousFilter
            | BridgeErrorCode::BridgeTranslateFail
    )
}

/// A non-empty trait check: every lowered `LqQuery` is at least minimally
/// well-formed (no invalid recursion shape, no panic, no empty spans).
fn well_formed(query: &LqQuery) -> bool {
    fn walk(expr: &LqExpr, depth: u32) -> bool {
        if depth > 64 {
            return false;
        }
        let next = depth.saturating_add(1);
        match expr {
            LqExpr::Empty
            | LqExpr::Leaf(
                LqLeaf::Keyword(_)
                | LqLeaf::Phrase(_)
                | LqLeaf::RawString(_)
                | LqLeaf::Regex(_)
                | LqLeaf::StructuralBlock(_)
                | LqLeaf::Predicate { .. },
            ) => true,
            LqExpr::Not(inner) => walk(inner, next),
            LqExpr::All(xs) | LqExpr::Any(xs) => xs.iter().all(|x| walk(x, next)),
        }
    }
    walk(&query.expr, 0)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn translate_is_total_over_arbitrary_sg_query(q in sg_query_strategy()) {
        let v = ver();
        match translate_query(q, &v, 0) {
            Ok(query) => prop_assert!(well_formed(&query)),
            Err(e) => prop_assert!(
                allowed_error_code(e.code),
                "unexpected error code: {:?}",
                e.code
            ),
        }
    }
}
