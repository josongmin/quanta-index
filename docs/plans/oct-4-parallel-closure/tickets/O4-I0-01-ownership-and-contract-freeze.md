# O4-I0-01 — 공통 파일 소유권·계약·source epoch 관리

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P0 / `INTEGRATION` |
| 기준 웨이브 | [W0 — 소유권·실행 범위 고정](../waves/W0-ownership-and-scope.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

4개 에픽이 공통 source/schema/test registry를 중복 수정하지 않도록 단일 통합 경계를 운영한다.

## 배경과 현재 상태

이번 보완의 시작 기준은 main@f23af16f436c76ad4a700b75de4dd5b5771f56a6, checkout clean이다. 44bd68a1 이후 변경은 계획 문서이며 구현 소스는 같은 기준이다. 실행 착수 시 HEAD/dirty/로컬 ref를 다시 조회한다. 각 에픽의 owner-local proof와 해당 epoch qualification은 다르다. run.py·schema·registry·shared Rust runner changes가 경합의 중심이다.

## 착수 입력

- 현재 HEAD/status와 각 에픽의 planned file/hunk owner
- source/query/qrel/unit/profile의 계약 변경 목록, 기존 B07/B08/B09·SEP21 registry
- 실제 jobs/output/service-index resources 및 각 에픽의 diff/selector 결과

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | shared driver/proof/admission/phase consumers | E1/E2/E4 proposed consumer delta를 한 담당자가 반영해 current IR/validator를 유지한다. | SHARED |
| [benchmarks/retrieval/proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json) | python/rust/sdk identity lists | 실제 collection과 일치하게 새 tests를 등록한다. counts만으로 authority를 인정하지 않는다. | SHARED |
| [tools/benchmark/retrieval/retrieval_contract.py](../../../../tools/benchmark/retrieval/retrieval_contract.py) | shared runtime/clock/profile constants | 하나의 current identity owner를 유지하고 producer·consumer 동시 갱신을 관리한다. | SHARED |
| [benchmarks/retrieval/src/record.rs](../../../../benchmarks/retrieval/src/record.rs) | native result/unit identity | E1 span 계약과 E2/E4 capture clocks 변경을 현재 native record owner에서 함께 반영한다. | SHARED |
| [.github/workflows/ci.yml](../../../../.github/workflows/ci.yml) | pinned CI test/runtime | actual changed rails/pins만 반영하고 current checks를 관측한다. | SHARED |
| [Cargo.toml](../../../../Cargo.toml) | workspace/dependency authority | 필요한 dependency 변경만 Cargo.lock·uv/pyproject/CI와 같이 처리한다. | SHARED |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md) | current owner status | 현 source에서 완료된 helper/runtime/ring 항목과 실제 execution residual을 reconcile한다. | SHARED |
| [Cargo.lock](../../../../Cargo.lock) | Rust dependency resolution | 새 dependency가 실제 필요할 때 Cargo.toml과 같은 source epoch에서만 수정한다. | SHARED |
| [pyproject.toml](../../../../pyproject.toml) | benchmark Python dependencies | numeric/collector dependency 변경이 실제 필요한 경우 현 canonical Python environment에 반영한다. | SHARED |
| [uv.lock](../../../../uv.lock) | Python frozen resolution | pyproject.toml과 함께 frozen dependency resolution을 갱신하고 owner tests를 재실행한다. | SHARED |
| [docs/adr/OCT-04-002-configuration-and-generation-policy.md](../../../../docs/adr/OCT-04-002-configuration-and-generation-policy.md) | Proposed decision input | E3 actual operator/config 필요성을 모아 채택/불필요/추가 입력 필요 상태와 근거를 남긴다. 요구 부재를 구현 완료로 표시하지 않는다. | READ |
| [docs/adr/OCT-04-003-source-preparation-sdk.md](../../../../docs/adr/OCT-04-003-source-preparation-sdk.md) | Proposed decision input | 실제 producer fixture와 source-preparation 요구를 resolve하고 새 API의 선행 필요성을 판정한다. 현 lexical closure의 자동 선행 조건이 아니다. | READ |

## 실행 단계

1. ticket별 file/hunk owner와 mutation이 필요한 shared contract를 먼저 inventory로 확정한다.
2. shared file은 한 사람만 edit하며 에픽 owner는 change intent/test oracle를 제출한다.
3. main integration source epoch와 output namespace를 기록하고 old snapshot을 main에 덮어쓰지 않는다.
4. common required inventories·source/runtime/profile versions를 actual producers/consumers와 대조한다.
5. source/query/qrel/unit/profile이 변경될 때 영향받는 proof/capture만 재실행하도록 invalidation map을 만든다.
6. 각 owned diff·focused selectors·unrun gates를 모아 final source freeze와 I0-02를 진행한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- git status --short; git diff --check (각 command를 별도로 실행)
- source/required test identity collections와 proof-required-tests equality; producer/consumer field/profile consistency.
- Negative: 동시 schema twins, wrong owner reset/copy, count-only registry approval, stale source/context rebind를 거절한다.

## P2 설계 결정의 담당

- OCT-04-002는 I0가 E3의 실제 operator/config/readiness evidence를 받아 결정 근거를 기록한다. OCT-04-003은 I0가 실제 producer fixture/canonical caller를 받아 필요성을 판정한다.
- 입력이 없으면 해당 결정은 DEFERRED/BLOCKED로 남기고 구현 티켓을 합성하지 않는다. 불필요하다는 실제 판단이 있으면 이유와 trigger를 남긴다. 두 ADR는 아직 Proposed이며 이 계획 보완이 Accepted로 승격하지 않는다.

## 완료 조건

- 공통 파일에 한 명의 edit owner, 각 에픽 변경에 명시적 소비자·oracle·impact map이 있다.
- 실행 source/binary/input namespaces와 stale proof invalidation이 재생 가능하다.

## 중단·거절·재개 조건

- 현재 plans의 부분 dirty 상태를 깨끗하다고 표시하지 않는다. unrelated dirty work를 reset/stage/commit하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
