//! Persisted Legacy RepoMap member identifiers retained for old-root verification.

use crate::index::{IndexKind, SchemaId, StorageFormatId};

pub const NATIVE_LEGACY_REPOMAP_KIND_V1: IndexKind = IndexKind(*b"qidx.rmap.lg1!!!");
pub const NATIVE_LEGACY_REPOMAP_SCHEMA_V1: SchemaId = SchemaId(*b"qidx.rmap.lgs1!!");
pub const NATIVE_LEGACY_REPOMAP_FORMAT_V1: StorageFormatId = StorageFormatId(*b"qidx.rmap.lgf1!!");
