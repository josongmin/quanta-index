# SEP-26-003 — Retrieval Evidence Custody and Qualification Boundaries

Status: `Accepted`

Decided: 2026-09-27

Source campaign: RBR-00 and RBR-12; applies to RBR-01 through RBR-11

## Context

The remediation campaign produced implementation checks, focused tests, clean-source receipts, actual process probes
and exploratory product pairs. Those evidence classes are not interchangeable. The remediation packet, these SEP-26
ADRs and proof tooling are part of the retrieval source closure, so changing them invalidates earlier current-source
receipts.

## Decision

### Evidence layers

1. **Implementation**: code and accepted ADR contract exist.
2. **Owner-local proof**: the owning unit, fixture or bounded process check passes on an identified source and input.
3. **Current-source integration proof**: exact selected, executed and passed inventories finish on one immutable clean
   source with bound dependencies, configuration, binaries, environment and raw terminal output.
4. **Product qualification**: admitted external corpus/model, independent gold, frozen development/holdout custody,
   actual product pair, fresh-process replay and required quality/performance gates pass.
5. **Release or deployment**: release-specific admission, deployment and activation evidence passes.

Higher layers cannot be inferred from lower layers. Compilation is not a test pass; focused tests are not repository
qualification; a diagnostic pair is not quality or speed qualification.

### Receipt validity

A receipt binds the full Git revision and dirty state, source closure manifest, selected and executed test identities,
input and dependency digests, configuration, binaries, host/runtime, exact command, exit status, raw output and artifact
digests. Any bound input change makes the receipt stale.

Validators fail closed on missing, malformed, stale, duplicate, partial, reordered, forged, wrong-source,
wrong-environment, timed-out, interrupted or tampered evidence. A `pass` boolean, count, report summary or locally
self-reported hash is not an independent oracle.

Evidence documents, referenced raw files and optional latest/baseline pointers
are read through one no-follow path-custody boundary. The reader verifies the
regular leaf and directory ancestry before and after consumption and uses
descriptor-relative no-follow opens; a prior `is_file` or `exists` check does
not authorize a later read. Only a genuinely absent optional pointer is
absence. Dangling links, linked ancestors and same-byte symlink swaps are
failures, not empty/default evidence.

Staged raw evidence and evidence-document output use directory-descriptor
traversal with no-follow opens. Raw leaves and temporary document leaves are
created exclusively; document and pointer publication uses atomic rename and
directory sync. An existing run ID or staging directory is not overwritten.
Linked output parents and duplicate raw leaves or stages are typed refusals;
an existing advisory pointer may be atomically replaced but is never followed
for writing. This local path-custody contract does not claim hostile
concurrent directory-rename or remote filesystem attestation; those require
separate proof.

Conditional same-model and incremental claims are `NOT_APPLICABLE` while disabled. When enabled, they require their
raw vector or row-set inputs, typed operations, independent replay, source/model/dependency identity and execution
terminal. They cannot be opened by a summary JSON.

### Experiment and holdout custody

Before tuning, the external experiment manifest freezes task families, split keys, corpus and query digests, candidate
matrix, metrics, guards, seed, repetition structure, failure treatment and decision rule. Development selects one final
combination. The independent holdout is opened once for that combination. Timeouts, partials and failures remain in the
verdict and cannot be removed from the latency sample.

Qualified quality keeps graded density-aware NDCG@10 as the primary metric when independent adjudicated gold exists.
Exact-span recall, MRR, Hit@1, context cost and abstention are secondary metrics. Quiet-host query and ingest latency
are distinct from functional correctness.

## Consequences

- The historical RBR packet is recoverable from Git history, not a live status authority.
- Current decisions live only in accepted ADRs; unfinished execution work lives only in the active gap register.
- A documentation change under the remediation packet requires new source-bound proof before current qualification.
- Current verification and qualification status is maintained in the
  [active gap register](../plans/sep-26-retrieval-remediation/tickets/GAP-REGISTER.md),
  not in this decision record.
