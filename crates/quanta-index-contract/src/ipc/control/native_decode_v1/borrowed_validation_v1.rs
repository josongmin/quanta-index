//! Borrowed validation reuses the canonical head and CAS predicates.

use super::super::{
    ManifestGeneration, RepoId, RevisionId, SearchCorpusActivationValidationErrorV1 as Activation,
    SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityValidationErrorV1 as Identity,
};
use super::CorpusPolicyV1;
use serde::de;

/// SAME work model used by the head decode finisher and borrowed native entry.
/// Full admission and predicate causes enter external DATA before unit refusal.
pub(super) fn validate_head_with_policy_v1<P: CorpusPolicyV1, E: de::Error>(
    value: &SearchCorpusActiveHeadV1,
    policy: &mut P,
    work_failure: &mut Option<P::OriginalError>,
    validation_failure: &mut Option<Identity>,
) -> Result<(), E> {
    for field in [
        value.generation.lexical.repo_id.as_str(),
        value.generation.lexical.revision_id.as_str(),
        value.generation.semantic.repo_id.as_str(),
        value.generation.semantic.revision_id.as_str(),
        value.generation.lexical.manifest_digest.as_str(),
        value.generation.semantic.manifest_digest.as_str(),
        value.generation.semantic_content.row_root_digest.as_str(),
        value
            .generation
            .semantic_content
            .membership_root_digest
            .as_str(),
    ] {
        policy.bytes_v1(work_failure, field.len())?;
    }
    policy.work_v1(work_failure, 1)?;
    value.validate_v1().map_err(|cause| {
        *validation_failure = Some(cause);
        policy.semantic_v1(|| E::custom(cause))
    })
}

trait ExpectedActivePolicyV1 {
    type Error;
    fn head_v1(&mut self, head: &SearchCorpusActiveHeadV1) -> Result<(), Self::Error>;
    fn comparison_v1(&mut self, bytes: &[usize]) -> Result<(), Self::Error>;
    fn invalid_v1(&mut self, cause: Activation) -> Self::Error;
}

struct OrdinaryExpectedActiveV1;
impl ExpectedActivePolicyV1 for OrdinaryExpectedActiveV1 {
    type Error = Activation;
    fn head_v1(&mut self, head: &SearchCorpusActiveHeadV1) -> Result<(), Activation> {
        head.validate_v1()
            .map_err(Activation::ExpectedActiveIdentity)
    }
    fn comparison_v1(&mut self, _: &[usize]) -> Result<(), Activation> {
        Ok(())
    }
    fn invalid_v1(&mut self, cause: Activation) -> Activation {
        cause
    }
}

fn validate_expected_active_with_policy_v1<P: ExpectedActivePolicyV1>(
    candidate_repo: &RepoId,
    candidate_revision: &RevisionId,
    candidate_generation: ManifestGeneration,
    expected_active: Option<&SearchCorpusActiveHeadV1>,
    policy: &mut P,
) -> Result<(), P::Error> {
    let Some(expected_active) = expected_active else {
        return Ok(());
    };
    policy.head_v1(expected_active)?;
    let expected = &expected_active.generation.lexical;
    policy.comparison_v1(&[
        candidate_repo.as_str().len(),
        expected.repo_id.as_str().len(),
    ])?;
    if *candidate_repo != expected.repo_id {
        return Err(policy.invalid_v1(Activation::RepoMismatch));
    }
    policy.comparison_v1(&[
        candidate_revision.as_str().len(),
        expected.revision_id.as_str().len(),
    ])?;
    if *candidate_revision != expected.revision_id {
        return Err(policy.invalid_v1(Activation::RevisionMismatch));
    }
    policy.comparison_v1(&[])?;
    if candidate_generation.get() <= expected.manifest_generation.get() {
        return Err(policy.invalid_v1(Activation::CandidateGenerationMustAdvanceExpectedActive));
    }
    Ok(())
}

pub(in crate::ipc::control) fn validate_expected_active_scope_v1(
    candidate_repo: &RepoId,
    candidate_revision: &RevisionId,
    candidate_generation: ManifestGeneration,
    expected_active: Option<&SearchCorpusActiveHeadV1>,
) -> Result<(), Activation> {
    validate_expected_active_with_policy_v1(
        candidate_repo,
        candidate_revision,
        candidate_generation,
        expected_active,
        &mut OrdinaryExpectedActiveV1,
    )
}

#[cfg(feature = "quanta-native-identity-v1")]
mod native_v1 {
    use super::super::{
        HeadDataV1, NativeCorpusDecodeAdmissionV1, NativeCorpusDecodeDataV1, NativeCorpusPolicyV1,
    };
    use super::{
        Activation, CorpusPolicyV1, ExpectedActivePolicyV1, ManifestGeneration, RepoId, RevisionId,
        SearchCorpusActiveHeadV1, de, validate_expected_active_with_policy_v1,
        validate_head_with_policy_v1,
    };
    use crate::NativeIdentityDecodeDataRefusalV1 as Refusal;
    use crate::SearchPlaneActivateSearchCorpusGenerationCasRequest as CasRequest;
    use core::marker::PhantomData;

    struct NativeExpectedActiveV1<'data, P: NativeCorpusDecodeAdmissionV1 + ?Sized, E> {
        policy: NativeCorpusPolicyV1<'data, P>,
        state: &'data mut HeadDataV1<P::OriginalError, P::Funding>,
        marker: PhantomData<fn() -> E>,
    }
    impl<P: NativeCorpusDecodeAdmissionV1 + ?Sized, E: de::Error> ExpectedActivePolicyV1
        for NativeExpectedActiveV1<'_, P, E>
    {
        type Error = E;
        fn head_v1(&mut self, head: &SearchCorpusActiveHeadV1) -> Result<(), E> {
            let result = validate_head_with_policy_v1(
                head,
                &mut self.policy,
                &mut self.state.keys.failure,
                &mut self.state.validation_failure,
            );
            if let Some(cause) = self.state.validation_failure {
                self.state.activation_failure = Some(Activation::ExpectedActiveIdentity(cause));
            }
            result
        }
        fn comparison_v1(&mut self, bytes: &[usize]) -> Result<(), E> {
            self.policy.work_v1(&mut self.state.keys.failure, 1)?;
            for bytes in bytes {
                self.policy.bytes_v1(&mut self.state.keys.failure, *bytes)?;
            }
            Ok(())
        }
        fn invalid_v1(&mut self, cause: Activation) -> E {
            self.state.activation_failure = Some(cause);
            self.policy.semantic_v1(|| E::custom(cause))
        }
    }

    fn park_result_v1<E>(result: Result<(), E>, failure: &mut Option<E>) -> Result<(), Refusal> {
        match result {
            Ok(()) => Ok(()),
            Err(cause) => {
                *failure = Some(cause);
                Err(Refusal::OperationRefused)
            }
        }
    }

    impl SearchCorpusActiveHeadV1 {
        /// Validate a borrowed whole Head using the SAME decode-finisher work
        /// policy and predicates. No decode, identity copy, NFC scratch or birth.
        /// DATA retains complete original work/semantic causes; `failure` retains
        /// the SAME full dynamic error used by the decode policy. The caller owns
        /// this borrowed Head and its actual funding through the highest Source
        /// finisher. DATA retains no input/control/policy reference. A successful
        /// validation has no owned Head output and issues no Source authority.
        pub fn validate_into_with_native_admission_v1<
            P: NativeCorpusDecodeAdmissionV1 + ?Sized,
            E: de::Error,
        >(
            &self,
            admission: &mut P,
            data: &mut NativeCorpusDecodeDataV1<P::OriginalError, P::Funding>,
            failure: &mut Option<E>,
        ) -> Result<(), Refusal> {
            if failure.is_some() {
                return Err(Refusal::OccupiedOutput);
            }
            if !data.is_fresh_v1() {
                return Err(Refusal::UsedData);
            }
            data.attempted = true;
            let result = validate_head_with_policy_v1(
                self,
                &mut NativeCorpusPolicyV1(admission),
                &mut data.state.keys.failure,
                &mut data.state.validation_failure,
            );
            park_result_v1(result, failure)
        }
    }

    impl CasRequest {
        /// SAME expectation body as both ordinary APIs, over borrowed scope.
        /// Expected Head validation comes first, then repo/revision/strict
        /// generation advance. None performs no admission or candidate checks.
        /// One-shot DATA and every complete cause remain external; `failure`
        /// retains the SAME dynamic error policy as decode. Borrowed
        /// candidate/Head backing and its funding stay with the highest caller;
        /// no Snapshot, digest String, identity clone or authority is produced.
        pub fn validate_expected_active_scope_into_with_native_admission_v1<
            P: NativeCorpusDecodeAdmissionV1 + ?Sized,
            E: de::Error,
        >(
            candidate_repo: &RepoId,
            candidate_revision: &RevisionId,
            candidate_generation: ManifestGeneration,
            expected_active: Option<&SearchCorpusActiveHeadV1>,
            admission: &mut P,
            data: &mut NativeCorpusDecodeDataV1<P::OriginalError, P::Funding>,
            failure: &mut Option<E>,
        ) -> Result<(), Refusal> {
            if failure.is_some() {
                return Err(Refusal::OccupiedOutput);
            }
            if !data.is_fresh_v1() {
                return Err(Refusal::UsedData);
            }
            data.attempted = true;
            let result = validate_expected_active_with_policy_v1(
                candidate_repo,
                candidate_revision,
                candidate_generation,
                expected_active,
                &mut NativeExpectedActiveV1 {
                    policy: NativeCorpusPolicyV1(admission),
                    state: &mut data.state,
                    marker: PhantomData,
                },
            );
            park_result_v1(result, failure)
        }
    }
}
