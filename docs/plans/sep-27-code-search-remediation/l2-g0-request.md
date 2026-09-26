# L2 common-contract request

Status: **Historical G0 request, superseded by [L2_HANDOFF.md](handoffs/L2_HANDOFF.md)**.
The live contract and production implementation have since changed. Current
behavior remains NOT_RUN until the serialized owner checks execute. The proposals
below are retained as coordination history, not the active implementation spec.

Inspection started at `66cee47efdda7c5f3886ac58690aa645f44f691f`; concurrent work
moved HEAD to `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`. L2 owner source paths
were initially clean. No agent was created, and no commit, push, reset, or heavy
Rust command was run by L2 at this stage.

L0 is task `01a0dea8-aa8e-7d73-857f-b174f7be64fe`. L0 granted L2 an exclusive
lease to author `crates/quanta-index-contract/src/source_coverage.rs`; root exports,
base source identity and ingest DTO cutover remain L0-owned. L2 authored the strict
coverage/event codec, unit-set digest helper, eleven adapter regressions, four
materializer regressions, and the existing sealed-generation coverage artifact
with four lifecycle owner regressions. These are behavior **NOT_RUN** at this
stage. The earlier `l2-preparation-receipt.json` describes only the preparation
snapshot and is stale for current production changes.

Accepted local interface: coverage and event share one committed lexical artifact.
`VerifiedGeneration.coverage` and `.source_publication` come from that same proof;
L0 wires read-handle `source_file_coverage()` and `source_publication_event()`.
The manifest now represents coverage in format 7; unsupported old formats require
rebuild. This is not a compatibility reader or another generation system.

## Existing owners and missing connections

| Boundary | Current owner | Required L0 decision/change |
| --- | --- | --- |
| File identity and public admission | `quanta-index-contract/src/ipc/ingest.rs`: `SearchScopeKey`, `SearchCorpusReplaceScope`, `validate_surface_mutations_v1` | One source-file key, independent of surface aliases; source repo, revision/hash and payload binding; per-file coverage including files with no units |
| Defensive materialization | `quanta-index-lexical/src/adapter_ingest.rs`: `build_batch` and `build`; search-plane `ingest_dispatcher/search_corpus.rs`: `validate_batch_shape_v1` | A shared pure validation authority called before any generation preparation, writer acquisition or track mutation; no separate adapter rule set |
| Physical deletion and fact identity | L0-owned lexical `adapter.rs`, `schema.rs`, `documents.rs`, `channel_payloads.rs` | Delete by canonical `(source_repo,path)` within the containing generation; stamp the same source identity on chunks and symbols; retire text-authority IDs for that same owner |
| Sealed coverage persistence | L2-owned lexical `sealed_generation/{manifest,seal,verify,scrub}.rs`, `generation_dir.rs` | Accepted coverage DTO and exact current artifact format; commit admitted universe and inherited unchanged entries in the existing sealed manifest |
| Pinned coverage read | L0-owned lexical `lib.rs`, `adapter_open.rs`; core `domains/lexical/outbound.rs` | Carry decoded immutable coverage from `walk_sealed_generation` into `LoadedGeneration` and `TantivySearcher`, expose it on that same read handle |
| Strict pre-result gate | L1 planning/routes plus common query/core contract | Exact representation of effective source repo/path/language scope and typed incomplete-coverage error; all plans needing symbols, including generic Text routes |
| Producer source-event lineage | Public ingest/activation DTOs, durable catalog port and existing paired activation owner | Stream/event identity, canonical payload identity, expected committed source base; same event/different payload conflict; atomic state transition with existing publication authority |

## Minimum interface decisions needed

These signatures/changes are proposals for L0 to accept or replace in G0_READY.
They must not be interpreted as a second IR or an implemented API.

1. Extend the existing pure mutation validator rather than copying its rules.
   `SearchCorpusIngestBatch::validate_surface_mutations_v1` already returns a typed
   conflict; its lexical part must use canonical file identity and inspect record
   path/source ownership. A lower-level shared validator over decoded file
   mutations is needed by `LexicalIndexBuildPort::build`, whose public raw channel
   entry bypasses `build_batch`. Specify whether raw file mutation channels are
   migrated or rejected. Do not decode a late invalid operation after an earlier
   operation has applied.
2. Define the current replacement payload as one file owner plus file revision,
   source digest, language, text admission, symbol state, producer/extractor stamp
   and unit-set commitment. Keep `Complete(0)`, `NotRequested`, `Unsupported`,
   `ParseFailed`, and `ProducerFailed` distinct. Include source ownership on
   tombstones, and decide accepted clear semantics explicitly. L2 will reject
   independent Symbol clear on coverage-bound generations.
3. For lexical internal wiring, add an immutable coverage value to the existing
   `VerifiedGeneration` result or a `SealedGenerationVisitor::coverage(...)`
   callback, then hand that value to the existing searcher. L0 chooses the shared
   DTO/type name; L2 implements the persisted artifact and verification path.
   Absent or unknown coverage never becomes Complete.
4. Expose a read-handle method for strict coverage over a validated effective
   pre-result scope, with `Result<..., CoreError>` and an explicit typed refusal
   for any admitted in-scope non-Complete file. The scope must not be constructed
   from result hits. Agree with L1 which existing normalized filters carry the
   exact repo/path/language semantics; do not introduce another matcher.
5. Define source-event identity and expected-base lineage at the existing durable
   publication boundary. Current body-digest idempotency includes target
   generation, so repackaging an old event under a newer generation is not source
   freshness proof. Specify how prepared/sealed/activated state and replay after
   uncertain acknowledgement are reconciled, including event retention across
   generation reclaim. Preserve paired lexical/semantic activation.

## Concrete regressions prepared

`crates/quanta-index-lexical/tests/l2_file_mutation.rs` currently contains eleven
tests against the existing production entry points:

- Chunk then Symbol alias and reverse order: reject before creating target files.
- Chunk record, symbol record, and definition-span path mismatches: reject before
  storage mutation; chunk case places a valid scope first to expose partial apply.
- File replacement plus Symbol-alias tombstone: reject before storage mutation.
- Raw channel alias pair: reject before generation preparation.
- Cross-kind candidate-ID collision and clear/file-replacement overlap: reject
  before any target files are created.
- Valid combined replacement: preserve new text and symbols, remove old symbols,
  retain a live immutable old-generation reader.
- Tombstone: remove that file's text and symbols while inheriting another file.

These are **NOT_RUN** behavior regressions, not demonstrated RED/GREEN. Only
`rustfmt --edition 2024 --check crates/quanta-index-lexical/tests/l2_file_mutation.rs`
has executed, exit 0. The fixtures intentionally exercise the adapter boundary;
the dispatcher's canonical body-digest check is excluded.

After G0_READY, migrate fixtures to its payload and add source identity collisions,
cross-kind unit collisions, empty/zero-symbol coverage, failure states, inherited
incomplete scope, narrow/broad strict scopes, wrong-source and tampered/missing
coverage, source-event replay/conflict/reorder, activation failure, restart and
pinned-reader/reclaim cases. Gate logic and DTOs must use the shared owners above.

## Execution coordination

Proposed first behavior command, pending L0's serialized slot and stable inputs:

```sh
./scripts/cargow test -p quanta-index-lexical --test l2_file_mutation -- --test-threads=1
```

Then run the relevant sealed-manifest/delta/retention tests after implementation.
The repository requires the daemon rail and owning scenario proof for activation,
generation resolution, query pin, state root or shared-ingress claims. Owner
regressions do not qualify that lifecycle. No performance, full-source-byte,
embedding-free, installed-process or whole-engine claim is made.
