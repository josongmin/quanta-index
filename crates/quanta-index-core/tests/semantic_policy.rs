//! Unit tests for semantic policy validation edges.

#![forbid(unsafe_code)]

use quanta_index_core::{CoreError, SemanticPolicy};

fn typed_code_or_debug(result: Result<(), CoreError>) -> String {
    match result {
        Err(CoreError::Typed { code, .. }) => code,
        other => format!("unexpected result: {other:?}"),
    }
}

// `top_k` is one contract for every route (QI-BB-025). The semantic route used
// to answer zero with `INVALID_FILTER_VALUE` and above-ceiling with
// `PLAN_LIMIT_EXCEEDED`, while hybrid answered both with its own code and the
// dispatcher's probe refused the public maximum outright. One code now.
#[test]
fn semantic_top_k_zero_uses_the_shared_out_of_range_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(0)),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
    );
}

#[test]
fn semantic_top_k_above_ceiling_uses_the_shared_out_of_range_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(
            SemanticPolicy::max_top_k().saturating_add(1),
        )),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
    );
}

#[test]
fn semantic_top_k_public_maximum_is_accepted() {
    assert!(SemanticPolicy::validate_top_k(SemanticPolicy::max_top_k()).is_ok());
    assert_eq!(
        SemanticPolicy::max_top_k(),
        quanta_index_contract::PUBLIC_TOP_K_MAX
    );
}

#[test]
fn semantic_query_vector_nan_is_invalid() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_query_vector(&[1.0, f32::NAN])),
        "SEM_INVALID_VECTOR"
    );
}

#[test]
fn semantic_query_vector_zero_norm_is_invalid() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_query_vector(&[0.0, 0.0, 0.0])),
        "SEM_INVALID_VECTOR"
    );
}

#[test]
fn semantic_query_vector_finite_non_zero_is_valid() {
    assert!(SemanticPolicy::validate_query_vector(&[1.0, 0.0, 2.0]).is_ok());
}

// ---------------------------------------------------------------------------
// QI-BB-031 — the vector contract is one validator and one normalizer,
// applied to fresh provider output, cache hits and every ingested row.
// ---------------------------------------------------------------------------

mod vector_contract {
    use quanta_index_contract::EmbeddingNormalization;
    use quanta_index_core::{
        CoreError, L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider, SemanticPolicy,
        TextEmbeddingProvider,
    };

    fn code(result: Result<(), CoreError>) -> String {
        match result {
            Err(CoreError::Typed { code, .. }) => code,
            other => format!("unexpected result: {other:?}"),
        }
    }

    /// Under `L2Unit`, only a unit vector of the right dimension passes; under
    /// `None`, any finite non-zero vector of the right dimension passes.
    #[test]
    fn the_validator_names_every_defect_and_accepts_the_contract() {
        let unit = [0.6_f32, 0.8];
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(&unit, 2, EmbeddingNormalization::L2Unit)
                .is_ok()
        );
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(&unit, 2, EmbeddingNormalization::None)
                .is_ok()
        );
        let cases: [(&str, Vec<f32>, EmbeddingNormalization); 7] = [
            ("half", vec![0.3, 0.4], EmbeddingNormalization::L2Unit),
            ("double", vec![1.2, 1.6], EmbeddingNormalization::L2Unit),
            ("zero", vec![0.0, 0.0], EmbeddingNormalization::L2Unit),
            ("zero raw", vec![0.0, 0.0], EmbeddingNormalization::None),
            ("nan", vec![f32::NAN, 0.0], EmbeddingNormalization::None),
            (
                "inf",
                vec![f32::INFINITY, 0.0],
                EmbeddingNormalization::L2Unit,
            ),
            (
                "dimension",
                vec![1.0, 0.0, 0.0],
                EmbeddingNormalization::L2Unit,
            ),
        ];
        for (label, vector, normalization) in cases {
            assert_eq!(
                code(SemanticPolicy::validate_embedding_vector_v1(
                    &vector,
                    2,
                    normalization
                )),
                "SEM_INVALID_VECTOR",
                "{label}"
            );
        }
        // A raw contract does not care about the norm.
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(
                &[0.3, 0.4],
                2,
                EmbeddingNormalization::None
            )
            .is_ok()
        );
    }

    /// Normalization is deterministic and lands inside the tolerance even for
    /// a long vector, and refuses what it cannot normalize.
    #[test]
    fn normalization_is_deterministic_and_within_tolerance() {
        let mut vector = vec![3.0_f32, 4.0];
        SemanticPolicy::normalize_l2_unit_v1(&mut vector).expect("normalizable");
        assert_eq!(vector, vec![0.6, 0.8]);

        // A long, badly scaled vector with mixed magnitudes and signs.
        let mut long: Vec<f32> = (0..1536_u16)
            .map(|index| {
                let phase = f32::from(index) * 0.017;
                phase.sin() * 250.0
            })
            .collect();
        let mut again = long.clone();
        SemanticPolicy::normalize_l2_unit_v1(&mut long).expect("normalizable");
        SemanticPolicy::normalize_l2_unit_v1(&mut again).expect("normalizable");
        assert_eq!(long, again, "normalization is deterministic");
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(
                &long,
                1536,
                EmbeddingNormalization::L2Unit
            )
            .is_ok()
        );
        let norm: f64 = long
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < L2_UNIT_NORM_TOLERANCE, "norm {norm}");

        for (label, mut bad) in [("zero", vec![0.0_f32, 0.0]), ("nan", vec![f32::NAN, 1.0])] {
            assert_eq!(
                code(SemanticPolicy::normalize_l2_unit_v1(&mut bad)),
                "SEM_INVALID_VECTOR",
                "{label}"
            );
        }
    }

    /// A scripted raw provider: what it returns for each call.
    struct Scripted {
        vectors: Vec<Vec<f32>>,
        normalization: EmbeddingNormalization,
    }

    impl TextEmbeddingProvider for Scripted {
        fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok(self.vectors.clone())
        }
        fn model_id(&self) -> &'static str {
            "scripted"
        }
        fn model_revision(&self) -> &'static str {
            "r7"
        }
        fn dimension(&self) -> usize {
            2
        }
        fn normalization(&self) -> EmbeddingNormalization {
            self.normalization
        }
    }

    fn raw(vectors: Vec<Vec<f32>>) -> Scripted {
        Scripted {
            vectors,
            normalization: EmbeddingNormalization::None,
        }
    }

    /// The wrapper turns raw output into unit vectors, keeps the identity,
    /// promises `L2Unit`, and fails the whole batch on output it cannot
    /// normalize — the documented behaviour both paths share.
    #[test]
    fn the_wrapper_normalizes_raw_output_or_fails_the_batch_closed() {
        let provider = L2UnitEmbeddingProvider::new(raw(vec![vec![2.0, 0.0], vec![0.5, 0.5]]))
            .expect("raw provider wraps");
        assert_eq!(provider.model_id(), "scripted");
        assert_eq!(provider.model_revision(), "r7");
        assert_eq!(provider.dimension(), 2);
        assert_eq!(provider.normalization(), EmbeddingNormalization::L2Unit);
        let served = provider.embed_batch(&["a", "b"]).expect("normalized");
        assert_eq!(served.first(), Some(&vec![1.0, 0.0]));
        let second_norm: f64 = served
            .get(1)
            .expect("second vector")
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        assert!((second_norm - 1.0).abs() < L2_UNIT_NORM_TOLERANCE);

        for (label, vectors) in [
            ("zero", vec![vec![0.0, 0.0]]),
            ("nan", vec![vec![f32::NAN, 0.0]]),
            ("inf", vec![vec![f32::INFINITY, 1.0]]),
            ("dimension", vec![vec![1.0, 0.0, 0.0]]),
            ("count", vec![vec![1.0, 0.0], vec![0.0, 1.0]]),
        ] {
            let provider = L2UnitEmbeddingProvider::new(raw(vectors)).expect("raw provider wraps");
            let outcome = provider.embed_batch(&["only"]);
            assert!(
                outcome.is_err(),
                "{label}: must fail the batch, got {outcome:?}"
            );
        }
    }

    /// Stacking normalizers would hide which layer is trusted; a provider
    /// that already promises `L2Unit` is refused by the wrapper.
    #[test]
    fn the_wrapper_refuses_a_provider_that_already_promises_unit_vectors() {
        let already = Scripted {
            vectors: Vec::new(),
            normalization: EmbeddingNormalization::L2Unit,
        };
        assert!(matches!(
            L2UnitEmbeddingProvider::new(already),
            Err(CoreError::InvalidContract(_))
        ));
    }
}
