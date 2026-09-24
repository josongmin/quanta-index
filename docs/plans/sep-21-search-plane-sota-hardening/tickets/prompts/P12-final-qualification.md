# P12Q Final Qualification

The recipe and aggregate producer already exist. This qualification starts
only after the registered dependencies and authentic handoffs are ready; the
presence of P12A code alone is insufficient.

당신은 S21-13 phase B qualification-only owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`, 같은 디렉터리의
`README.md`, `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/README.md`를 읽고 그대로 적용한다. immediate P12A clean checkpoint/handoff가 start
HEAD와 exact match하고 aggregate infrastructure owner checks가 green일 때만 시작한다. product code, proof registry,
schema/writer/validator/recipe를 고치지 않는다. 결함은 owning lane으로 되돌리고 `BLOCKED`다.

읽을 문서:

- repo instructions
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/{FINAL-AUDIT,INDEX,ACTION-LIST}.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md`
- P00~P11 handoff 전부와 immediate P12A handoff

## REQUIRED INPUTS

- `SEMANTICA_CHECKOUT` points to the frozen paired checkout; canonical identity
  `github:josongmin/semantica-codegraph-v2`와 exact match해야 한다.
- `P12_TERMINAL_INPUT`: final source pair, registry digest, attested daemon path/SHA, host, config/corpus/provider/fixture
  digests, handoff list와 artifact digests를 담은 schema-valid absolute JSON path.
- pinned Linux production-like host identity와 actual release daemon absolute path/SHA.
- real-provider, deploy, activation, rollback 실행 승인을 각각 독립적으로 확인한다. 승인 없는 verdict는 `NOT_RUN`이다.

두 입력이 없거나 stale하면 즉시 `BLOCKED`다. 로컬 default, 과거 SHA, 대화의 DONE, mock binary로 보완하지 않는다.

## 목표

한 final clean quanta/Semantica source pair와 동일 attested release daemon binary에서 mandatory proof graph를 실행·수집하고
`CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`을 서로 독립적으로 판정한다.

## preflight

- quanta-index/Semantica exact clean HEAD, branch/upstream/merge-base, dirty=clean
- frozen `tools/ci/proof-authority.toml` digest와 proof ID/family/dependency DAG
- daemon binary SHA/features/toolchain/state-root format
- pinned Linux production-like host identity
- corpus/config/model/provider/fixture digests
- 모든 profile/selector의 non-empty actual target selection
- aggregate `product_handoffs` exact list:
  `P00,P01,P02A,P02B,P02I,P03,P04,P05,P06,P07,P08,P09,P10,P11`
- 별도 `infrastructure_handoff=P12A`; P12A result SHA는 current start HEAD와 exact match하고
  `p12a-proof-infrastructure` manifest가 source-bound green이어야 한다.

P12 output 자체와 P12A를 product handoff ledger에 포함하지 않는다. final aggregate가 P00→P11 product serial/fork-join
chain을 검증하고 P12A를 별도 infrastructure prerequisite로 검증한 뒤 마지막으로 P12 handoff/manifest를 발급한다.

## mandatory coverage

final source에서 전체 dependency closure를 topological order로 재실행한다:
`P00/P01/P02 → P03~P10 owner chain → P03~P10 release chain → P11 exact-pair/deploy/activate/rollback chain →
aggregate/P12`. registry의 release proof뿐 아니라 모든 `*-owner` dependency manifest도 final
`source-binding-digest/manifest-digest` keyed immutable archive에 재발급하고 `dependency_receipts`는 exact transitive
archive path/digest를 가리킨다. historical handoff archives는 덮어쓰지 않으며 aggregate는 historical ledger와 final
rerun ledger를 각각 검증한다.
family code `S/U/A/D/P/F/Q/X`는 coverage dimension이며 임의로 새 family 의미를 만들지 않는다.

- S: static architecture/public API/wire/module/policy
- U: owner-local positive/negative/recovery
- A: real SQLite/Tantivy/Lance/RepoMap adapter
- D: real SDK/UDS daemon consumer
- P: child-process signal/crash/restart/lease/readiness
- F: corruption/failpoint/concurrency/cancellation
- Q: fixed-corpus relevance, ANN recall, latency/QPS/RSS/FD/thread/disk/WAL
- X: external Semantica/provider/migration/deploy evidence

## proof rules

- each node records full source/binary/host/config identity and selected/executed/passed/failed/ignored counts
- missing/stale/skipped/zero-selected/wrong-binary/dirty mismatch is failure
- independent quality oracle cannot derive expected output from SUT output
- same binary progresses through process/external proof; untracked rebuild is a new identity
- macOS proof does not replace Linux credential/performance proof
- fake provider does not replace required real-provider proof
- final validator uses `--require-all --bind-source` and exact-pair validation
- canonical command is `just proof-authority-final-qualification`; raw `verify-rust` success is not terminal authority
- aggregate artifact is included in `P12_TERMINAL_INPUT` terminal artifacts and its digest must match the issued manifest

aggregate closed state computes these separately:

- `CODE_QUALIFIED`
- `DEPLOYED`
- `ACTIVATED`
- `ROLLBACK_PROVEN`

`PRODUCTION_READY` requires M0~M4 complete, registry digest fixed, legacy live path 0, full manifest valid,
migration/rollback/cross-repo/Linux/approved real-provider proof pass, unresolved P0/P1/`NOT_RUN`/`BLOCKED` 0. 승인 없는
deploy/activate/provider/rollback은 해당 verdict `NOT_RUN`이고 `PRODUCTION_READY=false`다.

proof node expected tuple은 `id=p12-final-qualification`, `family=S`,
`dependencies=[p12a-proof-infrastructure]`, `source_binding=exact-pair`,
`binary_binding=release-daemon`, `required_host=linux-production-like`다. aggregate가 `NOT_READY`이면 diagnostic artifact는
남길 수 있지만 P12 manifest를 발급하지 않는다.

최종 보고에는 exact source pair, registry/plan/aggregate digest, daemon binary, 모든 proof ID/family result/counts/artifact,
failed/skipped/not-run ledger, 네 verdict와 근거, unresolved risks와 `artifacts/sep-21/handoffs/P12.json`을 포함한다.
handoff는 `lane=P12`, `ticket=S21-13`, `paired_repositories`를 기록한다. tracked source write는 0이다.
report/handoff/receipt는 source-digest 제외 artifact에만 발급한다. tracked 결함이 발견되면
P12A 또는 owning product lane으로 되돌리고 새 checkpoint 후 전체 proof를 재실행하며 empty commit은 만들지 않는다.
provider egress, deploy, activate, rollback은 각각 별도
명시 승인 없이는 수행하지 않는다.
