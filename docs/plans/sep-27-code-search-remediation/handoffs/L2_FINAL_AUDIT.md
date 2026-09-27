# L2 final code audit — owner snapshot

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Superseded process/lint status: [L2_PROCESS_AUDIT.md](L2_PROCESS_AUDIT.md) records
the subsequent native SDK/daemon crash matrix and successful current lint rails.
The results below describe their own earlier frozen snapshots.

Five source-backed defects were reproduced and repaired. The final selected
search-plane command executed **261 passed, 0 failed, 0 ignored, 205 filtered**
with no input changes during execution. **VERIFIED** applies to that recorded
owner-test snapshot. Overall integration qualification remains **BLOCKED**:
the composed SDK scenario cannot compile the shared harness, and scoped lint
fails in a dependency before checking L2.

Recorded HEAD: `5571132655a83824731e7909b0e310951edad52b`, dirty shared `main`.
Another worker advanced HEAD during the audit. L2 did not commit, push, reset,
spawn agents, alter other owners' changes or update gate baselines.

The machine-readable audit binds commands, source snapshots,
dirty state, toolchain/environment, terminal counts, binary hashes and raw log
digests. L2_FINAL.source.json contains the 50-path owner
inventory. Whole-file snapshots may contain preserved concurrent edits.
At closeout, the recorded receipts/logs still matched their hashes, but concurrent
`core/domains/lexical/coverage.rs` edits changed an inventoried source after the
owner run. Current-source equivalence is **BLOCKED**; snapshots stay frozen and
no native run was repeated solely to chase this drift.

## Repaired defects

| ID | Reproduced failure | Final behavior |
| --- | --- | --- |
| RR-10 / P1 | Delta declared event 2 as parent while cloning event 1's physical snapshot, resurrecting unchanged old content. | The actual sealed base stream/event must match the declared parent. Missing/wrong stream/wrong parent refuse before target preparation; the correct base preserves new and unchanged content. |
| RR-11 / P2 | Invalid metadata CBOR passed admission; decoding happened after target preparation or source reservation. A raw replacement before a bad bundle already created storage. | Both typed entrypoints and the complete raw operation list decode bundles before any preparation, provider work or source reservation. Valid typed metadata remains accepted. |
| RR-12 / P1 | Structural Delta finalization started from an empty target, dropping untouched `b-1` while lexical storage retained it. | Build the target chunk universe from the complete base, apply canonical replacement/tombstone, and persist the full result plus its batch digest atomically in existing auxiliary rows. |
| RR-13 / P1 | After completed Delta finalization retired the base, an uncertain journal retry refused with `SEARCH_CORPUS_DELTA_BASE_NOT_SEALED`. | Finalize-only recovery requires the exact original Pending source binding, a completed target chunk transaction bound to the same batch, and both physically exact sealed tracks. Missing completion or missing original reservation still refuses. |
| RR-14 / P1 | Retention could remove base authority before target chunks were durable. An I/O failure/restart at that boundary left no complete target to recover. | Checkpoint target chunks before invoking retention. Track/history publication still follows the reconciled retention receipt. The failure regression proves chunks survive and track publication does not occur early. |

The structural regression also injects auxiliary transaction failure, checks
zero partial target publication, restores rows into a fresh ledger after base
retirement, applies a separate structural update, and retries the completed
target. Missing legacy completion metadata rejects. A Delta with physically
sealed base tracks but no complete base chunk authority refuses during admission.

## Executed evidence

Every native command used `QUANTA_INDEX_RESOURCE_ADMISSION=0 CARGO_BUILD_JOBS=2`
under the user's concurrent-work authorization. No other worker's lock was removed.

| Command / receipt | Terminal result | Boundary |
| --- | --- | --- |
| `audit-lineage-red` | 0 passed, 1 failed | Actual wrong-base admission before the repair. |
| `audit-bundle-red` | 0 passed, 2 failed | Typed admission and raw partial preparation; concurrent benchmark Python edits recorded. |
| `audit-lexical-final` | 23 passed, 0 failed | Lexical owner snapshot, no source drift during execution. |
| `audit-metadata-control` | 1 passed, 0 failed, 36 filtered | Real valid typed-metadata filtering; no source drift. |
| `audit-chunk-inheritance-red` | 0 passed, 1 failed | Observed `[new-a-1, new-a-2]` instead of `[b-1, new-a-1, new-a-2]`; concurrent L1 test edit recorded. |
| `audit-retired-base-red` | 0 passed, 1 failed | Real source catalog plus dispatcher/memory journal; original Delta retry refused. |
| `audit-retention-order-red-executed` | 0 passed, 1 failed | `retention ran before target chunks became durable`. |
| `audit-owner-closeout` | **261 passed, 0 failed, 0 ignored, 205 filtered** | Final five repairs and owner regressions; no source drift during execution. |

```sh
./scripts/cargow --lane test-integration-lane test -p quanta-index-lexical --test l2_file_mutation --locked
./scripts/cargow --lane test-integration-lane test -p quanta-index-lexical --test tantivy_smoke tantivy_executes_repo_metadata_filters_when_bundle_payload_is_typed --locked
./scripts/cargow --lane test-integration-lane test -p quanta-index-search-plane --lib --locked -- --skip query_dispatcher::
```

The lexical commands and final search-plane command bind different snapshots;
their counts are not one combined final-source qualification. The earlier 556
selected passes and fuzz run remain historical. The initial retention red attempt
did not execute because a concurrent L1 test used an invalid tuple-variant pattern;
L1 repaired its owned file before the executed red above. Intermediate owner runs
also preserve one new-test import failure and one fixture lacking complete base
authority; both were corrected before the final 261-test run.

## Remaining integration boundaries

- **FAILED**: `./scripts/cargow --lane check-lane clippy -p quanta-index-search-plane --lib --locked -- -D warnings` stopped at 11 core dependency lint errors. L2 lint completion is **NOT_RUN**. See `l2-proof/audit-clippy-terminal.json` and `.log`.
- **FAILED compilation / NOT_RUN behavior**: `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite l2_source_replay_keeps_original_publication_through_sdk_activation_and_restart --locked` reproduced 16 obsolete harness DTO uses (`scope`, `scope_digest`, missing `source_event`). See `l2-proof/audit-sdk-runtime-terminal.json` and `.log`. No fabricated Complete coverage or generation-derived source event was inserted to bypass this migration.
- **NOT_RUN**: full repository/daemon profiles, current API/module/cycle/hexagonal gates, installed-process crash cuts and performance qualification after these repairs. Older gate results in the handoff are historical.
- **BLOCKED external integration**: Semantica producer issuance/cutover is not connected by this audit. The SDK accepts a producer-fixed event and returns the original publication binding; no public SDK high-water/base lookup or event issuer exists. Internal `inspect_source_event` requires a complete known event. No inspected Index document establishes the Semantica issuance owner. The peer received these current-source boundaries.

The auxiliary state/meta schema now requires nullable `source_batch_digest`.
Legacy missing fields reject; there is no compatibility default or parallel
generation scheme. Persisted old state needs an offline rebuild. New publications
add a chunk checkpoint before retention and write inherited structural rows.
No memory, throughput or latency improvement is claimed.

Structural row restore here is owner-level proof; it is not a daemon restart or
physical crash proof. Coverage/source digests remain producer attestations, not
independent full-file byte proofs. Existing paired semantic activation is preserved.
