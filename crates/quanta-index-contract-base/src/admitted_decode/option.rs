//! Optional typed child decoding without an ordinary-deserialize fallback.

use core::fmt;

use serde::de::{self, DeserializeSeed, Visitor};

use super::DecodeBackingAdmissionV1;

/// Decode an existing `Option<T>` using exactly one admitted child seed.
pub struct AdmittedOptionalSeedV1<'a, A: ?Sized, S> {
    admission: &'a A,
    child: S,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized, S> AdmittedOptionalSeedV1<'a, A, S> {
    pub fn new(admission: &'a A, child: S) -> Self {
        Self { admission, child }
    }
}

impl<'de, A, S> DeserializeSeed<'de> for AdmittedOptionalSeedV1<'_, A, S>
where
    A: DecodeBackingAdmissionV1 + ?Sized,
    S: DeserializeSeed<'de>,
{
    type Value = Option<S::Value>;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_option(self);
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<'de, A, S> Visitor<'de> for AdmittedOptionalSeedV1<'_, A, S>
where
    A: DecodeBackingAdmissionV1 + ?Sized,
    S: DeserializeSeed<'de>,
{
    type Value = Option<S::Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional admitted value")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.child.deserialize(decoder).map(Some)
    }
}

#[cfg(test)]
mod tests;
