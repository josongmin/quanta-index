# L5 — Parser and source-fact remediation

Current follow-up: [L5_ADVERSARIAL_AUDIT](L5_ADVERSARIAL_AUDIT.md) records four
additional P2 counterexamples, fixes and current verification. The earlier
completion below remains bound to its original frozen snapshot.

Status: **VERIFIED for the L5 and integration scope below**. No confirmed open
P0–P2 finding remains in that scope. Whole-repository CI and release qualification
are **NOT_RUN**; external producers and ranking/performance claims are outside this task.

Base HEAD: `98601a66d8cab9c86232b3e62ce490c8b43b71b6` with uncommitted changes. Verification used the
frozen working-tree snapshot `/Users/songmin/.codex/worktrees/l5-final-proof/quanta-index`. The 367-file overlay preserved concurrent
work; no commit, push, reset or sub-agent was used. Of the 22 producer/integration
identity files, 20 still match the original checkout. Concurrent streaming changes
in `contract_proof.py` and `portable_proof.py` were preserved and checked separately:
183 consumer tests pass on the separate `l5-consumer-proof` snapshot, and that
consumer reproduces the frozen
SDK summary from its raw evidence and exact runner/daemon binaries. Both source
identities and separate receipts are recorded. These checks do not rebind frozen
producer proof to later shared-tree edits.

The user's latest instruction authorized completing the shared integration and
Rust execution. Earlier L0 ownership/slot blockers are superseded. No unsolicited
message was sent to L0.

## Changes

- Vendored, pinned TS/TSX grammar accepts generic `typeof import` calls with a
  trailing comma and type-only namespace re-exports, while rejecting malformed
  source. Independent syntax/component oracle details remain in `L5_PRE_G0.md`
  and the external component receipts.
- Immutable all-file preflight retains complete, unsupported and failed rows,
  bounds diagnostics/symbols/deadlines, rejects forged source, and gives fatal
  failures priority. Effective policy, source and grammar commitments are recorded.
- Canonical publication includes empty files and source/coverage commitments,
  rejects cross-file unit collisions, and distinguishes symbol-only from text admission.
- CLI rejects canonical output/state aliases before publication and accounts for
  preflight time once. Metrics use a portable sibling preflight reference.
- Python capture, verdict, schemas, archive inventory and overhead replay bind
  exact raw preflight bytes, corpus, frozen profile and current producer policy.
  Missing, malformed, reordered, duplicate, stale or mismatched evidence refuses.
- Canonical Rust selectors and summary labels include `l5_parser_regressions`.
  Registered/compiled inventory is Python 326, Rust 129 and SDK 20; three extra
  runner unit tests bring the Rust owner execution to 132.
- Source closure traverses registry dependencies to retain nested local patches,
  including `vendor/tantivy-sstable`. A failing synthetic graph regression was
  captured before the fix; all 56 source-closure tests pass.
- Explicit incomplete admission preserves `.txt`, extensionless and unknown-extension
  UTF-8 as `text` with Unsupported symbol coverage. Strict admission still refuses
  them. The previous `LICENSE` rejection was reproduced before the fix.

## Verification

All final rails exited 0 with identical before/after source inputs. Exact commands,
raw outputs, input hashes, environment and artifact digests are in
[L5_COMPLETION.json](L5_COMPLETION.json) and [l5-proof/current](l5-proof/current).

| Rail | Result | Scope |
| --- | --- | --- |
| Rust owner | 132 passed | 87 library, 3 runner, 25 chunking, 17 L5 regressions |
| SDK process | 20 passed | Real daemon, canonical publication, typed coverage gates, output aliases, empty and unsupported sources |
| Shared lifecycle | 64 passed; 397 filtered out | `search_corpus` publication, activation, retention, recovery and pinned-generation boundaries |
| Python | 469 passed | 326 retrieval tests plus proof/SDK/portable/source-closure consumers |
| Current consumer compatibility | 183 passed; SDK replay identical | Later streaming consumer changes; separate frozen source snapshot |
| Frozen Vite | 256/256 accounted; 2,006 symbols | 255 Complete, one intentional ParseFailed; no source excluded |
| Vite process probes | 6 expected outcomes | Four successful lexical/narrow-symbol queries, two typed incomplete-symbol refusals |

Canonical Rust commands used `./scripts/cargow --lane test-daemon-lane`.
Daemon build and benchmark tests used separate feature graphs. The explicit
`QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR=1` cache override did not change source
inputs; the frozen workspace and exact environment are bound in every receipt.
SDK discovery and execution use nextest inventory and terminal JSON events.

Vite commit: `bc598a6a8a6b7d6e157e9f19c16911cff8d2360c`.
Manifest SHA-256: `87ad16fe0c5626ee8ff625ddccf1645558c0b019d9229c5adcf24ffd0cbf7705`.
The intentional malformed file is
`packages/vite/src/node/ssr/__tests__/fixtures/errors/syntax-error.ts`.
Strict preflight exits 2 after emitting the same full 256-file census.
Its text remains searchable; broad or failed-scope symbol authority returns
`SYMBOL_COVERAGE_INCOMPLETE`; the complete `config.ts` scope returns the expected
`defineConfig` definition. The fixed hash-dev provider proves process plumbing,
not quality, speed or embedding-free publication.

## Evidence boundaries

Clean-source qualification deliberately remains separate: the work is uncommitted.
The dependency graph and exact dirty snapshot are bound; the clean-source gate
was not bypassed. Initial failed compiler/fixture checks, the interrupted partial
Python run and the interrupted cold-cache build are not successful evidence.
Current proof is limited to the final frozen rails and the separately bound
current-consumer compatibility checks above. An initial replay invocation passed
inventory and binary arguments in the wrong order and was rejected; the corrected
keyword-argument replay with both exact binaries passed.

Historical `L5_GOAL_AUDIT.json` and `L5_BLOCKED_AUDIT.json` are superseded by this
completion. No shared integration request remains pending for this L5 scope.
