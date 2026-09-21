#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod delta;
pub mod layout_v3;
pub mod materializer;
pub mod model;
mod object_store;
pub mod query;
pub mod reader;
pub mod store;

pub use delta::RepoMapDeltaApplier;
pub use layout_v3::{
    CandidateObjectAddressV1, ObservedFileMetadataV1, QuarantineIncidentAddressV1,
    QuarantinePayloadAddressV1, SecureMetadataPairV1, StateRootSecurityContextV1,
    StateRootSecurityVerificationErrorV1,
};
pub use materializer::{
    CandidateProjectionMetaV1, RepoMapGraphCompiler, RepoMapMaterializer, decode_compiled_payload,
    snapshot_from_projection,
};
pub use model::{RepoMapEntry, RepoMapIndexedSnapshot, RepoMapSnapshot, RepoMapSnapshotIndex};
pub use query::RepoMapQueryEngine;
pub use reader::RepoMapPinnedReader;
pub use store::{OpenedRepoMapStore, RepoMapGenerationStore};
