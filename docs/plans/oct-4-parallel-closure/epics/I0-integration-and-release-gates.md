# I0 — 단일 통합 담당·source 검증·release 게이트

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: 단일 통합 담당.
- 3개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

## 목적

공용 계약/파일과 final source epoch를 한 담당자가 통합하고, 정확히 영향을 받은 contract·SDK·CI·release/operations 범위의 proof를 발행한다.

## 배경과 현재 구현

- 현재 기준은 main@0df06e0c이며 직전44bd68a1 이후 차이는 기존 통합 문서 한 파일이다. 핸드오프의 staged81/vendor whitespace 문제는 현재 소스에서 해결됐지만 실제 제품/성능/release qualification은 별개다.
- run.py·schemas·Rust benchmark DTO/record·proof registry/CI/lockfile를 여러 에픽이 동시에 수정하면 producer/consumer binding과 evidence epoch가 갈라진다.
- 기존 Contract/SDK proof inventory와 portable verifier를 재사용한다. collection·focused pass·commit/push는 actual source/selected tests/real daemon/release gates의 대체가 아니다.
- SEP21 R0–R6와 P03–P12는 raw authority·P09process·P10restore·P11exact pair/Linux/운영 actions를 별도 다룬다.

## 목표 계약과 변경 원칙

- SHARED 파일은 I0만 공유 checkout에 반영한다. E1–E4는 proposal/owned diff+tests+affected contract를 전달하고 I0가 producer/consumer를 같이 변경한다.
- final source epoch는 해당 source/dependencies/config/binary/input/query/unit에서 정의한다. 입력 변경이 관련 proof/cell/report에 미치는 범위를 명시하고 영향 없는 raw를 무조건 버리지 않는다.
- conditional ticket은 code+proof 또는 조건 미성립의 실제 근거로 resolved다. 입력 부재 BLOCKED는 조건 미성립 NOT_APPLICABLE이 아니다.
- 배포·활성화·롤백은 실제 authorized target과 pre/post 관측이 있을 때만 각 상태를 발행한다. 이 계획 작성은 운영 action 실행 요청이 아니다.

## 해야 할 일

1. 파일/hunk·runtime resource·output namespace와 공용 contract 변화 목록을 고정하고 나머지 에픽을 동시에 착수시킨다.
2. owned changes와 conditional dispositions를 통합하고 surface별 repo mandatory gates를 실행한다.
3. final source에서 Contract·SDK proof/portable replay·hostedCI를 관측하고 capture/scale에 matching binaries를 전달한다.
4. 후속 E1 bootstrap/E4policy source 변경 시 affected proof/binaries/cells/report를 다시 판정하고 새 epoch를 발행한다.
5. release raw authority·P03–P12 current owners·paired producer/realprovider/Linux/state restore와 deployment/activation/rollback의 정확한 input/status를 별도 ledger에 닫는다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) | 공통 파일 소유권·계약·source epoch 관리 | P0 / `INTEGRATION` | 즉시 조사·fixture 준비 가능 |
| [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) | 최종 source의 Contract·SDK·CI 검증 | P0 / `PROOF_AND_BUILD` | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md), [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md), [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md), [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md), [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md), [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md), [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md), [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md), [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md), [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md), [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) |
| [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) | SEP-21 release·paired producer·배포 게이트 | P1 / `RELEASE_GATE` | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md), [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md), [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md), [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md), [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |

- `CONDITIONAL_CODE`는 병목/계약 실패 조건이 실제로 성립한 경우 구현한다. 조건 미성립은 근거가 있는 `NOT_APPLICABLE`로 닫는다.
- `PROOF_FIRST`/`PROOF_THEN_CONDITIONAL_CODE`는 baseline 결과와 독립 expected contract를 먼저 발행한다.
- 선행 결과가 `BLOCKED`/`NOT_RUN`이면 의존 실행은 완료로 표시하지 않는다. 소스 조사·fixture 준비는 계속 가능하다.

## 파일 소유권과 수정 위치

`OWNED`: 이 에픽 담당자가 해당 파일의 변경을 통합한다. `SHARED`: I0가 최종 공유 checkout에 반영하며 이 에픽은 구체적인 변경 proposal와 검증을 제출한다. `READ`: 기존 구현을 소비/검증하며 새 수정의 소유권을 뜻하지 않는다. 정확한 수정 내용·알고리즘·테스트는 각 연결 티켓의 파일 표에 있다.

| 파일 | 현재 진입점 / 확인할 경계 | 소유 모드 | 구체적 작업 |
| --- | --- | --- | --- |
| [.github/workflows/ci.yml](../../../../.github/workflows/ci.yml) | pinned CI test/runtime<br>hosted current-source checks | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md), [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [Cargo.lock](../../../../Cargo.lock) | Rust dependency resolution | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [Cargo.toml](../../../../Cargo.toml) | workspace/dependency authority | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [Justfile](../../../../Justfile) | retrieval-contract-local / retrieval-contract-proof / retrieval-sdk-proof-fresh / rust profiles | SHARED | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [benchmarks/retrieval/proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json) | python/rust/sdk identity lists | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [benchmarks/retrieval/src/record.rs](../../../../benchmarks/retrieval/src/record.rs) | native result/unit identity | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs) | existing backup/restore refusal proof | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) | R0–R6/P03–P12 current gate | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md) | current owner status | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [pyproject.toml](../../../../pyproject.toml) | benchmark Python dependencies | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [scripts/verify-repomap-cross-repo.sh](../../../../scripts/verify-repomap-cross-repo.sh) | current canonical paired verification | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/benchmark/retrieval/contract_proof.py](../../../../tools/benchmark/retrieval/contract_proof.py) | canonical contract execution | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/benchmark/retrieval/portable_proof.py](../../../../tools/benchmark/retrieval/portable_proof.py) | verify execution-context | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/benchmark/retrieval/proof_inventory.py](../../../../tools/benchmark/retrieval/proof_inventory.py) | actual pytest/Nextest selectors and verify | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/benchmark/retrieval/retrieval_contract.py](../../../../tools/benchmark/retrieval/retrieval_contract.py) | shared runtime/clock/profile constants | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | shared driver/proof/admission/phase consumers | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [tools/benchmark/retrieval/sdk_proof.py](../../../../tools/benchmark/retrieval/sdk_proof.py) | real-daemon SDK proof | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/ci/lint/check-proof-authority.py](../../../../tools/ci/lint/check-proof-authority.py) | paired_source_snapshot / aggregate checks | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/proof-authority.toml](../../../../tools/ci/proof-authority.toml) | registered execution-result authorities | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/proof_execution_result.py](../../../../tools/ci/proof_execution_result.py) | raw runner result authority | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/source_closure.py](../../../../tools/ci/source_closure.py) | final source closure | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/ci/write-proof-manifest.py](../../../../tools/ci/write-proof-manifest.py) | execution result custody | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [uv.lock](../../../../uv.lock) | Python frozen resolution | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |

## 병렬 착수와 의존 경계

I0는 공용 파일 통합·build admission·final source freeze 담당 한 명이다. E1–E4를 기다리며 schema/registry/CI inventory와 기존 authority negative controls를 준비한다. 동일 release host의 부하 proof는 직렬 실행한다.

I0-01이 ownership·contract를 freeze하고, I0-02가 해당 epoch의 owners/dispositions를 소비한다. E2/E4 실행은 I0-02의 binaries를 소비한다. I0-03의 release gate는 최종 labels/policy/operator/performance와 exact source pair를 요구한다.

## 에픽 완료 조건

- 공유 경계와 actual source/selector/result/binaries가 하나의 current authority에서 재생된다.
- 실행된 verification만 VERIFIED/FAILED로 표시하고 입력 부재·미실행·비적용을 구별한다.
- CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN은 실제로 관측한 별도 범위만 발행한다.

## 원본과 계약 근거

- [docs/handoff/oct-4/FINAL-REMAINING-WORK.md](../../../../docs/handoff/oct-4/FINAL-REMAINING-WORK.md)
- [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)
- [AGENT_CORE.md](../../../../AGENT_CORE.md)
- [AGENT_PLAYBOOK.md](../../../../AGENT_PLAYBOOK.md)
