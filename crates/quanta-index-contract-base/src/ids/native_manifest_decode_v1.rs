//! Unit decode of the canonical numeric generation, retaining wrong owned
//! wire input without implementing a second numeric validator.
use super::ManifestGeneration;
use super::NativeIdentityDecodeDataRefusalV1;
use crate::retained_scalar_decode_v1::{ScalarDataV1, decode_scalar_into_v1};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed},
};

/// Source-free physical state for one numeric generation occurrence.
pub struct NativeManifestGenerationDecodeDataV1 {
    state: ScalarDataV1<ManifestGeneration>,
    attempted: bool,
    completed: bool,
}
impl NativeManifestGenerationDecodeDataV1 {
    #[must_use]
    pub const fn new_v1() -> Self {
        Self {
            state: ScalarDataV1::new_v1(),
            attempted: false,
            completed: false,
        }
    }
    /// Pure move to an external parent slot, preserving an occupied slot.
    pub fn complete_into_slot_v1(
        &mut self,
        output: &mut Option<ManifestGeneration>,
    ) -> Result<(), NativeIdentityDecodeDataRefusalV1> {
        if output.is_some() {
            return Err(NativeIdentityDecodeDataRefusalV1::OccupiedOutput);
        }
        if !self.completed || self.state.output.is_none() {
            return Err(NativeIdentityDecodeDataRefusalV1::MissingResult);
        }
        *output = self.state.output.take();
        Ok(())
    }
}
impl Default for NativeManifestGenerationDecodeDataV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}
struct ManifestSeedV1<'a>(&'a mut NativeManifestGenerationDecodeDataV1);
impl<'de> DeserializeSeed<'de> for ManifestSeedV1<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.0.attempted {
            return Err(de::Error::custom(
                "native manifest generation DATA is already used",
            ));
        }
        self.0.attempted = true;
        decode_scalar_into_v1(d, &mut self.0.state, true)?;
        self.0.completed = true;
        Ok(())
    }
}
impl ManifestGeneration {
    /// SAME numeric visitor with an external unit result. Native input must
    /// support self-describing `deserialize_any` so rejected owned strings
    /// reach retained DATA before the canonical scalar visitor refuses them.
    /// Capture the returned full deserializer error outside the highest Source.
    pub fn native_decode_seed_v1<'de>(
        data: &mut NativeManifestGenerationDecodeDataV1,
    ) -> impl DeserializeSeed<'de, Value = ()> + '_ {
        ManifestSeedV1(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_numeric_decode_preserves_u64_boundaries_and_owned_wrong_type_v1() {
        for value in [0, 1, u64::MAX] {
            let mut data = NativeManifestGenerationDecodeDataV1::new_v1();
            ManifestGeneration::native_decode_seed_v1(&mut data)
                .deserialize(serde::de::value::U64Deserializer::<serde_json::Error>::new(
                    value,
                ))
                .expect("canonical number");
            let mut output = None;
            data.complete_into_slot_v1(&mut output).expect("pure move");
            assert_eq!(output.map(ManifestGeneration::get), Some(value));
        }
        let wire = String::from("owned-wrong-number");
        let pointer = wire.as_ptr();
        let mut data = NativeManifestGenerationDecodeDataV1::new_v1();
        assert!(
            ManifestGeneration::native_decode_seed_v1(&mut data)
                .deserialize(serde::de::value::StringDeserializer::<serde_json::Error>::new(wire))
                .is_err()
        );
        assert_eq!(
            data.state.refused_string.as_deref(),
            Some("owned-wrong-number")
        );
        assert_eq!(
            data.state
                .refused_string
                .as_ref()
                .expect("retained")
                .as_ptr(),
            pointer
        );
        let mut output = None;
        assert!(data.complete_into_slot_v1(&mut output).is_err());
    }
}
