# OCT-04 병렬 종료 계획

**4개 구현/검증 에픽 + 단일 통합 담당(I0), 별도 티켓 29개.** 원본 5개 handoff의 중복·완료 구현을 제외한 합집합이다. 에픽별 목적·배경·해야 할 일과 티켓별 정확한 파일/함수/수정 방식·독립 검증·완료 조건을 분리했다.

- 계획 상태: `PLANNED`. 본 문서 작성은 새 모델 검수·제품 실행·구현·benchmark·배포를 수행한 결과가 아니다.
- 소스 기준: `main@0df06e0c3608e2198d98b96d34d44eaa9a5857af`. 작성 시작 시 checkout clean, 로컬 origin/main과 동일; 원격 fetch 관측은 아니다.
- 직전 구현 기준 `44bd68a1f41e15e7366adead56d825f8259ef46b` 이후 변경은 기존 통합 문서 한 파일이다. 핸드오프의 `2062fed3` dirty/staged 상태를 현재 사실로 재사용하지 않는다.
- 원본: [agent-1](../../handoff/oct-4/agent-1.md), [agent-2](../../handoff/oct-4/agent-2.md), [agent-3](../../handoff/oct-4/agent-3.md), [agent-4](../../handoff/oct-4/agent-4.md), [agent-5](../../handoff/oct-4/agent-5.md).
- [기존 통합 잔여 목록](../../handoff/oct-4/FINAL-REMAINING-WORK.md)은 합집합 요약, **이 디렉터리와 [티켓 인덱스](tickets/INDEX.md)는 담당 분할·실행 상세의 기준**이다. 실제 실행 결과는 기존 B07/B08/B09·SEP21 owning tickets에 반영한다.

## 1. 에픽 담당 분할

| 담당 | 목적과 소유 영역 | 티켓 수 | 바로 시작할 조사·fixture |
| --- | --- | --- | --- |
| [E1 — 정답·검수·admission과 독립 평가](epics/E1-labels-admission-and-gold.md) | 하나의 source/query/rubric 결속 라벨 권위에서 실제 검수와 admission을 발행하고, 파일 적중·정확한 선언 이름·NL relevance를 각 단위로 평가한다. | 7 | [O4-E1-01](tickets/O4-E1-01-original-review-resume.md), [O4-E1-04](tickets/O4-E1-04-precise-name-span.md), [O4-E1-05](tickets/O4-E1-05-untouched-holdout.md) |
| [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](epics/E2-external-capture-and-timing.md) | 5제품의 실제 native source 범위와 completed-response 시간을 같은 계약에서 수집하고, 모든 필수 셀의 결과와 blind candidate union을 E1에 전달한다. | 6 | [O4-E2-01](tickets/O4-E2-01-native-completed-timer.md), [O4-E2-02](tickets/O4-E2-02-external-index-universe.md), [O4-E2-03](tickets/O4-E2-03-required-cells-and-scheduling.md), [O4-E2-05](tickets/O4-E2-05-semble-process-attribution.md), [O4-E2-06](tickets/O4-E2-06-quality-only-warmup.md) |
| [E3 — Active 선택·read-view lifetime·운영 계약](epics/E3-selection-and-operational-safety.md) | 선택→view acquisition→응답의 generation/token custody와 admitted publish·readiness·operator truth를 실제 counterexample에서 검증하고 확인된 결함을 소유 권위에서 수정한다. | 6 | [O4-E3-01](tickets/O4-E3-01-active-selection-race.md), [O4-E3-04](tickets/O4-E3-04-maintenance-health-metering.md), [O4-E3-05](tickets/O4-E3-05-publish-timeout-replay.md), [O4-E3-06](tickets/O4-E3-06-operator-event-proof.md) |
| [E4 — 인덱싱·typo 실행 비용·release 성능·scale](epics/E4-storage-query-and-scale.md) | 현재 lexical lifecycle과 전체 query 비용의 병목을 먼저 측정하고, output·durability·resource 계약을 지키는 구조 변경만 채택한 뒤 정식 반복 성능과 scale을 판정한다. | 7 | [O4-E4-01](tickets/O4-E4-01-index-phase-profile.md), [O4-E4-03](tickets/O4-E4-03-ascii-scanner-decision.md) |
| [I0 — 단일 통합 담당·source 검증·release 게이트](epics/I0-integration-and-release-gates.md) | 공용 계약/파일과 final source epoch를 한 담당자가 통합하고, 정확히 영향을 받은 contract·SDK·CI·release/operations 범위의 proof를 발행한다. | 3 | [O4-I0-01](tickets/O4-I0-01-ownership-and-contract-freeze.md) |

담당자는 자신의 에픽 문서와 각 티켓을 먼저 읽고 source/dirty/hunk ownership을 확인한다. 같은 에픽 안의 동일 파일은 한 담당자가 순서대로 합친다. 당장 모든 dependent runtime 실행을 시작한다는 뜻은 아니다.

## 2. 실행 순서와 피드백

| 단계 | 실제 종료 경계 |
| --- | --- |
| A — 병렬 준비 | I0 ownership/contract 관리, E1 원본 실패·name oracle·holdout, E2 timer/scope/controller/Semble/warmup, E3 결정적 safety proofs, E4 lifecycle/ASCII profile을 병렬 준비한다. |
| B — 필요한 source 변경 | 재현된 계약 실패·측정된 병목만 수정한다. E3 selection decision→admission→single-RPC, E4 profiling→conditional durable/token 변경을 같은 권위에서 통합한다. |
| C — 해당 source epoch 검증 | [O4-I0-02](tickets/O4-I0-02-matching-source-proof.md)에서 필요한 Contract·SDK·surface gates를 수행하고 matching binaries를 발행한다. 조건부 티켓은 code+proof 또는 근거 있는 NOT_APPLICABLE로 resolve한다. |
| D — admission과 실제 셀 | [O4-E1-03](tickets/O4-E1-03-admission-and-split.md)의 source-bound admission을 소비해 [O4-E2-04](tickets/O4-E2-04-fresh-five-product-captures.md)를 실행한다. **repository별 선행 결과**를 적용하므로 ready 저장소는 실패한 sibling을 기다리지 않는다. cohort 전체 상태도 함께 보존한다. |
| E — 마지막 union과 재채점 | [O4-E1-06](tickets/O4-E1-06-final-pool-and-scoreboards.md)이 E2 fresh pool의 새 pair를 실제 검수하고 qrel/suite/admission revision을 발행한다. 요청 binding이 유지되면 immutable raw replay, 바뀌면 영향 셀만 새 root에서 실행한다. |
| F — 성능·정책·release | E4 scale/정식 성능과 E1 independent labels/holdout 기반 정책 RCA를 닫는다. source 변경이 있으면 C–E의 영향 범위를 다시 열고, 최종 [O4-I0-03](tickets/O4-I0-03-release-operational-gates.md)에서 release/운영 게이트를 별도로 판정한다. |

- 티켓 선행 목록은 초기 DAG다. 최종 qrel/source/policy revision은 **새 epoch의 영향 범위 재검증**이며 옛 receipt를 새 결과로 재명명하지 않는다.
- E1-07의 순수 numeric 변경은 scorer tests/report 재생을, E4-07의 planner/ranking/policy 변경은 affected SDK/Contract/binary/native cells를 다시 판정한다. 변경과 무관한 raw를 무조건 재실행하지 않는다.
- 후속 정책이 유지돼야 하는지 바뀌어야 하는지 먼저 결정한다. 근거 없는 모델·RRF·청킹·스토리지 전면 교체를 final gate의 선행 작업으로 만들지 않는다.
- code inspection·작은 owner fixtures는 병렬 가능하다. build/cache/fixture root·외부 index/model jobs·output namespace는 resource owner가 관리한다. 실제 제품/ingest/scale/정식 성능과 heavy builds는 동일 host admission에서 직렬 실행한다.

## 3. 공용 파일 소유권

**SHARED 파일은 I0가 공유 checkout에 반영한다.** E1–E4는 proposal/owned diff·producer/consumer 영향·좁은 tests를 제출한다. 동일 파일을 서로 덮어쓰거나 실패 consumer에 보정 계층을 추가하지 않는다.

| 소유 | 범위와 규칙 |
| --- | --- |
| E1 | review/admission inputs·evaluator/source_oracle·split/corpus binding·fresh join/report·execution_batch. 라벨·단위·평가 수식 권위 하나. |
| E2 | native external collector·SG/OG scope producer·Semble/ARB adapter. transport/worker/completed clocks 의미를 보존한다. |
| E3 | SDK client/binding/observability·plane selection/read-view/retention·runtime maintenance/readiness·publish timeout. 기존 operator surfaces는 proof 우선. |
| E4 | lexical index/source/coverage/searcher·query timing helper·scale/open-loop harness·policy RCA. 저장/query 비용의 전체 tradeoff를 증명한다. |
| I0 | 아래 SHARED 파일, shared schemas·CI·dependency resolution·source freeze/build admission·release status. Rust benchmark DTO/SDK seam과 IPC 계측 proposal는 관련 에픽 검토 후 통합한다. |

- [.github/workflows/ci.yml](../../../.github/workflows/ci.yml)
- [Cargo.lock](../../../Cargo.lock)
- [Cargo.toml](../../../Cargo.toml)
- [Justfile](../../../Justfile)
- [benchmarks/retrieval/proof-required-tests.json](../../../benchmarks/retrieval/proof-required-tests.json)
- [benchmarks/retrieval/src/diagnostics.rs](../../../benchmarks/retrieval/src/diagnostics.rs)
- [benchmarks/retrieval/src/main.rs](../../../benchmarks/retrieval/src/main.rs)
- [benchmarks/retrieval/src/query_plan.rs](../../../benchmarks/retrieval/src/query_plan.rs)
- [benchmarks/retrieval/src/record.rs](../../../benchmarks/retrieval/src/record.rs)
- [benchmarks/retrieval/src/symbols.rs](../../../benchmarks/retrieval/src/symbols.rs)
- [benchmarks/retrieval/tests/sdk_roundtrip.rs](../../../benchmarks/retrieval/tests/sdk_roundtrip.rs)
- [crates/quanta-index-contract/src/ipc/split.rs](../../../crates/quanta-index-contract/src/ipc/split.rs)
- [crates/quanta-index-ipc/src/server.rs](../../../crates/quanta-index-ipc/src/server.rs)
- [crates/quanta-index-ipc/src/server/tests.rs](../../../crates/quanta-index-ipc/src/server/tests.rs)
- [crates/quanta-index-search-plane/src/query_dispatcher/planning.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher/planning.rs)
- [crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs)
- [crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs](../../../crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs)
- [crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs](../../../crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs)
- [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)
- [docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md](../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md)
- [pyproject.toml](../../../pyproject.toml)
- [tools/benchmark/retrieval/README.md](../../../tools/benchmark/retrieval/README.md)
- [tools/benchmark/retrieval/admission.schema.json](../../../tools/benchmark/retrieval/admission.schema.json)
- [tools/benchmark/retrieval/lexical_file_comparison.py](../../../tools/benchmark/retrieval/lexical_file_comparison.py)
- [tools/benchmark/retrieval/retrieval_contract.py](../../../tools/benchmark/retrieval/retrieval_contract.py)
- [tools/benchmark/retrieval/run.py](../../../tools/benchmark/retrieval/run.py)
- [tools/ci/lint/check-proof-authority.py](../../../tools/ci/lint/check-proof-authority.py)
- [tools/ci/proof-authority.toml](../../../tools/ci/proof-authority.toml)
- [tools/ci/proof_execution_result.py](../../../tools/ci/proof_execution_result.py)
- [tools/ci/tests/test_retrieval_benchmark.py](../../../tools/ci/tests/test_retrieval_benchmark.py)
- [tools/ci/write-proof-manifest.py](../../../tools/ci/write-proof-manifest.py)
- [uv.lock](../../../uv.lock)

`OWNED`/`READ`/`SHARED`의 정확한 파일·함수·변경 방식은 [에픽 파일 지도](epics/E1-labels-admission-and-gold.md)와 각 티켓 표에 있다. 공통 schema 변경은 actual producer/consumer 필드가 필요할 때만 선택한다.

## 4. 구현·검증 상태 규칙

- `PLANNED`는 작업 workflow 상태다. `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`은 실제 verification 범위에 쓴다.
- `CONDITIONAL_CODE`: 측정된 병목·contract 실패 조건이 성립한 경우 구현. 입력 부재나 아직 측정하지 않은 상태는 NOT_APPLICABLE이 아니다.
- `PROOF_FIRST`와 `PROOF_THEN_CONDITIONAL_CODE`: fixed independent expectation으로 baseline을 판정한 후 필요 시 구조 수정. compile/component pass만으로 seam proof를 닫지 않는다.
- `PROOF_ONLY`: 기존 구현의 actual process/negative path 검증. 현재 ring/API/SDK/CLI를 재구현하는 티켓이 아니다.
- 모든 티켓의 명령은 **검증 계획, NOT_RUN**이다. `<...>`는 실제 input/output 값을 실행 전에 결정할 placeholder다. filter를 쓰면 actual selected test IDs와 0-test 여부를 확인한다.
- runtime crate는 `autotests=false`다. `e2e_read_view`/`e2e_ingest_idempotency`는 `runtime_fast_suite`, generation/cursor/restart는 `runtime_risk_suite`, crash/readiness는 `runtime_extended_suite` filter로 실행한다. `--test e2e_...`라는 존재하지 않는 binary를 만들지 않는다.
- 공개SDK/contract → `just rust-public-api`; wire/decode → `just rust-fuzz-smoke`; module/facade → `just rust-hexagonal`+`just rust-cargo-modules`; selection/state/ingress → `just rust-profile test-daemon`+owning scenario. 근거: [AGENT_PLAYBOOK](../../../AGENT_PLAYBOOK.md).
- 일회성 logs/raw/receipts/captures는 checkout 밖 fresh root에 둔다. 계획 문서는 evidence dump나 qualified result가 아니다.

## 5. 원본 잔여 → 티켓 합집합

| 남은 작업 | 분리 티켓 | 원본 위치 |
| --- | --- | --- |
| 원본 C3 실패·unresolved·valid raw cache 복구 | [O4-E1-01](tickets/O4-E1-01-original-review-resume.md) | agent-1 §3, agent-2 A2-01, agent-3 P0-2, agent-5 §11 |
| supplemental union 실제 검수·adjudication·merged qrels | [O4-E1-02](tickets/O4-E1-02-supplemental-labels.md), [O4-E1-06](tickets/O4-E1-06-final-pool-and-scoreboards.md) | agent-1 §3, agent-2 A2-01/05, agent-3 P0-1/3, agent-4 A4-08 |
| suite/split/license/source/threshold/admission·C5 stale4 reissue | [O4-E1-03](tickets/O4-E1-03-admission-and-split.md) | agent-1 §3, agent-2 A2-02, agent-3 P0-3 |
| declaration-name/ID/span·file hit 분리 | [O4-E1-04](tickets/O4-E1-04-precise-name-span.md) | agent-2 A2-05, agent-4 A4-08, agent-5 span evaluator |
| 독립 NL/relevance/same-name/no-answer·미사용 holdout | [O4-E1-02](tickets/O4-E1-02-supplemental-labels.md), [O4-E1-05](tickets/O4-E1-05-untouched-holdout.md) | agent-1 semantic quality, agent-2 A2-05, agent-5 gold/holdout |
| final pool·scoreboards·repository CI·pool exposure sensitivity | [O4-E1-06](tickets/O4-E1-06-final-pool-and-scoreboards.md) | agent-2 A2-05, agent-3 P0-4, agent-5 capture/report |
| cold bootstrap bounded vectorization | [O4-E1-07](tickets/O4-E1-07-bounded-bootstrap.md) | agent-4 A4-04 |
| native completed-response 외부 timer | [O4-E2-01](tickets/O4-E2-01-native-completed-timer.md) | agent-2 A2-03, agent-4 A4-06 |
| SG/OG 전체 indexed universe·before/after·scope | [O4-E2-02](tickets/O4-E2-02-external-index-universe.md) | agent-1 B08, agent-2 A2-04, agent-4 A4-08, agent-5 external index |
| required cells·ready/failed queue 실제 소비·fresh five-product | [O4-E2-03](tickets/O4-E2-03-required-cells-and-scheduling.md), [O4-E2-04](tickets/O4-E2-04-fresh-five-product-captures.md) | agent-2 A2-02/04, agent-3 P0-4, agent-4 A4-08 |
| Semble process residual의 child clock 귀속 | [O4-E2-05](tickets/O4-E2-05-semble-process-attribution.md) | agent-4 A4-06 |
| quality-only warmup0 실제 parity·speed refusal | [O4-E2-06](tickets/O4-E2-06-quality-only-warmup.md) | agent-4 A4-03 |
| G1→G2/G3 selection/retention/acquisition·bounded admission pin | [O4-E3-01](tickets/O4-E3-01-active-selection-race.md), [O4-E3-02](tickets/O4-E3-02-admission-pin-transfer.md) | agent-1 P04, agent-4 A4-02 관련 설계 |
| single-RPC Active query·snapshot/token/ABA binding | [O4-E3-03](tickets/O4-E3-03-atomic-active-query-rpc.md) | agent-4 A4-02 |
| slow full-tree metering·readiness freshness | [O4-E3-04](tickets/O4-E3-04-maintenance-health-metering.md) | agent-1 P09, OCT-04-001 |
| admitted slow publish timeout·operation exact replay | [O4-E3-05](tickets/O4-E3-05-publish-timeout-replay.md) | agent-1 P09, OCT-04-001 |
| 기존 operator process truth·ring/auth/SDK/searchctl proof | [O4-E3-06](tickets/O4-E3-06-operator-event-proof.md) | SEP-21 P09; 현 source에서 구현 확인 |
| full/delta/delete/no-op/reopen resource/phase·조건부 ingress 귀속 | [O4-E4-01](tickets/O4-E4-01-index-phase-profile.md) | agent-1 B07/P09, agent-3 P1-2/P2-1, agent-4 §6 |
| durable generation batch·inherited hardlink·crash cuts | [O4-E4-02](tickets/O4-E4-02-generation-durable-barriers.md) | agent-3 P1-2, agent-4 A4-01 |
| ASCII deletion whole-call 유지/수정/철회 | [O4-E4-03](tickets/O4-E4-03-ascii-scanner-decision.md) | agent-3 P1-1, agent-4 A4-05 |
| distinct raw token source/witness authority·완전성 | [O4-E4-04](tickets/O4-E4-04-source-token-authority.md) | agent-4 A4-05 |
| matching release scale/open-loop/OS restart | [O4-E4-05](tickets/O4-E4-05-release-scale-load.md) | agent-3 P2-2, agent-4 A4-07 |
| 동일 경계 반복 성능·host admission·5roots/1000obs | [O4-E4-06](tickets/O4-E4-06-qualified-performance.md) | agent-1 B07, agent-3 P2-3, agent-4 A4-07, agent-5 B07 |
| default23·Gin4·NL/semantic candidate/fusion/model/chunking RCA | [O4-E4-07](tickets/O4-E4-07-policy-and-semantic-residuals.md) | agent-1 semantic quality, agent-5 semantic/product policy |
| ownership·shared schema/registry·current source Contract/SDK/CI | [O4-I0-01](tickets/O4-I0-01-ownership-and-contract-freeze.md), [O4-I0-02](tickets/O4-I0-02-matching-source-proof.md) | 모든 agent handoff의 source/ownership 및 verification 잔여 |
| R0–R6/P03–P12·raw authority·exact pair/Linux/restore/배포 | [O4-I0-03](tickets/O4-I0-03-release-operational-gates.md) | agent-1 P04/P09, SEP-21 final residual plan |

과거 reviewer/capture watcher가 살아 있다는 기록은 현재 실행 상태가 아니다. 이번 조사에서 원본 review·supplemental queue·remaining terminal은 FAILED였다. repaired tmp `/private/tmp/qi-bench-defect-fix-20261004-46iq_iok`는 현재 MISSING이며, 실제 입력 생성 preflight부터 현 canonical source로 재결속한다. 외부 원본 BASE와 구체적 log는 [O4-E1-01](tickets/O4-E1-01-original-review-resume.md)에 있다.

## 6. 조건부 후속과 범위 밖

| 항목 | 착수 조건 / 현재 처리 |
| --- | --- |
| immutable source/coverage pack | group barriers 후에도 content full-sync가 실제 지배할 때만 별도 storage 설계/티켓을 추가한다. digest→offset/length·bounded read·torn pack·GC/inherited references proof 필요. E4-02에 묶지 않는다. |
| ingest slot 확대 / async ACK | measured contention와 admitted durable-source custody·ACK/replay 계약을 먼저 입증한다. E3/E4 프로파일링 전 구현하지 않는다. |
| OCT-04-002 config/generation policy | 실제 operator 요구가 확인돼야 채택한다. [Proposed ADR](../../adr/OCT-04-002-configuration-and-generation-policy.md); 현재 mandatory implementation ticket 없음. |
| OCT-04-003 source-preparation SDK | 실제 producer fixture·required contract가 필요하다. [Proposed ADR](../../adr/OCT-04-003-source-preparation-sdk.md); 기존 Semantica 경로와의 필요성 검증 전 새 API를 만들지 않는다. |
| 전체 CoIR/CORE/CSN 수입·새 scorer/IR/harness | 현재 남은 실패를 해결하는 근거 없음. 준비된 범위와 unknown/excluded population만 보고한다. |
| 사람 검수·real-provider·Linux/배포 | 실제 reviewer/환경/target가 있어야 해당 qualification을 발행한다. AI 결과를 human review로 승격하거나 lexical repair의 선행 조건으로 새 cross-repo API를 끼워 넣지 않는다. |
| 과거 staged81/vendor whitespace·threshold/queue/runtime-pin/Unicode/JS grammar 수리 | 현재 main에 반영된 구현이다. 현재 source의 필요한 좁은 rail만 수행하며 같은 수정 티켓을 다시 만들지 않는다. |

## 7. 담당별 착수 문구

각 담당에게 해당 에픽 파일 한 개와 아래 범위를 전달하면 된다.

- **E1**: E1 문서의 7개 티켓을 맡고 original review→supplemental→source-bound admission→fresh final pool→scoreboards를 한 라벨 권위로 닫는다. evaluator/source gold·holdout/name metric 변경을 소유하고 SHARED 변경은 I0에 제출한다.
- **E2**: E2 문서의 6개 티켓을 맡고 native completed timer·index universe·actual required-cell execution·Semble phase·warmup parity를 닫는다. E1 repository admission과 I0 matching binaries를 소비한다.
- **E3**: E3 문서의 6개 티켓을 맡고 deterministic selection/retention·SDK binding·timeout/replay·slowwalk/readiness·operator proof를 닫는다. 현 계약과 Proposed 강화 결정을 구분하고 입증된 결함만 수정한다.
- **E4**: E4 문서의 7개 티켓을 맡고 lifecycle/ASCII profile→조건부 저장/token 변경→release scale/performance와 독립 gold 기반 정책 RCA를 수행한다. 전체 비용·durability·output 계약을 보존한다.
- **I0**: 공용 파일·hunk와 runtime resource admission을 관리하고 각 에픽의 owned changes/dispositions를 통합한다. final source Contract/SDK/CI와 이후 release/operations 상태를 별도로 판정한다.

## 8. 문서 작성의 검증 범위

- `VERIFIED`: 원본 5개 handoff 잔여의 대응표, 현 source 파일/진입점·Cargo test target 확인, 생성 문서의 local path/link·29개 ID/의존 DAG·shared ownership·whitespace 검증.
- `NOT_RUN`: ticket implementation, Rust/Python product tests, 새 AI review·native captures·정식 성능/scale, hosted CI·paired provider/Linux·배포/활성화/롤백.
