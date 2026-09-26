# SEP-26 Retrieval Remediation

Status: `PARTIAL`

Consolidated: 2026-09-27 from clean pre-documentation snapshot
`5e6addd5814ce8b71af808ff201ddd0b18fbe6c4`.

## Canonical owners

| Concern | Canonical document |
|---|---|
| Query policy, publication, result proof and span accounting | [SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md) |
| Observation, comparator profiles, semantic/ANN proof, defaults and resource accounting | [SEP-26-002](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md) |
| Evidence custody, experiment admission and qualification | [SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md) |
| Compact decision lookup | [SEP-26 decision registry](../../../adr/SEP-26-DECISION-REGISTRY.md) |
| Work still required | [Active gap register](GAP-REGISTER.md) |
| Qualification commands and oracles | [Test plan](TEST-PLAN.md) |
| Historical ticket and audit map | [Archive manifest](ARCHIVE-MANIFEST.md) |

Accepted ADRs own current architecture. The RBR ticket files and long audit ledgers are retained as historical
implementation and evidence records. They do not override an ADR or the active gap register.

## Current boundary

- The architecture and implemented contract decisions are `Accepted`.
- This documentation change is inside the retrieval source closure. Earlier source-bound receipts are stale for the
  resulting revision.
- Current-source integration proof after consolidation is `NOT_RUN`.
- Final external `PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` are `NOT_RUN`.
- Conditional same-model or incremental claims remain `NOT_APPLICABLE` unless a run explicitly enables them and
  supplies the raw proof required by SEP-26-003.

## Ticket map

| Ticket | Decision owner | Remaining work |
|---|---|---|
| RBR-00 | SEP-26-003 | G-01 |
| RBR-01 | SEP-26-002 | G-02 |
| RBR-02 | SEP-26-001 | G-01 |
| RBR-03 | SEP-26-002 | G-03 |
| RBR-04 | SEP-26-001 | G-03 |
| RBR-05 | SEP-26-001 | G-01 |
| RBR-06 | SEP-26-001 | G-03 |
| RBR-07 | SEP-26-002 | G-03 |
| RBR-08 | SEP-26-001/002 | G-04 |
| RBR-09 | SEP-26-002 | G-02/G-03 |
| RBR-10 | SEP-26-002 | G-05 |
| RBR-11 | SEP-26-002 | G-06 |
| RBR-12 | SEP-26-003 | G-01/G-03 |
