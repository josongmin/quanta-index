//! Unit access to the existing retained scalar decoder for closed wire leaves.

use super::{ScalarDataV1, decode_scalar_into_v1};
use crate::NativeIdentityDecodeDataRefusalV1 as Refusal;
use serde::{
    Deserializer,
    de::{self, DeserializeOwned, DeserializeSeed},
};

mod sealed {
    pub trait Leaf {}
    impl Leaf for u32 {}
    impl Leaf for u64 {}
    impl Leaf for [u8; 32] {}
}

/// Closed canonical wire leaves supported by the retained unit decoder.
///
/// Their existing Serde visitors stop at the first invalid owned string/bytes.
/// The byte array visits only u8 elements. Arbitrary nested Copy values could
/// accept multiple owned inputs and overwrite the private retained slots, so
/// this trait is sealed rather than blanket-implemented for Deserialize.
pub trait NativeRetainedScalarLeafV1: sealed::Leaf + DeserializeOwned + Copy {}
impl NativeRetainedScalarLeafV1 for u32 {}
impl NativeRetainedScalarLeafV1 for u64 {}
impl NativeRetainedScalarLeafV1 for [u8; 32] {}

/// Source-free physical DATA for one closed leaf occurrence.
///
/// Owned wrong-type input enters the SAME private scalar storage before the
/// SAME Serde visitor observes a borrowed view. No input borrow, Source,
/// policy, callback, or allocator is retained. Full deserializer errors belong
/// in a separate external slot, filled by `try_decode_into_v1`. Retain both
/// slots through the highest Source finisher; a unit result issues no authority.
/// Every leaf occurrence needs its own DATA.
///
/// ```
/// use quanta_index_contract_base::NativeRetainedScalarDecodeDataV1;
/// let data = NativeRetainedScalarDecodeDataV1::<u64>::new_v1();
/// assert!(data.is_fresh_v1());
/// ```
///
/// Borrowed values cannot escape through the output:
/// ```compile_fail
/// use quanta_index_contract_base::NativeRetainedScalarDecodeDataV1;
/// let _ = NativeRetainedScalarDecodeDataV1::<&str>::new_v1();
/// ```
/// Nested Copy values are also outside the closed wire-leaf contract:
/// ```compile_fail
/// use quanta_index_contract_base::NativeRetainedScalarDecodeDataV1;
/// let _ = NativeRetainedScalarDecodeDataV1::<(u64, u64)>::new_v1();
/// ```
pub struct NativeRetainedScalarDecodeDataV1<T: NativeRetainedScalarLeafV1> {
    state: ScalarDataV1<T>,
    attempted: bool,
    completed: bool,
}

impl<T: NativeRetainedScalarLeafV1> NativeRetainedScalarDecodeDataV1<T> {
    #[must_use]
    pub const fn new_v1() -> Self {
        Self {
            state: ScalarDataV1::new_v1(),
            attempted: false,
            completed: false,
        }
    }

    #[must_use]
    pub const fn is_fresh_v1(&self) -> bool {
        !self.attempted
    }

    /// Read the actual parked owned input without cloning or formatting it.
    #[must_use]
    pub fn retained_string_v1(&self) -> Option<&str> {
        self.state.refused_string.as_deref()
    }

    /// Read the actual parked owned input without cloning or formatting it.
    #[must_use]
    pub fn retained_bytes_v1(&self) -> Option<&[u8]> {
        self.state.refused_bytes.as_deref()
    }

    /// Pure output move. Retained wire backing stays in DATA until terminal drop.
    pub fn complete_into_slot_v1(&mut self, output: &mut Option<T>) -> Result<(), Refusal> {
        if output.is_some() {
            return Err(Refusal::OccupiedOutput);
        }
        if !self.completed || self.state.output.is_none() {
            return Err(Refusal::MissingResult);
        }
        *output = self.state.output.take();
        Ok(())
    }

    /// Decode into external DATA and park the complete original `D::Error`
    /// before returning a finite status. Used DATA and occupied errors reject
    /// before any decoder poll, preserving existing state and original cause.
    pub fn try_decode_into_v1<'de, D: Deserializer<'de>>(
        &mut self,
        deserializer: D,
        failure: &mut Option<D::Error>,
    ) -> Result<(), Refusal> {
        if failure.is_some() {
            return Err(Refusal::OccupiedOutput);
        }
        if !self.is_fresh_v1() {
            return Err(Refusal::UsedData);
        }
        match self.native_decode_seed_v1().deserialize(deserializer) {
            Ok(()) => Ok(()),
            Err(cause) => {
                *failure = Some(cause);
                Err(Refusal::OperationRefused)
            }
        }
    }

    /// Unit seed over the SAME private decoder and scalar visitors.
    ///
    /// Native input must support self-describing `deserialize_any`. Its error
    /// type owns the dynamic error policy, including invalid type/length and
    /// the finite used-DATA marker; this API adds no diagnostic formatting or
    /// replacement scalar validator. If calling the seed directly, move its
    /// complete error into external DATA before finishing Source.
    pub fn native_decode_seed_v1<'de>(&mut self) -> impl DeserializeSeed<'de, Value = ()> + '_ {
        NativeScalarSeedV1(self)
    }
}

impl<T: NativeRetainedScalarLeafV1> Default for NativeRetainedScalarDecodeDataV1<T> {
    fn default() -> Self {
        Self::new_v1()
    }
}

struct NativeScalarSeedV1<'data, T: NativeRetainedScalarLeafV1>(
    &'data mut NativeRetainedScalarDecodeDataV1<T>,
);

impl<'de, T: NativeRetainedScalarLeafV1> DeserializeSeed<'de> for NativeScalarSeedV1<'_, T> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if !self.0.is_fresh_v1() {
            return Err(de::Error::custom(Refusal::UsedData));
        }
        self.0.attempted = true;
        decode_scalar_into_v1(deserializer, &mut self.0.state, true)?;
        self.0.completed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
