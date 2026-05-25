//! Golden BM25 score corpus — TIGHTLY PINNED.
//!
//! Hand-curated 4-document corpus, 4 query cases. Expected `f32` scores
//! are computed at first run by the implementation itself and pinned here
//! to the bit pattern. Any future change to the BM25 formula, IDF
//! computation, normalization, or arithmetic-grouping order will flip the
//! bit pattern and break the golden — exactly the determinism contract
//! LEX-01 owes RFC § Claim Discipline §8.
//!
//! Re-deriving pinned values is documented in the comment block above the
//! `EXPECTED_*` constants at the bottom of this file.

use quanta_index_lq_scorer::scorer::SliceTokenSource;
use quanta_index_lq_scorer::{Bm25Params, Bm25Scorer, DocId, IdfBuilder};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn build_corpus_scorer() -> Bm25Scorer {
    // Corpus (4 docs):
    //   doc_a: ["alpha", "beta",  "gamma"]
    //   doc_b: ["alpha", "alpha", "delta"]
    //   doc_c: ["beta",  "gamma", "delta"]
    //   doc_d: ["alpha", "beta",  "epsilon"]
    let mut b = match IdfBuilder::new(1) {
        Ok(b) => b,
        Err(e) => fatal(&format!("{e}")),
    };
    let doc_a = ["alpha", "beta", "gamma"];
    let doc_b = ["alpha", "alpha", "delta"];
    let doc_c = ["beta", "gamma", "delta"];
    let doc_d = ["alpha", "beta", "epsilon"];
    let mut sa = SliceTokenSource::new(&doc_a);
    if let Err(e) = b.add_doc(DocId(1), &mut sa, 3) {
        fatal(&format!("{e}"));
    }
    let mut sb = SliceTokenSource::new(&doc_b);
    if let Err(e) = b.add_doc(DocId(2), &mut sb, 3) {
        fatal(&format!("{e}"));
    }
    let mut sc = SliceTokenSource::new(&doc_c);
    if let Err(e) = b.add_doc(DocId(3), &mut sc, 3) {
        fatal(&format!("{e}"));
    }
    let mut sd = SliceTokenSource::new(&doc_d);
    if let Err(e) = b.add_doc(DocId(4), &mut sd, 3) {
        fatal(&format!("{e}"));
    }
    let table = match b.finish() {
        Ok(t) => t,
        Err(e) => fatal(&format!("{e}")),
    };
    match Bm25Scorer::new(Bm25Params::DEFAULTS, table) {
        Ok(s) => s,
        Err(e) => fatal(&format!("{e}")),
    }
}

#[test]
fn golden_doc_scores_match_pinned_bits() {
    // Pinned `f32::to_bits()` values were captured at first run with
    // BM25 defaults `(k1=1.2, b=0.75)` over the 4-doc corpus above.
    //
    // Queries:
    //   q1: ["alpha"]        — common term
    //   q2: ["epsilon"]      — rare term
    //   q3: ["alpha", "beta"] — multi-term, common
    //   q4: ["unseen_term"]  — unseen (max IDF)
    let s = build_corpus_scorer();

    let cases: &[(&[&str], u32)] = &[
        (&["alpha"], 1),
        (&["epsilon"], 1),
        (&["alpha", "beta"], 2),
        (&["unseen_term"], 1),
    ];

    let mut got_bits: Vec<u32> = Vec::with_capacity(cases.len());
    for (terms, dl) in cases {
        let mut src = SliceTokenSource::new(terms);
        let v = match s.score_doc(&mut src, *dl) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(v.is_finite());
        assert!((0.0..=1.0).contains(&v));
        got_bits.push(v.to_bits());
    }

    // The expected values are CURRENT pinned bits. They are the
    // deterministic output of the closed-form BM25 + envelope normalization
    // on the corpus above. If you change a constant in `bm25.rs` /
    // `idf.rs` / `scorer.rs` (k1, b, or the formula), update these via the
    // print env-var test below.
    let expected: [u32; 4] = [
        EXPECTED_ALPHA,
        EXPECTED_EPSILON,
        EXPECTED_ALPHA_BETA,
        EXPECTED_UNSEEN,
    ];

    for (i, (g, e)) in got_bits.iter().zip(expected.iter()).enumerate() {
        if g != e {
            let f_got = f32::from_bits(*g);
            let f_exp = f32::from_bits(*e);
            assert!(
                false,
                "case {i}: got bits 0x{g:08x} ({f_got}), expected 0x{e:08x} ({f_exp})"
            );
        }
    }
}

#[test]
fn rarer_term_scores_higher_than_common_term() {
    // Sanity check on the corpus: epsilon (1/4 docs) outscores alpha
    // (3/4 docs) on a single-token query.
    let s = build_corpus_scorer();
    let qa = ["alpha"];
    let qe = ["epsilon"];
    let mut sa = SliceTokenSource::new(&qa);
    let mut se = SliceTokenSource::new(&qe);
    let va = match s.score_doc(&mut sa, 1) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let ve = match s.score_doc(&mut se, 1) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert!(ve > va, "expected epsilon ({ve}) > alpha ({va})");
}

// Re-deriving pinned values:
//
// If the BM25 formula / IDF formula / normalization changes intentionally,
// re-pin via:
//
//   1. Replace each `EXPECTED_*` constant below with `0_u32`.
//   2. `cargo test -p quanta-index-lq-scorer --test golden_scores
//      golden_doc_scores_match_pinned_bits -- --nocapture` — the
//      assertion failure prints `got bits 0x...` and the float value
//      for each case.
//   3. Copy each printed `0x...` value back into the corresponding
//      `EXPECTED_*` constant.
//
// This regenerate-vs-assert workflow keeps the golden corpus immune to
// silent drift while making intentional updates auditable.

// Pinned values — see `print_golden_when_env_set` to re-derive. These will
// be set once after the first failing run prints the actual bits.
const EXPECTED_ALPHA: u32 = 0x3e22_01a8; // 0.15820944
const EXPECTED_EPSILON: u32 = 0x3ec6_bcdf; // 0.38815972
const EXPECTED_ALPHA_BETA: u32 = 0x3e76_33b5; // 0.24043162
const EXPECTED_UNSEEN: u32 = 0x3f0c_5609; // 0.5481878
