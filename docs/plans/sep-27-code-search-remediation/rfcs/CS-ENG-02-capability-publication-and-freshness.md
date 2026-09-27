# CS-ENG-02 — Capability publication and freshness

Status: engine contract and L2 implementation present; whole-product qualification
**BLOCKED**. Current implementation, owner proof and native daemon recovery
evidence: [L2 handoff](../handoffs/L2_HANDOFF.md) and
[L2 process audit](../handoffs/L2_PROCESS_AUDIT.md). The problem description below
records the pre-cutover baseline; unchecked DoD items are not completion claims.
Category: engine publication. Findings: F06; G01 remains a lifecycle proof gap.
Final audit: E03/E04 in [engine-audit.md](../engine-audit.md). Depends on ENG-01
and the PROD-01 producer payload agreement.

## Current disposition and remaining acceptance

Audit source: `b42a9b5d` plus dirty overlay; full identity and remaining work are
in [CS-INT-01](CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).
Canonical file coverage, source-event lineage, immutable generation binding and
strict incomplete-symbol refusal are implemented. Earlier owner/recovery passes
are historical execution evidence; this audit does not reopen those repaired
defects or promote them to final-source qualification.

- **L2-C1, VERIFIED source cost mechanism; performance qualification NOT_RUN:**
  `sealed_generation/coverage.rs::write_staged_coverage` serializes all effective
  coverage rows into one `source-file-coverage.cbor` on a changed generation.
  A one-file Delta therefore incurs O(admitted files) coverage serialization/write
  cost. The previous 169,994-byte sample belongs to the pre-format-8 source and is
  historical, not a current measurement. Measure total fresh bytes and touched
  shards at increasing corpus sizes; declare an admitted cost ceiling. If that
  ceiling requires incremental persistence, implement committed shard inheritance
  with replacement/tombstone, tamper, restart and reclaim controls. This is a cost
  issue, not a reproduced stale-result or atomicity defect.
- **INT-R1, NOT_RUN:** rerun combined publication, wrong-source, cross-stream,
  crash/restart and migrated storage fixtures after the format-8 merge. Execute
  the existing real-daemon SDK cases explicitly; default ignored discovery is
  not execution. External producer cutover and installed activation are separate
  acceptance boundaries.

## Purpose and RCA

The following problem description records the pre-cutover baseline; it is not
a statement that the current typed coverage/publication contract is absent.

Permit text search over valid admitted source bytes without claiming complete
symbol extraction for malformed or unsupported files. The current benchmark
requires all-file symbols before publication; Vite therefore produces no lexical
query results. That is an intentional **benchmark producer** admission policy,
not a product completeness gate or evidence of partial publication. The product
`SearchCorpusReplaceScope` carries scope/digest/chunks/symbols, without per-file
complete/unsupported/failed extraction authority. Its symbol endpoint can return
exact zero without proving parser coverage. Add that product authority; preserve
the benchmark's existing strict policy instead of assuming the product has it.

Current starting points:

- [Ingest contract](../../../../crates/quanta-index-contract/src/ipc/ingest.rs),
  [batch body](../../../../crates/quanta-index-contract/src/ipc/batch_body.rs).
- [Lexical ingest](../../../../crates/quanta-index-lexical/src/adapter_ingest.rs).
- Producer [batch](../../../../benchmarks/retrieval/src/batch.rs),
  [symbols](../../../../benchmarks/retrieval/src/symbols.rs),
  [runner](../../../../benchmarks/retrieval/src/main.rs).
- [SDK publication boundary](../../../../benchmarks/retrieval/src/sdk.rs).

These are ingress owners, not an exhaustive lifecycle consumer inventory.
Inventory persistence, activation, registry, SDK and cursor readers before cutover.

## Fix the file mutation identity before adding coverage states

The final audit reproduced, using cached native components, accepted same-path
Chunk and Symbol scope mutations overwriting one another: Chunk→Symbol leaves
only symbols; reverse order leaves only text. Admission keys scopes by
`(doc_surface, path)`, but lexical replacement/deletion uses the path across both
kinds. A combined File replacement works. This is an ingress/write-owner mismatch,
not a reason to split publication into separate transactions.

Another accepted payload binds scope `a.rs` but contains records at `b.rs`.
Tombstoning `a.rs` leaves those records. Validate the source-file owner of **every**
contained record before materialization, not only field shape and digest syntax.

Adopt one canonical lexical file-replacement key, with chunks, symbols and coverage
in one payload. Reject alternate Chunk/Symbol scope aliases that overlap that
same file, replace/tombstone conflicts, mismatched internal paths and cross-kind
ID collisions at public ingress and defensive materialization boundaries. Validate
federated source identity consistently with ENG-03; a pin-relative path alone
cannot identify two different source files. Recompute/bind content digests from
validated canonical payloads rather than trusting a matching caller label.

Canonical payload digest verification is not independent verification of the
entire source file. When ingress receives only facts/chunks, retain that
producer-attestation boundary. An independently verified full-source claim needs
the immutable source bytes/reference and their validation; never pretend to
recompute bytes that were not provided.

Owners include [scope validation](../../../../crates/quanta-index-contract/src/ipc/ingest.rs)
and [path-wide mutation](../../../../crates/quanta-index-lexical/src/adapter.rs),
plus dispatcher/materializer preflight. Existing whole-file deletion already
removes old symbols on a correctly formed combined replacement: do not describe
that supported path as a newly demonstrated stale-symbol bug.

Independent `clear_surfaces: Symbol` must not leave Complete coverage after
deleting its facts. Initially reject such independent clears on coverage-bound
canonical generations. Any supported whole-surface clear must atomically remove
or invalidate every affected file's capability and unit commitment. Define
Chunk/File/Module clear semantics explicitly; do not infer them from a shared
surface enum or change a retained old snapshot in place.

## Decision: source-bound capability state

Each file revision binds repository/path/source hash, text admission, symbol
coverage state, producer identity and published unit-set digest. Suggested states
below describe semantics; adopt names through the existing versioned contract.

| Symbol state | Meaning | Strict symbol exhaustion allowed? |
| --- | --- | --- |
| Complete, zero or more facts | Supported parse completed and extraction contract satisfied | Yes, within complete requested scope |
| Not requested | This profile did not attempt extraction | No |
| Unsupported | Producer has no admitted capability for this language/form | No |
| Parse failed | Supported input failed syntax/producer validation | No |
| Producer failed | Crash, timeout, cancellation or resource failure | No |

Zero facts is a cardinality of Complete, not an error fallback. A recovered parse
with ERROR/MISSING nodes does not imply completeness. Capability descriptors bind
parser/grammar revision, lockfile, language and extraction policy.

Store an admitted-source universe, including empty files and files with zero
chunks/symbols. Completeness cannot be inferred from returned facts. Bind coverage
as an artifact of the existing lexical sealed generation: manifest commitment,
seal/open/scrub, delta inheritance, recovery and reclaim all include it. Use the
same generation read handle, not an independently mutable registry.

Strict coverage is checked over the effective repo/path/language source scope
before result matching, and for every resolved plan that needs symbol authority,
not just the endpoint named "symbol". A delta inherits unchanged files' coverage;
checking only updated files would hide an incomplete base. Producer completeness
is attested under its pinned extraction policy; engine integrity checks do not
independently prove that a parser found every declaration.

Profiles state whether they require text, complete symbols, or both. A text-only
profile may publish text plus a typed symbol-coverage status. A strict symbol
query validates completeness over its effective repository/path scope before an
exhaustive no-answer claim. Initially refuse incomplete strict scopes with a
typed coverage error. Partial symbol answers require a separately explicit mode
and response completeness metadata; they are not needed to close this RFC.

## Atomic update and lifecycle invariants

The unit of replacement remains one source file revision: text chunks, symbols
and capability state advance together. Do not split them into independently
visible publications.

1. Prepare all new units and coverage metadata under an inactive generation.
2. Validate supplied source identity and payload binding, identity collisions,
   spans and scope digest; independently verify source bytes only when available.
3. Commit/activate through the existing atomic publication owner.
4. Queries bind one active snapshot. Old readers may finish on the old snapshot;
   new readers cannot observe new text with old symbols or missing coverage state.
5. If a newer text-admitted revision fails symbol parsing, its active symbol set
   is empty with failure status; prior symbols cannot masquerade as current facts.
6. If the selected strict publication profile rejects the update, old active state
   remains old, explicitly identified; an ingest failure is not a new revision ACK.
7. Rename/delete advance current file identities. A cursor pinned to an immutable
   retained old snapshot remains valid for that snapshot; reject cross-snapshot
   reuse and released/expired pins, not every old cursor after any edit. Source-event
   idempotency is a new requirement, distinct from existing body-digest replay.
   The producer supplies stream/event identity and expected source base; ingress
   validates it against the committed owner state. Same event/different payload
   is a conflict, not a retry. Existing activation CAS/generation monotonicity
   does not prove source-event freshness when stale content is repackaged with a
   higher generation: bind the producer's expected base/source revision or event
   lineage, and refuse conflicts rather than using wall-clock ordering.
8. Crash or uncertain acknowledgement is reconciled from committed state. Do not
   issue a second unbound activation or label ACK time as search-visible time.

Preserve publication/GC custody already owned by MISC-01. This RFC adds domain
capability semantics, not another staging, transaction or cleanup framework.

## Architectural boundary and costs

Parsing stays producer-owned. Searchd consumes validated facts and does not gain
a Tree-sitter/compiler dependency. SCIP is optional producer interchange when
precise occurrences exist; no full reference-resolution subsystem is required.

Text-only search does not prove embedding-free construction. Record actual
profile execution: extraction, chunking, embedding, publication and indexing.
Eliding a stage must be supported by the real producer/daemon contract and tested;
do not infer skipped work from a label such as `model: none`.

The initial cutover retains canonical semantic derivation and paired
lexical/semantic activation. Removing embeddings or introducing independently
activatable lexical-only generations is a separate lifecycle change, not an
implicit consequence of optional symbol capability.

This changes accepted coverage/publication contracts. Update SEP-26-001 and all
schema/registry/evaluator consumers atomically upon acceptance. Reject unknown
coverage stamps; do not silently import old records as Complete.

## Tests and DoD

- [ ] Valid zero-symbol, unsupported, malformed and producer-timeout states are
  distinct in producer output, SDK responses and evaluator evidence.
- [ ] Public decode/admission/materialization reject same-file surface aliases in
  both orders, replace/tombstone overlap and scope/record path/source mismatch.
- [ ] Clear-surface combinations cannot retain false Complete coverage, including
  inherited base entries; rejected clears perform zero mutation.
- [ ] Source-event replay, same-event/different-payload and stale-event/newer-
  generation cases distinguish source lineage from generation/CAS ordering.
- [ ] Correct combined replacement still preserves text+symbols; delete removes
  precisely its owner file, including federated equal-path controls.
- [ ] Lexical-only profile searches the admitted malformed-file bytes; strict
  symbol scope refuses incomplete coverage without reporting exhaustive no-answer.
- [ ] Full replacement, text-only update, symbol-only fact change, rename/delete,
  parse failure then repair, duplicate/stale update and restart are exercised.
- [ ] Read-while-write tests see only complete old/new snapshots and source-bound
  registry spans; wrong-context cursors and wrong-source facts reject while valid
  retained old-snapshot cursors continue under the existing lifecycle contract.
- [ ] Fault injection before/after staging, commit and activation proves recovery
  semantics through the installed daemon, not only an in-memory fake.
- [ ] Capability metadata participates in scope/custody digests and source closure.
- [ ] Empty-file coverage, inherited incomplete base files and absent/tampered
  coverage artifacts have explicit strict-query, seal/open and restart tests.
- [ ] Measured update visibility and cost are reported by BENCH-04, separately
  from local invariant tests and from ingest acknowledgement latency.

Independent oracle: known source revisions and expected declaration sets in small
fixtures, plus externally observable snapshot/query results. No stale-state bug
is declared fixed until the relevant sequence has actually been executed.

References: [S02, S04, S05, S06](../references.md). Rollout and source-bound proof:
[CS-INT-01](CS-INT-01-integration-and-qualification.md).
