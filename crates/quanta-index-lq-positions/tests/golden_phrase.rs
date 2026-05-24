//! Golden phrase / adjacency corpus — hand-pinned.
//!
//! 4-document corpus, 5 query cases. Expected `(DocId, start, end)` triples
//! are pinned; any change to the phrase intersect algorithm, adjacency
//! window semantics, or sort order will break the golden and force a
//! deliberate update.

use quanta_index_lq_positions::{
    AdjacencyConfig, DocId, NormalizerVersion, PhraseMatch, Position, PositionsBuilder,
    PositionsIndex, query_adjacency, query_phrase,
};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn build_corpus() -> PositionsIndex {
    // Doc 100: "fn handle request response handle"
    // Doc 101: "fn parse stream handle reply"
    // Doc 102: "handle fn request"               (order broken)
    // Doc 103: "fn handle fn handle fn handle"   (repeated pair)
    let docs: &[(DocId, &[&str])] = &[
        (
            DocId(100),
            &["fn", "handle", "request", "response", "handle"],
        ),
        (DocId(101), &["fn", "parse", "stream", "handle", "reply"]),
        (DocId(102), &["handle", "fn", "request"]),
        (
            DocId(103),
            &["fn", "handle", "fn", "handle", "fn", "handle"],
        ),
    ];

    let mut b = PositionsBuilder::new(7, NormalizerVersion::new(1, 0));
    for (doc_id, tokens) in docs {
        for (idx, t) in tokens.iter().enumerate() {
            let Ok(p) = u32::try_from(idx) else {
                fatal("position overflow in fixture");
            };
            b.add_token(*doc_id, t, Position(p));
        }
    }
    match b.finish() {
        Ok(idx) => idx,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn sorted_spans(matches: &[PhraseMatch]) -> Vec<(u64, u32, u32)> {
    let mut v: Vec<(u64, u32, u32)> = matches
        .iter()
        .map(|m| (m.doc_id.0, m.start_position.0, m.end_position.0))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn golden_phrase_fn_handle() {
    let idx = build_corpus();
    let r = match query_phrase(&idx, &["fn", "handle"]) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // doc 100: anchor 0 (fn@0, handle@1)
    // doc 101: no contiguous fn handle (fn@0, handle@3)
    // doc 102: no contiguous fn handle (handle@0, fn@1)
    // doc 103: anchors 0, 2, 4
    assert_eq!(
        sorted_spans(&r.matches),
        vec![(100, 0, 1), (103, 0, 1), (103, 2, 3), (103, 4, 5),]
    );
}

#[test]
fn golden_phrase_handle_request() {
    let idx = build_corpus();
    let r = match query_phrase(&idx, &["handle", "request"]) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // No doc has handle immediately followed by request.
    // doc 100: handle@1,4; request@2 -> handle@1 followed by request@2 -> hit
    // doc 102: handle@0; request@2  -> not contiguous
    assert_eq!(sorted_spans(&r.matches), vec![(100, 1, 2)]);
}

#[test]
fn golden_phrase_three_term() {
    let idx = build_corpus();
    let r = match query_phrase(&idx, &["fn", "handle", "request"]) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // doc 100: fn@0, handle@1, request@2 -> hit
    // doc 103: fn@0, handle@1, request@? -> no request in 103
    assert_eq!(sorted_spans(&r.matches), vec![(100, 0, 2)]);
}

#[test]
fn golden_phrase_no_match() {
    let idx = build_corpus();
    let r = match query_phrase(&idx, &["zzz", "qqq"]) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert!(r.matches.is_empty());
}

#[test]
fn golden_adjacency_default_window() {
    let idx = build_corpus();
    let cfg = AdjacencyConfig::default_window();
    let r = match query_adjacency(&idx, "fn", "handle", &cfg) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // Pairs within window 8, distances:
    //   doc 100: fn@0 vs handle@1,4 -> dist 1, 4 (both <=8)
    //   doc 101: fn@0 vs handle@3 -> dist 3 (<=8)
    //   doc 102: fn@1 vs handle@0 -> dist 1 (<=8)
    //   doc 103: fn@0,2,4 vs handle@1,3,5 -> 9 pairs all <=5
    // Spans use min/max ordering.
    let spans = sorted_spans(&r.matches);
    // Count must be 2 + 1 + 1 + 9 = 13.
    assert_eq!(spans.len(), 13);
    // Spot-check the doc-100 entries.
    assert!(spans.contains(&(100, 0, 1)));
    assert!(spans.contains(&(100, 0, 4)));
    // Spot-check the doc-102 entry — symmetric (handle before fn).
    assert!(spans.contains(&(102, 0, 1)));
}

#[test]
fn golden_adjacency_tight_window_drops_far_pairs() {
    let idx = build_corpus();
    let Some(cfg) = AdjacencyConfig::new(2) else {
        fatal("AdjacencyConfig::new(2)");
    };
    let r = match query_adjacency(&idx, "fn", "handle", &cfg) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let spans = sorted_spans(&r.matches);
    // Distances <=2 only:
    //   doc 100: fn@0 / handle@1 -> 1
    //   doc 101: fn@0 / handle@3 -> 3 (dropped)
    //   doc 102: fn@1 / handle@0 -> 1
    //   doc 103: fn@0/h@1=1; fn@2/h@1=1; fn@2/h@3=1; fn@4/h@3=1; fn@4/h@5=1; fn@0/h@3=3 dropped; fn@2/h@5=3 dropped
    //           => 5 hits in doc 103
    assert_eq!(spans.len(), 1 + 1 + 5);
    assert!(spans.contains(&(100, 0, 1)));
    assert!(spans.contains(&(102, 0, 1)));
}
