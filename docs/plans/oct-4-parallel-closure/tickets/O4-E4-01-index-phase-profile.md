# O4-E4-01 — 인덱싱 lifecycle 비용·resource 원인 분해

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

full/delta/delete/no-op/reopen의 저장·검증·shard 비용과 transient resource를 분해해 필요한 최적화와 capacity 거절을 결정한다.

## 배경과 현재 상태

현재 diagnostic9/protocol7/phase4, ingest preparation/coverage/source children, SDK request-local attribution과 sampled CPU/RSS가 있다. 과거 g9의 coverage+source2.346초/SDKpublish3.581초는 한 busy-host release 진단이다. compiler/model preparation와 index/publish/seal/activate 시간은 다른 경계다.

## 착수 입력

- 현 source/driver schema와 matching release binaries; fresh same corpus roots
- full/delta/delete/no-op/reopen fixture와 independent fresh rebuild oracle
- disk free/allocated/logical/transient와 sampled RSS/CPU probe scope

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-contract/src/ipc/ingest_observation.rs](../../../../crates/quanta-index-contract/src/ipc/ingest_observation.rs) | ingest stage observation | READ: 현재 child clocks를 재사용한다. 실제 빠진 phase만 I0가 contract에 통합한다. | READ |
| [crates/quanta-index-lexical/src/adapter_ingest.rs](../../../../crates/quanta-index-lexical/src/adapter_ingest.rs) | publish preparation/build observations | 기존 measured stage와 parent bounds를 사용하고 observer overhead/on-off를 보존한다. | OWNED |
| [crates/quanta-index-lexical/src/adapter_open.rs](../../../../crates/quanta-index-lexical/src/adapter_open.rs) | cold-open phase | same-process adapter reopen와 actual process restart를 다른 observation으로 기록한다. | OWNED |
| [crates/quanta-index-lexical/src/file_authority.rs](../../../../crates/quanta-index-lexical/src/file_authority.rs) | plan_ops / apply_plan / from_verified_files | source persistence·digest/admission·postings build의 실제 work를 구별한다. | OWNED |
| [benchmarks/retrieval/src/diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs) | current ingest/query diagnostics | 현 schema field를 소비하며 parent bounds와 absent/invalid state를 보존한다. | SHARED |
| [tools/benchmark/retrieval/query_timing_overhead.py](../../../../tools/benchmark/retrieval/query_timing_overhead.py) | observation profile | observation on/off output equality와 request work counter를 현 paths로 대조한다. | OWNED |
| [crates/quanta-index-lexical/tests/l2_file_mutation.rs](../../../../crates/quanta-index-lexical/tests/l2_file_mutation.rs) | full/delta/delete/no-op fixture | 독립 fresh rebuild와 expected sources/windows/coverage로 parity를 증명한다. | OWNED |
| [crates/quanta-index-ipc/src/server.rs](../../../../crates/quanta-index-ipc/src/server.rs) | RequestEventScope / DispatchContext.record_event_v1 | request/connection-bound 기존 events를 먼저 소비한다. prevalidation/ingress residual이 측정상 계속 클 때만 E3와 함께 최소 stage를 제안하고 I0가 반영한다. | SHARED |
| [crates/quanta-index-ipc/src/server/tests.rs](../../../../crates/quanta-index-ipc/src/server/tests.rs) | request event / ingress/deadline controls | 계측이 바뀌면 request/connection 정확한 join·terminal1회·zero drops와 credentials/deadline/cancel/partial-response를 독립 fixture로 검사한다. | SHARED |

## 실행 단계

1. 현재 child/parent 시간과 work counters의 포함 관계를 static source graph에서 고정한다.
2. 작은 full/delta/delete/no-op/reopen actual run에서 rows/cursor/source authority를 fresh rebuild와 비교한다.
3. matching release에서 source/coverage/shard/digest/sync 또는 명시적 unattributed 구간을 분해한다.
4. prevalidation/ingress residual이 반복해서 큰 경우만 기존 request-local events에서 정확한 request/connection join과 zero drops를 확인한다. 필요한 최소 계측은 E3 경계 검토 후 I0가 통합하며 RPC roundtrip 차이를 IPC로 단정하지 않는다.
5. 관측 overhead·sample gap·parent probes/CPU domain을 표기하고 disk logical/physical/transient를 따로 측정한다.
6. 내부 성능 원인과 resource admission refusal을 E4-02/03/04/05의 조건으로 전달한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- ./scripts/cargow test -p quanta-index-lexical --test l2_file_mutation --locked
- uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_completed_response_timing.py -q -k 'ingest or detailed_file_authority or observation'
- Negative: child>parent, missing observer phase, process CPU scope 혼동, wrong source/delete/no-op result, sample gap을 peak으로 정상화 거절.
- IPC 계측 변경 시 ./scripts/cargow test -p quanta-index-ipc --lib --all-features --locked 및 실제 SDK request/connection join fixture; decode/wire 변경은 just rust-fuzz-smoke.

## 완료 조건

- 모든 lifecycle phase는 explicit timer/resource domain과 source-bound result parity를 갖는다.
- 어떤 비용이 어느 work에 속하는지 설명하고 실제 큰 residual이 아니면 추가 instrumentation을 중단한다.

## 중단·거절·재개 조건

- file별 fsync를 봤다는 사실만으로 주원인을 단정하지 않는다. syscall 특권이 없으면 physical I/O는 NOT_RUN이며 임의 privilege escalation으로 대체하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
