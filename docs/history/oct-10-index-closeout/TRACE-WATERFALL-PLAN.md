# One-run troubleshooting waterfall: audit and implementation plan

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> 기술 계약/날짜별 설계 근거 참고 문서다. 현재 작업·담당·순서는 [Oct-17 SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 갱신한다. 아래 과거 상태/packet/미구현 판정은 현재 배정표가 아니다. 실행 결과는 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md), legacy의 Accepted 결정은 기존 ADR을 따른다.

Native cost packet D and checkpoint sequencing: [RFC §23.77](../../rfc/oct-10-native-incremental-vnext.md#2377-parallel-delivery-plan-and-exclusive-write-sets-2026-10-13).
Extend existing owners for Native mode and producer/projection/Catalog/WAL/
object/scratch/upkeep/freshness phases. Static instrumentation precedes matched
CP4 execution; no extra runner, no legacy scale promotion.


Date: 2026-10-12. Final contract audit revision: 4.
Status: trace contract and source-specific execution plan. Recorded P00–P12
implementation/capture/viewer results and failures are owned by
[VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md), not duplicated as a whole-current-tree verdict here.
Legacy XL lifecycle, Small capture, offline viewer compatibility and quiet-host
Off/Record performance are separate evidence scopes.

This document owns the trace contract; [TRACE-EXECUTION-PACKETS.md](../../plans/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md)
preserves its historical file assignments. Native lifecycle/backend contracts
follow [Native RFC §1.1](../../rfc/oct-10-native-incremental-vnext.md#11-reading-order-and-decision-boundaries).
Trace P/W identifiers are local to this workstream, not native RFC stages.

The 2026-10-12 remaining-work/ownership observation is preserved in
[Native RFC section 23.55](../../rfc/oct-10-native-incremental-vnext.md#2355--current-source-final-action-register-2026-10-12).
Reuse the existing trace/phase/attempt owners for native measurements; do not
create another telemetry stack or repeat accepted viewer/producer fixtures
without changed inputs. Native failure-prefix coverage, observer overhead and
matched Off/Record under admitted host load still need their applicable
execution. Legacy traces and Small/viewer PASS cannot establish native initial
build, fixed-delta cost, sustained maintenance or RSS. Deployment/trace audits
in progress retain their exact write sets in the execution packets.

목표는 한 번 실행한 결과에서 **어디서 기다렸고, 무엇을 얼마나 처리했으며,
어느 구간에서 실패했는지** 같은 시간축으로 확인하는 것이다. 단계별 소요 시간을
나열하는 로그만으로는 이 목표를 충족하지 못한다. 기존 요청 진단·owner 계측을
확장하고 Chrome trace JSON으로 내보낸다. 별도 telemetry 서버나 자체 UI는 범위 밖이다.

선행 근거: [LEXICAL-QUERY-PLAN.md §10.30](../../plans/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md#1030-병렬-독립-rca와-과도한-수리-범위의-재감사-2026-10-10).
소스 감사 기준은 `HEAD=8f11cad9587aec1668a9fb3f5f1f055337487071`와 당시 공유 dirty
checkout이다. HEAD만으로 실행 소스를 식별하지 않는다. 과거 XL 수치는 §10.30의
`a15d994…` source / `ba1b69c…` binary 실행에 한정한다.


## P10 canonical profile and build boundary (2026-10-11)

The execution packet write set is defined in [TRACE-EXECUTION-PACKETS.md](../../plans/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md#p10-current-canonical-capture-follow-up-2026-10-11).

- `--managed-build` builds `scale_matrix` through the canonical `scripts/cargow`
  release recipe into a new external target, verifies the exact Cargo artifact,
  binds tracked source, dependency source, Cargo config and tool epochs, and runs
  the copied executable in the same invocation. Imported build receipts cannot
  qualify a caller-supplied binary. This is observed local build custody; compiler
  libraries, system libraries and same-UID/kernel isolation are excluded.
- A canonical-only `canonical-profile.json` consumes validated product artifacts
  and complete typed trace metrics. Every measured phase is covered exactly once;
  seal sync/BM25 evidence must be present. Daemon request roots resolve to phases
  only through unambiguous diagnostic attempt links and actual parent ancestry,
  with computation owners checked inside the client attempt. Server post-write
  root tails may finish later and remain correlation, not parent nesting.
  Missing metrics remain missing.
- Harness seal/upload/activation now pass their actual phase context and negotiate
  existing IPC tokens. CAS/receipt authority and IPC admission remain unchanged.
  Wire-size preview is explicitly the plain product envelope; diagnostic token
  and handshake bytes are observer overhead and still undergo actual IPC admission.
- `VERIFIED_DIAGNOSTIC` requires both observed managed build and canonical profile.
  Legacy marker timer equivalence and quiet-host Off/Record comparison stay
  `NOT_RUN`; nested clocks are not added. Unsupported process I/O stays unavailable.
  Old caller-supplied binaries retain `build_binding=UNPROVEN`.
- The historical XL artifact has query attempt links but no ingest attempt links.
  It cannot be retroactively promoted to a canonical seal profile. Its product
  success and lossless trace remain valid in their original scope.

Example for a clean committed source, with output outside the checkout:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.causal_cost_capture --cwd "$SOURCE" --source-revision "$REVISION" --managed-build --tier small --seed 1 --disk-observation boundary-only --max-seconds 180 --out-root "$NEW_OUTPUT" --timeline
```

This interface is implemented. A full fresh release build through this mode is
not inferred from fixture tests; see the separate executed scopes in VALIDATION.md.

The 2026-10-12 fresh managed Small execution is observed local diagnostic proof.
It does not qualify legacy timer equivalence, new Large/XL or performance. The
corrected viewer is an offline export of that same immutable raw trace. Actual
Perfetto UI parsing verifies 52 causal flows. Hex-u64 V2 IDs and zero-duration
points at the observed client start prevent dropped or backwards arrows;
request/phase durations and raw trace authority are unchanged.

## 1. 추가 감사: 확인된 공백과 한계

| 확인된 사실 | 운영상 결과 | 수정 owner |
| --- | --- | --- |
| `causal_profile.rs::timed_sync/timed_work`는 종료 후 label·duration만 출력 | 시작 시점·부모·동시 작업·실패 중인 호출을 복원할 수 없음 | core 진단 계약 + lexical producer |
| `causal_cost_profile.py::parse_trace`는 단일 active phase만 허용하고 `QI_INGEST_TRACE_V1`을 읽지 않음 | 현재 세부 로그는 공식 분석의 waterfall 입력이 아님 | benchmark parser/exporter |
| 성공한 `SearchCorpusIngestObservation`에 owner 시간은 있으나 오류의 `?` 반환에는 없음 | 느린 성공보다 장애 실행의 관측이 부족 | search-plane request scope |
| IPC ring은 process instance·request ID·손실 정보를 갖지만 1,024건 메모리 보관 | 프로세스 kill 후 회수 불가; polling으로 내구성을 만들 수 없음 | 기존 IPC sink의 stream projection |
| 서버는 같은 connection에서 wire request ID 재사용을 금지하지 않음 | process/plane/connection/request 조합도 attempt 유일키가 아님 | admitted envelope마다 root span 생성 |
| harness readiness retry가 같은 envelope를 새 연결에서 재전송 | client attempt와 server span을 request ID·시간 근접성으로 정확히 연결할 수 없음 | IPC diagnostic attempt link |
| `PhaseSampler` setup은 phase start 앞, teardown은 end 뒤; 최종 결과는 tier 완료 후 기록 | observer 조사 비용이 숨고 실패 실행에서는 상태도 유실 | harness + capture |
| `same_process_reopen` 하나가 stop/join/start/boot를 감쌈 | 900초 timeout의 정확한 정지 지점은 미확정 | harness + searchd lifecycle |
| 최초 `full_ingest`가 lazy daemon boot도 포함 | 초기 색인 비용과 시작 비용을 구분하지 못함 | harness의 `ensure_driver/start_driver` |
| Text query 응답의 `explanation`을 harness가 버림 | 서버가 만든 stage timing과 CodeSearch 작업량이 벤치에서 사라짐 | harness query adapter |
| sync마다 env 조회와 stderr 출력; XL 674,091줄 / 48,750,141B | 출력 비용은 leaf sync 시간 밖이지만 상위 wall 안에 들어감 | lexical aggregate counters |
| phase start marker의 쓰기 실패가 benchmark work 실행을 막음 | 진단 채널 오류와 제품 작업 결과가 결합됨 | harness observer boundary |

`quanta-index-lq-obs::SpanEvent`는 타입과 테스트가 있으나 현재 runtime producer 연결은
확인되지 않았다. 이것을 설치된 tracing으로 간주하지 않는다. query execution trace는
엔진 호출·기여의 정확성 기록이며 시간축 trace를 대신하지 않는다.

**이번에 확정하지 못한 것:** retention 잔여 약 44–48초의 hash/read/SQLite/inode별
몫, reopen 내부 정지 지점, trace 출력으로 증가한 wall time. 기존 로그에 없는 시각을
추정해 waterfall을 그리지 않는다. 새 계측 전에는 이 세 항목을 UNKNOWN으로 유지한다.

## 2. 사용자가 받는 결과

기존 `causal_cost_capture`에 opt-in `--timeline` export를 추가한다. capture는 원래부터
진단 실행이며, 전환 후에는 옵션 유무와 무관하게 하나의 canonical structured recorder를
사용한다. `--timeline`은 viewer 자동 생성 여부를 고른다. 아래는 **구현 예정 인터페이스**다.
기존 source/bin/tier/seed/caps 인자는 그대로 필요하며 임의의 기본값으로 바꾸지 않는다.

```text
python3 -m tools.benchmark.retrieval.causal_cost_capture \
  <existing source, binary, tier, seed and cap arguments> \
  --timeline --out-root /private/tmp/<new-capture-directory>
```

- 시작한 capture는 성공·오류·timeout 모두에서 같은 exporter를 자동 시도한다.
  preflight 거절, 출력 디스크 장애, 유효 event가 없는 stream에는 viewer 파일을 보장하지
  않는다. 가능한 raw/실행 오류를 보존하고 `viewer_available=false`와 이유를 알린다.
- `causal-waterfall.json`: 한 번 열면 되는 viewer 파일. root / request / storage /
  lifecycle / observer / trace-health track, 오류 지점, 작업량을 포함한다.
- `causal-timeline.jsonl`: 실행 중 기록한 원본. stderr의 임의 문장과 분리한다.
- `timeline-status.json`: clock/손실/미완료/잘린 행/수집 범위와 source binding 상태.
  `execution.json`의 제품 실행·timeout 상태를 대체하지 않는다.
- `timeline-summary.json`: 가장 긴 완료 구간, 명시적으로 잰 lock wait, 단계별 작업량,
  미분류 wall, 끝을 받지 못한 구간. viewer 내부 metadata에도 핵심 상태를 넣는다.
- 동일 원본을 **재실행 없이** offline export할 수 있다. 성공용 `causal-profile.json`은
  기존 qualification 조건을 만족할 때만 발급한다. 부분 waterfall 생성은 PASS가 아니다.

Perfetto는 Chrome JSON의 slice, counter, flow를 직접 표시한다.
[공식 import 문서](https://perfetto.dev/docs/getting-started/other-formats)를 따른다.
별도 웹 서비스·OTLP collector 설치를 이 기능의 선행 조건으로 만들지 않는다.

## 3. 먼저 고정할 인터페이스와 의미

### 3.1 하나의 진단 context, 기존 요청 authority 유지

1. core에 고정 stage/counter enum, span handle, clock/sink port를 둔다. core는
   IPC 구현·파일 포맷·Perfetto에 의존하지 않는다. 외부 raw format에는 schema version을 둔다.
2. `RequestBudgetV1`의 `RequestStageDiagnosticPortV1`와 IPC `ProviderEventBridgeV1`를
   이 계약에 맞춰 함께 변경한다. 기존 요청 envelope ID를 그대로 전달한다. 기존
   provider/window 이벤트를 새 계측 옆에서 중복 생성하지 않고 같은 이벤트를 투영한다.
   기존 port의 transport 전용 의미를 명시적으로 개정하고 공통 context의 root를
   `Request` / `Operation`으로 구분한다. `RequestEventV1`에 가짜 요청을 넣어 공용화하지 않는다.
3. lexical `build_batch_with_owner` 및 retention의 관련 port에 명시적 optional
   diagnostic context를 전달한다. 전역 env/TLS에서 현재 요청을 추측하지 않는다.
   진단 때문에 deadline·cancel·usage·ack authority를 생성하거나 변경하지 않는다.
4. boot, maintenance, harness root는 요청과 독립된 동일 계약의 context를 사용한다.
   요청 ID는 absent로 둔다. 이들을 위해 가짜 `RequestBudgetV1`을 만들지 않는다.
5. composition root가 한 선택적 sink를 주입한다. IPC ring은 현재 bounded 조회 용도를
   유지하고 stream은 같은 owner event의 출력 경로가 된다. 모든 세부 이벤트를 ring에
   넣어 transport 실패 이력을 밀어내지 않는다. 수집 범위는 stream metadata에 명시한다.
6. lifecycle/context scope, 단계 enum, raw schema와 exporter fixture를 먼저 합의한다.
   현재 내부 IR의 V1/V2 이중 경로 및 서로 다른 타이머 구현을 남기지 않는다.

**송신 시도 연결은 W0의 필수 seam이다.** trace-enabled IPC 호출마다 client attempt token을
새로 만들고 선택적인 고정 크기 diagnostic envelope field로 전달한다. readiness retry도
다른 token을 사용한다. server는 token을 link 속성으로 기록하고 자기 root span은 별도로
발급한다. envelope/SDK/IPC/harness의 encode·decode·validation을 함께 변경한다. request ID,
batch digest, idempotency/replay identity, deadline, usage/ack의 의미에는 포함하지 않는다.
클라이언트 제공 token은 신뢰·권한·span 소유권의 근거가 아니다. 아래 wire 경계를 같이 고정한다.

- 현재 query/control decoder는 unknown field를 무시하지만 ingest decoder는 거부한다.
  1차 적용은 **동일 빌드 harness 또는 동시 교체한 SDK/server bundle**로 제한한다.
  지원이 확인되지 않은 peer에는 field를 보내지 않고 unlinked로 둔다. 이 기능을 위해
  별도 capability 협상 프로토콜을 만들지 않는다. 구버전 ingest에 보내도 안전하다고 가정하지 않는다.
- token absent는 `null`이 아니라 필드 생략이다. 기존 `{request_id,payload}` JSON/CBOR
  shape를 보존한다. query/control/ingest request의 수동 serde를 각각 수정하며 response용
  공용 helper에 잘못 추가하지 않는다. `RequestEnvelope::into_parts/validated` 반환 경계도
  token을 보존하도록 함께 변경한다. token은 envelope 생성 시점이 아닌 **송신 시도마다** 주입한다.
- token 생성·계측 실패는 field absent로 처리한다. 유효한 프레임 내 token 값/중복의
  진단 검증 실패는 token만 버리고 각각 `invalid_attempt_token` 또는
  `duplicate_attempt_token` health와 `unlinked_attempt`로 남긴다. 기존 request/payload,
  frame 구조·깊이·크기 검증은 완화하지 않는다. 임의의 큰 값을 추가 할당해 보관하지 않는다.
  이 진단 필드만의 예외를 `ipc/ingest.rs`의 strict-serde 문서와 테스트에 명시한다.
  visitor는 잘못된 token 값만 bounded하게 소비하며 frame 문법 오류를 삼키지 않는다.
- diagnostic field 때문에 기존 codec의 wire/decoded cap을 초과하면 송신 전에 token을
  생략한다. token 추가에 따른 **transport** frame digest는 기존 codec이 다시 계산하며,
  **product** batch/source-event payload digest는 동일해야 한다. 일반 encode/decode 오류를
  telemetry 오류로 오인해 성공 처리하지 않는다.
- exporter는 같은 capture에서 token이 client attempt 하나와 server ingress 하나에
  유일하게 대응할 때만 flow를 만든다. 중복·미도착·위조 가능성이 있는 대응은
  ambiguous/unlinked로 보존한다. request ID나 시간 근접성으로 보정하지 않는다.

### 3.2 식별자와 이벤트

| 필드 | 계약 |
| --- | --- |
| `capture_id`, `stream_id`, `clock_domain` | 수집 실행·writer·시계의 식별자. 제품 generation이나 durable journal sequence가 아님 |
| `process_instance`, `plane`, `connection_id`, `request_id` | 기존 admitted 요청의 상관 정보. request ID 단독으로 join하지 않음 |
| `span_id`, `parent_span_id`, `links` | span ID는 stream 내 유일. 요청 attempt도 고유 root span으로 구분. 동기 실행 흐름은 실제 caller context를 parent로 전달하고, 비동기 fork/join은 명시적 link로 기록 |
| `output_seq`, `ts_ns` | 단일 writer의 기록 순서와 producer의 공통 단조시계 offset. 기록 순서는 인과 관계나 손실 증명이 아님 |
| `lane_id`, `stage` | 실행 논리 lane과 고정 enum. task가 thread를 이동해도 scope를 보존 |
| `kind`, `outcome`, `error_code` | begin/end/counter/health/link; 성공은 explicit finish에서만. 오류는 기존 typed code |
| `counters` | 명시된 단위·집계 범위의 고정 key. unavailable과 0을 구분 |

`request_id`의 유일성을 wire가 보장한다고 가정하지 않는다. 같은 ID의 재시도·재접속을
서로 다른 span으로 유지한다. 새로운 span ID는 **진단 식별자**이며 제품 요청 ID를
대체하지 않는다. counter overflow/ID exhaustion은 trace invalidity이며 wrapping하지 않는다.
root span은 envelope 검증 직후, shutdown/queue refusal 전에 생성한다. 검증 이전의
frame/decode/zero-ID 거절은 connection-level ingress outcome으로 남긴다. IPC ring의
insertion sequence를 attempt/span ID로 재사용하지 않는다.
span 종료/aggregate publish는 한 번만 수행한다. child worker의 context 전달·종료 주체를
명시하고 lexical guard를 무심코 `await` 전체에 걸쳐 thread-local stack으로 유지하지 않는다.
`same_process_reopen`은 phase context를 `RuntimeStop`, `RuntimeJoin`, `RuntimeBoot`까지 명시적으로
전달한다. capture/clock 공유와 시간 겹침은 부모 관계를 대신하지 않는다. standalone boot는
기존 process-local operation root를 유지하고, 선택적 parent가 있으면 해당 parent의 child로 기록한다.
선택적 parent가 유효하지만 child span/lease 생성이 실패하면 제품 실행은 계속하고 해당 runtime
진단 주입을 생략한다. 더 바깥 capture root로 fallback해 잘못된 인과를 만들지 않는다.

새 canonical raw stream은 `causal-timeline.schema.json`의 **schema_version 2**를 쓴다.
v2는 닫힌 sync-label aggregate, adoption flush reason/occupancy aggregate, 인증된 closure
참조·edge·byte counters를 추가한다. offline parser는 기존 v1 event vocabulary를 계속
읽되 v1 stream에서 v2 aggregate event가 나오면 `INVALID`로 거부한다. writer는 v2만 생성하며,
v1 header/footer와 v2 event를 섞지 않는다.

### 3.3 시계: 같은 그림에 잘못된 시간축을 섞지 않는다

- 1차 구현은 현재 **한 OS 프로세스의 scale harness와 daemon threads**를 모두 포함한다.
  최상위 capture owner가 monotonic clock origin·span allocator·writer를 소유하고
  daemon start/reopen, worker, observer에 clone-inject한다. same-process reopen은 phase parent context도
  명시적으로 전달해 stop/join/boot를 해당 phase의 descendant로 기록한다. runtime 재구성 때 새 writer나
  allocator를 만들지 않는다. daemon incarnation이 바뀌어도 capture의 stream과 clock domain은 유지한다.
  현재 request-relative elapsed는 보존하되, exporter용 offset은 이 공통 origin에서 잰다.
- standalone searchd에도 부팅 초기에 같은 process-local clock을 한 번 만든다.
  daemon 내부 요청·background·boot는 하나의 waterfall로 진단할 수 있다.
- Python outer timeout clock이나 별도 SDK 프로세스의 `Instant`는 같은 origin이 아니다.
  초기 구현에서 Python은 종료 상태와 outer elapsed를 metadata로 제공한다. 근거 없이
  daemon track의 특정 시각에 kill 선을 긋지 않는다. harness 내부 client wire 구간은
  공유 clock으로 기록하고 admitted request와 link한다.
- 별도 프로세스 client 전체를 한 축에 올리는 확장은 clock mapping과 uncertainty를
  검증한 뒤 허용한다. mapping이 없으면 독립 clock track임을 명시하고 교차 latency와
  critical path 계산을 금지한다. wall-clock timestamp만으로 정렬하지 않는다.
  [Perfetto clock-sync 원칙](https://perfetto.dev/docs/concepts/clock-sync)을 참고한다.

### 3.4 wall time, 작업량, 부분 실패

- 완료 span의 inclusive wall은 `end - begin`. 직접 child interval을 부모 시간 범위와
  교차시킨 뒤 **합집합**을 빼서 `unattributed_wall`을 구한다. 음수가 되어서는 안 된다.
  부모 밖으로 지속하는 async 작업은 strict child가 아닌 관련 작업 link로 표현한다.
  합산한 child 시간이나 CPU self-time이 아니다.
  concurrent overlap은 별도 값으로 보이며 parent+child를 다시 합하지 않는다.
- sync/read/hash/SQLite의 호출 횟수·누적시간은 **work counter**다. 시간 위치를
  기록하지 않은 225초 합계에 가짜 continuous bar를 만들지 않는다. parallel work의
  누적 시간은 parent wall보다 클 수 있다. 인과 link 없이 critical path를 추론하지 않는다.
- lock wait는 acquire 직전 begin부터 실제 획득까지, hold는 획득부터 release까지 잰다.
  cancellation/refusal 전 대기도 기록한다. 관측 sink의 대기는 제품 lock 밖에서 처리한다.
- error/cancel/panic은 각 outcome을 기록한다. Drop without finish는 aborted이며 성공이
  아니다. SIGKILL은 Drop을 실행하지 않으므로 end를 합성하지 않는다.
- end를 받지 못한 span은 `END_NOT_OBSERVED`. 이벤트 손실이 있으면 실행이 계속됐는지도
  불명이다. 그림의 점선 extent는 같은 clock의 마지막 관측 시각까지의 표시용 범위일 뿐
  실제 duration이나 실행 시간의 하한이 아니다. 완료 aggregate에서 제외한다.
- product outcome, trace completeness(`COMPLETE/PARTIAL/INVALID`), collection coverage,
  performance qualification을 별도 축으로 둔다. 실패한 작업의 trace 자체는 COMPLETE일 수 있다.

## 4. 필수 계측 위치와 작업량

stage 이름은 아래 경계에서 고정한다. 기존 owner 타이머와 관측값을 재사용한다.

| 경로 / 소유 파일 | 필수 span | 단계 종료 시 집계할 값 |
| --- | --- | --- |
| IPC `server.rs` / `counters.rs` | admission, queue wait, dispatch, backend, encode/write, cancellation | route, outcome, existing provider/window ID; queue wait와 backend elapsed 분리 |
| `ingest_dispatcher/search_corpus.rs` | operation stripe wait/hold, preflight, semantic, lexical, finalize, activate | documents/changes; replay/finalize-only 여부; existing owner stage times |
| semantic `build.rs` | prepare, stream/delete/append, embedding, seal/promotion | existing semantic owner counts/durations; 성공 report 이전의 오류에도 begin/end 및 partial counters 보존 |
| 같은 파일의 finalize | source transaction 1, fence, retention record, source transaction 2 | transaction count; generation/pair correlation |
| `readiness/search_corpus_history.rs` / retention policy | pair/root lock wait/hold, root snapshot, plan/measure, durable history write, reap | retained pairs, measure calls, removed roots |
| `search_corpus_retention.rs` | lexical pair measure, semantic pair measure, lexical physical track, semantic physical track | logical/physical bytes, examined files/inodes, unavailable reason |
| lexical `adapter_lifecycle.rs`, `lexical_gc.rs` | generation inode scan, root authenticate, unique closure | unique refs, dependency edges, authenticated bytes, SQLite batches/rows; read/hash/SQLite accumulated time |
| `reclaim_area.rs` | physical inode census | dirs/files/unique inodes/bytes; traversal error |
| Native/File producer | prepare, mutate, compact, maps, seal/publish | new/reused objects by kind, compacted segments/docs/bytes, postings/map mutations, live/dead counts |
| `lexical_store/store.rs` | adoption/commit group aggregate per parent stage | group count; flush reason(refs/bytes/explicit); refs/bytes occupancy buckets; sync count/sum/max/failure by closed label |
| query dispatcher, lexical `searcher/code_search.rs`, `file_authority/paged/reader.rs`, store | parse/plan, read view, candidate, exact verify, rank/project | admitted/read/byte limits, actual consumed reads/bytes, first refusing guard, candidate/verified/matched counts, bounded regex plan shape; owner error-path snapshot |
| harness `query_text_with_pin` | client encode/connect/write/decode, each fixed literal/regex request, correctness probe groups | preserve existing explanation/execution stats; distinguish wire wall from server wall |
| harness `scale.rs`, `bin/scale_matrix.rs` | full/delta/noop/delete work+activate, first/warm queries, repo/File probes, reopen, cleanup; binary의 artifact writing | existing owner clock reconciliation, invocation count, outcome |
| harness `stop_driver/start_driver` | signal, daemon join, observer drop, socket cleanup; config/runtime/spawn/socket wait | join target and outcome; timeout scope |
| searchd `runtime.rs`, `maintenance.rs` | lease/catalog, history restore, reclaim, inventory, active pair proof, initial maintenance scan; worker stop/join | objects/files/bytes; initial vs periodic maintenance; active-generation correlation |
| harness `PhaseSampler` | observer setup/probe/teardown on separate lane | probe duration/status; disk/RSS/CPU/I/O available or unavailable; sampling mode |

개별 object span은 만들지 않는다. 긴 closure의 진행 정보가 필요하면 bounded counter
checkpoint를 단계 경계 또는 낮은 빈도로 출력한다. raw path, source body, query text,
embedding, peer secret은 기록하지 않는다. fixture/query ordinal과 고정 shape로 충분하다.
correlation 값은 trace argument이며 process metric의 고카디널리티 label로 승격하지 않는다.

lazy boot를 기존 full_ingest 밖으로 옮겨 측정값을 개선하지 않는다. 기존 outer timing
범위 안에 하위 span을 추가한다. benchmark semantics를 바꾸려면 별도 명시적 변경이다.
현재 observer 작업 역시 수행 범위를 바꾸지 않고 먼저 비용을 드러낸다.

## 5. 수집 경로: 실패에도 남고 제품을 막지 않게

- opt-in 설정은 boot/capture에서 한 번 해석한다. disabled 경로는 per-object env 조회,
  format, I/O, clock read를 하지 않는다. enabled micro-counter는 owner-local 고정 배열에
  누적한다. 요청 간 공유 mutex나 개별 객체 로그를 루프에 추가하지 않는다.
- 저빈도 begin/end와 aggregate만 bounded nonblocking queue에 넣고 **전용 writer 하나**가
  JSONL로 직렬화한다. 제품 lock 안에서 file write/flush/join을 수행하지 않는다.
  stage begin은 종료를 기다리지 않고 writer에 즉시 전달한다. writer는 wake 시 배출하고
  짧은 flush 주기를 갖되 event마다 fsync하지 않는다.
- `output_seq`는 **writer가 dequeue 후** 발급한다. enqueue 전 global 번호는 producer
  preemption으로 파일 순서와 역전되므로 사용하지 않는다. 출력 번호와 IPC ring sequence는
  범위가 다르다. producer 손실은 번호 gap 대신 `attempted/accepted/dropped`와 writer의
  `written/unwritten`을 구분해 집계한다. counter의 단위는 header/footer를 제외한 data event다.
- 정상 close는 `Recording → Closing → Closed`다. 새 emit admission을 닫고 이미 진입한
  enqueue가 모두 해결된 뒤 accepted events를 drain한다. 그 다음에만 최종 accounting과
  footer를 기록·flush한다. `attempted = accepted + dropped`,
  `accepted = written + unwritten`을 검증하며 COMPLETE는 dropped=unwritten=0이어야 한다.
  counter/ID overflow는 명시적 invalidity다. rolling health snapshot을 최종 합계로 쓰지 않는다.
- close owner는 `scale_matrix.rs`의 artifact write/실패 처리까지 살아 있다. runtime
  재오픈은 close가 아니다. producer worker 정지와 in-flight emit 장벽 전에는 완료 footer를
  쓰지 않는다. **in-flight=0만으로 충분하지 않다.** emit 가능한 producer context/lease가
  모두 종료됐거나 앞으로 emit할 수 없다는 scope 소유권 증명이 있어야 COMPLETE를 발급한다.
  잠든 context를 보관한 채 close하는 경우는 미완료다. Closing 중 늦은 emit은 drop/위반으로
  집계하고 PARTIAL로 남긴다. COMPLETE footer 이후에는 emit 자체가 소유권상 불가능해야 한다.
  footer 뒤 data event를 쓰지 않으며, 사후 위반을 메모리에만 기록해 완료 footer를 정당화하지 않는다.
- queue full, file cap, write error의 health 기록도 best effort다. footer/producer-side
  최종 accounting을 회수하지 못하면 원인·손실 수는 UNKNOWN, trace는 PARTIAL로 둔다.
  writer 완료를 deadline까지만 기다린다. 2초 drain 제한을 blocking `JoinHandle::join`으로
  구현하지 않는다. 제한 초과는 footer/완료 확인 없음이며 제품 종료를 무한 대기시키지 않는다.
- kill 직전 queued/tail 이벤트와 OS crash의
  디스크 내구성까지 보장하지 않는다. footer가 없거나 sink 상태를 회수하지 못하면
  COMPLETE를 발급하지 않는다. parent capture는 이미 기록된 완전한 행을 보존한다.
- source/stream SHA와 export는 producer 프로세스 종료 또는 writer의 실제 정지 뒤에
  확정한다. 아직 파일을 쓸 수 있는 detached writer를 두고 immutable raw를 주장하지 않는다.
  standalone 프로세스에서 writer 정지를 확인하지 못하면 final export는 미완료로 남긴다.
  live snapshot export나 반복 trace session 관리까지 1차 범위를 확장하지 않는다.
- trace open 실패는 요청한 **수집 준비 실패**로 실행 전에 알린다. 시작 후 sink 실패는
  제품 성공·오류를 바꾸지 않고 진단 실패를 따로 남긴다. primary product error를
  exporter/cleanup 오류로 덮지 않는다. capture 오류 exit와 producer exit도 구분한다.
- 기존 stdout/stderr도 보존한다. `QUANTA_INDEX_CAUSAL_PROFILE_V1`은 capture의 child env에서
  제거하고, parent env에 남아 있어도 새 recorder와 함께 활성화되지 않도록 한다.
  sync 외 BM25 census/live/boundary, semantic commitment, ingest 및 phase marker까지
  같은 typed recorder로 전환한다. stderr marker 출력 실패가 제품 작업을 막는 경로도 제거한다.
- env 제거에 묶여 꺼지는 `PhaseSampler::process_write_io_snapshot`은 명시적 capture
  설정으로 이관한다. 기존 profile의 필수 sync/BM25/phase coverage와 owner duration,
  process I/O 관측을 canonical event에서 재구성하고 strict replay/tests를 함께 전환한다.
  producer와 consumer를 한 번에 전환하며 최종 상태에 legacy/internal 이중 경로를 남기지 않는다.
  이 동등성이 검증되기 전에는 partial viewer만 검증할 수 있고 `causal-profile.json`이나
  `VERIFIED_DIAGNOSTIC`은 발급하지 않는다. 이전 raw artifact는 변경·가짜 변환하지 않는다.
- 기존 profile v1의 `trace_sha256`은 stderr의 SHA다. 새 canonical JSONL을 입력으로
  사용하는 profile은 schema를 올리고 `timeline_sha256`과 `stderr_sha256`을 명시한다.
  동일 필드의 provenance를 조용히 바꾸지 않는다. 실행 record의 profile/schema 참조와
  hash 검증도 함께 갱신한다. 과거 profile v1 artifact를 덮어쓰지 않는다.
- 수집 출력은 checkout 밖, 한 capture만 쓰는 새 디렉터리에 둔다. source/binary/input/
  caps와 stream SHA는 기존 capture 경계에서 기록한다. clean HEAD + binary SHA만으로
  build provenance를 증명하지 않는다. 기존 build receipt가 없으면 binding은 UNPROVEN이다.

**초기 자원 목표이며 실측값이 아님:** queue 최대 4MiB, raw stream 최대 16MiB /
32,768 records, flush 목표 250ms, normal drain 상한 2초. cap 초과는 PARTIAL로 표시하고
footer/health용 여유를 예약한다. scheduler/filesystem stall에서 flush 지연 상한을 보장하지 않는다.
W0에서 event 최대 encoded bytes, enum별 counter/links 상한, checkpoint 빈도, queue 실제
resident 상한을 고정한다. 가변 길이 무제한 payload를 queue 앞에서 할당하지 않는다.
정상 XL의 계획된 최대 request/retry/window/stage 수로 record·byte 상한을 계산하고,
그 범위를 넘는 실행은 손실 없는 capture로 광고하지 않는다. 기본 수치가 검증된 capacity라는 뜻은 아니다.
파일/객체 수에 비례하는 로그를 금지하고, 허용한 요청·stage·checkpoint 수에 비례하도록 검증한다.

## 6. exporter의 정확성 계약

- unknown schema/kind/stage, duplicate span/end, end-before-begin, parent cycle,
  다른 capture의 혼입, counter overflow, timestamp 범위 오류를 검사한다.
  schema가 깨진 내부 행은 INVALID. **마지막 불완전한 행만** 잘라 유효 prefix를 복원한다.
- concurrent producer의 enqueue 순서와 timestamp 순서는 다를 수 있다. writer `output_seq`
  연속성과 각 span의 시간 일관성을 각각 검사한다. `end-before-begin`은 span을 연결한 뒤
  timestamp가 역전된 경우이며 파일 행의 순서만으로 판정하지 않는다. 서로 다른 thread 이벤트의 timestamp가
  파일 순서대로 증가하지 않는다는 이유만으로 INVALID로 처리하지 않는다.
- parent 없는 child/end-only는 loss가 명시됐거나 capture 경계가 잘린 경우 PARTIAL로
  보존하되 연결을 추측하지 않는다. 완전한 stream에서의 모순은 INVALID다. 수집되지
  않은 구간을 NOT_RUN이라고 주장하지 않는다. 실제 invocation/outcome 증거를 사용한다.
- COMPLETE에는 header/footer, producer 범위, loss=0, 유효 parent/terminal 관계가
  필요하다. terminal이 없는 span이 있으면 PARTIAL 또는 계약 위반이며 성공 duration을
  만들지 않는다. `output_seq` 연속성만으로 producer loss=0을 추론하지 않는다.
  footer 합계와 실제 data record 수를 대조한다. 외부 raw를 바꿔 성공 report로 승격하지 않는다.
- Chrome JSON은 완료 span에 `X`, 상태/실패에 instant, work에 counter/args, 인과에
  flow를 사용한다. 원본 ns를 보존하고 viewer 단위로만 변환한다. integer ID의 JS 정밀도
  손실을 피하도록 ID encoding을 고정한다.
- 논리 lifetime은 CPU thread 점유가 아니다. crossing spans는 별도 logical track에
  배치하고 부모·worker 연결은 flow/metadata로 표시한다. 같은 track의 잘못된 nesting으로
  Perfetto overflow track을 만들지 않는다. INVALID trace도 raw와 오류 설명은 보존한다.

## 7. 구현 순서와 병렬 write set

파일 수정권·새 파일·금지 파일·허용 hunk·handoff·검증 명령은
[TRACE-EXECUTION-PACKETS.md](../../plans/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md)의 P00–P09가 실행 기준이다.
아래 wave 표는 순서 요약이며 directory 전체 수정권을 부여하지 않는다.

| Wave | 작업 / 소유 범위 | 종료 조건 |
| --- | --- | --- |
| W0, 직렬 | core diagnostic contract, stage/counter registry, clock/identity, optional IPC attempt field, raw schema + fixed fixtures | §3/5/6를 타입·테스트 계약으로 고정; sink fail-open 및 attempt correlation 합의 |
| W1-A, 병렬 | IPC/SDK bridge·codec/ring projection, searchd recorder/file-writer adapter·composition root, runtime/maintenance lifecycle | request/boot/maintenance가 같은 context를 사용; close 장벽·정상/실패/재시작 연결 |
| W1-B, 병렬 | lexical producer/searcher/reader/store, semantic build, retention/search-plane spans + aggregates | 전수 조사·lock·compaction·sync 비용 분리; 루프 출력 제거; 성공 report 이전 실패 계수 보존 |
| W1-C, 병렬 | `harness.rs`/`scale.rs`/`scale_matrix.rs`의 lifecycle·observer·query explanation, Python capture/parser/exporter | 한 실행으로 partial 포함 viewer 생성; 기존 timing 범위와 strict qualification 보존 |
| W2, 직렬 통합 | 각 lane의 독립 oracle 및 format/clock/failure 교차 검토, 기존 consumer와 schema 동시 전환 | 같은 소스에서 모든 acceptance 테스트 통과; field/ID 의미 중복 없음 |
| W3, 직렬 실행 | 좁은 lifecycle/kill 검증 → profile on/off Large → 필요 시 XL 한 번 | 관측 부담·실패 복구·실제 waterfall을 확인한 뒤 병목 수리에 착수 |

같은 파일의 수정권은 한 lane에 둔다. 공통 contract는 W0 후 동결하고 변경 제안은 root가
통합한다. 현재 공유 checkout의 다른 full-suite/build 작업을 중복 실행하지 않는다.
root/interface와 module wiring을 검토해 실행할 준비가 된 뒤 runtime을 수행한다.

## 8. 수용 기준: 녹색으로 오인하지 않게

1. **고정 oracle:** fake clock으로 sequential/nested/overlapping interval의 inclusive,
   union, residual을 손계산 fixture와 대조한다. 부모 밖 관련 작업의 제외·교차 구간과
   residual의 비음수 조건을 검사한다. clock domain 혼합과 unit/ID overflow를 거절한다.
2. **correlation:** 동시 요청, 동일 wire ID 재사용, connection 교체, worker fork/join,
   readiness retry, daemon reopen에서 서로 다른 attempt가 합쳐지지 않는다. 부모와 flow가
   실제 caller와 일치한다. diagnostic token 유무가 batch digest·replay·usage를 바꾸지 않는다.
   absent field의 기존 wire shape, JSON/CBOR malformed/duplicate token의 unlinked 처리,
   unknown peer의 field 미송신, 크기 경계에서 token 생략, 모호한 token의 flow 거절을 검사한다.
3. **lifecycle fault:** stop join/history restore/reclaim/active proof/initial maintenance
   각 경계에서 통제된 block·error·cancel을 주입한다. start와 이미 배출된 선행 결과가
   남고 미완료를 성공으로 바꾸지 않는다. 큰 corpus 없이 좁은 테스트로 먼저 확인한다.
4. **sink fault:** queue overflow, unwritable/full output, writer stall과 exporter 오류가
   제품 return/ack·usage settlement를 바꾸지 않는지 확인한다. truncated tail, missing
   footer, repeated/end-only record, panic도 검사한다. 별도 SIGKILL 시험에서는 남은
   prefix의 무결성·미완료 표시를 검증한다. kill 뒤 없어진 return/ack를 만들어 비교하지
   않으며, 관측되지 않은 제품 terminal outcome은 UNKNOWN이다.
   timestamp 후 enqueue 전 preemption, close와 in-flight enqueue 경쟁, 늦은 context,
   dormant context를 보유한 close, accepted-but-unwritten tail, drain timeout, footer 뒤 쓰기 금지를 검증한다. sink fault가
   제품 오류를 **반환하도록 바꾸지 않는다**는 계약이며 계측의 시간·자원 교란이 0이라는 뜻은 아니다.
5. **coverage:** full/delta/noop/delete, activate, correctness queries, lazy boot/reopen,
   observer setup/teardown, artifact write가 한 파일에 나타난다. success explanation과
   failed query guard/counter가 요청 root에 연결된다. 미관측 값을 0으로 만들지 않는다.
6. **timing 보존:** 기존 owner duration reconciliation과 새 interval을 같은 경계로 대조한다.
   observer·logger 시간을 제거한 것처럼 기존 기준을 조작하지 않는다. 현재의 producer wall과
   diagnostic-on 수치는 성능 개선 예측으로 쓰지 않는다.
   sync/BM25/phase/process-I/O 필수 계측의 전환 매핑과 strict profile rejection fixture를
   확인한다. legacy env on/off와 무관하게 새 recorder 외 출력 경로가 켜지지 않아야 한다.
   profile schema와 원본 종류별 digest를 대조하고, stderr 해시를 timeline 해시로 잘못
   사용한 fixture는 거절한다.
7. **overhead:** 같은 source/bin/input/caps에서 on/off 순서를 교대해 Large 최소 3쌍을
   단독 실행한다. producer에 명시적 recorder `Off/Record` 설정을 두고 동일 binary의
   동일 scale command를 대조한다. `--timeline`은 export 옵션이므로 A/B 스위치가 아니다.
   Off 실행도 source/bin/input/caps와 summary/manifest·제품 oracle를 동일하게 묶지만
   causal profile 성공을 발급하지 않는다. disk/process-I/O observer 모드는 두 실행에서
   같게 고정한다. cache/host 상태를 명시하고 phase별 paired ratio를 보고한다. 초기 목표는
   full ingest+seal, delta/noop/delete, reopen, 동일 ordinal query group **각각의** paired
   ratio 중앙값 증가 5% 이하, 정상 capture drop=0 및 위 bounded output이다. 기존 고빈도
   sync logging은 양쪽 모두 꺼야 한다. 짧은 단계는 절대 us/ms 차이·분산도 보고하고
   측정 noise가 차이를 지배하면 INCONCLUSIVE로 둔다. 큰 full 단계로 작은 단계의 회귀를
   상쇄하지 않는다. 분산이 크거나 목표를 넘으면 intrusive/unqualified로 남기며 XL 개선률
   판정에 사용하지 않는다. 3쌍만으로 작은 차이의 통계적 확실성을 주장하지 않는다.
8. **viewer:** 작은 성공·부분 실패 fixture를 실제 Perfetto에서 열어 stage/flow/오류와
   불완전 표시를 확인한다. JSON syntax 통과만으로 viewer 검증을 대신하지 않는다.
9. **XL 종료 조건:** 위 계약을 통과한 계측으로 한 번 실행해 44–48초 retention 잔여와
   reopen 내부를 분리한다. 실패 시 partial을 분석하고 같은 실패를 단순 재실행하지 않는다.
   scale 전체 PASS와 검색 품질·타 엔진 비교의 qualification은 각각 따로 검증한다.

## 9. 진단 후 구조 수리의 결정 기준

- closure/hash가 지배하면 retained closure 계산의 증분 계약을 수리한다. physical
  inode census가 지배하면 physical accounting owner를 수리한다. 둘을 미리 모두 만들지 않는다.
- lock wait가 지배하면 lock 밖 계산 + version check/reservation을 검토한다. snapshot
  일관성을 버리거나 단순 unlock으로 안전성을 희생하지 않는다.
- compaction의 docs/bytes/object 증폭이 크면 bounded debt 정책을 검토한다.
  durability group 비용이 크면 refs/bytes/flush reason 근거로 group commit 경계를 조정한다.
- reopen의 join 대기와 active-proof/초기 조사 비용은 다른 수리다. watermark 또는 timer
  확대만으로 해결 완료라고 하지 않는다.
- CodeSearch 후보/검증 I/O가 cap을 넘으면 planner의 선택도·segment 실행을 수리한다.
  missing hit, typed budget refusal, timeout, 느린 성공을 별도 실패 유형으로 유지한다.

이 문서는 계약·구조 계획 SSOT다. 현재 worktree의 구현·집중 검증 상태는 다음과 같다.

- **VERIFIED — earlier managed-worktree focused Rust tests/build only:** `./scripts/cargow --lane test-fast-lane test -p quanta-index-search-plane --lib --all-features --locked` — 565 passed; same command for `quanta-index-lexical` — 394 passed, 1 ignored; `quanta-index-core --lib diagnostic_trace` — 12 passed; `quanta-index-searchd --lib trace_recorder` — 8 passed. Also, `./scripts/cargow --lane test-integration-lane test -p quanta-index-search-plane -p quanta-index-lexical --all-targets --all-features --locked --no-run` compiled all targets in those two packages, including external caller tests. These results belong to the managed trace worktree, not the logical checkout, and are not source-integration proof.
- **VERIFIED — process regressions:** `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(/^(e2e_scale_process_restart::)?trace_timeline_process_(close_keeps_clock_domains_separate|kill_keeps_partial_without_cross_clock_join)$/)' --success-output final` — 2 passed.
- **VERIFIED — Python parser/export tests:** `uv run --locked --extra dev python -m pytest tools/ci/tests/test_causal_cost_capture.py tools/ci/tests/test_causal_cost_profile.py tools/ci/tests/test_causal_cost_timeline.py -q` — 167 passed on 2026-10-11.
- **VERIFIED — affected package compile coverage:** `./scripts/cargow --lane test-integration-lane test -p quanta-index-semantic --all-targets --all-features --locked --no-run` and `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --all-targets --all-features --locked --no-run` — both completed successfully; these compile test targets without executing them.
- **VERIFIED — P06 regression on managed worktree only:** the test `small_scale_capture_negotiates_query_attempt_links` passed there (1 test). The logical checkout now has the same test, but its direct run did not reach the test binary: Cargo compilation failed in newly untracked source dependencies. At the captured failure, `quanta-index-repomap/src/native/mod.rs:589` borrowed moved `prefix_key`; `quanta-index-kernel/src/update.rs:1133` compared `ObjectRef` with `Option<ObjectRef>`; `projection.rs` also emitted missing `object_children` methods, but those methods exist in the current untracked `quanta-index-contract/src/index.rs`, indicating source changed during that command. These files are outside P09 and unclassified/no-touch. The build was interrupted after diagnostics; this is not a passed test.
- **VERIFIED — P09 managed-worktree snapshot build and Large diagnostics:** a temporary clean Git snapshot (`49e1cab793b98567ff62b111fc76120f3cc0c22d`, `/tmp/quanta-index-p09.HJoTCU`) matches the managed trace worktree's source tree; it does **not** match `/Users/songmin/Documents/code-new/quanta-index` (102 dirty-file differences at the original audit snapshot). Release `scale_matrix` build passed for that snapshot. Large Off and Record both completed for 4,096 files. Record trace: `Complete`, 18,995 events, zero dropped/unwritten events, zero health events; strict timeline export parsed 8,976 spans. Offline viewer export succeeded, but its execution receipt correctly says `OFFLINE_EXPORT_UNVERIFIED` / `producer_executed=false`. None of this qualifies the logical checkout.
- **VERIFIED — Large Record waterfall finding (diagnostic only):** the complete 103.438 s capture contains a 62.771 s `full_seal`; its 58.038 s `lexical_build` includes 45.645 s `build_prepare`, with nested `native_publish` 22.291 s and `file_publish` 21.826 s. The lexical span reports 79,662 sync calls and 34.318 s accumulated sync work with zero failures. Across the capture, the largest sync aggregates are `pool_control_file_full` (10.201 s / 1,874 calls), `pool_control_parent_full` (9.589 s / 1,874), `pool_adoption_full_barrier` (6.220 s / 615), and `pool_object_file_full` (3.144 s / 615). These are accumulated work counters, can overlap/nest, and must not be added to inclusive span wall. This identifies repeated durability/control-file work as a concrete Large seal cost; it does **not** explain or close the historical XL 93.969 s delta RCA.
- **NOT_QUALIFIED — Large Off/Record latency:** both runs required `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` because host load exceeded the preflight limit. Do not compare or publish these timings as performance evidence.
- **FAILED — XL Off diagnostic attempt:** 32,768-file `full_seal/build_seal` exceeded the existing 600,000 ms client request timeout under host contention. The refusal is preserved at `/tmp/quanta-index-p09-runs/xlarge-off-fixed/refusal.json`. This is an observed run failure, not a qualified product regression or performance result.
- **FAILED workload / VERIFIED trace — XL Record:** under the same contended host, the 32,768-file `full_seal/build_seal` request exceeded the existing 600,000 ms timeout. The capture nevertheless closed `Complete` with 43,008 events and zero dropped/unwritten events; strict timeline parse/export succeeded with no health or ambiguous links. The recorded `full_seal` lasted 610.19 s; request span 626.91 s was cancelled by the client while backend operation ended `ok`. Its nested spans show `lexical_build` 542.98 s (642,932 sync calls / 259.376 s accumulated sync work), `build_prepare` 412.54 s, native/file publish 212.03/182.42 s, finalize 118.65 s, and retention lock/measure about 55/51.76 s. Sync/work sums and nested spans overlap: do not add them as wall time. This localizes the XL full-seal timeout to the large lexical publish/finalize path, but it is a contended diagnostic, not a quiet-host performance result or a cause for the historical 93.969 s delta.
- **NOT_RUN — quiet-host Off/Record pairs and opening the exported trace in Perfetto.** The latest preflight reports one-minute load 11.52 against limit 8.0 (no foreign Rust process); host sampling shows sustained CPU from the Virtualization VM, `ken-mcp`, and Docker Desktop. Do not claim quiet-host timing from this state. Earlier XL lifecycle attempts remain separate from this trace implementation.

Verification covers the named managed-worktree focused tests, lossless trace parsing, and diagnostic scale executions stated above. The logical checkout is unqualified. No trace-overhead, Perfetto-rendering, quiet-host Large, or XL performance claim is established.
