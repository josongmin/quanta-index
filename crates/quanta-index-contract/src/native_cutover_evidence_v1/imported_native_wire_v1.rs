//! Compact FD6 transport for a completed historical Native FULL import.
//! Canonical request and journal identities still use their original DTOs.

use std::fmt;

use serde::{
    de::{
        value::{MapAccessDeserializer, SeqAccessDeserializer},
        Error as _, MapAccess, SeqAccess, VariantAccess, Visitor,
    },
    ser::{Error as _, SerializeStructVariant},
    Deserialize, Deserializer, Serialize, Serializer,
};

use super::{
    NativeCutoverDigest64V1, NativeCutoverEvidenceSourcePublicationBindingV1,
    NativeCutoverEvidenceTargetV1,
};
use crate::{
    index::{
        Digest, ImmutableOperation, NativeImportedNativeSourceCompactV1,
        NativeSourceOriginCompactV1, ObjectRef, OperationKey, OwnerMutationSet, SourceCut,
        SourceEvent, SourceOperationMode, SourceTextAdmission, SourceTransition, UpdateKind,
        UpdateRequest, ViewRef,
    },
    RepoId, RevisionId,
};

type Import = (
    NativeImportedNativeSourceCompactV1,
    NativeCutoverEvidenceTargetV1,
    UpdateRequest,
);

struct FixedBytes<const N: usize>([u8; N]);

impl<const N: usize> Serialize for FixedBytes<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de, const N: usize> Deserialize<'de> for FixedBytes<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ByteString<const N: usize>;

        impl<const N: usize> Visitor<'_> for ByteString<N> {
            type Value = FixedBytes<N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a {N}-byte string")
            }

            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                bytes
                    .try_into()
                    .map(FixedBytes)
                    .map_err(|_| E::invalid_length(bytes.len(), &self))
            }
        }

        deserializer.deserialize_bytes(ByteString::<N>)
    }
}

type Hash = FixedBytes<32>;
type Hex = FixedBytes<64>;
type Origin<R, V> = ExactTuple<(Hash, Hash, R, V, Hash, u64, Hash, Hash), 8>;
type Binding = ExactTuple<(Hex, Hex, Hex, u64), 4>;
type Head<R, V> = ExactTuple<
    (
        Origin<R, V>,
        Hash,
        OperationKey,
        Hash,
        Hash,
        Binding,
        u64,
        Hash,
        Hash,
        Hash,
    ),
    10,
>;
type Target = ExactTuple<
    (
        ViewRef,
        SourceCut,
        ObjectRef,
        Digest,
        SourceTextAdmission,
        Digest,
    ),
    6,
>;

struct ExactTuple<T, const N: usize>(T);

impl<T: Serialize, const N: usize> Serialize for ExactTuple<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for ExactTuple<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TupleVisitor<T, const N: usize>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for TupleVisitor<T, N> {
            type Value = ExactTuple<T, N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a {N}-element FD6 tuple")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                if seq.size_hint() != Some(N) {
                    return Err(A::Error::custom("invalid FD6 tuple length"));
                }
                T::deserialize(SeqAccessDeserializer::new(seq)).map(ExactTuple)
            }
        }

        deserializer.deserialize_tuple(N, TupleVisitor::<T, N>(std::marker::PhantomData))
    }
}

fn historical_request(
    target: &NativeCutoverEvidenceTargetV1,
    owners: OwnerMutationSet,
) -> Result<UpdateRequest, serde_json::Error> {
    let mut transition = SourceTransition {
        expected_source: target.expected_source,
        owners,
        events: Vec::with_capacity(1),
    };
    let operation = target.operation;
    transition
        .events
        .push(historical_event(operation, &transition)?);
    Ok(UpdateRequest {
        operation,
        expected_view: target.expected_view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn historical_event(
    operation: OperationKey,
    transition: &SourceTransition,
) -> Result<SourceEvent, serde_json::Error> {
    Ok(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    })
}

pub(super) fn serialize_variant<S: Serializer>(
    serializer: S,
    name: &'static str,
    index: u32,
    variant: &'static str,
    head: &NativeImportedNativeSourceCompactV1,
    target: &NativeCutoverEvidenceTargetV1,
    request: &UpdateRequest,
) -> Result<S::Ok, S::Error> {
    // Keep the historical JSON DTO and all of its identity hashes intact.
    if serializer.is_human_readable() {
        let mut record = serializer.serialize_struct_variant(name, index, variant, 3)?;
        record.serialize_field("head", head)?;
        record.serialize_field("target", target)?;
        record.serialize_field("request", request)?;
        return record.end();
    }
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err(S::Error::custom(
            "Native import request is not a first FULL",
        ));
    };
    let owners = transition.owners;
    if head.source_operation.key != target.operation
        || head.source_operation.source_mode != SourceOperationMode::Advance
        || target.source_mode != SourceOperationMode::Advance
        || target.event_predecessor != 0
        || target.event_sequence != 1
        || request.operation != target.operation
        || request.expected_view != target.expected_view
        || transition.expected_source != target.expected_source
        || transition.events.as_slice()
            != [historical_event(target.operation, transition).map_err(S::Error::custom)?]
    {
        return Err(S::Error::custom(
            "Native import request differs from its retained target",
        ));
    }
    let origin = &head.origin;
    let binding = head.source_binding;
    let packed_head: Head<_, _> = ExactTuple((
        ExactTuple((
            FixedBytes(origin.old_root_nonce.0),
            FixedBytes(origin.old_routes_ref.0 .0),
            &origin.repo_id,
            &origin.revision_id,
            FixedBytes(origin.old_source_head_sha256.0),
            origin.file_count,
            FixedBytes(origin.file_census_sha256.0),
            FixedBytes(origin.accepted_transcript_sha256.0),
        )),
        FixedBytes(head.candidate_root_nonce.0),
        target.operation,
        FixedBytes(head.source_operation.request.0 .0),
        FixedBytes(head.source_publication_sha256.0),
        ExactTuple((
            FixedBytes(binding.transition_digest.0),
            FixedBytes(binding.completion_pointer_digest.0),
            FixedBytes(binding.physical_target_source_digest.0),
            binding.target_snapshot_id,
        )),
        head.original_snapshot_id,
        FixedBytes(head.original_request_sha256.0),
        FixedBytes(head.new_source_manifest_sha256.0),
        FixedBytes(head.completed_journal_digest.0),
    ));
    let packed_target: Target = ExactTuple((
        target.expected_view,
        target.expected_source,
        target.expected_profile,
        target.owner_identity_policy_digest,
        target.text_admission,
        target.canonical_target_digest,
    ));
    serializer.serialize_newtype_variant(
        name,
        index,
        variant,
        &(packed_head, packed_target, owners),
    )
}

struct LegacyImport {
    head: NativeImportedNativeSourceCompactV1,
    target: NativeCutoverEvidenceTargetV1,
    request: UpdateRequest,
}
crate::native_serde_struct!(LegacyImport {
    head: NativeImportedNativeSourceCompactV1,
    target: NativeCutoverEvidenceTargetV1,
    request: UpdateRequest
});

struct DecodedImport(Import);

impl<'de> Deserialize<'de> for DecodedImport {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ImportBody;

        impl<'de> Visitor<'de> for ImportBody {
            type Value = DecodedImport;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a compact Native import or its historical named record")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let LegacyImport {
                    head,
                    target,
                    request,
                } = LegacyImport::deserialize(MapAccessDeserializer::new(map))?;
                Ok(DecodedImport((head, target, request)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let ExactTuple((
                    origin,
                    nonce,
                    operation,
                    request_ref,
                    publication,
                    binding,
                    snapshot,
                    request_hash,
                    manifest,
                    completion,
                )): Head<RepoId, RevisionId> = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(0, &self))?;
                let ExactTuple((view, source, profile, policy, text, target_digest)): Target = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(1, &self))?;
                let owners = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(2, &self))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(4, &self));
                }
                let ExactTuple((
                    old_nonce,
                    routes,
                    repo_id,
                    revision_id,
                    old_head,
                    file_count,
                    census,
                    transcript,
                )) = origin;
                let ExactTuple((transition, pointer, physical, target_snapshot)) = binding;
                let head = NativeImportedNativeSourceCompactV1 {
                    origin: NativeSourceOriginCompactV1 {
                        old_root_nonce: Digest(old_nonce.0),
                        old_routes_ref: ObjectRef(Digest(routes.0)),
                        repo_id,
                        revision_id,
                        old_source_head_sha256: Digest(old_head.0),
                        file_count,
                        file_census_sha256: Digest(census.0),
                        accepted_transcript_sha256: Digest(transcript.0),
                    },
                    candidate_root_nonce: Digest(nonce.0),
                    source_operation: ImmutableOperation {
                        key: operation,
                        request: ObjectRef(Digest(request_ref.0)),
                        source_mode: SourceOperationMode::Advance,
                    },
                    source_publication_sha256: Digest(publication.0),
                    source_binding: NativeCutoverEvidenceSourcePublicationBindingV1 {
                        transition_digest: NativeCutoverDigest64V1(transition.0),
                        completion_pointer_digest: NativeCutoverDigest64V1(pointer.0),
                        physical_target_source_digest: NativeCutoverDigest64V1(physical.0),
                        target_snapshot_id: target_snapshot,
                    },
                    original_snapshot_id: snapshot,
                    original_request_sha256: Digest(request_hash.0),
                    new_source_manifest_sha256: Digest(manifest.0),
                    completed_journal_digest: Digest(completion.0),
                };
                let target = NativeCutoverEvidenceTargetV1 {
                    operation,
                    expected_view: view,
                    expected_source: source,
                    expected_profile: profile,
                    source_mode: SourceOperationMode::Advance,
                    event_predecessor: 0,
                    event_sequence: 1,
                    owner_identity_policy_digest: policy,
                    text_admission: text,
                    canonical_target_digest: target_digest,
                };
                let request = historical_request(&target, owners).map_err(A::Error::custom)?;
                Ok(DecodedImport((head, target, request)))
            }
        }

        deserializer.deserialize_any(ImportBody)
    }
}

pub(super) fn deserialize_variant<'de, A: VariantAccess<'de>>(
    variant: A,
) -> Result<Import, A::Error> {
    variant
        .newtype_variant::<DecodedImport>()
        .map(|decoded| decoded.0)
}
