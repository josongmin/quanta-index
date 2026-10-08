# OCT-04-003 — Source preparation SDK

Status: **Offline preparation integrated in local main `433a9363`; focused
verification `VERIFIED`** (2026-10-08). **Breaking publication recovery and
Semantica V5 remain isolated candidates.** This is not release, installed-host,
performance, or cross-repository acceptance.
[MAY-27-002](MAY-27-002-sdk-ingress-and-public-surface-boundary.md) owns the public
ingress and publication boundary.

## Decision and scope

Preparation is an optional, offline producer-side layer. The existing
`SearchCorpusBatch` remains the only search-corpus publication batch; direct
HIR-aware producers may continue to construct it without an adapter.
`quanta_index_sdk::preparation` provides:

- Compile-time `SourceAdapter<Input>` with declared `PreparationCapabilities`.
  Built-in `PlainTextAdapter` and `MarkdownAdapter` accept exact UTF-8 source
  bytes and emit lexical-only chunks with original byte/line spans. Markdown
  is not rendered or inferred into symbols. Invalid UTF-8, oversized lines,
  invalid provenance, or budget violations fail; no parser fallback runs.
- `PreparationBudgets` and a hashable effective `PreparationProfile`. Built-in
  `PlainTextAdapter::profile(budgets)` and `MarkdownAdapter::profile(budgets)`
  pin the implementation recipe revision. Custom adapters may construct their
  own profile. A recipe or budget change changes the profile digest.
- `SourceContext` uses the existing `SourceFileRevision` and caller-owned stable
  source key. Opaque `PreparedSource::from_lexical` checks custom lexical,
  symbol, and canonical `SemanticSourceReplaceScopeV1` contributions before
  lowering. Typed semantic and `ClusterCard` membership authority use the
  existing contract; unsupported or incomplete contributions are rejected.
- `PriorSourceManifest`, `CompleteSourceSet`, `ReconcileIntent`, and
  `reconcile_complete_universe` compare a caller-declared complete source
  universe to caller-owned prior state. Omission means deletion only for that
  declared universe. Moves tombstone original keys; changed recipes or
  semantic content replace prior contributions. Prior entries retain the full
  existing `SourceFileCoverage`, including language, completeness and policy;
  unchanged comparison covers that whole value and semantic contribution stamps.
  Canonical serde ordering rejects duplicate or malformed keys.

No second wire IR, daemon plugin registry, SDK durable store, or native
image/audio/video engine is introduced. Another source format can participate
only by emitting source-bound text and existing typed contributions. OCR or
captions, if supplied by a producer, remain derived text, not native media
indexing. The daemon's contract and query capabilities do not expand here.

## Public construction example

This function uses only public constructors and builders. Its caller provides
an attested `SourceFileRevision` whose SHA-256 matches `bytes`, a producer event
identity, and the target publication identity. The budget values illustrate
caller policy; they are not SDK ceilings.

```rust
use quanta_index_contract::SourcePublicationEvent;
use quanta_index_sdk::{
    ManifestGeneration, RepoId, RevisionId, SearchCorpusBatch, SourceFileRevision,
};
use quanta_index_sdk::preparation::{
    CompleteSourceSet, MarkdownAdapter, PreparationBudgets, PreparationError,
    PriorSourceManifest, ReconcileIntent, SourceAdapter, SourceContext, TextSource,
    reconcile_complete_universe,
};

pub fn prepare_readme(
    source: SourceFileRevision,
    bytes: &[u8],
    target_repo: RepoId,
    target_revision: RevisionId,
    generation: ManifestGeneration,
    event: SourcePublicationEvent,
) -> Result<(SearchCorpusBatch, PriorSourceManifest), PreparationError> {
    let budgets = PreparationBudgets::new(1024 * 1024, 1024 * 1024, 64 * 1024, 256, 4 * 1024 * 1024)?;
    let profile = MarkdownAdapter::profile(budgets)?;
    let context = SourceContext::new("readme-stable-key", source, profile)?;
    let prepared = MarkdownAdapter.prepare(TextSource::new(context, bytes))?;
    let prior = PriorSourceManifest::new(vec![])?;
    let intent = ReconcileIntent::new(target_repo.clone(), target_revision.clone(), None);
    let changes = reconcile_complete_universe(
        &prior, CompleteSourceSet::new(vec![prepared]), intent, 4 * 1024 * 1024,
    )?;
    let batch = SearchCorpusBatch::replace_generation(
        target_repo, target_revision, generation, "producer-manifest-digest",
    ).source_event(event);
    changes.apply_to_batch(batch)
}
```

For subsequent generations, recover the **confirmed** prior manifest and
pass it with the full current source set and
`ReconcileIntent::new(repo, revision, Some(base_generation))`; lower into a
matching `SearchCorpusBatch::delta`. `apply_to_batch` returns the checked batch
and **planned** next manifest. The supplied batch may carry target metadata
and event identity, but must have empty lexical and semantic mutation lists;
mixing a caller-authored clear, replace, or tombstone would invalidate the
planned manifest. The method checks repository, revision, base, mode,
canonical mutations, membership authority, and actual CBOR body length against
the explicit aggregate budget. A baseline
`ReplaceGeneration` emits all current sources; unchanged entries may be
omitted only from a delta. Deletion-only deltas still require an explicit
aggregate budget.

The caller must retain the original event, expected-base event/generation,
publication outcome or receipt, and planned manifest together. Persist or
recover the planned manifest as confirmed state only after reconciling the
intended publication and activation. An ambiguous outcome must not advance
the prior snapshot; the SDK performs no hidden durable I/O.

## Publication and recovery

**Candidate-only API:** the following example requires the isolated publication
recovery bundle. Main `433a9363` retains `publish_and_activate` and
`ActivationAfterPublish`; preparation does not require this API change.

`publish_outcome` exposes the validated original publication and committed
receipt, including a source-event replay whose original target differs from
the attempted batch. `PublishedBatchEvidence::from(&outcome)` binds these for
the explicit activation CAS, which requires a sealed receipt. Invalid evidence
or expected-head input is refused before control I/O; the caller still owns the
provided evidence. Control failures retain that checked evidence in `AfterPublish`:

```rust
use quanta_index_contract::SearchCorpusActiveHeadV1;
use quanta_index_sdk::{
    PublishedBatchEvidence, QuantaIndex, SdkError, SearchCorpusBatch,
};

pub fn publish_then_activate(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch<true>,
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<(), SdkError> {
    let outcome = client.search_corpus().publish_outcome(batch)?;
    let evidence = PublishedBatchEvidence::from(&outcome);
    let _ack = client.search_corpus().activate_published(&evidence, expected_active)?;
    Ok(())
}
```

`SdkError::AfterPublish` retains the original evidence and typed cause when
observation after a verified publish or activation fails. Its
`PublishedBatchFailureStage` is `Observation` or `Activation`; callers can read
`SdkError::published_evidence()`, `published_receipt()`, and
`published_publication()` for reconciliation. A transport or response failure
before a validated publish outcome has no verified receipt. Neither the
explicit calls nor `publish_and_activate` are an atomic transaction.

## Current implementation audit and caller contracts

The 2026-10-08 source audit distinguishes shared main `09387a9a`, the isolated
Quanta candidate, Semantica owned commits `5873774c51a` / `53b86d09049` /
`6f5dc8fc13f`, and
foreign producer migration inputs. The actionable path inventory, execution
order and merge conditions are in the
[SDK integration ticket](../plans/oct-4-parallel-closure/tickets/INDEX.md#sdk-구현-후보-통합-재감사--2026-10-08).
Offline preparation is now integrated in local main `433a9363`; the explicit
publication API and V5 consumer remain isolated. The audit and candidate results
below predate the split; current main verification is recorded in the next section.

The audit reproduced a preparation defect through two public API tests: with
identical source, recipe and unit set, a symbol coverage state change or empty
source language change was incorrectly omitted as unchanged. The repair stores
the canonical `SourceFileCoverage` directly in `PriorSourceEntry` and compares
all coverage fields. It does not add a separate digest or second metadata owner.
`PriorSourceEntry::new(stable_key, coverage, semantic_scopes)` and
`coverage()` expose this single owner; `source()`, `profile_sha256()` and
`unit_set_sha256()` derive from it. The unpublished prior serde tuple is now
`(stable_key, coverage, semantic_scopes)`. The incomplete five-field candidate
format is refused; there is no compatibility fallback or new engine wire IR.
Actual pre-fix execution failed both regressions; the repaired SDK selection
passed 152 unit tests and 5 external tests, including the coverage transitions.

The same audit executed the retrieval benchmark library selection and found
an exhaustive `SdkError` match that omitted `AfterPublish` (E0004, no test
bodies). The coupled repair in `benchmarks/retrieval/src/sdk.rs` preserves the
cause's status and code while adding the failed stage to its message. Two
classification tests pass, covering both stages, timeout/unavailable/error,
and exclusion of publication evidence from diagnostics. This is consumer
correctness, not an executed retrieval performance benchmark.

The canonical daemon rail then exposed an additional test consumer gap:
`e2e_lifecycle_history::activate_at_gate` expected a bare remote CAS conflict.
The repaired observer accepts only an Activation-stage `AfterPublish` whose
cause is the CAS conflict and whose durable original receipt/binding matches
the fixture publication. Observation failures and unrelated causes still fail.
The focused independent lifecycle history passed after repair; the initial
fail-fast rail ran51 passed/1failed and did not execute248 tests. A full rerun
subsequently passed300/300,10skipped (2slow/1leaky), exit0. The leaky
observation remains a separate process-cleanup investigation; passing test
bodies do not prove all child/FD shutdown behavior.
A diagnostic replay of the same300 tests with all reporter statuses passed
again (10skipped,2slow,0leaky). It did not identify the original leaky owner or
prove a cleanup repair. The common local scope runner now uses live
`--status-level leak` so future occurrences retain the test name without
printing ordinary passes. Its14 Python tests and affected-file Ruff check pass;
no native timeout, test selection or concurrency changed.

The API deliberately leaves these authority boundaries with the caller:

- `manifest_digest` is supplied by the producer. The SDK checks token shape,
  not an invented hash of the source universe. The producer must define what
  that digest commits to, retain it with the intended target and event, and
  validate returned original publication evidence against that intent.
- A prior manifest has no independent generation or event authority. The caller
  must associate it with the confirmed repo/revision/generation/event and pass
  the matching `base_generation`. A structurally valid foreign prior is not
  automatically authenticated by `reconcile_complete_universe`.
- `CompleteSourceSet` asserts the complete contribution universe under the
  supplied prior. Partial enumeration is not a valid omission-as-deletion input.
  Built-in profiles fail on invalid source rather than silently skipping it.
- The source revision participates in chunk identity. Passing a new repository
  commit as every source revision replaces every affected source identity even
  when its bytes are unchanged. This layer does not promise O(changed-files)
  ingest, hidden embedding reuse, or a peak allocator/RSS bound. The configured
  byte/count limits constrain admitted preparation and serialized batches.
- Current built-ins are exact UTF-8 text and Markdown. A generic adapter extension
  point is not implemented support for every repository format; another format
  must provide its own source-bound text and supported typed contributions.

The V5 snapshot and frozen publication consumer are implemented candidates;
Runtime aggregate behavior, the six durable barriers and the process fixture
remain `NOT_RUN`. The earlier isolated Runtime owner rerun with SDK code `e3499fce` and the R3 oracle
also ended in compile `FAILED`: 19 kernel-contract diagnostics, no test bodies,
finished owner verdict RED. Its source digest is
`2d2669674eef6208f5ff61a93f046e43b15cb02cc3b49df86db6d0ca9eda1399`;
`/tmp/sem-sdk-r3-audit-20261008-result.json` locates the immutable receipt.
All14 primary diagnostic files differ from main, so19 is not a current
shared-main error count. Source inspection does not establish
that the new producer closure compiles. The ticket separates this integration
work from R3's independent expected-partition oracle and full projection-store
cold restart. The R3 test-only oracle is now implemented with fixed G1/G2/G3
mutation/unchanged sets, exact parent members and two omission controls;
implementation is not an executed Runtime result. The process fixture preserves the external projection authority
and uses a test manifest observer; it does not prove an installed Linux rollout.

## Verification and acceptance boundary

### Current main split

The preparation unit lowers into the existing batch API. Shared membership
canonicalization runs before both lowering and semantic stamping. Reversing
identical memberships first reproduced a public-test failure, then passed with
identical wire, prior stamp, CBOR resume and no-op; actual changes still replace.
The budget contract is `max_input_bytes`, `max_emitted_text_bytes` (chunk plus
semantic text), and `max_batch_bytes` (serialized contributions and final wire).
Original input and symbol/semantic metadata have their respective input/batch
limits; emitted text is not an allocator or RSS limit.

| Command | Observed result | Scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-sdk --lib --test preparation_public --locked -- --quiet` | `VERIFIED`: 145 unit + 7 external tests | Integrated preparation, existing publication API; includes input/text/metadata/batch boundaries and canonical membership |
| `./scripts/cargow test -p quanta-index-searchctl -p quanta-index-retrieval-bench --lib --locked` | `VERIFIED`: 46 CLI + 125 benchmark tests | Current-enum consumer backport, cause classification and original evidence; no benchmark execution |
| `./scripts/cargow clippy -p quanta-index-sdk --lib --tests --locked -- -D warnings` and corresponding CLI/benchmark command | `VERIFIED` | Production and test targets; initial doc lint was repaired and rerun |
| `just rust-public-api`, `just rust-cargo-modules`, `just rust-hexagonal` | `VERIFIED` | Preparation API baseline and module/architecture boundaries; main still has the old publication enum |
| `just rust-profile validate-shared-surface` | `VERIFIED`: 1,079 shared + 248 integration (8 skipped) + 25 CLI | Current split; independent from the historical candidate's 1,087 shared tests |
| `./scripts/cargow --lane daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --message-format=json-render-diagnostics` then `QUANTA_INDEX_L2_TEST_BINARY=<actual artifact> ./scripts/cargow test -p quanta-index-sdk --test l2_daemon_publication --locked prepared_text_and_markdown_move_delete_noop_survive_restart -- --ignored --nocapture --test-threads=1` | `VERIFIED`: build + 1 process scenario (4 filtered) | Fresh main daemon; full/move/delete/no-op/prior resume and query after restart. Not all five publication scenarios or the full daemon rail |
| `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_run_local_test_scope.py tools/ci/tests/test_proof_operational_result.py -q -o cache_dir=/tmp/qi-final-integration-pytest` and affected-file Ruff | `VERIFIED`: 75 passed, lint clean | Independent tooling integrated in `23b5659f`; no real-host install proof |

The fresh main daemon SHA-256 was
`1005ff0b3034dd881c3d89875573d382530105db5e6ffb741b20101acca0fbdd`.
The process command consumed the actual compiler artifact. No prior candidate
binary was substituted. Remote CI, push, installed-host and scale qualification
are `NOT_RUN` for this integration.

### Historical full-candidate selection

These focused checks executed before the current split and later preparation
repairs. They cover their recorded candidate only:

| Command | Observed result | Scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-sdk --lib --test preparation_public --locked` | `VERIFIED`: 152 unit tests and 5 external integration tests passed | Text/Markdown and custom adapters, exact Unicode spans, deterministic identity, budgets, complete-universe no-op/delete/move, coverage-only changes, canonical semantic authority, prior-manifest serde, original-publication activation, and post-publication failure evidence |
| `./scripts/cargow test -p quanta-index-retrieval-bench --lib --locked classification -- --nocapture` | `VERIFIED`: 2 passed, 123 filtered after the E0004 repair | Typed cause classification and stage context after successful publication, without receipt payload diagnostics |
| `./scripts/cargow clippy -p quanta-index-retrieval-bench --lib --tests --locked -- -D warnings` | `VERIFIED` | Benchmark consumer library and test targets; no retrieval benchmark execution |
| `./scripts/cargow test -p quanta-index-contract --lib ipc::ingest_observation --locked` | `VERIFIED`: 9 tests passed | A durable publication requires a positive sequence, including empty apply and replay; precommit draft validation remains separate |
| `./scripts/cargow --lane daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked sdk_frontdoor -- --nocapture --test-threads=1` | `VERIFIED`: 21 passed, 47 filtered | SDK route, composite generation, pinned/restart/replay, timeout/refusal and cross-surface frontdoor integration |
| `./scripts/cargow --lane daemon-lane test -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked e2e_ingest_preflight -- --nocapture --test-threads=1` | `VERIFIED`: 2 passed, 93 filtered | Canonical SDK publication, remote preflight refusal and unchanged durable state on refusal |
| `./scripts/cargow clippy -p quanta-index-sdk --lib --tests --all-features --locked -- -D warnings` | `VERIFIED` | SDK production and test targets |
| `./scripts/cargow clippy -p quanta-index-searchctl --all-targets --all-features --locked -- -D warnings` | `VERIFIED` | Coupled CLI error mapping compiles and preserves the cause's exit class |
| `./scripts/cargow --lane daemon-lane clippy -p quanta-index-searchd-runtime --test runtime_fast_suite --test runtime_extended_suite --all-features --locked -- -D warnings` | `VERIFIED` | Both affected Runtime integration targets compile with strict Clippy |
| `QUANTA_INDEX_L2_TEST_BINARY=<fresh compiler artifact> ./scripts/cargow test -p quanta-index-sdk --test l2_daemon_publication --locked -- --ignored --nocapture --test-threads=1` | `VERIFIED`: 5 process tests and all 8 crash cuts passed | Preparation full/move/delete/no-op, original publication and separate activation CAS, source-event replay, cross-stream fencing, and real daemon restart |
| `just rust-profile validate-shared-surface` | `VERIFIED`: all selected compile targets; 1,087 shared + 248 integration (8 skipped) + 25 CLI tests passed | Canonical shared surface, integration-fast and CLI-smoke rail; no Semantica Runtime or daemon rail |
| `just rust-profile test-daemon` | `VERIFIED`: 300 passed, 10 skipped; 2 slow, 1 leaky, exit0 after lifecycle consumer repair | Canonical25 catalog rows/5 binaries. Leaky stdout/stderr lifetime is a separate unresolved cleanup observation, not a memory-leak diagnosis |
| `./scripts/cargow --lane daemon-lane clippy -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -- -D warnings` | `VERIFIED` | Affected lifecycle observer and extended Runtime tests |
| `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_run_local_test_scope.py -q` and affected-file Ruff | `VERIFIED`: 14 passed; Ruff clean | Named leaked-output diagnostics; original intermittent cleanup observation remains unresolved |
| `python3 tools/ci/lint/check-public-api.py --packages quanta-index-contract quanta-index-sdk` | `VERIFIED` | Generated baseline reviewed and both public surfaces match |

Cross-repository Runtime snapshot and publication-recovery tests, installed
process, and performance qualification are separate scopes. They remain
`NOT_RUN` until their own execution completes;
these focused results do not qualify a release. Semantica has an implementation
candidate for cumulative current semantic-owner state in the existing prepared
artifact V5. Its G1-to-G2-to-G3, retention, and restart behavior require actual
Runtime QBC tests; source inspection alone does not establish acceptance.
### Historical dependency attempts

Earlier isolated Runtime attempts stopped before test bodies: parser271,
then lock reconciliation, interproc/PTA91, two localized Java contract/lifetime
errors, and kernel-contract19. The last old result is
`/tmp/sem-sdk-shadow-final-v6-result.json`; it is a failed captured-input
receipt, not a current-main error count. All14 files behind those19 diagnostics
have changed in main. The isolated Java repairs now also exist in main.
Foreign migration inputs are not owned SDK/V5 changes and must not be staged
as this feature. Current candidate execution and remaining closure are owned by
the integration ticket; old diagnostic arithmetic is not a new compiler result.

### Frozen publication correctness

The final coupled source audit also found a frozen-aggregate replay hazard:
the SDK may correctly recover an original publication for a retargeted source
event, while the aggregate still requires the submitted target. Activating that
original target and rejecting its receipt afterward changes visibility before
rejecting the frozen aggregate. The candidate consumer now obtains the publish
outcome, compares the complete original publication binding with the frozen
submission before control CAS, and retains original `AfterPublish` evidence on
mismatch. Same-identity replay still activates. Two owner regressions cover
zero activation calls on retarget and the successful same-identity replay.
The QBC kernel `index_sdk_ingress::publish` selection completed with a GREEN
finished owner receipt: 28 passed, including both new regressions; 81 filtered.
This proves the ingress owner decision before activation, not a full Runtime
aggregate process or real control IPC. Pre-fix runtime reproduction was
`NOT_RUN`; the defect was established by the reachable source path.
Semantica QBC has executed the error-classification test (1 passed), existing
prepared-receipt tests (9 passed), and the exact V5 snapshot-byte artifact
reference/closed-serde test (1 passed). Each finished owner receipt is GREEN;
these counts cover separate selected behavior, not a Runtime or paired release.
The first contract test compilation failed on an existing native-corpus test
import and private helper visibility; the test-only owner repair preserves its
validation assertions. Full boundary and parser gate failures remain recorded
in the [closure ticket](../plans/oct-4-parallel-closure/tickets/INDEX.md).

## Main comparison and merge disposition

The audit compared Quanta base `09387a9a` with candidate `3c08aab5`, and
Semantica main `3b8f86dd9ea` with candidate `6f5dc8fc13f`. The 37/27 paths
are historical inventories, not indivisible merge units. The current candidate
code checkpoint is `0dd6ce81` (38 paths including the new CLI regression). Its
SDK156+external7 and CLI46+benchmark125 selections, strict Clippy, and both
public API checks passed after the current repairs. These candidate results do
not qualify main or the Semantica Runtime. Main now includes:

| Unit | Local integration |
| --- | --- |
| Existing publication-error consumers | `c75668c7`: current `ActivationAfterPublish`, fixed cause exit/status and retained original evidence |
| Nextest reporter + operational paths (4 files) | `23b5659f`: retain leaked-test diagnostics; reject lexical path aliases/overlap before actors |
| Offline preparation (9 paths/shared hunks) | `433a9363`: coverage owner, shared membership canonicalization, precise text budgets, public regressions and existing-API L2 |
| Breaking publication + Semantica kernel4 | Held for current-pair ingress and actual aggregate caller/replay/CAS/restart proof |
| V5 contract/runtime20 Rust | Held as one writer/reader/test contract, requiring transition, Runtime, durable and process proof |
| Native helper1 + Semantica lock2 | Separate helper if required; reconcile current lock edges rather than copy stale snapshots |

The reproduced main E0004 omissions and both preparation findings are repaired
and verified above. The unexplained historical Nextest leaky observation remains
cleanup triage; the reporter preserves future test identities. It is not a
blanket gate on independent preparation/tooling.

V5 rejects completed V4 artifacts without canonical semantic-owner state.
Upgrade requires explicit full ReplaceGeneration to establish a V5 baseline,
then V5 delta, preserving event ancestry and frozen expected-active CAS. Do not
synthesize snapshots or silently retry a failed delta as full. Added candidate
regressions cover V4 artifact refusal, lexical shadow on/off, and actual missing
V5 predecessor refusal/no activation/retry. V4 artifact refusal passed its exact
owning QBC selector (1 passed, 712 filtered); feature and Runtime/process tests
remain pending.
They do not establish a completed V4-to-V5 process transition or full cold-root
recovery. Aggregate authority already requires `sqlite-store` in current main;
no-SQLite aggregate refusal is not, by itself, a newly introduced regression.
The supported feature matrix still needs owning execution.

The [integration ticket](../plans/oct-4-parallel-closure/tickets/INDEX.md#sdk-구현-후보-통합-재감사--2026-10-08)
owns the exact remaining files, final pair inputs and acceptance sequence.

## Remaining coupled integration

### Current canonical pair result

A fresh private pair captures Semantica main `3b8f86dd9ea` plus 3,976 current
canonical dirty paths, then applies the owned 25 Rust files and reconciles the
SDK dependency edges in both current lockfiles. Actual siblings are the final
Quanta candidate and clean QGLang `9a9212da9526d50f9b264d7ee34dbe4cd0b03fd9`.
Owning ingress QBC passed 28 tests (81 filtered). Runtime QBC failed before test
bodies: two unused imports first; after a private import-only repair, E0599 at
`quanta-runtime-query-surface-build-context/src/build_context/mod.rs:446`.
That current canonical caller uses removed `.as_ref().clone()` on the verified
output carrier. The second finished owner receipt is RED at source digest
`cb75211dccc0f8bf4ab6c6a1efadfaebebaa3e42470a0fff11c685e07c4ac4b8`.
Repair the canonical BuildContext controlled output read/copy closure; do not
restore raw getters, invent current loans or disable features. Shared Semantica
main is untouched. Thus D/E integration remains held, with durable/process
`NOT_RUN`. Kernel success does not qualify the aggregate caller. Prepared-receipt selection
passed 9 tests; exact V4 baseline refusal and V5 snapshot/digest binding each
passed 1, with finished GREEN and hash-bound publication-complete markers. A wider artifact selection
failed (297 passed, 1 failed) at the manifest-lane fixture baseline coverage check
(`commit_and_lane_status.rs:143`); it is not a green contract-suite result.

### Preserved candidate and historical dependency attempts

The Quanta candidate is based on `09387a9a` in
`/Users/songmin/.codex/worktrees/sdk-preparation-final/quanta-index`.
The Semantica candidate is based on `109fd7575d2` in
`/Users/songmin/.codex/worktrees/sdk-publish-recovery/semantica-sdk`; its sibling
Quanta checkout contains the same SDK code. Foreign native migration inputs
are not owned publication changes. Keep the breaking SDK and its ingress
consumers coupled; treat V5 as the separate unit above:

Semantica's owned V5/error-consumer/test changes are saved locally as
`5873774c51a`, frozen-publication repair `53b86d09049`, and fixed-partition
test `6f5dc8fc13f` on `codex/semantic-publication-v5` (27 owned files across
three initial commits). Additional V4/baseline/feature regressions are preserved
in `1193ea8024e` (three owned test files); staged formatting and safety hooks
passed. Runtime behavior tests remain unexecuted. The R3 source
was included in the fresh failed Runtime compile; no R3 test body executed. The foreign parser/native
migration remains unstaged; this is a review candidate, not a qualified Runtime
or published pair. The subsequent Runtime compile failure prevents V5
acceptance. A publication-only split also needs fresh ingress tests and actual
existing aggregate caller/replay/CAS/restart proof; kernel28 alone does not
qualify that split. No such split has been integrated.

1. Obtain a complete interproc/PTA native carrier owner bundle. Its callers must
   consume the admitted carrier, fallible accessors and authentic current/output
   loans. Restoring legacy fields, `Clone`/`Eq`, or weakening feature gates is not
   a repair. The Java ordinary parser consumer closure also remains unqualified.
   A read-only comparison first found 58 newer foreign paths plus ten referenced
   child differences. The declared `canonical_verified_operations_v3.rs` was
   initially absent; the owner subsequently created it. That single addition
   does not complete the dependency closure. A broader source comparison of
   the 283 recorded dependency crate roots found 217 composable differences
   and 50 conflicts with existing isolated parser/PTA proof repairs. That
   prospective capture was not applied. The two lockfiles also need explicit
   composition with the SDK dependency edges. Reconcile the canonical owner
   bundle as a whole; do not apply only the non-conflicting subset or fabricate
   old accessors, copies or current loans to make the build pass. The current
   audit found 64 of those 217 old input paths had changed again. The 50 conflicts
   were between foreign proof inputs, not the owned 27-file feature: current main
   still has 23 unchanged owned Rust preimages, two absent new files and only
   two overlapping lockfiles. Prefer the complete canonical producer source
   plus the owned 25 Rust changes and reconciled locks over replaying stale
   foreign repairs. See the ticket for the precise owner and test sequence.
2. Run the Runtime `shadow_delta_orchestration` module through
   `scripts/quanta-build-cli owner run`, package `quanta-runtime`, `test/lib`,
   compile policy `feature-isolation:quanta-runtime.no-default.9c3270892708`,
   `--ignored-policy exclude`. Then run the six durable barrier selectors listed
   in the closure ticket through that same owner.
3. Run both ignored process selectors: the exact selector ending in
   `shadow_delta_orchestration::completed_v5_publication::completed_v5_publication_prune_restart_then_g3_delta_v1`
   and `shadow_delta_orchestration::completed_v5_publication::completed_v5_baseline_refuses_missing_predecessor_before_activation_then_retries_v1`,
   with `--ignored-policy only`, `QUANTA_INDEX_SEARCHD_BIN`, and a verified
   `QUANTA_INDEX_SEARCHD_SHA256`. The process test is explicitly ignored by default;
   it must not silently pass without its daemon. It completes real aggregate
   G1/G2 publications, deletes the G1 artifact, reopens SQLite and a fresh artifact
   reader, restarts SearchPlane, and drives production G3. Fixed owner identities
   check retained owners, renamed tombstones and parent membership. The fixture
   and its owner-issued `cfg(test)` closure helper are implemented but `NOT_RUN`.
4. Integrate publication recovery with its full SDK/contract/CLI/benchmark/test/API
   consumer bundle and Semantica's four ingress files after the selected pair
   passes. V5's 20 producer/reader/test files require the separate transition,
   owner and process proof above. Source-based splitting does not authorize
   arbitrary partial V5 application. Recheck the selected final pair before
   publication. Passing Quanta tests does not qualify Semantica Runtime.

The process fixture retains the external projection/manifest owner. It does
not prove projection-store cold restart: `AtomicIndexProjectionWriterV1::new`
creates empty roots; attaching the prepared artifact store does not restore them.
`maybe_reuse_persisted_prepared_commit_v1` and
`published_chunk_identities_by_path_for_generation_v1` require the existing root.
This is an existing recovery seam, distinct from V5 semantic snapshot loading;
its Runtime reproducer is `NOT_RUN`. Do not infer a root from a manifest or
introduce a second durable protocol inside the source-preparation SDK.

Actual Linux installation/activation/restore needs the authorized target host,
binary/config/state destinations, backup/retention policy and independent
observer. These inputs remain `BLOCKED`. New-SDK scale/performance qualification,
full-suite and remote CI acceptance remain separate `NOT_RUN` scopes.
