# W2-09 — 기존 harness의 Native 총비용·cold build·catch-up·waterfall

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-09)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R13,R16,R39 / NI-03. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

- [x] `DONE_IMPL`: legacy E2eRuntime scale/trace/reopen/counters와 별도 Kernel `native_noop_cost`를 재사용한다. 두 경로는 한 Native producer E2E가 아니다.
- [x] `DONE_IMPL`(harness source 호출부): `scale/native_index.rs`와 `scale_matrix`의 명시적 Index-native 분기가 같은 `NativeUdsClient`/configured daemon/`BenchArtifactV1`를 연결한다. 해당 bin의 생산 check는 통과했으며 실제 실행 수용과 계층 계약은 별도다. 이 adapter를 두 번째 runner로 재구현하지 않는다.
- [x] `DONE_IMPL`(reopen oracle): 마지막 delete ACK ViewRef와 새 process Latest head identity 일치·삭제 token 부재·survivor token/path 존재를 같은 view에서 검사한다. 실행 결과와 source 경계는 SSOT·VALIDATION에서 소비한다.
- [x] `STRUCTURE_VERIFIED`: black-box harness의 공개 SDK client 의존 한 edge를 현 정책에 결합했다. 기존 production Runtime→harness·SDK→harness 금지와 Runtime dev-dependency 허용은 유지한다. 정책 회귀11개 통과는 CLI/daemon/성능 수용이 아니다.
- [x] `DONE_IMPL`(선택적 진단): 같은 Index-native adapter에 `QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1=1`의 child PID RSS·inode별 allocated `st_blocks` 관측을 결합했다. 기존 artifact detail/failure prefix를 사용하고 고정 메모리 요약·관측 누락·간격·probe 비용을 보존한다. 실행 수용은 별도다.
- [x] `DONE_IMPL`(선택적 Linux I/O 진단): 같은 child sampler가 `/proc/<pid>/io`의 `read_bytes`·`write_bytes`·`cancelled_write_bytes`를 100ms 주기와 종료 전 경계에서 관측한다. 비Linux는 `unsupported`와 누락 이유를 기록한다. 실행 수용과 media-level I/O 수용은 별도다.
- [x] `DONE_IMPL`(harness trace): 기존 진단 capture lease 아래 Index-native harness의 이름이 고정된 26개 phase에 identity/terminal을 부여했다. 별도 child daemon process 내부 span은 없다. 실행된 trace의 complete·loss·UI 수용은 별도다.
- [x] `DONE_IMPL`(명시적 foreground funding): `QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON`의 네 축을 기존 Core `WorkLimits`로 decode해 실제 daemon foreground config에 전달한다. 미지정 기본값은 기존 고정 profile이다. 소스 구현과 런타임 수용을 구분한다.
- [x] `DONE_IMPL`(유한 churn): `QUANTA_INDEX_NATIVE_CHURN_CYCLES=1..256`은 기존 delete 뒤 같은 owner의 실제 이전 ref CAS·고유 본문·ACK/query와 직전 token 부재·과거 exact view·마지막 ACK의 재오픈 head를 검사한다. 미지정은 기존 경로며 모드/횟수는 config digest에 결합한다. 장시간 steady-state나 held read pin 수용의 대체물이 아니다.
- `TextQuerySyntax::Native`만 지정한 기존 legacy 경로는 Native Index producer/event/ACK/Kernel 경로가 연결됐다는 증거가 아니다.

## Scope

기존 harness/trace 아래 실제 Native producer→searchability와 필수 유지보수까지의 총비용을 독립 입력 축으로 측정한다.

Index-native와 실제 Semantica producer 통합을 구분해 같은 harness/원장에서
다룬다. 전자는 기존 `NativeUdsClient`와 실제 configured daemon을 통해
SourceAdvance/Resync/ACK/query 경로를 실행하며 quanta-runtime 없이 준비할 수
있다. 기존 `native_product_process.rs`/`native_index_uds.rs`가 이 제품 경로의
source entry다. 기존 Kernel `native_noop_cost`만 실행한 결과는 UDS/제품
총비용으로 승격하지 않는다. 새 transport/producer/runner를 만들지 않는다.

Semantica capture/analysis/Original 비용은 W2-05/06의 실제 producer 인계를
소비할 때 추가한다. Index-only 샘플에서 제외한 capture/analysis/setup 비용을
명시하고, 그 샘플을 실제 Semantica 전체 파이프라인 성능으로 부르지 않는다.
SDK producer 미연결을 이유로 독립 Index-native adapter/계측까지 보류하지 않는다.

기존 `scale_matrix`의 명시적 `--backend index-native --native-daemon-bin` 분기는
같은 `BenchArtifactV1` writer 아래 configured `serve-native` child와
`NativeUdsClient`를 사용한다. 현재 fixture는 batched source staging,
SourceAdvance ACK, exact-view text 검색, 한 파일 delta/delete, 이전 view 검색,
독립 daemon reopen을 실행한다. owner mutation이 없어도 producer frontier를 전진시키는 SourceAdvance는 새 view를
게시·ACK한다. no-op fixture는 owner/path root 보존·frontier 변경·새 ACK view의 검색을
검사하며 이를 전체 업데이트 비용0으로 부르지 않는다.
`TextQuerySyntax::Native`만 선택한 legacy 결과를 이 분기의 결과로 섞지 않는다.

현 구현은 Index-only CoreRequired text fixture다. Semantica capture/analysis,
Original 예산, async catch-up, 실제 파생 fanout, physical write bytes/GC phase는
계측되지 않았으며 artifact에 그 경계를 남긴다. child RSS와 allocated root는
명시적 선택 시에만 별도 관측한다. RSS는 Linux `/proc/<pid>/status` 또는
macOS `ps`의 실제 child 값이고 parent resource 필드를 덮어쓰지 않는다.
RSS100ms/disk500ms 주기의 고정 크기 요약은 횟수·누락·최대 관측 간격·첫/마지막
값·관측 최대·probe wall time을 남긴다. 연속 peak 또는 physical write bytes를
주장하지 않는다. PID는 원 Child를 reap하기 전에 sampler를 stop/join해서 결합한다.
Linux의 child I/O 관측은 `/proc/<pid>/io`의 세 OS process-accounted counter를
100ms 주기와 종료 전 경계에서 읽어 첫/마지막·차이·표본 창·누락·counter 역행을
기록한다. 이는 device/media write bytes가 아니고, 첫 표본 전 작업과 마지막 표본
후 작업을 포함한 전체 생애 I/O도 아니다. 비Linux에서는 지원하지 않으며 임의의
0값이나 추정치로 대체하지 않는다. counter 역행 시 delta를 만들지 않는다.
관측 범위는 socket-ready부터 shutdown signal 전까지이며 startup·완전 drain은
포함하지 않는다. 관측 teardown 비용은 현재 drain phase wall time에도 포함된다.
개별 filesystem syscall의 중단 가능성은 보장하지 않는다. 이 항목은 수용 완료
표시가 아니다. 주기 sampler의 probe 오류·누락·counter 역행·worker 실패는
observation metadata에 남지만 root `Ok`만으로 표본의 완전성이나 I/O 수용을
판정할 수 없다. 필수 표본과 누락·오류 상태를 독립 검사한다.

Index-native trace의 26개 phase는 corpus/config/startup, workspace/profile,
full source·event ACK·search, delta/no-op/delete·old-view, 선택적 churn,
drain/reopen 및 재오픈 read/search/drain에 걸친 **harness** phase다. child daemon의
Catalog/WAL/object/scratch/seal/GC 내부 원인별 시간이나 span으로 해석하지 않는다.
capture가 없는 실행은 이 span의 존재를 성능 증거로 승격하지 않는다. phase span은
작업 본체를 감싸며 `wall_ms`와 정확히 같은 시간이 아니다. `wall_ms`에는 trace
begin/finish 비용도 포함될 수 있다. 전후 disk census는 둘 밖의 별도
`observer_ms`다.
후행 census 실패 시 work span terminal은 success일 수 있지만 phase
`completed=false`와 전체 operation/refusal 실패를 보존한다. root/phase span
`Ok`만으로 census·sampler를 포함한 측정 수용을 판단하지 않는다.

`max_observed_gap_ms`는 실패 probe도 포함한 시도 간 간격이며 성공한 데이터 표본
사이의 최대 공백 보장은 아니다. missing count/reason과 함께 읽는다. 유한 churn의
`ack_wall_ms`는 begin-input·source/mutation staging부터 게시 ACK까지를 포함하며
ACK endpoint 단독 latency가 아니다. query 시간은 별도이며 max cycle은 둘을 포함한다.

정식 scale CLI는 clean-HEAD 귀속 규칙을 유지한다. shared dirty checkout의 기능
검증은 같은 `measure_index_native_tier`를 호출하는 opt-in
`real_daemon_churn_and_resource_observation_preserve_product_gold` 테스트를 사용하며
정식 BenchArtifact를 쓰지 않는다. 추가 opt-in
`real_daemon_selected_scale_and_churn_preserve_product_gold`는 같은 helper·producer·
독립 query/reopen gold를 사용한다. `QUANTA_INDEX_NATIVE_FUNCTIONAL_TIER`는 기존
small/medium/large/xlarge tier를 명시하며, churn은 기존1..256 범위다. 선언된
corpus 폭과 모든 mandatory phase·양 daemon epoch의 실제 자원 표본을 검사하고,
tier/seed/corpus·config·daemon digest/phase·누락을 terminal 진단으로 출력한다.
새 artifact/runner나 dirty-source 귀속 우회는 만들지 않는다. 이 기능 통과로
clean-source/장시간 steady-state/성능 수용을 대체하지 않는다.

foreground 작업 예산의 미지정 기본값은 모든 tier에 기존과 동일한
`reads=1,000,000`, `writes=1,000,000`, `bytes=536,870,912`,
`mutations=1,000,000`이다. 별도 실행 입력 `QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON`은
정확히 `reads`·`writes`·`bytes`·`mutations` 네 개의 양의 `u64` 필드를 가진 JSON
object여야 한다. 누락·중복·미지 필드, 0·음수·소수·`u64` 초과값과 비UTF-8 입력은
거절한다. 명시값은 **foreground logical WorkLimits만** 바꾼다. physical cap,
execution I/O·memory grant, maintenance WorkLimits, 600초 request deadline은 그대로다.
결과에 따라 자동으로 증액하거나 최초 실패를 보고 재시도하는 정책은 없다.

선택된 값은 corpus 생성·임시 root·daemon 시작 전에 확정한다. generator 최소 token
mutation `N*(2*L+7)`을 checked 계산해 선택된 `mutations`보다 크면 측정 함수가
명시적으로 거절한다. CLI는 거절 artifact의 source binding을 기록하려고 이후
corpus를 생성할 수 있다. N은 repo 수×repo당 파일 수, L은 생성 줄 수이며
guaranteed hit3개·repo/file anchor 각2개를 포함한다. 이 검사는 **필요조건**이며
downstream scratch/map/object 등 네 축의 충분 예산 증명이 아니다. 기존 고정
profile의 Large/XL 거절 및 이전 Large full-ACK `ResourceLimited`는 그대로 실패
증거다. 명시 profile의 Large/XL은 별도 입력·config digest로 취급하며, 수용은
실제 ACK/search/reopen 결과가 결정한다.

성공 detail과 config digest에는 선택된 4축이, benchmark config digest에는
`fixed`/`explicit` mode와 4축 전체가 결합된다. 실패 시 decode에 성공한 선택
profile은 오류 context에 mode와 4축을 남기고, phase 실행 뒤 실패는 prefix에도
보존한다. decode 전 잘못된 JSON에는 유효한 선택 profile이 없다. 최신 소스의
상태 조회 보완 후 Core/Searchd/Harness·`scale_matrix` selected strict와 Mac 선정 회귀
**52/52는 `VERIFIED`**다. 이전 44/45건은 구버전 결과로 합산하지 않는다.
Pre-reconciliation source: Linux daemon/harness build와 Linux 선택 회귀10/10, Small Off/Record 각20 churn·검색/재오픈·IO gold 및 누락0 capture는 **`VERIFIED`**다. Medium fixed는 full ACK `ResourceLimited` **`FAILED`**(churn0)이며 고정 입력을 증액하지 않는다. Large explicit도 full ACK에서 SDK `Unknown(original key)`로 **`FAILED`**다. 원 key status 확인을 빠뜨린 harness 호출부를 보완했으며 영향 회귀52/52·primary strict는 통과했고 새 actual/XL은 **`NOT_RUN`**이다. 기존45/45·이전 strict는 후속 수리 전 소스 결과로 보존한다.
검증 선택·명령·결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)의
최신 W2-09 기록을 따른다. 이 선정 결과를 clean-source 성능·장시간 운영·전체
W2-09 수용으로 확장하지 않는다.

`quanta-index-searchd-harness → quanta-index-sdk`는 실제 client를 사용하는
black-box harness의 의도적 한 방향으로 정책에 결합됐다. 이 구조 수리를
재개발하지 않는다. 실행 상태는 SSOT의 W2-09 행만 갱신한다. clean-source 정식
측정, Large/XL·장시간·실 producer·Off/Record 수용은 각각의 입력과 gold를
요구한다. 명령·실패 이력·기능/계측 한계는 기존
[검증 기록](../../../plans/oct-10-index-closeout/VALIDATION.md#w2-07-platform-final-2026-10-17)을 따른다.

Same-key 확인은 private harness에서 Submit 1회와 원래 600초 budget으로
수행한다. `Accepted`/`Unknown` 뒤 원 key `Status`만 조회하며 `Pending`은
원 deadline 안에서만 대기한다. `Refused`·status `Unknown`·조회 오류·취소/만료는
실패다. 결과의 `immediate_ack`는 원 응답 수신, `reconciled_status`는 durable
게시 상태 확인이며 서로 다른 증거다. 실패 prefix에도 submit/status 경로를
보존하고 config identity에는 관측 결과 대신 고정 확인방법 버전만 결합한다.
새 이벤트 재전송·Resume·timeout 증액은 하지 않는다.

## Frozen qualification profile

기존 harness 설정/BenchArtifact와 VALIDATION 안에서 실행 전 다음을 고정한다. 별도 runner·측정 원장·profile registry를 추가하지 않는다.

- source/dependencies/config/binary, hardware·cold/warm 상태, observer Off/Record 순서와 경합 조건.
- N/H/M/D/K/output·dimension·query/filter/update trace·실행 길이, 실제 producer 또는 Index-only 범위.
- 기존 RFC §12.1–12.3의 work/debt/space·recall/latency gate와 각 profile의 아직 미정인 수치. ANN Large는 recall@10 >= 0.95와 고정 latency SLO, cached adoption은 p95 <= 500ms/p99 <= 2s라는 제안 gate를 qualified 조건에서 평가한다.
- admission/timeout/refusal/cancel·실패 prefix·mandatory drain/GC, child RSS·actual IO 측정 유무와 sampling 한계. 성공 표본만 평균내거나 parent RSS/논리 bytes를 child peak/physical IO로 대체하지 않는다.
- foreground profile의 `fixed`/`explicit` mode·네 축·config digest와 초기/실행 실패의 선택 예산 기록. Linux OS-accounted child I/O의 표본 창·누락·역행을 확인하고 비Linux `unsupported`를 보존한다. 26개 harness phase와 child 내부 비용을 구분한다.

미정 SLO는 탐색 측정·기능 gold를 막지 않는다. 그 실행은 qualification PASS가 아니며 결과를 본 뒤 threshold를 바꾸면 별도 profile이다. 물리·ANN·graph·headless owner의 독립 gold를 재사용하고 W2-19는 이 같은 측정 결과를 소비한다.

## Acceptance

- actual event→ACK→searchability와 producer/capture/analysis/Catalog/WAL/object/scratch/seal/publish/upkeep/GC/reopen의 actual call·phase counters를 검사한다.
- N/history H/true fanout/output bytes를 독립 변경하는 cold/one-file/delete/no-op/catch-up cases를 사용한다.
- 검색/ranking/old-pin/delete gold 후 같은 candidate/input/host의 Native Large/XL·mixed-load를 비교한다. failure/timeout prefix, observer Off/Record, admission wait, setup/mandatory drain과 RSS/I/O 측정 한계를 표시한다.
- 명시 4축 JSON은 엄격 decode·선택 metadata·generator 하한을 검사한다. 하한을 넘겨도 ResourceLimited·deadline·physical admission을 실패로 유지하고, 명시 profile을 고정 profile의 연속 측정으로 합치지 않는다.
- CoreRequired text의 async catch-up `NOT_APPLICABLE`을 전체 semantic/producer catch-up의 완료로 해석하지 않는다. no-op은 frontier-only SourceAdvance의 게시·ACK와 owner/path 보존을 검사한다. 기존 blanket InvalidInput 기대는 현재 SourceCut/Kernel 계약과 맞지 않아 폐기했으며 실제 업데이트 비용0의 증거가 아니다. root logical bytes와 parent RSS를 physical write 또는 child peak로 바꾸어 보고하지 않는다.
- 구현된 reopen oracle의 final ACK/head identity·삭제 부재·survivor 존재를 실제 daemon에서 실행한다. 같은 process의 `old_pin_search`는 과거 Exact view 검색이며 유지한 장기 pin·GC·공간 압박 수용의 증거가 아니다.
- W2-02의 반복 readback/History route 수와 W2-03 prior index·전체 snapshot output 비용을 호출/byte/peak 축으로 분리한다. borrowed clone 제거·Original grant 값만으로 전체 시간·RSS 상한 또는 O(Delta)를 보장하지 않는다.

## Source entry points

- [scale.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale.rs)
- [scale_matrix.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs)
- [native_index.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index.rs)
- [foreground workload](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/workload.rs)
- [child resource observations](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/observation.rs)
- [harness phase trace](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/trace.rs)
- [native_noop_cost.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/tests/native_noop_cost.rs)
- [native_uds.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/native_uds.rs)
- [native_product_process.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/native_product_process.rs)
- [native_index_uds.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/native_index_uds.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
