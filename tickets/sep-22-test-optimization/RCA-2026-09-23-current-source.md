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

The shared `fail_closed_wait` helper checks its deadline between poll calls.
An in-flight ready value or terminal refusal can therefore complete after the
nominal duration. TOPT-06 requires a typed timeout for a spent retry wait, but
does not specify a hard completion deadline for an in-flight call. The earlier
strict-deadline change was reverted; this is **not a confirmed open defect**
and needs no user contract decision for this packet.

The focused checks above do not satisfy the `TOPT-08` full-rail or performance
gates. A prior `just fmt-check` was red on unrelated concurrent edits; a later
package-scoped `./scripts/cargow fmt --check -p quanta-index-lexical -p
quanta-index-searchd-harness -p quanta-index-searchd-runtime` passed.

## Integration-gate regressions found later on Sep 23

- `test-daemon` exposed a route-specific harness classification bug. A pinned
  structural query returned `StrGenerationNotReady`, but the structural page
  path used the text warmup predicate and retried that terminal refusal for
  15 seconds. The result became `HARNESS_START` instead of the daemon's typed
  error. The exact test failed before the fix; the structural page owner now
  uses the ordinary readiness predicate. The same selector passed afterward,
  and the eight `e2e_perf_chaos::structural_` cases passed (one nextest `LEAK`
  warning under a heavily contended host).
- A concurrent Clippy cleanup changed the cancellation registry's poisoned
  lock path to an empty wake list. That can mark a request cancelled while
  leaving an already registered waiter asleep. The request-budget owner now
  recovers the poisoned guard for registration, cancellation, cleanup, and
  census; a poison-injection test passed 1/1. This is a correctness repair,
  not a lint-only change.

The prior 203-case `test-daemon` run failed on the structural classification
and stopped with 48 cases not run. It is not a full green receipt. Re-run the
broader rail only after writer reconciliation on a stable source.

## Committed-tree integration receipt

This section supersedes the earlier dirty-tree verification status, not its
failure history. The test-optimization changes and the provider-grant repair
are integrated in `main` at `81fcec7ffd6baf9690e60528f80048cea7cc34f3`.
The clean isolated test checkout used commit `90f646053d058ceab7b56abfe9843bcde01a8e1a`;
both commits have the identical tracked tree
`997803009e906f7c06f6284a656f4fa1481e5a6b`. The shared `main` checkout
has concurrent uncommitted Rust/benchmark/CI files outside this repair and is
not the source of these receipts. `origin/main` was still `97f7c9a` (local
`main` ahead 10); no remote push is claimed.

The first exhaustive daemon run on `c0baa5d` found a real fixture contract
failure: its OpenAI metrics case selected an external profile but the harness
could only pass the default incomplete egress grant. The daemon correctly
refused boot with `PROVIDER_EGRESS_DENIED` (`tenant_id` required). Commit
`81fcec7` gives the harness an explicit grant input without changing the
fail-closed default; the metrics fixture supplies test authorization, while
the manual OpenAI E2E and relevance A/B read the operator's grant at their
entry points. No grant or source-content consent is synthesized by the
harness.

| Rail on the identical tracked tree | Result |
|---|---|
| `just fmt-check`; test-authority lint | pass; pass |
| `just rust-profile test-fast` | exit 0 |
| `just rust-profile test-integration` | 204 + 6 + 58 = 268 passed, 0 skipped |
| `just rust-profile test-daemon-fast` | 61 passed; 1 intentionally ignored external-API test |
| `runtime_extended_suite` (no fail-fast) | 60 passed; 0 skipped; one non-failing nextest leak warning |
| Complete `daemon-all` selector with `--no-fail-fast` | 304 passed, 0 failed; 1 intentionally ignored external-API test; nextest run `bbb8bb49-be12-4878-b1d4-4d800630e400` |
| `quanta-index-searchd` and harness library units | 84 passed + 93 passed |

The complete selector uses the same seven binaries, features, and four test
threads as `just rust-profile test-daemon-all`; `--no-fail-fast` only prevents
an early failure from hiding later cases. The registered `daemon` selector's
three binaries are a subset of those seven. The earlier exact `1bf5f0a`
`test-daemon` run passed 203/203, but is not promoted to a final-tree receipt.
All 18 retained finding rows have owner-side code on this tracked tree; this
crosswalk/source check does not replace each ticket's mutation or timing
acceptance criterion.

Qualification is still **blocked**, for distinct reasons:

1. `just rust-clippy` exited 101 on this tracked tree. A keep-going diagnostic
   exposed lint failures in `lq-regex`, `lq-norm`, `embed`, `catalog`, `SDK`,
   and `search-plane`; it was stopped after the workspace failure was
   decisive, so its output is not a complete error inventory. Concurrent
   uncommitted edits in the shared checkout overlap many of those owners.
   They must be reconciled by their writer before a new HEAD is qualified.
2. `just rust-public-api` exited 1 because the contract baseline lacks the
   already-present `FileOwnerProjectionErrorV1: Error` implementation. This
   is outside the provider-grant repair; do not update the baseline without
   confirming the contract owner's intended public surface.
3. `just rust-profile verify-rust` was not run: its required Clippy step is
   already red. No `CODE_QUALIFIED` or product-qualified verdict is issued.
4. TOPT-00 has no uncontended pre/post baseline or historical admission
   record. Other Cargo/rustc/mutation work was active throughout the run;
   the 714-second daemon duration is correctness evidence only, not a speed
   result. R1-R4 and TH-4 still need the specified quiet-host repeated
   measurements, and R5 needs the conserved-case-count comparison.

Next gate: reconcile the concurrent writers, fix the Clippy and public-API
failures at one new clean HEAD, rerun source-bound required rails if Rust source
changes, then collect the TOPT-00 performance protocol on a quiet host. Until
then, TOPT-01..07 are code-landed, while TOPT-00/08 and overall closure remain
open. Do not interpret the historical Sep 22 audit's “18 open actions” as 18
remaining implementation defects.

## Clippy sweep and broader rails (later on Sep 23)

Dirty-tree receipts at `HEAD 81fcec7` with concurrent peer edits present
(`searchd-runtime`, `searchd-harness`, `benchmarks/retrieval`,
`tools/benchmark`; 89 dirty paths at closeout). Supersedes the receipt
above on `lq-regex`, `lq-norm`, `embed`, `catalog`, `sdk`, and
`search-plane`: all are Clippy-clean now, plus `repomap`, `searchctl`, and
`searchd` app owners (33 `.rs` files, behavior-preserving; see TOPT-08
evidence). `just fmt-check` exit 0. `just rust-clippy` (CI recipe) exit 101
with errors ONLY in `benchmarks/retrieval` (152, retrieval-lane owned,
untouched). Broader rails: test-fast 1859/0, test-integration 268/0,
test-daemon 203 passed / 1 skipped, test-daemon-all 304 passed / 1 skipped;
`rust-policy` and `rust-machete` exit 0. `just rust-doc` exit 101 on
peer-active files only (owning lane fixing in-tree). `verify-rust` not
green; no frozen-source re-run; timing still needs a quiet host. TOPT-08
stays blocked; no verdict promoted.

## Integrated-source supersession

The Clippy/rustdoc/public-API blocker list above is historical to the named
snapshots. Benchmark/harness, documentation-link, and public-API baseline
repairs were committed in `e42cb82`; the 33-file Rust Clippy owner sweep was
integrated at `f478f69e5e0006afb7ea36360be9b4dd7558d252`. The shared
checkout still contains unrelated Sep 23 retrieval benchmark work, which was
not staged with the TOPT changes. Clean-source gate results and remaining
qualification gaps are tracked in `SEP23-GATE-FOLLOWUP.md`.
