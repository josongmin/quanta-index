# Plan History Index

Status: `HISTORICAL RECOVERY INDEX`

## Oct-05 repository-wide history cleanup

Pre-edit/deletion revision: `158a08bc0d9b396089467b009117e23c168b77f7`; exact bodies were checked against Git.
Recover with `git show 158a08bc0d9b396089467b009117e23c168b77f7:<repository-relative-path>`. External preimages:
`/tmp/qi-oct5-whole-doc-cleanup-vdhexoag`, `manifest.json`.

- Retired `sep-27-code-search-remediation/references.md`: dated research survey,
  not an executable acceptance source. BENCH-01/03/04 retain corpus/gold/holdout,
  statistical and comparator requirements; current methodology is ADR-owned.
- Retired `rfcs/CS-ENG-04-match-anchored-snippets.md`: implemented guards and
  deferred exact-cap decision consolidated in [SEP-27-003](../adr/SEP-27-003-code-search-source-and-preview-contract.md#deferred-regex-allocation-cap-cs-eng-04).
  Conditional P3/reopen workload/threshold/dependency-slice requirements remain;
  no cap, dependency fork, worker or new release blocker is accepted.
- Completed handoff/custody prose moves to SEP-21-004; handoff README retains
  commands/scope, historical audit stays separate from current qualification.
- Current semantic prior-state acceptance has no dependency on an old root audit.
  MISC selects current on-disk format from source instead of a frozen format-9
  label. Complete C4/C5/hosted/producer acceptance remains open.

The [documentation history index](../ARCHIVE-INDEX.md#oct-05-repository-wide-history-cleanup)
records the root audit, duplicate OCT-04 summary, runbook/guide custody and risk
mapping. Active parents stay open; no owner, proof registry or test target changes.

## Oct-05 residual owner clarification

Pre-edit revision: `3f4877c95183769e82d870f71e296c832c9014ed`. All fifteen edited bodies matched this revision
byte-for-byte before editing. Recover any original with
`git show 3f4877c95183769e82d870f71e296c832c9014ed:<repository-relative-path>`.

ENG-02, BENCH-01–04, QIT-09 and the SEP-21 execution map retain their independent
acceptance, quantitative bounds and stable owner IDs. Completed coverage decoding/
cache/row-copy decisions are consolidated in OCT-05-004. Old cost/CI chronology,
unchecked implementation worklists and duplicate source/status claims are removed.
The QIT board retains every QIT-00–09 and all proposed thresholds; current proof
registry/staging and every R0–R6 release/producer/action oracle remain unchanged.

SEP-21 now points to implemented Active/maintenance/operator/proof-result owners
instead of requiring them to be recreated. SEP-27-005 points to the implemented
OCT-05-003 diagnostic/meter owner; request IDs remain outside metric labels and
ring events do not replace counter authority. Current final-source/installed/Linux/
hosted/provider/paired/state evidence and P11 missing typed operational producers,
independent observers and authorized target inputs remain open. BENCH acceptance
does not relabel exposed mechanical/AI/native diagnostics as fresh qualification.
No source implementation, registry or runtime/provider state is changed here.
Exact external preimages/digests: `/tmp/qi-oct5-residual-owner-cleanup-ftnk49sw`, `manifest.json`.

## Oct-05 benchmark and quality ledger compaction

Pre-edit revision: `121ad9309303d8de9b0e189a04a4b65bf6d80c40`. All existing edited bodies matched that revision
byte-for-byte. Recover any exact body with
`git show 121ad9309303d8de9b0e189a04a4b65bf6d80c40:<repository-relative-path>`.

The live B01–B09 tickets and their Sep-30 README/index, J7Q-01/03/04 and quality
README, and CS-INT-01 retain acceptance and stable owner IDs. Repeated historical
execution/RCA/research bodies, temporary paths and source-specific test totals
are removed from those live owners. Completed planner/parser/intake, batch/clock
and optional ranking/Explain decisions are consolidated in OCT-05-001/002/004.
The OCT-04 ledger owns current execution state; unmet review/holdout/native scope,
host/capacity/resource/platform/release and operational contracts remain open.

MISC retains all current acceptance and quantitative contracts while removing
duplicate Sep-28/29 receipts; BENCH-01–04 links target the current integration
controls. These five additional preimages also match the revision above.
The source-map/SSOT/root navigation and SEP-21 residual wording were corrected;
`docs/README.md` is the documentation entrypoint. P11 missing typed operational
producers/recipes remain code work, with required action/observer/target inputs.
No runtime, benchmark or release qualification follows from this compaction.
Exact external preimages and digests: `/tmp/qi-oct5-doc-sweep-_8g9_zdf`, `manifest.json`.

## Oct-05 handoff and ticket compaction

Pre-deletion revision: `52980f58f9c08b8560b6262499071cfb7ca610c7`.
All 41 removed plan bodies matched this revision before deletion:

| Removed set under `docs/plans/oct-4-parallel-closure/` | Files | Current owner |
| --- | ---: | --- |
| `tickets/O4-*.md` | 29 | [Single residual ledger](oct-4-parallel-closure/tickets/INDEX.md): all original IDs, unmet inputs/acceptance and execution entrypoints |
| `epics/*.md` | 5 | [Owner/transfer map](oct-4-parallel-closure/README.md) and the four [Accepted ADRs](../adr/README.md#oct-05-implemented-contracts) |
| `waves/W*.md` | 7 | [Remaining W0–W6 execution](oct-4-parallel-closure/WAVES.md) |

Recover an exact body with
`git show 52980f58:docs/plans/oct-4-parallel-closure/tickets/O4-E2-02-external-index-universe.md`.
Enumerate the old packet with
`git ls-tree -r --name-only 52980f58 -- docs/plans/oct-4-parallel-closure`.
The five original handoffs are recorded in the
[documentation archive](../ARCHIVE-INDEX.md#oct-05-handoff-compaction): 46 files removed in total.

Old execution commands, selectors, raw/output paths, SHA bindings, failed attempts
and owner test totals remain in these Git bodies; they were not synthesized into
current proof. Implemented contracts live in OCT-05-001/002/003/004. All incomplete
label/admission/name/holdout/index/performance/capacity/current-source/CI/provider/
paired/Linux/state/action gates remain active. P11 operational producer/recipes
remain unimplemented and four optimization/policy changes remain conditional.

The OCT-04 parent, README/WAVES and `tickets/INDEX.md` remain live. Their pre-edit
bodies are recoverable at the same revision; exact preimages were also retained
outside the checkout at `/tmp/qi-oct5-adr-compaction-y_jhbe9z`.
No redirect stubs or duplicate historical execution packet remain in the live tree.

## Oct-04 stale draft consolidation

The six Sep-23/24 draft bodies below are recoverable from clean
`e43cda8c87b4a06fecac82a266011e30f84a2986` with
`git show e43cda8c:<path>`. They were proposals, not implemented or accepted
decisions. Their old source/format numbers and file-level execution orders do
not apply to the current checkout. Re-audited open questions are compressed
into the non-authoritative [configuration proposal](../adr/OCT-04-002-configuration-and-generation-policy.md)
and [source SDK proposal](../adr/OCT-04-003-source-preparation-sdk.md).

| Removed draft | Current disposition |
| --- | --- |
| `docs/plans/sep-23-search-config-profiles/{rfc,implementation-plan}.md` | Configuration and generation admission remain proposed; semantic format is now 12. |
| `docs/plans/sep-24-{sdk-dsl,repository-format-sdk,source-preparation-sdk}-rfc.md` | Existing SDK/batch path is authoritative; optional adapter/format design remains proposed. |
| `docs/plans/sep-24-source-preparation-execution-plan.md` | Old gate/file ownership and Semantica assumptions require a new source-bound plan. |

Pre-deletion revision: `eff53181b2ab7a3d017a5c613574b12e4000b52e`.
The completed or superseded plan bodies are not live documentation. Recover an exact file with
`git show eff53181:<repository-relative-path>`; enumerate a packet with
`git ls-tree -r --name-only eff53181 -- docs/plans/<packet>`.
The old status and receipt in any recovered file apply only to its recorded
source. Current decisions live in [accepted ADRs](../adr/README.md); unfinished
work remains in active tickets and qualification ledgers.

| Removed packet or record set | Markdown files | Current authority |
|---|---:|---|
| `may-24-lexical-indexing-sourcegraph` | 31 | [DSL](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [Sourcegraph](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |
| `may-25-lexical-enhancement/tickets` | 20 | DSL and Sourcegraph ADRs; active capability matrix retained |
| `may-25-sdk-cutover-wave-plan.md` | 1 | [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |
| `may-26-indexing-residue-tasks` | 7 | DSL ADR |
| `may-27-dsl-master-closeout` and `may-27-structural-dsl` | 18 | DSL ADR |
| `may-28-lancedb-adoption` | 9 | [Semantic generation](../adr/MAY-31-001-lancedb-semantic-generation-authority.md) |
| `jun-2-dsl-final-cut`, `jun-2-dsl-hardening`, `jun-2-dsl-advanced` | 25 | DSL and [verification](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md) ADRs; current mechanics in [benchmark tooling](../../tools/benchmark/README.md) |
| `jun-4-dsl-extension`, `jun-4-sourcegraph-parity` | 21 | DSL and Sourcegraph ADRs |
| `jun-5-sourcegraph-tail-gaps`, `jun-6-sourcegraph-expansion` | 30 | Sourcegraph ADR |
| `jun-7-verification-hellgates` | 15 | Verification ADR |
| Superseded `jun-23-embedding-pipeline-sota/rfc.md` | 1 | [semantic ownership ADR](../adr/MAY-31-001-lancedb-semantic-generation-authority.md), [active semantic ledger](may-25-search-owned-semantic-derivation/README.md) |
| Superseded `search-plane-implementation-tickets.md` | 1 | [SEP-21 decisions](../adr/SEP-21-DECISION-REGISTRY.md), [residual plan](sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| Historical children of active `sep-21-search-plane-sota-hardening` | 11 | [SEP-21 registry](../adr/SEP-21-DECISION-REGISTRY.md) and active residual ledger |
| Superseded `sep-21-search-plane-sota-hardening/tickets/ACTION-LIST.md` | 1 | [current residual audit](sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md), [execution plan](sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| Historical child of active `may-25-search-owned-semantic-derivation` | 1 | Semantic generation ADR and active parent |
| Superseded `sep-23-search-config-profiles/sdk-interface.md` | 1 | [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md), active Sep-24 draft |
| Historical `sep-26-bench-migration` audit/closeout | 2 | [Verification ADR](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md), [execution SSOT](sep-27-misc/tickets/INDEX.md) |
| Superseded `sep-26-bench-migration` implementation inventory/matrix | 2 | [registry](../../tools/benchmark/registry.toml), [execution SSOT](sep-27-misc/tickets/INDEX.md) |
| Completed `sep-26-bench-migration/tickets/BM-03-DECISION.md` | 1 | [Benchmark orchestration ADR](../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md); execution contract consolidated |
| Historical `sep-26-retrieval-remediation/tickets` | 17 | [SEP-26 ADRs](../adr/SEP-26-DECISION-REGISTRY.md), [execution SSOT](sep-27-misc/tickets/INDEX.md) |

Total: 215 removed historical plan Markdown files. The SEP-26 historical
archive manifest and initial `audit-evidence.json` were also removed; both are
recoverable at the same Git revision.

Still live: the May-25 lexical capability/proof inventory, active semantic
packet, Sep-21 residual execution and Sep-23/24 drafts. RB/BM/RBR and TOPT
accepted contracts now live in the SEP-26/27 ADRs; open execution remains in the
[SEP-27 ledger](sep-27-misc/tickets/INDEX.md).
A packet is not complete merely because its superseded documents were removed.

## SEP-27 additional consolidation

Removed 25 plan Markdown files: eleven RB, eleven BM and three RBR files.
Together with four handoffs and twelve TOPT files, this is 41 documents; these
counts are additional to the historical 215 above. Contracts, acceptance
matrices and remaining work were inlined in the SSOT; no redirect stubs remain.
Exact dirty/untracked preimages are recoverable from the content backup recorded
in the [documentation recovery index](../ARCHIVE-INDEX.md#sep-27-execution-consolidation).


## SEP-27 completed code-search and implementation compaction

Pre-deletion revision: `1419f3087f4f09a6ecab4ef39c30a2bf32544d5d`. All 41 removed file bodies
matched that revision before deletion (40 Markdown and one preparation
JSON). Git retains their exact contents; no redirect stubs or live history
packet remains. Recovery is historical, not current qualification.

| Removed set | Current authority / remaining owner |
| --- | --- |
| `sep-27-code-search-remediation/handoffs/*.md` | [Code-search contracts](../adr/SEP-27-003-code-search-source-and-preview-contract.md); open allocation/cost/integration and benchmark work stays in the active RFCs |
| `evidence.md`, `engine-audit.md`, `l2-g0-request.md`, `l2-preparation-receipt.json` under that packet | Accepted code-search ADR; original diagnostics and G0 chronology in Git |
| `rfcs/CS-ENG-01-query-domain-and-result-contract.md`, `CS-ENG-03-definition-and-file-ranking.md`, `CS-PROD-01-parser-coverage-and-vite.md` | Code-search ADR; ranking/holdout and external/combined-source acceptance remain in BENCH-03 and CS-INT-01 |
| `sep-21-search-plane-sota-hardening/tickets/S21-00-authority-freeze-and-cutover-contract.md` | [SEP-21 accepted registry](../adr/SEP-21-DECISION-REGISTRY.md); current residual execution and proof authority retain S21-00 as an identity |
| Completed MISC implementation/RCA/terminal sections | [Capture/process/resource ADR](../adr/SEP-27-004-benchmark-capture-and-resource-custody.md); only open acceptance remains in MISC |
| May-25 lexical closeout chronology | Existing DSL/Sourcegraph ADRs; active capability matrix and machine-readable inventory retained |

Recover one body with `git show 1419f3087f4f09a6ecab4ef39c30a2bf32544d5d:<repository-relative-path>`.
Enumerate the code-search packet with
`git ls-tree -r --name-only 1419f3087f4f09a6ecab4ef39c30a2bf32544d5d -- docs/plans/sep-27-code-search-remediation`.
Recover the previous MISC ledger or lexical closeout with the same `git show`
command. Restore into a separate location, never over current active records.

Active May semantic follow-on, Jun/Jul quality/hardening, SEP-21 residual and
SEP-23/24 drafts remain open; age alone did not classify them as completed.

## SEP-27 second completion sweep

Pre-deletion revision: `0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`.
The three additional removed Markdown bodies matched that revision byte-for-byte
before deletion. This is additional to the 41-file compaction above.

| Removed record | Retained authority / remaining owner |
| --- | --- |
| `sep-21-search-plane-sota-hardening/tickets/EXECUTION-PROGRESS.md` | [SEP-27-005](../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md) carries completed catalog/recovery/supervision/proof decisions; active R0–R6 ledgers retain remaining acceptance |
| `sep-21-search-plane-sota-hardening/tickets/S21-05-read-view-v2-and-snapshot-lifetime.md` | SEP-21-003 and SEP-27-005; remaining P04 process/pin/retirement acceptance is retained in the residual execution plan |
| `may-25-search-owned-semantic-derivation/tickets/SEM-OWN-FOLLOWUP-jun23-seam-hardening.md` | Semantic/retrieval ADRs carry completed model/score/truth contracts; shared-provider, labeled ranking and model/dimension migration leads remain deferred in the parent design |

The SEP-21 index, residual status and S21-11/12/13 now retain open acceptance
without terminal counts, obsolete API descriptions or repeated repair history.
Recover their previous bodies and the removed files with
`git show 0b4839a4a8b4cf99e870b4251395b6e3df8f4a21:<repository-relative-path>`.
No historical execution is promoted to current-source or release qualification.

## SEP-27 operator and semantic compaction

Pre-deletion revision: `0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`.
Six `may-25-search-owned-semantic-derivation/tickets/SEM-OWN-00` through
`SEM-OWN-05` Markdown bodies matched that revision byte-for-byte before removal.
They combined completed public-surface work with a superseded chunk-text and
subscriber-worker design. Implemented boundaries live in
[MAY-31-001](../adr/MAY-31-001-lancedb-semantic-generation-authority.md);
unfinished proof/provider/observability leads remain in
[the semantic residual ledger](may-25-search-owned-semantic-derivation/tickets/INDEX.md).
Proposed worker/job/seal types are not promoted to implemented contracts.

Recover with `git show 0b4839a4a8b4cf99e870b4251395b6e3df8f4a21:<repository-relative-path>`.
Benchmark README design/scoring/ratchet content is consolidated into SEP-26-003,
SEP-27-004 and JUN-08-001. Benchmark and searchctl READMEs retain usage only;
Sourcegraph coverage generation now writes `docs/reference/sourcegraph-filter-parity.md`.
The old benchmark/reference bodies are recoverable at the same revision; the
active runbook's dirty preimage is retained outside the repository under
`/tmp/qi-doc-cleanup-preimages-rbxlmdok`. No historical result is new qualification.

## SEP-27 quality and test-authority completion sweep

Pre-deletion revision: `0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`.
All 18 removed plan Markdown bodies matched that revision before deletion.
The additional `docs/ranked-key-tables.md` body also matched that revision;
its implemented storage contract is consolidated into
[SEP-27-003](../adr/SEP-27-003-code-search-source-and-preview-contract.md),
including the 64 MiB resident limit, integrity window and producer rebuild.
This sweep removes 19 files in total; current-source integration proof remains open.

| Removed historical record | Implemented decision / remaining acceptance |
| --- | --- |
| Jun-7 `rfc.md`, command/artifact and measurement matrices, source map and three worker/checklist/rule documents (7 files) | [JUN-08-001](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md); current registration is code-owned, open J7Q-01–04 remain in the quality residual index |
| J7Q-00/05/06/07/08 implementation tickets (5 files) | Existing scope/policy, diagnosis, repair, typed preview and producer registration; consumer/operator/aggregate execution obligations retained in [the residual index](jun-7-search-product-quality/tickets-wave2/INDEX.md) |
| Jul-15 CI/invariant/source matrices, worker/rule scaffolding and dependency DAG (6 files) | [SEP-27-005](../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md); all unmet QIT owner/quantitative/hosted/release acceptance retained in [the residual board](jul-15-sota-test-hardening/tickets/00-ticket-status-board.md) |

Recover an exact body with `git show 0b4839a4a8b4cf99e870b4251395b6e3df8f4a21:<repository-relative-path>`.
The mixed packets were compacted by implementation/acceptance scope; neither
whole quality nor whole QIT qualification is declared complete. Dirty preimages
of retained edited files were copied outside the repository before this sweep.

The lexical capability prose and MISC contract/command copies were subsequently
compacted without deleting pending acceptance. See the
[document compaction record](../ARCHIVE-INDEX.md#sep-27-remaining-document-compaction)
for exact dirty preimages. The lexical TOML ledger and all 143 purpose checks
remain; canonical comparison/admission now lives in SEP-26-003.

## SEP-27 code-search live-ledger evidence pruning

Pre-edit revision: `e56ce86d62bc71225d6994c79d1bd5fefe42bc1f`.
The active CS-INT-01, ENG-02/04 and BENCH-01/02/03/04 files remain live for open
acceptance. Their pre-edit bodies at this revision retain the older HEAD-bound
test totals, `/tmp` scratch receipts, one-off task count/cost table, local backend probe
outcomes and completed implementation chronology removed from the live ledgers.
Recover one with `git show e56ce86d62bc71225d6994c79d1bd5fefe42bc1f:<repository-relative-path>`.
Accepted source/preview, native capture and statistical decision boundaries
are in [SEP-27-003](../adr/SEP-27-003-code-search-source-and-preview-contract.md),
[SEP-27-004](../adr/SEP-27-004-benchmark-capture-and-resource-custody.md) and
[SEP-26-003](../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md).
Historical local outcomes do not qualify the current checkout.

## SEP-21 proof-prompt compaction

Pre-deletion HEAD: `e56ce86d62bc71225d6994c79d1bd5fefe42bc1f`.
The four unchanged `sep-21-search-plane-sota-hardening/tickets/prompts/P10*`,
`P11*`, `P12A*` and `P12-final-qualification.md` bodies matched that revision
byte-for-byte before removal. `git show <HEAD>:<path>` recovers each one.
Their current commands and staged boundaries remain in the
[execution entrypoint](sep-21-search-plane-sota-hardening/tickets/prompts/README.md);
P10/P11 acceptance remains in S21-11/12, and complete final graph/threshold
acceptance in [S21-13](sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md).
S21-12 was also reduced from 135 to 75 lines: its exact-pair chain, negative
matrix, four proof nodes and stop conditions remain active. Its unchanged
preimage matched the same HEAD and is in the external backup below.
The dirty preimage of this archive index and exact preimages of all affected
files are retained outside the checkout at `/tmp/qi-sep21-prompt-compaction-8mq_zouv`.
Deleting duplicate prompts neither issues P10/P11/P12A receipts nor qualifies
release/deployment/activation/rollback.

## OCT-05 reference inventory relocation and MISC contract consolidation

Pre-edit revision: `c7b0ce88617d8ae0f582ab41cdf223e5962bd8ab`.
15 edited/retired source bodies matched that revision before mutation.
The source-map dirty preimage, including concurrent paired-runner navigation,
is preserved externally; only its embedder boundary rows change in this pass.
Recover a body with `git show c7b0ce88617d8ae0f582ab41cdf223e5962bd8ab:<repository-relative-path>`.

| Historical path / body | Current authority |
| --- | --- |
| `may-25-lexical-enhancement/README.md` | Redundant navigation retired; [DSL entrypoint](../reference/dsl-capabilities.md) owns navigation |
| `may-25-lexical-enhancement/lexical-capability-matrix.md` and `dsl-proof-ledger.toml` | Moved to [DSL proof inventory](../reference/dsl-proof-inventory.md) and [machine ledger](../reference/dsl-proof-ledger.toml); all canonical predicates and per-surface entries preserved; checker/hook consume the new path |
| `docs/analysis/quanta-index-purpose-validation-checklist.md` | Moved to [purpose audit inventory](../reference/purpose-audit-inventory.md); all 143 G0–G13 IDs/conditions retained as reference rows, without unchecked task status |
| MISC permanent fixture/wait and retrieval contract copies | [SEP-27-005](../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#test-fixture-and-wait-invariants), [SEP-26-001](../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md) and [SEP-26-002](../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md); pending actual execution, inputs and numeric measurement acceptance stay in [MISC](sep-27-misc/tickets/INDEX.md) |

The source map now follows the daemon's PotionCode default and explicit development
hash selector. This pass issues no model, runtime, hosted, performance or release
qualification. The external preimage directory is `/tmp/qi-oct5-reference-consolidation-wq7ztn2_`.
