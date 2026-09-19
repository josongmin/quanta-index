# Control Plane

There is no build queue controller in this repo.

Current control-plane meaning (lives inline in `quanta-index-search-plane`, no separate `quanta-index-control` crate):
- `SearchPlaneControlDispatcher` (`crates/quanta-index-search-plane/src/control_dispatcher.rs`) — control IPC surface
- `ActivationCatalog` (`crates/quanta-index-search-plane/src/readiness.rs`) — `state_root/activations/*.json`, rehydrated by `ActivationCatalog::open`
- semantic generations — persisted, generation-scoped under `state_root/indexes/semantic/{repo}/{revision}/g{generation}/` by `quanta-index-semantic`; opened directly from sealed durable state (no boot replay). Legacy `state_root/semantic/journal.cbor` is migration input only (`LegacySemanticJournalStore`, one-shot `migrate_legacy_semantic_journal` + `MIGRATED` marker)
- sealed-generation GC — a retired generation leaves its namespace by one durable rename into its track root's reserved `.reclaim/` area (`quanta_index_core::reclaim_directory`) before it is removed; boot and every reclaim pass finish what a crash left there (`SealedGenerationReclaimPort::finish_interrupted_reclaims`). Test and debug daemons exit at the seal or GC point `QUANTA_INDEX_CRASH_POINT` names, for the crash matrix (`quanta_index_search_plane::crash_point`)
- `AuxiliaryAuthorityStore` (history / runtime-metadata / structural) — `state_root/authorities/`, restored via `restore_into(ledger)`
- readiness `Ledger` bootstrapped by `quanta-index-searchd::app::runtime::assemble` (`bootstrap_persisted_lexical_state`; semantic via `semantic_boot::migrate_legacy_semantic_journal` then `semantic_boot::seed_persisted_semantic_readiness`)
