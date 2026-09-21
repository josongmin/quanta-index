use quanta_index_contract::{INTERNAL_FETCH_CEILING, PUBLIC_TOP_K_MAX};
use quanta_index_core::{CoreError, LexicalSearchPageV1};

use crate::query_dispatcher::tests::support::common::{build_probe_query, candidate};
use crate::query_dispatcher::window::{
    finalize_probe_window_v1, fused_window_v1, lexical_fetch_limit_v1, lexical_page_window_v1,
    probe_top_k_v1,
};

#[test]
fn query_window_uses_one_continuation_row_and_never_requires_full_count_v1() {
    use quanta_index_contract::{CandidateCountV1, QueryResultWindowV1};

    let mut exact = vec![1_u8, 2];
    assert_eq!(
        finalize_probe_window_v1(&mut exact, 3).expect("valid exact window"),
        QueryResultWindowV1::exact(2)
    );
    let mut continued = vec![1_u8, 2, 3, 4];
    let window = finalize_probe_window_v1(&mut continued, 3).expect("valid lower bound");
    assert_eq!(continued, vec![1, 2, 3]);
    assert_eq!(window.returned(), 3);
    assert_eq!(window.candidate_count(), CandidateCountV1::AtLeast(4));
    assert!(window.has_more());
    let fused = fused_window_v1(100, 100, 100, true).expect("capped lane is a lower bound");
    assert_eq!(fused.candidate_count(), CandidateCountV1::AtLeast(101));
    assert!(fused.has_more());
    assert!(fused_window_v1(100, 99, 99, true).is_err());
    assert_eq!(probe_top_k_v1(9_999).expect("one-row probe within ceiling"), 10_000);
    // The public maximum is accepted and probes one row past it; the
    // internal fetch ceiling is the contract's, not the caller's.
    assert_eq!(
        probe_top_k_v1(PUBLIC_TOP_K_MAX).expect("public maximum is accepted"),
        INTERNAL_FETCH_CEILING
    );
    for refused in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        match probe_top_k_v1(refused) {
            Err(CoreError::Typed { code, .. }) => assert_eq!(
                code,
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::QueryTopKOutOfRange,
                ),
            ),
            other => {
                panic!("top_k={refused} must be refused with the shared code, got {other:?}")
            }
        }
    }
}

#[test]
fn count_options_take_an_exact_window_from_the_adapter_and_never_widen_the_page_v1() {
    use quanta_index_contract::{CandidateCountV1, LqCountBound};

    let mut query = build_probe_query("needle");
    query.options.count = Some(LqCountBound::All);
    assert_eq!(
        lexical_fetch_limit_v1(&query, 1).expect("count:all fetch limit"),
        1,
        "an exact total makes the continuation probe unnecessary"
    );
    query.options.count = None;
    assert_eq!(
        lexical_fetch_limit_v1(&query, 1).expect("plain fetch limit"),
        2,
        "without a count the page carries one probe row"
    );

    // The adapter proved three matches but the page is one row.
    let mut page = LexicalSearchPageV1 {
        candidates: vec![candidate("alpha", 1.0)],
        exact_total: Some(3),
    };
    let window = lexical_page_window_v1(&mut page, 1, 1).expect("exact window");
    assert_eq!(window.returned(), 1);
    assert_eq!(window.candidate_count(), CandidateCountV1::Exact(3));
    assert!(window.has_more());

    // A projection fetched with a probe row still cuts to the page and
    // keeps the exact total.
    let mut projected = LexicalSearchPageV1 {
        candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
        exact_total: Some(5),
    };
    let window = lexical_page_window_v1(&mut projected, 1, 2).expect("projected window");
    assert_eq!(projected.candidates.len(), 1);
    assert_eq!(window.candidate_count(), CandidateCountV1::Exact(5));
    assert!(window.has_more());

    // An adapter that returns more rows than it was asked for is a contract defect.
    let mut oversized = LexicalSearchPageV1 {
        candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
        exact_total: Some(2),
    };
    assert!(lexical_page_window_v1(&mut oversized, 1, 1).is_err());

    // An exact total below the returned rows is a contract defect.
    let mut contradictory = LexicalSearchPageV1 {
        candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
        exact_total: Some(1),
    };
    assert!(lexical_page_window_v1(&mut contradictory, 5, 6).is_err());

    // Without an exact total the probe row is consumed into `has_more`.
    let mut probed = LexicalSearchPageV1 {
        candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
        exact_total: None,
    };
    let window = lexical_page_window_v1(&mut probed, 1, 2).expect("probe window");
    assert_eq!(probed.candidates.len(), 1);
    assert!(window.has_more());
}
