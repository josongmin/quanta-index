# O4-I0-03 — SEP-21 release·paired producer·배포 게이트

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P1 / `RELEASE_GATE` |
| 기준 웨이브 | [W6 — release·운영·전체 잔여 판정](../waves/W6-release-and-final-closure.md) |
| 실행 상태 | local P12A infrastructure259 passed `VERIFIED`; release/paired/Linux/action qualification은 `NOT_RUN`/입력 `BLOCKED` |
| 선행 결과 | [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

벤치 판단과 별개로 현재 release counterexample·source pair·운영 actions의 custody를 확인해 code/deploy/activate/rollback 상태를 각각 발행한다.

## 배경과 현재 상태

SEP21 R0–R6는 proof-result authority, P03–P08, P09, semantic omission source oracle, P10 restore, P11 source pair/LINUX/operations와 P12 aggregate를 다룬다. 이미 있는 owners/targets는 재구현하지 않고 current-source gaps만 닫는다. lexical-only benchmark 수리에 새 Semantica API/E2E를 선행 조건으로 끼워 넣지 않는다.

## 2026-10-04 current local/release 구분

- current registry에는 executable local P00/P01/P02A/P02B, P03–P10 owner 및 P12A가 있다. P03–P10 release nodes와 P11 cross-repo/actions는 `staged`이며 `linux-production-like` host를 요구한다. workspace/daemon PASS나 registry lint만으로 `CODE_QUALIFIED`를 발행하지 않는다.
- `VERIFIED`: `QUANTA_PROOF_RAW_DIR=/Users/songmin/Documents/code-new/qi-oct4-p12a-proof-20261004-v1 just proof-p12a-proof-infrastructure` — exit0, 259 passed/127.88s, collection259 및 raw JUnit/inventory는 외부 root에 있다. registry-only lint는25 proofs/0 validated manifests였다. 이 owner 실행은 final exact-source manifest 또는 `CODE_QUALIFIED` 발행이 아니다.
- root가 조회한 Semantica live HEAD는 `811a7a49b582cb10e76b5d03baf67e24f77b5a05`이고 수정 파일22개가 있다. 다른 작업 소유이며 exact clean pair가 아니다. `rust-verify-hellgate-cross-repo`를 이 상태로 실행해 즉시 거절되는 것을 qualification으로 사용하지 않는다.
- P11 deployment/activation/rollback의 registry command strings에 대응하는 recipes는 현재 없다. actual authorized Linux host/path/config/state/retention/rollback window 및 typed pre/post action authority는 입력/설계 `BLOCKED`다. 기존 pending 사용자 입력을 임의로 채우거나 실행 target을 만들지 않는다.

## 착수 입력

### 코드 우선 재감사

- P11의 세 operational recipe/typed result producer는 실제 미구현으로 남는다. existing source의 결과 parser/schema는 nextest/pytest test authority이며, registry는 이 staged action을 발행 가능한 proof로 승인하지 않는다.
- 단순 host 미준비와 별개로 deploy/activate/restore-forward의 명령·독립 pre/post 관측·정확한 성공 판정 계약이 아직 없다. generic shell exit0이나 caller-written success JSON을 operational authority로 채택하지 않는다. 현재 issuer의 staged refusal은 유지한다.
- 코드-only 감사에서 나머지 owner/registry control 경로의 확정 결함은 발견하지 못했다. 이 판단은 P11 구현 완료나 운영 qualification을 뜻하지 않는다.

- current SEP21 residual ledger/proof authority와 실제 P03–P12 target inventory
- exact Semantica/Quanta source pair·resolved dependency graph·real provider policy
- authorized Linux host/path/config/state/retention/rollback window; 입력이 없으면 해당 operational stage BLOCKED

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/ci/proof-authority.toml](../../../../tools/ci/proof-authority.toml) | registered execution-result authorities | 실제 target/result parser/host/source state와 대조해 증거 없는 executable promotion을 거절한다. | SHARED |
| [tools/ci/proof_execution_result.py](../../../../tools/ci/proof_execution_result.py) | raw runner result authority | current machine result의 forged/partial/timeout/skipped/wrong source/host/binary counterexamples를 검증한다. | SHARED |
| [tools/ci/write-proof-manifest.py](../../../../tools/ci/write-proof-manifest.py) | execution result custody | hand-written terminal+arbitrary log를 passed authority로 인정하지 않도록 현재 parser/result binding을 확인한다. | SHARED |
| [tools/ci/lint/check-proof-authority.py](../../../../tools/ci/lint/check-proof-authority.py) | paired_source_snapshot / aggregate checks | trusted producing CI run, resolved package roots/source pair/lock/binary와 bundle binding을 검사한다. | SHARED |
| [scripts/verify-repomap-cross-repo.sh](../../../../scripts/verify-repomap-cross-repo.sh) | current canonical paired verification | 실제 Semantica dependency graph가 frozen Quanta roots로 연결됐는지 검증한 뒤 QBC paired rail을 실행한다. | READ |
| [crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs) | existing backup/restore refusal proof | stopped daemon·exclusive lease/current format backup/verify/restore-forward·wrong format/target를 실제 release root에서 검증한다. | READ |
| [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) | R0–R6/P03–P12 current gate | 현재 code와 evidence를 재대조한 disposition·exact target·blocked operational input만 갱신한다. | SHARED |
| [crates/quanta-index-contract/src/ipc/ingest.rs](../../../../crates/quanta-index-contract/src/ipc/ingest.rs) | source batch / terminal receipt | semantic required-scope oracle가 실제 producer source-plan에서 발행된 경우만 existing batch/receipt에 결속한다. self-authored completeness flag를 독립 oracle로 만들지 않는다. | SHARED |
| [crates/quanta-index-search-plane/src/semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs) | derive_semantic_stream_from_semantic_sources_v1 | legitimate unchanged delta와 omitted/duplicate required scope를 구별하는 current derive/ingest boundary를 확인한다. 원인별 oracle 부재는 BLOCKED로 남긴다. | SHARED |
| [tools/ci/paired_cargo_resolution.py](../../../../tools/ci/paired_cargo_resolution.py) | validate_resolution | 현재 typed Cargo metadata의 실제 package roots·consumer feature profile·lock mapping을 exact pair receipt에 결속한다. | SHARED |
| [tools/ci/binary_custody.py](../../../../tools/ci/binary_custody.py) | pin / verify | built/provided/custody binary identity와 later source/binary drift를 existing custody helper로 검사한다. | READ |
| [tools/ci/proof-manifest.schema.json](../../../../tools/ci/proof-manifest.schema.json) | ProofManifestV1 source/action/result fields | operational-action mode 도입이 필요한 경우 실제 typed pre/post producer·authority parser/checker와 같은 current schema로 진화시킨다. | SHARED |
| [tools/ci/write-proof-aggregate.py](../../../../tools/ci/write-proof-aggregate.py) | aggregate verdict / source pair | P00–P12 prerequisites와 개별 stage results에서 final verdict를 계산한다. missing/blocked/staged를 passed로 바꾸지 않는다. | SHARED |
| [tools/ci/tests/test_paired_cargo_resolution.py](../../../../tools/ci/tests/test_paired_cargo_resolution.py) | resolved dependency mutants | second Quanta root/wrong package/feature/lock mapping을 refused로 검증한다. | SHARED |
| [tools/ci/tests/test_write_proof_aggregate.py](../../../../tools/ci/tests/test_write_proof_aggregate.py) | aggregate stage separation | deploy-only→activated/rollback 승격, missing prereqs/wrong exact pair refusal를 검증한다. | SHARED |
| [crates/quanta-index-searchd/src/app/state_format.rs](../../../../crates/quanta-index-searchd/src/app/state_format.rs) | current format / authority | current-format root identity·legacy refusal를 소비한다. 새 legacy importer를 만들지 않는다. | READ |
| [crates/quanta-index-searchd/src/app/state_migration.rs](../../../../crates/quanta-index-searchd/src/app/state_migration.rs) | backup / verify / restore-forward composition | stopped daemon/exclusive lease·retention·activation incarnation rotation을 actual release process에서 검증한다. | READ |
| [crates/quanta-index-searchd-runtime/src/state_migration.rs](../../../../crates/quanta-index-searchd-runtime/src/state_migration.rs) | state custody runtime owner | original manifest·bounded sidecars·exact paths과 wrong target/format refusal를 현재 owner에서 확인한다. | READ |
| [crates/quanta-index-searchd/src/cli/command.rs](../../../../crates/quanta-index-searchd/src/cli/command.rs) | state migration CLI | 실제 disposable/authorized target에 current backup/verify/restore-forward를 실행하고 rollback boundary를 분리한다. | READ |
| [docs/operator/state-cutover-runbook.md](../../../../docs/operator/state-cutover-runbook.md) | operator cutover/rollback steps | 현 state roots·retention/lease/restore-forward 조건과 actual operational receipts에서 관측한 단계만 갱신한다. | SHARED |
| [Justfile](../../../../Justfile) | proof-p11-deployment / proof-p11-activation / proof-p11-rollback / final qualification | 현재 없는 P11 action recipes를 implemented라고 표시하지 않는다. 실제 authorized target 입력과 typed action/result authority가 확보된 경우만 현재 front door에 추가한다. | SHARED |

## 실행 단계

1. R0 authoritative raw test/action result·trusted host/CI run binding을 현재 구현/negative tests로 audit한다.
2. P03 activation/ACK replay, P04 lifetime, P05 ordering/cursor, P06 SDK identity, P07 real-provider policy, P08 release signal/child-loss proof를 existing owners에서 발행한다.
3. P09는 E3 actual process proof를 소비하고 semantic omission은 producer source-plan에서 독립 required-scope oracle가 있는지 확인한다. 새 completeness flag를 자기 검증 값으로 만들지 않는다.
4. P10 state roots와 retention obligations를 inventory해 current-format backup/verify/restore-forward를 disposable와 authorized target에 분리 실행한다.
5. P11 exact paired source·actual dependency roots·QBC build/results와 Linux release를 bind한다. repo 밖 Semantica 파일은 canonical graph에서 실제 경로를 resolve한 뒤만 scope에 넣는다.
6. authorized host/config/root가 확보되면 deployment→activation→rollback의 distinct pre/post observation을 각각 발행하고 P12 aggregate를 exact source/raw에 bind한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_proof_execution_result.py tools/ci/tests/test_write_proof_manifest.py tools/ci/tests/test_check_proof_authority.py tools/ci/tests/test_paired_cargo_resolution.py tools/ci/tests/test_write_proof_aggregate.py -q`
- `just proof-p12a-proof-infrastructure` — registry가 요구하는 raw authority/aggregate/paired/local-scope/handoff owners 전체. formal claim에는 이 registered recipe의 actual result가 필요하다.
- current registry의 selected release/paired targets를 actual commands로 실행하고 raw authority를 independent verifier로 확인한다.
- Negative: wrong binary/source pair/package root/host class/CI run, forged counts, missing operation pre/post, ACK divergence, wrong format/rollback boundary 거절.

## Registry의 실제 target·release 잔여

| 범위 | 현 canonical 진입점 / 별도 필요한 결과 |
| --- | --- |
| P00–P02 | proof-p00-authority-freeze, proof-p01-canonical-identity, proof-p02a-repomap-compiler, proof-p02b-operation-journal의 **현재 source 결과**도 aggregate CODE_QUALIFIED prerequisites다. 과거 완료만으로 생략하지 않는다. |
| P03–P06 owners | just proof-p03-candidate-activation-owner / proof-p04-read-view-lifetime-owner / proof-p05-query-truth-owner / proof-p06-sdk-binding-owner. Linux fresh release targets는 별도 source/binary/raw를 요구한다. |
| P07 | just proof-p07-provider-boundary-owner는 local provider boundary다. p07-provider-boundary release node는 approved real-provider inputs·concrete target가 없는 staged 상태이므로 owner pass로 닫지 않는다. |
| P08–P10 | just proof-p08-runtime-supervisor-owner / proof-p09-control-readiness-owner / proof-p10-state-migration-owner. actual Linux release binary의 signals/child-loss/readiness/bounded events/current-format custody와 typed targets가 별도 필요하다. |
| P11 pair | just rust-verify-hellgate-cross-repo <actual-Semantica-checkout>. 현재 script는 clean exact pair·provided binary custody·resolved dependency graph/QBC tests를 요구한다. input env와 package roots는 actual source에서 resolve한다. |
| P11 actions | registry의 proof-p11-deployment/activation/rollback은 선언된 future command며 현재 Justfile recipe가 없다. typed operational result producer·manifest/checker mode·distinct pre/post targets를 구현/검증한 뒤에만 executable로 전환한다. |
| P12 | just proof-authority-code-gate / proof-authority-release-gate / proof-authority-final-qualification은 SEMANTICA_CHECKOUT·actual manifests를 요구한다. aggregate는 p12a-proof-infrastructure current-source 결과도 prerequisite로 소비한다. --require-all --bind-source의 final exact pair acceptance가 있어야 aggregate가 적격이다. |

- 위 owner command는 NOT_RUN이다. release registry의 staged node를 넘기기 위해 빈 test-authority list·hand-written terminal/log·owner result를 reuse하지 않는다.
- proof-manifest.schema.json·result producer/checker·aggregate·registered commands를 하나의 current authority로 수정한다. 새 동등한 evidence 체계를 만들지 않는다.
- Semantica의 search_plane_handoff_dispatch lexical_batch/semantic_state와 source-plan/manifest, consumer Cargo roots는 실제 canonical graph에서 경로를 resolve한다. oracle가 없으면 producer-side work를 명시적으로 scope에 잡으며 lexical benchmark와 독립 진행한다.
- authorized host/root가 없는 경우 Linux/operation scope는 BLOCKED다. ledger를 발행한 것과 실제 CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN 달성을 구분한다.

## Qualification별 선행 범위

- CODE_QUALIFIED는 현 registry의 P00–P11 source-bound prerequisites로 판정한다. E1 전체 라벨/미사용 holdout·E4 최적화/performance를 무조건 선행 조건으로 추가하지 않는다.
- 제품 품질·unseen 정책·성능을 함께 주장하면 E1-06·E1-05·E4-06/07의 해당 scope 결과도 별도로 요구한다. source 변경이 있으면 I0-02 epoch를 다시 발행한다.
- P09 owner proof에는 E3-06의 실제 daemon diagnostics를 소비한다. Linux release·real-provider·paired producer·state restore·operational actions는 각각 실제 target/input/result를 요구한다.
- BLOCKED/NOT_RUN 상태 inventory 작성은 ledger 작업 완료다. 요청한 qualification 자체는 해당 필수 결과가 없으면 미완료이며 실행 티켓을 닫지 않는다.

## 완료 조건

- 요청된 CODE_QUALIFIED·DEPLOYED·ACTIVATED·ROLLBACK_PROVEN은 각각 실제 필수 proof가 있어야 달성된다. BLOCKED/NOT_RUN ledger만 발행했다면 상태 정리는 끝났어도 해당 qualification 작업은 남아 있다.
- bench completion·commit/push/local focused tests로 release state를 대체하지 않는다.

## 중단·거절·재개 조건

- 이 티켓은 release gate 지도이며 현재 user의 문서 작성 요청이 배포 action 실행을 뜻하지 않는다.
- 실제 authorized host/target 입력이 없으면 해당 stage는 재현 가능한 BLOCKED로 남긴다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
