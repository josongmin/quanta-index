//! Product-owned normalization admission and owned convenience operations.

use super::{
    NfcError, NormalizationDataV1, NormalizationOutcomeV1, ScratchAdmission, ScratchDemand,
    ScratchOwner, for_each_nfc_into_v1, is_nfc_into_v1,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "Field names preserve the existing admission contract during paired migration"
)]
pub struct NativeNormalizationScratchDemandV1 {
    pub owner_v1: ScratchOwner,
    pub current_bytes_v1: usize,
    pub new_bytes_v1: usize,
}

pub trait NativeNormalizationAdmissionV1 {
    type Error;

    fn checkpoint_work_v1(&mut self, units_v1: u64) -> Result<(), Self::Error>;
    fn native_birth_v1(
        &mut self,
        demand_v1: NativeNormalizationScratchDemandV1,
        birth_v1: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error>;
    fn release_scratch_v1(&mut self, owner_v1: ScratchOwner);
}

impl<P: NativeNormalizationAdmissionV1> ScratchAdmission for P {
    type Error = P::Error;

    fn checkpoint(&mut self, units: u64) -> Result<(), Self::Error> {
        self.checkpoint_work_v1(units)
    }

    fn birth(
        &mut self,
        demand: ScratchDemand,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        self.native_birth_v1(
            NativeNormalizationScratchDemandV1 {
                owner_v1: demand.owner,
                current_bytes_v1: demand.current_bytes,
                new_bytes_v1: demand.new_bytes,
            },
            birth,
        )
    }

    fn release(&mut self, owner: ScratchOwner) {
        self.release_scratch_v1(owner);
    }
}

fn finish_local<P: NativeNormalizationAdmissionV1>(
    mut data: NormalizationDataV1<P::Error>,
    admission: &mut P,
) -> Result<NormalizationOutcomeV1, NfcError<P::Error>> {
    let mut result = None;
    let transferred = data.result_into_slot_v1(&mut result);
    data.release_scratch_v1(admission);
    if transferred.is_err() {
        return Err(NfcError::InvalidNativeProducer);
    }
    result.ok_or(NfcError::InvalidNativeProducer)?
}

pub fn try_is_nfc_with_native_admission_v1<P: NativeNormalizationAdmissionV1>(
    input: &str,
    admission: &mut P,
) -> Result<bool, NfcError<P::Error>> {
    let mut data = NormalizationDataV1::new_v1();
    let _status = is_nfc_into_v1(input, &mut data, admission);
    match finish_local(data, admission)? {
        NormalizationOutcomeV1::IsNfc(value) => Ok(value),
        NormalizationOutcomeV1::Streamed => Err(NfcError::InvalidNativeProducer),
    }
}

pub fn try_for_each_nfc_with_native_admission_v1<P, F>(
    input: &str,
    admission: &mut P,
    emit: F,
) -> Result<(), NfcError<P::Error>>
where
    P: NativeNormalizationAdmissionV1,
    F: FnMut(char) -> Result<(), P::Error>,
{
    let mut data = NormalizationDataV1::new_v1();
    let _status = for_each_nfc_into_v1(input, &mut data, admission, emit);
    match finish_local(data, admission)? {
        NormalizationOutcomeV1::Streamed => Ok(()),
        NormalizationOutcomeV1::IsNfc(_) => Err(NfcError::InvalidNativeProducer),
    }
}
