//! Golden semantic fixture — 5 docs, 3 queries, pinned `AnnResult` rows.
//!
//! This file is the spec-bound parity check for the SEM-01 executor:
//! given a fixed corpus, three known query vectors must produce the
//! exact `(doc_id, score-approx)` sequence below. Failures here mean
//! either the cosine kernel drifted or the executor's
//! `score DESC, doc_id ASC` tiebreak broke.

use quanta_index_lq_semantic::{
    AnnResult, DocId, Embedding, SemanticIndex, SemanticIndexBuilder, query_cosine_topk,
};

fn must_emb(v: Vec<f32>) -> Embedding {
    let Ok(e) = Embedding::new(v) else {
        std::process::abort();
    };
    e
}

fn must_index() -> SemanticIndex {
    // Five docs in a 3-d space. Vectors deliberately span the unit
    // sphere so cosine scores cover [-1, 1].
    let Ok(mut b) = SemanticIndexBuilder::new(7, 3) else {
        std::process::abort();
    };
    let e1 = must_emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
    if b.add_embedding(DocId(1), &e1).is_err() {
        std::process::abort();
    }
    let e2 = must_emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
    if b.add_embedding(DocId(2), &e2).is_err() {
        std::process::abort();
    }
    let e3 = must_emb(vec![0.0_f32, 0.0_f32, 1.0_f32]);
    if b.add_embedding(DocId(3), &e3).is_err() {
        std::process::abort();
    }
    let e4 = must_emb(vec![-1.0_f32, 0.0_f32, 0.0_f32]);
    if b.add_embedding(DocId(4), &e4).is_err() {
        std::process::abort();
    }
    let e5 = must_emb(vec![1.0_f32, 1.0_f32, 0.0_f32]);
    if b.add_embedding(DocId(5), &e5).is_err() {
        std::process::abort();
    }
    b.finish()
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn assert_hit(got: &AnnResult, want_doc: DocId, want_score: f32) {
    assert_eq!(got.doc_id, want_doc, "doc_id mismatch");
    assert!(
        approx(got.score, want_score),
        "score for {:?}: got {}, want {}",
        want_doc,
        got.score,
        want_score
    );
}

#[test]
fn query_1_self_similarity_top1() {
    // Query == doc 1's vector → cosine 1.0; expected single hit.
    let idx = must_index();
    let q = must_emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
    let out = match query_cosine_topk(&idx, &q, 1) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(out.len(), 1);
    let Some(r) = out.first() else {
        assert!(false, "missing first");
        return;
    };
    assert_hit(r, DocId(1), 1.0_f32);
}

#[test]
fn query_2_top3_ordered_descending() {
    // Query [1,1,0] / √2 — cosine with doc 5 [1,1,0] is 1.0; with doc 1
    // [1,0,0] is 1/√2; with doc 2 [0,1,0] is 1/√2; with doc 3 [0,0,1] is
    // 0; with doc 4 [-1,0,0] is -1/√2. Top 3: doc 5 (1.0), then doc 1
    // and doc 2 tied at 1/√2 ≈ 0.7071 — tiebreak by ascending DocId
    // means doc 1 then doc 2.
    let idx = must_index();
    let inv_sqrt2 = 1.0_f32 / 2.0_f32.sqrt();
    let q = must_emb(vec![inv_sqrt2, inv_sqrt2, 0.0_f32]);
    let out = match query_cosine_topk(&idx, &q, 3) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(out.len(), 3);
    let Some(r0) = out.first() else {
        assert!(false, "missing first");
        return;
    };
    let Some(r1) = out.get(1) else {
        assert!(false, "missing second");
        return;
    };
    let Some(r2) = out.get(2) else {
        assert!(false, "missing third");
        return;
    };
    assert_hit(r0, DocId(5), 1.0_f32);
    assert_hit(r1, DocId(1), inv_sqrt2);
    assert_hit(r2, DocId(2), inv_sqrt2);
}

#[test]
fn query_3_negative_scores_at_bottom() {
    // Query [1,0,0] returns:
    //   doc 1 → 1.0
    //   doc 5 → 1/√2  ≈ 0.7071
    //   doc 2 → 0.0
    //   doc 3 → 0.0   (tiebreak: doc 2 then doc 3 by ascending id)
    //   doc 4 → -1.0
    let idx = must_index();
    let q = must_emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
    let out = match query_cosine_topk(&idx, &q, 5) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(out.len(), 5);
    let inv_sqrt2 = 1.0_f32 / 2.0_f32.sqrt();
    let Some(r0) = out.first() else {
        assert!(false, "missing first");
        return;
    };
    let Some(r1) = out.get(1) else {
        assert!(false, "missing second");
        return;
    };
    let Some(r2) = out.get(2) else {
        assert!(false, "missing third");
        return;
    };
    let Some(r3) = out.get(3) else {
        assert!(false, "missing fourth");
        return;
    };
    let Some(r4) = out.get(4) else {
        assert!(false, "missing fifth");
        return;
    };
    assert_hit(r0, DocId(1), 1.0_f32);
    assert_hit(r1, DocId(5), inv_sqrt2);
    assert_hit(r2, DocId(2), 0.0_f32);
    assert_hit(r3, DocId(3), 0.0_f32);
    assert_hit(r4, DocId(4), -1.0_f32);
}
