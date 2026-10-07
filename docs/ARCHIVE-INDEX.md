# Documentation and Ticket Recovery Index

Status: `HISTORICAL RECOVERY INDEX`

Completed decisions live in [ADRs](adr/README.md); unfinished acceptance lives in
[the active ledger](plans/oct-4-parallel-closure/tickets/INDEX.md) and its scoped owners.
Historical commands, counts, receipts and failure sequences do not qualify newer source.

## Historical record recovery

Pre-consolidation Git revision: `6a3f6afc8c286176962e722ce75524aefcfa7607`.
Recover a tracked body with `git show 6a3f6afc:<repository-relative-path>`.
The earlier recovery indexes are themselves retained in Git:

```sh
git show 6a3f6afc:docs/ARCHIVE-INDEX.md
git show 6a3f6afc:docs/plans/ARCHIVE-INDEX.md
```

These contain the prior exact revisions, removed-path sets, successor ADRs and
external backup identities for May–Oct-05 records. In particular, dirty/untracked
SEP-27 preimages need their named external content backup; Git does not recover
bytes that were never committed. Restore into a separate directory.

The Oct-07 pre-edit content, including concurrent dirty benchmark/CI updates,
is saved outside the checkout at `/tmp/qi-oct7-doc-consolidation-xz1fej2f/before`;
`manifest.json` identifies each SHA-256. This temporary backup is not durable Git
history. The archived baseline below is the recovery source for committed bodies.

| Retired or compacted record | Current authority |
| --- | --- |
| `docs/plans/oct-4-parallel-closure/WAVES.md` | [One execution order](plans/oct-4-parallel-closure/tickets/INDEX.md#실행-순서); latest input/ownership updates retained there |
| OCT-04 `tickets/INDEX.md` completed checkpoints, sidebar history and duplicate status | [OCT-05 ADRs](adr/README.md#oct-05-implemented-contracts); residual21 IDs remain live |
| O4-E3-01–06 | [Runtime contract and completed scope disposition](adr/OCT-05-003-active-query-and-runtime-lifecycle.md#completed-execution-scopes); E3-02 was unadopted/NOT_APPLICABLE |
| O4-E2-01 | [Completed-clock contract](adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-clock-scope); repeated performance remains E4-06 |
| O4-I0-01 | [Shared-source validation](adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#shared-source-validation); source validation remains a standing rule |
| SEP-21 `CURRENT-RESIDUAL-2026-09-26.md` | [R0–R6 execution/acceptance](plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md); S21-11/12/13 retain detailed acceptance |
| `docs/plans/ARCHIVE-INDEX.md` and repetitive archive chronology | This single recovery index; prior exact mappings remain in the two Git bodies above |

## Completed evidence

The following are historical source-bound results recorded before this cleanup,
not executions performed by documentation consolidation:

| Source / scope | Recorded completion and retained evidence |
| --- | --- |
| `e37123eb` SDK/Contract | SDK27; Contract Python793/Rust191; separate portable replay. `/Users/songmin/.codex/task-evidence/quanta-final-main-e37123eb-20261006-sdk`, `quanta-final-main-e37123eb-20261006-contract`, `quanta-final-main-e37123eb-20261006-custody` |
| `e37123eb` selected runtime/open-loop | Runtime15 and open-loop20; complete source/commands/exclusions in the Git baseline OCT-04 index |
| `9221d771` hosted CI | [Required verify1268](https://circleci.com/gh/josongmin/quanta-index/1268): regular jobs completed; bench is compilation, not samples. Later6a3f6afc closure remains I0-02 |
| `09103820` CS/SG/OG ready9 | CS180 and SG/OG each180 requests, separate replay9/9. Durable native-v3 and SG/OG-v5 roots are named in the active ledger. Earlier Docker failure and missing OG historical commit remain separate failures/blocked history |

Exact raw/result contexts retain their original revisions and provenance. Missing
old temporary roots cannot be recreated from written hashes, counts or current
manifests. Current full5/holdout/scale/RSS/qualified speed/pair/operations still
need their active owner's inputs and proof.
