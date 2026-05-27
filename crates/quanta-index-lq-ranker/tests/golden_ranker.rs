//! Golden ordering for the composite ranker.
//!
//! Two weight vectors (`RankerWeights::DEFAULTS` and a manual
//! `(0.5, 0.2, 0.15, 0.1, 0.05)` profile) are evaluated against five
//! hand-tuned candidates. The top-3 `doc_id` sequence is pinned per
//! weight vector so regressions in the composition arithmetic or the
//! tiebreak ordering surface here.

use quanta_index_lq_ranker::{
    CandidateSignals, CompositeScorer, RankerWeights, ScoredCandidate, rank_candidates,
};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn scorer_for(weights: RankerWeights) -> CompositeScorer {
    match CompositeScorer::new(weights) {
        Ok(s) => s,
        Err(e) => fatal(&format!("CompositeScorer::new failed: {e}")),
    }
}

fn manual_weights() -> RankerWeights {
    match RankerWeights::new(0.5, 0.2, 0.15, 0.1, 0.05) {
        Ok(v) => v,
        Err(e) => fatal(&format!("manual weights must validate: {e}")),
    }
}

fn signals(bm25: f32, pp: f32, sb: f32, rec: f32, boost: f32) -> CandidateSignals {
    CandidateSignals {
        bm25,
        path_prior: pp,
        symbol_boost: sb,
        recency: rec,
        boost_directive: boost,
    }
}

fn score(scorer: &CompositeScorer, sig: CandidateSignals) -> f32 {
    match scorer.score(&sig) {
        Ok(v) => v,
        Err(e) => fatal(&format!("score failed for {sig}: {e}")),
    }
}

fn build(scorer: &CompositeScorer, doc_id: u64, sig: CandidateSignals) -> ScoredCandidate {
    ScoredCandidate {
        doc_id,
        repo_id: 1,
        generation: 1,
        repo_relative_path: Box::<str>::from("src/lib.rs"),
        start_line: 1,
        score: score(scorer, sig),
        signals: sig,
    }
}

fn build_set(scorer: &CompositeScorer) -> Vec<ScoredCandidate> {
    vec![
        build(scorer, 100, signals(0.9, 0.5, 0.5, 0.5, 1.0)),
        build(scorer, 200, signals(0.8, 0.4, 0.4, 0.4, 1.0)),
        build(scorer, 300, signals(0.5, 0.9, 0.9, 0.9, 2.0)),
        build(scorer, 400, signals(0.6, 0.3, 0.3, 0.3, 1.0)),
        build(scorer, 500, signals(0.2, 0.2, 0.2, 0.2, 1.0)),
    ]
}

fn ids(v: &[ScoredCandidate]) -> Vec<u64> {
    v.iter().map(|c| c.doc_id).collect()
}

#[test]
fn defaults_pins_top3_to_100_200_300() {
    let scorer = scorer_for(RankerWeights::DEFAULTS);
    let set = build_set(&scorer);
    let got = rank_candidates(set);
    // Hand-computed scores:
    //   doc 100 = 0.7*0.9 + 0.1*0.5 + 0.1*0.5 + 0.05*0.5 + 0     = 0.755
    //   doc 200 = 0.7*0.8 + 0.1*0.4 + 0.1*0.4 + 0.05*0.4 + 0     = 0.660
    //   doc 300 = 0.7*0.5 + 0.1*0.9 + 0.1*0.9 + 0.05*0.9 + 0.05  = 0.625
    //   doc 400 = 0.7*0.6 + 0.1*0.3 + 0.1*0.3 + 0.05*0.3 + 0     = 0.495
    //   doc 500 = 0.7*0.2 + 0.1*0.2 + 0.1*0.2 + 0.05*0.2 + 0     = 0.190
    let top3: Vec<u64> = ids(&got).into_iter().take(3).collect();
    assert_eq!(top3, vec![100, 200, 300]);
}

#[test]
fn manual_weights_pin_top3_to_300_100_200() {
    let scorer = scorer_for(manual_weights());
    let set = build_set(&scorer);
    let got = rank_candidates(set);
    // Hand-computed scores under (0.5, 0.2, 0.15, 0.1, 0.05):
    //   doc 100 = 0.45 + 0.10 + 0.075 + 0.05  + 0     = 0.675
    //   doc 200 = 0.40 + 0.08 + 0.06  + 0.04  + 0     = 0.580
    //   doc 300 = 0.25 + 0.18 + 0.135 + 0.09  + 0.05  = 0.705
    //   doc 400 = 0.30 + 0.06 + 0.045 + 0.03  + 0     = 0.435
    //   doc 500 = 0.10 + 0.04 + 0.030 + 0.02  + 0     = 0.190
    let top3: Vec<u64> = ids(&got).into_iter().take(3).collect();
    assert_eq!(top3, vec![300, 100, 200]);
}

#[test]
fn full_ordering_is_deterministic_under_defaults() {
    let scorer = scorer_for(RankerWeights::DEFAULTS);
    let set = build_set(&scorer);
    let got = rank_candidates(set);
    assert_eq!(ids(&got), vec![100, 200, 300, 400, 500]);
}

#[test]
fn full_ordering_is_deterministic_under_manual_weights() {
    let scorer = scorer_for(manual_weights());
    let set = build_set(&scorer);
    let got = rank_candidates(set);
    assert_eq!(ids(&got), vec![300, 100, 200, 400, 500]);
}
