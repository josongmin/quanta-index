use super::*;
use crate::{
    NativeIdentityDecodeDataRefusalV1 as Refusal,
    SearchCorpusActivationValidationErrorV1 as Activation,
    SearchCorpusGenerationIdentityValidationErrorV1 as Identity,
    SearchPlaneActivateSearchCorpusGenerationCasRequest as Cas,
};
use core::fmt;

#[derive(Debug)]
struct DynamicError {
    marker: String,
    full: Box<u8>,
}
impl fmt::Display for DynamicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.marker)
    }
}
impl std::error::Error for DynamicError {}
impl de::Error for DynamicError {
    fn custom<T: fmt::Display>(cause: T) -> Self {
        Self {
            marker: cause.to_string(),
            full: Box::new(79),
        }
    }
}
fn head() -> SearchCorpusActiveHeadV1 {
    super::super::super::qi_act_01_tests::corpus_head(3, "digest-3", 1)
}
fn validate_head(
    head: &SearchCorpusActiveHeadV1,
    admission: &mut Admission,
    data: &mut NativeCorpusDecodeDataV1<Box<u8>, Funding>,
    failure: &mut Option<DynamicError>,
) -> Result<(), Refusal> {
    head.validate_into_with_native_admission_v1(admission, data, failure)
}
fn validate_cas(
    candidate_repo: &RepoId,
    candidate_revision: &RevisionId,
    candidate_generation: u64,
    expected: Option<&SearchCorpusActiveHeadV1>,
    admission: &mut Admission,
    data: &mut NativeCorpusDecodeDataV1<Box<u8>, Funding>,
    failure: &mut Option<DynamicError>,
) -> Result<(), Refusal> {
    Cas::validate_expected_active_scope_into_with_native_admission_v1(
        candidate_repo,
        candidate_revision,
        ManifestGeneration::new(candidate_generation),
        expected,
        admission,
        data,
        failure,
    )
}

#[test]
fn borrowed_whole_head_preserves_predicate_order_without_birth_or_input_retention() {
    let valid = head();
    let mut malformed = Vec::new();
    let mut value = valid.clone();
    value.generation.semantic_content.row_root_digest = "bad".to_string();
    value.generation.lexical.track = SearchPlaneTrackKind::Structural;
    malformed.push((value, Identity::SemanticContentRootInvalid));
    let mut value = valid.clone();
    value.generation.lexical.track = SearchPlaneTrackKind::Semantic;
    value.generation.semantic.track = SearchPlaneTrackKind::Lexical;
    malformed.push((value, Identity::LexicalTrackRequired));
    let mut value = valid.clone();
    value.generation.semantic.track = SearchPlaneTrackKind::Lexical;
    malformed.push((value, Identity::SemanticTrackRequired));
    let mut value = valid.clone();
    value.generation.semantic.repo_id = RepoId::new("other").unwrap();
    value.generation.semantic.revision_id = RevisionId::new("other").unwrap();
    malformed.push((value, Identity::RepoMismatch));
    let mut value = valid.clone();
    value.generation.semantic.revision_id = RevisionId::new("other").unwrap();
    value.generation.semantic.manifest_generation = ManifestGeneration::new(4);
    malformed.push((value, Identity::RevisionMismatch));
    let mut value = valid.clone();
    value.generation.semantic.manifest_generation = ManifestGeneration::new(4);
    value.generation.semantic.manifest_digest = "other".to_string();
    malformed.push((value, Identity::GenerationMismatch));
    let mut value = valid.clone();
    value.generation.semantic.manifest_digest = "other".to_string();
    malformed.push((value, Identity::DigestMismatch));
    let mut value = valid;
    value.generation.lexical.manifest_digest = " \t".to_string();
    value.generation.semantic.manifest_digest = " \t".to_string();
    malformed.push((value, Identity::EmptyDigest));
    for (value, expected) in malformed {
        assert_eq!(value.validate_v1(), Err(expected));
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut failure = None;
        let mut admission = Admission::default();
        assert_eq!(
            validate_head(&value, &mut admission, &mut data, &mut failure),
            Err(Refusal::OperationRefused)
        );
        assert!(
            matches!(data.failure_v1(), Some(NativeCorpusDecodeFailureV1::Validation(cause)) if *cause == expected)
        );
        assert!(
            failure
                .as_ref()
                .unwrap()
                .marker
                .contains("InvalidData(Semantic")
        );
        assert_eq!(admission.invalid_calls.get(), 1);
        assert_eq!(
            (admission.copies, admission.borrowed, admission.owned),
            (0, 0, 0)
        );
        assert_eq!(
            data.complete_into_slot_v1(&mut None),
            Err(Refusal::MissingResult)
        );
    }
    let data = {
        let value = head();
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut admission = Admission::default();
        assert!(validate_head(&value, &mut admission, &mut data, &mut None).is_ok());
        // Fixed fixture: 2*(repo=4 + rev=3 + digest=8) + 2*sha256-root=71 + one node.
        assert_eq!(admission.work, 173);
        assert_eq!(
            (admission.copies, admission.borrowed, admission.owned),
            (0, 0, 0)
        );
        data
    };
    assert!(!data.is_fresh_v1());
    assert!(data.failure_v1().is_none());
}

#[test]
fn borrowed_head_retains_exact_work_and_dynamic_causes_and_rejects_reentry_before_poll() {
    for limit in [0, 7, 172] {
        let cause = Box::new(53);
        let pointer = core::ptr::from_ref(cause.as_ref());
        let mut admission = Admission {
            work_limit: Some(limit),
            work_cause: Some(cause),
            ..Admission::default()
        };
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut failure = None;
        assert_eq!(
            validate_head(&head(), &mut admission, &mut data, &mut failure),
            Err(Refusal::OperationRefused)
        );
        assert!(
            matches!(data.failure_v1(), Some(NativeCorpusDecodeFailureV1::Work(cause)) if core::ptr::from_ref(cause.as_ref()) == pointer && **cause == 53)
        );
        let full_pointer = core::ptr::from_ref(failure.as_ref().unwrap().full.as_ref());
        let polls = admission.work_calls;
        assert_eq!(
            validate_head(&head(), &mut admission, &mut data, &mut failure),
            Err(Refusal::OccupiedOutput)
        );
        assert_eq!(
            core::ptr::from_ref(failure.as_ref().unwrap().full.as_ref()),
            full_pointer
        );
        assert_eq!(
            validate_head(&head(), &mut admission, &mut data, &mut None),
            Err(Refusal::UsedData)
        );
        assert_eq!(admission.work_calls, polls);
        assert_eq!(admission.invalid_calls.get(), 0);
        assert_eq!(admission.copies, 0);
        drop(admission);
        assert!(
            matches!(data.failure_v1(), Some(NativeCorpusDecodeFailureV1::Work(cause)) if core::ptr::from_ref(cause.as_ref()) == pointer)
        );
    }
}

#[test]
fn borrowed_native_cas_has_the_same_expected_head_first_scope_and_advance_predicates() {
    let expected = head();
    let repo = RepoId::new("repo").unwrap();
    let revision = RevisionId::new("rev").unwrap();
    let other_repo = RepoId::new("other-repo").unwrap();
    let other_revision = RevisionId::new("other-rev").unwrap();
    let mut invalid = expected.clone();
    invalid.generation.semantic.track = SearchPlaneTrackKind::Lexical;
    for (candidate_repo, candidate_revision, generation, expected, wanted) in [
        (
            &other_repo,
            &other_revision,
            0,
            &invalid,
            Some(Activation::ExpectedActiveIdentity(
                Identity::SemanticTrackRequired,
            )),
        ),
        (
            &other_repo,
            &other_revision,
            0,
            &expected,
            Some(Activation::RepoMismatch),
        ),
        (
            &repo,
            &other_revision,
            0,
            &expected,
            Some(Activation::RevisionMismatch),
        ),
        (
            &repo,
            &revision,
            2,
            &expected,
            Some(Activation::CandidateGenerationMustAdvanceExpectedActive),
        ),
        (
            &repo,
            &revision,
            3,
            &expected,
            Some(Activation::CandidateGenerationMustAdvanceExpectedActive),
        ),
        (&repo, &revision, 4, &expected, None),
        (&repo, &revision, u64::MAX, &expected, None),
    ] {
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut failure = None;
        let mut admission = Admission::default();
        let result = validate_cas(
            candidate_repo,
            candidate_revision,
            generation,
            Some(expected),
            &mut admission,
            &mut data,
            &mut failure,
        );
        assert_eq!(
            result,
            wanted.map_or(Ok(()), |_| Err(Refusal::OperationRefused))
        );
        assert_eq!(
            Cas::validate_expected_active_scope_v1(
                candidate_repo,
                candidate_revision,
                ManifestGeneration::new(generation),
                Some(expected)
            ),
            wanted.map_or(Ok(()), Err)
        );
        if let Some(wanted) = wanted {
            assert!(
                matches!(data.failure_v1(), Some(NativeCorpusDecodeFailureV1::Activation(cause)) if *cause == wanted)
            );
            assert!(
                failure
                    .as_ref()
                    .unwrap()
                    .marker
                    .contains("InvalidData(Semantic")
            );
        } else {
            assert!(data.failure_v1().is_none());
            assert!(failure.is_none());
            // Head=173, repo=(1+4+4), revision=(1+3+3), generation=1.
            assert_eq!(admission.work, 190);
        }
        assert_eq!(
            (admission.copies, admission.borrowed, admission.owned),
            (0, 0, 0)
        );
    }
}

#[test]
fn native_cas_preserves_none_and_used_or_occupied_data_without_any_poll() {
    let expected = head();
    let repo = &expected.generation.lexical.repo_id;
    let revision = &expected.generation.lexical.revision_id;
    let mut admission = Admission {
        work_limit: Some(0),
        ..Admission::default()
    };
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    assert!(
        validate_cas(
            repo,
            revision,
            0,
            None,
            &mut admission,
            &mut data,
            &mut None
        )
        .is_ok()
    );
    assert_eq!(admission.work_calls, 0);
    assert_eq!(admission.invalid_calls.get(), 0);
    assert!(data.failure_v1().is_none());
    assert!(!data.is_fresh_v1());
    assert_eq!(
        validate_cas(
            repo,
            revision,
            0,
            Some(&expected),
            &mut admission,
            &mut data,
            &mut None
        ),
        Err(Refusal::UsedData)
    );
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut failure = Some(DynamicError {
        marker: "original".to_string(),
        full: Box::new(83),
    });
    let pointer = core::ptr::from_ref(failure.as_ref().unwrap().full.as_ref());
    assert_eq!(
        validate_cas(
            repo,
            revision,
            4,
            Some(&expected),
            &mut admission,
            &mut data,
            &mut failure
        ),
        Err(Refusal::OccupiedOutput)
    );
    assert!(data.is_fresh_v1());
    assert_eq!(
        core::ptr::from_ref(failure.as_ref().unwrap().full.as_ref()),
        pointer
    );
    assert_eq!(admission.work_calls, 0);
}

#[test]
fn native_cas_source_work_refusals_precede_unpaid_comparisons_and_preserve_original_box() {
    let expected = head();
    for limit in [173, 174, 178, 182, 183, 189] {
        let cause = Box::new(61);
        let pointer = core::ptr::from_ref(cause.as_ref());
        let mut admission = Admission {
            work_limit: Some(limit),
            work_cause: Some(cause),
            ..Admission::default()
        };
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut failure = None;
        assert_eq!(
            validate_cas(
                &expected.generation.lexical.repo_id,
                &expected.generation.lexical.revision_id,
                3,
                Some(&expected),
                &mut admission,
                &mut data,
                &mut failure
            ),
            Err(Refusal::OperationRefused)
        );
        assert!(
            matches!(data.failure_v1(), Some(NativeCorpusDecodeFailureV1::Work(cause)) if core::ptr::from_ref(cause.as_ref()) == pointer && **cause == 61)
        );
        assert!(failure.as_ref().unwrap().marker.contains("Admission"));
        assert!(data.state.activation_failure.is_none());
        assert_eq!(admission.invalid_calls.get(), 0);
        let polls = admission.work_calls;
        assert_eq!(
            validate_cas(
                &expected.generation.lexical.repo_id,
                &expected.generation.lexical.revision_id,
                4,
                Some(&expected),
                &mut admission,
                &mut data,
                &mut None
            ),
            Err(Refusal::UsedData)
        );
        assert_eq!(admission.work_calls, polls);
    }
}

#[test]
fn decode_finisher_and_borrowed_validation_retain_the_same_complete_semantic_cause() {
    let mut value = head();
    value.generation.semantic.manifest_generation = ManifestGeneration::new(4);
    let wire = serde_json::to_string(&value).unwrap();
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut admission = Admission::default();
    let mut decoder = serde_json::Deserializer::from_str(&wire);
    let mut failure = None;
    assert_eq!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            &mut decoder,
            &mut admission,
            &mut data,
            &mut failure
        ),
        Err(Refusal::OperationRefused)
    );
    assert!(matches!(
        data.failure_v1(),
        Some(NativeCorpusDecodeFailureV1::Validation(
            Identity::GenerationMismatch
        ))
    ));
    assert!(
        data.state.output.is_some(),
        "actual decoded head remains external"
    );
    assert_eq!(
        data.complete_into_slot_v1(&mut None),
        Err(Refusal::MissingResult)
    );
    let polls = admission.work_calls;
    assert_eq!(
        validate_head(&value, &mut admission, &mut data, &mut None),
        Err(Refusal::UsedData)
    );
    assert_eq!(admission.work_calls, polls);
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    assert_eq!(
        validate_head(&value, &mut admission, &mut data, &mut None),
        Err(Refusal::OperationRefused)
    );
    assert!(matches!(
        data.failure_v1(),
        Some(NativeCorpusDecodeFailureV1::Validation(
            Identity::GenerationMismatch
        ))
    ));
}
