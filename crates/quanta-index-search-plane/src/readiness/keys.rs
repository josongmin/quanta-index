//! Authority map keys: `(repo, revision, generation)` and per-track variants.

use std::fmt;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::serde_support::impl_struct_serde;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct AuthorityKey {
    pub(super) repo_id: RepoId,
    pub(super) revision_id: RevisionId,
    pub(super) generation: ManifestGeneration,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TrackAuthorityKey {
    pub(super) repo_id: RepoId,
    pub(super) revision_id: RevisionId,
    pub(super) track: SearchPlaneTrackKind,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TrackGenerationKey {
    pub(super) repo_id: RepoId,
    pub(super) revision_id: RevisionId,
    pub(super) track: SearchPlaneTrackKind,
    pub(super) generation: ManifestGeneration,
}

impl_struct_serde!(AuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
});

impl_struct_serde!(TrackAuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
});
