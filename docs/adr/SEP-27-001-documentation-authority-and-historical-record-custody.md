# Documentation Authority and Historical Record Custody

Status: `Accepted`

Decided: 2026-09-27

## Context

The repository retains dated static audits, bugbash findings, gate decisions,
handoffs, implementation ledgers, and verification receipts. Those records are
useful for provenance, but their status and source identity do not transfer to
the current checkout. Several records also predate accepted architecture or a
newer current-source ledger.

Without an explicit custody rule, a frozen `PASS`, `PURPOSE_RED`, open-finding
count, or implementation status can be mistaken for current authority.

## Decision

1. Accepted product decisions are owned by `docs/adr/README.md` and its linked
   decision registries.
2. Current capability and implementation state are established by live source,
   checked inventories, and the single current residual ledger.
3. Verification follows the repository contract: routine focused edits use
   relevant source inspection and terminal results; formal replay, release and
   qualified benchmarks bind the required exact inputs and evidence. A receipt
   is required only by the selected contract. Historical output, status labels
   and counts do not qualify a later source revision.
4. Completed or superseded records are removed from the live documentation
   tree once their decisions are represented by an accepted ADR and their
   unfinished work has a current owner. Git history retains the exact bodies;
   the single `docs/ARCHIVE-INDEX.md` identifies the
   pre-deletion revision and recovery command. A deleted record is not a live
   link or a current authority.
5. An unfinished implementation, measurement, qualification, deployment or
   activation gate retains its owner in the current residual ledger. A dated
   parent packet may be retired after this transfer; deleting it does not close
   its remaining gates.
6. A historical finding is neither a current defect nor proof of repair. Reopen
   it only after reproducing the reachable condition against current source.

## Authority order

For conflicts, use this order:

1. current source and executable contract;
2. accepted ADR and decision registry;
3. current machine-readable authority or active ticket ledger;
4. current-source receipt with matching inputs and environment;
5. historical audit, plan, handoff, or receipt.

Lower-ranked material cannot override a higher-ranked owner. A current-source
failure still blocks qualification even when an older accepted decision says
what the implementation should do.

## Initial custody set

The initial classification covered 21 non-plan Markdown records:

- three Sep 21 purpose-audit snapshots;
- eight Sep 16 bugbash records, including four local gate decisions;
- two pre-de-channelize SSOT documents;
- six Sep 22 test-optimization audit inputs;
- two Sep 23 TOPT integration and gate receipts.

The recovery boundary, successor authorities, and deliberately retained active
records and completed implementation plans are listed in
[the recovery index](../ARCHIVE-INDEX.md#historical-record-recovery). The later SEP-26 retrieval
packet follows the same rule: accepted decisions in SEP-26-001/002/003,
unfinished execution in the
[single residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md), normative
contracts in the bound Accepted ADRs, and historical detail
in Git or the verified external content backup for dirty/untracked preimages.

## Consequences

- Live links target accepted ADRs or active ledgers, not deleted records.
- Git history, including the pre-deletion revision named by the recovery index,
  preserves the old bodies without presenting them as current documentation.
- Removing a completed packet does not assert that every finding in it was
  implemented or qualified; open work stays in the active owner ledger.

## Operator documentation boundary

Benchmark and CLI directories retain command usage, required input fields,
output interpretation and failure recovery. Accepted architecture, scoring,
custody and admission rules live in ADRs; generated capability matrices live in
`docs/reference`. Completed progress reports, handoffs and frozen result counts
are removed rather than copied into operator READMEs. Unfinished acceptance stays
in its active owner ledger. Normative decisions moved into ADRs remain bound by
the affected source-closure profiles.
