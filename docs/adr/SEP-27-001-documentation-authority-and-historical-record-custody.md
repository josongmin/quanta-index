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
   checked inventories, and the current ticket or residual ledger named by the
   owning packet.
3. Verification claims require a fresh exact-source receipt under the repository
   verification contract. Historical test output, status labels, and counts do
   not qualify a later source revision.
4. Completed or superseded records are removed from the live documentation
   tree once their decisions are represented by an accepted ADR and their
   unfinished work has a current owner. Git history retains the exact bodies;
   `docs/ARCHIVE-INDEX.md` and `docs/plans/ARCHIVE-INDEX.md` identify the
   pre-deletion revision and recovery command. A deleted record is not a live
   link or a current authority.
5. A parent packet remains active when any implementation, measurement,
   qualification, deployment, or activation gate remains open. Historical child
   records may be archived individually without closing the parent.
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
records are listed in [the documentation history index](../ARCHIVE-INDEX.md).
Completed implementation plans are grouped in
[the plan history index](../plans/ARCHIVE-INDEX.md). The later SEP-26 retrieval
packet follows the same rule: accepted decisions in SEP-26-001/002/003,
unfinished execution and inline contracts in the
[SEP-27 execution SSOT](../plans/sep-27-misc/tickets/INDEX.md), historical detail
in Git or the verified external content backup for dirty/untracked preimages.

## Consequences

- Live links target accepted ADRs or active ledgers, not deleted records.
- Git history, including the pre-deletion revision named by the indexes,
  preserves the old bodies without presenting them as current documentation.
- Removing a completed packet does not assert that every finding in it was
  implemented or qualified; open work stays in the active owner ledger.
