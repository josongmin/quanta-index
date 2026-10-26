//! The selected payload and provenance of one ranked native semantic member.
//!
//! This type is shared by daemon and SDK. It carries a binding, not a
//! source excerpt; consumers must resolve `payload` through the selected view.

use crate::index::{Digest, ObjectRef, OwnerInputBinding};

const MAX_SEMANTIC_CHUNK_PAYLOAD_BYTES: u64 = 16_777_216;

/// Half-open byte span or rendered member ordinal of the selected payload.
///
/// `[0, 0)` is the only permitted empty span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSemanticChunkSpan {
    SourcePayload { start_byte: u64, end_byte: u64 },
    ProjectionPayload { start_byte: u64, end_byte: u64 },
    RenderedMember { ordinal: u32 },
}
quanta_index_contract_base::native_serde_enum!(NativeSemanticChunkSpan {
    units {}
    newtypes {}
    records {
        SourcePayload = 0 { start_byte: u64, end_byte: u64 },
        ProjectionPayload = 1 { start_byte: u64, end_byte: u64 },
        RenderedMember = 2 { ordinal: u32 }
    }
});

/// Provenance for one ranked semantic vector.
///
/// `id` is the 32-byte logical vector ID in the selected view.
/// `input_digest` commits to the selected UTF-8 span or rendered member text; validation of
/// that digest requires a read of the selected payload by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSemanticChunk {
    pub id: Digest,
    pub payload: ObjectRef,
    pub input: OwnerInputBinding,
    pub input_digest: Digest,
    pub span: NativeSemanticChunkSpan,
}
quanta_index_contract_base::native_serde_struct!(NativeSemanticChunk {
    id: Digest,
    payload: ObjectRef,
    input: OwnerInputBinding,
    input_digest: Digest,
    span: NativeSemanticChunkSpan,
});

/// Why a semantic chunk failed structural binding validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSemanticChunkValidationError {
    IdMismatch,
    RecipeMismatch,
    SpanBounds,
    InputPolicyMismatch,
}

impl std::fmt::Display for NativeSemanticChunkValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::IdMismatch => "semantic chunk identity differs from the ranked vector ID",
            Self::RecipeMismatch => {
                "semantic chunk derivation policy differs from the selected recipe"
            }
            Self::SpanBounds => "semantic chunk span is outside the bounded selected payload",
            Self::InputPolicyMismatch => {
                "semantic chunk input binding does not match its payload domain"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NativeSemanticChunkValidationError {}

impl NativeSemanticChunk {
    /// Validate structural binding before exposing a ranked hit.
    ///
    /// This check does not authenticate object contents. The selected-view
    /// reader must separately prove `payload`, UTF-8 boundaries and digest.
    pub fn validate_binding(
        &self,
        vector_id: &[u8],
        recipe: Digest,
    ) -> Result<(), NativeSemanticChunkValidationError> {
        if vector_id != self.id.0.as_slice() {
            return Err(NativeSemanticChunkValidationError::IdMismatch);
        }
        if self.input.derivation_policy != recipe {
            return Err(NativeSemanticChunkValidationError::RecipeMismatch);
        }
        let (start_byte, end_byte) = match self.span {
            NativeSemanticChunkSpan::SourcePayload {
                start_byte,
                end_byte,
            } => {
                if self.input.source.is_none()
                    || self.input.projection.is_some()
                    || self.input.producer_context != Digest([0; 32])
                {
                    return Err(NativeSemanticChunkValidationError::InputPolicyMismatch);
                }
                (start_byte, end_byte)
            }
            NativeSemanticChunkSpan::ProjectionPayload {
                start_byte,
                end_byte,
            } => {
                if self.input.projection.is_none() {
                    return Err(NativeSemanticChunkValidationError::InputPolicyMismatch);
                }
                (start_byte, end_byte)
            }
            NativeSemanticChunkSpan::RenderedMember { ordinal } => {
                if self.input.projection.is_none() {
                    return Err(NativeSemanticChunkValidationError::InputPolicyMismatch);
                }
                if ordinal >= 4096 {
                    return Err(NativeSemanticChunkValidationError::SpanBounds);
                }
                return Ok(());
            }
        };
        if start_byte > end_byte
            || end_byte > MAX_SEMANTIC_CHUNK_PAYLOAD_BYTES
            || (start_byte == end_byte && start_byte != 0)
        {
            return Err(NativeSemanticChunkValidationError::SpanBounds);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("condition failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!(
                    "values differ: {} and {}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
        }};
    }

    fn source_chunk() -> NativeSemanticChunk {
        NativeSemanticChunk {
            id: Digest([1; 32]),
            payload: ObjectRef(Digest([2; 32])),
            input: OwnerInputBinding {
                source: Some(ObjectRef(Digest([3; 32]))),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: Digest([4; 32]),
            },
            input_digest: Digest([5; 32]),
            span: NativeSemanticChunkSpan::SourcePayload {
                start_byte: 0,
                end_byte: 4,
            },
        }
    }

    #[test]
    fn semantic_chunk_span_has_fixed_json_and_cbor_tags() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixtures = [
            (
                NativeSemanticChunkSpan::SourcePayload {
                    start_byte: 0,
                    end_byte: 4,
                },
                r#"{"SourcePayload":{"start_byte":0,"end_byte":4}}"#,
                b"\xa1\x6dSourcePayload\xa2\x6astart_byte\x00\x68end_byte\x04".as_slice(),
            ),
            (
                NativeSemanticChunkSpan::ProjectionPayload {
                    start_byte: 0,
                    end_byte: 4,
                },
                r#"{"ProjectionPayload":{"start_byte":0,"end_byte":4}}"#,
                b"\xa1\x71ProjectionPayload\xa2\x6astart_byte\x00\x68end_byte\x04".as_slice(),
            ),
        ];
        for (span, json_gold, cbor_gold) in fixtures {
            check_eq!(serde_json::to_string(&span)?, json_gold);
            check_eq!(
                serde_json::from_str::<NativeSemanticChunkSpan>(json_gold)?,
                span
            );
            let mut encoded = Vec::new();
            ciborium::ser::into_writer(&span, &mut encoded)?;
            check_eq!(encoded.as_slice(), cbor_gold);
            check_eq!(
                ciborium::de::from_reader::<NativeSemanticChunkSpan, _>(cbor_gold)?,
                span
            );
        }
        for bad in [
            r#"{"SourcePayload":{"start_byte":0}}"#,
            r#"{"SourcePayload":{"start_byte":0,"end_byte":4,"other":1}}"#,
            r#"{"SourcePayload":{"start_byte":0,"start_byte":0,"end_byte":4}}"#,
            r#"{"Unknown":{"start_byte":0,"end_byte":4}}"#,
        ] {
            check!(serde_json::from_str::<NativeSemanticChunkSpan>(bad).is_err());
        }
        Ok(())
    }

    #[test]
    fn semantic_chunk_record_preserves_named_binding_in_json_and_cbor()
    -> Result<(), Box<dyn std::error::Error>> {
        let chunk = source_chunk();
        check_eq!(
            serde_json::to_value(chunk)?,
            json!({
                "id": ([1_u8; 32]),
                "payload": ([2_u8; 32]),
                "input": {
                    "source": ([3_u8; 32]),
                    "projection": null,
                    "producer_context": ([0_u8; 32]),
                    "derivation_policy": ([4_u8; 32])
                },
                "input_digest": ([5_u8; 32]),
                "span": {"SourcePayload": {"start_byte": 0, "end_byte": 4}}
            })
        );
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&chunk, &mut encoded)?;
        check_eq!(
            ciborium::de::from_reader::<NativeSemanticChunk, _>(encoded.as_slice())?,
            chunk
        );
        Ok(())
    }

    #[test]
    fn semantic_chunk_binding_rejects_id_recipe_policy_and_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        let chunk = source_chunk();
        let vector_id = [1_u8; 32];
        let recipe = Digest([4; 32]);
        check_eq!(chunk.validate_binding(&vector_id, recipe), Ok(()));
        check_eq!(
            chunk.validate_binding(&[1_u8; 31], recipe),
            Err(NativeSemanticChunkValidationError::IdMismatch)
        );
        let mut wrong_id = vector_id;
        let Some(first_byte) = wrong_id.get_mut(0) else {
            return Err("fixed vector ID has no first byte".into());
        };
        *first_byte = 2;
        check_eq!(
            chunk.validate_binding(&wrong_id, recipe),
            Err(NativeSemanticChunkValidationError::IdMismatch)
        );
        check_eq!(
            chunk.validate_binding(&vector_id, Digest([6; 32])),
            Err(NativeSemanticChunkValidationError::RecipeMismatch)
        );
        let mut invalid = chunk;
        invalid.span = NativeSemanticChunkSpan::SourcePayload {
            start_byte: 5,
            end_byte: 4,
        };
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::SpanBounds)
        );
        invalid.span = NativeSemanticChunkSpan::SourcePayload {
            start_byte: 0,
            end_byte: 16_777_217,
        };
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::SpanBounds)
        );
        invalid.span = NativeSemanticChunkSpan::SourcePayload {
            start_byte: 1,
            end_byte: 1,
        };
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::SpanBounds)
        );
        invalid.span = NativeSemanticChunkSpan::SourcePayload {
            start_byte: 0,
            end_byte: 0,
        };
        check_eq!(invalid.validate_binding(&vector_id, recipe), Ok(()));
        invalid.input.producer_context = Digest([7; 32]);
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::InputPolicyMismatch)
        );
        invalid.input.producer_context = Digest([0; 32]);
        invalid.input.source = None;
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::InputPolicyMismatch)
        );
        invalid.span = NativeSemanticChunkSpan::ProjectionPayload {
            start_byte: 0,
            end_byte: 4,
        };
        check_eq!(
            invalid.validate_binding(&vector_id, recipe),
            Err(NativeSemanticChunkValidationError::InputPolicyMismatch)
        );
        invalid.input.projection = Some(ObjectRef(Digest([8; 32])));
        check_eq!(invalid.validate_binding(&vector_id, recipe), Ok(()));
        invalid.input.source = Some(ObjectRef(Digest([3; 32])));
        check_eq!(invalid.validate_binding(&vector_id, recipe), Ok(()));
        Ok(())
    }
}
