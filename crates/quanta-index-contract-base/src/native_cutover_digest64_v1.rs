//! Canonical fixed 64-byte digest wire used by the Native cutover evidence.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverDigest64V1(pub [u8; 64]);

impl serde::Serialize for NativeCutoverDigest64V1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tuple = serializer.serialize_tuple(64)?;
        for byte in &self.0 {
            tuple.serialize_element(byte)?;
        }
        tuple.end()
    }
}

impl<'de> serde::Deserialize<'de> for NativeCutoverDigest64V1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DigestVisitor;
        impl<'de> serde::de::Visitor<'de> for DigestVisitor {
            type Value = NativeCutoverDigest64V1;
            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("exactly 64 digest bytes")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = [0_u8; 64];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = sequence
                        .next_element::<u8>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
                }
                if sequence.next_element::<u8>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(65, &self));
                }
                Ok(NativeCutoverDigest64V1(bytes))
            }
        }
        deserializer.deserialize_tuple(64, DigestVisitor)
    }
}
