# I0 — 단일 통합 담당·source 검증·release 게이트

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: 단일 통합 담당.
- 3개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

- 웨이브 배치: [W0](../waves/W0-ownership-and-scope.md), [W3](../waves/W3-source-validation-and-admission.md), [W6](../waves/W6-release-and-final-closure.md). [전체 웨이브 지도](../WAVES.md)의 같은 단계 내부 순서·인계 조건을 따른다.

## 목적

공용 계약/파일과 final source epoch를 한 담당자가 통합하고, 정확히 영향을 받은 contract·SDK·CI·release/operations 범위의 proof를 발행한다.

## 배경과 현재 구현

- 감사 시작 기준은 main@f23af16f436c76ad4a700b75de4dd5b5771f56a6, clean이다. 44bd68a1 이후 차이는 문서뿐이며 product source 변경은 없다. 실행 때 HEAD/dirty/ownership을 다시 확인한다. staged81/vendor whitespace 해결과 제품/성능/release qualification을 구별한다.
- run.py·schemas·Rust benchmark DTO/record·proof registry/CI/lockfile를 여러 에픽이 동시에 수정하면 producer/consumer binding과 evidence epoch가 갈라진다.
- 기존 Contract/SDK proof inventory와 portable verifier를 재사용한다. collection·focused pass·commit/push는 actual source/selected tests/real daemon/release gates의 대체가 아니다.
- SEP21 R0–R6/P00–P12는 raw authority·P09process·P10restore·P11exact pair/Linux/actions를 별도 다룬다. P00/P01/P02A/P02B도 CODE_QUALIFIED prerequisites이며 P11 action recipes는 registry 선언만 있고 현재 Justfile에 없다.

## 목표 계약과 변경 원칙

- SHARED 파일은 I0만 공유 checkout에 반영한다. E1–E4는 proposal/owned diff+tests+affected contract를 전달하고 I0가 producer/consumer를 같이 변경한다.
- final source epoch는 해당 source/dependencies/config/binary/input/query/unit에서 정의한다. 입력 변경이 관련 proof/cell/report에 미치는 범위를 명시하고 영향 없는 raw를 무조건 버리지 않는다.
- conditional ticket은 code+proof 또는 조건 미성립의 실제 근거로 resolved다. 입력 부재 BLOCKED는 조건 미성립 NOT_APPLICABLE이 아니다.
- 배포·활성화·롤백은 실제 authorized target과 pre/post 관측이 있을 때만 각 상태를 발행한다. 이 계획 작성은 운영 action 실행 요청이 아니다.

## 해야 할 일

1. 파일/hunk·runtime resource·output namespace와 공용 contract 변화 목록을 고정하고 나머지 에픽을 동시에 착수시킨다.
2. 선택한 capture/performance/release epoch에 들어갈 product·collector·admission/scorer 변경을 PREPARE에서 통합하고 mandatory gates를 VALIDATE한다. unrelated conditional optimizations를 모든 실행의 선행으로 만들지 않는다.
3. 해당 source에서 Contract·SDK proof/portable replay·hostedCI 상태를 판정해 E1 admission ISSUE와 E2/E4 실행에 matching binaries를 전달한다. ISSUE 이후 source 변경은 다음 epoch다.
4. 후속 E1 bootstrap/E4policy source 변경 시 affected proof/binaries/cells/report를 다시 판정하고 새 epoch를 발행한다.
5. release raw authority·P00–P12 current owners·paired producer/realprovider/Linux/state restore/actions의 실제 target/input/results를 판정한다. Proposed OCT-04-002/003은 I0가 실제 요구에 따라 need/no-need/missing-input disposition으로 관리한다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) | 공통 파일 소유권·계약·source epoch 관리 | P0 / `INTEGRATION` | 즉시 조사·fixture 준비 가능 |
| [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) | 최종 source의 Contract·SDK·CI 검증 | P0 / `PROOF_AND_BUILD` | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) | SEP-21 release·paired producer·배포 게이트 | P1 / `RELEASE_GATE` | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |

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
| [Justfile](../../../../Justfile) | retrieval-contract-local / retrieval-contract-proof / retrieval-sdk-proof-fresh / rust profiles<br>proof-p11-deployment / proof-p11-activation / proof-p11-rollback / final qualification | SHARED | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md), [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [benchmarks/retrieval/proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json) | python/rust/sdk identity lists | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [benchmarks/retrieval/src/record.rs](../../../../benchmarks/retrieval/src/record.rs) | native result/unit identity | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [crates/quanta-index-contract/src/ipc/ingest.rs](../../../../crates/quanta-index-contract/src/ipc/ingest.rs) | source batch / terminal receipt | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-search-plane/src/semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs) | derive_semantic_stream_from_semantic_sources_v1 | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-searchd-runtime/src/state_migration.rs](../../../../crates/quanta-index-searchd-runtime/src/state_migration.rs) | state custody runtime owner | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs) | existing backup/restore refusal proof | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-searchd/src/app/state_format.rs](../../../../crates/quanta-index-searchd/src/app/state_format.rs) | current format / authority | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-searchd/src/app/state_migration.rs](../../../../crates/quanta-index-searchd/src/app/state_migration.rs) | backup / verify / restore-forward composition | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [crates/quanta-index-searchd/src/cli/command.rs](../../../../crates/quanta-index-searchd/src/cli/command.rs) | state migration CLI | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [docs/adr/OCT-04-002-configuration-and-generation-policy.md](../../../../docs/adr/OCT-04-002-configuration-and-generation-policy.md) | Proposed decision input | READ | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [docs/adr/OCT-04-003-source-preparation-sdk.md](../../../../docs/adr/OCT-04-003-source-preparation-sdk.md) | Proposed decision input | READ | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |
| [docs/operator/state-cutover-runbook.md](../../../../docs/operator/state-cutover-runbook.md) | operator cutover/rollback steps | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
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
| [tools/ci/binary_custody.py](../../../../tools/ci/binary_custody.py) | pin / verify | READ | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/lint/check-proof-authority.py](../../../../tools/ci/lint/check-proof-authority.py) | paired_source_snapshot / aggregate checks | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/paired_cargo_resolution.py](../../../../tools/ci/paired_cargo_resolution.py) | validate_resolution | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/proof-authority.toml](../../../../tools/ci/proof-authority.toml) | registered execution-result authorities | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/proof-manifest.schema.json](../../../../tools/ci/proof-manifest.schema.json) | ProofManifestV1 source/action/result fields | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/proof_execution_result.py](../../../../tools/ci/proof_execution_result.py) | raw runner result authority | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/source_closure.py](../../../../tools/ci/source_closure.py) | final source closure | READ | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [tools/ci/tests/test_paired_cargo_resolution.py](../../../../tools/ci/tests/test_paired_cargo_resolution.py) | resolved dependency mutants | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/tests/test_write_proof_aggregate.py](../../../../tools/ci/tests/test_write_proof_aggregate.py) | aggregate stage separation | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/write-proof-aggregate.py](../../../../tools/ci/write-proof-aggregate.py) | aggregate verdict / source pair | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [tools/ci/write-proof-manifest.py](../../../../tools/ci/write-proof-manifest.py) | execution result custody | SHARED | [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) |
| [uv.lock](../../../../uv.lock) | Python frozen resolution | SHARED | [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) |

## 병렬 착수와 의존 경계

I0는 공용 파일 통합·build admission·선택 source epoch 담당 한 명이다. E1–E4의 PREPARE와 함께 schema/registry/CI inventory·기존 authority controls를 준비하고 선택 scope의 proposal를 통합한다. 동일 release host의 부하 proof는 직렬 실행한다.

I0-01이 ownership·contract를 관리하고 I0-02가 선택 epoch의 code-ready owners/dispositions를 VALIDATE한다. E1 admission과 E2/E4는 같은 결과를 소비한다. I0-03 source qualification은 registry prerequisites와 exact pair를 요구하고, 제품 품질/unseen/performance 주장에만 해당 E1/E4 결과를 요구한다. missing Linux/operations inputs가 lexical benchmark 실행을 막지 않는다.

## 에픽 완료 조건

- 공유 경계와 actual source/selector/result/binaries가 하나의 current authority에서 재생된다.
- 실행된 verification만 VERIFIED/FAILED로 표시하고 입력 부재·미실행·비적용을 구별한다.
- CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN은 실제 필수 proof가 있는 별도 범위만 발행한다. BLOCKED/NOT_RUN ledger 정리만으로 요청된 qualification 작업을 완료 처리하지 않는다.

## 원본과 계약 근거

- [docs/handoff/oct-4/FINAL-REMAINING-WORK.md](../../../../docs/handoff/oct-4/FINAL-REMAINING-WORK.md)
- [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)
- [AGENT_CORE.md](../../../../AGENT_CORE.md)
- [AGENT_PLAYBOOK.md](../../../../AGENT_PLAYBOOK.md)
