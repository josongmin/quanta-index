//! Golden corpus — 5 Sourcegraph syntax → expected LQ shape (or
//! expected typed reject).
//!
//! Each row is one [BRIDGE-01 § 6.1](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md)
//! subset-table outcome. Reading the rows in source order:
//!
//! 1. `repo:` filter — **adopted** (1:1 LQ `repo:`).
//! 2. `fork:no` filter — **adopted** as LQ `fork:no`.
//! 3. `content:hello` filter — **normalized** to LQ `Pattern{literal}`.
//! 4. `index:no` directive — **refused** with `BRIDGE_UNSUPPORTED_DIRECTIVE`.
//! 5. `colorscheme:dark` (post-pin Sourcegraph-future filter we have
//!    not registered) — **refused** with `BRIDGE_UNSUPPORTED_FILTER`.
//! 6. `repo:has.file(path:src/lib.rs)` predicate — **adopted** as active
//!    LQ predicate placeholder.

use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqPredicateArg};
use quanta_index_lq_bridge::{
    BridgeCandidate, BridgeErrorCode, SourcegraphVersionTag, TRANSLATOR_VERSION, parse_sourcegraph,
    translate_query,
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
