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

    fn scraped(tallies: &quanta_index_core::RawNormTallies) -> Result<(u64, u64, f64), CoreError> {
        use quanta_index_core::{MetricSourcePort, MetricValueV1};
        let points = tallies.scrape()?;
        let counter = |name: &str| {
            points
                .iter()
                .find(|point| point.name == name)
                .map(|point| point.value)
        };
        match (
            counter("semantic_embedding_raw_vectors_normalized_total"),
            counter("semantic_embedding_raw_vectors_off_unit_total"),
            counter("semantic_embedding_raw_norm_deviation_max"),
        ) {
            (
                Some(MetricValueV1::Counter(normalized)),
                Some(MetricValueV1::Counter(off_unit)),
                Some(MetricValueV1::Gauge(max_deviation)),
            ) => Ok((normalized, off_unit, max_deviation)),
            other => Err(CoreError::Storage(format!(
                "the tallies name three points: {other:?}"
            ))),
        }
    }

    /// The wrapper reports how far its raw provider was from unit before it
    /// normalized (QI-BB-031 보완 #4).
    ///
    /// A unit vector, one inside the tolerance and one of norm 2.0 count as
    /// three normalized, one off unit, and a largest deviation of exactly
    /// 1.0; a batch the wrapper refuses records nothing, and a later batch
    /// with a smaller deviation keeps the maximum.
    #[test]
    fn the_wrapper_reports_how_far_its_raw_provider_was_from_unit() {
        let provider = L2UnitEmbeddingProvider::new(raw(vec![
            vec![0.6, 0.8],
            vec![0.9996, 0.0],
            vec![2.0, 0.0],
        ]))
        .expect("raw provider wraps");
        let tallies = provider.raw_norm_tallies();
        assert_eq!(scraped(&tallies).expect("the tallies scrape"), (0, 0, 0.0));
        let _served = provider
            .embed_batch(&["unit", "close", "double"])
            .expect("normalized");
        assert_eq!(scraped(&tallies).expect("the tallies scrape"), (3, 1, 1.0));

        let refused = L2UnitEmbeddingProvider::new(raw(vec![vec![3.0, 0.0], vec![f32::NAN, 0.0]]))
            .expect("raw provider wraps");
        let refused_tallies = refused.raw_norm_tallies();
        assert!(refused.embed_batch(&["triple", "nan"]).is_err());
        assert_eq!(
            scraped(&refused_tallies).expect("the tallies scrape"),
            (0, 0, 0.0),
            "a refused batch records nothing, not even its first vector"
        );

        let smaller =
            L2UnitEmbeddingProvider::new(raw(vec![vec![0.5, 0.0]])).expect("raw provider wraps");
        let smaller_tallies = smaller.raw_norm_tallies();
        let _served = smaller.embed_batch(&["half"]).expect("normalized");
        let _served = smaller.embed_batch(&["half"]).expect("normalized");
        assert_eq!(
            scraped(&smaller_tallies).expect("the tallies scrape"),
            (2, 2, 0.5)
        );
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

// The dense lane contract renders one trace line a reader can act on
// (QI-BB-027): the index, whether the seal proved it, the effort, where
// the index's centroids came from, and how every segment was built — the
// appended segments' actual construction beam width is named, never the
// trained recipe claimed for them.
#[test]
fn dense_lane_trace_names_index_attestation_effort_lineage_and_segment_builds() {
    use quanta_index_core::{
        DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexSegmentBuildV1, DenseIndexTrainingV1,
        DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1,
    };
    let effort = || DenseIndexEffortV1 {
        index_kind: "ivf_hnsw_sq".to_string(),
        partitions: 3,
        nprobes: 3,
        ef_floor: 64,
        ef_per_candidate: 2,
        refine_factor: 2,
    };
    let exact = DenseLaneContractV1 {
        index: DenseIndexV1::Exact,
        attestation: DenseLaneAttestationV1::Sealed,
    };
    assert_eq!(
        exact.trace_detail(),
        "dense.index=exact; dense.attestation=sealed"
    );
    let trained = DenseLaneContractV1 {
        index: DenseIndexV1::Approximate {
            effort: effort(),
            lineage: DenseIndexTrainingV1 {
                trained_at_generation: 3,
                trained_rows: 1_200,
                appended_rows: 0,
                deleted_rows: 0,
            },
            build: DenseIndexBuildV1 {
                hnsw_m: 20,
                hnsw_ef_construction: 300,
                appended_segments: Vec::new(),
            },
        },
        attestation: DenseLaneAttestationV1::Sealed,
    };
    assert_eq!(
        trained.trace_detail(),
        "dense.index=ivf_hnsw_sq; dense.attestation=sealed; dense.partitions=3; dense.nprobes=3; dense.ef=max(64,2*candidates); dense.refine_factor=2; ann.trained_at=g3; ann.appended=0/1200; ann.deleted=0; ann.hnsw_m=20; ann.hnsw_ef_construction=300; ann.appended_segments=0; ann.appended_segments_m/ef_construction=none"
    );
    let appended = DenseLaneContractV1 {
        index: DenseIndexV1::Approximate {
            effort: effort(),
            lineage: DenseIndexTrainingV1 {
                trained_at_generation: 3,
                trained_rows: 1_200,
                appended_rows: 120,
                deleted_rows: 4,
            },
            build: DenseIndexBuildV1 {
                hnsw_m: 20,
                hnsw_ef_construction: 300,
                appended_segments: vec![
                    DenseIndexSegmentBuildV1 {
                        hnsw_m: 20,
                        hnsw_ef_construction: 150,
                    },
                    DenseIndexSegmentBuildV1 {
                        hnsw_m: 20,
                        hnsw_ef_construction: 150,
                    },
                ],
            },
        },
        attestation: DenseLaneAttestationV1::SealedByAnotherLibraryVersion,
    };
    assert_eq!(
        appended.trace_detail(),
        "dense.index=ivf_hnsw_sq; dense.attestation=sealed_by_another_library_version; dense.partitions=3; dense.nprobes=3; dense.ef=max(64,2*candidates); dense.refine_factor=2; ann.trained_at=g3; ann.appended=120/1200; ann.deleted=4; ann.hnsw_m=20; ann.hnsw_ef_construction=300; ann.appended_segments=2; ann.appended_segments_m/ef_construction=20/150,20/150"
    );
}
