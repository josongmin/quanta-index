//! BM25 scoring with deterministic envelope normalization.
//!
//! The scorer combines a frozen [`crate::bm25::Bm25Params`] pair with a
//! frozen [`crate::idf::IdfTable`] and an opaque term stream
//! [`IdfTokenSource`] to produce a final score in the closed envelope
//! `[0.0, 1.0]`.
//!
//! ## Normalization closed-form
//!
//! Given the raw BM25 sum
//! `s = Σ_t idf(t) · (tf_t · (k1+1)) / (tf_t + k1 · (1 - b + b · dl/avgdl))`,
//! the envelope-normalized score is:
//!
//! ```text
//! score = s / (s + alpha)
//! ```
//!
//! where `alpha = 1.0 + ln(N + 1)` and `N = table.total_docs()`. The
//! function is monotonic in `s`, maps `s == 0` to `0.0`, and asymptotes at
//! `1.0` as `s` grows. Any NaN or non-finite signal at any stage surfaces
//! as [`crate::errors::ScorerErrorCode::NanSignal`] — never silently
//! clipped.
//!
//! The normalization carries no learned parameters; it is fully determined
//! by the generation's `total_docs`, which is itself frozen. Cross-instance
//! reproducibility is therefore reduced to per-generation byte equality of
//! the IDF table and `Bm25Params`. Per LEX01-R2, we avoid `mul_add` /
//! `ln_1p` and stick to the literal formula so x86 / aarch64 produce
//! byte-identical f32 outputs.
//!
//! D18 — no proc-macro derives anywhere on the wire surface.

use crate::bm25::Bm25Params;
use crate::errors::{ScorerError, ScorerErrorCode};
use crate::idf::{IdfTable, idf_for_term};

/// Opaque term source consumed by [`Bm25Scorer::score_doc`].
///
/// Abstracts over the LEX-00 `Token` stream: the scorer only needs
/// term-string iteration and is agnostic to how those terms were produced.
pub trait IdfTokenSource {
    /// Yield the next term, or `None` at end-of-stream.
    fn next_term(&mut self) -> Option<&str>;
}

/// Slice-backed [`IdfTokenSource`] for fixtures and tests.
pub struct SliceTokenSource<'a> {
    inner: core::slice::Iter<'a, &'a str>,
}

impl<'a> SliceTokenSource<'a> {
    #[must_use]
    pub fn new(terms: &'a [&'a str]) -> Self {
        Self {
            inner: terms.iter(),
        }
    }
}

impl IdfTokenSource for SliceTokenSource<'_> {
    fn next_term(&mut self) -> Option<&str> {
        self.inner.next().copied()
    }
}

/// Frozen-state BM25 scorer.
#[derive(Clone, Debug)]
pub struct Bm25Scorer {
    params: Bm25Params,
    table: IdfTable,
    alpha: f64,
}

impl Bm25Scorer {
    /// Construct a scorer from frozen parameters and IDF table.
    ///
    /// `alpha = 1.0 + ln(total_docs + 1)` is precomputed once so the hot
    /// path is allocation-free and arithmetic stays in a small bounded
    /// envelope.
    pub fn new(params: Bm25Params, table: IdfTable) -> Result<Self, ScorerError> {
        let n = table.total_docs();
        if n == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                "scorer requires IdfTable with non-zero total_docs",
            ));
        }
        let alpha = compute_alpha(n);
        if !alpha.is_finite() || alpha <= 0.0 {
            return Err(ScorerError::new(
                ScorerErrorCode::ScoreNormalizationFailed,
                "alpha must be finite and positive",
            ));
        }
        Ok(Self {
            params,
            table,
            alpha,
        })
    }

    #[must_use]
    pub const fn params(&self) -> &Bm25Params {
        &self.params
    }

    #[must_use]
    pub const fn table(&self) -> &IdfTable {
        &self.table
    }

    /// Score a single document against this scorer's frozen state.
    ///
    /// `doc_terms` provides the post-analyzer term stream for the document
    /// (one token per occurrence — repeated terms aggregate into the
    /// term-frequency count). `doc_len` is the document's analyzer-emitted
    /// token count.
    ///
    /// Returns a normalized score in `[0.0, 1.0]`. Any NaN or non-finite
    /// intermediate surfaces as [`ScorerErrorCode::NanSignal`].
    #[expect(
        clippy::suboptimal_flops,
        reason = "explicit `a + b * c` (not `mul_add`) keeps x86 / aarch64 byte-identical per LEX01-R2"
    )]
    pub fn score_doc(
        &self,
        doc_terms: &mut dyn IdfTokenSource,
        doc_len: u32,
    ) -> Result<f32, ScorerError> {
        use std::collections::BTreeMap;
        let mut tf: BTreeMap<Box<str>, u32> = BTreeMap::new();
        while let Some(term) = doc_terms.next_term() {
            let entry = tf.entry(term.into()).or_insert(0);
            *entry = entry.saturating_add(1);
        }

        let k1 = f32_to_f64(self.params.k1());
        let b = f32_to_f64(self.params.b());
        let avgdl = self.table.avg_doc_len().max(1.0);
        let dl = u32_to_f64(doc_len).max(0.0);
        let length_norm = 1.0 - b + b * (dl / avgdl);

        let mut accum: f64 = 0.0;
        for (term, &tf_t) in &tf {
            if tf_t == 0 {
                continue;
            }
            let idf = f32_to_f64(idf_for_term(&self.table, term));
            if !idf.is_finite() {
                return Err(ScorerError::new(
                    ScorerErrorCode::NanSignal,
                    "IDF returned non-finite value",
                ));
            }
            let tf_f = u32_to_f64(tf_t);
            let numerator = tf_f * (k1 + 1.0);
            let denominator = tf_f + k1 * length_norm;
            if denominator <= 0.0 || !denominator.is_finite() {
                return Err(ScorerError::new(
                    ScorerErrorCode::NanSignal,
                    "BM25 denominator non-positive or non-finite",
                ));
            }
            let contrib = idf * (numerator / denominator);
            if !contrib.is_finite() {
                return Err(ScorerError::new(
                    ScorerErrorCode::NanSignal,
                    "BM25 per-term contribution non-finite",
                ));
            }
            accum += contrib;
        }

        if !accum.is_finite() {
            return Err(ScorerError::new(
                ScorerErrorCode::NanSignal,
                "raw BM25 accumulator non-finite",
            ));
        }
        if accum < 0.0 {
            return Err(ScorerError::new(
                ScorerErrorCode::NanSignal,
                "raw BM25 accumulator negative — IDF/tf invariants violated",
            ));
        }

        let normalized = accum / (accum + self.alpha);
        if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return Err(ScorerError::new(
                ScorerErrorCode::ScoreNormalizationFailed,
                "normalized score outside [0.0, 1.0]",
            ));
        }
        Ok(f64_to_f32(normalized))
    }
}

#[expect(
    clippy::imprecise_flops,
    reason = "ln(N + 1) is the documented closed-form alpha; ln_1p would diverge across arch"
)]
fn compute_alpha(n: u64) -> f64 {
    1.0 + (u64_to_f64(n) + 1.0).ln()
}

fn f32_to_f64(v: f32) -> f64 {
    f64::from(v)
}

fn u32_to_f64(v: u32) -> f64 {
    f64::from(v)
}

fn u64_to_f64(v: u64) -> f64 {
    #[expect(
        clippy::as_conversions,
        reason = "f64::from(u64) is not available for u64 above 2^32; explicit `as` is the only stable conversion"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "alpha precision-loss above 2^53 corpora is sub-ULP and preserves monotonicity"
    )]
    let f = v as f64;
    f
}

fn f64_to_f32(v: f64) -> f32 {
    #[expect(
        clippy::as_conversions,
        reason = "the f64 to f32 narrowing cast is the documented downcast"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "downcast f64 -> f32 at envelope; range [0.0, 1.0] guaranteed by caller-side checks"
    )]
    let f = v as f32;
    f
}

#[cfg(test)]
mod tests {
    use super::{Bm25Scorer, IdfTokenSource, SliceTokenSource};
    use crate::bm25::Bm25Params;
    use crate::errors::ScorerErrorCode;
    use crate::idf::IdfTable;
    use std::collections::BTreeMap;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn build_table() -> IdfTable {
        let mut m: BTreeMap<Box<str>, u64> = BTreeMap::new();
        for (k, v) in [("foo", 1u64), ("bar", 2), ("baz", 3)] {
            let prior = m.insert(k.into(), v);
            assert!(prior.is_none());
        }
        match IdfTable::new(1, 3, m, 4.0) {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        }
    }

    fn build_scorer() -> Bm25Scorer {
        let t = build_table();
        match Bm25Scorer::new(Bm25Params::DEFAULTS, t) {
            Ok(s) => s,
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn empty_doc_scores_to_zero() {
        let s = build_scorer();
        let terms: [&str; 0] = [];
        let mut src = SliceTokenSource::new(&terms);
        match s.score_doc(&mut src, 0) {
            Ok(v) => assert!(v.abs() < f32::EPSILON, "expected zero, got {v}"),
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn score_is_within_envelope() {
        let s = build_scorer();
        let terms = ["foo", "foo", "bar", "baz"];
        let mut src = SliceTokenSource::new(&terms);
        match s.score_doc(&mut src, 4) {
            Ok(v) => {
                assert!(v.is_finite());
                assert!((0.0..=1.0).contains(&v), "score {v} outside [0,1]");
                assert!(v > 0.0);
            }
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn higher_tf_yields_higher_score() {
        let s = build_scorer();
        let low = ["foo"];
        let hi = ["foo", "foo", "foo", "foo"];
        let mut lsrc = SliceTokenSource::new(&low);
        let mut hsrc = SliceTokenSource::new(&hi);
        let lo = match s.score_doc(&mut lsrc, 4) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let hi_v = match s.score_doc(&mut hsrc, 4) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(hi_v > lo, "expected {hi_v} > {lo}");
    }

    #[test]
    fn rarer_term_yields_higher_score() {
        let s = build_scorer();
        let foo = ["foo"];
        let baz = ["baz"];
        let mut foosrc = SliceTokenSource::new(&foo);
        let mut bazsrc = SliceTokenSource::new(&baz);
        let vfoo = match s.score_doc(&mut foosrc, 1) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let vbaz = match s.score_doc(&mut bazsrc, 1) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(vfoo > vbaz, "expected {vfoo} > {vbaz}");
    }

    #[test]
    fn unknown_term_in_doc_contributes_max_idf() {
        let s = build_scorer();
        let unk = ["NEVERSEEN"];
        let baz = ["baz"];
        let mut usrc = SliceTokenSource::new(&unk);
        let mut bsrc = SliceTokenSource::new(&baz);
        let vu = match s.score_doc(&mut usrc, 1) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let vb = match s.score_doc(&mut bsrc, 1) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(vu > vb, "expected {vu} > {vb}");
    }

    #[test]
    fn same_input_same_output() {
        let s = build_scorer();
        let terms = ["foo", "bar", "baz", "foo"];
        let mut a = SliceTokenSource::new(&terms);
        let mut b = SliceTokenSource::new(&terms);
        let va = match s.score_doc(&mut a, 4) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        let vb = match s.score_doc(&mut b, 4) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(va.to_bits(), vb.to_bits());
    }

    #[test]
    fn scorer_rejects_empty_table() {
        match IdfTable::new(1, 0, BTreeMap::new(), 1.0) {
            Ok(_) => assert!(false, "table should reject empty"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::EmptyCorpus),
        }
    }

    #[test]
    fn slice_token_source_walks_once() {
        let terms = ["a", "b", "c"];
        let mut src = SliceTokenSource::new(&terms);
        assert_eq!(src.next_term(), Some("a"));
        assert_eq!(src.next_term(), Some("b"));
        assert_eq!(src.next_term(), Some("c"));
        assert_eq!(src.next_term(), None);
    }
}
