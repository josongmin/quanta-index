//! Caller-owned normalization state and its funding lifetime.

use super::{
    Decomposition, NfcError, Recomposition, Scratch, ScratchAdmission, ScratchBuffers,
    next_composed, stream_nfc, work,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalizationOutcomeV1 {
    IsNfc(bool),
    Streamed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalizationDataRefusalV1 {
    UsedData,
    OperationRefused,
    OccupiedOutput,
    MissingResult,
}

/// One attempt's real backing and result. The caller retains its funding bank
/// until after this value is dropped or its scratch is explicitly released.
pub struct NormalizationDataV1<E> {
    // Real backing drops before an error that may itself retain a grant.
    buffers: ScratchBuffers,
    attempted: bool,
    released: bool,
    result: Option<Result<NormalizationOutcomeV1, NfcError<E>>>,
}

impl<E> Default for NormalizationDataV1<E> {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl<E> NormalizationDataV1<E> {
    #[must_use]
    pub fn new_v1() -> Self {
        Self {
            buffers: ScratchBuffers::default(),
            attempted: false,
            released: false,
            result: None,
        }
    }

    #[must_use]
    pub fn is_fresh_v1(&self) -> bool {
        !self.attempted && !self.released
    }

    pub fn result_v1(&self) -> Option<Result<NormalizationOutcomeV1, &NfcError<E>>> {
        self.result.as_ref().map(|result| match result {
            Ok(outcome) => Ok(*outcome),
            Err(error) => Err(error),
        })
    }

    pub fn result_into_slot_v1(
        &mut self,
        output: &mut Option<Result<NormalizationOutcomeV1, NfcError<E>>>,
    ) -> Result<(), NormalizationDataRefusalV1> {
        if output.is_some() {
            return Err(NormalizationDataRefusalV1::OccupiedOutput);
        }
        if self.result.is_none() {
            return Err(NormalizationDataRefusalV1::MissingResult);
        }
        *output = self.result.take();
        Ok(())
    }

    pub fn release_scratch_v1<A: ScratchAdmission<Error = E>>(&mut self, admission: &mut A) {
        if self.attempted && !self.released {
            self.released = true;
            self.buffers.release(admission);
        }
    }

    fn begin(&mut self) -> Result<(), NormalizationDataRefusalV1> {
        if !self.is_fresh_v1() {
            return Err(NormalizationDataRefusalV1::UsedData);
        }
        self.attempted = true;
        Ok(())
    }

    fn record(
        &mut self,
        result: Result<NormalizationOutcomeV1, NfcError<E>>,
    ) -> Result<(), NormalizationDataRefusalV1> {
        let succeeded = result.is_ok();
        self.result = Some(result);
        if succeeded {
            Ok(())
        } else {
            Err(NormalizationDataRefusalV1::OperationRefused)
        }
    }
}

/// Stream NFC into the caller's emitter while retaining backing and errors.
pub fn for_each_nfc_into_v1<A, F>(
    source: &str,
    data: &mut NormalizationDataV1<A::Error>,
    admission: &mut A,
    emit: F,
) -> Result<(), NormalizationDataRefusalV1>
where
    A: ScratchAdmission,
    F: FnMut(char) -> Result<(), A::Error>,
{
    data.begin()?;
    let result = {
        let mut scratch = Scratch::new(admission, &mut data.buffers);
        stream_nfc(source, &mut scratch, emit).map(|()| NormalizationOutcomeV1::Streamed)
    };
    data.record(result)
}

/// Compare an identity of at most 512 UTF-8 bytes against canonical NFC.
pub fn is_nfc_into_v1<A: ScratchAdmission>(
    source: &str,
    data: &mut NormalizationDataV1<A::Error>,
    admission: &mut A,
) -> Result<(), NormalizationDataRefusalV1> {
    data.begin()?;
    let result = {
        let mut scratch = Scratch::new(admission, &mut data.buffers);
        (|| {
            work(&mut scratch, 0)?;
            if source.len() > 512 {
                return Err(NfcError::InputTooLong);
            }
            compare_nfc(source, &mut scratch).map(NormalizationOutcomeV1::IsNfc)
        })()
    };
    data.record(result)
}

/// Compare borrowed text of any length under the caller's work and scratch policy.
/// The caller retains the data and all admitted backing through its finisher.
pub fn is_nfc_unbounded_into_v1<A: ScratchAdmission>(
    source: &str,
    data: &mut NormalizationDataV1<A::Error>,
    admission: &mut A,
) -> Result<(), NormalizationDataRefusalV1> {
    data.begin()?;
    let result = {
        let mut scratch = Scratch::new(admission, &mut data.buffers);
        compare_nfc_with_completion(source, &mut scratch).map(NormalizationOutcomeV1::IsNfc)
    };
    data.record(result)
}

pub(super) fn compare_nfc_with_completion<A: ScratchAdmission>(
    source: &str,
    scratch: &mut Scratch<'_, A>,
) -> Result<bool, NfcError<A::Error>> {
    work(scratch, 0)?;
    let canonical = compare_nfc(source, scratch)?;
    work(scratch, 0)?;
    Ok(canonical)
}

fn compare_nfc<A: ScratchAdmission>(
    source: &str,
    scratch: &mut Scratch<'_, A>,
) -> Result<bool, NfcError<A::Error>> {
    let mut original = source.chars();
    let mut decomposition = Decomposition {
        input: source.chars(),
        ready: 0..0,
    };
    let mut recomposition = Recomposition::default();
    loop {
        let next = next_composed(scratch, &mut decomposition, &mut recomposition)?;
        work(scratch, 1)?;
        match (next, original.next()) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) if left == right => {}
            _ => return Ok(false),
        }
    }
}
