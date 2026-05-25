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

use quanta_index_lq_bridge::{
    BridgeCandidate, BridgeErrorCode, LqDirective, SourcegraphVersionTag, TRANSLATOR_VERSION,
    parse_sourcegraph, translate,
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
    let lq = match translate(q, &ver()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let LqDirective::Filtered { filters, body } = lq else {
        assert!(false, "expected Filtered");
        return;
    };
    assert_eq!(filters.len(), 1);
    let Some(LqDirective::Filter { name, value }) = filters.first() else {
        assert!(false, "expected Filter");
        return;
    };
    assert_eq!(&**name, "repo");
    assert_eq!(&**value, "acme/foo");
    let LqDirective::Pattern { kind, body } = *body else {
        assert!(false, "expected Pattern body");
        return;
    };
    assert_eq!(&*kind, "literal");
    assert_eq!(&*body, "bar");
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
    let lq = match translate(q, &ver()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let LqDirective::Filtered { filters, .. } = lq else {
        assert!(false, "expected Filtered");
        return;
    };
    let Some(LqDirective::Filter { name, value }) = filters.first() else {
        assert!(false, "expected Filter");
        return;
    };
    assert_eq!(&**name, "fork");
    assert_eq!(&**value, "no");
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
    let lq = match translate(q, &ver()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let LqDirective::Pattern { kind, body } = lq else {
        assert!(false, "expected Pattern");
        return;
    };
    assert_eq!(&*kind, "literal");
    assert_eq!(&*body, "hello");
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
    match translate(q, &ver()) {
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
    let lq = match translate(q, &ver()) {
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
    let lq = match translate(q, &ver()) {
        Ok(d) => d,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let LqDirective::Predicate { name, args_raw } = lq else {
        assert!(false, "expected Predicate");
        return;
    };
    assert_eq!(&*name, "repo.has.file");
    assert_eq!(&*args_raw, "path:src/lib.rs");
}
