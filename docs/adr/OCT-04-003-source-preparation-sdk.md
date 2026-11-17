# OCT-04-003 — Source preparation SDK

Status: `Implemented source contract`.

The preparation/publication bundle was integrated in Index main `3aeebae8`
and included in the Oct-10 source `de279c45`. That history is not current-tree,
Semantica, installed-host, performance or release qualification.

Offline preparation owns source adapters, prior comparison and checked lowering.
The `SearchCorpusBatch` and publish/activate examples below use the generation-based
Legacy API family. They do not define selected-view Native publication or turn a
preparation hint into Source, membership, view or execution authority. Native
contracts remain in [NOV-12-001](NOV-12-001-native-query-and-semantic-engine.md).
The public ingress contract remains [MAY-27-002](MAY-27-002-sdk-ingress-and-public-surface-boundary.md).

Current source owners are [preparation](../../crates/quanta-index-sdk/src/preparation/implementation.rs),
[publication facade](../../crates/quanta-index-sdk/src/lexical.rs),
[client constructors](../../crates/quanta-index-sdk/src/client.rs) and
[profile selection](../../crates/quanta-index-sdk/src/config.rs).
Ordinary `connect` resolves Native; callers of the Legacy transport example use
explicit `connect_legacy`. Pure offline preparation requires no connection.

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

`publish_outcome` and `activate_published` are existing public methods on the
Legacy search-corpus facade. They are not an uncommitted API proposal and do not
require another client/decoder. Use a Legacy-connected client for this family.

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

This generic SDK example activates the verified original target. A frozen
aggregate must additionally compare the complete original
`SourcePublicationBinding` with its frozen submitted binding **before** CAS.
The producer must retain the original evidence on a retarget mismatch without
issuing activation; actual caller integration is owned by E01/E02. A successful SDK replay alone
does not authorize advancing a different aggregate's planned manifest.

`SdkError::AfterPublish` retains the original evidence and typed cause when
observation after a verified publish or activation fails. Its
`PublishedBatchFailureStage` is `Observation` or `Activation`; callers can read
`SdkError::published_evidence()`, `published_receipt()`, and
`published_publication()` for reconciliation. A transport or response failure
before a validated publish outcome has no verified receipt. Neither the
explicit calls nor `publish_and_activate` are an atomic transaction. The stage
identifies the failed operation, not whether CAS took effect: an acknowledgement
or elapsed-time conversion can fail after activation. Reconcile the active head
before retrying or confirming producer state.

## Caller authority and resource boundaries

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

The budget contract separates input bytes, emitted chunk/semantic text and
serialized contribution/final-batch bytes. Symbol/semantic metadata consumes its
actual input/batch limit. Shared membership canonicalization precedes lowering
and semantic stamps. Full `SourceFileCoverage` is the prior comparison owner;
coverage/language/completeness-only changes cannot become unchanged. These byte
limits are not an allocator/RSS bound or a promise of O(changed-files) ingest.

## Coupled producer and recovery boundary

- A frozen producer validates the actual canonical snapshot against its Lexical
  batch and original publication binding; a V5 envelope/tag alone is insufficient.
- Preserve authentic first-V5-FULL baseline then DELTA ancestry/expected-active
  CAS. Do not synthesize a missing predecessor, retag V4 as V5 or silently retry
  a rejected delta as FULL.
- Admit encoded size, decode/re-encode/validation allocations, full-scope copies
  and repeated reads under their real Original/custody. A wire ceiling does not
  establish peak memory, bounded decode work or an interruption ceiling.
- Fresh recovery needs actual source closure and current host/root/receipt
  custody. For generation-based paths, reuse one durable occupied-generation
  authority; active-plus-one does not account for immutable sealed generations.
  This recorded supply constraint is not a request to add a second allocator or
  to translate a Legacy generation into Native view/Source authority.

## Verification and acceptance boundary

The Oct-8/9 candidate/main tables, isolated branches, compiler attempts and remote
HEAD observations are historical. They neither form a current missing-code list
nor qualify another producer/cohort. Their exact commands, binary/source digests,
run IDs, failures and limits are in the [recovery record](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup).

The earlier process rail's 300 passing bodies and diagnostic no-leak rerun did
not identify the original leaky owner or prove a cleanup repair. The historical
512MiB artifact wire bound was not a global Native frame, allocation or RSS bound.
Actual current format/request limits, feature floors and owning body inventory
must come from their real consumers. No source-only/API/static gate closes the
six durable barriers, Runtime aggregate, native process or installed rollout scope.

## Remaining coupled integration

[E01](../plans/oct-22-finalizing/E01-producer-generation.md) and
[E02](../plans/oct-22-finalizing/E02-publication-recovery.md) own the actual complete
producer/caller, same-source frozen binding and recovery joins. Current SDK/Runtime
execution and public Published acceptance follow the existing
[NC-F packet](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#p07-final-actions-20261115).
[E08](../plans/oct-22-finalizing/E08-integration-release.md) retains installed,
independent-process, platform, performance, CI and release acceptance. Reuse valid
source-bound proof; do not replay an old fixed-count request or infer current
compiler errors/active claims from the retired pair tables.

## Historical checkpoint recovery

The old section fragments below resolve to the exact preserved source. They are
historical evidence, not another implementation or verification queue.

| Retired section | Original record |
| --- | --- |
| <a id="2026-10-09-integration-checkpoint"></a>2026-10-09 integration checkpoint | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="current-main-follow-up"></a>Current-main follow-up | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="historical-isolated-pair"></a>Historical isolated pair | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="2026-10-08-implementation-audit-and-caller-contracts"></a>2026-10-08 implementation audit and caller contracts | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="current-main-split"></a>Current main split | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="historical-full-candidate-selection"></a>Historical full-candidate selection | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="historical-dependency-attempts"></a>Historical dependency attempts | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="oct-8-frozen-publication-correctness-historical-candidate-proof"></a>Oct-8 frozen publication correctness (historical candidate proof) | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="main-comparison-and-merge-disposition"></a>Main comparison and merge disposition | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="current-pair-inputs-and-result"></a>Current pair inputs and result | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |
| <a id="preserved-candidate-and-historical-dependency-attempts"></a>Preserved candidate and historical dependency attempts | [preimage](../ARCHIVE-INDEX.md#2026-11-15-sdk-producer-contract-cleanup) |

Earlier owner/reference routes: [record](../ARCHIVE-INDEX.md#oct-10-plan-retirement), [record](../plans/oct-22-finalizing/E08-integration-release.md#release-inputs), [record](../plans/oct-22-finalizing/README.md).
