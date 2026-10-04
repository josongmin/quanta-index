# O4-E3-03 — 단일 RPC의 Active 선택·검색·응답 결속

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `CODE_AND_PROOF` |
| 기준 웨이브 | [W2 — 확인된 결함 수리·선택 최적화](../waves/W2-repairs-and-selected-optimizations.md) |
| 실행 상태 | 지원7 Active variant의 실제 daemon single-RPC/head/token/row와 stale-token 거절 `VERIFIED`; SDK27 assertion PASS/stdio leak1 관측, 단독 재실행 ordinary PASS. current daemon profile 실행 중; formal release qualification `NOT_RUN` |
| 선행 결과 | [O4-E3-01](O4-E3-01-active-selection-race.md), [O4-E3-02](O4-E3-02-admission-pin-transfer.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 2026-10-04 통합 갱신

- Text/Symbol/Semantic/Hybrid/HybridSeed/History/RuntimeMetadata의 optional `selected_active_head`를 선택 당시 catalog snapshot에서 materialize한다. SDK는 이 head와 response generation/token/secondary lane identity를 검사한다.
- 지원 Active route의 사전 resolve RPC를 제거했다. Structural Active refusal, SemanticWorkBounded exact-generation 제한, Active cursor의 exact-pin 요구와 `rev:at.time` ancestor preflight는 별도 계약으로 보존한다.
- strict wire 방문자, response literals, searchctl/harness, 실제 benchmark request-event consumer를 함께 수정했다. 선택 후 read-view admission 전에 G1이 폐기되면 기존 typed refusal이 가능하며 이 변경이 admission lease를 추가한 것은 아니다.
- 초기 중앙 compile에서 Structural macro field와 SDK test import 누락을 확인해 수정했다. owner batch, 실제 Text/Symbol one-RPC, wire fuzz smoke 및 public API baseline은 아래 실행 범위에서 통과했다.
- `real_daemon_sdk_active_text_and_symbol_bind_one_selected_head_without_resolve`를 실제 daemon SDK integration에 추가했다. query-only SDK의 Text/Symbol 각 1RPC, ACK와 selected generation/token 결속, query-ring admission/terminal을 검사한다. G31→G32 실제 successor activation 뒤 G31 token 요청의 typed `NotReady`도 같은 fixture에서 검증한다.
- `VERIFIED`: test-fast-lane env를 source하고 같은 lane의 debug `quanta-index-searchd`를 `QUANTA_INDEX_SEARCHD_BIN`으로 pin한 뒤 `./scripts/cargow --lane test-fast-lane test --workspace --test sdk_roundtrip --all-features --locked` —26 passed /0 failed /25.07초, exit0. 새 live Active/stale-token case도 통과했다. 이 결과는 Linux/fresh-release formal SDK proof가 아니다.
- 중앙 workspace 실행에서 stale SDK positive mocks 2건을 수리한 뒤 SDK lib와 search-plane lib는 통과했다. 이 결과는 새 live SDK integration 실행을 대신하지 않는다. wire 4-target fuzz smoke와 public API baseline check는 `VERIFIED`다.

## 2026-10-04 지원 Active route 실제 확장 검증

- 실제 SDK27 첫 배치는26 passed/1 failed였다. 새 remaining-route fixture의 History positive가 `HistoryShardUnavailable`을 반환했다. 현 `history_shard_requirements`는 symbolic `rev:`에 ref·tag shard를 모두 요구하지만 fixture는 ref만 발행했다. production refusal는 계약대로였고 fixture의 G41/G42별 tag를 commit metadata와 같은 SHA로 실제 게시하도록 수정했다. branch query를 제거하거나 head/pin/1RPC/row·stale-token oracle를 완화하지 않았다.
- `VERIFIED`: matching debug daemon을 `QUANTA_INDEX_SEARCHD_BIN`으로 pin한 `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked --no-fail-fast --test-threads 4` — exit0,27 run/27 passed,0 skipped, tests19.169s. 지원 Semantic/Hybrid/HybridSeed/History/RuntimeMetadata 각각 G41와 G42의 exact head/pin/nonempty 또는 fixed row·request count1 및 G42 이후 stale G41 typed refusal가 통과했다. Text/Symbol live fixture도 같은 배치에서 통과했다.
- Nextest는 `actual_runner_binary_emits_receipt_bound_v5_record`의 stdio handle leak1건을 표시했다. 해당 selector를 `-E 'test(=actual_runner_binary_emits_receipt_bound_v5_record)' --success-output final`로 단독 재실행한 결과 exit0,1 passed/26 skipped,9.236s이며 leak 표시는 없었다. 명백한 inherited-capture 경로를 정적 추적에서 찾지 못했고, 원래 leak 관측은 보존한다. 이것만으로 모든 process cleanup 또는 formal SDK qualification을 선언하지 않는다.
- fixture는 hash-dev, secure local state와 실제 OS daemon/UDS scope다. learned semantic 품질·speedup·Linux 및 current-source formal release proof는 별도 결과가 필요하다. Structural Active refusal, SemanticWorkBounded exact-only, cursor/ancestor 도메인 계약은 유지한다.

## 목적

SDK Active 요청에서 사전 resolve 왕복을 제거할 수 있는 원자적 선택/검색 계약을 만들되 generation/token/ABA 검증을 유지한다.

## 배경과 현재 상태

변경 전 SDK pin_active_selector는 ResolveActiveGeneration을 먼저 보내고 explicit pin+ResolvedActive token으로 다음 query를 구성했다. 현재 지원 Active 경로는 선택 당시 head/token을 query response로 받아 검증한다. 기존 응답 pin만으로 A→B→A token identity를 검증할 수 있다고 가정하지 않는다. 과거 2개 query의 resolve1.5–1.6ms는 비용 위치일 뿐 절감 보장이 아니다.

## 착수 입력

- E3-01/02의 admission decision과 linearization point
- active/pinned/tokened/cursor/RevAtTime/current binding fixtures, malformed response controls

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-sdk/src/client.rs](../../../../crates/quanta-index-sdk/src/client.rs) | pin_active_selector / pin_active_query / resolve_lexical_query_generation / dispatch_query_inner | atomic active-query route가 final selected snapshot을 반환할 때 pre-resolve를 제거하고 exact response binding을 유지한다. | OWNED |
| [crates/quanta-index-sdk/src/binding.rs](../../../../crates/quanta-index-sdk/src/binding.rs) | query generation/domain/response validation | generation/token/domain/variant/row identity를 actual selected response와 검증한다. missing/wrong snapshot 거절. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/selection.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs) | active and joint selection | 한 snapshot에서 track pin/token을 결정하고 acquired view까지 결속한다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/planning.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/planning.rs) | planned lexical/semantic query | 선택 authority와 response materialization 사이에 ambient latest read를 추가하지 않는다. | SHARED |
| [crates/quanta-index-contract/src/ipc/split.rs](../../../../crates/quanta-index-contract/src/ipc/split.rs) | SearchPlaneQueryIpcRequest/Response | 현 DTO에서 selected snapshot/token 증거가 부족할 때만 I0와 현재 wire를 진화시킨다. | SHARED |
| [crates/quanta-index-sdk/src/tests/query_tests.rs](../../../../crates/quanta-index-sdk/src/tests/query_tests.rs) | observed active query RPCs | fixed atomic snapshot expected rows와 RPC count, token conflicts/ABA/continuation tests를 추가한다. | OWNED |
| [benchmarks/retrieval/tests/sdk_roundtrip.rs](../../../../benchmarks/retrieval/tests/sdk_roundtrip.rs) | real-daemon query observation | 단일RPC actual SDK seam과 normalized result parity를 검증한다. | SHARED |
| [crates/quanta-index-contract/src/results/query_responses.rs](../../../../crates/quanta-index-contract/src/results/query_responses.rs) | TextQueryResponse / SymbolQueryResponse / SemanticQueryResponse / HybridQueryResponse / HybridSeedQueryResponse / SearchPlaneHistoryQueryResponse / SearchPlaneRuntimeMetadataQueryResponse | 실제 결과 DTO·custom serializer/strict visitor에서 selected snapshot/token 증거를 결속한다. 현재 generation 필드만으로 token/ABA가 검증된다고 가정하지 않는다. 변경 시 split envelope·SDK binding·fixtures를 같은 계약에서 갱신한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs) | ResolveActiveGeneration / dispatch | 현재 Active resolution과 query dispatch caller를 조사하고 single-query 선택 identity의 생성/전달 지점을 같이 갱신한다. 사전 resolve를 다른 숨은 RPC로 옮기지 않는다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs) | lexical response materialization | 선택된 view의 identity를 final Text response에 직접 결속한다. Symbol/Semantic/Hybrid 등 영향 variant의 생성 지점도 함께 inventory하고 변경한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs) | semantic response materialization | semantic/hybrid single-selection의 actual track/snapshot에서 응답 authority를 구성한다. 서로 다른 catalog 재조회로 track pair를 조합하지 않는다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs) | hybrid response materialization | joint selection/view의 lexical+semantic source identity를 실제 hybrid response와 결속한다. E4 policy/ranking hunk와 I0가 충돌 없이 통합한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/history.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/history.rs) | SearchPlaneHistoryQueryResponse materialization | History Active의 실제 generation/token과 selected read-view identity를 응답에 결속한다. selected variant에 대한 SDK binding과 strict DTO를 함께 검증한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/runtime_metadata.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/runtime_metadata.rs) | SearchPlaneRuntimeMetadataQueryResponse materialization | RuntimeMetadata Active의 selected scope/generation identity를 응답과 같은 권위에서 구성하고 unsupported selector의 typed refusal을 유지한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs) | HybridSeedQueryResponse materialization | 양 track selection과 SeedFusionIdentity/contribution source를 한 snapshot에서 결속하고 hidden pre-resolve/RPC 재도입을 거절한다. | SHARED |

## 실행 단계

1. 현 response의 selected token/snapshot 증거를 확인해 필요한 최소 wire/binding 변경을 확정한다.
2. 한 catalog selection→acquired read-view→result response를 같은 선택 identity로 결속한다.
3. SDK raw Active/ResolvedActive/explicit pin callers를 함께 업데이트하고 old two-step fallback 계층을 장기 유지하지 않는다.
4. ABA, concurrent activate, explicit conflict, cursor continuation, stale generation, ancestor domain, credential/deadline/cancel/reconnect cases를 실행한다.
5. representative fixed fixture→full exact1196 순서로 row/order/count/status/byte/unit parity를 검증하고 E4-06에 source change를 넘긴다.

## 변경 전 RPC 비용 inventory와 현재 검증 범위

아래 추가 요청 수는 변경 전 baseline이다. 현재 SDK는 지원 Active variant를 단일 query로 구성하며 실제 daemon Text/Symbol count는 위 integration 실행에서 확인했다. 나머지 variant의 unit binding을 live roundtrip 증거로 승격하지 않는다.

| 현재 경로 | 현 source에서 확인할 추가 요청 | 단일 선택 변경의 요구 |
| --- | --- | --- |
| Text/Symbol/History/RuntimeMetadata의 Active | 보통 Active resolve 1회+본 query | selected snapshot/token·정확한 row binding을 같은 응답에서 검증 |
| Semantic의 Active+lexical_scope Active | 각 track resolve, 이후 query | lexical/semantic scope가 같은 joint-selection 계약을 충족하는지 검사 |
| Hybrid/HybridSeed의 양 track Active | track별 resolve 2회+query: 총3회 가능 | 두 track을 한 snapshot에서 선택하고 응답 contribution identity까지 결속 |
| Text rev:at.time(...) | resolve_lexical_query_generation의 별도 ResolveLexicalGeneration | ancestor domain/pin을 유지하고 RPC 제거 여부를 별도 판정; 숨은 resolve를 count에서 제외하지 않음 |
| SemanticWorkBoundedV1/Structural | exact generation 필요 또는 Active 미지원 | 현재 typed refusal 유지; 지원 범위를 이 최적화에서 자동 확대하지 않음 |

- source의 pin_active_query 모든 arms와 resolve_lexical_query_generation을 inventory한다. Text2RPC 관측을 모든 route의 보편적 baseline으로 사용하지 않는다.
- contract/results/query_responses.rs의 현재 generation 필드와 strict serializers를 기준으로 selected token 증거를 설계한다. 필요한 영향을 받는 History/RuntimeMetadata/HybridSeed 응답 producer도 같은 변경에서 inventory한다.
- 실제 SDK trace의 request_id+RPC kind로 route별 전후 count를 검증한다. 각 route의 domain/generation/token/variant/rows와 ancestor/cursor semantics가 독립 expected snapshot을 만족해야 해당 one-RPC claim을 발행한다.

## 남은 검증 계획 — current daemon/formal release qualification NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-sdk --lib --all-features --locked`
- `./scripts/cargow test -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked`
- `just rust-public-api; wire/decode 변경 시 just rust-fuzz-smoke; generation selection 변경 시 just rust-profile test-daemon.`
- Negative: response token/variant/domain/row identity mutation, A→B→A stale token 및 cursor rebind 거절.

## 완료 조건

- 지원 inventory의 각 Active variant가 선택된 generation/token과 exact-bound response로 하나의 query RPC를 실행한다. route별 실제 count·선택 계약·parity를 발행하고 unsupported/exact-only variant의 typed refusal을 유지한다.
- atomic linearization·result identity 보존과 runtime tests가 입증되며 speedup은 E4-06에서 따로 판정한다.

## 중단·거절·재개 조건

- server Active 지원만 보고 client pre-resolve부터 삭제하지 않는다. token 증거 없이 fixed-pin query를 Active query와 동등하다고 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
