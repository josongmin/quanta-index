//! Wire identities shared by cutover evidence and the Source restore manifest.

use crate::index::{
    Digest, ObjectRef, OperationKey, SourceCut, SourceOperationMode, SourceTextAdmission, ViewRef,
};
use quanta_index_contract_base::NativeCutoverDigest64V1;

/// An authenticated signed-store target. `expected_source` is the prior CAS
/// cut; a Q receiver must independently read the actually Published `SourceCut`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceTargetV1 {
    pub operation: OperationKey,
    pub expected_view: ViewRef,
    pub expected_source: SourceCut,
    pub expected_profile: ObjectRef,
    pub source_mode: SourceOperationMode,
    pub event_predecessor: u64,
    pub event_sequence: u64,
    pub owner_identity_policy_digest: Digest,
    pub text_admission: SourceTextAdmission,
    pub canonical_target_digest: Digest,
}
crate::native_serde_struct!(NativeCutoverEvidenceTargetV1 {
    operation: OperationKey,
    expected_view: ViewRef,
    expected_source: SourceCut,
    expected_profile: ObjectRef,
    source_mode: SourceOperationMode,
    event_predecessor: u64,
    event_sequence: u64,
    owner_identity_policy_digest: Digest,
    text_admission: SourceTextAdmission,
    canonical_target_digest: Digest
});

/// The existing producer's committed Source association. Ordinary Replay and
/// Native initial and completed Watch facts reuse these exact axes and wire.
/// Terminal/ACK and Q publication identities remain on their existing facts.
/// Decoding this tuple never proves that its producer or publication is genuine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceSourcePublicationBindingV1 {
    pub transition_digest: NativeCutoverDigest64V1,
    pub completion_pointer_digest: NativeCutoverDigest64V1,
    pub physical_target_source_digest: NativeCutoverDigest64V1,
    pub target_snapshot_id: u64,
}
crate::native_serde_struct!(NativeCutoverEvidenceSourcePublicationBindingV1 {
    transition_digest: NativeCutoverDigest64V1,
    completion_pointer_digest: NativeCutoverDigest64V1,
    physical_target_source_digest: NativeCutoverDigest64V1,
    target_snapshot_id: u64
});
