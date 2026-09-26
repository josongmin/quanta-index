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
4. Completed or superseded records stay at their stable paths. Each receives a
   historical banner and an entry in `docs/ARCHIVE-INDEX.md`; the original body
   remains unchanged as provenance.
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

This decision classifies 21 non-plan Markdown records:

- three Sep 21 purpose-audit snapshots;
- eight Sep 16 bugbash records, including four local gate decisions;
- two pre-de-channelize SSOT documents;
- six Sep 22 test-optimization audit inputs;
- two Sep 23 TOPT integration and gate receipts.

The exact paths, successor authorities, and deliberately retained active records
are listed in [the documentation archive index](../ARCHIVE-INDEX.md). Completed
implementation plans remain indexed separately in
[the completed-plan archive](../plans/ARCHIVE-INDEX.md).

## Consequences

- Stable historical links continue to resolve.
- Search results expose the archive boundary before the historical claim.
- Current ledgers stay small enough to audit without deleting provenance.
- Archiving a record does not assert that every finding in it was implemented or
  qualified; it asserts only that the record is no longer current authority.
