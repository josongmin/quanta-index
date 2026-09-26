# L2 handoff — final code audit repaired five additional defects

Latest audit: [L2_FINAL_AUDIT.md](L2_FINAL_AUDIT.md) and
[L2_FINAL_AUDIT.json](L2_FINAL_AUDIT.json). These supersede earlier verification
statements below. Five additional defects (RR-10 through RR-14) were reproduced
and repaired: physical Delta lineage, malformed bundle admission, inherited
structural chunks, retry after base retirement, and checkpoint-before-retention
ordering.

Latest owner result: **261 passed, 0 failed, 0 ignored, 205 filtered**, with no
input changes during execution. **VERIFIED** is limited to the exact recorded
owner-test snapshot in `l2-proof/audit-owner-closeout.json`.
Overall integration qualification remains **BLOCKED**: the SDK scenario again
failed harness compilation on 16 old DTO uses; scoped lint stopped on 11 core
dependency errors. Behavior and target lint execution were **NOT_RUN**.

Source snapshot: shared dirty `main`, HEAD
`5571132655a83824731e7909b0e310951edad52b`.
Recorded owner source inventory: [L2_FINAL.source.json](L2_FINAL.source.json).
Closeout revalidation observed later `core/domains/lexical/coverage.rs` drift.
Shared source continues to change; do not promote snapshot receipts into current
whole-repository qualification. Existing concurrent work was preserved. L2 did
not commit, push, reset, create an agent or send a proactive L0 message.

The strict existing auxiliary state/meta rows now require nullable
`source_batch_digest`; missing legacy fields reject and require an offline rebuild.
A new target checkpoints complete chunks before retention, then records track
and rollback authority after the reconciled receipt. No new generation/IR scheme
or performance claim was introduced.

Earlier `L2_HANDOFF.source.json`, `L2_RR_REOPEN.source.json`, the 98601a66 source
snapshot, 556-test execution and fuzz receipts are historical baselines.

## Implementation and contract effects

- Canonical source-file replacement combines source revision, text, symbols and
  explicit coverage. Validation rejects surface aliases, conflicting mutations,
  wrong source/path and malformed commitments before either track mutates.
- Physical deletion and text-authority retirement use `(source_repo, path)`.
  A same-path file from another source survives replacement and tombstone.
- Full and delta publications commit coverage plus source event into the existing
  lexical generation. Manifest format 7 binds the artifact; seal/open/verify
  reject missing, unexpected or tampered coverage. Empty files and explicit
  incomplete extraction states remain distinguishable. Missing evidence is not
  promoted to Complete.
- Delta proves its base, copies unchanged admitted-file coverage, and rejects
  candidate IDs colliding outside the retired file set. Coverage-bound raw
  mutation and independent Symbol clear refuse. Old immutable readers remain
  valid; reclaimed bases and mismatched identities refuse.
- The strict capability gate checks the effective pre-result repo/path/language
  universe using L1's scope matcher. Hit predicates do not determine coverage.
- Existing ActivationCatalog repository envelopes (format 2) hold all revision
  roots, source streams, event identity history, reservations and high-water.
  One durable replacement publishes paired roots and source freshness together.
  Legacy root files refuse pending an offline rebuild. Rollback preserves source
  high-water; reclaim preserves event identity history.
- The materializer reserves only after physical-plan, repair-mode and semantic
  admission under its operation lock, but before repair, provider or track writes.
  Journal reconciliation proves staging; unknown/uncertain evidence never frees
  a reservation by timeout.
- Required public `SourcePublicationBinding` contains the exact event, original
  lexical snapshot and original journal digest. The dispatcher returns the
  immutable catalog binding. Observations describe the current request; replay
  receipts describe the original publication. SDK validates both with or without
  observation and activates the original lexical/semantic pair. Incompatible
  explicit CAS expectations refuse. Unbound receipt conversion was removed.
  See [replay contract details](L2_REPLAY_BINDING_REQUEST.md).

Changed-source inventory spans lexical mutation/sealing, ingest dispatch and
materialization, activation catalog/lifecycle, contract request/observation,
SDK receipt/activation binding and dedicated regressions. The exact 50-path
inventory and per-file SHA-256 values are in `L2_FINAL.source.json`; whole-file
changes may include preserved edits from other owners.

## Reviewed repairs

| Finding | Implemented repair and covered boundary |
| --- | --- |
| RR-01 / P1 | Original replay binding is explicit through contract, dispatcher, SDK and activation. Retargeted observed/unobserved replay, malformed bindings and CAS refusal have owner regressions. |
| RR-02 / P2 | Envelope encode/decode reject multiple source events occupying one physical target; reopen and distinct-revision controls included. |
| RR-03 / P2 | Negative mutation fixtures refresh independent unit/event commitments and assert the intended path/conflict error. |
| RR-04 / P2 | Full replacement admits only matching-identity typed sidecar corruption to the existing repair planner. Delta/wrong identity/direct open/build remain strict. |
| RR-05 / P2 | Ingest and semantic fixtures use actual UTF-8 byte lengths and refresh commitments after edits, allowing their intended recovery/provider assertions to execute. |
| RR-06 / P2 | Invalid Delta/semantic preflight no longer consumes a source reservation before physical work. Subsequent valid publication remains possible. |
| RR-07 / P2 | Source apply errors retain Uncertain and the original journal/body binding for fenced retry. Other routes preserve their terminal refusal policy. |
| RR-08 / P1 | A separate writer mutex serializes catalog mutation; snapshot readers retain the acknowledged pair during filesystem I/O. Memory promotion follows durable sync; writer poison fences serving/mutation. |
| RR-09 / P2 | A transient preflight refusal after an uncertain apply retains Prepared instead of freezing Refused. The regression recovers the same original journal without retargeting. |

The regex fixture uses supported `a[.]rs` syntax to exclude independently known
failed `z.rs`; production regex semantics were not changed. Batch-dependent
observation helpers live beside the request DTO to avoid adding a request-module
cycle through the observation module.

## Prior owner checks — historical snapshot

The earlier command used canonical tooling with explicitly authorized concurrent
execution: `QUANTA_INDEX_RESOURCE_ADMISSION=0 CARGO_BUILD_JOBS=2`.
No other worker's admission lock was removed.

```sh
./scripts/cargow --lane test-integration-lane test -p quanta-index-contract -p quanta-index-sdk -p quanta-index-search-plane --lib --locked -- --skip query_dispatcher::
./scripts/cargow --lane test-integration-lane test -p quanta-index-lexical --test l2_file_mutation --locked
```

| Executed target | Passed | Failed |
| --- | ---: | ---: |
| Contract library | 163 | 0 |
| SDK library | 114 | 0 |
| Search-plane library excluding query dispatcher | 259 | 0 |
| Lexical L2 mutation integration | 20 | 0 |
| Total | 556 | 0 |

No ignored tests; 203 query-dispatcher tests were explicitly filtered.
`l2-proof/owner-terminal.json` records commands, before/after input hashes,
toolchain/platform, terminal counts, log digest and executed binary digests.
Its SHA-256 is `3ae358d6605c49be399e1d9110884a3fa8614b3e4cca20fca762ad3458420f34`.
Relevant shared preview/candidate dependencies changed during execution, so
**final-source qualification is BLOCKED**. No repeated owner run is requested
merely to chase concurrent edits.

An earlier full search-plane run had one remaining failure after L2 repairs:
`query_dispatcher::tests::l1_query_domain_window::contradictory_languages_do_not_hide_tokenless_phrase`.
L1 received that observed failure and raw log without a rerun request. Its status
on later L1 source is not asserted here. Selected formatting/whitespace commands
exited 0; those are source hygiene, not behavioral proof.

## Earlier composed runtime and shared gate results

The authored
`sdk_frontdoor::l2_source_replay_keeps_original_publication_through_sdk_activation_and_restart`
uses the real SDK, sockets, storage, journal and activation catalog. It publishes
an original event, retries against a different revision/generation, activates the
original pair, restarts, and checks original receipt/head retention.

```sh
./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite l2_source_replay_keeps_original_publication_through_sdk_activation_and_restart --locked
```

**Compilation FAILED; behavior NOT_RUN; composed proof BLOCKED.**
`quanta-index-searchd-harness/src/harness.rs` still has 16 old contract uses:
removed `scope`/`scope_digest` and missing `source_event`.
`l2-proof/sdk-runtime-l2.json` and `.log` preserve the diagnostics.
Its producer fixtures and unsealed protocol need a coherent migration; inventing
Complete coverage or generation-derived event IDs would invalidate this proof.
The earlier full daemon profile timed out at resource admission before tests.

| Gate | Observed result |
| --- | --- |
| `just rust-public-api` | FAILED: contract baseline drift; SDK pinned-nightly compilation rejects deprecated `collection_budget::fetch_update`. |
| `just rust-cargo-modules` | FAILED: shared contract/core baseline drift. |
| `just rust-hexagonal` | FAILED: `ranked_page_tests.rs` contains forbidden `segment_id` boundary token. |
| `just rust-module-cycles` | FAILED: workspace scanner cannot resolve lexical `ranked_page::rows`. Scoped contract scan shows only the existing ingest/payload cycle, with no observation module in it. |
| `just rust-profile test-daemon` | FAILED at initial resource admission timeout; no test execution. |
| `just rust-fuzz-smoke` | Exit 0; four targets completed without an observed crash. Final-source qualification BLOCKED by source drift. |

The completed fuzz rail processed 404,124 request inputs, 501,619 response
inputs, 3,382,806 corpus-ingest inputs and 57,541 LQ-parser inputs: **4,346,090**
in total (61/61/61/62 seconds). `l2-proof/fuzz-current-summary.json` binds the
terminal summaries, raw receipt/log digests and observed nightly toolchain.
Sources changed during this rail, and the initial mutable corpus was not frozen;
these are successful execution results, not final-source qualification.

Raw logs, receipts and hashes are in `l2-proof/` and the current manifest.
No API/module baseline was changed by L2.

## Remaining integration and evidence limits

1. Migrate the shared searchd-harness producer fixtures and sealing protocol (the final audit reattempt still found 16 compile errors);
   then execute the composed SDK replay/activation/restart scenario and required
   daemon profile on fixed, recorded inputs.
2. Integrate shared API/module baselines and resolve the recorded shared gate
   failures at the serial integration boundary. Revalidate later-source L1 query
   behavior there; the earlier failure is not a current-source verdict.
3. Freeze all correctness-relevant source/dependencies/config/binaries before
   final qualification. Current selected passes are owner execution evidence.
4. Semantica producer issuance and downstream migration remain external. The
   peer owner received verified Index RFC/port/catalog paths. No inspected Index
   document assigns a concrete Semantica durable event-issuance owner; none is
   claimed agreed. No public SDK source-base/high-water lookup or event issuer
   exists; the peer was given that current-source boundary. Generation-derived event IDs remain invalid.

Catalog mutation is serialized; no throughput claim is made. Per-repository
limits remain 16 MiB encoded envelope, 256 revision roots, 256 streams and 8192
retained events, refusing exhaustion rather than evicting replay history. A
post-rename parent-fsync failure fences the catalog. Previously persisted Refused
journal rows are not rewritten. Owner retry tests use the memory journal plus
durable catalog; SQLite lease recovery and installed-process durability remain
outside their proof. Parsing stays producer-owned, and scope digests are not an
independent full-file source-byte proof. Existing paired semantic activation is
preserved; embedding-free publication is not claimed.
