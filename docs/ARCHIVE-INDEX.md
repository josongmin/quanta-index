# Documentation and Ticket Recovery Index

Status: `HISTORICAL RECOVERY INDEX`

Accepted contracts live in [ADRs](adr/README.md); unfinished work lives once in
[the residual ledger](plans/oct-4-parallel-closure/tickets/INDEX.md).
Historical commands, counts and receipts do not qualify newer source.

## Historical record recovery

The final dated RFC/plan/ticket retirement on 2026-10-07 uses pre-deletion revision
`4f07c3c5ac57f161aa6870a5cc661b0038932993`. It removes 39 Markdown files under
`docs/plans/` plus the redundant `tickets/README.md`; the current residual ledger
is retained. The executable handoff schema moves unchanged to
`tools/ci/lane-handoff.schema.json`.

Recover an original body into a separate directory:

```sh
git show 4f07c3c5:<repository-relative-path>
git ls-tree -r --name-only 4f07c3c5 docs/plans tickets
```

| Retired packet | Current contract / remaining owner |
| --- | --- |
| Sep27 code-search RFCs, Sep30 B01–B09 | [Review/corpus/statistics](adr/OCT-05-001-review-admission-and-result-identity.md), [native/update](adr/OCT-05-002-native-capture-clock-and-index-scope.md), [cost/host](adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md); E1/E2/E4/I0 |
| Sep21 residual plan, S21-11/12/13, prompts/handoff usage | [Proof/backup/custody](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md), [installed/pair/actions](adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance), operator runbooks and executable registry; I0 |
| Jul15 QIT, Sep27 MISC and root ticket index | [Selected regression/platform](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-regression-and-platform-acceptance), [CI provider](adr/SEP-28-001-circleci-provider-and-credit-boundary.md); test/platform/E1/E2/E4 |
| Jun7 J7Q quality and May25 semantic | [Consumer preview/operator](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#consumer-preview-and-operator-acceptance), [semantic generation](adr/MAY-31-001-lancedb-semantic-generation-authority.md), cost/review ADRs; semantic/E1/E4/I0 |
| Oct4 owner map / Semantica pointer | [Single owners](plans/oct-4-parallel-closure/tickets/INDEX.md#owners), [release/proof](plans/oct-4-parallel-closure/tickets/INDEX.md#release-and-proof); external producer fact resolution stays producer-owned |

Existing IDs route through [the scope table](plans/oct-4-parallel-closure/tickets/INDEX.md#legacy-scope-routes).
Completed source-bound cache/lifecycle/F15/CI/Medium/Ready9 checkpoints remain in
those ADRs; deleting a packet does not issue fresh execution or qualification.
Unadmitted numeric hardening/holdout targets remain historical proposals.

## Earlier recovery boundaries

- `6a3f6afc8c286176962e722ce75524aefcfa7607`: earlier May–Oct05 documentation
  and packet consolidation, including the original `docs/ARCHIVE-INDEX.md` and
  `docs/plans/ARCHIVE-INDEX.md` mappings.
- `a5e87614bee79bf9d67c8358df01313242548798`: earlier Oct07 completed OCT04
  status detail and B07 execution chronology.

Read those historical recovery indexes with `git show <revision>:<path>` for
exact older source/artifact identities and removed-path sets. Dirty/untracked
Sep27 preimages require the external content backup named there; Git cannot
recover never-committed bytes. Temporary external backups are not durable history.
Missing old native/raw/review roots cannot be recreated from hashes or counters.
