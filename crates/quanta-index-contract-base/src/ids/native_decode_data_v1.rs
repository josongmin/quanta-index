//! External physical state for one canonical identity decode occurrence.

use super::NativeIdentityConstructionErrorV1;
use unicode_normalization::NativeNormalizationDataV1;

/// Status only. Full original construction failures remain in external DATA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIdentityDecodeDataRefusalV1 {
    UsedData,
    InvalidNativeProducer,
    OperationRefused,
    OccupiedOutput,
    MissingResult,
}

impl core::fmt::Display for NativeIdentityDecodeDataRefusalV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "native identity DATA refused: {self:?}")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum NativeIdentityDecodePhaseV1 {
    Fresh,
    Reading,
    Funding,
    Completed,
}

pub(super) struct NativeIdentityDecodeStateV1<T, E> {
    pub(super) backing: String,
    pub(super) output: Option<T>,
    pub(super) normalization: NativeNormalizationDataV1<E>,
    pub(super) failure: Option<NativeIdentityConstructionErrorV1<E>>,
    pub(super) phase: NativeIdentityDecodePhaseV1,
    pub(super) construction_attempted: bool,
}

/// One occurrence, containing actual backing and the full original error.
///
/// `F` is the caller's source-free, actual funding bank. It is filled only by
/// the transient admission adapter and drops after all physical state/errors.
/// No input borrow, control, callback, canonical-ID substitute or issuer is
/// stored. Every collection occurrence needs a distinct DATA loan. The caller
/// retains DATA outside its highest Source and drops it only after finishing.
pub struct NativeIdentityDecodeDataV1<T, E, F> {
    // Declaration order keeps real backing and complete causes ahead of grants.
    pub(super) state: NativeIdentityDecodeStateV1<T, E>,
    funding: Option<F>,
}

impl<T, E, F> NativeIdentityDecodeDataV1<T, E, F> {
    /// Empty physical state; neither a String nor funding bank is allocated.
    #[must_use]
    pub fn new_v1() -> Self {
        Self {
            state: NativeIdentityDecodeStateV1 {
                backing: String::new(),
                output: None,
                normalization: NativeNormalizationDataV1::new_v1(),
                failure: None,
                phase: NativeIdentityDecodePhaseV1::Fresh,
                construction_attempted: false,
            },
            funding: None,
        }
    }

    #[must_use]
    pub fn is_fresh_v1(&self) -> bool {
        self.state.phase == NativeIdentityDecodePhaseV1::Fresh
    }

    /// Observe the exact original failure; no formatting, mapping or clone.
    #[must_use]
    pub fn failure_v1(&self) -> Option<&NativeIdentityConstructionErrorV1<E>> {
        self.state.failure.as_ref()
    }

    /// Pure move to another external slot, or publication after the finisher.
    /// Physical normalization state and funding stay here until terminal drop.
    pub fn complete_into_slot_v1(
        &mut self,
        output: &mut Option<T>,
    ) -> Result<(), NativeIdentityDecodeDataRefusalV1> {
        if output.is_some() {
            return Err(NativeIdentityDecodeDataRefusalV1::OccupiedOutput);
        }
        if self.state.phase != NativeIdentityDecodePhaseV1::Completed
            || self.state.failure.is_some()
            || self.state.output.is_none()
        {
            return Err(NativeIdentityDecodeDataRefusalV1::MissingResult);
        }
        *output = self.state.output.take();
        Ok(())
    }

    pub(super) fn begin_v1(&mut self) -> bool {
        if !self.is_fresh_v1() {
            return false;
        }
        self.state.phase = NativeIdentityDecodePhaseV1::Reading;
        true
    }

    pub(super) fn is_complete_v1(&self) -> bool {
        self.state.output.is_some() && self.state.failure.is_none()
    }

    pub(super) fn loan_v1<'data, 'input>(
        &'data mut self,
        input: NativeIdentityDecodeInputV1<'input>,
    ) -> NativeIdentityDecodeLoanV1<'data, 'input, T, E, F> {
        NativeIdentityDecodeLoanV1 {
            state: &mut self.state,
            funding: &mut self.funding,
            input,
        }
    }
}

impl<T, E, F> Default for NativeIdentityDecodeDataV1<T, E, F> {
    fn default() -> Self {
        Self::new_v1()
    }
}

#[derive(Clone, Copy)]
pub(super) enum NativeIdentityDecodeInputV1<'input> {
    Borrowed(&'input str),
    Owned,
}

/// Transient wire-bound loan. Only the canonical visitor can create one.
/// Its borrowed input never enters DATA, and the caller cannot replace it.
pub struct NativeIdentityDecodeLoanV1<'data, 'input, T, E, F> {
    state: &'data mut NativeIdentityDecodeStateV1<T, E>,
    funding: &'data mut Option<F>,
    input: NativeIdentityDecodeInputV1<'input>,
}

impl<T, E, F> NativeIdentityDecodeLoanV1<'_, '_, T, E, F> {
    /// Loan physical state and its actual funding bank to one transient
    /// adapter. The runner retains the exact canonical visitor's wire input.
    pub fn with_funding_v1<R>(
        &mut self,
        visit: impl for<'data, 'input> FnOnce(
            &mut NativeIdentityDecodeRunnerV1<'data, 'input, T, E>,
            &mut Option<F>,
        ) -> R,
    ) -> Result<R, NativeIdentityDecodeDataRefusalV1> {
        if self.state.phase != NativeIdentityDecodePhaseV1::Reading
            || self.state.construction_attempted
            || self.state.output.is_some()
            || self.state.failure.is_some()
        {
            return Err(NativeIdentityDecodeDataRefusalV1::UsedData);
        }
        self.state.phase = NativeIdentityDecodePhaseV1::Funding;
        let mut runner = NativeIdentityDecodeRunnerV1 {
            state: &mut *self.state,
            input: self.input,
        };
        Ok(visit(&mut runner, &mut *self.funding))
    }
}

/// Transient canonical constructor. It exposes no raw backing/output setter
/// and accepts no substitute identity input or caller-supplied NFC verdict.
pub struct NativeIdentityDecodeRunnerV1<'data, 'input, T, E> {
    pub(super) state: &'data mut NativeIdentityDecodeStateV1<T, E>,
    pub(super) input: NativeIdentityDecodeInputV1<'input>,
}

impl<T, E> NativeIdentityDecodeRunnerV1<'_, '_, T, E> {
    /// Inspect the exact retained cause for a finite Serde-facing marker.
    #[must_use]
    pub fn failure_v1(&self) -> Option<&NativeIdentityConstructionErrorV1<E>> {
        self.state.failure.as_ref()
    }

    /// Park a full pre-construction admission refusal by pure transfer. On a
    /// used runner or missing cause the caller's error slot is preserved.
    pub fn refuse_admission_v1(
        &mut self,
        cause: &mut Option<E>,
    ) -> Result<(), NativeIdentityDecodeDataRefusalV1> {
        if self.state.construction_attempted || self.state.failure.is_some() {
            return Err(NativeIdentityDecodeDataRefusalV1::UsedData);
        }
        self.state.construction_attempted = true;
        let Some(cause) = cause.take() else {
            self.state.failure = Some(NativeIdentityConstructionErrorV1::Normalization(
                unicode_normalization::NativeNormalizationErrorV1::InvalidNativeProducer,
            ));
            return Err(NativeIdentityDecodeDataRefusalV1::InvalidNativeProducer);
        };
        self.state.failure = Some(NativeIdentityConstructionErrorV1::Normalization(
            unicode_normalization::NativeNormalizationErrorV1::Admission(cause),
        ));
        Err(NativeIdentityDecodeDataRefusalV1::OperationRefused)
    }

    pub(super) fn record_v1(
        &mut self,
        result: Result<(), NativeIdentityConstructionErrorV1<E>>,
    ) -> Result<(), NativeIdentityDecodeDataRefusalV1> {
        match result {
            Ok(()) => Ok(()),
            Err(cause) => {
                self.state.failure = Some(cause);
                Err(NativeIdentityDecodeDataRefusalV1::OperationRefused)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RepoId;
    #[test]
    fn loan_and_runner_are_one_attempt_and_preserve_full_refused_error_v1() {
        let mut data = NativeIdentityDecodeDataV1::<RepoId, Box<u8>, ()>::new_v1();
        assert!(data.begin_v1());
        let mut cause = Some(Box::new(83_u8));
        let pointer = core::ptr::from_ref(cause.as_ref().expect("cause").as_ref());
        let mut extra = Some(Box::new(89_u8));
        let mut loan = data.loan_v1(NativeIdentityDecodeInputV1::Borrowed("repo"));
        loan.with_funding_v1(|runner, funding| {
            *funding = Some(());
            assert_eq!(
                runner.refuse_admission_v1(&mut cause),
                Err(NativeIdentityDecodeDataRefusalV1::OperationRefused)
            );
            assert!(cause.is_none());
            assert_eq!(
                runner.refuse_admission_v1(&mut extra),
                Err(NativeIdentityDecodeDataRefusalV1::UsedData)
            );
        })
        .expect("first loan");
        let mut invoked = false;
        assert_eq!(
            loan.with_funding_v1(|_, _| {
                invoked = true;
            }),
            Err(NativeIdentityDecodeDataRefusalV1::UsedData)
        );
        assert!(!invoked);
        assert_eq!(extra.as_deref(), Some(&89));
        let Some(NativeIdentityConstructionErrorV1::Normalization(
            unicode_normalization::NativeNormalizationErrorV1::Admission(cause),
        )) = data.failure_v1()
        else {
            assert!(false, "full cause missing");
            return;
        };
        assert_eq!(core::ptr::from_ref(cause.as_ref()), pointer);
    }
    #[test]
    fn absent_original_error_poisoning_cannot_be_retried_v1() {
        let mut data = NativeIdentityDecodeDataV1::<RepoId, u8, ()>::new_v1();
        assert!(data.begin_v1());
        data.loan_v1(NativeIdentityDecodeInputV1::Borrowed("repo"))
            .with_funding_v1(|runner, _| {
                assert_eq!(
                    runner.refuse_admission_v1(&mut None),
                    Err(NativeIdentityDecodeDataRefusalV1::InvalidNativeProducer)
                );
                let mut cause = Some(7);
                assert_eq!(
                    runner.refuse_admission_v1(&mut cause),
                    Err(NativeIdentityDecodeDataRefusalV1::UsedData)
                );
                assert_eq!(cause, Some(7));
            })
            .expect("first loan");
        assert!(matches!(
            data.failure_v1(),
            Some(NativeIdentityConstructionErrorV1::Normalization(
                unicode_normalization::NativeNormalizationErrorV1::InvalidNativeProducer
            ))
        ));
    }
}
