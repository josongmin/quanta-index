//! Search-plane readiness authority: the in-memory ledger, the durable
//! activation catalog, and the file-backed auxiliary authority store.
//!
//! Module map (dependencies point downward only):
//!
//! - `activation_catalog` — the query-time serve-head authority
//!   (`state_root/activations/`). Depends on `search_corpus_generation`,
//!   `pair_digest`, `durable_fs`, `errors`.
//! - `search_corpus_history` — the retention-bounded sealed search-corpus
//!   history the `AuxiliaryAuthorityStore` keeps under
//!   `authorities/search-corpus/`. Depends on `auxiliary_store` (the store
//!   it extends), `ledger`, `retention_receipt`, `search_corpus_generation`,
//!   `pair_digest`, `durable_fs`, `serde_support`, `errors`.
//! - `auxiliary_store` — opening the store, startup reconciliation, restore
//!   and legacy migration. Depends on `ledger`, the state modules, `keys`,
//!   `search_corpus_generation`, `pair_digest`, `durable_fs`.
//! - `ledger_apply` — how batches, deltas and channel ops land on the
//!   ledger's auxiliary states. Depends on `ledger`, the state modules.
//! - `ledger` — the in-memory `Ledger`. Depends on `aux_epoch`, the state
//!   modules, `keys`, `track_state`, `retention_receipt`, `errors`.
//! - `aux_epoch` — the epoch-named, retention-bounded snapshot registry
//!   of one auxiliary authority generation (QI-BB-020 W2). Leaf.
//! - `history_state`, `runtime_state`, `structural_state` — one authority
//!   state family each. Depend on `keys`, `track_state`, `serde_support`,
//!   `errors`.
//! - `search_corpus_generation` — the composite generation identity and its
//!   persisted root shape. Depends on `errors`.
//! - `keys`, `track_state`, `retention_receipt`, `pair_digest`, `durable_fs`,
//!   `serde_support`, `errors` — leaves.

mod activation_catalog;
mod aux_epoch;
mod auxiliary_store;
mod durable_fs;
mod errors;
mod history_state;
mod keys;
mod ledger;
mod ledger_apply;
mod pair_digest;
mod retention_receipt;
mod runtime_state;
mod search_corpus_generation;
mod search_corpus_history;
mod serde_support;
mod structural_state;
mod track_state;

pub use crate::search_corpus_retention::{
    PairIndexBytesMeasurer, SearchCorpusHistoryRetentionPolicyV1, SearchCorpusIndexBytesPort,
};

pub use activation_catalog::{ActivationCatalog, ActiveGenerationRecord};
pub use aux_epoch::{AuxEpochRefusedError, AuxRead};
pub use auxiliary_store::{
    AuxiliaryAuthorityStore, LegacyAuxiliaryMigrationReceipt, restore_auxiliary_rows_into,
};
#[cfg(test)]
pub(crate) use auxiliary_store::{ScriptedIndexBytesV1, TEST_INDEX_BYTES_PER_GENERATION};
pub use history_state::{HistoryAuthorityState, HistoryDiffKey};
pub(crate) use ledger::AuxDomainState;
pub use ledger::Ledger;
pub use retention_receipt::SearchCorpusHistoryRetentionReceiptV1;
pub use runtime_state::{ChangedDocState, DirtyDocState, DocFacetState, RuntimeMetadataState};
pub use search_corpus_generation::{
    PreparedSearchCorpusGenerationV1, SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1,
};
pub use search_corpus_history::SealedSearchCorpusAuthorityStateV1;
pub use structural_state::StructuralAuthorityState;
pub use track_state::TrackLedger;

#[cfg(test)]
pub(crate) use errors::{
    ERR_ROLLBACK_CAS_CONFLICT, ERR_SEMANTIC_GENERATION_NOT_SEALED,
    ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH,
};
pub(crate) use errors::{
    ERR_RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE, ERR_SEARCH_TRACK_GENERATION_NOT_SEALED,
};
pub(crate) use history_state::{
    HistoryDelta, HistoryStateMeta, RefChange, history_diff_search_text,
};
pub(crate) use pair_digest::{SEARCH_CORPUS_LOCK_STRIPES_V1, search_corpus_lock_stripe_v1};
pub(crate) use runtime_state::{
    RuntimeCatalogDelta, RuntimeDirtyDelta, RuntimeStateMeta, enforce_runtime_catalog_batch_order,
    validate_runtime_catalog_doc_ids,
};
pub(crate) use structural_state::{
    StructuralChunksDelta, StructuralStateMeta, StructuralTreesDelta,
    verify_parse_tree_against_chunk_map,
};
pub(crate) use track_state::TrackAuthorityState;

#[cfg(test)]
mod tests;
