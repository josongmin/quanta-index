//! Typed identity leaf admission for source-file DTO decoders.

use serde::Deserializer;

use super::DecodeBackingAdmissionV1;
use crate::{RepoId, RevisionId};

/// Adapt the existing native identity seed to a typed enclosing DTO field.
///
/// A production implementation must invoke `RepoId::native_decode_seed_v1`
/// or `RevisionId::native_decode_seed_v1` with a fresh external
/// `NativeIdentityDecodeDataV1` for each occurrence. The seed writes `()`, so
/// the implementation then calls `complete_into_slot_v1` to move the typed
/// result. It must retain each DATA, its original NFC producer and funding
/// through the returned DTO's lifetime. This port neither mints that authority
/// nor permits an ordinary constructor as an admission fallback.
pub trait SourceIdentityDecodeAdmissionV1: DecodeBackingAdmissionV1 {
    fn decode_repo_id_v1<'de, D: Deserializer<'de>>(&self, decoder: D) -> Result<RepoId, D::Error>;

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<RevisionId, D::Error>;
}
