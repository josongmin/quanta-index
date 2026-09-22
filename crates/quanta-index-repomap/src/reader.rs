//! The pinned `RepoMap` snapshot executor (S21-05).
//!
//! The handle and its RAII pin lease live in the private `pinned` module, the lower
//! module both this facade and `store` depend on; this module keeps the
//! historical `reader::PinnedRepoMapSnapshotV1` path working.

pub use crate::pinned::PinnedRepoMapSnapshotV1;
