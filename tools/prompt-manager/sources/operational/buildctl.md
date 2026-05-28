# Control Plane

There is no build queue controller in this repo.

Current control-plane meaning (lives inline in `quanta-index-search-plane`, no separate `quanta-index-control` crate):
- `SearchPlaneControlDispatcher` (`crates/quanta-index-search-plane/src/control_dispatcher.rs`) — control IPC surface
- `ActivationCatalog` (`crates/quanta-index-search-plane/src/readiness.rs`) — `state_root/activations/*.json`, rehydrated by `ActivationCatalog::open`
- `SemanticAuthorityStore` (`crates/quanta-index-search-plane/src/ingest_dispatcher.rs`) — `state_root/semantic/journal.cbor`, replayed into HNSW via `replay_into` on boot
- `AuxiliaryAuthorityStore` (history / runtime-metadata / structural) — `state_root/authorities/`, restored via `restore_into(ledger)`
- readiness `Ledger` bootstrapped by `quanta-index-searchd::app::runtime::assemble` (`bootstrap_persisted_lexical_state`, `bootstrap_persisted_semantic_state`)
