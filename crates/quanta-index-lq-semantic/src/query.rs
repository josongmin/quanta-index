//! Deterministic top-k executor over a [`SemanticIndex`].
//!
//! Strategy: exact NN — iterate every doc, compute cosine, sort by
//! `score DESC, doc_id ASC`, take the first `top_k`. This is O(N · D)
//! per query; correctness over scaling at MVP. The 100k-document cutoff
//! ([`crate::types::EXACT_NN_CUTOFF`]) is the spec-locked boundary
//! above which we fail closed with
//! [`SemanticErrorCode::SemAnnNondeterministic`]: pinned-seed HNSW
//! is the next step (SEM-01 spec §4.4, risk R-ANN-DET).
//!
//! Determinism rules (SEM-01 spec §5.6 + §6.1 UC-SEM-03):
//!
//! 1. score ordering is descending,
//! 2. ties break by ascending [`DocId`],
//! 3. same `(query, generation)` → same byte sequence two runs later.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::cmp::Ordering;

use crate::cosine::cosine_similarity;
use crate::errors::{LimitDimension, SemanticError, SemanticErrorCode};
use crate::index::SemanticIndex;
use crate::types::{DocId, EXACT_NN_CUTOFF, Embedding, MAX_TOP_K};

/// A single hit from the executor. `score` is the cosine similarity in
/// the canonical `[-1.0, 1.0]` interval (subject to `f32` rounding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnnResult {
    pub doc_id: DocId,
    pub score: f32,
}

impl AnnResult {
    /// Construct an `AnnResult`. Validates that `score` is finite; if
    /// the kernel produces a non-finite score we surface it as
    /// [`SemanticErrorCode::SemInvalidVector`] rather than silently
    /// propagating it.
    pub fn new(doc_id: DocId, score: f32) -> Result<Self, SemanticError> {
        if !score.is_finite() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("AnnResult score is non-finite: {score}"),
            ));
        }
        Ok(Self { doc_id, score })
    }
}

impl serde::Serialize for AnnResult {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("doc_id", &self.doc_id)?;
        m.serialize_entry("score", &self.score)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for AnnResult {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = AnnResult;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("AnnResult map (doc_id, score)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<AnnResult, M::Error> {
                let mut doc_id: Option<DocId> = None;
                let mut score: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "doc_id" => {
                            if doc_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("doc_id"));
                            }
                            doc_id = Some(map.next_value()?);
                        }
                        "score" => {
                            if score.is_some() {
                                return Err(serde::de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["doc_id", "score"],
                            ));
                        }
                    }
                }
                let doc_id = doc_id.ok_or_else(|| serde::de::Error::missing_field("doc_id"))?;
                let score = score.ok_or_else(|| serde::de::Error::missing_field("score"))?;
                if !score.is_finite() {
                    return Err(serde::de::Error::custom(format!(
                        "AnnResult score non-finite: {score}"
                    )));
                }
                Ok(AnnResult { doc_id, score })
            }
        }
        de.deserialize_map(V)
    }
}

/// Execute a cosine top-k query against `idx`. Fails closed on every
/// documented anomaly:
///
/// - `top_k == 0` returns an empty `Vec` (no work). The AST gate at
///   construction time should reject 0 with `PARSE_INVALID_FILTER_VALUE`
///   before reaching here; this is the defense-in-depth bottom layer.
/// - `top_k > MAX_TOP_K` returns
///   [`SemanticErrorCode::PlanLimitExceeded`] with
///   [`LimitDimension::TopK`].
/// - `idx.corpus_size() > EXACT_NN_CUTOFF` returns
///   [`SemanticErrorCode::SemAnnNondeterministic`]; pinned-seed HNSW
///   for larger corpora is deferred.
/// - dim mismatch between `query` and `idx` returns
///   [`SemanticErrorCode::SemDimMismatch`].
///
/// Any cosine kernel error propagates up unchanged.
pub fn query_cosine_topk(
    idx: &SemanticIndex,
    query: &Embedding,
    top_k: u32,
) -> Result<Vec<AnnResult>, SemanticError> {
    if top_k == 0 {
        return Ok(Vec::new());
    }
    if top_k > MAX_TOP_K {
        return Err(SemanticError::plan_limit(
            LimitDimension::TopK,
            format!("top_k {top_k} exceeds MAX_TOP_K {MAX_TOP_K}"),
        ));
    }
    if query.dim() != idx.dim() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemDimMismatch,
            format!("query dim {} != index dim {}", query.dim(), idx.dim()),
        ));
    }
    if idx.corpus_size() > EXACT_NN_CUTOFF {
        return Err(SemanticError::new(
            SemanticErrorCode::SemAnnNondeterministic,
            format!(
                "corpus_size {} > EXACT_NN_CUTOFF {EXACT_NN_CUTOFF}; pinned-seed HNSW not yet shipped",
                idx.corpus_size()
            ),
        ));
    }

    let q = query.as_slice();
    let mut hits: Vec<AnnResult> = Vec::with_capacity(idx.corpus_size());
    for (doc_id, vec) in idx.iter() {
        let score = cosine_similarity(q, vec)?;
        hits.push(AnnResult::new(doc_id, score)?);
    }

    // Sort by score DESC; ties → ascending DocId. Use `total_cmp` so NaN
    // would be handled total-ordering (but we already gated NaN out at
    // construction and at the cosine kernel).
    hits.sort_by(|a, b| {
        let by_score = b.score.total_cmp(&a.score);
        match by_score {
            Ordering::Less | Ordering::Greater => by_score,
            Ordering::Equal => a.doc_id.cmp(&b.doc_id),
        }
    });

    let k = usize::try_from(top_k).map_err(|e| {
        SemanticError::new(
            SemanticErrorCode::PlanLimitExceeded,
            format!("top_k cast: {e}"),
        )
    })?;
    if hits.len() > k {
        hits.truncate(k);
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::{AnnResult, query_cosine_topk};
    use crate::errors::{LimitDimension, SemanticErrorCode};
    use crate::index::SemanticIndexBuilder;
    use crate::types::{DocId, Embedding, MAX_TOP_K};

    fn emb(v: Vec<f32>) -> Embedding {
        let Ok(e) = Embedding::new(v) else {
            std::process::abort();
        };
        e
    }

    fn fixture() -> crate::index::SemanticIndex {
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            std::process::abort();
        };
        // Each entry deliberately on the unit circle for stable cosine
        // expectations.
        let e10 = emb(vec![1.0_f32, 0.0_f32]);
        if b.add_embedding(DocId(10), &e10).is_err() {
            std::process::abort();
        }
        let e20 = emb(vec![0.0_f32, 1.0_f32]);
        if b.add_embedding(DocId(20), &e20).is_err() {
            std::process::abort();
        }
        let e30 = emb(vec![-1.0_f32, 0.0_f32]);
        if b.add_embedding(DocId(30), &e30).is_err() {
            std::process::abort();
        }
        b.finish()
    }

    #[test]
    fn ann_result_rejects_non_finite_score() {
        match AnnResult::new(DocId(1), f32::NAN) {
            Ok(_) => assert!(false, "must reject NaN"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn top_k_zero_returns_empty() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        match query_cosine_topk(&i, &q, 0) {
            Ok(v) => assert!(v.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn top_k_above_max_errors_with_dimension() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        let k = MAX_TOP_K + 1;
        match query_cosine_topk(&i, &q, k) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::TopK));
            }
        }
    }

    #[test]
    fn top_k_at_max_succeeds_on_small_corpus() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        match query_cosine_topk(&i, &q, MAX_TOP_K) {
            Ok(v) => assert_eq!(v.len(), 3),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn dim_mismatch_errors() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
        match query_cosine_topk(&i, &q, 1) {
            Ok(_) => assert!(false, "must reject mismatch"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn topk_orders_by_score_desc() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        let out = match query_cosine_topk(&i, &q, 3) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // Expected: DocId(10) cosine 1.0, DocId(20) cosine 0.0, DocId(30)
        // cosine -1.0.
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
        assert_eq!(r0.doc_id, DocId(10));
        assert_eq!(r1.doc_id, DocId(20));
        assert_eq!(r2.doc_id, DocId(30));
        assert!((r0.score - 1.0_f32).abs() < 1e-6);
        assert!((r1.score - 0.0_f32).abs() < 1e-6);
        assert!((r2.score - (-1.0_f32)).abs() < 1e-6);
    }

    #[test]
    fn topk_truncates_to_k() {
        let i = fixture();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        match query_cosine_topk(&i, &q, 1) {
            Ok(v) => {
                assert_eq!(v.len(), 1);
                let Some(r) = v.first() else {
                    assert!(false, "missing first");
                    return;
                };
                assert_eq!(r.doc_id, DocId(10));
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn topk_breaks_ties_by_ascending_docid() {
        // Two docs with identical vectors → identical cosine → DocId
        // ascending wins.
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let e99 = emb(vec![1.0_f32, 0.0_f32]);
        if let Err(e) = b.add_embedding(DocId(99), &e99) {
            assert!(false, "{e}");
            return;
        }
        let e7 = emb(vec![1.0_f32, 0.0_f32]);
        if let Err(e) = b.add_embedding(DocId(7), &e7) {
            assert!(false, "{e}");
            return;
        }
        let i = b.finish();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        let out = match query_cosine_topk(&i, &q, 2) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let Some(r0) = out.first() else {
            assert!(false, "missing first");
            return;
        };
        let Some(r1) = out.get(1) else {
            assert!(false, "missing second");
            return;
        };
        assert_eq!(r0.doc_id, DocId(7));
        assert_eq!(r1.doc_id, DocId(99));
    }

    #[test]
    fn topk_determinism_two_runs_identical() {
        let i = fixture();
        let q = emb(vec![0.5_f32, 0.5_f32]);
        let a = match query_cosine_topk(&i, &q, 3) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let b = match query_cosine_topk(&i, &q, 3) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(a, b);
    }

    #[test]
    fn topk_empty_corpus_returns_empty() {
        let Ok(b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let i = b.finish();
        let q = emb(vec![1.0_f32, 0.0_f32]);
        match query_cosine_topk(&i, &q, 5) {
            Ok(v) => assert!(v.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn ann_result_serde_roundtrip() {
        let r = match AnnResult::new(DocId(42), 0.875_f32) {
            Ok(r) => r,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&r, &mut buf) {
            assert!(false, "{e}");
            return;
        }
        match ciborium::de::from_reader::<AnnResult, _>(buf.as_slice()) {
            Ok(g) => assert_eq!(g, r),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
