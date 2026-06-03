//! Golden corpus — Sourcegraph syntax → expected LQ shape (or typed reject).
//!
//! Each row is one [BRIDGE-01 § 6.1](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md)
//! subset-table outcome. Rows 1–8 are the original bridge subset; rows 9–19
//! cover JFC-05 widened history/runtime filter lowering (translator-only proof;
//! executable SG/native parity lives in `e2e_dual_syntax_lowering_parity`).

use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqPredicateArg, LqType};
use quanta_index_lq_bridge::{
    BridgeCandidate, BridgeErrorCode, SourcegraphVersionTag, TRANSLATOR_VERSION, parse_sourcegraph,
    translate_query,
};

fn filters_for(raw: &str) -> Vec<LqFilter> {
    let q = match parse_sourcegraph(raw) {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return Vec::new();
        }
    };
    match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d.filters,
        Err(e) => {
            assert!(false, "{e}");
            Vec::new()
        }
    }
}

fn assert_filter_present<F>(raw: &str, label: &str, predicate: F)
where
    F: Fn(&LqFilter) -> bool,
{
    let filters = filters_for(raw);
    assert!(
        filters.iter().any(|filter| predicate(filter)),
        "expected {label} on `{raw}`, got {:?}",
        filters
    );
}

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

#[test]
fn row1_adopted_repo_filter() {
    let q = match parse_sourcegraph("repo:acme/foo bar") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "repo:acme/foo bar";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("bar".to_string())));
    assert_eq!(
        lq.filters,
        vec![LqFilter::Repo {
            pattern: "acme/foo".to_string(),
            revs: Vec::new(),
        }]
    );
}

#[test]
fn row2_adopted_fork_no_filter() {
    let q = match parse_sourcegraph("fork:no needle") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "fork:no needle";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
    assert_eq!(
        lq.filters,
        vec![LqFilter::Fork {
            mode: quanta_index_contract::LqYesNoOnly::No,
        }]
    );
}

#[test]
fn row3_normalized_content_to_pattern_leaf() {
    let q = match parse_sourcegraph("content:hello") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "content:hello";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("hello".to_string())));
}

#[test]
fn row4_refused_index_no_directive() {
    let q = match parse_sourcegraph("index:no foo") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    match translate_query(q, &ver(), "index:no foo".len()) {
        Ok(_) => assert!(false, "index:no must refuse"),
        Err(e) => {
            assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective);
            match e.source_construct.as_deref() {
                Some(s) => assert_eq!(s, "index:no"),
                None => assert!(false, "expected source_construct"),
            }
        }
    }
}

#[test]
fn row5_refused_unregistered_filter_name() {
    match parse_sourcegraph("colorscheme:dark foo") {
        Ok(_) => assert!(false, "unknown filter must refuse"),
        Err(e) => {
            assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedFilter);
            match e.source_construct.as_deref() {
                Some(s) => assert_eq!(s, "colorscheme"),
                None => assert!(false, "expected source_construct"),
            }
        }
    }
}

#[test]
fn candidate_envelope_stamps_translator_version() {
    let q = match parse_sourcegraph("lang:rust foo") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let lq = match translate_query(q, &ver(), "lang:rust foo".len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let c = BridgeCandidate::new("lang:rust foo", lq);
    assert_eq!(c.translator_version.as_ref(), TRANSLATOR_VERSION);
    assert_eq!(c.source_syntax.as_ref(), "lang:rust foo");
}

#[test]
fn row7_adopted_history_before_filter() {
    let raw = "type:commit before:1970-01-01T00:00:00.020Z needle";
    let q = match parse_sourcegraph(raw) {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert!(
        lq.filters.iter().any(|filter| matches!(
            filter,
            LqFilter::Before { timeref } if timeref == "1970-01-01T00:00:00.020Z"
        )),
        "expected before: filter, got {:?}",
        lq.filters
    );
    assert!(
        lq.filters.iter().any(|filter| matches!(
            filter,
            LqFilter::Type {
                kind: LqType::Commit
            }
        )),
        "expected type:commit filter, got {:?}",
        lq.filters
    );
}

#[test]
fn row8_adopted_diff_added_filter() {
    let raw = "type:diff diff.added:unwrap needle";
    let q = match parse_sourcegraph(raw) {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert!(
        lq.filters.iter().any(|filter| matches!(
            filter,
            LqFilter::DiffAdded { pattern } if pattern == "unwrap"
        )),
        "expected diff.added filter, got {:?}",
        lq.filters
    );
}

#[test]
fn row6_repo_predicate_lowers_to_predicate_placeholder() {
    let q = match parse_sourcegraph("repo:has.file(path:src/lib.rs)") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "repo:has.file(path:src/lib.rs)";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(
        lq.expr,
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }],
        })
    );
}

#[test]
fn row6b_repo_has_path_alias_lowers_to_repo_has_file_path_filter() {
    let q = match parse_sourcegraph("repo:has.path(src/lib.rs)") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "repo:has.path(src/lib.rs)";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(
        lq.expr,
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }],
        })
    );
}

#[test]
fn row6c_repo_has_content_lowers_to_predicate_placeholder() {
    let q = match parse_sourcegraph("repo:has.content(corp-a)") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "repo:has.content(corp-a)";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(
        lq.expr,
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Keyword("corp-a".to_string())],
        })
    );
}

#[test]
fn row6d_file_contains_content_alias_lowers_to_content_leaf() {
    let q = match parse_sourcegraph("file:contains.content(\"lemon yellow banana\")") {
        Ok(q) => q,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let raw = "file:contains.content(\"lemon yellow banana\")";
    let lq = match translate_query(q, &ver(), raw.len()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(
        lq.expr,
        LqExpr::Leaf(LqLeaf::Phrase("lemon yellow banana".to_string()))
    );
}

#[test]
fn row9_adopted_history_since_filter() {
    assert_filter_present(
        "type:commit since:1970-01-01T00:00:00.022Z needle",
        "since:",
        |filter| matches!(filter, LqFilter::Since { timeref } if timeref == "1970-01-01T00:00:00.022Z"),
    );
}

#[test]
fn row10_adopted_history_after_filter() {
    assert_filter_present(
        "type:commit after:1970-01-01T00:00:00.015Z needle",
        "after:",
        |filter| matches!(filter, LqFilter::After { timeref } if timeref == "1970-01-01T00:00:00.015Z"),
    );
}

#[test]
fn row11_adopted_history_until_filter() {
    assert_filter_present(
        "type:commit until:1970-01-01T00:00:00.012Z needle",
        "until:",
        |filter| matches!(filter, LqFilter::Until { timeref } if timeref == "1970-01-01T00:00:00.012Z"),
    );
}

#[test]
fn row12_adopted_diff_removed_filter() {
    assert_filter_present(
        "type:diff diff.removed:parity_removed_marker needle",
        "diff.removed:",
        |filter| matches!(filter, LqFilter::DiffRemoved { pattern } if pattern == "parity_removed_marker"),
    );
}

#[test]
fn row13_adopted_diff_touched_filter() {
    assert_filter_present(
        "type:diff diff.touched:parity_touched_marker needle",
        "diff.touched:",
        |filter| matches!(filter, LqFilter::DiffTouched { pattern } if pattern == "parity_touched_marker"),
    );
}

#[test]
fn row14_adopted_runtime_changed_filter() {
    assert_filter_present(
        "changed:since=1970-01-01T00:00:00.010Z needle",
        "changed:",
        |filter| matches!(filter, LqFilter::Changed { scope } if scope == "since=1970-01-01T00:00:00.010Z"),
    );
}

#[test]
fn row15_adopted_runtime_stale_filter() {
    assert_filter_present(
        "stale:before=1970-01-01T00:00:00.030Z needle",
        "stale:",
        |filter| matches!(filter, LqFilter::Stale { scope } if scope == "before=1970-01-01T00:00:00.030Z"),
    );
}

#[test]
fn row16_adopted_runtime_snapshot_filter() {
    assert_filter_present(
        "snapshot:active needle",
        "snapshot:",
        |filter| matches!(filter, LqFilter::Snapshot { name } if name == "active"),
    );
}

#[test]
fn row17_adopted_runtime_meta_service_filter() {
    assert_filter_present(
        "meta.service:search needle",
        "meta.service:",
        |filter| matches!(filter, LqFilter::MetaService { id } if id == "search"),
    );
}

#[test]
fn row18_adopted_runtime_meta_layer_filter() {
    assert_filter_present(
        "meta.layer:index needle",
        "meta.layer:",
        |filter| matches!(filter, LqFilter::MetaLayer { id } if id == "index"),
    );
}

#[test]
fn row19_adopted_runtime_meta_surface_filter() {
    assert_filter_present(
        "meta.surface:lexical needle",
        "meta.surface:",
        |filter| matches!(filter, LqFilter::MetaSurface { id } if id == "lexical"),
    );
}

#[test]
fn row20_adopted_runtime_invalidated_by_filter() {
    assert_filter_present(
        "invalidated_by:rebuild=lexical needle",
        "invalidated_by:",
        |filter| {
            matches!(
                filter,
                LqFilter::InvalidatedBy { source } if source == "rebuild=lexical"
            )
        },
    );
}
