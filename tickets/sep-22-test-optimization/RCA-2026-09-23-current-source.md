# SEP-23 current-source RCA and partial verification

This is a **dirty-worktree, owner-local** repair record, not TOPT-00 admission,
TOPT-08 qualification, or performance evidence. `HEAD` observed while
recording this note was `2b54b72d2fd20c2ee24cc28124d70899892186e2`;
it advanced during testing, and concurrent uncommitted edits were present.
The commands therefore do not share a proven immutable source digest.
Re-freeze the final integrated tree before promoting any result.

| Finding | Root cause | Owner repair | Current proof |
|---|---|---|---|
| TOPT-03 lease holder survived parent exit until a test deadline | A release file encoded only explicit release; parent death had no event channel. | The parent owns the holder's stdin pipe. `R` releases explicitly; EOF releases when the parent exits. The holder has no release polling or deadline. | `just rust-profile test-runtime-supervisor-owner`: 10/10 passed, nextest run `0b20c427-5822-4a62-8a3f-862b134d5e67`. |
| TOPT-02 watcher thread panic could look like clean disarm | `join()` was discarded, and `disarm()` had no error result. | Propagate join failure through `disarm()` to typed `PeerWatchFailed` connection closure; retain join-on-drop for unwind cleanup. | `./scripts/cargow test -p quanta-index-ipc --lib`: 45/45 passed, including injected watcher panic and connection-consumer tests. |
| TOPT-05 matrix smoke accepted duplicate candidate rows | The assertion deduplicated actual results before exact comparison. | Sort without deduplication and reject a duplicate-needle mutation. | Matrix selector passed 2/2 after the final rename, nextest run `8474b03d-8030-4f5f-8036-c92eb0870591`. |
| TOPT-01 catalog clock could conceal an invalid call | Exhausted or poisoned clock scripts returned the last timestamp. | Panic at the scripted clock owner on exhaustion or poison, with negative tests. | `./scripts/cargow test -p quanta-index-catalog --test idempotency`: 18/18 passed. |

`python3 tools/ci/lint/check-test-authority.py` passed. `git diff --check`
passed for the four repaired owner files before the final test-name-only rename.

## Qualification still open

- TOPT-00: no uncontended before/after baseline, complete selector inventory,
  audit-input digest, or immutable clean-source admission was captured before
  these repairs. Concurrent Cargo/rustc/cargo-mutants activity and unrelated
  dirty paths prohibit a clean timing claim. This cannot be reconstructed from
  focused green tests.
- TOPT-08: full required Rust profiles, all 18 finding rows, and same-source
  performance comparison remain unqualified. `just fmt-check` failed on
  concurrently edited files outside the four repair owners; do not format or
  claim ownership of those files from this lane.
- The shared `quanta-index-ipc/src/server.rs` diff also contains an unrelated
  `ReadinessTimeout` classification hunk. It is outside this RCA and needs
  writer reconciliation before staging this file.
- Next gate: reconcile writers and reach a stable source snapshot; record all
  dirty ownership and selector counts; run the TOPT-00 protocol on a quiet
  host; then execute TOPT-08's broader rails and compare only source-matched
  uncontended samples. Keep these tickets open until their own acceptance
  criteria are satisfied.

## Additional owner repairs and audit (later on Sep 23)

These are focused dirty-tree receipts, not a replacement for TOPT-00 or
TOPT-08. Other writers advanced `HEAD` and edited the workspace manifests
while the tests ran, so no immutable same-source digest is claimed.

| Finding | Root cause | Owner repair | Focused receipt |
|---|---|---|---|
| Harness readiness could exceed 15 seconds and mislabel a late ready response | The readiness loop gave each IPC request its full 30-second timeout and made one more request after the 15-second wait expired. | `wait_for_query_response` bounds every IPC attempt by the earliest readiness, request, and caller deadline; it no longer makes the post-timeout request. Transport errors retain their actual typed message. | `quanta-index-searchd-harness` deadline unit test 1/1; `just rust-profile test-daemon-fast` 61/61. |
| SDK wait adapter assertions depended on scheduler timing | Three scripted tests used `RealTicker` with a 100 ms window and asserted a retry count; a deschedule could turn an intended retry into a false-red. | Inject `WaitTicker` into the adapter tests and advance a virtual clock. The production-facing wrapper continues to use `RealTicker`. The live scrape test asserts timeout type and predicate evidence, not a minimum number of 100 ms polls. | SDK selector 3/3; extended scrape selector 1/1. |
| File-owner projection failed response encoding | The lexical projector used the source-repo authority ID as the projection row's `repo_id`, although the wire contract requires each row to carry the paired ranked candidate's identity. Multi-repo fixture queries then returned `Remote(InvalidRequest)` after CBOR encoding failed. | Keep source-repo ID only for authority lookup; copy `candidate.repo_id` into the projection row. Include typed-error evidence in the E2E assertion. | Exact E2E RED with `file owner projection row 0 does not match`; after owner repair the same selector passed 1/1, and daemon-fast passed 61/61. |

The shared `fail_closed_wait` helper remains unresolved as a **contract
choice**, not an accepted code fix. Current code checks the deadline between
poll calls, so an in-flight ready value or terminal refusal can be accepted
after the deadline. A strict-deadline change and two RED tests were reverted
by another writer. Decide whether the contract is a hard completion deadline
or a poll-start/admission deadline, then update both implementation and tests
under one owner; do not silently reapply the reverted change.

The focused checks above do not satisfy the `TOPT-08` full-rail or performance
gates. A prior `just fmt-check` was red on unrelated concurrent edits; a later
package-scoped `./scripts/cargow fmt --check -p quanta-index-lexical -p
quanta-index-searchd-harness -p quanta-index-searchd-runtime` passed.
