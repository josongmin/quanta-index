# LDB-03 — Search-plane Runtime and Readiness Cutover

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [LDB-02-lance-persisted-semantic-adapter.md](LDB-02-lance-persisted-semantic-adapter.md)

## 1. Purpose

Cut the runtime over from semantic journal replay to persisted semantic
generation open semantics.

## 2. Owner files

- `crates/quanta-index-searchd-runtime/src/lib.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
- `crates/quanta-index-search-plane/src/readiness.rs`
- `crates/quanta-index-searchd-runtime/tests/...`

## 3. Required runtime changes

- stop boot-time `bootstrap_persisted_semantic_state(...).replay_into(...)`
- replace that path with semantic readiness seeding from persisted semantic
  generation directories/markers
- direct semantic ingest must update the durable adapter first, then update the
  ledger/readiness state
- runtime composition must no longer require `SemanticAuthorityStore` as the
  main semantic durability path

## 4. Work items

- define a semantic analogue to lexical readiness seeding from `state_root`
- remove or isolate `SemanticAuthorityStore` from steady-state runtime assembly
- ensure semantic readiness in the ledger reflects durable persisted-open truth,
  not "we wrote a journal row"
- keep activation semantics unchanged:
  - lexical and semantic both required where the track is selected
  - missing semantic durable state remains typed not-ready

## 5. Test plan

- restart test: persisted semantic generation becomes query-ready without
  replaying all prior batches
- readiness test: incomplete semantic generation does not appear ready on boot
- activation test: semantic track activation still fails closed if durable
  semantic state is missing or unsealed
- direct ingest test: failed durable write does not leave semantic readiness in
  a false-ready state

## 6. DoD

- searchd runtime no longer depends on semantic full replay for normal boot
- semantic readiness derives from persisted generation truth
- direct semantic ingest and restart semantics agree on the same durable source

## 7. Failure modes

- marking semantic ready before the persisted generation is actually openable
- leaving replay bootstrap as a hidden backup path
- splitting semantic readiness truth between ledger markers and stale journal
  rows
