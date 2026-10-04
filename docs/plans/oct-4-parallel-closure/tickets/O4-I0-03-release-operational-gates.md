# O4-I0-03 — SEP-21 release·paired producer·배포 게이트

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P1 / `RELEASE_GATE` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E1-06](O4-E1-06-final-pool-and-scoreboards.md), [O4-E3-06](O4-E3-06-operator-event-proof.md), [O4-E4-06](O4-E4-06-qualified-performance.md), [O4-E4-07](O4-E4-07-policy-and-semantic-residuals.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

벤치 판단과 별개로 현재 release counterexample·source pair·운영 actions의 custody를 확인해 code/deploy/activate/rollback 상태를 각각 발행한다.

## 배경과 현재 상태

SEP21 R0–R6는 proof-result authority, P03–P08, P09, semantic omission source oracle, P10 restore, P11 source pair/LINUX/operations와 P12 aggregate를 다룬다. 이미 있는 owners/targets는 재구현하지 않고 current-source gaps만 닫는다. lexical-only benchmark 수리에 새 Semantica API/E2E를 선행 조건으로 끼워 넣지 않는다.

## 착수 입력

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

## 실행 단계

1. R0 authoritative raw test/action result·trusted host/CI run binding을 현재 구현/negative tests로 audit한다.
2. P03 activation/ACK replay, P04 lifetime, P05 ordering/cursor, P06 SDK identity, P07 real-provider policy, P08 release signal/child-loss proof를 existing owners에서 발행한다.
3. P09는 E3 actual process proof를 소비하고 semantic omission은 producer source-plan에서 독립 required-scope oracle가 있는지 확인한다. 새 completeness flag를 자기 검증 값으로 만들지 않는다.
4. P10 state roots와 retention obligations를 inventory해 current-format backup/verify/restore-forward를 disposable와 authorized target에 분리 실행한다.
5. P11 exact paired source·actual dependency roots·QBC build/results와 Linux release를 bind한다. repo 밖 Semantica 파일은 canonical graph에서 실제 경로를 resolve한 뒤만 scope에 넣는다.
6. authorized host/config/root가 확보되면 deployment→activation→rollback의 distinct pre/post observation을 각각 발행하고 P12 aggregate를 exact source/raw에 bind한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- 기존 tools/ci/tests/test_proof_execution_result.py, test_write_proof_manifest.py, test_check_proof_authority.py owner tests를 canonical environment에서 실행한다.
- current registry의 selected release/paired targets를 actual commands로 실행하고 raw authority를 independent verifier로 확인한다.
- Negative: wrong binary/source pair/package root/host class/CI run, forged counts, missing operation pre/post, ACK divergence, wrong format/rollback boundary 거절.

## 완료 조건

- CODE_QUALIFIED·DEPLOYED·ACTIVATED·ROLLBACK_PROVEN 각 범위에 실제로 관측된 별도 proof 또는 명시적 BLOCKED/NOT_RUN이 있다.
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
