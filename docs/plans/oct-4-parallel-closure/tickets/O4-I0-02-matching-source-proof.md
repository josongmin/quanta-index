# O4-I0-02 — 최종 source의 Contract·SDK·CI 검증

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P0 / `PROOF_AND_BUILD` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-I0-01](O4-I0-01-ownership-and-contract-freeze.md), [O4-E1-04](O4-E1-04-precise-name-span.md), [O4-E2-01](O4-E2-01-native-completed-timer.md), [O4-E2-05](O4-E2-05-semble-process-attribution.md), [O4-E3-02](O4-E3-02-admission-pin-transfer.md), [O4-E3-03](O4-E3-03-atomic-active-query-rpc.md), [O4-E3-04](O4-E3-04-maintenance-health-metering.md), [O4-E3-05](O4-E3-05-publish-timeout-replay.md), [O4-E3-06](O4-E3-06-operator-event-proof.md), [O4-E4-02](O4-E4-02-generation-durable-barriers.md), [O4-E4-03](O4-E4-03-ascii-scanner-decision.md), [O4-E4-04](O4-E4-04-source-token-authority.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

각 에픽의 변경·조건부 disposition을 통합한 source에서 필요한 테스트와 실제 SDK seam을 입증하고 matching capture binaries를 제공한다.

## 배경과 현재 상태

과거 frozen7ff/181 SDK25/25, d1a1b709 captures·hardening owner tests는 현재 successor source의 증거가 아니다. current registry collection은 test 실행과 별개다. SDK proof는 scale harness binaries도 증명하지 않는다.

## 착수 입력

- 각 prerequisite ticket의 code/proof 또는 근거 있는 NOT_APPLICABLE disposition
- final current source/lock/config, exact registry, canonical toolchain/resource admission
- checkout 밖 fresh Contract/SDK/source-closure/output roots

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/proof_inventory.py](../../../../tools/benchmark/retrieval/proof_inventory.py) | actual pytest/Nextest selectors and verify | actual collected identities를 source authority와 검사하고 zero/missing/skipped를 구분한다. | READ |
| [tools/benchmark/retrieval/contract_proof.py](../../../../tools/benchmark/retrieval/contract_proof.py) | canonical contract execution | 등록된 Python/Rust rail과 raw machine result를 final source에서 실행한다. | READ |
| [tools/benchmark/retrieval/sdk_proof.py](../../../../tools/benchmark/retrieval/sdk_proof.py) | real-daemon SDK proof | matching runner/searchd build·live SDK tests·context evidence를 발행한다. | READ |
| [tools/benchmark/retrieval/portable_proof.py](../../../../tools/benchmark/retrieval/portable_proof.py) | verify execution-context | independent source/commands/results/binary/raw replay를 한다. | READ |
| [Justfile](../../../../Justfile) | retrieval-contract-local / retrieval-contract-proof / retrieval-sdk-proof-fresh / rust profiles | 기존 front door를 사용하며 actual gap가 입증될 때만 recipe를 수정한다. | SHARED |
| [tools/ci/source_closure.py](../../../../tools/ci/source_closure.py) | final source closure | scope에 필요한 source/dependency roots를 bind하고 source drift를 거절한다. | READ |
| [.github/workflows/ci.yml](../../../../.github/workflows/ci.yml) | hosted current-source checks | 현재 source check/run와 결과를 조회하고 source-local proof와 분리한다. | SHARED |

## 실행 단계

1. conditional tickets는 구현 완료 또는 조건 미성립의 실제 근거가 있어야 prerequisite resolution으로 인정한다.
2. focused tests와 AGENT_PLAYBOOK surface별 escalation을 통합 source에서 실행한다.
3. source/runtime/lockfile과 actual test identity inventory를 고정하고 canonical Contract·SDK proof를 새 외부 root에서 실행한다.
4. portable proof verifier로 source/binary/input/results를 독립 재생하고 실제 selected/executed/passed/failed/skipped를 확인한다.
5. 필요한 hosted CI를 exact source에서 관측하고 remote result가 없으면 NOT_RUN/BLOCKED로 기록한다.
6. matching runner/daemon binary를 E2-04·E4-05/06에 넘긴다. source epoch가 바뀌면 영향을 다시 검증한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- just retrieval-contract-local
- just retrieval-contract-proof <fresh-external-contract-root>
- just retrieval-sdk-proof-fresh <fresh-external-sdk-root>
- uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt <fresh-external-sdk-root>/execution-context.json
- public SDK/contract: just rust-public-api; wire/decode: just rust-fuzz-smoke; module: just rust-hexagonal + just rust-cargo-modules; selection/state/ingress: just rust-profile test-daemon.

## 완료 조건

- exact final source/command/selector/binary의 raw results가 authoritative verifier를 통과한다.
- 필요한 CI/SDK/contract surfaces와 제외한 provider/Linux/release/scale scope를 명시한다.

## 중단·거절·재개 조건

- host admission refusal·build-lock timeout·interrupted/partial runner를 테스트 성공으로 표시하지 않는다.
- <fresh-external-...>는 실행 전 결정할 placeholder이며 기존 root로 재실행하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
