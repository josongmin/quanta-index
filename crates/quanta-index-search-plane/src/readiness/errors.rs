//! Typed error codes the readiness authorities emit.

use quanta_index_contract::SearchPlaneErrorCodeV2 as Code;

pub(crate) const ERR_RUNTIME_CATALOG_STALE_BATCH: Code = Code::RuntimeCatalogStaleBatch;
pub(crate) const ERR_RUNTIME_CATALOG_CONFLICTING_BATCH: Code = Code::RuntimeCatalogConflictingBatch;
pub(crate) const ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID: Code = Code::RuntimeCatalogUnknownDocId;
