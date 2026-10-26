//! Bounded byte-string decoding into the existing `Vec<u8>` DTO field.

use core::fmt;

use serde::de::{self, DeserializeSeed, SeqAccess, Visitor};

use super::{DecodeBackingAdmissionV1, DecodeFieldGuardV1, DecodeFieldKindV1};

/// Decode bytes with the same byte-string and integer-sequence wire forms as
/// the ordinary source-byte visitor. The admission owner must outlive the Vec.
pub struct AdmittedByteStringSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_bytes: usize,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> AdmittedByteStringSeedV1<'a, A> {
    pub fn new(admission: &'a A, max_bytes: usize) -> Self {
        Self {
            admission,
            max_bytes,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedByteStringSeedV1<'_, A>
{
    type Value = Vec<u8>;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field =
            DecodeFieldGuardV1::enter(self.admission, DecodeFieldKindV1::Bytes, self.max_bytes)?;
        let result = decoder.deserialize_byte_buf(self);
        field.finish(result)
    }
}

struct RefuseExtraByte;

impl<'de> DeserializeSeed<'de> for RefuseExtraByte {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, _decoder: D) -> Result<(), D::Error> {
        Err(de::Error::custom("admitted bytes exceed their byte limit"))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'de> for AdmittedByteStringSeedV1<'_, A> {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded byte string or byte sequence")
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        if value.len() > self.max_bytes {
            return Err(E::custom("admitted bytes exceed their byte limit"));
        }
        let mut output = Vec::new();
        self.admission
            .reserve_bytes(&mut output, value.len())
            .map_err(E::custom)?;
        if output.capacity() < value.len() {
            return Err(E::custom(
                "admitted bytes backing is smaller than its input",
            ));
        }
        output.extend_from_slice(value);
        Ok(output)
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        if value.len() > self.max_bytes {
            return Err(E::custom("admitted bytes exceed their byte limit"));
        }
        // An empty Vec with zero capacity has no backing to verify or retain.
        // A parser-owned empty Vec with capacity still requires provenance.
        if value.capacity() == 0 {
            return Ok(value);
        }
        self.admission
            .verify_parser_owned_bytes(&value, value.capacity())
            .map_err(E::custom)?;
        Ok(value)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Self::Value, S::Error> {
        let mut output = Vec::new();
        loop {
            if output.len() == self.max_bytes {
                if sequence.next_element_seed(RefuseExtraByte)?.is_none() {
                    return Ok(output);
                }
                return Err(de::Error::custom("admitted bytes exceed their byte limit"));
            }
            let Some(byte) = sequence.next_element::<u8>()? else {
                return Ok(output);
            };
            self.admission
                .reserve_bytes(&mut output, 1)
                .map_err(de::Error::custom)?;
            if output.len() == output.capacity() {
                return Err(de::Error::custom(
                    "admitted bytes backing has no reserved slot",
                ));
            }
            output.push(byte);
        }
    }
}

#[cfg(test)]
mod tests;
