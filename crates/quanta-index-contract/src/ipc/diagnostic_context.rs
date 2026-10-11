//! Optional attempt correlation for query, control, and ingest requests.

use core::fmt;
use core::num::NonZeroU64;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Identifies one client send attempt across a request envelope.
///
/// The token has no product authority. Its fixed wire representation is 48
/// lowercase hexadecimal characters: 16 capture-id bytes followed by the
/// nonzero span ID as 8 big-endian bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiagnosticAttemptTokenV1 {
    capture_id: [u8; 16],
    attempt_span_id: NonZeroU64,
}

/// Terminal outcome of one concrete SDK transport send attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticAttemptOutcomeV1 {
    Ok,
    Error,
    Cancelled,
}

impl DiagnosticAttemptTokenV1 {
    #[must_use]
    pub const fn new(capture_id: [u8; 16], attempt_span_id: NonZeroU64) -> Self {
        Self {
            capture_id,
            attempt_span_id,
        }
    }

    /// Returns the stored capture identity, infallible by construction.
    #[must_use]
    pub const fn capture_id(self) -> [u8; 16] {
        self.capture_id
    }

    #[must_use]
    pub const fn attempt_span_id(self) -> NonZeroU64 {
        self.attempt_span_id
    }
}

impl fmt::Display for DiagnosticAttemptTokenV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.capture_id {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "{:016x}", self.attempt_span_id.get())
    }
}

impl Serialize for DiagnosticAttemptTokenV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct AttemptTokenVisitor;

impl Visitor<'_> for AttemptTokenVisitor {
    type Value = DiagnosticAttemptTokenV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a 48-character lowercase diagnostic attempt token")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.len() != 48 {
            return Err(E::invalid_length(value.len(), &self));
        }
        let mut bytes = [0_u8; 24];
        for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
            let [high, low] = pair else {
                return Err(E::invalid_length(pair.len(), &self));
            };
            let Some(high) = hex_nibble(*high) else {
                return Err(E::custom(
                    "diagnostic attempt token must be lowercase hexadecimal",
                ));
            };
            let Some(low) = hex_nibble(*low) else {
                return Err(E::custom(
                    "diagnostic attempt token must be lowercase hexadecimal",
                ));
            };
            *byte = (high << 4) | low;
        }
        let mut capture_id = [0_u8; 16];
        capture_id.copy_from_slice(&bytes[..16]);
        let mut span_bytes = [0_u8; 8];
        span_bytes.copy_from_slice(&bytes[16..]);
        let attempt_span_id = u64::from_be_bytes(span_bytes);
        let Some(attempt_span_id) = NonZeroU64::new(attempt_span_id) else {
            return Err(E::custom("diagnostic attempt span ID must be nonzero"));
        };
        Ok(DiagnosticAttemptTokenV1::new(capture_id, attempt_span_id))
    }
}

impl<'de> Deserialize<'de> for DiagnosticAttemptTokenV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(AttemptTokenVisitor)
    }
}

/// Parse outcome at the request boundary. Invalid and duplicate diagnostic
/// fields are fail-open for the product request and remain visible to the
/// server as an unlinked attempt with a health event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticAttemptFieldV1 {
    #[default]
    Absent,
    Linked(DiagnosticAttemptTokenV1),
    Invalid,
    Duplicate,
}

impl DiagnosticAttemptFieldV1 {
    #[must_use]
    pub const fn token(self) -> Option<DiagnosticAttemptTokenV1> {
        match self {
            Self::Linked(token) => Some(token),
            Self::Absent | Self::Invalid | Self::Duplicate => None,
        }
    }
}

/// Tolerant wrapper used only for the diagnostic field.
///
/// Any valid framed value
/// that is not the bounded token string becomes `Invalid`; request and payload
/// decoding retain their existing strict validation.
pub(crate) struct TolerantDiagnosticAttemptV1(pub DiagnosticAttemptFieldV1);

struct TolerantVisitor;

impl<'de> Visitor<'de> for TolerantVisitor {
    type Value = TolerantDiagnosticAttemptV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional diagnostic attempt token")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let parsed =
            serde::de::value::StrDeserializer::<E>::new(value).deserialize_str(AttemptTokenVisitor);
        Ok(TolerantDiagnosticAttemptV1(parsed.map_or(
            DiagnosticAttemptFieldV1::Invalid,
            DiagnosticAttemptFieldV1::Linked,
        )))
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while sequence.next_element::<de::IgnoredAny>()?.is_some() {}
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        while map
            .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
            .is_some()
        {}
        Ok(TolerantDiagnosticAttemptV1(
            DiagnosticAttemptFieldV1::Invalid,
        ))
    }
}

impl<'de> Deserialize<'de> for TolerantDiagnosticAttemptV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(TolerantVisitor)
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte
            .checked_sub(b'a')
            .and_then(|offset| offset.checked_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use super::{DiagnosticAttemptFieldV1, DiagnosticAttemptTokenV1, TolerantDiagnosticAttemptV1};

    #[test]
    fn token_uses_fixed_lowercase_hex_and_round_trips() {
        let token = DiagnosticAttemptTokenV1::new([0xab; 16], NonZeroU64::new(0x1234).unwrap());
        let encoded = serde_json::to_string(&token).expect("serialize token");
        assert_eq!(encoded, format!("\"{}0000000000001234\"", "ab".repeat(16)));
        assert_eq!(
            serde_json::from_str::<DiagnosticAttemptTokenV1>(&encoded).expect("valid token"),
            token
        );
        assert_eq!(encoded.len(), 50);
    }

    #[test]
    fn invalid_or_zero_id_tokens_are_rejected_by_the_strict_type() {
        for invalid in [
            "\"short\"",
            "\"AB0000000000000000000000000000000000000000000000\"",
            "\"000000000000000000000000000000000000000000000000\"",
        ] {
            assert!(serde_json::from_str::<DiagnosticAttemptTokenV1>(invalid).is_err());
        }
        assert_eq!(DiagnosticAttemptFieldV1::Absent.token(), None);
    }

    #[test]
    fn valid_frame_with_malformed_diagnostic_value_becomes_unlinked() {
        for invalid in ["null", "17", "[]", "{\"arbitrary\":true}"] {
            let decoded = serde_json::from_str::<TolerantDiagnosticAttemptV1>(invalid)
                .expect("diagnostic-only malformed value is tolerated");
            assert_eq!(decoded.0, DiagnosticAttemptFieldV1::Invalid, "{invalid}");
        }
    }
}
