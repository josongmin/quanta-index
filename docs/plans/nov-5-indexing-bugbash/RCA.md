# Native 초기 인덱싱 구조적 RCA

Status: `SOURCE-BOUND HISTORICAL RCA`. This file preserves the Nov-5–12 inputs,
measurements, failed hypotheses and executed outcomes. Its dated "current" and
next-step statements are not live assignments. Current Native implementation and
acceptance follow the [active plan](../nov-12-native-cutover/README.md); reusable
execution contracts and historical lookup follow [WORKPLAN](WORKPLAN.md).

<a id="e-source-audit-20261111"></a>
## E 설치 경로: 현재 코드 직접 대조 — 2026-11-11

범위는 Core prepared DATA → Kernel `AttemptObjectReads` → Native cold
`OccurrenceWrites` → Storage leaf run → Catalog reserve/advance/confirm과
scratch confirmation/poison이다. 이 범위의 추가 확정 생산 결함은 찾지 못했다.
다른 Native 알고리즘·workspace 전체의 무결함 판정은 아니다.

| 확인한 경계 | 현재 코드의 근거 |
| --- | --- |
| 실제 진입 | Kernel `attempt_reads.rs::try_prepare_leaf_install_run`이 원 attempt/caps/budget을 전달한다. Native `text.rs`는 cold일 때만 `install_objects`를 설정하고 `flush_before_input`을 호출한다. |
| adoption 전 거절 | `text/occurrence_run.rs::try_start_run`이 inputs/pending/payload_memory 개수와 예상 밖 confirmation backing을 optional 예약·복사 전에 확인한다. |
| 원 권한·bounded DATA | `object/prepared_leaf_run.rs`는 request identity·정확 deadline·attempt/sink를 확인한다. 최대256 refs/2 packets, 원 wire/registration/path/output grant와 원 work fee를 유지한다. failed push는 handle을 poison한다. |
| 설치 경계 | `object.rs::install_admitted_run`이 기존 installer를 공유한다. 첫 reserve → 첫 sync → atomic confirm+reserve → 둘째 sync → 최종 confirm이며 파일 sync/barrier를 생략하지 않는다. |
| 논리 확정 | `OccurrenceWrites::flush`가 전체 ordered identity를 확인한 뒤 `InventoryConfirmation::finish`를 한 번 호출한다. 미완료 receipt Drop은 spool을 poison하고 `InventoryBatch::flush`도 실패 시 batch/spool을 무효화한다. |

문서와 증거의 갭은 다음처럼 정정했다.

1. **인계 상태 노후화:** E 외부 DoD에 v10 terminal 미수신·후속3개 미실행이
   남아 있었다. 원본 `v10/terminal.json`은 source `813cb26a…`에서 lib/Python/
   hexagonal exit0·completed=true이며 `lib.stderr`의61/61에 custody mismatch,
   admission cancellation, changed deadline의 정확한 PASS body가 있다.
   해당 과거 cohort의 회귀 공백은 닫혔다.
2. **후행 코드와 실행 소스 차이:** 위 cohort와 현재 E 관련10파일을 직접
   비교했다. `object_install.rs`, `prepared_leaf_run.rs`, wire preparation,
   I/O fault hook, `occurrence_run.rs`, union/custody/scope 회귀는 당시 바이트와
   같았다(이번 rustdoc 교정 전). `batch_metadata.rs`에는 count 기반 memory-bound
   helper가, `confirmed_batch.rs`에는 SQL bytes census 공유 및
   `confirmation_program_memory_bound`가 추가됐다. 공용 `text.rs`,
   `text_spool.rs`, budget/port/installer도 후행 변경이 있으므로61/61을 현재
   전체 경로의 통과로 전용하지 않는다. 최신 후보는 해당 변경을 소비해야 한다.
3. **transaction 문구 과장:** Core trait의 "No database transaction"은
   caller의 private scratch에도 적용되는 문장으로 읽혔다. 실제 `TextSpool::new`는
   `BEGIN IMMEDIATE`를 열고 scratch receipt가 파일 설치를 감싼다. 영속 Catalog
   write transaction과 immutable-file I/O의 비중첩으로 rustdoc 범위를 고쳤다.
   scratch transaction/내구성/실행 코드는 변경하지 않았다.
4. **남은 failure-integration 증거:** Native union 회귀는 `TestObjects`, Storage
   fault 회귀는 실제 파일+`TraceAuthority`, Catalog 경계 회귀는 실제 SQLite를
   각각 사용한다. 이들 PASS를 실제 Native+FileObjectStore+SqliteCatalog의
   2-packet fault→재오픈/publish/pin/GC 전체가 한 실행에서 통과한 증거로 합치지
   않는다. 그 조합의 실행 근거는 이번 감사에서 확인하지 못했다.

이번 변경은 rustdoc·문서 정정뿐이며 새 제품 동작을 구현하지 않았다.
Cargo/nextest/제품 재실행은 하지 않았다. F1/F2의 단일 후보/실행 owner를
유지하고, 현재 source 변경에 영향받는 경계·회귀만 한 큐에서 확인한다.
전체 workspace lint backlog를 E의 새 생산 결함으로 재발급하지 않는다.

<a id="final-remaining-audit-20261111"></a>
## 최종 마감의 입력 누락과 실행 상태

초기 감사 시16파일은338개를 실행한 ac0a0c8e와 동일했고 새 확정 Native 생산 결함은
찾지 못했다. 후행 F3a 전체 Clippy에서 이 소유5파일의49개 lint(생산43·테스트6)가 드러나 코드 수정과
통합 정적 리뷰를 끝냈다. 첫 Native 회귀는 명시 import에 남은 미사용 trait1개로
본문0/exit101 실패였다. 그 import만 제거한 r8의 Native338/338·Contract/Core335/335·
IPC27/27은 각각 통과했다. 전체 lint나 원 제품 수용으로 승격하지 않는다.
D의 구 build9/source2b2980과22a8712e는 후행 수정 이전이다. 새824629e4를 동결해
canonical shared 입장으로 bench pair를 VERIFIED/exit0 완료했다. source·입력·
도구/config/deps·두 바이너리 결속을 확인했다.
따라서 현재 막힌 것은 "확정된 새 인덱싱 알고리즘 구현"과 같지 않다.

- **integration/proof gap:** 구 후보의 PreparedInventoryChanges는 원 spool을 보관하지
  않고 complete에 따로 받으며 미완료 Drop 무효화가 없다. 현재 main에는 두 경계가
  묶여 있다. lower authority 이동과 Scratch trace-enter health 수리도 후행분이다.
  구 후보의 alias 검증 자체가 없다는 뜻은 아니다. 최신 수리를 검증할 제품 후보부터
  재고정해야 한다. 이 누락을 새 duplicate 구현으로 해결하지 않는다.
  후속22a8712e는 당시 생산8파일을 반영했고 새824629e4는 이후 부모5/공통 계약
  수정도 소비했고 실제 빌드 terminal까지 검증했다. 남은 경계는 원 제품 수용이다.
- **environment/execution gap:** host-exclusive10은3600초 admission 대기 후 종료했다.
  controller PID가 없고 새 x3/m3 산출물도 없다. 해당 후보의120초 ACK/검색/재오픈·
  Mixed·Off/Record 새 비교는 NOT_RUN이다. 옛18f 실패와 새 후보 판정은 분리한다.
- **공용 gate/검색 scope:** API/module snapshot은 새 HybridFile/cleanup surface를
  반영한 renderer 갱신과 API/module 재검사는 통과했다. wire inventory도 수정됐다.
  file-profile cap-return 수정은 현재 소스 benchmark6/6으로 재컴파일·실행했다. 이들은 기존
  검색 Root의 소유이며 Native indexing 구현을 새로 뜯을 근거가 아니다.

이전 C34 PID는 현재 없지만 다른 global lock FD가 있어 입장 가능 여부를 단정하지
않는다. 후속 확인에서 두 QBC의 cargo 자식 rustc CPU 시간이 실제 증가했다. 현재
관측은 진행 중인 shared 컴파일과 일치하며 잠금 누수·순서 교착의 증거는 없다.
FD 목록만으로 정확한 flock mode나 host-free를 입증하지 않는다.

사용자의 실제 위임 이후 Root는 영향 회귀/소스 합류를 완료해 실행권을 반환했고,
신규 Native 담당이 최종 후보 빌드를 완료했다. 기존 D는 이력으로 보존한다.
완료 r5는734개·106파일의 전체 lint로 FAILED다. 이를 모두 Native 마감의 선행
구현으로 묶는 것은 별도 owner 작업/테스트의 범위 확장이다. 실행 순서를 Root의
변경 영향 회귀/소스 합류 → 신규 Native F1/F2 → Root 공통 잔여로 분리했고 원120초 기준은
유지했다. 전체 lint·release green은 별도이며 새 Native 제품 결과는 아직 없다.

기존 D는 최종 실행 위임을 이전 "외부 위임 무시" 규칙으로 거절했고 추가 실행0/idle이다.
이는 code/build/Product 실패가 아니다. 사용자가 허용한 신규 실행 채팅 하나로 F1/F2를
이전했고 착수를 확인했다. 기존 소스/실행기/증거를 소비하며 생산 구현을 복제하지 않는다.

후속 wait0 및 canonical EX900초 단일 예약은 실행 lock busy로 BLOCKED였다.
04:52:54 KST 예약/잠금을 해제했고 PID43963은 없다. Large/XL/Mixed 본문0이며
이 후보의120초 코드 실패나 성공으로 판정할 수 없다. fresh lock RCA의 이전 세
실행자 중 둘은 종료됐고 남은 PID91030의 rustc CPU는 진행했다. 중첩 global 잠금·
누수/교착의 증거는 없다. 별도 코드 수리로 이 호스트 입장 차단을 닫았다고 표시하지 않는다.

900초 만료 뒤 새로운 shared 작업이 들어온 것도 확인했다. 기존 P07 job의1시간31분
진행과 비교하면900초 입장 예약이 기존 heavy compile 종료 전에 끝날 수 있다.
최종 pair를 보존한7200초 예약에서 canonical EX는179.9초 뒤 획득했고 project admission도
통과했다. 이후600초 timing preflight가 load22.57959(한도8.0)로 차단했다. 종료 시점의
foreign Rust는0이었고 제품 runs는0이었다. controller는05:21:33 KST 종료·해제됐다.
[실행과 기록](#native-final-executor-7200-20261111)
원120초·Mixed 조건을 유지한 F1/F2와 공용 F3의 단일 소유는
[최종 잔여 계획](WORKPLAN.md#final-remaining-audit-20261111)을 따른다.

<a id="session-hardening-20261110"></a>
## Session hardening: 구현 결함과 테스트 증거를 분리

현재 소스/consumer/negative oracle을 병렬 읽기 전용 감사했다. 최근 alias/SQL 패킷과
앞선 이 채팅의 Posting 정규화·packed row를 포함했고 타 owner 구현은 재발급하지 않았다.
확정 생산 P0~P2는 발견하지 못했다. 아래는 테스트 증거의 수리이며 제품120초 해결의
새 증거가 아니다. 초기 리뷰 소스는29f93e6d, 테스트 보완 입력은ac0a0c8e다.

- **introduced — 정상 alias fixture의 identity 위반:** 같은76B payload/빈 children을
  임의 ref181/ref182 두 개로 등록했다. canonical object는 wire92B의 content hash라
  두 identity가 동시에 정상일 수 없다. 독립 hashlib oracle은 position0의 ref를
  `160a5a73…`, position1을 `af0c3b33…`로 계산했다. 현재 fixture는 기존 canonical
  registration helper를 재사용하고 서로 다른 유효 position wire/ref를 쓴다.
  이전4!=2 RED는 mock control-flow 증거이며 정상 제품 identity 증거로 승격하지 않는다.
- **introduced proof gap — retained mock의 byte fee 누락:** reads1만 부과해 실제
  read→wire→decode 비용 보존을 입증하지 못했다. mock을 맞추되 mock으로 실제 adapter를
  증명하지 않는다. 별도 실제 FileObjectStore test가 child0/2·payload3B에 대해
  wire19/83B, 총22/150B·raw usage·경계 ResourceLimited를 검사한다. 파일을 제거한 뒤
  retained DATA 성공도 확인해 원 객체 파일 재오픈을 요구하지 않음을 실제 owner에서 검증한다.
- **introduced proof gap — 준비 이후 partial completion:** 기존 회귀는 alias/Diff 오류나
  완료 전 Drop까지만 보았다. 정상 Root와 동일 WorkBudget에 Top write1개를 남겨
  첫 owner 완료 후 둘째가 Writes로 거절되는 실제 분기를 검사한다. Root0/가짜 overflow를
  기대하지 않으며 부분 로컬 효과1/9와 spool poison을 함께 본다. 제품 prepare가
  `batch.flush?` 후에만 root/candidate를 만드는 연결은 정적으로 확인했다. 새 회귀는
  owner-local 증거이며 process ACK/출판/재오픈 증거로 승격하지 않는다.
- **pre-existing adjacent test gap — all selectors 명칭과 oracle:** 기존512 selector
  회귀는 header512와 양끝0/255만 검색했다. 재오픈 후0..255 양 case512개 전체의
  `(1,[ordinal])`을 확인하도록 기존 루프만 확장했다. 새 indexing logic은 없다.

정상 sourceac0a0c8e의338/338은 VERIFIED다. Drop guard 제거에서2개 실제 body가 Top read assertion으로 FAILED/exit100였고,
원복 후 같은2개가 PASS/exit0여서 guard 민감도도 VERIFIED다. 이는 owner-local
실패 분기를 잡는 증거이며 process 출판 수용으로 승격하지 않는다. 기존335개 성공을 이
강화된 oracle의 실행 성공으로 바꾸지 않는다. 기존 Native 제품 acceptance owner의
원120초·ACK/검색/재오픈·Mixed는 별도이며 이 감사는 중복 controller를 만들지 않는다.

생산 코드8파일의 새 변화는0이다. 이번 보완은 테스트3파일이며 정상338과 mutation2/원복2를
합산하지 않는다. actual Native Off 진입은 trace path가 없으면 capture 없이 실행되고,
Kernel AttemptObjectReads.read_retained은 원 adapter로 전달됨도 정적으로 확인했다.

<a id="alias-sql-implementation-20261110"></a>
## 두 반복과 새 의존성 순환 수리

현재 코드 패킷은29f93e6d다. 기존18f clean-host XL120 실패를 확인했으므로
호스트만 원인으로 닫지 않는다. 새 코드의 XL120 수용/절감률은 아직 측정하지 않았다.
이전 Record CPU/wall을 신규 개선율로 전용하지 않는다.

### Checked DATA를 버린 뒤 다시 읽던 경계

기존 alias verifier는 persisted get·정확 ref·parse·count/first anchor·전체 key seal을
마친 DATA/layout을 버렸고 Diff cursor가 다시 get/parse했다. 현재는 private-field,
non-Clone `VerifiedColdInventory`가 root/checked DATA/원 object port/attempt를 묶는다.
소비 시 attempt·root/ref·childless와 `read_retained`의 정확한 DATA를 확인한다.
최초 Diff read의 fee·provider 거절과64행 retained/count·취소/deadline을 유지한다.
고정 QITXOC01의 두 ref mock 회귀는 old2ad에서4!=2 RED였고29f의335에서 통과했다.
임의 ref라는 fixture 한계는 위 hardening에서 정정했고, 새 canonical fixture의 get2는
ac0a0c8e의338에서 확인했다. 과거4→2를 정상 identity/제품/전체 XL 개선율로 전용하지 않는다.

배치 cache 대신 alias마다 canonical Diff를 준비한다. `PreparedInventoryChanges`는
원 batch/roots/spool의 배타적 view이며 decoded block을 추가 보관하지 않는다.
모든 alias·Diff·마지막 Posting flush가 통과해야 contribution/top/Root 완료를 시작한다.
준비 중 Posting은 provisional이다. 늦은 seal/count/ref/token 오류·provider 거절/변조·
취소·다른 attempt·미완료 Drop은 batch와 원 spool을 poison한다. 버퍼가 남은 Top
cursor/read/write/packing도 재사용하지 못한다. 기존 bounded occurrence cache2는 유지한다.

### SQL shape 반복과 표본만 있던 Posting 관측

공유 mask·SQL 길이·번호의 반복 계산을 한 shape로 모았다. 길이는 decimal 자리수
구간으로 checked 계산하고 metadata grant 뒤 번호 table을 한 번 만든다. render와
일반/ID bind가 같은 shape를 소비한다. 원 WorkUsage·실제 metadata 선예약·variable
한도·callback-before-effects·positive ID·취소·poison은 유지하며 새 cache는 없다.

Record의 PostingEdit36(query/preflight),37(capacity),38(binding/intern),39(execution)는
전체 flush의 CPU/wall을 관측한다. pager 표본34/35는37/39의 child다. 성공39에
rows/batches를 기록하고 진입 오류는 원인별 health로 남긴다. Off clock 호출0을 회귀로
확인했다. VFS22는 connection-scope sibling이며 겹침을 합산하지 않는다.
기존 집계기는 `(stage,lane,nearest_phase)`로 분리하고 pager는35만 선택하므로39의
전체 rows와35의 표본 rows를 더하지 않는다. 표본 cache miss0은 전체 비용 증거가 아니다.

### typed proof가 만든 양방향 의존

수리 중 cursor가 upper `entry_patch::VerifiedColdInventory`를 받아 새 모듈 cycle을
만들었다. proof·seal·검증 생성자를 기존 lower occurrence owner로 이동했다.
producer는 검증 constructor에 위임하고 기존 경로는 동일 타입 re-export다. 미검증 DATA
생성자나 두 번째 IR은 없다. HEAD 두 crate의 기존 SCC2와 최종 key 집합이 같아 추가
cycle0을 확인했다. 기존2건을 새 구현 결함으로 중복 발급하지 않는다.

### 검증과 남은 수용

- 최신29f93e6d의 Scratch 전체·Native Text/Spool335/335는 VERIFIED, exit0,
  nextest `a0b3c65f-2de4-48df-8104-05332464924c`다.
- Scratch 자체 strict/49개는 dcde21cf에서 VERIFIED이고 이후 해당 scope 변경0이다.
  원 dependency-inclusive Clippy는 다른 Contract 입력17 lint로 FAILED다. 이 패킷의
  자체 strict 성공을 전체 lint·전체 main/release 성공으로 전용하지 않는다.
- 새 pair·Large/XL ACK·검색·재오픈·Mixed·matched-host 수용은 NOT_RUN이다. 기존
  Native 실행 owner에 유지하며 제품 controller를 중복 시작하지 않았다.
- server120초·원 자원·정확성·보존성 검사는 유지했다. 기존 C replay/Catalog GC/E를
  재구현하지 않았고 관련 없는 main hunk를 보존했다.

[독점 범위·정확한 명령·증거 경계](WORKPLAN.md#alias-sql-implementation-20261110)를 따른다.

<a id="remaining-cost-audit-20261110"></a>
## 추가 RCA: 새 실행 없이 좁힌 수리 후보

현재 소스와 기존 complete trace를 병렬로 읽기 전용 감사했다. 제품·Cargo 실행은
추가하지 않았다. 아래는 확정된 코드 반복과 미확정인 성능 기여율을 구분한다.
공유 main의 cold output/arena replay와 Catalog GC 수리는 기존 소유 범위로 보존한다.

### 호스트 단독 원인으로 닫을 수 없음

`/private/tmp/qidx-xl-mixed-rca-z1a_82he/x2-control`의 원본을 재확인했다.
source18f6f89d의 preflight는 clean/load1=4.723633/foreign Rust0이고 주기 관측도
별도 Rust 없음이었다. XL은 원120초 BuildPrepare deadline으로 FAILED,
outer timeout false/producer238.568초였다. accepted work는
R3,707,595/W12,898,307/B10,895,598,466/M17,241,442이며 수치·memory·외부 자원
거절은 없다. 이 사실은 '호스트만 안정되면 반드시 해결'이라는 결론을 배제한다.
18f 결과를 후행49e/243 또는 최신 main의 판정으로 전용하지 않는다.

### 후보 1: independent seal의 checked DATA handoff

**코드상 중복은 확인, 절감 시간과 안전한 채택은 미확정**이다.

- `entry_patch.rs:70-135`의 `ColdInventoryAlias::verify`는 persisted object get,
  `CheckedBlock::parse`, 모든 key의 seal 검증을 마치고 `EntryMap`만 반환한다.
  검증한 DATA와 layout은 버린다.
- `InventoryBatch::flush`는 모든 alias를 선검증한 뒤 owner 순서대로
  `finish_change → diff_inventory`를 호출한다. `PackedInventoryCursor::open`은
  동일 ref를 다시 get/parse한다. 현재 bounded object cache가 일부 get을 완화해도
  layout 재파싱은 남는다.
- 수정 방향은 verifier가 정확한 root/ref/checked DATA를 소유한 typed 결과를
  반환하고 Diff가 이를 소비하는 것이다. 최초 Diff read의 논리 fee·거절은
  `read_retained`로 유지하고, 64행 경계와 count 검증도 유지해야 한다.
- **현재 순서를 단순 변경하는 패치는 채택하지 않는다.** owner별 verify 직후
  finish하면 뒤 alias 실패 전에 앞 owner의 contribution·posting·root 상태가
  달라진다. 256개 checked block을 새 cache에 보관하는 해결도 채택하지 않는다.
- 최대 한 block만 보관하려면 alias 검증·Diff 준비와 최종 owner 반영을 분리하는
  계약이 필요하다. 모든 alias 성공 전에는 contribution/top/root 완료를 하지 않고,
  provisional Posting SQL을 허용할지와 late failure 시 poison/폐기를 명시해야 한다.
  현재 all-alias-before-finish와 오류/과금 순서가 달라지므로 구현 전 독립 회귀가 필요하다.

필수 회귀는 뒤 alias의 잘못된 key seal/count/ref, 늦은 Diff count 오류,
취소·deadline·메모리 거절에서 성공 prefix가 게시되지 않음, 원 attempt fence,
읽기 fee·retained refusal, 기존 payload wire/검색/재오픈 동일성이다. 이는 기존
cold token credit/replay나 E 설치를 다시 구현하는 항목이 아니다.

### 후보 2: Posting 내부 비용 분리 후 SQL shape 정리

동일 성공 Diff prefix9,472개의49e CPU15.109829초 중 직계 자식은
PostingEdit33 9.824908초(32,544회), InventoryRead30 3.956529초(66,304회),
OccurrenceRead31 0.005459초, 나머지1.322933초다. 부모와 자식을 더하지 않는다.
양쪽 prefix의 WorkUsage 네 축은 항목별로 모두 일치했다. 다만 변경하지 않은
단계도 느려진 cohort이므로 이 비율을 코드 개선/퇴행의 인과로 해석하지 않는다.

기존 VFS 카운터도 추가로 읽었다. 다음은 같은 prefix의 Posting33 아래 범위다.

| 관측 | 6ffa x2r | 49e x1r |
| --- | ---: | ---: |
| lane22 scope/segment 수 | 25,754 | 25,754 |
| 실제 lower xWrite 횟수 | 52,626 | 52,626 |
| 요청 bytes | 215,556,096 | 215,556,096 |
| write 실패·admission/post 거절 | 0 | 0 |
| lane22 전체 wall | 3.394253s | 7.116484s |
| BeforeWrite admission callback wall | 1.233939s | 2.470581s |
| lower write envelope wall | 0.379536s | 1.070632s |
| AfterWrite callback wall | 0.069234s | 0.161500s |

lane22의12개 카운터는 양쪽25,754개 모두 존재했다. 세 callback 구간은 CPU나
장치 서비스시간이 아니다. 49e 잔여3.413771초는 scope의 다른 작업·간격이며
이를 전부 SQLite 실행이나 host 대기로 분류하지 않는다. lane22 수를 물리 write
횟수로 해석하지 않는다. BeforeWrite는 원 publication/allowance/capacity 인증이므로
호출을 생략하는 해결은 배제한다.

`edit_write.rs`는 매 flush마다 공유 mask·SQL 길이·파라미터 번호를 계산하고
같은 shape의 String을 다시 만든다. `prepare_cached`는 statement를 재사용하지만
그 앞의 문자열 생성은 재사용하지 않는다. 번호 helper도 길이 계산·SQL 출력·
binding에서 반복 호출한다. 이는 확정 반복이지만 Posting33 CPU 중 차지하는
시간은 현재 확인할 수 없다. capacity/execute의 기존 계측은19개 표본뿐이다.

다음 최소 계측은 기존 Posting33을 기준으로 preflight/query construction,
capacity, binding/intern, execute의 Record 전용 CPU·wall·총 rows/flushes를
집계하는 것이다. Off에서는 clock/SQL/추가 grant가 없어야 한다. 기존19개
pager 표본을 전체 시간으로 외삽하거나 새 cache를 먼저 추가하지 않는다.
측정상 의미가 있을 때만 하나의 pure SQL shape 계산을 출력/bind가 공유하도록
정리한다. 8 sharing mask와 count1/64/80/127/128, 원 fee/max variable,
late callback/cancel-before-effects 및 poison 회귀가 수용 조건이다.

### 이번 수리 후보에서 제외

- ingress/SourceApply/실제 lexical body의 UTF-8 검사는 시점과 권한이 다르다.
  기존에 전달되는 typed UTF-8 proof가 없으므로 단순 생략하지 않는다.
- identify 뒤 install의 hash는 raw payload와 expected ref 일치의 검증이다.
  검증된 payload를 받는 계약 없이 두 번째 hash를 제거하지 않는다.
- E의256-input run/3 transaction과 객체별 anchored writer는 이미 연결돼 있다.
  같은 설치를 다시 구현하거나 COMMIT·inode 검사를 삭제하지 않는다.
- StageSources의 owned bytes→borrowed input→새 Vec 복사는 제거 여지가 있지만
  이번 SourceAdvance120초 밖이므로 우선 수리 대상으로 올리지 않는다.
- SQL template 전체 cache·across-flush value ID cache·checked block batch cache는
  추가 상태/메모리/실패 경계를 늘리므로 측정 없이 도입하지 않는다.

<a id="packed-diff-rca-20261110"></a>
## 완전한 x2r: CPU 반복과 중간 페이지 할당

source6ffa의 child `/private/tmp/qp6f-y1rh4krj/x2r/causal-timeline.jsonl.native-xlarge-children/first.jsonl`
은 complete, accepted/written3,329,922, dropped/unwritten/open0이다. 원120초
deadline 실패이며 outer timeout은 아니다. parent attempt41907은120.080초,
SourceApply41978은30.381초/CPU21.721초, BuildPrepare214875는86.267초다.
동일 서버 RequestBudget이 SourceApply→BuildPrepare로 전달되므로 둘 다120초를
소비한다. 클라이언트 대기600초와 혼동하지 않는다.

| 겹침 없는 구간 또는 하위 귀속 | wall | 관측 thread CPU | 범위 |
| --- | ---: | ---: | --- |
| SourceApply | 30.381s | 21.721s | 32,768 owner |
| InventoryPrepare | 47.164s | 34.729s | 23,428회, 마지막1회 실패 |
| InventoryDiff | 24.568s | 23.511s | 완료23,296회 |
| PostingEdit/33 | 14.989s | 14.398s | Diff 하위80,127 flush |
| ManagedObjectInstall | 18.980s | 14.956s | InventoryPrepare 하위182group/23,296input |
| E Catalog COMMIT | 10.512s | 4.800s | InventoryPrepare 하위273회 |

Posting을 Diff에, install/COMMIT을 InventoryPrepare에 다시 더하지 않는다.
물리 input23,296은 전체 XL 문서 수가 아니라 실패 전91개의256-input run이다.
COMMIT273회는 reserve→advance(previous confirm+next reserve)→confirm과 일치한다.
추가 중복 COMMIT은 발견하지 못했다. write0.540초/fsync0.695초보다 객체별
ticket 준비3.605초/open2.947초/publish-retire5.313초/settlement2.399초가 크다.
이미 E가 fanout/dedup을 수행하므로 파일 수를 단순 중복이라고 간주하지 않는다.

Source owner 준비의 record get9.443초/body get·검증9.370초는 미세 Record 계측이
섞인 수치다. staging의 UTF-8 검사만 믿고 SourceApply 검증을 생략하면 arbitrary
stage_object/기존 base 경로가 깨진다. ingress/exact-ref 증명 없는 생략은 채택하지 않는다.
호스트 load와 wall−CPU에는 I/O·스케줄링·락이 섞여 있으므로 CPU 외 시간을 전부
host 경합으로 분류하지 않는다. host preflight blocked, 정식 timing은 NOT_RUN이다.

확정된 중간 표현 반복은 packed inventory→64행 MapPage→각 key Vec/lease→
Diff head→Posting inline copy다. CheckedBlock이 정렬·회원·위치를 이미 인증했는데
Diff가 한 번에 두 head만 필요해도 페이지 전체의 키를 다시 할당한다. 신규 수리는
그 소비 경계에 callback row visitor를 연결한다. 작은 키는 기존32B inline 표현을
사용하고 긴 키는 원래 메모리 예약을 지킨다. 새로운 wire/대체 indexing logic은 없다.
retained 읽기는64행 경계, 키 bytes는 실제 행마다 부과하며 실패한 행의 ordinal은
전진하지 않는다. count 검증·취소·deadline과 최종 attempt 내 block 승격을 유지한다.
고정 후보49e585522d2d2e8b9800a3287f6272a8daf3922d의 묶음 회귀315/315는
VERIFIED, nextest `606ff834-7f84-4e4b-8ae9-1e7dfcbbf67c`다. main의 대응2파일과
검증 고정 시점에 byte-identical이었다. 실패 후 재시도·64행 경계·취소·mode 혼합·고정 wire/count와
inline head 수명/추가 grant 없음까지 확인했다. 이 성공은 기존312와 합산하지 않는다.
XL 시간 절감률·원XL120초 수용은 NOT_RUN이다. 제품 pair는 같은 후보에서 cached
build 한 번만 수행하며 이전6ffa pair의 Large 성공을 새 후보로 재발급하지 않는다.

49e의 새 pair build는 exit0/4분38초, 별도 `qpd49-c0snn4v6/l1` Large Off는
원120초 ACK33.985초·첫 검색97.410ms·재오픈1.234초·검색70.946ms와
이전 view/delta/noop/delete를 실제 통과했다. 6ffa와 host 조건이 다르므로 두
ACK 시간 차이를 코드의 순개선율로 해석하지 않는다.

같은49e pair의 XL Off `qpd49-c0snn4v6/x1`은 **FAILED**, outer timeout false,
producer339.027초다. staging175.325초 완료 후 full ACK phase148.093초가
미완료로 끝났다. 서버의 첫 거절은 원120초 deadline/BuildPrepare이며 accepted
R2,936,845/W6,930,910/B6,170,432,801/M9,108,679다. numeric fee·memory·
external resource 거절은 없다. checkpoint `storage/object/write_preparation.rs:133`은
마지막 검사 위치이며 누적 병목의 원인 증거가 아니다. XL 검색·재오픈은 NOT_RUN이다.

호스트1Hz 관측299개 모두 외부 Rust 작업이 있었다. 전체 제품 구간 load1은
min16.442/median21.697/max62.891, 가용 disk는106.6GB→95.4GB였다.
관측 sample과 서버 span은 동일한 경계가 아니므로 peak62.891을 전부 ACK 중
경합으로 귀속하지 않는다. 전체 구간에 외부 Rust가 있었다는 사실과 코드 효율의
기여율은 별개다. 이전6ffa Record와 현재 Off의 도달량 차이도 새 코드의 회귀율이 아니다.
새 diff의 할당 제거와315개 정합성 회귀는 닫혔지만 원 XL 수용은 아직 닫히지 않았다.
근거 없는 재실행·COMMIT 삭제·입력 축소·deadline 증가는 하지 않는다. Mixed 및 정식
timing은 NOT_RUN으로 유지한다.

추가 x2r 귀속에서 LexicalBuild의 직계 자식은 InventoryPrepare47.164초,
InventoryDiff24.568초, InventoryMapApply6.132초(CPU5.784초)다. 남은 약8.4초
wall/5.17초CPU는 상당 부분 owner 준비 사이에 분산된다. 루프에는
`SourceInputCursor::next_change`의 record 읽기/JSON 검증과 `batch.push_change`가
있지만 어느 함수가 이 시간을 얼마나 쓰는지는 현재 trace로 분리되지 않는다.
Posting/root에는 아직 도달하지 않았으며 Literal 작업이라고 귀속할 근거도 없다.
49e의 후행 Record `x1r`은 재빌드 없이 한 번만 수집해 성공한 Diff의 같은 prefix를
비교한다. footer 완전성·WorkUsage 일치·CPU counter 존재를 먼저 확인한다.
실행 순서 자체는 exact owner identity 증명이 아니며, 서로 다른 host wall과
서로 다른 도달량을 속도 개선율로 계산하지 않는다.

49e `x1r`도 원server120초 BuildPrepare deadline으로 FAILED, outer timeout false다.
child186,688,920B, accepted/written1,845,794, dropped/unwritten/open0으로 complete다.
old6ffa x2r와 new49e x1r의 성공 Diff 접두부 N=9,472개는 각각4축 WorkUsage가
항목별로 모두 일치했다. CPU 합은 old9.384975초/new15.109829초(+61.0%), wall은
9.940365초/17.761853초다. 이번 자료에는 Diff CPU 절감 증거가 없다.
그러나 변경되지 않은 Prepare 성공 접두부의 CPU도13.835→24.802초(+79%),
MapApply37개의 CPU도2.407→4.233초(+76%)다. host/cohort의 전반적 편차 때문에
Diff 증가만을 코드 회귀라고 단정하지도 않는다. 이 비율로 보정한 속도 개선율도 발급하지 않는다.

추가 제품 실행은 멈췄다. 기존 page/새 row 경로를 같은 프로세스에서 ABBA 순서로
교차하는 작은 재사용 진단을 추가해 실행했다. 고정 wire·키 순서·ref·WorkUsage·retained 읽기·
메모리 grant 반환이 같은지 확인하고 CPU/할당을 관측한다. 시간 자체를 테스트 성공
기준으로 삼거나 이 진단을 원 XL 수용으로 승격하지 않는다. 아래 진단 결과에 따라
이 추가 row 경로를 유지했다.

교차 진단은 test-only243b6ccf에서 최적화 bench profile로 **VERIFIED**, nextest
`d1395529-9628-4ffa-a792-af96ac041285`의1개 본문이0.034초에 통과했다(992 제외).
65개 고정 키·한 vector인 QITXOC01 wire를 page/row 두 경로에서 읽었다. 각 leg는
256 cursor, ABBA 두 순환이다. 모든 cursor의 고정 키/ref·checksum·WorkUsage
(reads2/bytes325/writes0/mutations0)·객체 읽기2회(최초 get1+retained1)와 grant 반환이 일치했다.

| 관측 | page 4 legs | row 4 legs | 범위 |
| --- | ---: | ---: | --- |
| process CPU 합 | 13,588µs | 10,425µs | 이 진단의 cursor 구간만, 23.3% 감소 관측 |
| CPU/leg 범위 | 3,272–3,685µs | 2,592–2,643µs | 각256 cursor |
| memory reservation calls/leg | 17,408 | 256 | cursor당68→1회 |

키별 중간 페이지/예약의 제거는 확인됐으므로 row 경로를 유지한다. 이 fixture는
TestObjects/TestMaps와 계수용 memory scope를 쓴 짧은 단일 프로세스 진단이다.
SQL·실제 파일 I/O·Catalog COMMIT·실제 allocator/RSS는 포함하지 않으며 전체 XL
속도나 tracing overhead 비율을 의미하지 않는다. 변경하지 않은 Prepare/MapApply까지
느려진 두 제품 Record의 편차를 이 수치로 보정하지 않는다. 원 XL120 수용은 여전히
FAILED이며 Mixed·matched-host Off/Record·전체 workspace/release 수용은 NOT_RUN이다.

<a id="posting-product-rca-20261110"></a>
## 재감사: 코드 반복과 실행 증거의 공백

최종 제품 입력은 `6ffa888f8eb6de563ab9fe8ab00a9d7604ddc6e2`다. 기존18f에
정규화 8파일을 문맥 병합한 결과는 main 대응 파일과 byte-identical이었다.
현재 Native Core/Runtime/Storage/Catalog 생산 코드와 cold census/output도
main과 일치함을 다시 확인했다. 원 Large/XL120·Mixed 수용은 아직 닫지 않는다.

최소 source b534에는 이미 main에 구현된 canonical/Unicode cold census와
scoped output memory credit가 없었다. 이 입력으로 최종 제품 시간을 판정하면
옛 경로를 다시 분석하게 된다. b534의 build1은 이를 확인한 뒤 중단했다.
exit143은 SIGTERM 취소이며 제품 body0이다. source/binary가 다른 결과를 혼합하지 않는다.

통합 c459의 첫 회귀는 test-only private API/trait scope 누락으로 compile101/body0.
기존 main의 두 fixture 수리를 소비한176 후보는311개 중310 PASS/1 FAILED였다.
실패는 production이 정상 수락한 rooted directory tag `0x83`을 오래된 테스트가
invalid로 분류한 탓이다. unknown tag를 `0xff`로 바꾸고 directory의 잘못된 폭/child
count 거절을 추가했다. 네 rooted layout `0x80`–`0x83`의 정확한 9B wire·두 child·
토큰 수를 독립 고정 gold와 empty object store로 확인해 정상 형식도 검증한다.
제품 decoder·wire·budget을 바꾸지 않았다.

그 수정까지의6ffa 후보는 Native Text/Spool·Scratch lib **312/312 PASS**,
720 제외·nextest `bd41323c-7765-4764-aaad-fa948d34d263`다. 앞선74/299개와
합산하지 않는다. 새 normal cached bench pair는 외부 `posting-build-2`에 결속한다.

### Source6ffa의 실제 Large·XL과 host 분리

normal cached build2는 exit0이다. daemon SHA는 `9e1e3a69d06e545c64aa6b6bebfd9a1324d6e3862ea7e4f2849046bb96028f69`,
harness SHA는 `be77ec906de8eddb09b25598adb057783d50ad3a42198bedcd4bbaceb2aed6d0`다.
실행 전후 source/config/deps/vendor locks/tool epochs·binary를 대조했다. 이 수동
결속과 capture의 prebuilt `build_binding=UNPROVEN`은 별개이며 정식 수용으로 바꾸지 않는다.
원본은 checkout 밖 `/private/tmp/qp6f-y1rh4krj/{l1,x1,x1r}` 및 각 `*-control`이다.

- `l1` Large Off: exit0·timeout false·PRODUCT_ARTIFACT_VERIFIED. full ACK42.163452초,
  첫 검색98.711250ms, 재오픈1.302801초, 재오픈 head68.462791ms·검색80.607083ms다.
  configure/full/delta/noop/delete의5 publication 모두 status poll 없이 immediate ACK.
- `x1` XL Off: exit1·outer timeout false. 입력 적재128.526276초 완료 뒤 원 server120초
  BuildPrepare deadline, ACK 단계144.723655초 미완료. checkpoint block.rs:88은 마지막
  검사 위치다. accepted work R3,144,312/W8,340,784/B7,394,882,859/M10,916,460,
  numeric charge·memory·external resource refusal 없음. root byte 순증은219,160,521B다.
- x1 host의1분 load18.2–32.7, 외부 Rust3–5개, VM counter28개 구간의 평균 swap
  in/out12.9/13.0MiB/s, 인접10초 구간 최대71.9/113.9MiB/s였다. l1은 더 큰
  swap211.8/173.6MiB/s에서도 통과했다. 입력량이 다르므로 성능 비율은 비교하지
  않지만, 현재 자료로 host만이 XL 실패의 원인이라고 단정할 수 없다.

`x1r` Record는 원 deadline에서 실패했고 child는64MiB 한도에서 partial이다.
accepted2,220,285 중 written672,235/unwritten1,548,050, dropped0/open5다.
SourceApply 진행 중 기록이 끝나 seq2 BuildPrepare/설치/COMMIT은 기록되지 않았다.
상위 harness timeline의 complete와 child 완전성을 혼동하지 않는다. daemon 전체
reserve647·confirm646·write/sync87,384를 이 요청의 구축 비용으로 귀속하지 않는다.
같은 binary에서 출력 allowance만512MiB인 `x2r` 수집이 완료됐다. 완전한
footer·parent chain의 귀속은 위 최신 절을 따른다. 원120초·작업·메모리·disk 제품 한도는 그대로다.

현재 host load/swap·가용 공간은 별도 raw 관측으로 보존한다. 자기 checkout의
비활성 debug incremental cache만 Cargo lock을 얻어3,417,548KiB 정리했다.
활성 bench target·기존 binary/source/원 증거 및 타 작업은 삭제하지 않았다.

- E는 `OccurrenceWrites → LeafRun → install_admitted_run`으로 연결돼 있고
  일반 install의 metadata 재준비를 우회한다. 두 물리 group의 reserve →
  previous confirm+next reserve → last confirm은 Catalog 3 transaction이다.
  이를 새 미연결이나 단순 중복 reserve로 발급할 근거는 없다.
- Native daemon은 단일 Kernel/Catalog read session을 공유한다. 기존 동시 관측
  grouping을 유지하며 순차 inode/pin/clock 관측을 삭제하거나 전역 group을 다시
  추가할 근거는 없다.
- Posting 사전의 각 miss는 별도 `reserve_for_write`를 호출한다. 일반 capacity
  검사에는 page_count/freelist_count/page_size/max_page_count PRAGMA가 있다.
  양측 값이 모두 고유한 128행 packet에서는 최대256번의 검사 경로를 탈 수 있다.
  이 수는 source 기반 연산 상한이며 현재 corpus의 miss 비율·시간 실측이 아니다.
  정확한 miss의 합계로 한 번 예약하는 변경은 가능하지만 중복·기존 hit·NULL·
  hard cap·부분 실패·fee·취소를 보존해야 한다. 성능 근거 없이 미리 채택하지 않는다.

현재 반복되던 검증 공백은 35.2% pager footprint 절감을 실제120초 개선으로
연결하지 못했다는 점이다. 새 후보를 먼저 실행하고, host wall−CPU만으로
호스트 탓 또는 코드 탓을 확정하지 않는다. 실행 순서는
[WORKPLAN](WORKPLAN.md#posting-product-rca-20261110)을 따른다.

<a id="posting-value-normalization-20261110"></a>
## 2026-11-10 Posting 사전 정규화 — 확정 중복과 미확정 성능

### 확정된 비용과 배제한 가설

Posting scratch는 이미 단일 `BEGIN IMMEDIATE` 안에서 작성한다. batch마다
COMMIT한다는 가설은 코드와 맞지 않는다. 기존 r56의 main xWrite
118,718회 × 4,096B = 486,268,928B도 동일 페이지 재쓰기의 증명이 아니다.
역색인 확장·최종 pager 점유·실제 성공 행 수를 구분해야 한다. lane35의 첫8회와
2의 거듭제곱 표본 20개를 전체 SQL 실행 수 또는 전체 시간으로 외삽하지 않는다.

확정 중복은 `(term, owner)`의 여러 행에 같은 33B object-reference wire를
반복 저장하는 구조다. 기존 bind 공유는 SQL parameter 수를 줄이지만 저장
payload를 공유하지 않는다. 반대로 term별 ref가 다른 경우도 있으므로 owner만
키로 삼아 p/n을 합치는 최적화는 허용하지 않는다.

### 구현과 실패 경계

변경은 disposable scratch에만 적용한다. `posting_values(id, wire UNIQUE)`에
정확한 canonical wire를 intern하고 `posting.p/n`에는 양의 ID 또는 원 NULL을
저장한다. `posting_edits`의 LEFT JOIN은 기존 wire를 반환한다. root/range/sparse
reader는 원 ID type·양수 여부와 해당 wire의 존재를 먼저 검사한다. dangling ID가
NULL edit로 바뀌거나 malformed row가 JOIN에서 사라지지 않는다.

writer는 모든 row callback·canonical encoding·순수 binding을 완료한 뒤
intern을 시작한다. 기존 preexecute 거절은 SQL mutation을 시작하지 않는다.
첫 intern 또는 실제 `raw_execute` 진입 뒤 오류는 spool을 poison하고 원 오류를
반환한다. 정규화 후보 최종 리뷰에서 all-NULL batch는 intern을 호출하지 않아 부분 SQL 실패
뒤 재사용될 수 있음을 발견했고, 별도 `before_execute` callback으로 닫았다.
두 번째 posting INSERT에 FAIL을 주는 회귀는 이 경우의 원 Storage 오류와
후속 write·root·이미 buffered된 range/sparse·receipt 거절을 확인하도록 작성했다.

사전 exact lookup과 INSERT RETURNING은 vendored rusqlite의 `query_one`으로
단일 행 뒤 DONE까지 확인한다. 첫 ROW만 반환하고 reset 오류를 버리는 경로를
사용하지 않는다. NULL은 사전을 조회하지 않는다. 기존 posting work fee는
보존하고 dictionary SELECT·INSERT와 capacity를 추가 청구한다. 임시 value binding
참조 배열과 batch-local 두 ID 메모의 전체 `size_of`는 사용 전에 별도 grant를
얻고 종료 시 반환한다. 원 cache/cap/120초·durability를 늘리거나 오류 뒤 budget을
되돌리지 않는다.

두 owner/ref group이 한 packet에 섞이면 전체-column 공유 판정은 false다.
그 상태에서 단순 사전 정규화는 같은 wire를 행마다 조회하는 새 반복을 만들었다.
고정 512×64 회귀의 128행 batch는 정확한 고유 값이 두 개다. 이를 batch 안의
두 borrowed `MapValue`→양의 ID로 재사용하며, 값 동등성으로 비교하고 intern과
후속 checkpoint가 성공한 ID만 남긴다. hit에서도 원 취소 checkpoint를 유지한다.
메모는 호출 간 보존하지 않고 실제 타입 크기를 위 grant에 포함한다. 원 행 fee는
유지하고 실제 생략한 사전 SQL의 추가 fee만 발생하지 않는다. 두 next 값 교대와
별도 shared prior를 두 batch에서 반복하는 oracle를 추가해 호출 간 누수·추가
callback·잘못된 binding을 확인하도록 했다. 이 마지막 수정도 아래 300-body 실행에
포함됐고 해당 회귀는 통과했다.

### 관측과 수용 한계

격리 source `/Users/songmin/.codex/qidx-rca-snapshot-etX5Ij/source`의 base
`345cf03966773d259ab559803a9414f51df85711`에 정규화 6파일을 적용한 첫 후보는
nextest `7ff83a46-a7ac-42ce-bdf2-e002a0e75960`에서 **26/26 PASS**, 992 제외였다.
초기 test module 경로/중복 import·미사용 반환값의 compile 실패는 body0이며
그 후의 실행과 구분한다. 최신 execution-start callback과 batch-local 메모 수리는
이 실행 이후다.

독립 저장 회귀는 512 owners × 64 terms = 32,768 Posting 행을 원 BLOB writer와
새 writer에 넣고 정렬된 전체 canonical wire를 고정 gold와 대조했다.
page_count × page_size는 기존 2,981,888B, 정규화 1,933,312B로 1,048,576B
(35.1648%) 줄었다. 중복 wire 1,081,344B는 사전 512행의 16,896B가 됐다.
이 값은 uncommitted page를 포함한 **pager footprint**다. allocated st_blocks,
xWrite bytes·RSS·시간 절감률이 아니며, XL source 수와 Posting 행 수도 다르다.
다섯 reader query의 PK 순서·임시 B-tree 없음, 64/513 경계도 집중 회귀에 포함된다.

tradeoff는 사전 조회·삽입과 최대 두 ID JOIN의 추가 비용이다. 고유 값이 많은
입력에서 저장 이득이 작거나 원 work budget이 먼저 소진될 수 있으므로 실제
SourceAdvance의 accepted work와 ACK/search/reopen을 확인해야 한다. 현재
용량·정확성 증거만으로 XL120 해결 또는 전체 성능 P0–P2 부재를 선언하지 않는다.

공유 main 통합 시 기존 `map_edit_sql_memory_bound`, `MemoryCreditLimits`,
packet allocation/confirmation bound 변경을 보존했다. 이 별도 supplier 패킷을
다시 구현하지 않았다. `occurrence_pack::prepare`의 scoped credit는
`run_owner_blocks` 반환 시 끝나고 Posting은 이후 `finish_change`의
`diff_inventory`에서 flush된다. 따라서 ordinary inventory SQL bound에 Posting
메타데이터를 섞어 추가하지 않았다. 이는 소스 수명 확인이며 main 전체 런타임
검증은 아니다. 두 fixture migration을 포함한 최종 소유 파일은 8개다.

### 마지막 수정의 실행과 기존 fixture 실패 분리

일반 Rust 회귀의 첫 canonical 대기 세 건은 각각 570.896초·396.945초·366.809초
뒤 취소돼 body0이었다. 살아 있는 예약·다른 프로세스·target은 변경하지 않았다.
이후 메모리 여유37%·디스크54GiB를 확인하고 일반 회귀만 front door의
`QUANTA_INDEX_RESOURCE_ADMISSION=0 CARGO_BUILD_JOBS=1`로 실행했다. nextest도
1 thread다. 제품 timing preflight는 해제하지 않았고 아래 수치는 host 성능 수용이 아니다.

| 실행 | 관측 |
| --- | --- |
| `f62cff9a-acea-4e15-abf8-9d3bcac6fc9a` | 300 선택, 185 PASS/1 FAILED 뒤 fail-fast; 114 미실행. 마지막 callback·메모 포함 컴파일 완료 |
| baseline `3a62a2c1-4547-4b18-a492-709c63026047` | 정규화 전 base345로 원 source 5개만 일시 대조한 body1도 같은 token fixture `ResourceLimited`. 종료 후 후보 source bytes 전부 복원 |
| `5aa7c3e6-37fc-429c-9b51-139346708dd7` | no-fail-fast 300 실행, 297 PASS/3 FAILED, 720 제외 |
| `4549745e-3a54-48fd-ab91-abb1eeb77550` | fixture 2개 migration 뒤 300 실행, 299 PASS/1 FAILED, 720 제외. 새 Posting·Scratch·malformed/poison·메모 및 cursor 회귀 통과 |

남은 baseline fixture는 `widened_cold_optional_memory_denial_leaves_sql_fallback_fundable`다.
2,048 tokens × 2 surfaces = 4,096 token rows를 write limit1,024의 `packet_budget`
안에서 생성해 Posting 실행 전 거절된다. 공유 main에는 이미 해당 담당자의
짧은 token 수·긴 key 입력 수정이 있다. 같은 코드/fixture를 다시 구현하거나
제품 work budget을 늘리지 않았다. 그 main 수정은 이번 격리 source에 없으므로
299/300을 전체 suite 성공으로 표기하지 않는다.

다른 두 실패는 테스트 adapter의 저장 가정이었다. `edit_pages`는 기존 C 공유
p/n numbered SQL을 positional SQL로 세어 실행0으로 오판했고, 고정 입력의
독립 numbered SQL로 실제 INSERT 한 번을 계속 검증한다. `posting_cursor_ranges`는
physical p/n을 BLOB으로 읽었다. production view를 재사용하지 않고 dictionary를
직접 LEFT JOIN해 원 129행 fixed wire gold·PK 순서·ordinal oracle를 유지했다.

Scratch/Lexical `clippy --no-deps --lib --tests -D warnings`는 Lexical lib의
218개 지적으로 exit101/FAILED였다. 이 때문에 tests lint까지 도달하지 못했다.
소유 지적을 추출하는 진단용 cap-lints 실행은 전체 strict green을 만들지 않는다.

후속 수정은 테스트 HashSet을 deterministic BTreeSet으로 변경하고 helper의
Option mapping을 caller로 옮겼다. 이전 Q3 Scratch 계측 테스트는 poison mutex를
복구해서 읽는 대신 오류로 전파하고 guard를 마지막 사용 뒤 해제한다. Result
oracle는 panic 대신 오류를 반환하며 원 6-counter·failed SQL zero 조건은 보존한다.
최종 소유 selector는 `964c18eb-f5a9-4e4a-91cb-868000456340`에서 **74/74 PASS**,
946 제외·exit0이다. 앞선 `fb37d58e-52c7-4252-8442-9dbc94c28ff4`의 74개와 합산하지 않는다.

같은 최종 격리 후보의 Scratch `--no-deps --lib --tests -D warnings`는 exit0,
진단0으로 **VERIFIED**다. Lexical lib/tests의 `--cap-lints warn` 진단은 총599개,
이번 변경 줄의 primary span 지적0이었다. 이 exit0은 strict 수락이 아니며 전체
Lexical의 기존 strict 실패는 남는다. 최종 main 8파일 scoped rustfmt와 전체
`git diff --check`는 exit0이다. main과 격리 source의 차이는 보존한 memory-credit
supplier helpers/imports·그 테스트다. 이 차이와 main 전체 Cargo/SDK를 격리 후보의
통과로 수용하지 않는다. 새 정규화 후보의 Large/XL120·Mixed 실행은 `NOT_RUN`이다.

Mixed의 실제 factory→dispatcher→ticket 경로는 단일 Kernel과 Catalog OS read
session을 공유한다. OS session 간 그룹 확장은 해당 경로의 원인 수리가 아니다.
Catalog wall−thread CPU는 I/O·lock·deschedule을 함께 포함하므로 호스트 경합만의
수치로 해석하지 않는다. Mixed Record는 현 harness가 명시적으로 거절하므로
Mixed Off 요청 진단과 별도 XL Record VFS를 같은 요청의 인과 trace로 합치지 않는다.


<a id="xl-mixed-resume-20261110"></a>
## 2026-11-10–11 XL·Mixed 재개 — 실제 경로와 검증 경계

r57 XL의 직접 거절은 원 server120초 `BuildPrepare` deadline이다. 마지막
object checkpoint는 지배 비용의 증명이 아니다. Mixed의 첫 query queue/IPC,
마지막 old-view witness와 일부 Catalog COMMIT 표본도 서로 다른 요청이다.
한 표본을 전체 실패의 내부 원인이나 호스트 영향으로 확대하지 않는다.

실제 경로에서 `KernelServices::new`는 `catalog.open_read_session()`을 한 번
호출하고, ticket 획득은 같은 `services.read_session` Arc를 복제한다
(`kernel/src/update.rs`의 생성자·두 ticket 획득 경로). NativeDispatcher도 단일
Kernel Arc를 공유한다. 공개 Native read session/ticket 수와 Catalog OS session
수를 구분해야 한다. r57도 이 경로를 사용하므로 **OS 세션 간 COMMIT 병합 부재는
이 Mixed 부하의 원인이 아니다**. 이번 시도에서 추가했던 Catalog 전역 그룹과
전용 회귀4개는 자기 변경 hunk만 제거했다. 그 실험의 통과를 Mixed 수리나 최종
코드 검증으로 재사용하지 않는다. 기존 per-session group·각 durable clock 관측,
원 ticket/inode/fence·취소·budget·pin·정산 기준은 보존했다.

유지한 수리는 누락된 HybridFile typed IPC repo-scope 분기를 기존 Hybrid의
명시적 generation authority에 연결한 호출부와 실제 strict 지적6건이다.
peer-watch 지역 type 위치와 queued-peer fixture의 불필요한 값 전달,
managed-install 문서 표기와 Result 테스트의 panic을 정리했다. 관련
Catalog/IPC tests strict는 exit0이었고 영향 회귀3/3
(`cbee8544-cc04-477b-b766-56bb55dca04e`)도 통과했다. 전역 그룹 제거 뒤
기존 Catalog 관측 회귀26/26(`64f9395d-cd28-49c4-ab93-aa90beff6c34`)과
Catalog tests strict exit0을 다시 확인했다.

첫 API 컴파일은 위 IPC 분기 누락으로 body0이었다. 초기 전역 그룹은 재오픈
메모리 회귀와 empty-Weak oracle 교정을 거쳤으나 실제 Mixed 경로와 맞지 않아
채택하지 않았다. source `ad87f615d124d3995fb1e0c7eee839e14fabfc66`의 canonical
bench pair build는 exit0·source/config/deps/tools/vendor lock 전후 일치를
확인했지만 해당 실험을 포함하므로 최종 제품 후보로 사용하지 않는다. 무시된
vendor Cargo.lock6개를 빠뜨린 첫 준비는 custody 검사에서 컴파일 전에 거절됐다.
후속 후보는 실제 유지 소스로 다시 동결하며 runtime code와 무관한 관측 실험을
성능 개선으로 계산하지 않는다.

전역 그룹을 제거한 새 동결 후보 `18da8e5c1160a16eff1adc7c75331af7aa003bda`의
컴파일은 cold inventory helper의 `spool` 이름 누락으로 `FAILED`/제품 body0이었다.
`run_owner_blocks`는 원래 `PackTarget.spool`을 사용해야 한다. helper 분리 전의
원 참조를 closure 생성 전에 복사하도록 연결했고 다른 spool 생성·budget 변경·
검증 생략을 추가하지 않았다. 해당 한 줄만 고정 source에 반영한 후속 후보는
`18f6f89d16651c6ad48170ee9e02ab58aa641afb`다. SQL 기준·동일 메모리 상한·
Unicode 고정 gold 회귀와 canonical bench pair build는 다음처럼 구분한다.

- **VERIFIED — pair build**: `./scripts/cargow --lane bench-lane build
  -p quanta-index-searchd-harness --bin scale_matrix -p quanta-index-searchd-runtime
  --bin quanta-index-searchd --all-features --locked --offline --profile bench
  --message-format=json-render-diagnostics`가 exit0이었다. source/config/deps/tools/
  vendor lock 전후가 일치한다. 사본은 checkout 밖 `build4`이며 scale SHA는
  `b3e41a2bfc66c41fe9b6e3c5689263b955f5475028d0928bf94d2e358f2b207f`, daemon은
  `d551a95d8a6409c721caba1c1ef7c00aa9110ecbf3a7383223dcfc33cdf4ac17`다.
  일반 cached bench build이며 managed-fresh/release 수용으로 승격하지 않는다.
- **첫 영향 실행 — 11 PASS/1 FAILED**: canonical `test-workspace-lane nextest run
  --locked --offline -p quanta-index-lexical -p quanta-index-searchd-runtime
  --all-features --lib`, 고정 selector의12개를1 thread/no-fail-fast로 실행했다
  (`c901aedb-6fce-41ab-a4ba-65331de3be71`). 동일 상한의 nonalias SQL 경로,
  Unicode fixed gold/optional fallback·directory overlap 거절과 Runtime memory-credit
  7개는 통과했다. trait import·private scan 대신 공개된 해당 EntryMap 경로 사용,
  atomic must-use·unused-mut의 test-only compile 수리가 본문 실행보다 앞선다.
- **실패 RCA·수리 — 해당1/1 PASS**: `cold_arena_does_not_starve_mandatory_packed_output`
  fixture는4단어의 sensitive/folded8개 term을 만들었지만 production alias 최소는9다.
  `try_packed_alias`의 `None`을 fixture가 `ResourceLimited`로 바꿔 메모리 검증 전에
  실패했다. 고정5단어/10개 term·500 positions로 수정했고, alias 부재는 명시적
  fixture 계약 위반으로 분리했다. 원 최소9·생산자 상한·실제 필요량보다1바이트
  작은 ceiling과 SQL 대조·grant 반환 검증은 유지했다. `-p quanta-index-lexical
  --all-features --lib -E 'test(cold_arena_does_not_starve_mandatory_packed_output)'
  --no-tests fail --test-threads 1`의 수리 후 실행
  `dfd57fe1-2437-487a-86eb-983b6fdf82bd`는 exit0/1 PASS다.
  수정 보호검사 실패 뒤 잘못 이어진 수정 전 재실행1회는 새 검증에 합산하지 않는다.

위 test-only 수정은 공유 main에 있으며 동결 pair의 제품 소스를 바꾸지 않는다.
정확한 build/제품 귀속은 위 source와 외부 경로
`/private/tmp/qidx-xl-mixed-rca-z1a_82he/build4/terminal.json`을 따른다.
수정한8개 Rust leaf의 `rustfmt --check --edition 2024 --config skip_children=true`와
해당 파일·세 SSOT 문서의 `git diff --check`는 exit0이다. 전체 suite/strict 수용으로
확대하지 않는다.

`just rust-hexagonal`은 exit0이었다. 후행 protected module 검사는 `FAILED`다.
Contract GoTestTrace context/HybridFile과 Core file-rank·MemoryCredit·
ObjectInstallMemoryShape 추가가 baseline과 달랐고 Kernel tree는 일치했다.
이 별도 구현의 baseline을 자동 승인하지 않는다.

첫 XL 입장 `x1`은 canonical timing preflight의 별도 Cargo/rustc6개와
load15.450195(limit8)로 `BLOCKED`였다. 제품 본문0이며 Mixed도 미실행이다.
이후 확인도 별도 Rust 실행과 host load로 입장 거절됐다. 원 XL120초 또는
Mixed50QPS·1Hz·960초·검색p95≤100ms/p99≤500ms·freshnessp95≤1초/p99≤5초·
pin600초·settle60초는 아직 `NOT_RUN`이다. r57의 `FAILED`를 유지한다.
2026-11-10 05:22–05:32 KST의 전용 대기도 제품 시작 없이 차단됐다. 마지막
canonical preflight는 별도 Rust4개·load17.115723(limit8), exit1/`blocked`다.
해당 대기 프로세스만 종료했고 `x2`/`m2` 실행·control 경로는 모두 없다.
`gate-wait-terminal.json`은 입장 차단 기록이며 제품 실행 결과가 아니다.
대기 중 lock 선점·별도 Rust 프로세스 중단·잠금 삭제·contended override는 없었다.

05:36 KST 재개 요청 뒤 source/config/tools custody와 두 binary SHA를 다시 확인했다.
main의 D 진단·관측7개 파일과 `occurrence_pack`도 후보와 byte 동일하다. 새 build나
통과한 회귀를 반복하지 않았다. 단일 controller가 원 timing preflight `clean`을
기다리며, 이때 관측은 별도 Rust6개·load17.31543(limit8)/`blocked`였다.
동시에 진행 중인 Posting 후보 build는 Core memory·Native helper·Runtime 입력이
다르므로 성공하더라도 이 source의 검증으로 전용하지 않는다. 제품은 아직 미실행이다.

단순 대기의 입장 문제는 프로젝트 `build-test.lock`이 호스트 전체 Rust 입장을
직렬화하지 않는 경계다. 별도 호스트 작업들이 새로 들어와 preflight가 계속 거절됐다.
35분10.735초 뒤 읽기 전용 대기 controller를 정상 종료했다. `x2`/`m2` 실행·control
경로는 모두 없었고 제품 중단·재실행은 없었다.

06:17 KST부터 기존 호스트 권한인
`quanta_build_cli._compile_activity_lock_v1(mode="exclusive", wait_seconds=3600)`를
사용했다. API는 admission EX를 먼저 잡고 실행 SH 소유자의 반환을 기다린다.
새 shared admission의 nonblocking 획득이 거절되는 것을 확인했다. 이 순서는
새 호스트 컴파일의 연속 입장에 밀리는 문제를 다루며, 기존 실행은 취소하지 않는다.
호스트 execution EX 뒤 프로젝트 prebuilt leaf를 잡고 원 preflight를 다시 검사한다.
원 gate가 계속 거절되면 제품을 시작하지 않고 예약을 반환한다. 제어 상태는
`/private/tmp/qidx-xl-mixed-rca-z1a_82he/host-exclusive/status.json`이다.

입장 도구 준비에서 Python의 `tomllib`·`blake3`·`unicodedata2` 의존성 차이가
드러났으며 모두 잠금 획득·제품 실행 전이었다. 호스트 admission은 기존 QBC
Python에서, source/tools custody와 제품은 receipt의 기존 QIDX Python에서 별도
프로세스로 실행하도록 정리했다. 제품 Python의 원 binary SHA와 source/config/tools·
daemon/harness SHA를 다시 확인했다. 의존성 우회나 새 제품 build는 추가하지 않았다.

### `18f6f89d…`의 실제 XL·Mixed 결과

전역 execution EX와 프로젝트 leaf 아래 원 CLI로 `x2` XL Off와 `m2` Mixed Off를
각1회 실행했다. preflight는 각각06:47/06:51 KST `clean`, 별도 Rust0,
load4.723633/4.654785였다. source/binary/tools custody는 전후 일치하고
`dirty_after=false`, timeout도 없었다. 주기 host 관측의 별도 Rust도0이었다.
두 실행은 producer exit1·`UNVERIFIED`/`FAILED`이며07:03 KST 예약을 반환했다.
로컬 prebuilt capture의 `build_binding=UNPROVEN`을 정식 release 수용으로 바꾸지 않는다.

| 실행 | 실제 실패와 범위 |
| --- | --- |
| XL `x2` | StageSources88.981초 완료 뒤 full ACK136.552초 미완료. 원 server120초의 BuildPrepare deadline이며 마지막 checkpoint는 `storage/src/object.rs:865`. accepted reads3,707,595/writes12,898,307/bytes10,895,598,466/mutations17,241,442, memory/charge 거절은 없었다. 전체238.568초나 마지막 checkpoint를 원 server 예산·지배 비용으로 해석하지 않는다 |
| Mixed `m2` | 최초 ACK23.118초·첫 검색61.414ms 및 delta/noop/delete/old-pin 전단계는 완료. 본문은 held-read 정산60초 실패로 종료. 전체700.060초는960초 완료가 아니다 |

Mixed 마지막 pin은600.486초에 해제됐고 `confirmed_last_pin=true`다.61회 정산 poll의
마지막 floor/head는318/318, target pin은 없었으나 `Pending`이었다. 보존된 typed
blocker는 `UnretiredView`, `ReclaimQueue`, `TerminalCustody`다. custody ready 순번은
26950에서27342로 진행했고 후행 표본의 oldest operation은 최신 발행 근처였다.
처음·마지막 표본만으로 전체 정산 시간이 한 blocker에 속한다고 단정하지 않는다.

검색은 completed22,082/failed21,562다. 고정 typed 분류는 세 query class 모두
`sdk_transport`이며, 이를 내부 서버 원인으로 세분하지 않는다. 첫 실패 task31은
unique, 전체2002.031ms 중 queue1708.195ms였고 SDK는 IPC Read291ms timeout을
보고했다. 예정33,050개 중 queue-full10,905개, 원 전체 예정48,000개는 미완료다.
발행은 warmup29/60 ACK, measure285/601 ACK·missed316이며 측정 freshness
p95=3160.801ms/p99=3960.932ms다. 정산만 고쳐도 검색·발행 수용이 되는 것은 아니다.

관측된 retention 누적 표본은 floor rounds19,020·custody pages5,919·rows183,686이었다.
이는 선택된 진행 표본이며 최종 총량으로 승격하지 않는다. 코드에서는 terminal
custody 삭제가 참조가 남은 LIVE 객체도 무조건 GC 후보에 넣고, 이후 GC retire가
`reference_exists`를 확인해 별도 writer transaction으로 dequeue한다. 같은 삭제
transaction에서 남은 참조를 정확히 확인하면 이 왕복을 제거할 수 있다. 마지막
참조 제거의 재등록, 기존 queue/수명 checksum·fence·rollback은 그대로 검증해야 한다.
이 경로의 존재와 해당 후보·정산 성능의 지배 비용은 별도 주장으로 유지한다.

### 참조가 남은 LIVE 객체의 새 후보 생성 수리

기존 public Catalog API로 만든 회귀는 nextest
`b87dd252-61fa-4ace-babc-154be605eeef`에서1/1 **FAILED**였다. 다른 durable ingress가
같은 객체를 소유하는데 terminal release 뒤 queue에1개가 생겼다. 기존 GC retire는
그 참조를 확인하고 별도 writer commit으로 다시 dequeue한다.

`reclaim::enqueue_unreferenced`는 기존 lifecycle·queue checksum 검증을 공유한다.
기존 후보가 있으면 그대로 두고, 새 `LIVE` 후보에만 같은 삭제 transaction의
`reference_exists`를 적용한다. 양의 참조가 없거나 비-LIVE이면 원 enqueue 경로다.
원 cleanup fee는 유지하고 추가 reference read는 원 `WorkBudget`에 별도 과금한다.
상한을 올리거나 최종 read-settlement 판정을 줄이지 않는다. 마지막 참조의 삭제는
기존 release/enqueue 경로를 유지한다. 이미 GC retire가 참조가 있는 후보를
dequeue하므로, 이 마지막 참조 재등록은 원래부터 필요한 liveness 계약이다.

수리 후 `test-workspace-lane nextest run --locked --offline -p quanta-index-catalog
--lib`, protected_queue/reclaim/read_settlement/terminal_custody 선택43개가
`118b2249-3fed-4202-a91b-9e60d46cd815`에서43/43 PASS했다. 새5개는 reopen·실제
마지막 참조 제거, 기존 후보 보존과 실제 retire의 참조 확인, corrupt queue의
rollback, reference SQL 오류의 rollback, 살아 있는 parent edge의 자식 보호를
검증한다. 원 코드의 실패와 수정 후 성공을 별도 추가 본문으로 합산하지 않는다.

Catalog lib/tests strict는 첫 실행에서 문서 첫 문단 길이·테스트 SQL guard의 이른
해제2건으로 실패했다. 두 지적만 고친 뒤 canonical Clippy
`--locked --offline --no-deps -p quanta-index-catalog --lib --tests -- -D warnings`는
exit0이었다. 수정3파일 rustfmt/diff도 exit0이다. 문서·테스트 lock 수명 보정은
생산자 동작을 바꾸지 않아43개 본문을 다시 실행하지 않았다.

Posting/Scratch와 cold-output 결합 회귀는 canonical
`nextest run --locked --offline -p quanta-index-lexical
-p quanta-index-scratch-sqlite --all-features --lib --no-tests fail
--test-threads 1 --no-fail-fast`의 고정 selector에서81/81 PASS했다
(`c8f6565c-a6d2-4599-8e11-93bc2b7bba4d`,957개 제외).
selector는 Scratch 전체 lib, posting_sql/edit_pages/posting_cursor_ranges,
malformed 거절, 고정5개 cold/Unicode 메모리 회귀다. 전체 lexical 검증은 아니다.

그 뒤 추가된 canonical cold arena→SQL replay가 context를 다시 빌리는 위치에서
E0499가 발생했다. `if let`의 임시 `Option<ScopedMemoryBudget>` 소멸 전까지
첫 budget 가변 참조가 유지되는 컴파일 오류이며 새 테스트 본문은0개였다.
기존 reset/replay·과금·실패 전파는 유지하고, replay 성공 bool을 결정한 뒤
그 임시 값의 수명이 끝난 바깥에서 context를 재사용하도록 수리했다.
같은 canonical lexical `--all-features --lib`에서
`install_envelope`, `token_spool_batch`, `cold_tokens::tests`만 선택한
`62d6ddc7-91c1-4b08-8751-a77dadb808dd`는24/24 PASS·973개 제외였다.
물리 packet 상한·pending payload/flush peak·실제256-input 설치·잘못된 최종 output,
고정 Unicode/SQL·같은 memory ceiling·취소/메모리 환급을 검증한다.
81과24의 겹치는 본문은 새 성공 수로 합산하지 않는다. 별도로 canonical replay의
Unicode/반복 position 대조 및 packet/SQL credit 거절·취소 전 무효과2개는
고정 source `5923c719…`에서2/2 PASS했다
(`7d124c2d-b83e-4a78-9c21-edf6289e8831`,995개 제외).

동일 source의 canonical bench pair build는 `FAILED`/exit101·제품 body0이었다.
`ingest_dispatcher/staged_admission.rs:54`의 `HashSet::insert` 반환값을
`-D unused-results`가 거절했다. `let _inserted = ...`로 반환값만 소비하며
admission의 중복 repo·slot/byte quota·release 동작은 바꾸지 않았다.
실패 당시 source는 `refs/heads/codex/xl-mixed-source5-compile-failure`와
원 `build5/{inputs,terminal}.json`, compiler stdout/stderr에 보존한다.
제품을 실행하지 않은 source5 checkout에서 해당 한 줄만 새 commit한 revision은
`85f90d3c9eafa25dd2f614428f7fc57ff2da5cfc`다. 새 build6는 그 revision의 입력을
다시 결속하며 실패한 build5를 수정하거나 성공으로 승격하지 않는다.

`85f90d3c…`의 build6는 같은 canonical bench pair 명령에서 **VERIFIED**/exit0이다.
source/config/deps/tools/vendor lock 전후가 일치했다. scale SHA는
`c88801b3ef5af2d04661b9ecc0df7492f282994de4d2ecb6529259d618e97555`, daemon은
`629611532b7cf574794987a57dfb865f2ad066eb13dd48b8494bb03e3db1b8a7`다.
일반 cached bench build이며 managed-fresh/release 수용은 아니다.

다음 admission 회귀의 lib test compile은 `idempotency.rs`의 미사용 trait import2개와
성공 receipt 반환값 미소비2개로 body0 **FAILED**였다. main의 해당 test-only4곳만
수리했다. production admission 모듈은 고정 후보와 바이트가 같다.
canonical `nextest --locked --offline -p quanta-index-search-plane --all-features
--lib`, staged_admission의3개와 `staged_commits_overlap_across_repos_and_refuse_same_repo`
1개를1 thread/no-fail-fast로 선택한
`814c54d9-7cda-452d-b29c-a737a855ef52`는4/4 PASS·623개 제외다.
slot/byte quota·동일 repo 거절·다른 repo 동시 발행·취소/실패/unwind 환급을 확인했다.
이 test-only 수리로 제품 source5/pair는 바꾸거나 재빌드하지 않았다.

18:02 KST source/binary/tools 재검사를 통과한 단일 controller가 원 canonical host
admission EX 입장을 기다리기 시작했다. 현 `x3`·`m3`는 **NOT_RUN**이며 기존
별도 실행의 잠금을 취소·삭제하거나 host preflight를 우회하지 않는다.
제어 경로는 checkout 밖 `host-exclusive6/status.json`, 후속 제품 종결 원장은
`products6-terminal.json`이다. 새로운 제품 성공이나 성능 효과는 아직 관측하지 않았다.

18:09 KST 원 `check_host_contention.py --expected-os darwin`의 독립 입장 점검도
**BLOCKED**였다. 구판 검사는 별도 Rust1개로 보고했고 load44.555176이
원 한도8을 넘었다. 이 점검은 제품 본문이 아니며 x3/m3 실행 수는0이다.
argv 기반의 구판 Rust 보고를 실제 실행의 독립 증거로 사용하지 않는다.
호스트 예약 대기만의 문제가 아니므로 contended override나 별도 제품 경로로
우회하지 않는다. 원본은 checkout 밖 `original-admission-assessment-85f9.json`이다.

18:22 KST 대기 중 current main과 대조해 Native Text·EntryMap·Scratch의 후행 생산
변경을 확인했다. 자기 host controller만 SIGTERM으로 종료했고 exit143·제품0회다.
이는 제품/컴파일 실패나 입장 timeout이 아니다. x3/m3/control 경로가 생성되지 않았고
`products6-terminal.json`도 없다. source85f9는
`refs/heads/codex/xl-mixed-source6-ready`와 원 build6 pair/입력에 보존했다.

미실행 checkout에 current main을 정확히 복사한 후속 source는
`cb1c07ebd7eb6d870a29de6752c04e19eb62aab9`다. 입력3189개·vendor lock11개·
복사 전후 drift0이며 실제 실행했던18f snapshot은 변경하지 않았다.
기존 alias checked-DATA handoff·late failure poison·Posting SQL shape와
directory interval 검증을 재구현하지 않고 소비한다. 해당 Native Text/Spool·Scratch
영향 회귀와 새 source-bound pair를 별도 실행하며85f9의 통과를 새 source로 승격하지 않는다.

### 후행 Native cohort의 실제 compile·본문 회귀

`cb1c07eb…`의 Native Text/Spool·Scratch `--all-features --lib` 배치는 exit101/
body0이었다. typed alias가 generic observer의 context 수명을 통해 빠져나가는
E0521, fixture 정수 타입 추론·기존 `PreparedRows` API 사용과 작성 중 진단 fixture의
미사용 import가 있었다. 후행 진단 fixture는 실제 사용을 포함한 입력으로 소비했고,
span 실패 시의 명시적 diagnostic mask는 같은 호출 scope에서 구성해 verified DATA의
attempt 수명을 유지했다. 정수 타입과 `as_slice().is_empty()`만 fixture에서 보정했다.
후속 `b96a357c…`의 test-only unused-mut2건도 실제 body0 실패로 남겼다.

두 fixture만 수리한 `e6f8a540…`는 nextest
`2428050e-575b-4c74-9008-051189c576f2`에서332개 실행,331 PASS/1 FAILED·721개 제외다.
실제 실패는 `inventory_batch_admits_metadata_before_leaf_work_and_keeps_attempt_control`의
기존 `FenceLost` 오라클이다. 새 `self.failed` 검사가 foreign attempt보다 먼저
`BackendViolation`을 반환했다. 기대 오류를 바꾸지 않고 생산자 guard를 원
cancellation→foreign fence→same-attempt poison 순서로 수리했다.

수정 후 source `8af31cb57c2c6c1d39fd712be1e854f67c758428`의 canonical lexical
`--all-features --lib`에서 해당1개와 alias_handoff6개만 선택한
`4bbc3917-b394-4161-a721-383eb60cbe71`는7/7 PASS·999개 제외다. late alias/diff 실패,
inventory invalidation 뒤 buffered cursor/writer 거절, retained 변경/거절·취소와
attempt 불일치, 실제 block당1회 get 오라클을 유지했다. 331개 전체를 재실행하거나
7개와 추가 성공 수로 합산하지 않는다.

수정 보호 검사 실패 뒤 수정 전7개가 잘못 이어진 중복1회가 있었다
(`956f9572-4618-4c98-b680-13337fb99abb`,6 PASS/1 FAILED). 실행 연결 오류이며
수리 후 검증이나 새 성공으로 계산하지 않는다. source9/실패 기록은 보존한다.
검증된8af31cb5 cohort에서 build7로 source/config/deps/tools와 pair 결속을 시도했다.
원 XL·Mixed 본문은 아직 NOT_RUN이며 이전 제품 FAILED는 그대로다.

build7은 `ipc/src/source_upload.rs:932`의 `pending.remove(index)`가 반환한
`PendingCleanup`을 `-D unused-results`가 거절해 exit101/제품 body0이었다.
기존에도 그 statement 끝에서 즉시 폐기되던 값이므로 명시적 `drop(...)` 한 줄만
반영했다. 후속 source는 `55ce973f0e85da8e25a7e5db38c03562f8909334`다.
Source10·build7 실패 기록은 보존하고 Native 본문은 재실행하지 않는다.
원 scale.rs의 유일한 차이는 cfg(test) outcome의 `upload_cleanup: None` 추가다.
제품 부하·deadline·pin·정산·resource/budget/clock 기준은 그대로다.

같은 canonical bench 명령의 build8은 **VERIFIED**/exit0이다.
source/config/deps/tools/vendor lock 전후가 일치했고 source는 dirty가 아니다.
harness SHA는 `dbd53c77119542839c0b3f820161c43f5fb281f211dcfb194e5d7b0b8c024ce4`,
daemon SHA는 `a734c08f88436da8065c417ea412f3acbab9afd292b83e9b0d58b62a6367b33c`다.
일반 cached bench/source-bound local 검증이며 managed-fresh/release 수용은 아니다.
수정 생산 코드6파일의 scoped rustfmt check도 exit0이었다.

20:03 KST 원 source/binary/tools 재검사를 통과한 단일 `host-exclusive8` controller는
canonical host admission EX 뒤 기존 execution SH 반환을 기다렸다. 이 source의
x3/m3 제품은 **NOT_RUN**이었으며 global execution→프로젝트 leaf→원 preflight를
모두 통과한 뒤에만 시작한다. 기존 작업은 취소하지 않고 검증 중 cohort를 후행
shared-main 변경으로 교체하지 않는다. source55 결과를 다른 source로 승격하지 않는다.

22:40 KST host-exclusive8의 canonical wait3600초가 끝나
`CompileActivityBusyError`/exit1이었다. 이는 제품 실패가 아니라 입장 BLOCKED다.
프로젝트 leaf와 x3/m3/control·products8 종결 원장 모두 생성되지 않아 제품 본문은0개다.
canonical finally가 admission 예약을 반환했고 PID82476의 임시 idle-sleep assertion도
자동 종료됐다. 기존 실행권 보유 작업은 취소하지 않았다.

22:41 같은 source55/build8의 custody 재검사를 통과한 단일 host-exclusive9가
원 입장 대기를 이어갔다. 제품·테스트·빌드는 반복하지 않았다. 새로운 controller의
PID 수명에만 idle sleep을 억제했고 종료 시 자동 해제됐다. 상태와 종결 원장은
checkout 밖 `host-exclusive9/status.json`, `products9-terminal.json`이다.

23:36 KST host-exclusive9는 canonical global EX와 project leaf를 얻었으나 원
timing gate가600초 동안 거절해23:46 BLOCKED/exit1로 종료했다. 양쪽 예약이
반환됐고 x3/m3/control은 생성되지 않았다. 마지막 receipt의 load31.724609는
허용8을 초과하므로 아래 판별 오류와 독립된 유효 차단 조건이다.

**확정된 preflight producer 오류**: 기존 `ps ... command=` 전체 argv에 unanchored
Cargo/rustc 정규식을 적용했다. Rust로 보고된 PID71339는 OS `comm`상 Python3.12이고,
실제 일치 문자열은 대기 wrapper의 `CODEGRAPH_REAL_CARGO_BIN=/opt/cargo/bin/cargo`
인자 중 `/cargo `였다. 이 argv는 현재 Cargo 실행의 증거가 아니다. 자식 부재·CPU0은
분류 기준으로 사용하지 않는다. preflight를 `ps ... comm=`로 바꾸고 OS 실행 파일
basename에만 전체 일치를 적용했다. 원래 comm을 샘플링하던 주기 host monitor도
동일 함수를 호출한다. PID 제외 목록이나 argv 추측을 추가하지 않았다.

- 수리 전 새 `test_main_samples_executables_instead_of_launcher_arguments`는1 FAILED였다.
  수리 후 `./.venv/bin/python -B -m pytest -q -p no:cacheprovider
  tools/ci/tests/test_check_host_contention.py`는 **31 PASS**다. 실제 Cargo/rustc·Cargo
  subcommand 거절, 정확한 basename/공백 경로, 원 ancestor/load/override 계약을 포함한다.
- `tools/ci/tests/test_benchmark_profile_capture.py`의 기존 consumer 검증은 **74 PASS**다.
  끝난 session의 terminal 결과를 이 채팅의 기존 도구 기록에서 회수했으며 재실행하지 않았다.
  세 Python 파일의 scoped Ruff와 diff whitespace 검사도 exit0이다.
- 수정한 gate의 실제 관측은 별도 Cargo/rustc2개와 load36.984863/limit8로 **BLOCKED**였다.
  판별 오류 수리가 실제 호스트 입장이나 XL/Mixed 성공을 의미하지 않는다.
- 세 Python 파일만 동결한 후보는 `2b2980dad2523cba5481e09e2510dc4b2b077443`다.
  부모55ce와 Rust/limits/vendor는 동일하고 이전 ref/build8은 보존했다. 도구 입력이
  바뀌었으므로 build8 증거를 새 source로 전용하지 않고 build9의 canonical cached
  pair build를 실행했다. build9는 **VERIFIED**/exit0이며 source/config/deps/tools/vendor
  lock 전후가 일치한다. scale/daemon SHA는 각각 기존 build8의 `dbd53c77…`/`a734c08f…`와
  동일하다. 새 입력 결속은 checkout 밖 `build9/{inputs,terminal}.json`을 따른다.

11월11일00:06 KST 단일 host-exclusive10은 새 source/binary/tools 재검사를 통과하고
canonical global EX를 기다렸다. x3/m3/control은 생성되지 않았고 제품은 **NOT_RUN**이다.
주기 monitor의 실제 observe 호출도 import/실행 exit0이었다. 이 관측에서 실제 별도
Rust5개·load16.876953을 보고했으며 캡처 경계74개가 실제 observe 전체를 직접 검증한
것으로 확대하지 않는다. PID80802 controller 수명에만 임시 idle-sleep assertion을
걸었고 종료 시 자동 반환됐다. 기존 실패·대기 원장은 덮어쓰지 않았다.

**01:06 KST 최종 입장 — BLOCKED**: 원3600초 대기가 끝나 canonical API의 admission
획득 호출에서 `CompileActivityBusyError`/exit1로 종료했다. execution EX·project leaf에
입장하지 않았으며 leaf stdout/stderr, x3/m3/control, `products10-terminal.json` 모두
없다. 새 제품 본문은0개다. `host-exclusive10/status.json`의 마지막
`canonical_host_exclusive_released`는 finally 종결 표기이며 EX 획득 증거가 아니다.
실제 CLI 오류와 반환값을 함께 사용한다. PID80802 종료와 idle-sleep assertion의 exit0을
확인했고 source2b private checkout은 clean이다. Main의 Python 세 파일도 동결본과
동일하다. 통과한 테스트·빌드를 다시 실행하지 않았다. 최종 제품 수용은 미완료이며
같은 bound pair의 원 XL·Mixed 실행이 남는다.

**11월11일02:16 KST 재확인 — execution 입장 BLOCKED**:
`/Users/songmin/.local/bin/python3.12 -B
/private/tmp/qidx-xl-mixed-rca-z1a_82he/host_exclusive_products11.py`가 기존
source2b/build9의 source/config/tools·binary 결속을 확인한 뒤 canonical API로 잠금
즉시 획득을 시도했다. `wait_seconds=0`에서도 실제 nonblocking flock을 먼저 호출하는
owner 구현을 확인했다. admission 획득에는 성공했으나 두 번째 execution EX 획득에서
`CompileActivityBusyError`/exit1이었다. 01:06의 admission busy와 차단 단계를 구분한다.
PID·파일 존재·실행 시간 추정으로 해제 여부를 판정하지 않았다. 얻었던 admission은
canonical finally가 반환했다. 최종 `host-exclusive11/status.json`은
`host_admitted=false`, `leaf_exit=1`, `products_terminal_exists=false`이고 PID10899는
종료됐다. 이 값은 양쪽 호스트 입장을 완료하지 못했다는 뜻이다. project leaf stdout/
stderr·timing receipt·x3/m3/control·`products11-terminal.json` 모두 없다. 이번 빌드·
테스트·제품 실행은0회이며 기존 실패 증거와 제품 기준을 유지한다. 행정 대기0초를
제품120/960초 deadline 변경으로 해석하지 않는다. source2b private checkout은 clean이며
새 코드 후보나 pair를 만들지 않았다.

**11월11일02:31 KST 상태 감사 — source2b와 현재 Main의 차이 확인**: 공유 HEAD는
여전히 `ade5fadc1…`이나 dirty 생산 소스가 추가로 달라져 있다. 아래8파일을 실제
bytes와 `git diff --no-index`로 비교했다. source2b private checkout은 clean이며
기존 build9/제품 기록은 변경하지 않았다.

- DIFF: `crates/quanta-index-scratch-sqlite/src/edit_write.rs`.
- DIFF: `crates/quanta-index-lexical/src/native/text.rs`.
- DIFF: `crates/quanta-index-lexical/src/native/text/prepared_inventory.rs`.
- DIFF: `crates/quanta-index-lexical/src/native/text/entry_patch.rs`.
- DIFF: `crates/quanta-index-lexical/src/native/text/entry_map/occurrence.rs`.
- MATCH: `crates/quanta-index-scratch-sqlite/src/lib.rs`.
- MATCH: `crates/quanta-index-lexical/src/native/text_spool.rs`.
- MATCH: `crates/quanta-index-lexical/src/native/text_spool/posting_sql.rs`.

차이에는 `PreparedInventoryChanges`가 원 spool 참조를 수명에 결속하고 완료하지
못한 Drop에서 batch 실패·inventory invalidation을 수행하는 변경, 호출부의 spool
인자 제거, 검증 authority의 occurrence 이동, Posting span 진입 실패를 typed health로
남기는 변경이 있다. 새 구현이나 성능 효과를 이 감사에서 검증한 것은 아니다.
build9 VERIFIED를 현재 Main의 컴파일/제품 성공으로 사용할 수 없다. 이 채팅에서 최신
Main의 전체 source/Cargo/contracts/config/deps/tools 대조·동결은 NOT_RUN이며 그 이후 제품
검증도 남는다. 이번 감사의 새 빌드·테스트·제품 실행은0회다. 마지막 실제 잠금
획득 점검은02:16의 execution busy이며 이 파일 대조로 현재 잠금 상태를 추정하지 않는다.

### 후속 RCA: 현재 입력 소비와 parent 성공 판정

전체 Main3189입력과 무시된 vendor lock11개를 새 private checkout에 동결했다.
source는 `22a8712e081daa57430c82e2a3189e957e833ec7`이며 원본 source5/build9는
변경하지 않았다. APFS clone의 Git mutable inode가 서로 다름과 복사 전후 drift0을
확인했다. 기록은 checkout 밖 `/private/tmp/qidx-xl-mixed-rca-z1a_82he/`의
`main-20261111-inputs.json`이다. source2b 대비31파일이 다르고,19개는 Rust/build
소비 파일이다. 이미 구현된 후행 Native 수리를 새로 작성하지 않았다.

실제 정상338/338의 terminal/body와 Drop guard 제거2 FAILED/원복2 PASS를 읽었다.
그 sourceac0와 새 동결본의 생산8파일 및 canonical alias/selector/실제 FileObjectStore
retained fee 회귀3파일은 bytes가 같다. 기존 본문을 새로 실행하거나 합산하지 않았다.
한편 ac0 대비 전체 현재 Rust 입력10개는 별도 Contract/IPC/SDK/benchmark 소비 변화여서
338 성공을 전체 새 Main/제품 수용으로 전용하지 않는다.

정적 `main-20261111-static.json`은 VERIFIED다. 원 source18f와 budget/memory/read attempt/
read settlement/native resources/Mixed/held-read8파일이 같고 scale.rs는 cfg(test)의 cleanup
필드만 다르다. Cargo manifest/lock 차이0, vendor lock11개, 정규화한 Cargo config가
Main과 같고, 도구 bytes와 기존 외부 dependency615경로의 bytes도 같다. 실제 Cargo
metadata/build는 NOT_RUN이며 이 정적 대조로 실행 graph나 binary를 합성하지 않는다.

**확정된 parent 판정 오류**: 기존 `run_products_locked11.py`의 실제 AST 식은2개 child의
product_verified만 true면 자식 exit1을 포함해도 all_products_verified=true가 됐다.
새 parent는 case3개의 정확한 수/순서·각 exit0·status VERIFIED·product true를 함께
요구한다. 실제 새 식을 정상, nonzero child, FAILED status, 누락/false product, 누락 case,
잘못된 순서, 중복 case의 독립8입력으로 확인했다. 정상만 true였고 나머지는 모두 false다.
이는 새 controller 판정 검증이며 Native 프로세스 제품이나338 회귀 재실행이 아니다.

`main-20261111-runner-review.json`은 위8판정과 기존 `_native_inputs`의 실제5개 인자
정책 검사를 기록한다. Large/XL Off와 같은-source 실패 후의 Record, 원 Mixed Off가
허용됐다. Mixed Record는 기존 producer의 `requires Off trace mode`로 거절됨을 확인했다.
Mixed를 Record로 바꾸거나 별도 Large Record를 Mixed의 trace로 표시하지 않는다.
새 rail은 manifest로 source/build/case를 함께 묶고, actual build에 manifest/runner SHA를
결속한다. 구 source5/build9/x3/m3를 새 실행에 하드코드하지 않는다. child의 기능 성공이
있어도 foreign Rust 관측 시 전체 수용은 BLOCKED이며 formal timing qualification은
별도 NOT_RUN으로 남긴다. 기존 capture/artifact validators를 그대로 사용한다.

**입장/소스 경계**: 03:08 KST canonical host EX의 즉시 획득은 admission busy로
BLOCKED였다(`main-20261111-admission-check.json`). 공용 F3a의 실제 gate와 Clippy는
별도 owner가 실행·수리했고, 동결 뒤 Main Contract/Core가 다시 바뀐 것을 새 guard가
검출했다. guard는 byte/mode/inventory 불일치 시 Cargo와 제품 시작을 거절한다. 따라서
22a는 정적 checkpoint이며 최신 Main의 최종 실행 후보로 승격하지 않는다. 공용 소스
경계 종료 후 새 immutable r2 이상을 한 번 고정하는 helper를 준비했고 미실행 상태다.
이번 새 Rust build/test/제품/장기 admission waiter는0회다. 정적 준비를 제품 완료로
기록하지 않으며 원 XL·Mixed 실패와 새 제품 NOT_RUN은 그대로다.

03:20 KST 마지막 준비 확인에서도 canonical shared(build)와 exclusive(product)가
각각 admission busy로 BLOCKED였다. 기록은 `main-20261111-admission-final.json`이며
별도 장기 waiter나 compiler를 만들지 않았다. 공용 workspace Clippy는 r4/exit101이
마지막 실제 결과이고 소스 경계 종료는 아직 확인하지 못했다. 이는 Native 제품의
실행 실패가 아니라 필수 source/admission 경계의 차단이다. 새 build/products 경로는
생성되지 않았고7개 runner SHA는 실제 인자/판정 검증 때와 일치했다.

**11월11일03:46 KST 재확인**: 공용 r5의 exit101, 실제 진단734개/106파일과
source-before/after JSON 일치를 확인했다. 이는 전체 workspace lint의 FAILED이며
Native734개 런타임 결함이나 원 제품 실행 실패로 재분류하지 않는다. 전체 lint green을
Native focused 수용의 선행 조건으로 추가하지 않는다.

그 뒤 Main의 Native `text.rs`, `text/prepared_inventory.rs`, `text/entry_patch.rs`,
`text/entry_map/occurrence.rs`, `text_spool.rs`가22a에서 다시 바뀌었다. Scratch2파일과
posting_sql.rs는22a와 같았다. 따라서 앞선 생산8/회귀3파일의 ac0 일치와338 성공을
후행5파일의 검증으로 전용할 수 없다. 기존 F3a 단일 경로에는 새 Native338 및
Contract/Core·IPC 영향 회귀 runner와 고정 r6 입력이 준비돼 있지만 실제 command/log/
exit·종결 소스 결과는 아직 없다. 이 결과가 없는 상태를 compiler/test 성공으로 만들지
않으며 중복 회귀도 시작하지 않았다. source boundary 확인 후 최종 동결이 필요하다.

동일 시점 canonical host API의 shared/exclusive wait0 획득을 각각 시도했으며 둘 다
admission lock busy로 BLOCKED였다. `resume-20261111-latest-admission.json`에 원 오류와
시점을 남겼다. 현재 결과 대조는 `resume-20261111-source-boundary-status.json`이다.
새 최종 r2 source/manifest·build/products 경로는 없고7개 runner는 검증 때의 bytes와
같다. 이는 유효 입력과 실제 실행권의 차단이며, 타 owner 잠금 제거·PID 추정·경합
override·기존 실패 전용으로 해결하지 않는다.

이 수리는 불필요한 후보 왕복과 컴파일 수명 경계의 수리이며 XL120·Mixed960의
수용은 아직 아니다. 새 source-bound pair의 제품 실행을 거쳐 전체 검색·발행·
pin·정산 조건을 다시 판정해야 한다. 실행했던 `18f6f89d…` source는 변경하지 않는다.

원본은 checkout 밖 `x2|m2/{execution.json,artifact/refusal.json}`과 각 control의
host preflight/terminal이다. fixture는 정상 정리돼 native-tmp에 DB가 남아 있지
않으며, 누락된 DB나 부분 진단으로 성공·잔여량0을 만들지 않는다.

현행 상태는 [README](README.md), 구현 순서는 [WORKPLAN](WORKPLAN.md)이 소유한다.

<a id="current-causal-rca"></a>
## 2026-11-09 재감사 — 직접 원인과 증폭 요인

재감사 출발점은 HEAD `ade5fadc1…`와 공유 작업 트리였고, A–E 접점을 연결한
현재 고정 후보는 `2275203…`이다. 별도 담당의 cold Map, ASCII/SDK, File
authority 변경은 보존했다. 기존 r55 Mixed Off와 r51 XL Off/Record 원본을
오프라인 대조했다. 소스별 v8c/d/e/h Rust 고유 회귀 144개와 Python profile
396/396은 통과했다. 현재 후보의 v8h scratch strict Clippy는 9개 진단으로
`FAILED`; scoped Core/Kernel module tree는 직접 재검사해 통과했다. r56 pair
build는 exit 0·source/config/deps/tools 전후 일치, Large 공식 제품 실행은
`PRODUCT_ARTIFACT_VERIFIED`다(ACK 33.689초·검색/재오픈/증분/삭제/old pin 완료).
xr56 XL Off·xr56r XL Record와 lm16 Mixed Off는 모두 `FAILED`다. capture는
`PARTIAL_DIAGNOSTIC`, build binding은 `UNPROVEN`, 정식 성능 수용은 `NOT_RUN`이다.
아래 r51/r55 실측은 각 과거 고정 소스에만 귀속한다.

<a id="r56-동결-pair-실행과-현재-코드-경계"></a>
### r56 동결 pair 실행과 현재 코드 경계 — 2026-11-10

원본은 외부 `/Users/songmin/.codex/qib6-_0r6mac7/{lr56,xr56,xr56r,lm16}`의
`execution.json`, `artifact/{summary,refusal}.json`과 Record child trace다.
네 실행은 동일 동결 소스 `2275203ac5e616512e7d3710a8cfbcfa3f8c6bca`의
pair를 사용했다. 도구의 prebuilt `build_binding=UNPROVEN`과 로컬 source/
binary 일치 확인은 별개의 주장이다. 정식 matched-host 성능 qualification은 없다.

| 실행 | 제품 판정과 직접 결과 | 비교 제한 |
| --- | --- | --- |
| `lr56` Large Off | `VERIFIED`: ACK 33.689초, 첫 검색 78.242ms, 재오픈 1.110초·검색 84.513ms; 증분·삭제·old pin 완료 | `PARTIAL_DIAGNOSTIC`; 제품 lifecycle 범위만 성공 |
| `xr56` XL Off | `FAILED`: SourceAdvance의 원 server120초 `BuildPrepare` deadline. StageSources 111.990초, full ACK phase는 143.428초에서 미완료; checkpoint는 scratch `edit_write.rs:416` | staging과 ACK phase를 더해 server120초와 비교하지 않음 |
| `xr56r` XL Record | `FAILED`: 같은 원 server120초 `BuildPrepare` deadline. StageSources 129.229초, full ACK phase 146.498초 미완료; 최종 checkpoint는 `storage/object/read_io.rs:165` | Off와 도달 작업량이 달라 속도·overhead 비율 계산 불가 |
| `lm16` Mixed Off | `FAILED`: 전단계 full ACK 31.705초·첫 검색 완료 뒤 부하 실패. 4,022 검색 완료 중 3,995 실패, offered queue full 1,940 | 960초 완주가 아닌 175.658초 producer 종료 전 partial |

`lm16`의 첫 검색 실패는 unique class의 SDK IPC Read timeout 425ms이며,
요청 전체 2.001초 중 queue 대기 1.574초였다. 완료 검색 실패의 SDK 경계
counter는 unique 2,814·broad 793·miss 388이고 모두
`transport_read_timeout`이다. 이 counter는 서버 내부 root cause의 분포가
아니다. 발행 warmup은 14/60, 측정 구간은 13 ACK/56 예정·43 missed이며
첫 current witness 오류는 IPC Read timeout 1,999ms다. 해당 부분 구간의
freshness p95는 7.196초다. pin은 121.212초에 해제됐고 renewal 1회,
settlement poll 0회다. 따라서 50 QPS·1Hz·960초·pin600초·settle60초 중
완료를 주장할 수 없다. 기존 r55의 16,307 실패와 같은 단일 원인으로 합산하지 않는다.

`xr56r`의 seq2 SourceAdvance child trace는 complete/loss0/open0이다.
SourceApply 40.017초(thread CPU 26.904초), BuildPrepare 75.517초,
그 안의 LexicalBuild 75.515초(thread CPU 45.007초)였다. seq2 Build의
accepted work는 reads 809,952·writes 5,862,600·bytes 4,757,640,801·
mutations 7,923,499다. InventoryPrepare 39.510초/CPU20.883초,
Occurrence pack 30.109초/CPU14.957초, Diff 25.390초/CPU17.211초,
PostingEdit 부모 44,025 scopes 17.973초/CPU11.536초다. Managed install
100 packets 15.149초/CPU9.299초의 하위 Catalog COMMIT은 150회
10.321초/CPU2.789초다. 이들은 중첩된 span이므로 합산하지 않는다.
이 중 Lexical inventory pack 아래의 `ManagedObjectAdvance` 50회는 E의
두-packet prepared leaf run이다. 같은 parent 경계에 reserve 50회, 물리 설치
100회, confirm 50회가 있고, 해당 caller의 ordinary `put_batch`에는 advance가
없다. Source Map 아래의 별도 advance 17회는 이 수에 포함하지 않는다.
따라서 완료된 Build prefix의 E 실제 채택은 확인됐으며 미연결 가설은 배제한다.
단일 packet 경로와 동진척 비교가 없어 E의 순성능 절감률은 계산하지 않는다.
Record 실패 위치는 object read checkpoint지만 이는 마지막으로 실행한
budget 검사일 뿐 지배 stage의 증명이 아니다.

Posting SQL `capacity`/`execute` 세부 span 20개는
`scratch-sqlite/src/edit_write.rs:23–55`의 첫 8회 또는 2의 거듭제곱 ordinal
**표본**이다. 이를 전체 실행 20회, SQL 비용 2ms, 빈 fast-return 다수라고
해석할 수 없다. PostingEdit 부모 44,025 scopes와 표본의 관계를 보존하고,
다음 코드 비용 소유는 실제 총 batch/execute·capacity 검사 비용을 같은
seq2 요청에 귀속해 확인한 뒤 정한다. 기존 r51의 전체 75,723 flush를
r56의 호출 총수로 대입하지 않는다.

Record footer의 object I/O reserve 643회/37.897초, confirm 643회/
62.082초, barrier 9.468초와 86,968 object write/sync는 **전체 daemon**
집계다. Source upload가 SourceAdvance보다 앞서 약 129초 진행됐으므로
이 수치를 전부 BuildPrepare나 Native 설치 비용에 귀속하지 않는다.
Record는 219MB급 child stream과 다른 accepted work를 보였고, Off와
동일 진척 비교가 아니다. 호스트 대기와 tracing 증폭의 크기는 미확정이다.

`2275203…` 뒤 공유 Main에 작성된 Catalog 원 요청 mutex deadline·취소/
per-waiter 정산 보강 및 IPC pre-dispatch peer-close guard는 r56 실행 파일에
없다. 초기 `v9`는 신규 test sink `Debug` 누락으로 compile `FAILED`/body0.
이를 고친 `v9b` (`a8ccfd4…`)는 Catalog·IPC 선택 회귀 35/35 body와
scratch `--no-deps --lib -D warnings` strict exit0으로 해당 범위 `VERIFIED`다.
IPC 선택 회귀는 queue-closed 정상 및 streaming 요청의 dispatch 0·dispatch
slot 0, half-close/pipeline 정상 경계를 포함한다. 이 결과는 원 50 QPS의
worker 점유 개선 실측이 아니다. 현재 `813cb26…`는 추가 E 원본 custody
guard·음성 3 case 및 D macOS RSS/Python method 수정이 들어간 별도 후보로,
그 후보의 v10 영향 lib 61/61·Python profile 402/402(5.17초)·hexagonal
check는 모두 exit0이다. E original-custody mismatch가 optional grant보다
먼저 거절되는 회귀, Storage optional admission의 취소와 변경된 원 deadline
거절, native Mac RSS 6개 및 실제 child sampler lifecycle이 선택 body로
실행됐다. scoped rustfmt/`git diff --check`도 exit0이다. v10 원본은 외부
`/Users/songmin/.codex/qib6-_0r6mac7/v10/terminal.json` 및 각 stage 출력이다.
영향 선택 검증과 전체 workspace/release qualification을 구분한다.
r57의 실제 `normal-build-result.json`/`terminal.json`은 source `813cb26…`의
build exit0·pipeline_completed=true다. frozen HEAD 일치/clean과 파일 SHA
`scale_matrix=084e6bee…`, `searchd=926740bf…`를 재대조했다. 로컬 source-bound
build 범위이며 release qualification은 아니다. 벤치 컴파일 20분43초는
제품 시간에서 제외한다. 같은 pair의 `lr57` Large Off는 execution exit0·
timeout false·`PRODUCT_ARTIFACT_VERIFIED`다. 원 server120초의 최초 ACK
45.699652초, 첫 검색92.221ms, 재오픈1.902217초·재오픈 검색98.662ms와
증분/noop/삭제/old pin·재오픈 삭제를 완료했다. 전체 producer75.217초는
별도 wall이고 capture는 `PARTIAL_DIAGNOSTIC`이다.

`xr57` XL Off는 execution exit1·outer timeout false·producer366.454초로
`FAILED`다. StageSources는196.176초, 미완료 full ACK phase는152.562초다.
단일 SourceAdvance의 원 server120초 `BuildPrepare` deadline에서 거절됐으며
마지막 checkpoint는 `storage/object.rs:854:28`이다. 당시 누적 accepted work는
reads2,525,060·writes4,338,992·bytes3,616,339,645·mutations5,673,466;
denied charge/memory/external refusal은 없다. StageSources/upload와 ACK phase를
server120초의 한 구간으로 합산하지 않는다. r56과의 작업 진척도 달라 이 수치로
순수 코드 개선율, 호스트 영향이나 tracing overhead를 단정하지 않는다.
`lm17` Mixed Off는 같은 source `813cb26…`에서 exit1·outer timeout false·
producer413.994초로 `FAILED`다. `failure.phase_prefix.mixed_load`의
`first_failure_owner=publisher`이며 sequence76은 terminal ACK를 받았으나
`old_view_witness`에서 IPC Read 1,999ms timeout으로 끝났다. 첫 query 실패는
task8 broad class의 queue 623.669ms와 IPC Read 1,376ms, 전체
2,003.620ms다. 완료 검색 11,067건 중 실패 11,050건의 SDK 경계 cause는
unique 7,688·broad 2,240·miss 1,122건의 Read timeout이다. offered
16,645 due 중 queue full 5,514건이다. 이를 한 서버 내부 원인으로 합산하지 않는다.

발행 warmup은 ACK 8/60 예정·missed 52, 측정 구간은 ACK 63/265 예정·
missed 202다. 62개 측정 witness 완료와 실패한 sequence76을 구분한다.
두 값은 960초 완주 분모가 아니다. retained load 시작 193.093ms 뒤 해제
시도 334,761.217ms(경과334.568초), 확인된 release 334,831.435ms다.
renewal은 120,373.426ms·241,348.209ms의 두 건이며 settlement poll은
0이다. 600초 pin과 이후 60초 정산을 검증하지 못했다.

선택된 후반 connection11788/request683 표본에서는 query prepare
3,079.849ms, Catalog COMMIT 두 번 합계 3,036.091ms와 lock 77.634ms가
관측됐다. 이 하나의 표본은 11,050 실패 전체의 비용 분포나 fsync 시간을
대표하지 않는다. 첫 실패의 queue/IPC와 마지막 발행 witness도 다른 요청이다.

Native resource 첫 epoch는 408.125초다. native macOS RSS는 2,776표본·
missing0·observer wall224.095ms이고, fixture disk census는 781표본·
missing3·observer wall145.852초(동일 epoch wall의 약35.7%)다. 관측 wall은
부하와 겹치므로 제품 latency에서 빼거나 process CPU로 해석하지 않는다.
CLI의 `boundary-only`는 legacy phase disk 정책이며 Native opt-in
`opt_in_diagnostic_v2`는 별도 500ms periodic fixture census를 요구한다.
이를 꺼서 원 qualification을 통과시킬 수 없다. 공유 Main의 후속
`ObservationWorkers` process/disk 분리는 frozen r57에 없고 현재 pair의
실행 검증도 아니다. RSS 대기 분리 코드이며 전체 filesystem walk 비용을
제거하지 않으므로 중복 구현하지 않는다. native RSS 기능 적용만으로
속도 개선을 주장하지 않는다. controller queue는 완료됐지만
`all_products_verified=false`다.
기존 A/B/E producer 경로를 새로 구현하지 않으며,
남은 코드 비용 소유는 XL의 원 server120초 지배 구간과 Mixed의
queue/IPC·Catalog COMMIT·reader·발행 witness를 원 요청별로 나눠 결정한다.
원 deadline·50 QPS·1Hz·960초·pin600초·settle60초와 durable clock/fsync
경계는 유지한다.

<a id="r56-improvement-candidates"></a>
### r56 추가 비용 귀속과 개선 우선순위 — 2026-11-10

위 실행 상태를 반복하지 않고 full 요청과 Build 하위 비용을 추가로 나눴다.
입력은 `xr56r/causal-timeline.jsonl.native-xlarge-children/first.jsonl`과
`lm16/artifact/refusal.json`이다. child의 SHA-256은
`7465b6d57187e26eb769b095471dc513eda87bf36b90002b5ead5a08adae991d`다.
2,257,208 events의 연속 sequence·동일 capture·begin/end·footer를 오프라인
대조했고 dropped/unwritten/open spans/producer leases는 모두 0이다. 아래
집계는 진단이며 새 qualified replay나 Off/Record overhead 검증이 아니다.

**1. Mixed의 worker 점유와 대기 증폭.** 마지막 sequence 32는
`terminal_published/immediate_ack` 뒤 `current_witness`에서 실패했다.
submit 자체가 실패했다는 판정과 구분한다. 27개 발행 audit는 모두 submit
1회·status poll 0회다. retry/poll 경로를 새로 고치는 근거는 없다.
warmup/measure mutation 준비→ACK 중앙값은 2,930.500/2,327.825ms,
ACK→현재 검색 확인 중앙값은 473.156/669.764ms다. measure의 witness 완료
12건만 계산한 nearest-rank p95는 2,242.145ms이며 마지막 실패 건은 여기에
없다. 완료 표본만으로 전체 SLO를 통과시키지 않는다.

원 query worker 4개가 50 QPS를 안정적으로 처리하려면 평균 worker 점유가
80ms 이하여야 한다. 이는 큐 안정성의 필요조건이며 실제 평균 측정이나 p95/p99
수용이 아니다. 6,025 due 중 queue full 1,940건, 4,085 enqueued와
4,022 completed의 분모를 분리한다. 완료 실패 3,995건의 cause 합계는
일치하고 overflow=false지만 모두 SDK read timeout이라는 사실만으로
서버 내부 원인을 Catalog 한 곳으로 귀속할 수 없다.

보존된 첫 retained 성공 표본은 COMMIT 3–7회다. connection 102의 Catalog
connection acquisition은 183.816ms, COMMIT은 64.963ms, reader는 7.147ms다.
반면 connection 113은 Control 188.992ms, Interactive 235.184ms,
reader 370.320ms다. 표본은 첫 요청 위주이며 일부 JSON은 prefix/tail 표시
경계에서 분할돼 있다. 전체 평균으로 외삽하거나 분할 누락을 0으로 세지 않는다.
다음 판단은 기존 IPC peer-close/Catalog waiter 수정의 회귀 뒤 원 부하에서
offered queue·Control/Interactive·Catalog·reader를 나누는 것이다. 슬롯 증가나
live-ticket 검증 생략의 효과는 입증되지 않았다.

**2. XL Posting의 반복 페이지 쓰기.** seq2 full 요청 span 42500 안의
Posting flush/lane33은 44,025 scopes다. 그 아래 lane22는 40,671 write
scopes, main DB xWrite callback 118,718회/486,268,928B, WAL write 0,
write/admission/post-callback refusal 0을 기록했다. lane22 wall 13.362초
전체는 syscall 시간이 아니다. admission callback wall 3.848초,
lower envelope 1.220초, post callback 0.318초가 관측됐다. lower envelope도
C glue·native witness를 포함하므로 순수 디스크 시간으로 부를 수 없다.
이 값은 **전체 lane22 집계**이며 위 20개 sampled execute를 외삽한 값이 아니다.

`diff_inventory_inner`는 owner별로 term을 생성하고 spool PK는 `(term,k)`다.
현재 spool cache는 2MiB, journal/synchronous는 이미 OFF다. 필드별 bind
공유와 wide posting 경로도 이미 있다. 확인된 사실은 반복 물리 페이지 쓰기이며
cache spill/locality가 지배 원인이라는 것은 아직 가설이다. 연결별 hit/miss/spill·
write/row를 확인한 뒤 bounded key 정렬 run/merge 또는 원 memory grant 안의
locality 개선을 비교한다. 전체 RAM 정렬·무승인 cache/cap 확대나 bind 최적화만으로
I/O가 사라진다는 주장은 채택하지 않는다. cold Map·SDK 경로는 별도 소유다.

**3. XL 설치의 transaction 반복.** 같은 full 요청의 managed install은
124회/21.085초, Catalog COMMIT은 183회/15.125초다. 이 중 Build 하위는
위와 같이 install 100회/15.149초, COMMIT 150회/10.321초다. full과 Build,
부모/자식 시간을 섞거나 합산하지 않는다. 동일 full 요청의 object aggregate는
file sync 18,648회/0.734초, barrier 124회/2.614초, writer-ticket prepare
18,648회/3.895초, writer open 18,648회/2.906초다. daemon 전체 footer의
수치를 가져온 것이 아니다. 이 실행에서 개별 file sync만 지배 병목이라고 볼
근거보다 기존 reservation/confirmation/advance의 transaction 반복을 검토할
근거가 크다. 이미 있는 prepared leaf run·union receipt·two-packet adoption을
재사용하고 정확한 receipt·attempt/fence·부분 실패·crash·기존 memory 한도를 보존한다.

**4. 관측기의 남은 disk census.** lm16은 구형 `ps` sampler다.
172.271초 epoch에서 RSS probe 1,481회/wall 11.218초, fixture disk census
338회/wall 26.940초였다. 단일 worker의 RSS 최대 관측 간격은 1,017.905ms로,
설정 100ms와 달랐다. 이 wall은 CPU나 인과적 제품 overhead가 아니며 제품
latency에서 빼지 않는다. 현재 D는 safe `libproc` wrapper의 resident-byte/start
identity 조회로 `ps`를 제거했다. Native 82/82·Python 402/402와 실제 Small
20회에서 첫 epoch 162개/재오픈 5개 RSS 표본, missing 0을 확인했다.

native RSS 교체 직후에도 전체 fixture의 `st_blocks` census가 동일 worker를
점유했다. 후속 소스는 process/disk를 별도 worker로 나누고 공통 stop 신호와 두
join을 소유하는 RAII 경계를 추가했다. 단일 walk·원 100ms/500ms 주기·boundary·
inode subset·hardlink/rename/truncate 의미는 유지한다. 실제 native RSS가 disk
release 전에 진행하는 회귀를 작성했고 잠금 해제 뒤 실제 body로 검증했다.
최신 main Native 85/85, 테스트 lint 보정 뒤 worker 3/3, harness 정리 영향 9/9가
통과했다. 겹치는 선택은 합산하지 않는다. 외부 lease로 인한 기존 `BLOCKED`는
해소됐다. Literal API 불일치의 compile/body0 이력과 두 caller 정합성 수리는
[검증 마감](WORKPLAN.md#d-verification-closeout-20261110)에 남겼다.

r57 prebuilt와 main harness의 recipe/input-domain 불일치를 허용하지 않고 현재
source의 daemon을 빌드했다. 고정 사본 SHA `812a93f8…`으로 Small20과 HEAD
3-file corpus CLI를 통과했다. 실제 Small 출력의 RSS 표본은 180/5, missing0,
worker failure 없음이며 엄격한 Python resource 소비자도 수락했다. 이는 기능
검증으로, r57 Mixed의 2,207ms gap과 Small의 gap을 성능 개선률로 비교하지 않는다.
epoch당 thread 1개가 추가되며 대형 부하의 성능/메모리 효과는 아직 미검증이다.
traversal 자체의 비용 감소, Linux aggregate RSS·연속 peak도 별도 미수용이다.

**문구 기반 원인 추론 제거.** r56의 `transport_read_timeout` counter는 당시
Display 문구를 엄격히 파싱한 진단 분류다. SDK가 보존한 typed phase authority가
아니다. 후속 `mixed_diagnostics.rs`는 이 파싱을 제거해 `NativeUdsError` variant로
확인되는 `sdk_transport`까지만 집계한다. connect/read/write의 typed 원인이
SDK 경계에서 사라진 상태를 문구로 복원하지 않는다. 원문 오류·기존 실패 stage는
보존하지만 진단이 실행 권한·오류 우선순위나 원인 확정으로 승격되지 않게 한다.

최초 조사는 기존 raw의 오프라인 RCA였고 이후 D 수리와 영향 검증을 진행했다.
기존 r57 Large/XL/Mixed 실행의 결과를 회수했으며 동일 부하를 중복 실행하지 않았다.
harness의 기존 corpus/retained/offered-load/CLI strict 지적까지 수리했고 전체 대상
`--no-deps --all-targets -D warnings`는 exit0이다. 내부 inode membership map의
정책 예외는 한 지점의 이유를 명시하며 구조나 집계 순서를 바꾸지 않았다.
현재 D 기능 검증과 r57의 XL/Mixed 실패, Linux·전체 제품/운영 수용은 별도 범위다.

<a id="additional-static-audit-20261110"></a>
### 추가 정적 감사와 관측 수용 한계 — 2026-11-10

현재 main의 E Core→Kernel→Storage→Native prepared-run, Catalog read-attempt,
retained session·IPC 경계와 관측기 수명을 병렬로 재검토했다. 요청·deadline·취소,
grant/receipt custody, sync 후 confirm, COMMIT 후 응답과 worker join에서 추가로
확정된 P0–P2 정확성 결함은 찾지 못했다. 이는 정적 감사이며 새 제품 통과 판정이
아니다. IPC queued-peer 회귀의 실제 `slots.in_flight()==0` 단언도 이미 있어
누락 작업으로 재발급하지 않는다.

`causal_cost_profile.py::_native_sample`은 설정 주기·양의 표본 수·byte 정합성과
유한한 gap을 검사하지만 epoch 전체의 coverage는 판정하지 않는다. 기존 fixture를
메모리에서 변형해 두 epoch를 각각 960,000ms, 각 disk 축을 2표본·최대 gap
959,000ms로 설정해도 `_native_resource_observation(..., "macos")`의 진단 형식
검사가 수락함을 확인했다. 전체 Mixed replay나 원 부하 실행을 통과한 것은 아니다.

canonical 결과는 이미 `sampled_only=true`, 연속 peak/운영 수용 불가와
`performance_qualification=NOT_RUN`을 명시한다. 따라서 이 수락을 새 제품 P2
결함으로 분류하거나 임의의 gap 한도를 추가하지 않는다. 정식 자원 수용에는 별도의
관측 coverage 계약과 sparse-sample negative가 필요하다. 필요 시 첫/마지막 성공
관측 offset·성공 표본 사이 gap을 계약화하며, 현재 `max_observed_gap_ms`는 실패한
probe도 포함한 시도 사이 간격이므로 성공 표본의 coverage로 간주하지 않는다.
기존 100ms/500ms 설정과 sampled-only 범위는 유지한다.

실제 코드 성능 잔여는 기존 Q3의 XL120과 Mixed 부하 실패다. disk worker 분리는
RSS 대기를 분리한 구현이며 전체 tree walk의 비용 감소 구현이 아니다. 추가 벤치는
실행하지 않았고 다른 Map/ASCII/SDK 레인의 구현도 중복하지 않았다.

<a id="causal-measurement-repair-20261110"></a>
### 원인 분해 계약과 계측 수리 — 2026-11-10

이번 코드 대조로 다음 세 가설은 수정 대상으로 채택하지 않았다.

- Posting scratch는 이미 `BEGIN IMMEDIATE` 한 트랜잭션이며 journal/synchronous는
  OFF다. batch마다 새 COMMIT이 발생한다는 가설은 현재 코드와 맞지 않는다.
- r57의 한 searchd에서 `KernelServices`가 연 Catalog session을 retained ticket들이
  공유했다. 당시 session-local 관측 그룹은 이미 제품의 동시 요청을 묶었다. catalog-global
  그룹을 추가해 lm17을 해결한다는 근거는 없다. 7개 순차 live 관측의 durable floor를
  지연시키거나 관측을 생략하는 변경도 채택하지 않는다.
- disk census는 세 subset을 한 walk에서 계산하고 항목당 metadata를 한 번 조회한다.
  중복 stat 제거 또는 단순 `DirEntry::metadata` 교체가 Mac syscall 수를 줄인다는
  근거는 없다. 매 주기의 전체 allocated blocks·hardlink 의미를 유지하면 항목 수에
  비례하는 조회는 필요하다. 범위 판정만 미세 조정해 145.852초를 해결했다고 하지 않는다.

XL의 `(owner별 term 순서)` 입력과 `(term,owner)` SQL PK는 locality 가설의 근거다.
그러나 xWrite 118,718회/486,268,928B만으로 같은 페이지의 재쓰기가 지배 비용인지,
최종 출력량 자체가 큰지 구분할 수 없다. 128행 내부 정렬·전체 staging/merge·cache
확대는 아직 채택하지 않는다. 추가 staging의 I/O·4GiB scratch admission과 cache
메모리를 먼저 증명해야 하며 다른 cold Map/ASCII 담당의 구현을 재작성하지 않는다.

이를 구분하도록 기존 producer와 소비자의 같은 진단 계약을 수정했다.

| 경계 | 구현한 관측 | 의미·제약 |
| --- | --- | --- |
| Posting SQL | 기존 `NativeTextPostingEdit/lane35`의 first8+power-of-two 표본에서 같은 connection의 pager hit/miss/write/spill 전후 차이와 실제 성공 rows/batches | Off는 pager status를 호출하지 않는다. 네 pager 값은 전부 있거나 전부 없다. SQL 실패는 원 오류를 유지하고 counter를 생략한다. pager API 실패/역행은 진단 health일 뿐 제품 거절이 아니다. 표본 합계는 전체 workload 합계가 아니다. |
| Python trace→worker index | 정확히 CatalogLockHold/lane0와 Posting/lane35만 pager owner로 허용 | 잘못된 owner·부분/중복 counter는 거절한다. 성공 표본과 counter 없는 실패 표본의 정상 혼합은 수락한다. 실제 0과 미관측은 구분하며 기존 전역 write-error 무효화 정책을 유지한다. |
| Mixed retained Catalog | 선택된 요청의 기존 6개 validation 단계에 wall·동일 thread CPU·유효 CPU 표본 수를 함께 보존 | off/다른 Catalog/다른 owner는 CPU sampler를 호출하지 않는다. 누락/역행/부분 표본은 JSON `null`이다. leader의 그룹 COMMIT 비용을 모든 waiter의 개별 COMMIT으로 세지 않는다. wall−CPU는 I/O·deschedule·잠금을 포함하므로 host 경합의 확정값이 아니다. |

CPU primitive는 기존 searchd의 검증된 timespec 변환을 재사용하고 Catalog에는
선택적 함수 포인터만 주입한다. Core port에 OS/vendor 타입이나 새 권한을 추가하지
않았다. 원 deadline·작업 비용·메모리 상한·clock floor/fsync·제품 결과는 그대로다.
이 변경은 원인 분해의 빈틈을 수리한 것이며 원 XL/Mixed 성능 실패를 해결했다는
판정이 아니다. 다음 coherent source의 기존 Q3 실행에서 같은 요청의 pager/rows와
COMMIT wall/CPU/VFS sync를 연결해 실제 줄일 비용을 선택한다.

감사 뒤 공유 Catalog 소스에 `observation_registry` 기반 그룹 공유가 추가된 것도
확인했다. 위 r57의 session-local 설명은 당시 소스의 사실이다. 후행 구현의 부재나
성능 통과를 뜻하지 않으며 이번 계측 패킷에서 같은 그룹 확장을 재구현하지 않았다.

### 실패 범위부터 구분

| 범위 | 확인된 결과 | 의미 |
| --- | --- | --- |
| Large 4,096 sources | r50 로컬 ACK 27.622초·검색·재오픈 성공 | 과거 Large 실패를 현재의 실패로 반복 인용하지 않는다. 새 dirty 소스 전체의 통과 증거는 아니다. |
| XL 32,768 sources | r51 Off/Record 모두 서버 120초의 `BuildPrepare` deadline | ACK phase 142.171초는 서버 deadline과 다른 구간이다. checkpoint의 child 검증 자체가 병목이라는 뜻도 아니다. |
| Mixed 일반 검색 | 첫 실패는 `broad_top20`, queue 768.554ms 뒤 IPC Read 1,231ms timeout | 전체 2,001.390ms에 큐 대기가 포함된다. 마지막 held 검색과 다른 요청이다. |
| Mixed 실행 중단 | 네 번째 갱신 뒤 held 검색 IPC Read 555ms timeout | 전체 검색 실패·발행 지연의 유일 원인으로 일반화할 수 없다. |

### Mixed의 직접 구조 — 검증과 영속 쓰기의 결합

고정 소스 r55의 성공 retained Text 요청은 다음 7개 검증 경계를 거쳤다.

| 순서 | 경계 | r55 당시 코드 |
| --- | --- | --- |
| 1 | Registry 세션 진입 | `native_read_sessions.rs:466` |
| 2–3 | Kernel profile 읽기 전·후 | `kernel/src/update.rs:1411,1418` |
| 4 | reader·예약 해석 완료 | `kernel/src/update.rs:481` |
| 5–6 | reader 실행 전·최종 Kernel 검증 | `kernel/src/update.rs:782,895` |
| 7 | Registry 결과 반환 전 | `native_read_sessions.rs:474` |

당시 `ReadTicket::validate_for_request`는 매번 Catalog에 위임했다.
`catalog/src/native/authority/lifetime.rs:709`의 `validate_live_ticket`는
OS lock/inode·session/pin/fence를 확인하고, 단일 `Mutex<Connection>`에서
`BEGIN IMMEDIATE` → clock floor 확인/갱신 → COMMIT을 수행한다.
따라서 읽기 요청도 writer의 직렬 처리 시간을 반복 소비한다. 동일 clock 표본의
UPDATE 생략과 첫 Kernel stage의 검증 공유는 이미 구현돼 있다. 이를 새로운
수리로 다시 제안하지 않는다.

현재 고정 후보 `2275203…`는 이 일곱 관측 경계를 유지하고 Core/Catalog의
read attempt와 Kernel/Runtime의 실제 retained 경로를 연결했다. 성공 관측의
clock floor를 영속화한 뒤 결과를 반환하며 동시 관측만 bounded group COMMIT을
공유한다. 실제 COMMIT 공유율과 50 QPS 수용은 `NOT_RUN`이다.

r55 실패 요청의 Catalog 합계는 475.214ms/서버 519.909ms였다. 획득 대기
335.255ms와 실제 `WouldBlock` 277회, 자기 요청의 COMMIT 137.609ms가
확인됐다. reader는 이 요청에서 3.747ms였다. 보존된 성공 3건도 각각 COMMIT
7회를 수행했고, 요청당 COMMIT 합계 평균은 109.109ms였다. 이 관측 비용이
유지된다면 50 QPS는 COMMIT만으로 초당 5.455초의 직렬 점유를 요구한다.
7회 COMMIT으로 50 QPS를 처리하려면 다른 Catalog 작업을 제외해도 COMMIT당
평균이 2.857ms 미만이어야 한다. 이는 관측 비용의 조건부 산술이며 quiet-host의
성능 상한이나 B83 통합 후 처리량 예측은 아니다.

각 검증은 시간·fence·오류 경계를 보호한다. 현재 후보는 성공한 중간 관측의
clock floor를 COMMIT 뒤에만 인정하고, 실패·취소·재시작의 floor와 최종 확인을
별도 회귀로 검사하도록 작성됐다. 관측 전용 `observed_catalog_now_ms`만 pin
검증에 대입하거나 B82 TLS 관측에 권한을 싣는 변경은 채택하지 않았다.

### 큐·예약·발행은 별도 원인 범위를 가진다

r55 종료 전 부분 집계는 일반 검색 완료 16,319건 중 실패 16,307건,
offered queue full 7,818건이다. raw는 첫 실패와 제한된 서버 표본만 보존한다.
실패 전체의 route/원인별 분포가 없어 모두 같은 Catalog 원인이라고 판정하지 않는다.
다른 표본에는 Control admission 780.845ms, Interactive admission 283.832ms,
취소된 reader 371.195ms도 있다. held 요청의 reader 3.747ms를 모든 query
class의 실행 비용으로 사용하지 않는다.

Control은 Registry 진입과 Kernel profile/stage 검증 동안 유지되고, stage 준비가
끝나면 반환한 뒤 Interactive를 기다린다. Interactive도 결과 변환 뒤 최종 Kernel
검증 전에 반환한다. 이 순서는 이미 수리돼 있다. 앞쪽 Catalog 지연이 Control
점유를 늘리는 코드 경로는 있지만, 어떤 peer가 슬롯이나 Catalog mutex를 점유했는지
실측하지 못했다. publisher가 동일 Control class를 기다린다고 단정하지 않는다.

발행도 독립적으로 기준을 넘었다. 종료 전 warmup ACK는 8/60회, 측정 구간은
76/420회이며 missed tick은 각각 52·344회다. 측정 구간의 예정 시각→현재 검색
p95는 8.002초, p99는 27.417초다. 960초 완주 결과는 아니다.
측정 구간의 기존 `publication_records` 76건을 cumulative timing의 차분으로
분해하면 다음과 같다. 각 열의 percentile을 합산하지 않는다.

| 구간 | 중앙값 | p95 | p99 |
| --- | ---: | ---: | ---: |
| 입력 준비 완료→mutation 준비 완료 | 266.374ms | 764.832ms | 1,093.467ms |
| mutation 준비 완료→ACK | 3,227.284ms | 5,424.600ms | 24,951.404ms |
| ACK→현재 검색 확인 | 573.703ms | 1,243.838ms | 1,827.149ms |
| 이전 view witness | 366.323ms | 1,024.307ms | 1,711.248ms |

76건 모두 audit의 `submit_count=1`, `status_checks=0`, `immediate_ack`였다.
그 표본에서 상태 polling이나 submit retry 반복이 주원인이라는 가설은 지지되지
않는다. 반면 mutation→ACK는 RPC queue·Catalog 대기·실제 증분 구축을 포함하므로
이 수치 전체를 순수 인덱서 CPU로 부를 수 없다. 발행의 서비스 내부 귀속은 남았다.

정적 경로는 단일 owner `SourceAdvance` → `SourceDelta` → Registry의
`DeltaInput::Patch` → Native Text Incremental → `SourceChangeCursor`다.
변경 cursor는 `visit_diff`의 최대 64행 page를 사용한다. 매 tick Bootstrap이나
snapshot cursor로 강제 전환된다는 가설은 현재 호출 경로에서 지지되지 않는다.
다만 Patch에도 map structural diff, inventory/contribution flush, Posting 적용과
root 설치가 있으므로 O(1)이라고 판정하지 않는다. 증분 기능을 새로 구현하는 대신
현재 submit의 admission·SourceApply·member build·publication 경계를 확인해야 한다.
관련 접점은 `kernel/src/registry.rs:70`, `core/src/index/mutations.rs:677`,
`core/src/index/mutations/source_change_diff.rs:12`,
`lexical/src/native/text.rs:11193–11360`이다.

### XL의 직접 구조 — 구축 CPU·SQL·설치 반복

xr51은 SourceApply 32.029초와 BuildPrepare 85.057초로 합계 117.086초를
소비했다. lexical build child의 thread CPU는 68.102초/85.056초 wall,
SourceApply는 22.460초/32.029초 wall이다. outer BuildPrepare의 CPU 필드는
계측되지 않았으므로 CPU=0으로 해석하지 않는다. Record의 계측 비용은 포함돼
있지만 Off도 원 120초에 실패했다. 실패를 host 대기 하나로 설명할 근거가 없다.

| BuildPrepare 안 구간 | wall / thread CPU | 원인과 제한 |
| --- | --- | --- |
| Inventory Diff 22,016 owner | 27.106 / 26.320초 | 아래 Posting scratch를 포함한다. |
| Posting scratch flush 75,723회 | 18.001 / 17.406초 | `(term, owner)` SQLite B-tree 반복 적재. 21개 표본의 `raw_execute` 비중은 약 71%, capacity 확인은 약 5%다. 전체에 비율을 외삽하지 않는다. |
| Managed install 173회 | 17.410 / 13.797초 | group/pack 및 다른 부모 구간과 겹친다. |
| 설치 내부 Catalog COMMIT 345회 | 12.089 / 5.048초 | Managed install의 하위 비용이다. mutex wait 합계는 0.1ms 미만이다. |

이 표의 시간을 모두 더하지 않는다. Mixed와 달리 XL의 설치 COMMIT은 잠금
대기가 거의 없는 상태에서도 비용을 소비한다. 물리 packet을 늘리는 원인 중 하나는
256행 창과 1MiB occurrence staging 한도의 불일치다. 실제 packet 폭은
95·96·160·161개였다. 단일 packet의 ordinal·scratch confirmation을 보장하는
현재 custody와 installer 임시 메모리 승인도 함께 바꿔야 분할을 줄일 수 있다.
단순 cap 증설이나 reserve/sync/confirm 중 하나 삭제는 채택하지 않는다.

기존 VFS 계측도 재확인했다. BuildPrepare 범위에서 WAL native sync 604회,
main native sync 129회, anchored parent sync 604회가 관측됐다. 다만 이 span들은
Catalog COMMIT lane3의 자손이 아니고 scratch/다른 SQLite 작업을 포함할 수 있다.
따라서 이를 345 COMMIT의 정확한 내역으로 귀속하지 않는다. native sync lane은
FD witness를 포함하고 AfterSync settlement와 parent sync도 중첩될 수 있다.
COMMIT wall을 곧바로 fsync 시간이라고 부르거나 이 수치를 더해선 안 된다.

### 호스트·관측 증폭과 남은 불확실성

r55의 daemon 관측 epoch 612.583초에서 fixture disk census는 1,148회/
170.854초, macOS `ps` RSS는 4,437회/58.404초의 observer wall을 소비했다.
두 probe 합은 epoch의 약 37.4%다. 제품 latency와 시간이 겹치므로 이 합을
latency에서 차감하지 않는다. load1 중앙값 33.31과 host-wide swap 활동은
경합의 증거지만 해당 process의 COMMIT 증폭량을 알려주지 않는다.
관측기 교체 효과와 canonical tracing overhead도 미확정이다.

Mac child RSS는 aggregate process RSS가 아니다. r55의 aggregate RSS와 process
I/O는 unsupported이며 원 운영 자원 gate를 수용할 증거가 없다. r55 Mixed Off는
canonical timeline이 없고 두 실행의 정식 build binding은 `UNPROVEN`이다.
기존 source/binary 전후 일치와 정식 matched-host 성능 qualification을 구분한다.

### 수리 우선순위와 다음 실행의 판정

1. B83에서 검증 권한과 durable floor의 의미를 고정하고 반복 writer transaction을
   줄인다. 실제 clock/fence/취소/오류 우선순위 회귀와 쓰기·sync 감소가 채택 조건이다.
2. XL은 별도 담당의 cold Map/ASCII 후보를 반영한 뒤, Posting SQL 실행량과
   packet 분할/custody의 잔여를 각각 수정한다. case-paired 페이지 순회, 128행
   wide batch, confirmed-occurrence 재사용은 이미 존재하므로 재구현하지 않는다.
3. 수정 합류 뒤 한 큐에서 원 Large/XL 120초 ACK·검색·재오픈과 원 Mixed
   50 QPS·1Hz·960초·pin600초·settle60초를 검사한다. B83 통과만으로 일반 검색과
   증분 발행·정산까지 닫지 않는다. r55의 settlement poll은 0이다.
4. 그래도 실패하면 지원되는 Mixed Off의 기존 제한 표본과 원 phase timing으로
   첫 잔여를 선택한다. 전체 실패를 마지막 오류 하나로 설명하거나 지원하지 않는
   Mixed Record를 다시 실행하지 않는다. 필요 관측은 원인별 제한 집계와 기존 sync
   span의 소유 연결이다. 새로운 전체 trace 수집부터 시작하지 않는다.

원본은 외부 `qib6-_0r6mac7/lm15/artifact/refusal.json`, `lm15/stderr`,
`lm15/execution.json`, `xr51-summary.json`과 xr51 child trace에 있다.
publisher 차분은 `/failure/phase_prefix/mixed_load/measure/publisher/publication_records`
76건이며 최근접 rank percentile을 사용했다. 새 per-run evidence 파일은 만들지 않았다.

### 후속 SDK 인계와 accepted bytes 계측 구멍

별도 SDK 담당자는 `native-packed-file-v3-product-v1` 동결 pair의 원 SDK 60초
실행을 전달했다. Native etcd는 `WorkUsage.bytes=536870827`에서 추가457B,
Caddy는 reads1,000,000에서 추가1로 거절됐다는 내용이다. 이 수치는 담당자
인계이며 부모가 그 SDK raw 전체를 재검증한 결과는 아니다. 보고된 위치의 소스는
직접 대조했다. `scratch-sqlite/src/edit_page.rs::read_map_edit_budgeted`는
`OrdinalMapEdit::allocation_bytes`를 logical bytes에, `text_spool/token_groups.rs`는
새 term의 reads/4096B를 WorkUsage에 charge한다. 따라서 이 512MiB 실패를
process RSS나 SQLite 물리 DB 크기 한도라고 해석하지 않는다.

담당자는 동일 Caddy prefix의 첫53 root에서 Map reads735,540→463,538,
물리 Map 객체12,529→7,294 감소도 전달했다. 반면 etcd의 같은94 prepare/29 root
구간은 managed Map install이 없어 같은 원인으로 묶을 수 없다고 했다.
이 인계만으로 A–E 코드가 새 SDK cap을 통과한다고 예측하지 않는다. A/B는
retained read의 동시 관측 COMMIT, C는 bind 작업, D는 관측을 다루며 원 논리
작업량을 임의 할인하지 않는다. E의 Native 설치 packet 통합은 현재 소스에
연결됐지만 COMMIT 감소 효과는 `NOT_RUN`이다.

r55 당시 Kernel의 두 async BuildPrepare 경계는 `diagnostic::begin` →
`phase_budget` → await → `finish_work`를 써서 종료/실패 stage만 기록하고
accepted WorkUsage delta를 기록하지 않았다. 현재 후보는 기존 `WorkStage`를
해당 async 경계에도 연결했다. 미귀속 bytes를 특정 allocator나 Map 비용으로
추측하거나 부모/자식 delta를 더하지 않는다. 계측 보완의 컴파일·실제 producer
대조는 `NOT_RUN`이다.

## 초기 분석 이력

분석 기준은 저장된 C16/C17 기록, `51b70b34`·`86e6130e` 이력,
r4 제품 고정본 `0a97e858`과 B1 제품 후보 `d8e2c52d`다. 후자는 외부 local
snapshot source이며 r5 Medium 진단 body는 fixed work cap, lo5/lr5 Large Off/Record는
원120초 deadline에서 각각 실패했다. typed-fee 후보 `a6083fb2`는 r6 build 통과 뒤
mr6 Medium에서 다시 fixed512MiB 진단 cap으로 실패했다. B2 r7의 Medium 진단 cap·
Large120도 FAILED다. B3 mr8 Medium 진단 cap과 lo8 Off·lr8 Record Large120도 FAILED다.
과거 성능 숫자는 기존 VALIDATION에서 가져왔으며 해당 과거 외부 원본은 부재다.
아래 mr2와 r4 실행은 각각 source와 바이너리 전후 일치를 확인한 별도 cohort이며 과거 replay가 아니다.
[현재 가용성](README.md#과거-근거)은 별도로 기록한다.

## B4 실행과 B5 코드 수선

고정 소스 `213908833f462447fc229519d1d6f1472e978644`의 f26 집중423개는
Storage85·Catalog27·Native297·Kernel14 전부 통과했다. f27 Storage/Catalog
lib+tests strict는 기존 Storage `object/read_only/tests.rs` 변환 lint2건으로
FAILED, 소유 변경 파일 진단0이다. r9 정상 cached pair build는4분28초에 완료됐다.
이는 local source-bound build이며 managed-fresh·전체 Main qualification이 아니다.

- mr9 Medium 진단은 full ACK9.282초 미완료, producer18.446초·exit1이다.
  740,061R/464,764W/536,863,832B/236,679M 뒤9,968B charge가 기존512MiB
  진단 cap을 넘어 FAILED다. Large의 선행조건으로 쓰지 않는다.
- lo9 Off는 full ACK125.522초 미완료, producer163.804초·exit1이다.
  12,653,629R/7,616,728W/8,967,287,949B/2,602,795M에서 원120초 deadline으로
  실패했다. cap·memory·external refusal이 아니다.
- lr9 Record는 full ACK124.534초 미완료, producer158.351초·exit1이다.
  15,638,380R/8,502,561W/9,967,787,855B/3,447,298M에서 같은 deadline이다.
  두 실행의 실패 관측 위치는 Storage `map/completed_group.rs:140`이며
  동일한 source·binary pair에 자원 sampler는 둘 다Off였다.

lr9 child896,891 events는 COMPLETE이며 dropped/unwritten/open/lease0이다.
실패 LexicalBuild는 wall109.084초/CPU68.677초, Postings는 wall63.131초/
CPU39.771초다. 첫64-term page의 apply는32.329초/CPU23.082초에 완료했고,
둘째 page apply는27.287초/CPU16.433초에서 실패했다. 첫 page의 직접 자식은
reserve5.455초·install10.666초·confirm14.108초·scratch envelope0.670초다.
Catalog reserve+confirm은19.563초(60.5%)다. 둘째 page도 Catalog17.711초
(64.9%)로 주 비용이다. 첫 page는41×256+10=10,506 input slot, 둘째는
23×256=5,888 slot을 처리했다. r8의 첫 page38×256 실패와 진행 endpoint가
다르므로 wall 비율이나 counter 차이를 격리된 B4 speedup으로 주장하지 않는다.

lo9 시작 직전 load30.48·swap 약28GiB, lr8의 wall−CPU와 많은 page miss는
호스트 경합·blocking이 공존함을 시사한다. 어느 wait가 host paging인지 정확히
분해한 증거는 없다. 또한 Catalog의 실제 SQL·CPU·checkpoint 비용도 있으므로
host-only로 결론 내리지 않는다. 비정상적으로 반복되는 같은 실행을 추가하지 않고
실제 edge 작업을 줄이는 B5를 구현한다.

B5는 posting의 dense Tree `ObjectRef`를 **비교 전용 digest witness**로 바꾼다.
원래 query는 Contribution inventory도 읽어 posting ref와 정확 비교했다. 새 query는
같은 inventory의 실제 ref를 얻어 witness와 비교한 뒤 그 ref만 load한다. 참조
일치·Some→Some CAS·source pin·TF/positions 검사를 버리는 순수 membership unit은
선정하지 않았다. Root→owners→Contribution→inventory가 실제 occurrence의
물리 GC 경로이며, witness는 역참조·등록 권한이 아니다. Small≤8의 기존 명시
참조는 유지해32B inline ABI와 수만 singleton의 객체 수를 보존한다.
새 소스의 기능 proof는 f31(`5e41d204…`) Native301·Kernel15와,
타입 별칭·테스트 borrow만 교정한 현행 `3a2ad5dd…`의 f35 변경2개 composite다.
f36 strict는 소유 진단0/전역33개 기존 진단으로 FAILED이며 전체 green이 아니다.
r10 pair build를 완료했고 lo10 Off와 lr10 Record 모두 원120초 제한에서
FAILED다. 계약·소유·테스트는
[B5 계획](WORKPLAN.md#b03-b5--dense-posting-tree의-비교-witness와-실제-custody-분리)을 따른다.

## B5 동일 Posting 배치 구간 비교

`3a2ad5dd…`의 lo10 Off는 ACK123.507초·producer154.318초에 실패했다.
lr10 Record는 sampler Off, ACK127.683초·producer168.426초에 실패했다.
두 실행 모두 BuildPrepare deadline이며 전체 ACK·검색·재오픈은 미도달이다.
lr10 child footer는890,079 events와 loss/open/lease0을 기록했다.

lr9와 lr10에서 첫 dense page의 완료된 처음27개 256-input 배치(6,912개)를
맞춰 비교했다. 아래 시간은 같은 종류의 직접 자식에 한정한다.

| 구간 및 계수 | lr9 B4 | lr10 B5 |
| --- | ---: | ---: |
| Catalog reserve wall / thread CPU | 3.401s / 2.189s | 2.023s / 0.470s |
| 객체 install wall / thread CPU | 7.019s / 5.210s | 14.391s / 6.091s |
| Catalog confirm wall / thread CPU | 8.703s / 6.001s | 6.855s / 2.594s |
| Catalog pager misses | 1,090,193 | 83,149 |
| Catalog VFS write calls | 67,478 | 33,647 |
| Catalog VFS requested bytes | 857,017,120 | 388,452,656 |

lr10은 첫 Posting page의 28번째 설치 정산에서 멈췄다. lr9는 첫 page
42개 배치(10,506 input)를 완료한 뒤 둘째 page에서 멈췄다. 같은 앞27개
배치에서는 Catalog의 반복 작업이 줄었고 객체 설치의 wall−thread CPU가
약1.809초에서8.300초로 늘었다. lr10 시작 직전 load61.76·swap3.84GiB와
다른 실행 cohort의 조건 차이가 있다. 스케줄링과 파일시스템 대기의 정확한
비율은 측정되지 않았다. 위 계수는 쓰기 요청량이며 장치 실측 속도 비교가 아니다.

같은 27개 install의 phase packet에서 namespace/create13,851회는
wall3.654→9.336초, CPU2.956→4.210초였다. settlement54회는
wall0.357→1.613초, CPU0.332→0.417초였다. 두 구간이 install wall 증가분
7.372초 중 약6.938초를 설명한다. 파일 동기화6,912회의 wall은
0.375→0.441초였다. B6는 기존 shard에 대한 반복 `mkdir` 호출을 제거한다.
각 객체의 live shard 검사, device와 inode 검증, 동기화 및 정산은 계속 수행한다.

Record 첫 실행은 도구의 mount device 식별자 변경으로 producer 시작 전에
거절됐다. 15개 경로의 나머지 epoch 메타데이터가 동일함을 검사한 뒤 새
runtime epoch를 고정했다. 재실행의 환경 차이는 PATH 한 키이며 canonical
도구, frozen source, 실행 파일 guard는 유지했다.

## B6 로컬 Large

`c5c691e…`의 lo11 Off는 공용 실행 lease를 확보한 뒤 실행했다.
producer155.742초·ACK123.772초에 실패했고, BuildPrepare의
`managed_install.rs:598`이 deadline을 관측했다. 이 줄은 `try_reserve_new`가
기존 행을 발견한 뒤 `require_reserved` 전에 작업량을 청구하는 지점이다.
Off 실행은 child trace가 비어 있어 해당 분기 시간이나 도달한 Posting 페이지를
알 수 없다. 사용량은13,349,970R/
8,296,258W/10,899,408,339B/3,160,481M이다. 수치·메모리·외부 자원
거절 없이 전체 ACK·검색·재오픈 미도달이다.

45초 안정화 관측의 1분 load는18.52~21.96이었다. 실행 전후의 호스트 전체
swap in/out은16KiB page 기준81,536/58,576개였다. 공용 lease는 같은 lock을
사용하는 Cargo 작업을 직렬화했지만 호스트 전체를 격리하는 조건은 아니다.
진행 끝점이 다른 lo10과의 사용량 차이는 B6 속도나 회귀를 증명하지 않는다.

실행 요약 controller의 mr8/lo8 경로가 남았던 두 요약 오류는 외부
`r9/controller-correction.json`과 `l13ctl/controller-correction.json`에 정정했다.
위 수치는 실제 `mr9`·`lo9`·`lr9/execution.json`과 refusal 원본이다. source/binary
guard는 유지됐고, 잘못 읽은 과거 summary를 새 제품 결과로 사용하지 않는다.

## B7에서 분리할 비용

- **반복 트리 생성.** cold Posting의 seed42 소스 투영에서 DF>8인
  Sensitive/Folded term 쌍157개의 owner와 witness 편집 입력이 같다. B7은
  두 case를 같은 페이지에 놓는다. Storage는 같은 base와 편집 count 및
  fingerprint가 일치하고 양쪽 scratch 재검증이 끝난 경우에만 이미 만든 root를
  공유한다. 기존 root를 만드는 비용은 줄일 수 있고, 양쪽 cursor·CAS·scratch
  쓰기와 재검증은 남는다. 기존 도달 DAG 중복 제거와 Catalog의 기존 객체
  채택 때문에 root 생성 감소량과 실제 파일 설치 감소량은 다를 수 있다.
  두 case 범위를 읽는 SQL·버퍼의 추가 비용도 함께 측정한다.
- **물리 설치.** B5의 동일 prefix에서 늘어난 install wall 7.372초 중
  6.938초가 namespace/create와 settlement에 있다. 현재 namespace/create는
  Store root·shard 확인과 writer의 runtime allowance 및 파일 생성을 합산한다.
  settlement는 synced 정산과 이어지는 budget checkpoint를 포함한다.
  B6의 기존 shard mkdir 제거와 별도 Runtime owner의 parent anchor 단일 경로는
  이미 작업 범위에 있다. live namespace, ticket, 파일 및 디렉터리 sync에는 각각
  수명·공간·내구성 조건이 있어 정적 감사만으로 더 제거할 호출은 확인되지 않았다.
- **호스트 대기.** 같은 prefix의 install wall−thread CPU는 1.809초에서
  8.300초로 늘었다. 파일시스템·capacity lock 대기와 CPU deschedule이 모두
  포함될 수 있다. B6의 호스트 전체 swap 증가는 경합이 있었다는 정황이다.
  해당 프로세스의 stall 시간과 원인별 비율은 미확정이다. B5/B6의 Off도 실패해
  Record 계측 비용이 deadline 실패의 필수 조건은 아니다.

측정에서 **생성 생략**, **실제 설치량 감소**, **남은 설치 대기**를 따로 판정한다.
B7 검증은 [이력](WORKPLAN.md#b7-검증-및-분기)에 있다. 현재 수정과 실행은
[B9 계획](WORKPLAN.md#b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기)을 따른다.

`e3e8512…`의 lo12 Off도 원 120초 조건에서 FAILED다. producer160.101초,
full ACK124.763초 미완료이며 `object.rs:593`의 batch child validation 진입
checkpoint가 BuildPrepare deadline을 관측했다. 사용량은
8,658,991R/5,917,929W/6,968,641,937B/1,581,697M이며 수치·메모리·외부 자원
거절은 없다. 실행 직전 45초의 1분 load는38.92~75.36이었다. Off는 세부
trace가 없고, lo11과 끝점·호스트 조건이 달라 사용량 차이만으로 B7 효과를
판정하지 않는다.

### 계측 후 B7 Record

`654d204…`의 lr12는 원 120초에서 FAILED다. producer159.261초,
full ACK124.740초 미완료다. Catalog child adopt 후 checkpoint에서 deadline을
관측했다. 세부 child capture는 COMPLETE, 923,752 events, loss/open/lease0이다.
원본은 외부 `qib6-_0r6mac7/lr12`의 execution·refusal·child trace에 있다.

| lr12의 도달 범위 | wall / thread CPU | 작업량 |
| --- | ---: | --- |
| Posting 전체 | 61.332s / 18.959s | 첫 page 완료, 둘째 page 실패 |
| Posting map apply | 60.829s / 18.708s | 두 page에서 후보64 / 검증 재사용64 / build 시도64 |
| Posting reserve | 5.550s / 0.788s | 46 packets |
| Posting install | 27.428s / 9.603s | 46 packets 성공, 11,759 inputs |
| Posting confirm | 19.792s / 4.417s | 45 packets 성공, 1 실패 |
| reserve·confirm 내부 SQLite COMMIT | 21.179s / 3.921s | 91회 |
| 설치 중 namespace 검사 | 1.180s / 0.387s | 11,805회 |
| 설치 중 writer 생성 | 10.515s / 5.587s | 10,387회, allowance와 실제 open 포함 |
| 설치 중 파일 sync | 4.336s / 0.273s | 10,387회 |

첫 page는16.559초/CPU9.211초, 둘째는44.270초/CPU9.497초였다.
두 page 모두32개씩 root build를 생략했고 설치 입력은 각각5,871/5,888개였다.
이는 재사용 동작의 직접 증거다. 다른 term 구간과 호스트 조건의 B5 대비
제품 속도 비교에는 쓰지 않는다. 표의 install 내부 phase와 상위 wall은
포함 관계다. non-Posting에도 writer 생성21,896회, wall10.159초/CPU9.039초가
있었다. reserve·confirm wall25.342초 중 21.179초가 COMMIT이었다. namespace
미세 최적화보다 writer와 기존 내구성을 유지하는 커밋 횟수 절감을 우선한다.
같은 managed packet 안에서 COMMIT 구간에 포함된 VFS sync는228회·wall5.966초였다.
모두 같은 FD의 full sync 성공을 기록했다. COMMIT 전체를 sync syscall이나
Catalog lock wait 한 가지로 설명할 수 없다.

실행 중 외부 host 표본32개의 load1은21.91~64.54였다. 표본 구간156.49초 동안
호스트 전체 swap in/out은 16KiB page 기준937,474/892,837개였다. 첫·둘째
page의 wall−CPU 차이와 일치하는 경합 정황이며, 호스트 paging·파일시스템·
lock 대기의 정확한 인과 배분은 없다. Runtime parent anchor 작업을 이 lane에서
중복 구현하지 않는다. B8은 관측된 재사용 leader의 두 scratch 순회를 한 번으로
줄이는 수리이며 나머지 물리 설치 비용을 별도로 검증한다.

### B8 재검증

f47에서 새 손상 거절·빈 EOF를 포함한 Rust 동작9개가 통과했다. sync label의
Rust/schema 순서 불일치 1개는 enum 집합을 유지한 채 수정해 f48에서 통과했다.
두 근거 사이에 Rust 본문 변경은 없다. 최종 `6df7803…`의 r14 pair build도
통과했다. 원 Large lo13 Off는 producer149.444초, ACK123.344초 미완료로
FAILED다. `text_spool.rs:1464`의 Posting cardinality loop에서 deadline을
확인했다. 사용량은13,240,653R/8,294,801W/10,364,017,241B/3,219,987M이며
수치·메모리·외부 자원 거절은 없다. 전체 ACK·검색·재오픈은 미도달이다.

lo13 실행 직전 load1은18.87~27.57이었다. 실행 전후 호스트 전체 swap in/out은
16KiB page 기준167,903/129,587개였다. lo12보다 더 많은 작업량을 처리했지만
호스트 조건과 도달 끝점이 달라 B8의 제품 속도 효과는 미확정이다. lr12에서
cardinality 두 page에 걸린 시간은0.251초였다. lo13 checkpoint만 보고
cardinality를 새 병목으로 선택하지 않는다.

현재 Catalog API의 예약 COMMIT은 파일 생성 전에 provisional intent 및
reclaim 보호를 영속화한다. 확인 COMMIT은 다음 chunk가 사용할 child를
등록한다. 이 계약과 현재256개/bytes cap을 유지한 채 두 COMMIT을 합치는
로컬 경로는 확인되지 않았다. 다음 구조 수리는 물리 writer의 allowance·
temp anchor·open 비용을 구분하고 기존 Runtime parent-anchor 변경의 범위를
대조한다. Catalog는 독립 namespace나 내구성 완화를 도입하지 않고
반복 가능한 COMMIT·sync 비용을 기준으로 transaction 경계 재설계를 평가한다.

### B9의 수리 경계

같은 lr12의 첫·둘째 Posting page에서 wall은16.559→44.270초, thread CPU는
9.211→9.497초다. 각 page의 COMMIT은5.420/1.814초와15.759/2.107초
(wall/CPU), writer 생성은4.354/3.141초와6.161/2.446초였다. 둘째 page는
기존 파일1,372개도 검증했다. 같은 실행 안에서도 대기가 크게 달라졌고,
off-CPU를 호스트 paging이나 저장장치 대기 한쪽에 모두 귀속할 수 없다.

수리 가능한 코드 비용은 writer의 동일 FD 재조회와 두 packet 사이의 연속
Catalog COMMIT이다. 두 packet이 모두 찬 lr12와 비슷한 범위라면 46개의
reserve·confirm 쌍이 요구한 최대92회를 69회로 줄이는 구조다. 이 값은
구조상 횟수이며 새 제품 실행의 wall 개선으로 사용하지 않는다.
[B9 계약과 검증](WORKPLAN.md#b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기).

`d2c6ff8…` lo14 Off는 producer159.143초, ACK123.651초 미완료로 FAILED다.
`map/completed_group.rs:140`에서 BuildPrepare deadline을 확인했다. 사용량은
14,010,583R/8,609,851W/11,188,135,972B/3,442,595M이며 수치·메모리·외부
자원 거절은 없다. 실행 중 host load1은25.08~31.58이다. 결과는 원 제품 실패다.
Off checkpoint와 작업량만으로 실행 구간별 비용·제품 개선을 판정하지 않는다.

같은 pair의 lr13 Record도 producer156.479초, ACK123.934초 미완료로 FAILED다.
`fence_counter.rs:167`에서 BuildPrepare deadline을 확인했다. child는 COMPLETE,
1,009,588 events·loss/open/lease0이다. 첫 두 Posting page는 완료했고 세 번째
page가 중단됐다.

| 동일한 첫 Posting page | B7 lr12 | B9 lr13 |
| --- | ---: | ---: |
| 물리 install packets / inputs | 23 / 5,871 | 23 / 5,871 |
| SQLite COMMIT 횟수 | 46 | 35 |
| SQLite COMMIT wall / thread CPU | 5.420s / 1.814s | 5.142s / 2.131s |
| Map apply wall / thread CPU | 16.559s / 9.211s | 15.977s / 9.938s |

lr13 전체 Posting 도달 구간의 원자 전환은27회 성공·1회 실패였고, 성공한
COMMIT은84회다. 성공한 전환 하나가 두 원래 Catalog 단계의 commit 하나를
제거했다. 전체 Posting은47.164초/CPU25.768초, install은18.852초/CPU12.061초,
그 안의 writer 생성은11,796회·8.495초/CPU6.744초다. Catalog COMMIT은
13.347초/CPU5.244초였다. 종료 지점과 호스트 조건이 달라 전체 속도 비율은
구하지 않는다. 코드의 transaction 수 절감은 동일 prefix에서 직접 확인됐다.

Off/Record의 실행 중 host load1은 각각25.08~31.58 / 20.11~39.06이다.
동시 표본에서 호스트 전체 swap in+out은 약9.19/5.94GiB다. CPU와 wall의
차이는 대기 시간을 드러내지만 프로세스 paging·장치 I/O의 분리는 남는다.
다음 writer 계측은 ticket 준비와 실제 create/open 구간에만 필요하다. 별도
Runtime parent-anchor 작업의 수용 결과와 대조하고 중복 경로를 만들지 않는다.
원120초 전체 ACK·검색·재오픈은 열린 상태다.

### B10 — Posting 전 contribution 설치

lr13의 실패한 LexicalBuild에서 Posting 전 구간은64.160초였다. inventory
prepare22.369초, map apply13.590초, inventory diff9.700초와 직접 자식인
contribution reserve/install/confirm3.507/5.513/6.243초가 포함된다.
contribution은64개씩64회 물리 설치했고 두 Catalog 단계에 COMMIT128회가
필요했다. cold에서 기존256개 물리 상한까지 묶으면 4,096개 기준 설치16회,
COMMIT32회가 되는 구조다. scratch SQL은 계속64행이며 현재 수치는 코드
구조에서 계산한 상한이다. 실제 횟수와 시간은 B10 실행으로 확인한다.

나머지 inventory occurrence→entry map은 다음 root를 읽기 전에 앞 packet의
확인이 필요하다. 두 경계는 그대로 유지한다. writer의 Runtime FD 인증은
owner/mode까지 전달하지 않으므로 Storage의 검사와 임의로 합칠 수 없다.
이번 변경은 기존 directory FD 검증과 writer 비용 분리까지 적용한다.
[B10 계약](WORKPLAN.md#b03-b10--cold-contribution-배치와-writer-잔여-비용-분리).

`7cedcb7…`의 lo15 Off와 lr14 Record는 BuildPrepare deadline으로 FAILED다.
producer169.385/170.784초, ACK125.545/124.883초 미완료이며 검색·재오픈은
미도달이다. lr14의 harness trace는 닫혔지만 daemon child는 1,008,882 events,
drop485·open1인 PARTIAL이다. 아래 값은 그 child에서 도달한 구간이다.

| 도달 구간 | 관측 결과 |
| --- | --- |
| contribution 설치 | 동일 4,096 input에서64→16 packets |
| LexicalBuild의 inventory·Posting 밖 COMMIT |130→34회 |
| Posting writer ticket 준비 |13,439회·7.011s wall / 5.587s thread CPU |
| Posting writer create/open |13,439회·1.634s / 1.077s |
| Posting writer 생성 witness |13,439회·0.148s / 0.112s |

동시 host load1은 Off21.53~33.21, Record16.51~28.51이었다. 표본 중 호스트
전체 swap in+out은 약50.35/37.54GiB다. 이 수치로 프로세스 stall을 직접
분리할 수 없고 두 실행을 tracing overhead 비교에 사용하지 않는다.

ticket 준비가 writer의 주 비용이다. 코드 대조에서 Native writer가 1바이트
예약과 최종 크기 증액을 연속 실행하는 것을 확인했다. 기존 claim 안에서
최종 bound를 예약하면 두 번째 root·maintenance 검사와 growth ticket 갱신을
제거할 수 있다. 생성 직전과 publish 경계의 독립 인증은 유지한다.
[B11 계약](WORKPLAN.md#b03-b11--native-writer의-최초-전체-크기-예약).

### B11 — 최초 전체 writer 예약 검증

`add8f4c…` f53의 Runtime writer12개와 r17 pair build는 통과했다. lo16 Off와
lr15 Record는 원120초에서 FAILED다. producer160.818/155.661초,
ACK124.449/124.080초 미완료이며 전체 ACK·검색·재오픈은 미도달이다.
Off는 `object.rs:773`, Record는 `text_spool.rs:1464`의 BuildPrepare checkpoint에서
deadline을 확인했다. lr15 daemon child는 COMPLETE, 1,005,615 events·loss/open0이다.

| 동일 첫 Posting page | B10 lr14에서 관측 | B11 lr15 |
| --- | ---: | ---: |
| 실제 writer 생성 | 5,871 | 5,871 |
| ticket 준비 wall / thread CPU | 2.809s / 2.507s | 1.366s / 1.334s |
| Posting map apply wall / thread CPU | 13.855s / 9.132s | 15.288s / 5.851s |

두 번째 완료 page도 writer 생성4,635회로 같았고 ticket 준비 CPU는
1.860→1.384초였다. 첫 page에서 CPU가 줄면서 wall은 늘었으므로
제품 속도는 따로 판정한다. B11은 1바이트 예약·증액 경로 제거와 동일 구간
ticket CPU 감소를 확인했다. 4,096 contribution의 16 packets도 complete child에서
재확인했다.

Off/Record의 동시 load1은 각각23.18~29.95 / 14.18~23.96, 표본 중 호스트
전체 swap in+out은 약55.09/68.83GiB다. 제품 수용 미완료를 호스트 한 가지로
귀속하지 않는다. 남은 writer temp anchor는 별도 Runtime owner 코드와 합류 후
동일 parent FD가 실제 create·정산에 이어지는 범위를 확인한다. contribution
wrapper 제거는 기존 정책과 이전 reader가 읽는 persisted 값의 호환 설계가
선행돼야 하므로 이번 변경에 포함하지 않았다.

## B12 — writer parent custody와 Catalog COMMIT 잔여 비용

B11의 동일 앞 두 Posting page는 물리 설치 그룹23/24개, Catalog COMMIT35/36회다.
COMMIT 합계는 wall13.021초와 thread CPU3.254초다. 같은 managed-object parent
아래의 유일한 COMMIT 구간과 시간 포함관계로 VFS 이벤트를 매칭했다. WAL sync
164회와 DB sync46회는 모두 성공·same FD·full-default였고, 해당 VFS 구간은
총5.913초였다. settlement와 checkpoint까지 합친 구간 합집합은11.847초다.
VFS sync 구간에는 native 호출과 사후 witness 검증이 함께 들어간다. 따라서
off-CPU9.767초를 디스크 대기와 호스트 스케줄링으로 정확히 나눌 수는 없다.
Catalog의 WAL, `synchronous=FULL`, `fullfsync=ON`, IMMEDIATE write transaction과
기존 인접 두 설치 그룹의 atomic advance를 확인했다.

연속 그룹까지 advance를 확장하면 두 page의 COMMIT이 이론상71→49회다.
현재 구현은 combined 64MiB, sorted union gate, 동일 attempt, 전 단계의 물리
내구성을 함께 보존한다. Rolling 연결에는 기존 provisional custody를 유지하면서
gate를 재획득할 때 GC·취소·재시도·교차 실행이 안전하다는 증명이 필요하다.
한 단계의 SQLite 설정을 바꾸거나 트랜잭션 경계를 제거해 이 비용을 줄일 근거는 없다.

B12는 최종 claim에서 연 부모 FD를 임시 이름에 재사용하고, 생성과 no-replace
publish를 held FD에서 실행한다. 미등록 FD도 기존 qualified cleanup에 연결했다.
초기 고정본 `8f114a2…` f54는18/19개였다. Retarget 후 retry case가 ticket의
allowance 종료 전 snapshot을 비교해 실패했고, 종료 뒤 실제 환급을 확인하도록 교정했다.

추가 감사에서 경로로 연 대체 shard를 fsync한 뒤 원래 shard가 복원되는 ABA를
확인했다. Live pathname과 held parent가 정산 시 다시 같아져도 실제 rename
대상에 대한 fsync를 증명할 수 없다. B12 후속은 기존 barrier·fsync 횟수를
유지하면서 실제 sync한 objects와 shard FD의 식별자를 정산에 전달한다.
Objects FD는 전체 batch의 정산이 끝날 때까지 보유한다. Runtime은 기존 nofollow
ancestor walk에서 두 식별자와 원래 held shard 및 inode를 확인한다.
`2e19d9d…` f55는 실제 fsync 후 ABA, 잘못된 objects inode, 원본 FD 취소,
scalar/batch·Catalog replay를 포함해23개가 통과했다.

같은 고정 소스의 Large Off/Record는 둘 다 원120초 `BuildPrepare` deadline으로
실패했다. Off는 Storage batch settlement 뒤 checkpoint, Record는 다음 native
writer admission에서 거절됐다. client ACK 구간은124.819/147.588초, producer는
156.646/179.860초다. 숫자 work cap·memory·외부 admission 거절은 관측되지
않았으며 검색·재오픈에는 들어가지 못했다. Record child는 COMPLETE이고
1,100,192 events, loss/open0이다.

B11과 B12 Record에서 정상 완료한 동일 첫 세 Posting page를 대조했다.
두 실행 모두 Catalog 입력18,015개·실제 writer14,486개·COMMIT107회다.

| 구간 | B11 thread CPU | B12 thread CPU |
| --- | ---: | ---: |
| writer ticket prepare | 3.962초 | 4.838초 |
| writer open | 0.908초 | 2.455초 |
| publish/retire | 1.451초 | 3.437초 |
| settlement | 0.682초 | 2.197초 |
| 네 writer 구간 합계 | 7.004초 | 12.928초 |
| Catalog COMMIT | 5.566초 | 5.939초 |
| MapApply | 22.613초 | 30.190초 |

동일 작업의 writer CPU 증가는 새 parent 확인·FD 인증 경계에 집중된다.
B12 Off/Record의 host load1은14.41~21.26/15.83~56.00, 호스트 전체 swap
in+out은 약39.92/73.94GiB였다. Off/Record의 ACK 차이는 순수 trace overhead로
귀속할 수 없다. B13은 fsync identity 보존과 실제 open·rename 직전 확인은
유지하면서 그 사이의 중복 parent 확인을 줄인다.

## B13 — writer CPU 회수와 최상위 map 잔여

`44486c9…` f57은 Runtime·Storage23개를 통과했다. r19 pair build를 고정해
seed42·4,096 source·16 repo·원120초로 실행했다. Off/Record의 producer는
153.303/153.167초, client ACK 구간은124.150/124.491초였고 둘 다 full ACK
미완료다. Record daemon의 실제 요청은120.149초에 거절됐다. 양쪽 모두
Storage batch settlement 뒤 checkpoint에서 `BuildPrepare` deadline에 걸렸다.
정량 work cap·메모리·외부 자원 거절은 없었다. 검색·재오픈은 미도달이다.

Record child는 COMPLETE, 1,203,874 events, loss/open0이다. B12와 동일한
앞 세 Posting page의 입력18,015개·writer14,486개·COMMIT107회에서 네 writer
구간 thread CPU가12.928→8.269초로 감소했다. 세부 구간은 ticket 준비
4.838→2.381초, open2.455→1.821초, publish/retire3.437→2.409초,
settlement2.197→1.657초다. MapApply CPU는30.190→19.561초였다. Off/Record의
host load1은16.41~23.30/14.39~30.81, 호스트 전체 swap in+out은
약17.42/33.05GiB여서 순수 tracing overhead는 판정할 수 없다.

B13 Record는 개별 Posting page 이후 `NativeTextPostings` lane39의 최상위
terms/owners `apply_stream`에 도달했다. 실패 prefix의 MapApply는
wall7.057초·thread CPU4.423초였다. 물리 설치는14개 그룹·3,584개 writer,
Catalog COMMIT은20회·wall2.664초·thread CPU1.039초였다. Lexical BuildPrepare
112.737초 중 Posting 전체 구간은58.437초였다. 최상위 map의 전체 실행 시간은
관측되지 않았다. 이 위치를 deadline의 마지막 비용으로 식별했고, 제품 전체
120초 통과 조건은 열어 둔다.

### B13 요청 전체 원인 분해

`lr17`의 닫힌 daemon child에서 실패한 요청의 dispatch·build span과 하위
구간을 재집계했다. Daemon은 queue admission 이후 원120초 budget을 발급한다.
Harness client와 외부 producer는 각각600초 상한이다. Trace상 서버
queue wait는 반올림0초였고, 120.149초는 실패한 daemon 요청 span이다. Client
ACK 구간124.491초에는 read-view·mutation staging·submit·status가 들어가며,
producer153.167초는 별도 source staging과 프로세스 수명도 포함한다.
Record의 reads는 원 cap의 약21%, bytes는 약2%에서 deadline이 먼저 거절했다.
[Native 정책](../../../crates/quanta-index-searchd-runtime/src/native.rs#L104),
[서버 budget](../../../crates/quanta-index-ipc/src/server.rs#L1870),
[harness client 상한](../../../crates/quanta-index-searchd-harness/src/scale/native_index.rs#L70).

| 구간 | Wall | 관측 thread CPU |
| --- | ---: | ---: |
| Daemon 요청 | 120.149초 | — |
| BuildPrepare 진입 전 | 7.349초 | — |
| Lexical build | 112.737초 | 60.952초 |
| ↳ Posting 전체 | 58.437초 | 30.574초 |
| ↳ Inventory 준비·diff·map 적용 | 44.456초 | 24.271초 |
| ↳ 나머지 lexical 구간 | 9.844초 | — |

같은 BuildPrepare prefix의 관리 객체 설치는254개 그룹·39,555개 입력·실제
writer33,019개였다. 물리 설치 span은 wall37.625초·thread CPU25.449초다.
이 중 데이터 write 호출의 wall 합계는1.880초였고, writer 준비·open·
publish/retire·settlement의 thread CPU 합계는20.700초였다.
Catalog 관리 COMMIT은453회·wall32.290초·thread CPU9.361초였다. 동일
관리 호출 부모와 COMMIT 구간 포함관계로 WAL/DB VFS sync994회를 매칭했고,
그 구간은13.772초다. 이 값에는 native sync와 VFS witness가 포함된다.
별도 객체 shared barrier254회의 wall은4.595초다. 모두 위 단계에 이미
포함된 값이다. Lexical wall과 관측 thread CPU의 차이51.785초에는 sync,
스케줄링, 계측 범위 밖의 대기가 포함될 수 있다. 호스트 swap과 daemon의
대기를 같은 양으로 환산할 수 없다.

마지막 `terms/owners` 적용에서는 기존 top scratch를 읽어 MapScratch에 edit를
다시 기록한다. 중복 staging은 코드에서 확인된다. 관측된7.057초 prefix에는
물리 설치14개 그룹3.146초와 Catalog COMMIT20회2.664초도 들어간다. 중복
staging의 독립 시간과 최상위 map의 완료 시간은 미측정이다. 해당 시점의
`object.rs:1049`는 settlement 이후 deadline checkpoint다.
[top stream](../../../crates/quanta-index-lexical/src/native/text.rs#L9981),
[Storage edit staging](../../../crates/quanta-index-storage/src/map.rs#L2073).

완료된 첫 네 Posting page의 물리 그룹90개는 기존 pair advance에서
COMMIT136회였다. 각 페이지에서 모든 인접 그룹을 연쇄로 묶는다면 수학적
최솟값은 합계94회다. 절감량으로 사용할 수치는 아니다. 새 계약에서
64MiB·sorted gate·provisional custody와 취소·GC·재시작을 보존해야 한다.
현재 file/dir sync와 Catalog WAL/FULL/fullfsync는 내구성 경계다.
[Catalog 연결 정책](../../../crates/quanta-index-catalog/src/connection.rs#L465).

로컬 Off·Record 모두 실행 전 host settle 조건을 충족하지 못했다. 원 E07은
전용 Linux aarch64 2CPU/6GiB·local SSD와 paired Off/Record를 요구한다.
같은 고정 실행 파일의 원120초 Off를 경합이 줄어든 환경에서 먼저 확인한다.
여전히 실패하면 별도 진단 후보의 Native 서버 budget을 늘려 완주 시간을
측정한다. Client watchdog과 Query 서버 budget은 이 Native 제한을 바꾸지
않는다. 진단 후보의 완주 결과는 원120초 수용과 분리한다.
[E07 고정 profile](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles).

## B14 — 물리 그룹 축소와 기존 advance 연결

B13의 동일 Large fixture에서 occurrence와 inventory block은 각각
64개 객체 × 64그룹이었다. Posting의 큰 그룹은 대부분 이미256개였다.
초기 empty-base inventory 창을128개로 확대하고 SQL row page64는 분리했다.
Reachable Map은 합계64MiB 이내의 최대4개 기존 물리 packet을 하나의
sorted gate union과 연속 Catalog advance로 처리한다. 각 packet의
파일·디렉터리 barrier와 정확한 FD 정산이 다음 advance보다 먼저 끝난다.

초기 r21 Large Off는17.280초의 ACK 구간에서 `BuildPrepare`의
`ResourceLimited`로 실패했고 `WorkBudget.first_denial`은 `none`이었다.
같은 pair의 별도 Medium Record는 occurrence128개 물리 설치를 완료한 뒤
첫 `NativeTextInventoryMapApply`에서 실패했다. child17,472 events,
loss/open0이다. 빈 inventory에서 64개를 초과하는 Block을 준비한 후
공용 staging 생성자의64개 검사에 걸리는 소스 경로와 일치한다.
`60db2fc…`는 cold inventory만 기존 물리 packet 상한을 사용하는 생성자로
연결했다. ACK·검색·재오픈 판정은 이 교정본의 실행에 귀속한다.

교정본 Large Record의 B13 `lr17` 대 B14 `lr19` 비교는 occurrence와 inventory
block 각각의 동일한 첫 2,816개 입력으로 제한한다. 각 경로에서 44×64가
22×128 물리 배치로 줄었다. 두 경로 합산 물리 배치는 88→44, reserve·confirm
Catalog COMMIT은 176→88이다. 실제 설치하는 고유 객체 수는 동일하다.
`lr19`는 이후 inventory scratch 단계에서 종료돼 전체 요청의 절감량은 미측정이다.

Large Off `lo20`은 batch namespace checkpoint에서, Record `lr19`는 scratch
SQLite edit write에서 원120초 deadline을 관측했다. 두 지점은 예산이 소진된
시점이다. 공유 Mac의 1분 load 관측 범위는 각각 28.48–37.95와 37.03–123.56이다.
실행 사이 경합 차이 때문에 ACK 차이로 코드 속도나 Record 오버헤드를 산출할 수 없다.
두 실행 모두 ACK 미완료였고 `lr19`는 최상위 map에 도달하지 않았다.

같은 pair의 Medium `mb15`에서 첫 최상위 terms/owners map은 1,512.5ms였다.

| 단계 | Wall time |
| --- | ---: |
| Cursor·검증·재적재 | 6.3ms |
| 위 단계에 포함된 edit scratch 쓰기 54회 | 2.6ms |
| Scratch replay·radix 구축 | 19.6ms |
| Scratch COMMIT | 10.1ms |
| 물리 설치 | 1,436.8ms |

물리 설치 구간에서 그룹 크기는 256·256·120개였다. Catalog COMMIT은 최초
reserve 1회, advance 2회, 최종 confirm 1회였다. 네 개 map 하위 단계의 합계는
edit scratch 쓰기를 중복 계상하지 않으며, 상위 span은 이 밖의 작업도 포함한다.
수집기는 v5 `map_apply` lane의 CPU counter와 부모 관계를 검사한다. 교정한
수집기로 보존된 first/reopen trace를 오프라인 검증했다. Medium 수치를 Large의
최상위 map 비용으로 환산할 근거는 없다.

## B15 — Inventory scratch와 물리 writer 병목

`lr19`가 120초 제한을 관측한 경로는 `BuildPrepare → LexicalBuild →
NativeTextInventoryDiff → NativeTextPostingEdit/33`이다. `edit_write.rs`에서
`raw_execute()`가 반환된 직후 checkpoint가 deadline을 거절했다. 마지막 호출은
135ms였다. 최상위 terms/owners `MapApply`는 미도달이며 앞에서 관측된 Storage
`map_apply` 34개는 모두 member assembly 소속이다. 22개 inventory map 배치는
EntryMap Block 파일을 설치했다.

| 동일 seed42·corpus의 비교 구간 | B13 `lr17` wall / thread CPU | B14 `lr19` wall / thread CPU |
| --- | ---: | ---: |
| 성공한 inventory posting edit 첫 11,769회 | 3.837 / 2.329초 | 18.032 / 3.296초 |
| 첫 occurrence 2,816개 물리 설치 | 4.319 / 2.546초 | 8.051 / 3.148초 |
| 첫 inventory Block 2,816개 물리 설치 | 4.111 / 2.551초 | 10.632 / 3.256초 |
| 위 occurrence Catalog COMMIT 88→44회 | 3.969 / 0.626초 | 7.521 / 0.426초 |
| 위 inventory Block Catalog COMMIT 88→44회 | 5.039 / 1.206초 | 9.809 / 1.441초 |

Posting edit 비교는 성공 span 순번을 맞췄다. trace에 행별 식별자는 없다.
`lr19`의 inventory diff 하위 posting edit 전체는 11,770회·wall 19.125초·thread CPU
3.324초이고, 중첩된 SQLite VFS lane22의 wall은 14.148초다. 따라서 두 시간을
합산하면 중복이다. 초기 posting edit는 owner 사이에서 유지되고 64행을 SQL 한 번으로
실행하므로 해당 호출 수 자체를 중복 쓰기로 분류할 근거가 없다.

`lr19`의 물리 설치는 새 writer 17,800개를 처리했다. 누적 wall/thread CPU는
ticket 준비 9.065/5.137초, open 5.952/3.835초, publish·retire 9.672/5.163초,
settlement 5.645/3.241초, 데이터 쓰기 2.291/0.649초였다. Barrier는 wall
3.215초·thread CPU 약 7ms였고 물리 쓰기와 sync 오류는 0건이었다.

`lr19` LexicalBuild의 wall 105.240초 중 해당 thread CPU는 26.239초였다.
같은 실행의 호스트 load1은 37.03–123.56, 호스트 전체 swap in+out은 약 46GiB였다.
비교 구간에서 wall 증가가 CPU 증가를 크게 초과했고 횟수가 줄어든 COMMIT에서도
같은 패턴이 관측됐다. 이 차이에는 SQLite 및 파일시스템 대기, lock 대기와
스케줄링 지연이 포함된다. trace가 각 항목의 시간이나 quiet-host 결과를 직접 측정하지 않는다.

코드에서 제거한 작업은 동일 값의 과금용 재인코딩, 동일 scratch 예약량 재조회,
동일 inode 확인 직후의 배치 writer 재조회다. 물리 파일 수와 내구성 sync 횟수,
논리 work caps는 기존 계약을 따른다. 이 변경의 120초 수용은 [실행 결과](README.md#현재-판정)에 기록한다.

### B15 교정본의 재검증

| 같은 비교 구간 | B14 `lr19` wall / thread CPU | B15 `lr20` wall / thread CPU |
| --- | ---: | ---: |
| 성공한 inventory posting edit 첫 11,769회 | 18.032 / 3.296초 | 2.892 / 2.429초 |
| 첫 occurrence 2,816개 물리 설치 | 8.051 / 3.148초 | 3.666 / 2.613초 |
| 첫 inventory Block 2,816개 물리 설치 | 10.632 / 3.256초 | 4.047 / 2.682초 |

`lr20`은 occurrence와 Block을 각각 32×128개 설치했다. 두 경로 합계 Catalog
COMMIT은 128회였다. 첫 두 Posting page는 각각 5,871·6,029개 입력과
23·24개 물리 배치를 처리했다. B13 동일 페이지의 COMMIT 35·36회 대비 29·30회였다.
세 번째 Posting page는 17개 물리 배치를 시작한 뒤 namespace checkpoint에서
원 `BuildPrepare` deadline에 걸렸다. 실패한 page의 MapApply 9.382초 중
물리 설치 lane은 7.825초였다. 최상위 map과 ACK·검색·재오픈은 미도달이다.

호스트 load1은 `lo21` 39.05–52.64, `lr20` 33.46–51.92였고 swap in+out은
각각 6.82GiB, 0.26GiB였다. `lr19`의 46GiB swap과 환경이 달랐다. 동일 구간의
CPU 감소는 관측됐으나 코드 단독 wall 개선폭이나 120초 여유는 이 비교로 확정할
수 없다. swap이 적었던 Record에서도 deadline이 남았으므로 swap 수치만으로
실패 전체를 설명할 수 없다.

Medium `mb16`은 동일 r25 pair의 256-source 기능 검증이다. ACK 4.789초,
첫 검색 65.5ms, 증분·삭제 ACK 및 검색, 재오픈 후 검색과 최종 drain이 완료됐다.
두 child trace와 canonical profile이 검증됐다. Large 120초와 경합을 통제한
비교는 계속 열린 상태다.

## B16 — Posting writer와 Catalog COMMIT

B15 `lr20`에서 세 번째 Posting page는 17×256개 입력의 물리 배치를 시작했다.
16개 배치가 barrier에 도달했고 마지막 배치는 namespace checkpoint의 원120초
deadline으로 종료됐다. 이 페이지의 MapApply 9.382초 중 설치 lane은 7.825초였다.
설치 lane 안에 아래 writer·COMMIT·barrier 구간이 포함된다.

| 구간 | 횟수 | Wall / thread CPU |
| --- | ---: | ---: |
| 신규 writer ticket 준비 | 2,615 | 0.705 / 0.638초 |
| 신규 writer open | 2,615 | 0.555 / 0.488초 |
| 게시 및 writer 종료 | 2,615 신규 파일의 처리 구간 | 0.731 / 0.618초 |
| 정산 | 배치별 정산 구간 | 0.401 / 0.391초 |
| 기존 객체 바이트 검증 | 1,591 | 1.124 / 0.594초 |
| Barrier | 16 | 0.322 / 약 0.001초 |
| Catalog reserve COMMIT | 5 | 0.447 / 0.147초 |
| Catalog advance COMMIT | 12 | 1.755 / 0.865초 |
| Catalog confirm COMMIT | 4 | 0.443 / 0.203초 |

COMMIT의 wall−thread CPU는 1.430초, 신규 writer의 네 구간 합계는 0.257초다.
SQLite와 파일시스템의 대기, lock 대기, 호스트 스케줄링이 모두 이 차이에 들어갈 수 있다.
기존 두 Posting page의 같은 입력·물리 배치 수에서 B13→B15 COMMIT은 71→59회,
wall 7.291→9.643초, thread CPU 2.488→3.262초였다. B13 host load1 14.39–30.81,
B15 33.46–51.92로 다르고 swap 변화는 반대 방향이어서 단독 원인을 확정할 수 없다.

기존 writer가 수행하는 held-parent·원본 FD 인증, no-replace 게시, 파일 및 디렉터리
동기화, 정산과 기존 객체의 바이트 검증은 유지한다. 수정한 반복은 최대4그룹마다
마지막 confirm과 다음 reserve를 각각 COMMIT하던 경계다. 최대8그룹을 연결해
동일한 23·24개 물리 배치가 64MiB 창에 들어갈 때 첫 두 페이지의 COMMIT은
구조상 29→26회, 30→27회다. 창 안의 메모리 승인과 256개 gate 상한은 유지되며,
실제 동시 요청의 gate 대기와 Large 수용 여부는 실행 결과에 귀속한다.

### B16 교정본의 재검증

`adf4c8ed…` 집중 회귀의 8그룹 성공 경로는 reserve 1회, advance 7회,
confirm 1회 순서였다. 9그룹 및 64MiB 초과는 첫 효과 전에 거절됐고, 후반
advance 실패 시 이후 파일은 만들어지지 않았다. 합계 9회 트랜잭션의 전이
순서는 확인했다. B15의 첫 두 Large 페이지를 같은 입력으로 비교한 횟수는 미측정이다.

`lo22` Off는 posting cardinality 쿼리의 checkpoint에서, `lr21` Record는 첫
Posting page `MapApply/3`의 radix 구축에서 `BuildPrepare` deadline을 관측했다.
`lr21`은 두 inventory 경로 각각 4,096개를 설치했지만 Posting의 `MapApply/5`
물리 설치에는 진입하지 못했다. 따라서 B15 세 번째 페이지의 writer 시간이나
COMMIT 횟수와 B16 Large를 직접 비교할 수 없다. 수치 work cap 거절은 없었다.

Off와 Record의 호스트 load1은 각각 25.21–40.74, 30.32–54.69였고 호스트 전체
swap in+out은 각각 86.78GiB, 55.63GiB였다. B15 `lr20`은 0.26GiB였다.
이번 실패 위치는 B15보다 앞이었으며 대기 시간을 B16 코드 변화에 귀속할 수 없다.
같은 r26 pair의 Medium에서는 ACK·검색·증분·삭제·재오픈과 최종 drain이 통과했다.
[실행 판정](README.md#현재-판정).

### 2026-11-09 후속 감사

- Main의 cold case-paired Posting, 분리 cardinality 범위와 childless witness
  구현은 B16 실행 소스와 동일하다. f61에서 case 페이지 경계·전체 키·잘못된
  surface와 witness Small/Tree 전이·CAS 회귀가 통과했다.
- 같은 첫 Posting 페이지에서 scratch write lane2는 두 실행 모두 3,767회다.
  B15 `lr20` wall/thread CPU는 0.511/0.447초, B16 `lr21`은 2.769/0.857초다.
  `lr21`의 cardinality는 0.267초에 완료했고, radix는 0.458초 진행 뒤 deadline에
  걸렸다. 실패 지점만으로 cardinality나 radix를 새 병목으로 선정할 근거는 없다.
- `mb17`의 Posting 페이지는 각각 최대 2개 물리 배치다. f63의 8그룹 전이와
  실제 Catalog 원자 전이는 검증됐으나, 일반 Posting 호출이 4그룹 경계를 넘을 때의
  실제 COMMIT 수를 관측한 제품 trace는 없다. 5–8개 물리 배치를 만드는 한 페이지에서
  이전 4그룹과 현 8그룹의 COMMIT 수, 같은 입력·출력·내구성 순서를 대조하는 것이
  가장 좁은 다음 검증이다. 원 Large 120초의 full ACK·검색·재오픈은 별도 수용 조건이다.

## B17 — 설치 창 리뷰 보완

B16의 64MiB는 wire 합계다. `MapBuffer`의 backing, `PreparedObject`의 live wire와
설치 전 `RegistrationBatch`가 요청 메모리를 함께 사용한다. 8배치는 기존 4배치에
들어가던 요청도 `MemoryScope`가 작은 경우 거절할 수 있다. 제품 경로에서 절감이
검증된 4배치 창으로 복원했다. 임의의 `ResourceLimited`를 잡아 재시도하면 이미
실행한 Catalog·파일 효과와 충돌할 수 있으므로 창을 설치 시작 전에 고정한다.

이전 gate 획득은 뒤쪽 gate가 잠겼을 때 낮은 순번의 prefix를 보유한 채 대기했다.
새 경로는 순서대로 한 번 시도한 뒤 충돌 시 prefix를 전부 놓고, 그 gate 하나만
기다린 뒤 다시 시도한다. 전체 gate를 얻은 뒤의 내구성·Catalog 전이 순서는 같다.
단위 회귀는 고의로 뒤쪽 gate를 잡고 실패한 union의 앞쪽 gate가 즉시 비는지 확인한다.

이전 관측은 atomic advance의 이전·다음 객체를 모두 confirm에 더했다. 새 관측은
이전 확인과 다음 예약을 각각 센다. 예를 들어 단일 객체 4배치가 성공하면 논리
예약·확인은 각각 4건이며 Catalog 전이는 reserve 1 + advance 3 + confirm 1이다.
한 advance 호출의 wall time은 기존 `confirm_elapsed`에 한 번만 기록한다.
atomic rollback은 두 논리 방향의 실패에 각각 반영한다. 기존 진단 필드와 trace
스키마는 유지한다. 이전 raw 요약의 reserve/confirm 수치는 이번 결과와 의미가 다르다.

### Large 수용 종료가 지연된 이유

B14–B16의 원 Large Off/Record 6회는 모두 `BuildPrepare`에서 서버 deadline을
맞았다. ACK 구간 123.633–126.587초는 실패가 반환된 시점이다. 완주에 4–7초가
더 필요하다는 계산에는 쓸 수 없다. B15 Record는 세 번째 Posting 물리 배치까지
도달했고 B16 Record는 첫 Posting 페이지의 radix 구축에서 종료됐다. B15 Record와
B16 Record의 호스트 전체 swap in/out은 0.26GiB와 55.63GiB였다. 서로 다른
진행 구간과 호스트 조건으로 전체 임계 경로와 필요한 절감량을 정하지 못했다.

B15의 동일 첫 두 페이지 COMMIT 71→59회는 직접 관측했다. B16의 8배치 추가
효과는 Large에서 미관측이며 B17은 메모리와 gate 검토에 따라 4배치를 복원했다.
B17의 18개 집중 회귀는 해당 코드 경계를 검증한다. 최신 소스의 최적화 pair와
Large ACK·검색·재오픈은 별도다.

종료 기준은 고정된 최신 소스·입력·work caps로 원 Large 서버120초의 full ACK를
받고 검색 및 재오픈 후 검색까지 확인하는 것이다. 경합을 통제한 Off가 실패하면
같은 소스의 Record와 전체 임계 경로를 볼 수 있는 완주 진단을 별도로 묶는다.
진단에서 확인된 반복 비용과 필요한 절감량으로 다음 코드 범위를 정한다. 진단 결과는
원120초 수용에 포함하지 않는다. 공유 호스트의 load·swap으로 실행 비교가 오염되면
해당 결과는 로컬 진단으로 남기고 수용 근거를 분리한다.

### 동일 작업량에서 첫 Posting 진입이 41초 밀린 구간

seed42의 B15 `lr20`과 B16 `lr21`은 각각 전체 4,096 owner의 inventory를 완료했다.
서버 요청 시작에서 첫 Posting 진입까지 70.668→112.041초로 41.373초 늘었다.
`NativeSourceApply`는 11.228→12.447초였다. Lexical 진입 후 Posting 전까지는
58.304→98.881초로 40.577초 증가했다. 같은 구간 thread CPU는 Lexical의
전체 CPU에서 Posting 자식 CPU를 빼면 약 33.43→37.80초다. 따라서 약 36초는
해당 스레드 CPU 증가로 설명되지 않는다.

| 겹치지 않는 Posting 이전 구간 | 두 실행의 횟수 | B15 wall / thread CPU | B16 wall / thread CPU |
| --- | ---: | ---: | ---: |
| occurrence·inventory Block·contribution 물리 설치 | 80배치, write·file sync 각각 12,288회 | 16.924 / 11.493초 | 29.040 / 12.981초 |
| 같은 배치의 Catalog COMMIT | 160회 | 14.332 / 3.146초 | 25.324 / 4.383초 |
| Inventory diff 내부 posting edit | 17,899회 | 5.221 / 3.826초 | 11.103 / 4.600초 |

세 행은 trace 시간 구간이 겹치지 않으며 합계 28.99초가 늘었다. VFS lane22는
상위 COMMIT 또는 edit에 포함되므로 위 수치에 더하지 않는다. B16의 유일한 제품
코드 변경은 reachable Map 설치 창 4→8이었다. 위 80배치는 occurrence 32×128,
Block 32×128, contribution 16×256의 직접 `put_batch` 경로로 처리돼 이 창의
대상이 아니었다. 처음 Posting을 시작할 때 B16에 남은 예산은 약 8초였다.
B15 첫 페이지 MapApply만 14.564초였고 그중 13.159초가 물리 설치 lane이었다.
두 호스트의 swap in/out 0.26GiB와 55.63GiB는 강한 교란 증거이며, 이 trace로
파일시스템 대기와 스케줄링의 개별 기여를 분리할 수 없다.

확인된 코드 후보는 `native/text/contribution.rs`의 인라인 경계다. 현재 Small만
인라인으로 바뀌고 Block·Tree는 `ContributionWrites::flush`에서 별도 파일로
설치한다. 같은 입력에서 contribution 파일만 4,096개이며 물리 설치와 reserve·confirm
전체가 9.920→18.840초였다. 기존 `MapValue::Inline`에 Block/Tree 구분과 token count를
담고 source record와 inventory root를 실제 child 두 개로 유지할 수 있다. 구현 시
기존 object/Small decoder와 새 tag, GC·pin·재오픈 계약을 함께 검증한다. Owners map의
wire와 객체 수가 달라질 수 있어 위 wall 전체를 순 절감으로 간주하지 않는다.

## B18 — Block·Tree owner contribution 인라인

이전 `ContributionWrites::flush`는 `Small` 외의 Block·Tree contribution을 별도
Object로 설치했다. 새 codec은 기존 MapValue wire 안에서 `0x80` Block과 `0x81` Tree,
8바이트 BE token count, 정확한 두 child `[source record, inventory root]`를 사용한다.
Map root의 직접 참조가 소유권을 유지한다. reader는 기존 Object·Small과 새 tag를
구분하고 잘못된 폭·child 수·알 수 없는 tag를 거절한다. output policy에는 새 layout
stamp를 넣었다. 이전 정책의 member를 새 프로파일로 선택하려면 새 artifact가 필요하다.

같은 B15/B16 prefix에서 이 변경이 직접 겨냥한 범위는 4,096개 wrapper와 32회
Catalog COMMIT이다. occurrence와 inventory Block은 각각 4,096개, 32×128개 배치로
남는다. 두 경로는 occurrence가 durable/confirmed된 뒤 Block을 만드는 계약을 사용한다.
현재 cold inventory 창128과 posting edit의 64행 SQL 배치를 재구현하지 않는다.
이후 Owners map의 설치량도 함께 보아 실제 순절감을 판정한다.

### B18 실행 결과

`ccd5ab92…`의 f68 집중 회귀 220개와 r27 최적화 build가 통과했다. 동일 pair의
seed42·4,096 sources·server 120초 Off `lo23` 및 Record `lr22`는 `BuildPrepare`
deadline으로 full ACK가 끝나지 않았다. `lo23`은 완료 그룹 scratch의 checkpoint,
`lr22`는 첫 Posting page 물리 설치의 namespace checkpoint에 걸렸다. 둘 다
numeric·memory·external-resource 거절은 없다. ACK 대기는 127.637/127.996초다.
공유 호스트 load1은 각각 34.24–65.27/36.46–72.42, swap in+out은 72.21/122.40GiB다.
해당 값은 호스트 구간 전체이며 한 요청의 swap량이나 tracing overhead로 환산할 수 없다.

`lr22` child trace는 889,561 events, drop/open 0으로 정상 종료했다. 같은 seed의
첫 Posting 전 완성 구간을 비교했다. 아래 선택 span의 시간 중복은 0이다.

| 구간 | B15 `lr20` | B16 `lr21` | B18 `lr22` |
| --- | ---: | ---: | ---: |
| Lexical 진입부터 첫 Posting까지 wall | 58.304초 | 98.881초 | 72.902초 |
| 실제 write / file sync | 12,288 / 12,288 | 12,288 / 12,288 | 8,192 / 8,192 |
| contribution 파일 설치 배치 | 16 × 256 | 16 × 256 | 0 |
| 전체 물리 배치 / wall | 80 / 16.924초 | 80 / 29.040초 | 64 / 16.742초 |
| Catalog COMMIT / wall | 160 / 14.332초 | 160 / 25.324초 | 128 / 22.791초 |
| scratch posting edit / wall | 17,899 / 5.221초 | 17,899 / 11.103초 | 17,899 / 6.486초 |

동일 prefix에서 write·fsync 각각 4,096회와 COMMIT 32회 감소가 확인됐다. wall의
25.980초 차이는 호스트가 달라 순수 코드 효과로 판정할 수 없다. B18에서는 여전히
occurrence와 inventory Block이 각각 4,096개를 쓴다. 첫 Posting page는 20×256개
설치 그룹을 시작했고 Catalog COMMIT span 24회·5.818초 뒤 중단됐다. 최상위
terms/owners Map 단계와 전체 ACK·검색·재오픈에는 도달하지 않았다.

동일 r27의 `mb18` Medium 256 sources는 full ACK 25.024초, 최초 검색 468.5ms,
증분·삭제·no-op ACK, pin 검색, 재오픈·검색·삭제 유지·drain을 완료했다. producer
exit0과 canonical artifact/profile을 검증했다. 전용 호스트 기준의 정식 성능 비교는
`NOT_RUN`이다.

### B19가 제거하는 inventory 파일

Large generator의 ASCII 파일에는 본문 120행과 고정 anchor 3행이 있다. 파일당
본문 고유 term은 최대 120개, 공통 term은 최대 23개다. case prefix를 두 가지로
써도 최대 286 keys다. 가장 긴 본문 고유 term은 14자, 공통 term은 27자다.
두 case의 position vector가 동일하므로 `QITXOC01`의 보수적 상한은
`16 + 2×120×19 + 2×23×32 + 2×143 + 4×367 = 7,802B`다.
이는 512 keys와 8,192B 한도 안이다. 파일별 고유 anchor가 객체마다 남는다.
고정 anchor만으로도 Small 한도 8 keys를 넘긴다. 따라서 이 입력의 `QITXEM02`
파일은 이미 저장된 단일 `QITXOC01`의 key를 되풀이하는 경우다.

일반 입력에 이 상한을 적용하지 않는다. B19 생산자가 실제 draft와 단일 occurrence의
key·ref·count를 검증한 뒤에만 파일 생략을 결정한다. 후보 인증 읽기와 key 비교는
새 비용이며, 파일·COMMIT 횟수와 실행 wall을 분리해 판정한다.

### B19 실행 결과

`046d2c02…`의 f69는 252개 통과, r28은 최적화 pair build를 통과했다. 같은
pair의 원 Large Off `lo24`와 Record `lr23`은 첫 ACK 전에 `BuildPrepare`
deadline으로 종료됐다. ACK 대기는 125.563/124.227초였다. numeric·memory·external
거절은 없다. Record child는 1,268,416 events, drop/open 0으로 종료했다.
호스트 load1은 Off 24.90–35.20, Record 22.91–30.37이고 swap in+out은
54.80/78.64GiB였다. B18과 부하가 달라 아래 wall 차이를 순수 코드 효과로
사용할 수 없다.

| 같은 Large 첫 Posting 전 | B18 `lr22` | B19 `lr23` |
| --- | ---: | ---: |
| Lexical 진입부터 첫 Posting wall | 72.902초 | 32.598초 |
| 실제 write / file sync | 8,192 / 8,192 | 4,096 / 4,096 |
| QITXEM02 물리 설치 배치 | 32 × 128 | 0 |
| 전체 물리 설치 배치 / wall | 64 / 16.742초 | 32 / 4.556초 |
| Catalog COMMIT / wall | 128 / 22.791초 | 64 / 3.945초 |
| scratch posting edit / wall | 17,899 / 6.486초 | 17,899 / 4.262초 |

남은 `NativeTextInventoryMapApply`의 32회·3.062초에는 후보 인증·파싱·CAS 검사가
포함된다. 실제 managed install은 0회다. B18/B19 합계로 같은 prefix의 파일 write와
file fsync가 각각 8,192회, COMMIT이 96회 줄었다.

`lr23`은 Posting 페이지 980개를 모두 처리했다. 그중 실제 물리 설치가 있는
7개 페이지의 MapApply는 62.886초다. 이미 동일 편집 root 265개를 재사용하고
143개를 만들었다. 96개 물리 배치의 입력 23,683개에서 실제 write·fsync는 각각
17,147회였고 설치 wall은 27.854초였다. 별도 COMMIT 122회는 19.969초였다.
둘은 이 MapApply 구간의 약 76%다. 현재 singleton과 Tree Posting 메타데이터는
이미 인라인이므로 이 비용을 wrapper 설치로 분류하지 않는다.

최상위 terms/owners MapApply는 6.077초 뒤 중단됐다. lane1 cursor·검증·scratch
쓰기 0.388초에는 lane2 edit 쓰기 0.255초가 포함된다. lane3 replay·radix build
0.697초, lane4 scratch COMMIT 0.103초는 성공했다. 실패한 lane5 물리 설치는
4.713초다. 이 prefix에서 6개 물리 배치가 시작됐고 실제 write·fsync 각각
1,403회, Catalog COMMIT 7회가 끝났다. 최초 거절은 설치 중 namespace checkpoint다.
따라서 마지막 중단 원인은 scratch가 아닌 파일 설치 경계의 deadline이다.
완료 전 중단된 구간으로 전체 설치 시간이나 여유분을 추산하지 않는다.

같은 pair의 `mb19` Medium 256 sources는 full ACK 4.975초, 최초 검색 119.5ms,
증분·삭제·no-op, pin 검색, 재오픈·검색·삭제 유지·drain을 완료했다. producer
exit0, 제품 artifact, canonical profile 및 first/reopen trace를 검증했다. 비교
환경과 managed-fresh build binding이 없는 로컬 캡처의 정식 성능 수용은 `NOT_RUN`이다.

## B20 — Dense Posting page 묶음과 최상위 Map 설치

B19 Record의 980개 Posting 페이지 중 첫 네 페이지가 lane37 61.372/63.164초를
썼다. 이 구간에 물리 설치 90/96회와 Catalog COMMIT 113/122회가 집중됐다.
B20의 `def73607…`은 dense Posting의 정렬 owner와 occurrence witness를
256행 이하의 childless 페이지에 넣고 실제 page child를 디렉터리가 소유한다.
원본 입력의 CAS·전체 stream digest·EOF를 확인한 뒤 leaf와 디렉터리를 설치한다.

| 동일 seed42·4,096 sources Record | B19 `lr23` | B20 `lr24` |
| --- | ---: | ---: |
| lane37 전체 Posting page wall | 63.164초 | 9.784초 |
| lane37 설치 배치 | 96 | 27 |
| lane37 실제 write / file sync | 17,147 / 17,147 | 1,969 / 1,969 |
| lane37 Catalog COMMIT | 122 | 54 |
| lane37 파일 설치 / COMMIT wall | 27.854 / 19.969초 | 4.744 / 2.251초 |

B20 Record의 최상위 terms/owners Map은 완주했다. MapApply는 wall 35.683초와
thread CPU 23.079초, 그중 lane5 설치는 wall 34.691초와 CPU 22.235초였다.
Managed 물리 설치 span의 wall은 20.010초, Catalog COMMIT 합계는 10.855초다.
실제 파일 write·fsync는 각각 12,668회였다. COMMIT 63회는 설치 50배치를
4배치씩 묶는 현재 계약의 `50 + ceil(50 / 4)`와 일치한다.
설치 하위 phase 중 writer ticket 준비는 4.339초, open은 3.465초,
게시·퇴역은 4.409초, settlement는 2.790초였다. 실제 write는 0.746초,
file sync는 0.821초, barrier는 1.167초였다. 부모와 자식 span은 합산하지 않는다.
cursor·scratch·radix build와 scratch COMMIT은 중첩을 제외하면 0.964초다.
현재 Storage radix는 짧은 terminal을 이미 부모에 인라인한다. 이 입력의 key는
인라인 한도인 128바이트보다 짧으므로 terminal 길이 한도 상향의 근거는 없다.

`lo25` Off는 full ACK 대기 124.935초에 `BuildPrepare` 설치 중 deadline으로
실패했다. `lr24` Record의 제품 artifact는 최초 ACK 104.337초·첫 검색 180.7ms,
증분·삭제·no-op·이전 pin·재오픈·drain을 완료했다. Record trace의 first footer는
876,074 events, reopen은 2,563 events로 모두 drop/open 0이며 canonical profile도
통과했다. Off/Record의 host load1은 각각 27.22–43.05/32.82–50.75, swap in+out은
27.31/16.57GiB였다. 두 실행은 조건이 달라 tracing overhead나 host 기여율은
확정할 수 없다. 제품 artifact는 검증됐으며 정식 matched-host 비교는 `NOT_RUN`이다.

후속 리뷰는 불가능한 singleton fence·중복 child를 거절하고, warm 편집을
dirty leaf 범위로 제한했다. B20 첫 고정본의 warm 전체 순회와 보완 소스의
국소 편집 성능은 별개다.

보완 소스 `13214505…`의 `lo26` Off는 원 4,096 sources·seed42·server 120초에서
최초 ACK 87.815초, 최초 검색 187.5ms, 재오픈 검색 112.5ms였다. 증분·삭제·no-op,
이전 pin 및 drain을 완료했고 producer는 131.170초에 exit0, 제품 artifact가 검증됐다.
ACK는 서버 기준의 해당 요청이며 producer wall은 그 앞뒤를 포함한다. 관측 load1은
22.36–36.18, 시스템 swap in+out은 26.36GiB였다. `lo25`와 소스 및 host 상태가
달라 두 실행의 차이를 warm 보완이나 host에 각각 귀속할 수 없다.

같은 r30 pair의 `lr25` Record는 full ACK126.355초에 거절됐다. 첫 denial은
`BuildPrepare`의 Catalog `managed_install.rs:844` protection 확인 전 deadline이다.
읽기·쓰기·바이트·mutation과 메모리 거절은 없었다. raw first footer는 860,849
events·drop/open0으로 닫혔다. 제품 증거는 실패이며 검색·재오픈은 실행되지 않았다.

| 최상위 Map, 첫 full 요청 | B20 `lr24` Record 완료 | B20 보완 `lr25` Record prefix |
| --- | ---: | ---: |
| MapApply wall / thread CPU | 35.683 / 23.079초 | 32.695 / 16.521초 |
| lane5 reachable install wall / CPU | 34.691 / 22.235초 | 31.136 / 15.536초 |
| 물리 배치 / write / file sync | 50 / 12,668 / 12,668 | 38 / 9,728 / 9,728 |
| 성공한 Catalog COMMIT / wall / CPU | 63 / 10.855 / 4.705초 | 47 / 11.501 / 3.321초 |

`lr25`는 38번째 물리 배치의 인접 advance 중 protection 확인에서 종료됐다.
관측 COMMIT 47개 자체는 성공했다. lexical wall은 86.443→103.094초로 늘고
thread CPU는 56.503→50.645초로 줄었다. `lr25`의 시스템 swap in+out은
40.32GiB였다. 동일 pair Off와 비교해도 host 조건이 달라 tracing CPU와 실행 대기를
분리해 귀속할 수 없다. 9,728개 파일을 설치한 중단 위치와 별개로, 해당 구간의
반복 설치 비용은 남아 있다. 정식 성능 수용은 `NOT_RUN`이다.

`lr25`의 860,849 이벤트 중 SQLite I/O `lane22`가 28,521 span·399,294 이벤트를
생성했다. runtime maintenance의 zero-write 10,406 span은 약 145,684 이벤트다.
실제 write·sync·거절 관측을 유지하면서 빈 scope의 패킷을 없앨 수 있는지 확인한다.
완주한 `lr24`의 전체 이벤트 수가 876,074로 더 많았으므로 이벤트 증가만으로
`lr25`의 deadline을 설명할 수 없다. 캡처 후 raw 검증 최적화는 ACK 이후 비용이다.

## B21 — 최상위 Map 물리 노드 귀속과 진단 검증

`lr24`에서는 terms와 owners가 같은 lane5 설치와 cross-root batch를 공유한다.
같은 물리 설치 비용을 두 root에 중복 귀속할 수 있으므로 root별 분포는 아직
미측정이다. B21의 `MapApply` 6·7은 `NativeTextPostings` 39 아래에서 순서대로
terms·owners의 staged 노드 `NewObjects`를 기록한다. 두 root에서 공유하는 노드는
기존 완료·durable mark에 따라 처음 도달한 root에 한 번 귀속한다. 실패 span의
값은 처리된 prefix이며, 성공 span도 실제 write 횟수·바이트·시간과 별도다.
제품 보고서는 해당 ancestry에서 두 lane이 모두 유효한 경우 요청별 staged 수를
최대 128건까지 집계한다. 이전 trace에는 이 필드가 없다.

고정 fixture는 owner key를 `[5; 16] || BE128(1..=4096)`으로 만든다. 모두
32바이트이며 QIMAP004의 128바이트 terminal 인라인 한도 안에 든다. nibble trie를
재구성하면 분기는 깊이 60·61·62·63에 각각 1·1·16·256개다. 첫 cold Full의
owner 물리 쓰기는 최대 274개이고, 실측 12,668개에서 terms는 **최소 12,394개**다.
이는 소스에서 계산한 하한이다. root별 파일 실측은 별도로 기록한다.

다음 구조 변경 후보는 terms에 적용하는 bounded immutable leaf page다.
기존 `QIMAP004` 읽기, 정확한 child edge,
순서 보존, 변경된 page의 copy-on-write, 이전 pin, 회수 및 장애 복구가 필요하다.
가변 fanout radix와 정렬 durable leaf의 설계 선례는 [ART](https://db.in.tum.de/~leis/papers/ART.pdf),
[HOT](https://15721.courses.cs.cmu.edu/spring2019/papers/08-oltpindexes2/p521-binna.pdf),
[SQLite file format](https://www.sqlite.org/fileformat.html)이다.

seed42 생성기를 재구성한 8,609,531바이트의 corpus digest는 `lr24`와 일치했다.
ASCII tokenizer에서 raw term 31,340개, 두 case surface의 key 62,680개가 나왔다.
각 posting 값을 owner와 초기 occurrence witness의 동일성으로 표현한 정적 radix
모델은 고유 terms 노드 12,394개를 냈다. owners 274개를 더하면 기존 trace의
물리 설치 12,668개와 일치한다. 두 surface의 같은 subtree는 이미 dedupe된다.
부모에 terminal-only child subtree를 한 단계 인라인하고 child 한도 85를 적용한
모델은 terms 5,272개, 합계 5,546개다. 기존 배치당 256개와 창 4개가 먼저 차는
경우 설치/COMMIT은 50/63에서 22/28이 된다. 새 형식의 실측이나 ACK 예측은 아니다.

B22 구현은 원래 node bitmap과 terminal bitmap 뒤에 packed bitmap을 둔다.
각 packed slot의 본문은 하나의 terminal-only QIMAP004 node다. 부모가 값의 실제
edge를 순서대로 소유한다. byte·edge 한도를 넘기는 slot은 독립 객체로 유지하고
부모를 준비하기 전에 stage한다. 기존 물리 identity는 그대로 읽으며, 새 형식을
기록한 namespace는 QIMAP004 전용 reader로 열 수 없다. 제품 롤백 경계를 검증한다.

### B25 부팅과 저장소 형식의 불일치

r32의 QIMAP005와 r31의 QIMAP004 전용 reader는 모두 v18 marker와 Catalog
stamp를 사용한다. 구 runtime의 boot는 물리 객체 closure와 요청·profile을
검증하지만 Text Map 도메인을 decode하지 않는다. 구 runtime이 새 Map을
포함한 v18 namespace에서 초기 검사를 통과하고 조회에서 `CorruptObject`를
반환할 수 있는 경로를 소스에서 확인했다.

B25는 기존 runtime marker와 Catalog stamp를 v19로 맞춘다. 이전 root와 binary는
pair로 보존하고, 새 root는 명시적 producer rebuild로 만든다. 객체 decoder의
QIMAP004 지원과 이전 Native state root의 admission은 별개다. 실제 교차 바이너리
검증에서 r31/v18과 r34/v19는 각자의 root에 소켓을 열고 정상 종료했다. 서로의
root에서는 exit70, 소켓 미생성, 기존 상태 파일 6개 해시 보존을 확인했다. 구 바이너리는
새 root를 foreign entry로, 새 바이너리는 구 root를 explicit rebuild로 거절했다.

r34 `645f4edf…`는 공유 코드와 일치하는 v19 source다. Off `lo30`와 Record `lr29`의
최초 ACK43.227초·44.613초, 첫 검색264.365ms·97.961ms, 재오픈 검색52.279ms·67.804ms를
확인했다. 두 producer는 exit0, 증분·삭제·no-op·이전 pin·재오픈·drain을 완료했다.
Record의 first/reopen footer는 674,977/717 events, drop/open 0이며 canonical 검증을
통과했다. 첫 Full의 terms 5,272개·owners 274개, write/file sync 각각 5,546회,
물리 배치 22회·COMMIT 28회가 r32와 같았다. Top Map wall/CPU는 9.074/6.459초였다.
Off load1 30.32–40.32·swap 이동8.294GiB와 Record load1 22.24–28.10·swap 이동
0.164GiB는 서로 다른 호스트 부하다. 이 두 실행으로 tracing overhead를 판정하지 않는다.

### r34 XL Off의 첫 거절

`lo31xl`은 동일 r34 소스와 바이너리로 64×512 source·seed42·113,019,656바이트를
실행했다. Large 명시 work cap, server dispatch120초, outer watchdog900초를 실행
전에 고정했다. 소스 staging은 184.633초에 완료했다. 원 operation seq2의
`SourceAdvance`는 `BuildPrepare`에서 deadline을 처음 거절했고, harness의
`full_event_ack`는 mutation staging을 포함해 154.718초 뒤 실패했다. producer
exit1·358.449초, outer timeout은 없었다. ACK·검색·재오픈은 실행되지 않았다.

거절 시 usage는 reads28,854,305, writes19,430,441, bytes26,313,528,668,
mutations6,144,405다. 모두 각 cap 아래였다. `object.rs:821`은 설치 직전 child
검증 checkpoint이며, 해당 줄만으로 누적 병목을 귀속할 수 없다. Off 실행 중
호스트 load1은 17.61–28.47이었다. 동일 Record에서 lexical prepare·posting·
물리 설치·Catalog COMMIT의 wall/CPU와 작업량을 분리한다.

XL은 별도 기능 gate다. Large의 제품 120초를 유지한다. 현재 구현에서 단일
`SourceAdvance`가 source apply, `CoreRequired` Text 구축, seal, Catalog commit을
하나의 요청 deadline에서 수행한다. Work cap 증가나 outer watchdog 연장으로
이 첫 거절은 해결되지 않는다.

### r34 XL Record 단계 귀속

동일 XL Record `lr30xl`도 `BuildPrepare`의 deadline으로 거절됐다. 소스 staging
194.181초와 `full_event_ack`159.818초를 기록했다. daemon child trace는
2,226,147 events로 정상 닫혔고 drop·unwritten·open0이다. 검색·재오픈 product
artifact는 없고 capture의 정식 상태는 `FAILED`다.

원 seq2 server 요청 120.099초의 직접 단계는 Catalog admission lock 7.365초,
SourceApply 46.947초와 부분 LexicalBuild 65.605초다. SourceApply는 OwnerPrepare
25.943초와 SourceMapApply 20.504초를 포함한다. LexicalBuild에서는 inventory
prepare 37.713초와 inventory diff 15.777초가 관측됐다. 관리 객체 설치 합계
16.211초는 inventory의 하위 비용이다. Posting, 최상위 Posting Map, seal,
최종 Catalog COMMIT에는 도달하지 않았다. 사용량 reads29,311,082,
writes19,655,022, bytes26.69GB, mutations6,204,511는 cap 아래였다.

같은 r34 Large Record의 SourceApply는 5.150초였고, LexicalBuild 35.960초는
Posting까지 완료한 수치다. XL에서는 source 준비·Map 물리 쓰기와 Text inventory
단계의 비용을 각각 줄여야 한다. 부분 LexicalBuild만으로 65.605초이므로 단일
작은 설치 변경으로 XL 120초 완료를 추정할 수 없다. 긴 구축을 여러 source
event로 나누면 중간 view가 공개된다. 단일 ACK의 의미를 유지하는 장기 설계는
내구 있는 준비 단위와 원자적 공개 경계를 분리해야 한다. 이 방향은 독립적인
불변 segment를 구성한 뒤 commit에서 공개하는 구현과 부합한다.
[Lucene IndexWriter](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/IndexWriter.html),
[Tantivy architecture](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md).

같은 XL Record의 Source OwnerPrepare 25.943초에서 `RecordGet` 32,768회는
11.913초, `BodyGetValidate` 32,768회는 11.904초였다. Base owner lookup은
32,768회·0.062초였다. SourceApply와 Lexical은 4MiB attempt read cache를
공유한다. 입력은 약 113MB이며, 이 기록에 cache hit/miss는 없다. B27 path Map
packing은 별도 단계다. Source record와 body의 재읽기를 줄이려면 같은 attempt의
인증된 ref를 재사용하는 범위와 UTF-8·길이·child 확인을 유지해야 한다.

### B27 Source 경로 Map 실측

XL Record SourceMapApply는 20.504초였다. 해당 하위 범위에서 파일 write·fsync가
각각 8,792회, 설치 36배치, 중첩 COMMIT 46회였다. 독립 radix 모델의 path 6,605와
owners 2,185, 고정 객체 2개가 파일 수와 일치한다. B27은 v19의 기존 QIMAP005를
path root에만 선택한다. path 값은 인라인이고 물리 child가 없다.

같은 Large42의 r34→r36 Record에서 SourceMapApply 하위 실제 write·fsync는
각각 1,095→727회, 설치 배치는 6→4회, 중첩 COMMIT은 9→6회였다. 모델의 path
819→451개와 일치한다. 관측 Map wall은 1.591→1.058초였다. 호스트가 다른 두
실행의 wall 차이를 코드 절감량으로 환산하지 않는다. r36 최초 ACK는 42.650초였고
검색·증분·삭제·이전 pin·재오픈을 완료했다. XL의 2,944개 감소는 모델이며
동일 XL의 새 결과는 `NOT_RUN`이다.

### XL staging의 반복 Catalog 트랜잭션

r34 XL은 32,768개를 32개씩 1,024회 동기 적재했다. Record staging은
194.181초, Off는 184.633초였다. Record 요청 합계 189.413초, p50 175.582ms,
p95 316.853ms다. 각 요청은 동일한 ingress에서 물리 설치 1회와 Catalog
reserve·confirm 각각 1회를 수행했다. Catalog lock 범위는 2,048회·126.088초,
lane 3 COMMIT은 2,048회·106.556초, SQLite VFS `xWrite`는 853,044회·요청
7.392GB였다. 하위 구간이 겹치므로 이 시간들은 합산하지 않는다.

`StageSources`는 이미 본문과 레코드를 같은 durable batch에 쓰며 동일한 본문을
합친다. B29는 256개 물리 객체 상한에서 128개 입력을 허용해 반복 트랜잭션을
줄인다. 객체마다 필요한 내구 설치와 8MiB 한도는 유지한다. 이 구간의 개선은
전체 소요 시간에 반영되며, ACK 안의 `BuildPrepare` deadline은 따로 남는다.

### r39 XL Record의 구조적 감소와 첫 거절

`lr31xl`은 동일 seed42·32,768개·113,019,656B 입력에서 StageSources
1,024→256회, daemon 전체 요청 1,289→521회를 기록했다. Staging은
194.181→120.360초였다. r34와 r39의 load1 범위가 15.86–28.54와 8.13–11.70으로
달라 wall 차이의 코드 기여율은 추정하지 않는다.

원 seq2는 `BuildPrepare`의 occurrence batch 설치 결과 ref 확인 중 원 120초
deadline으로 거절됐다. SourceApply는 53.226초·CPU27.031초, 부분 BuildPrepare는
62.905초·CPU44.587초였다. SourceMapApply는 r34→r39에서 20.504→14.467초,
Catalog lock hold 46→31회, SQLite xWrite 77,452→54,301회였다. 같은 비교에서
OwnerPrepare는 25.943→38.281초, CPU16.009→17.408초였다. wall 증가량을 단일
호스트 요인으로 확정할 수 없다. 부분 inventory prepare 35.472초와 diff 15.671초는
BuildPrepare 내부 범위다. Posting 완료·최종 publish·검색·재오픈은 미도달이다.
Trace footer는 2,072,143 events, drop·unwritten·open0이었다.

`SourceAdvance` 서버 span은 120.134초였다. harness의 Staging 120.360초,
event-ACK 149.740초는 서로 다른 측정 범위다. `OwnerPrepare`에서 32,768개
`RecordGet`은 합계 17.619초·CPU7.725초, `BodyGetValidate`는 합계
17.898초·CPU7.924초였다. 두 하위 span이 OwnerPrepare wall의 92.8%를
차지한다. 이 수치는 B31이 줄이는 Text의 후속 record 재읽기와 구분한다.
SourceAdvance는 별도 StageSources 요청이 설치한 ref 외에도 승인된 기존 ref를
받는다. 현재 stage 간 내구 인증서는 없다. 최초 읽기는 실제 object ref, 경로와
inode, payload digest, record·body 구조와 UTF-8을 검증한다. 이를 보존하면서
경로 검증을 배치화하려면 namespace 교체와 파일 race를 포함한 custody 설계가
필요하다. 현재 trace에는 경로 검사와 데이터 읽기를 분리할 정보가 없다.

부분 `InventoryPrepare` 8,899개 span에서 lane34 tokenizer·scratch 입력은
4.913초·CPU4.762초, lane35 정렬 그룹 조회·packed 준비는 18.269초·CPU10.513초였다.
두 구간은 각각 8,865개 span이다. lane35는 객체 준비와 설치를 포함하므로
18.269초 전체를 SQLite 비용으로 귀속할 수 없다. `InventoryDiff`의 15.671초에는
`PostingEdit` 9.102초가 포함된다. 하위 합계는 상위 시간에 중복 합산하지 않는다.

### r36 Mixed 워밍업 실패

`lm01`은 `ae2aaf64…`에서 Large42 초기 ACK 41.130초, 첫 검색 151.841ms,
증분 1.039초, 삭제 3.423초를 완료했다. Mixed warmup의 dispatched/completed
434개 중 413개가 실패했다. 1Hz 발행은 seq6·7 모두 ACK 8.218초·6.400초를
받았고 첫 발행의 최신 검색 확인은 8.656초였다. 다음 발행은 ACK 이후 head 조회
또는 검색 확인에서 `ResourceLimited`를 반환했고 snapshot 정리도 같은 오류를
반환했다. Missed tick은 7개다. 600초 retained read와 900초 measure는 미실행이다.

모든 offered query가 현재 snapshot의 단일 `ReadSession`을 공유한다. 서버가
`with_ticket`에서 entry mutex를 검색 전체에 걸쳐 보유하므로 한 세션 내 검색은
직렬이다. `ReadView`와 retained search 실행은 동일 Interactive ledger를 이용하며
Control과 Interactive의 즉시 승인 거절도 `ResourceLimited`가 된다. 호스트
load1은 8.26–13.49였다. 기존 artifact는 첫 검색 오류와 서버 거절 위치를 잃어
세션 대기, Catalog 대기, scheduler 거절과 호스트 영향을 분해할 수 없다.
후속 계측은 원 요청의 단계와 예산 거절 축을 bounded sample에 남긴다.

`ReadTicket::validate`가 사용하는 `SqliteCatalog::validate_ticket`은 blocking
writer mutex에서 `BEGIN IMMEDIATE`를 연다. session inode·pin·fence·만료를
검증하며 clock이 전진한 성공 검증마다 durable `native_work_clock_v1`을 갱신하고
COMMIT한다. 기존 Kernel 호출 수 회귀에서 retained 준비·실행 검증은 5번이며,
registry 전후 검증을 포함하면 7번이다. 50 QPS에서 이는 초당 최대 350번의
검증 경로 진입을 뜻한다. 각 호출의 물리 write 횟수나 `lm01` 원인 비율을
계산한 수치는 아니다. 동일 millisecond에는 clock 갱신을 생략한다. B30은 원 요청
예산과 trace를 mutex 대기까지 연결한다. writer 트랜잭션과 clock floor를 줄이는
변경은 pin·renew·clock 회귀와 함께 별도 검증이 필요하다.

`HeldRead::tick`은 1초 발행 tick의 예산으로 갱신을 요청한다. 종전 서버 갱신은
같은 session entry lock 안에서 예산 없는 writer lock을 기다렸다. B31은 writer
대기와 COMMIT 전 checkpoint에 원 예산을 전달한다. 해당 경로가 `lm01`의 원인이었는지
확인할 갱신 거절 증거는 없다. 같은 tick에서 held read나 정리가 먼저 실패하면
종전 artifact의 `scheduled` 집계가 해당 tick을 누락했다. 후속은 도달 시점에 집계한다.

### r39 Mixed의 Interactive 승인 거절

`lm02`는 72.281초에 producer exit1로 종료했다. Mixed 이전 Large 최초 ACK는
45.099초였다. 첫 warmup tick의 seq6은 ACK 6.419초 뒤 `current_witness`에서
`ResourceLimited`로 거절됐다. 168건의 검색이 끝났고 156건이 실패했다. 최초
검색 실패는 retained unique 검색 task1이었다. 서버의 첫 5개 retained 검색
표본은 `interactive_admission`에서 로컬 예산 거절 기록 없이 `ResourceLimited`를
반환했다. 나머지 표본 3개는 취소였다. 현재 결과로 CPU·I/O·메모리 중 실제 거절
축을 구분할 수 없다.

고정 profile은 CPU·I/O 용량이 각 5단위이며 maintenance를 포함한 보호 class
floor가 5단위를 사용한다. 같은 단위의 Interactive 예약을 동시에 추가 수용할
여유가 없다. host load1 8.59–10.77은 관측 조건이며, 서비스 ledger의 거절을
호스트 영향으로 환산하지 않는다. B34의 예약 대기로 이 충돌을 처리한 뒤에도
1Hz 발행·50 QPS와 같은 시각대의 latency를 각각 다시 검증해야 한다.

r31 Large Record 단일 변경은 StageSources 0.129초와 event ACK 1.003초가
순차적으로 1.132초였다. 단일 삭제는 별도 staging 없이 ACK 1.930초였고,
서버 `BuildPrepare` 1.538초 중 lexical 1.436초와 그 하위 Posting 1.359초가
기록됐다. harness는 한 tick의 발행을 마친 뒤 다음 tick에 진입한다. 두 단일
샘플은 당시 실행에서 1초 주기를 넘겼으며, Mixed의 sustained 처리량이나
코드와 호스트의 기여율을 수치로 확정하기에는 표본이 부족하다.
현재 warm packed posting은 `Posting::checked_df`와 `resolve_packed_warm`에서
동일 prior directory를 각각 읽고 decode한다. 변경되지 않은 페이지는 이미 COW로
유지된다. 제거 가능한 직접 반복은 변경 posting당 directory 1회이며, 1Hz에서의
절감 시간은 미측정이다.

### r40 Mixed의 발행 지연과 일반 검색 거절 분리

`lm03`은 Large 초기 ACK 36.929초와 증분·삭제 검색을 마친 뒤 Mixed
warmup에서 producer exit1로 종료했다. 첫 tick의 seq6 ACK는 4.321초였다.
그 뒤 최신 head 확인은 통과했고, 일반 Exact `SearchText` witness가
`ResourceLimited`를 반환했다. 첫 offered 실패는 retained unique task4의
UDS read timeout 1,998ms로 ACK보다 앞섰다. 검색 완료 91개 중 76개 실패,
도달 220개 중 queue full 66개였다. 서버의 retained 실패 표본 8개는 모두
`Cancelled`였고 session custody·준비·실행에 분포했다.

기존 서버 표본은 일반 Text를 수집하지 않아 witness 거절의 단계는 확정할 수
없다. B36은 일반 Text용 독립 제한 표본을 추가하고 ReadView의 pin validation을
원 요청 예산에 연결한다. r40 Off 실행 중 load1 6.92–8.90을 관측했다.
ACK 지연과 retained 검색의 Catalog/session/transport 대기 비율은 미확정이다.

### r41 Mixed의 다음 snapshot Control 거절

`lm04`는 첫 tick을 3.162초에 ACK하고 3.426초에 최신 witness를 확인했다.
이전 view도 통과했다. 다음 `Open`에서 서버 표본이 `retained_control`의
`control_admission`과 `first_local_denial=None`을 기록했다. 이 실행의
중단점은 기존 2초 검색 deadline이나 lexical work cap에서 발생하지 않았다.
검색 완료 81개 중 62개 실패, 도달 177개 중 queue full 33개다. 첫 UDS read
timeout 1,997ms는 첫 ACK 전이었다. load1은 4.85–6.14였다. Control 거절을
대기로 바꿔도 이미 관측된 3.162초 ACK와 3.426초 freshness는 별도로 남는다.

B38의 공유 query guard는 같은 세션의 전체 검색 직렬화만 줄인다.
Catalog의 `validate_live_ticket`은 여전히 OS guard/inode·pin fence·만료를
확인하고 writer mutex에서 `BEGIN IMMEDIATE`와 COMMIT을 수행한다. 동일
millisecond에는 clock UPDATE를 생략한다. durable floor를 유지하므로, 남은
병목을 확인할 때 mutex 대기, clock UPDATE 여부, COMMIT 시간을 분리한다.

### r43 XL Off에서 재현된 BuildPrepare deadline

`xo43`은 32,768개 source staging을 93.397초에 마쳤다. 최초 ACK phase는
140.304초에서 실패했다. daemon은 원 120초 request deadline, `BuildPrepare`,
`object.rs:862`의 install loop checkpoint를 기록했다. 첫 denial은 work 수치
상한과 별개이며, 이 checkpoint는 deadline을 감지한 위치를 나타낸다. trace Off에서도
발생했다. 검색·재오픈은 미실행이다. load1은 4.94–7.40 범위였다.

### r42 Mixed의 실제 진행과 만료 tick

`lm05`는 warmup 60 tick에서 14건, measure 57 tick에서 12건을 발행하고
각각 최신·이전 witness를 확인했다. 두 window에서 90 tick이 늦었다. 검색
완료 4,003건 중 3,949건이 실패했고, 도달 6,001건 중 queue full은 1,935건이다.
첫 검색 실패는 1,534ms 큐 대기 뒤 465ms UDS timeout이었다. 첫 ACK는 5.802초,
부분 measure의 freshness p95는 7.004초였다. load1은 5.27–7.17이었다.

최종 `held_read_tick`은 전체 index 116이었다. tick의 1초 budget 만료는
held origin 기준 117.082초인데, 첫 갱신은 같은 origin의 120초 이후에만
진입한다. publisher가 late 확인 전에 `HeldRead::tick`을 실행해 이미 만료된
budget으로 갱신을 시도했다. cleanup의 Release는 120.245초에 시작했고 원 pin
해제가 확인됐다. B39는 만료 tick을 먼저 기록하고 건너뛴다. 이 수리는 관측된
갱신 중단 경계를 고치며, 1Hz·50 QPS 수용 조건은 그대로다.

### r43 Mixed의 갱신 진행과 남은 제품 지연

`lm06`은 warmup 16건과 measure 100건을 발행했다. 총 487 tick 중 370개가
늦었다. 도달한 검색 24,350건 중 7,874건이 queue full이었고, 완료된 16,412건
중 16,287건이 실패했다. 새 mutation 준비 시각으로 measure의 제출부터 ACK까지를
분리했다. 이 구간의 중앙값은 2.742초, freshness p95/p99는 6.634/7.663초였다.
load1은 4.20–18.65였다. 제출 구간 내부의 Catalog, build, 물리 설치 비율은
이 실행에서 측정하지 않았다.

held-read는 약 120.178·240.791·362.791·486.898초에 네 번 갱신 ACK를 받았다.
네 번째 갱신 뒤 witness 검색이 tick의 남은 166ms에서 실패했다. cleanup은 같은
pin을 Release했고 마지막 pin 해제를 확인했다. 600초 보유와 settlement는
미도달이다. 최초 120초 갱신 전에 만료 tick을 호출하던 r42 중단은 재현되지 않았다.

### r44 Mixed의 held 해제와 settlement 거절

`lm07`은 원 50 QPS·1Hz 부하 아래 warmup 17건과 measure 167건을
발행했다. 661 tick 중 477개가 늦었고, 예정 query 33,051건 중
queue full은 10,699건, 완료 22,288건 중 실패는 22,023건이다. 첫 실패는
unique 검색으로 2초 중 1.707초를 scheduler 대기열에서 소모했다.
measure freshness p95/p99는 5.453/5.807초였다. 이 값은 중단된 창의 표본이다.

독립 held worker는 원 1초 tick에서 네 번 갱신했고 갱신 뒤 witness를
통과했다. load start 후 600초를 보유했으며 pin 시작 기준 600.434초에
Release ACK와 마지막 pin 제거를 확인했다. 다음 60초 내 61회 조회 뒤
마지막 응답은 `Pending`이었다. cleanup과 snapshot cleanup은 오류가 없다.

Catalog의 `Pending`은 target pin/floor, terminal work·custody, 미퇴역 view,
GC queue와 object lifecycle의 보수적 검사에서 반환된다. 증거는 어떤
조건이 실제 차단했는지 보존하지 않았다. query snapshot과 held 제품
session은 같은 Kernel의 Catalog guard를 사용한다. 단순히 query 세션이
별도 guard라서 차단됐다고 판정할 수 없다. epoch/global debt 검사가
지속 발행과 충돌할 가능성은 있으나 60초·정리 정책 변경의 근거는 미확정이다.

### r45 Mixed의 Terminal Custody 차단

`lm08`은 Large 4,096-source·seed42에서 704.012초에 중단됐다.
warmup 20건과 measure 160건이 ACK를 받았고 661개 tick 중 479개가
늦었다. 33,051 예정 검색 중 10,695개가 queue full로 탈락했다. 완료
22,292건 중 21,958건 실패, 최초 실패는 원 2초 중 scheduler 1.572초와
UDS read 427ms였다. 짧은 초기 샘플에서 session custody, query preparation,
control admission, query execution의 취소가 보였다. 한 구간에 비용을
귀속할 수 없다.

held는 네 번 갱신한 뒤 600.539초에 Release ACK와 마지막 target pin 제거를
확인했다. 61회 poll 후 60초 수용에 실패했다. 한 Catalog snapshot에서
`Pending(TerminalCustody)`, target 5, head=floor 169, target pin 없음이
관측됐다. 마지막 wire 응답은 `Pending`, head=floor 184, target pin 없음이었다.
한 차단 표본을 전체 61회에 일반화하지 않는다.

현재 차단 predicate는 같은 workspace/epoch의 terminal custody 전체를 본다.
어느 operation이 남았는지는 아직 관측되지 않았다. Large의 첫 full
SourceAdvance는 같은 ingress에 4,096개의 고유 본문과 4,096개의
SourceRecord를 설치한다. op2에 이전되는 이 8,192 custody rows는
mutation·map·adapter 객체가 제외된 하한이다. Work→Views→Custody 순환,
20ms 간격, 한 custody page당 한 행으로는 공용 lock·SQL을 제외해도
최대 16.7행/초, op2 ingress 하한에 약 492초가 걸린다. 이어진 발행과
더 큰 물리 객체 집합은 별도 debt다. B51은 조회 writer 예약만 줄이고 이
처리 상한을 변경하지 않는다.

B54는 Work/View 페이지와 custody root 페이지를 분리해 기존 한 transaction의
bounded release를 재사용한다. 글로벌 debt0 predicate와 60초는 유지한다.
별도 측정에서 가장 오래된 ready sequence, release rows/round, floor/phase
거절을 확인해 초기 적체·새 발행·maintenance admission을 구분한다.

### r47 Mixed의 같은 oldest custody 잔류

`lm09`는 B54–56의 32행 custody 페이지와 같은 1Hz·50QPS를 사용했다.
소스 `647dce43…`, cooperative lease, shared macOS에서 707.722초에 중단했다.
warmup 18·measure 176 ACK, 늦은 tick 464/661, 검색 실패 21,881/22,297,
queue full 10,690건이다. 첫 실패는 원 2초에서 scheduler 1.498초,
UDS read 약 502ms였다. measure freshness p95 5.536초·p99 7.521초다.

600.509초에 마지막 pin을 제거했지만 61회 뒤 `Pending`이었다.
custody 표본 1·16·32·48·60번째에서 oldest ready 20874가 같았다.
head와 exact floor는 184→187→190→195→198로 전진했다. 같은 ready row가
관측 기간에 계속 남아 있었음이 확인됐다. release된 자식 행 수, queue
크기, scope admission·floor 거절은 측정되지 않았다. 초기 대형 operation,
bounded sweep 재방문 빈도, SQLite 행당 비용을 실제 진행률로 구분한다.
배치 폭만 늘린 결과는 기존 60초 oracle을 충족하지 못했다.

### r48 Mixed의 이전 뷰 검색 중단과 정리 진행

`lm10`은 소스 `930e30da…`, cooperative lease에서 272.413초에
중단됐다. 최초 실패는 publisher의 `old_view_witness`, UDS read
1,999ms였다. 이후 snapshot drain의 해제도 같은 IPC 제한에서 실패했다.
warmup 16·measure 37 ACK, 늦은 tick 157개, 완료 검색 7,391건 중
7,291건 실패와 queue full 3,556건이다. 최초 검색 실패는 scheduler
1.565초와 UDS read 약 434ms였다. held는 한 번 갱신했고 cleanup이
마지막 pin을 해제했다. 원 600초 보유 및 settlement poll은 미도달이다.

opt-in progress는 r47의 20874를 초기 full op2로 확인했다. 45초 간격
다섯 표본에서 해당 operation의 커밋된 삭제는
1,152→4,288→6,624→8,416→9,984행이었다. 마지막 표본의 전체 custody
삭제는 30,787행·987페이지이고 oldest에 행이 더 남았다. floor 4,409회 중
17회가 거절됐고 maintenance admission·custody probe·release 거절은
0회였다. 표본까지 op2가 느리게 정리됐으며 page timeout 롤백은 없었다.
중단 전 확인된 병목은 이전 뷰 검색 지연이다. r47의 60초 정산과 별개로
기록한다. B62는 같은 page에서 반복하던 GC binding 검증과 SQL 준비를 줄인다.

### r49 Mixed의 정산 실패와 검색 처리율

`lm11`은 `d4e0a5dc…`의 고정 pair에서 724.781초, producer exit 1로
끝났다. 4회 갱신 뒤 600.550초에 마지막 pin 해제를 확인했다. 61회 poll
뒤에도 `Pending`이었으며 head와 Exact floor는 모두 154였다. 1·16·32·48·60번째
poll의 첫 blocker는 `TerminalCustody`, oldest ready는 모두 20874였다.
마지막 진행 표본은 전체 2,961 page에서 92,251행, 초기 op2에서 26,368행을
커밋했다. custody probe·release 거절은 없었다. 이 결과는 해제 후 60초 안에
실제 backlog를 소진하지 못한 경우다. 마지막 표본 이후 처리량은 집계하지 않는다.

검색 22,305건 중 22,185건 실패, queue full 10,681건, ACK 150건,
missed-late 511건이었다. 최초 검색 실패 task 16은 queue에서 1,539ms를
보낸 뒤 retained search 응답 읽기에서 460ms로 만료됐다. 서버의 첫 여덟
실패는 모두 request ID 1이었고 session custody 2건, query preparation 3건,
execution 2건, Control admission 1건에서 취소됐다. 후속 요청만 받는 B63
실패 타이밍은 0건이었다. 후속 요청의 실패 표본과 매 요청 새 counter를 쓰는
offered query 표본을 각각 제한해 서로 소진하지 않게 해야 한다.

고정 ledger의 5단위에서 각 보호 class가 1단위를 유지하므로 Interactive는
동시에 1개다. 50 QPS를 검색만으로 처리하려면 점유 시간 평균이 20ms 이하여야
한다. retained 검색당 7회 writer transaction도 같은 Catalog를 공유한다.
호스트는 실행 시작부에 load1 27–39였고 다른 실행과 일치하지 않는다.
각 병목의 wall 기여량 및 로컬 개선률은 아직 산출할 수 없다.

### r51 Mixed의 custody 진행과 정산 미수용

`lm12`는 `1482d857…`에서 704.736초, producer exit1로 종료했다.
4회 갱신 뒤 600.475초에 마지막 pin을 해제했다. 61회 poll의 최종 상태는
`Pending`, head와 Exact floor는 모두 176이었다. 초기 op2의 ready20874는
해제 전에 소진됐다. 마지막 진행 표본은 3,721 page·115,926행을 커밋했고
가장 오래된 작업은 op170/ready24723이었다. custody probe·release 거절은 0이다.

해제 뒤 `UnretiredView` 다음 `ReclaimQueue`가 첫 blocker로 관측됐다.
이후 새 publication의 `TerminalCustody`가 다시 나타났으며 ready 표본은
24611에서 24779로 전진했다. 개별 poll의 blocker 표본과 마지막 `Pending`
상태는 서로 다른 증거다. 해제 창 전체에서 어느 blocker가 가장 오래
유지됐는지와 reclaim 큐의 정확한 잔량은 현재 관측으로 알 수 없다.

r51 전체 704.736초에서 maintenance floor round는 11,918회, custody
처리량은 3,721 page·115,926행이었다. 20ms tick 설정에서 실제 평균은
초당 16.9 round, 5.28 custody page였다. 고정 Work→Views→Custody 순환은
Work/View가 비지 않으면 custody에 약 3회 중 1회만 배정한다. 추가
page가 전체 round 시간을 늘릴 수 있으므로 round당 분배율과 초당
실제 처리량을 별도로 판정한다. 정산은 target pin 외에도 해당 epoch의
custody와 전역 reclaim 부채가 남으면 Pending을 유지한다.

검색은 22,311건 중 22,106건 실패, queue full 10,676건이었다. 172건 ACK,
487개 missed-late였으며 960초 측정은 중단됐다. 최초 검색 실패 task12는
queue에서 1,280ms를 쓴 뒤 retained IPC read에서 720ms로 만료됐다.
호스트 load1 5.41–8.45와 r49의 5.73–38.95는 서로 다른 환경이다. 초기
custody 차단이 해소된 사실과 검색·정산 기준 미충족을 각각 기록한다.

### Mixed retained 검색의 Catalog 경계

retained SearchText는 registry pre/post 2회, Kernel profile pre/post 2회,
stage resolution 완료 1회, 실행 pre/post 2회의 Catalog validation을
거친다. 각 검증은 현재 session lock·inode, session/pin row와 durable
clock을 확인하고 commit한다. 같은 connection mutex와 `BEGIN IMMEDIATE`를
사용하므로 성공률 100%인 50 QPS에서 이 경로는 초당 350회 transaction을
요청한다. 새 시각이 floor를 전진시킬 때 clock row를 기록한다. 검증 경계
사이에 admission wait, profile read와 결과 변환이 있다.

B56은 lock 인증에서 이미 읽은 dev/inode를 같은 호출의 SQL row 확인에
사용해 중복 fstat 한 번과 inode blob 할당 두 번을 제거한다. 일곱 검증의
횟수는 유지한다. 정식 귀속은 Control wait·준비·Interactive wait·검색과
Catalog lock/commit의 시간 및 관측 손실을 별도로 확인해야 한다.

r51 `lm12` 초반 fresh-request 성공 표본 8건은 Interactive 대기
121.645–421.229ms, 중앙값 240.096ms였다. 예약 후 실행은
42.906–327.087ms, 중앙값 98.843ms였다. 이 실행 구간은 Kernel 전후
ticket validation과 reader를 포함한다. 가장 빠른 표본도 단일 슬롯의
50 QPS 필요 평균 점유 20ms보다 길었다. 오류 표본 5건은 모두 취소였고,
그중 session custody에서 끝난 경우가 4건이다. 초기 표본이므로 지속부하
분포나 lexical 단독 시간을 나타내지 않는다. 다음 진단은 `query.reader.query`
와 실행 전후 ticket validation을 직접 분리한다.

r52 `lm13`의 첫 fresh-request 성공 표본 8건에서 슬롯 점유 내 실행
중앙값은 68.978ms다. reader 중앙값 12.940ms, 실행 전후 ticket validation
합산 중앙값 47.911ms다. 6건의 reader는 7.635–18.744ms였고, 2건은
528.009ms와 698.285ms였다. 요청별 `before`, `reader`, `after`를
같은 span에서 기록했다. 이 표본으로 느린 두 요청의 query class나
전체 50 QPS 분포는 확정할 수 없다. 호스트 load1 범위는 16.02–41.45,
중앙값 22.12로 r51의 5.41–8.45와 다르다. 실행은 338.958초에 producer
exit 1로 끝났다. 첫 실패는
`held_read`의 갱신 직후 고정 view 검색에서 IPC Read deadline 726ms였다.
갱신 ACK는 120.317초와 241.375초에 도착했고 마지막 pin은 243.687초에
해제됐다. Publisher 취소는 후속 결과다. 원 600초 pin과 해제 후 정산은
`NOT_RUN`이며, r51의 정산 실패는 여전히 열려 있다.

같은 실패 요청의 서버 wall time은 Control admission 16.847ms,
Interactive admission 0.001ms, query preparation 274.534ms, session custody
357.351ms, query execution 65.167ms였다. 실행 중 reader는 3.343ms,
전후 ticket 검증은 18.070ms와 43.739ms였다. 마지막 stage는 `session_custody`,
결과는 `Cancelled`였다. 이 요청의 직접 지연 구간은 preparation과 custody다.
별도로 B76은 fresh 요청의 단일 Interactive 슬롯을 마지막 Kernel
ticket 검증 전에 반환한다. 변환 메모리와 기존 오류 우선순위는 f130에서
확인했다. 호스트 경합의 기여도와 sustained 50 QPS 효과는 현재 표본으로
산정할 수 없다.

B76이 옮기는 경계를 r52의 같은 8건에 대입해
`query_execution - after_ticket_validation + result_conversion`을
계산하면 중앙값은 35.888ms이고 20ms 이하는 2건이다. 이는 각 작업의
시간이 그대로라는 가정에서 슬롯 점유를 재분류한 값이다. B76 실행
시간이나 50 QPS 수용 결과로 사용하지 않는다.

r52 타이머는 outer custody 진입과 반환, query plan과 Kernel 준비를
각각 합산한다. 최초 실패에 대한 mutex·SQLite·호스트 대기 분해는
`NOT_RUN`이다. Record의 기존 `CatalogLockWait`·`CatalogLockHold`로
mutex 대기와 점유를 분리할 수 있다. 점유 내부의 transaction 시작,
row 검증, clock, COMMIT 분해는 별도다. 단계 통합은 durable clock,
pin 만료와 fence 검사를 유지해야 한다.

### r53 Mixed의 preparation 실패와 실제 custody 진전

`lm14`는 `a6003311…`의 동일 환경·최적화 pair에서 producer exit1,
474.416초로 종료했다. 갱신 ACK는 120.388·241.380·362.412초에 도착했다.
세 번째 갱신 뒤 고정 view 검색이 834ms IPC deadline에 도달했다.
같은 conn12841/request6의 서버 wall은 Control admission 32.333ms,
entry/pre custody 136.232ms, query preparation 629.304ms였다. 최초
원인은 held read이며 publisher는 후속 취소다. 이 요청은 Interactive
admission과 reader에 미도달했다.

마지막 pin 해제는 363.641초에 확인됐고 settlement poll은 0이다. 원
600초 pin·60초 정산은 `NOT_RUN`이다. 검색 12,226건 중 12,209건이
실패했고 queue full은 5,861건이다. 부하 도달은 48,000개 중 18,151개이며
publisher는 warmup 10건과 measure 46건을 발행했다.

fresh-request 성공 표본은 4건이다. reader는 6.300–9.049ms, 실행 전
ticket 검증은 9.426–365.714ms, 실행 후 검증은 57.964–176.109ms,
registry entry/pre는 10.135–31.186ms, exit/post는 41.090–165.379ms였다.
새 계측은 registry 시작과 종료를 분리했지만 검증 내부의 mutex 대기,
SQL·durable clock·COMMIT 비율은 아직 구분하지 않는다.

마지막 retention 표본은 7,049 round·1,881 custody page·59,174행이다.
초기 op2를 벗어나 oldest op56/ready22593까지 전진했고 custody probe와
release 거절은 0이었다. 전체 실행 중 host load1은 93개 표본에서
16.79–106.35, 중앙값 37.53이었다. r51·r52와 직접 처리율이나 개선율을
비교할 수 없다. 기본 Large 단계의 ACK 52.838초와 첫 검색 189.865ms는
성공했다. 증분·삭제·old pin 확인도 완료했으나 전체 Mixed 제품 수용은
실패했다.

### r54 Mixed Record의 실행 전 거절

`r54`의 `3c034df4…` 최적화 pair 빌드는 성공했으나 Mixed Record 요청은
capture preflight에서 `Native mixed load requires Off trace mode`로
거절됐다. Rust harness도 동일 조합을 거절한다. producer body는 0이고
실행·진단 상태는 `NOT_RUN`이다. 제어기의 뒤따른 `FileNotFoundError`는
없는 execution receipt를 읽은 오류이며 제품 실패의 원인이 아니다.

B82 후보는 같은 Mixed Off의 기존 선택된 retained-request 진단에
Catalog의 여섯 구간을 연결한다. 요청 처리의 동기 스레드와 같은 Catalog
객체에 scope를 묶고 시간·호출·실패 횟수를 고정 크기로 모은다. 중첩
관측은 복원하고 잘못된 종료 순서는 관측을 무효화한다. 결과는 요청 안의
여러 ticket 검증을 합산한 값이다. 원 타이머의 각 stage와 합산하지 않는다.
이 계측을 포함한 r55는 RCA 실행으로 분류하며 성능 qualification은
`NOT_RUN`으로 둔다.

별도 Native SDK read-cap 조사에서 현재 Catalog admission은 새 Attempt
객체의 reserve에 27회, unique confirmation에 20회의 논리 read를
계상한다. 12,524개가 각각 이 두 경로를 거쳤다면 최소 588,628회다.
이는 보고된 InventoryMapApply 754,642회의 78.0%에 해당하는 코드
산술이며 실제 SQL 호출 수나 wall 기여량은 아니다. 현재 경로는
transaction 내 child 인증, sequence counter, registration preflight를
이미 공유한다. 개별 custody·intent·GC·metadata 검증이 남으므로 27/20을
즉시 낮출 근거는 확인되지 않았다. 실제 statement별 계수와 work delta를
대조한 뒤 제거할 SQL과 계상을 함께 정해야 한다.

### r55 Mixed의 Catalog 대기와 반복 COMMIT

`lm15`는 `b7ffae26…`의 source와 binary를 실행 전후 확인했고
producer exit1·619.538초로 종료했다. 갱신 ACK는
120.725·241.690·362.658·483.889초에 도착했다. 네 번째 갱신 뒤
held 검색이 555ms IPC deadline에 도달했고 마지막 pin은 485.515초에
해제됐다. settlement poll은 0이다. 원 600초 pin·60초 정산은
`NOT_RUN`이다. 검색 16,319건 중 16,307건이 실패했고 queue full은
7,818건이었다.

실패한 conn17216/request8의 서버 stage wall 합계는 519.909ms다.

| 구간 | 요청 안 합계 |
| --- | --- |
| session guard | 1.793ms |
| Catalog connection 획득 | 335.255ms |
| BEGIN | 0.169ms |
| session/pin row 확인 | 0.315ms |
| durable clock 확인·갱신 | 0.073ms |
| COMMIT | 137.609ms |
| 실제 reader | 3.747ms |

여섯 Catalog 구간의 합은 475.214ms로 서버 stage의 91.4%다. 이 요청에서
같은 Catalog의 mutex가 실제 사용 중이었던 재시도는 277회였고, retry
sleep wall 합은 335.180ms였다. session guard와 connection은 각각
7회, SQL과 COMMIT은 각각 6회까지 진행했고 마지막 connection 획득이
취소됐다. 실제 mutex 충돌과 Catalog 쓰기 완료 비용이 확인됐다.
COMMIT 내부의 디스크·native VFS·호스트 스케줄 기여율은 아직 미분리다.

최종 stderr에 완전한 형태로 보존된 성공 표본은 3건이며 각각 7회의
검증·COMMIT을 기록했다. COMMIT 합계는 요청당
103.450·106.382·117.494ms, 평균 109.109ms다. 같은 서비스 시간이
유지된다는 가정에서 한 connection의 COMMIT만으로 가능한 처리량은
초당 약 9.17요청이다. 이는 이 표본의 직렬 점유 산술이며 정식 성능
상한이나 quiet-host 예측이 아니다. 50 QPS는 이 비용에서 COMMIT만으로
초당 약 5.46초의 직렬 점유를 요구한다.

실행 중에는 성공 6건을 관측했고 COMMIT 합계 범위는 76.529–117.494ms였다.
종료 시 16KiB stderr tail의 byte offset 3224 이전이 제외돼 앞선 성공
3건은 최종 raw tail에 남지 않았다. 위 재현 가능한 통계는 보존된 3건만
사용했다. 이후 진단 보존은 제한된 앞부분과 마지막 오류를 함께 남기는
방식을 검토한다.

host load1은 120개 표본에서 18.48–86.47, 중앙값 33.31이었다. 최초
Large ACK 79.839초·첫 검색 478.761ms, 증분·삭제·old pin은 완료했다.
호스트가 COMMIT wall을 얼마나 늘렸는지는 이 실행만으로 산정하지 않는다.
성능 qualification은 `NOT_RUN`이다.

직접 코드 원인은 `validate_live_ticket`가 pin 확인과 durable clock floor
갱신을 한 writer transaction으로 묶고, Registry와 Kernel이 한 retained
요청에서 이를 7회 호출하는 구조다. Row 조회와 clock UPDATE 자체는 이
실패 요청에서 합계 0.388ms였다. prepared statement 최적화나 query
reader 최적화보다 반복되는 내구성 경계가 우선이다. 단순 FIFO는 대기
공정성을 바꿀 수 있지만 이 COMMIT 점유를 제거하지 못한다.

### Mixed 관측기 자체 비용

r43 `lm06`의 537.094초 native observation epoch에서 fixture 전체
allocated-block census 1,048회에 wall 합계 116.576초, macOS child RSS
4,326회에 32.233초가 기록됐다. 직렬 sampler의 probe wall 합계는 epoch의
27.7%다. 서버와 시간이 겹치므로 제품 latency에서 이 합계를 차감하지 않는다.
Mixed Off는 canonical trace가 꺼져도 운영 profile의 periodic resource
관측은 유지한다. r44 `lm07`은 703.049초 epoch에서 fixture census
231.960초, child RSS 35.765초를 기록했다. probe wall 합계는 38.1%다.
현재 결과는 query·publisher가 관측과 어떤 자원을 얼마나 경합했는지
분리하지 않는다.

macOS는 100ms child RSS마다 `ps`를 실행하며, 500ms disk census는 각
경로의 `st_blocks`를 직접 읽고 scope별 inode 중복을 제거한다. 캐시된
논리 bytes를 physical allocation으로 사용하거나 hardlink/샘플 기준을
바꾸지 않는다. Darwin의 직접 RSS와 bulk directory attribute 조회는
각각 API·오류·실측 parity를 별도로 검증할 후보다.
[Apple XNU process attributes](https://github.com/apple/darwin-xnu/blob/main/bsd/sys/proc_info.h) ·
[Apple XNU getattrlistbulk](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2).

### r44 XL Off의 재실패

`xo44`는 동일 32,768 sources·seed42의 staging을 93.388초에 끝냈다.
원 120초 server deadline으로 BuildPrepare가 거절돼 harness ACK phase는
138.490초에서 실패했다. 첫 denial은 work count 또는 메모리가 아닌 deadline이다.
checkpoint는 packed occurrence의 `VectorIndex::locate`
(`text_occurrence/block.rs:55`)였다. r43의 물리 설치 checkpoint와 위치가
다르다. 별도 단계 계측이 없는 Off 실행으로 두 위치의 인과 비율을 추정하지 않는다.

실행 중 load1은 4.22–6.65였다. r43의 4.94–7.40보다 낮아도 원 실패가
재현됐다. load1은 CPU 경합·장치 지연의 정확한 분해가 아니다. r44의
logical reads는 57,799,380회, bytes는 54,158,120,563B였다. r43과 비교해
거절 시점까지의 작업량은 늘었으나 단계별 단위 처리량이나 성공을 증명하지 않는다.
첫 검색·재오픈은 미도달이다.

### r45 XL Off의 BuildPrepare 거절

`xo45`는 B48–50을 포함한 `5d830c1e…`에서 32,768 sources·seed42의
staging을 100.750초에 완료했다. 원 server 120초의 첫 차단은
BuildPrepare deadline이다. 실패까지의 ACK phase는 137.642초다.
checkpoint는 `object/read_io.rs:165`의 객체 bytes budget charge다.
원 객체 읽기는 namespace·shard·파일 identity·길이·digest 검증을 한다.
이 위치는 시간 초과를 관찰한 지점이다. r44의 occurrence vector checkpoint와
함께 단일 함수의 elapsed time이나 디바이스 병목을 증명하지 않는다.

거절 시 logical work는 reads 42,238,827회, writes 31,882,956회,
bytes 43,773,309,145B, mutations 12,173,147회였다. 실행 load1은
5.96–10.15로 r44의 4.22–6.65보다 높았다. 서로 다른 실패 prefix의
logical work 감소로 B50의 처리량이나 절감 시간을 계산할 수 없다.
ACK·첫 검색·재오픈은 미도달이다.

다음 XL 진단은 같은 fixture와 deadline에서 BuildPrepare의
inventory prepare·posting scratch·installed block seal·physical object
read를 분리한다. 기존 trace에서 확인한 59,861회 posting flush는
InventoryDiff에 포함된 비용이다. 서로 겹친 span을 합산하지 않으며,
full Record 진단과 Off 제품 수용은 각각 판정한다.

### r46 XL Record의 완료 prefix

`xr46`는 같은 binary·source `8684fe0c…`의 로컬 진단이다.
staging 106.676초, ACK phase 143.769초에서 실패했다. 원 server 120초의
BuildPrepare deadline은 Catalog object confirm의 lifecycle checkpoint
`managed_install.rs:844`에서 관측됐다. 최초 ACK·검색·재오픈은 미도달이다.

daemon Record는 256MiB에서 partial이다. 2,612,635개 기록, 857,584개
미기록, 열려 있는 span 8개가 footer에 남았다. 아래 집계는 full 요청
BuildPrepare 시작 165.891초에서 마지막 기록 222.633초 사이의 완료 span이다.

| BuildPrepare 완료 prefix | 건수 | 누적 wall |
| --- | ---: | ---: |
| Inventory prepare 전체 | 13,107 | 33.973초 |
| 그 안의 token spool / ordered group·pack | 각 13,056 | 6.641초 / 15.768초 |
| Inventory Diff | 12,844 | 15.563초 |
| Diff 안의 Posting scratch batch | 88,337 | 10.294초 |
| Inventory MapApply | 51 | 3.213초 |
| Managed object install | 102 | 10.433초 |
| Managed install 안의 Catalog COMMIT | 204 | 7.783초 |

COMMIT은 설치의 하위 구간이고, 설치 중 일부는 prepare와 겹친다.
전체 daemon의 별도 object stderr counter는 reserve/confirm 690회,
물리 write·file sync 각각 93,016회다. startup/staging과 BuildPrepare가
함께 들어 있다. scope 전체 reserve 35.005초, confirm 46.774초를 위
BuildPrepare 합계에 더하지 않는다. 완료 prefix에서 물리 write 0.275초,
file sync 0.380초, directory sync 0.153초, barrier 1.818초가 관측됐다.
계측된 설치 비용을 분리하면 Catalog COMMIT이 파일 write나 sync보다 크다.

cold token의 owner-local 인메모리 그룹, packed Diff의 ordinal 재사용,
Posting 전용 scratch batch를 각각 검증한다. 범용 SQL 경로의 hard file
cap과 bind limit를 유지한다. SQLite의 변수 한도는 연결 설정으로도 낮출
수 있으므로 batch 확대는 현재 connection에서 확인한다.
[SQLite limits](https://sqlite.org/limits.html).

### r48 XL Off의 설치 결과 checkpoint

`xo48`은 B57–61을 포함한 `930e30da…`, cooperative lease에서
32,768 sources·seed42 staging 105.752초, ACK phase 141.836초였다.
원 server 120초의 첫 차단은 BuildPrepare deadline이다. checkpoint는
`OccurrenceWrites::flush`의 `put_batch` 반환 이후 설치 결과 대조였다.
최초 ACK·검색·재오픈은 미도달이다.

거절까지 reads 3,354,864, writes 9,916,186, bytes 8,007,602,345,
mutations 13,064,994였다. r46의 35,834,681 reads와 12,806,904 mutations는
별도 실패 prefix다. 논리 scratch charge 감소는 확인했으며 두 prefix의
wall을 동등한 throughput으로 해석하지 않는다. 이번 load1은 6.81–8.92였다.

r46 Record에서 같은 full 요청 span은 131.412초에 시작했고 SourceApply는
134.496–165.890초의 31.394초, 그 안의 owner prepare는 20.929초였다.
32,768건에서 RecordGet 9.678초, BodyGetValidate 9.661초, record
decode/path 검증 0.053초다. Source map apply는 10.118초였다.
BuildPrepare는 165.891초에 시작했다. 두 단계는 원 요청 deadline을 공유한다.
Lexical은 SourceApply가 검증한 body를 토큰화할 때 다시 읽는다. 현재
4MiB attempt cache로는 XL body를 모두 유지할 수 없다. 단계 사이의 bounded
body 전달을 검토하려면 최초 인증과 뒤의 tokenizer 바이트 공급을 같이 보존해야
한다. r48의 새 scratch 경로 이후 실제 CPU·Catalog 시간은 다시 분리한다.

### r49 XL Record의 120초 귀속

`xr49`는 `d4e0a5dc…`의 최적화 pair와 원 server 120초를 사용했다.
staging 95.055초, ACK phase 140.130초, producer exit 1이다. 거절은
BuildPrepare 중 Storage 객체 decode의 bytes charge에서 관측됐다.
reads 3,484,721, writes 10,854,356, bytes 8,767,119,331,
mutations 14,383,635였다. 최초 ACK·검색·재오픈은 미도달이다.

daemon child trace는 3,144,831 events, 누락·drop·open span 모두 0이다.
실패한 SourceAdvance는 116.020초에 시작했다. 아래 wall은 해당 요청 안의
inclusive 시간이다. 부모와 자식 구간을 합산하지 않는다.

| 단계 | 건수 | 누적 wall | 누적 thread CPU |
| --- | ---: | ---: | ---: |
| SourceApply | 1 | 31.740초 | 22.921초 |
| SourceApply의 RecordGet / BodyGetValidate | 각 32,768 | 9.879초 / 9.607초 | 6.383초 / 6.584초 |
| SourceApply의 map apply | 1 | 10.299초 | 8.267초 |
| BuildPrepare의 lexical build | 1 | 85.140초 | 69.893초 |
| Inventory prepare의 token spool / ordered group·pack | 각 22,261 | 4.733초 / 23.429초 | 4.719초 / 17.954초 |
| Inventory Diff의 Posting scratch batch | 75,723 | 17.123초 | 16.448초 |
| Managed install | 173 | 16.967초 | 13.644초 |
| Managed install의 Catalog COMMIT | 346 | 11.022초 | 4.779초 |

BuildPrepare는 85.141초에 종료했다. Inventory Diff는 22,016개 owner에서
25.904초였고, Inventory MapApply는 86회 5.517초였다. Managed install의
물리 write 0.376초, file sync 0.522초, directory sync 0.280초와 barrier
2.807초는 하위 집계다. Catalog mutex wait는 346회 합계 0.002초 미만이었다.
설치 173회 중 87회(10.458초)는 group·pack 안에서 실행됐다. 해당 설치를
제외한 group·pack inclusive 잔여는 12.971초다. 설치의 `managed_object_inputs`
폭은 95·96·160·161개였다. 256행 inventory 창을 약 160개와 96개로
쪼개는 1MiB occurrence staging 한도가 실제 실행에서 작동했다.
이 실행에서는 Catalog의 COMMIT과 tokenizer·scratch 자체의 CPU 비용이
각각 관측됐다. 호스트 load1 4.86–11.64는 r48과 다른 조건이며, Record를
Off 속도 개선량으로 사용하지 않는다.

ASCII token 준비에서 Unicode case iterator의 중복 순회를 제거하는 B65는
이 중 일부를 겨냥한다. 그룹·패킹에서 키별 정렬과 allocation, Posting
scratch에서 실제 VM 실행, 설치에서 reserve/confirm의 고정 비용을 각각
분리해야 남은 절감 규모를 계산할 수 있다. SourceApply와 lexical은 한 시도의
4MiB read cache를 공유한다. 보유된 객체는 `read_retained`, 창 밖의 객체는
재인증 읽기를 거친다. 단계 간 전달을 바꾸려면 memory lease와 최초 인증을
보존해야 한다.

### r51 XL Off의 재실패

`xo51`은 B68–71 `1482d857…`에서 staging 98.897초와 ACK phase
141.436초를 기록했다. producer exit 1, 원 server 120초가 BuildPrepare의
물리 설치 child-validation checkpoint `object.rs:821`에서 관측됐다.
reads 3,552,143, writes 11,324,294, bytes 9,144,965,034,
mutations 15,024,737이었다. 호스트 load1은 5.71–7.07이었다. r48 Off보다
거절 시점까지 계상한 작업량이 많지만 동일한 phase prefix나 matched-host
비교가 성립하지 않는다. 최초 ACK·검색·재오픈은 미도달이다.

같은 pair의 `xr51` Record도 producer exit1이다. SourceApply 32.029초,
BuildPrepare 85.057초, staging 105.159초, ACK phase 142.171초였다.
거절 checkpoint는 Storage 설치 파일의 `object.rs:1095`이며, child trace
3,159,263 events에 누락·drop·open span이 없다. host load1은 5.41–7.86이다.
BuildPrepare는 22,016 owner의 Inventory Diff 27.106초, Posting flush
75,723회 18.001초, Managed install 173회 17.410초, 그 하위 Catalog
COMMIT 345회 12.089초를 기록했다. 겹친 stage를 합산하지 않는다.

B69의 같은 flush parent에 귀속된 21개 표본은 전체 3.305ms, capacity
admission 0.177ms, SQLite `raw_execute` 2.338ms, 나머지 0.790ms다.
표본 중앙값은 각각 0.107·0.007·0.055·0.033ms다. 해당 표본에서 capacity
확인은 약 5%, `raw_execute`는 약 71%다. 실행 비용에는 `(term, owner)`
B-tree 적재가 포함된다. 이 표본의 비율을 전체 18.001초에 외삽하지 않는다.
다음 코드 후보는 삽입 순서나 인덱스 구축 비용을 줄이는 쪽이다. 4GiB scratch
안에서 임시 정렬·중복 감지·정확한 term/owner 순서를 증명해야 한다.

외부 임시 디렉터리의 SQLite 3.50.4 prototype은 소문자 합성 fixture의
434,176 canonical 행을 세 layout으로 적재해 정렬된 결과 digest 일치를
확인했다. 각 layout 3회 중 적재·seal 중앙값은 기존 0.547초, append 후
정렬 삽입 0.539초, 동일 case 쌍 저장 0.189초였다. main DB 크기는 각각
40.89·79.08·20.23MiB였다. append 정렬의 실행 범위 0.499–0.981초는
기존 0.539–0.615초와 겹쳤다. 임시 정렬 파일의 peak 및 제품의 Unicode·혼합
case·page 경로는 이 prototype 범위 밖이다. 같은 suffix·owner·old/new ref를
증명한 쌍만 압축하고, term page·cardinality·cursor에 원래 case 키를
확장하는 계약이 있어야 제품 경로를 바꿀 수 있다.

### B53 — packed Diff의 term당 scalar memo

정적 감사에서 `validated_new_count_observed`는 exact packed cursor가 있어도
RAM scalar memo와 scratch `SELECT ref,validated_count`를 먼저 확인했다.
scalar `Validated(count)`는 leaf에만 기록되며 packed block은 key별 값이
다르다. 64개 기본 RAM 창에 들지 않은 packed ref는 각 term에서 같은
SQL을 조회한 뒤 같은 cursor로 돌아왔다. B53은 같은 ref로 확인된 cursor를
scalar memo보다 먼저 사용한다. 최초 retained read와 block/key 검증,
기존 independent seal, zero-cache 경로는 유지한다.

seed42 XL 생성기를 모델링한 결과 32,768개 파일의 case-prefixed term은
파일당 422–446개, packed payload는 6,274–6,846B로 alias 상한 안이다.
모두 packed 분기에 도달한다고 가정한 최대 조회 후보는 약 1,383만–1,461만
건이다. 실제 경로 비율과 wall 절감은 r46 실행 전까지 미측정이다.
별도로 alias seal과 Diff cursor가 동일 block을 각각 파싱한다. 두 번째 GET을
줄이더라도 독립 persisted seal과 batch 전체 확인 순서를 유지해야 한다.

### r46 XL Off의 scratch checkpoint

`xo46`은 B51–B53과 f112 golden 교정을 포함한 `8684fe0c…`에서
32,768 sources·seed42 staging을 99.610초에 완료했다. 최초 120초
server deadline은 다시 BuildPrepare에서 만료됐다. harness ACK phase는
141.811초, 최초 ACK·검색·재오픈은 미도달이다. checkpoint는
`scratch-sqlite/src/edit_write.rs:175`의 공용 batch `raw_execute`
직후다. 이 코드가 처리한 batch의 caller와 누적 시간을 Off에서 알 수 없다.

거절 prefix의 reads는 35,834,681회, writes 33,561,826회, bytes
45,571,418,304B, mutations 12,806,904회다. r45보다 reads가 6,404,146회
적고 writes가 1,678,870회 많다. 다른 완료 prefix이며 실행 load1의
범위는 4.29–12.41이었다. 이 차이로 처리량이나 B53의 wall 절감을 계산하지
않는다. 같은 binary의 Record를 cooperative lease에서 실행해
inventory/Diff와 physical install/COMMIT의 완료·실패 span을 나눈다.

### r43 후속 감사의 비용 경계

- **XL 물리 객체.** 이전 r39 Record에서 완료된 inventory owner 8,865개에
  lexical prepare 물리 write·fsync가 각각 8,864회였다. 이 근접 수치와 단일
  packed block을 inventory로 재사용하는 cold 경로를 대조했다. per-owner
  block 크기나 write batch 상한 증가로 이 물리 객체 수를 크게 줄일 근거는
  없다. cross-owner 묶음은 `(owner, term)` 선택자와 delta 격리, pin/GC,
  crash/reopen 계약을 바꾸므로 이 구간의 귀속이 확정될 때 별도로 판단한다.
- **XL SourceApply.** cold owner의 record와 body를 각각 한 번 검증한다.
  같은 시도에서 재접근한 인증 객체는 기존 4MiB window를 사용한다.
  r39 RecordGet 17.619초와 BodyGetValidate 17.898초는 inclusive wall이다.
  원 측정으로 물리 get, hash, syscall, 스케줄 대기 비율을 분리할 수 없다.
  StageSources의 선행 검증을 임의의 참조를 받는 SourceAdvance 권위로 사용하지 않는다.
- **Mixed 대기.** 첫 broad query는 원 2초 중 1.517초를 harness 대기열에서
  보냈고 IPC에 약 483ms가 남았다. 제출→ACK의 2.742초 중앙값과 함께 보면
  발행 처리량 및 검색 대기 문제는 재현됐다. 이 표본만으로 서버 query 자체의
  실행 시간, Catalog COMMIT 비중 또는 호스트 경합의 기여율을 단정할 수 없다.
- **확인된 반복.** warm Posting metadata가 한 immutable terms root의 최대
  64개 키에 scalar lookup을 호출했다. 기존 `ScopedReads::lookup_batch`는
  같은 scope의 인증 DATA window를 우회했다. B44는 scope 내 배치 경로와
  Posting 호출을 함께 바꾼다. 기존 wrapper와 df 검증 및 독립 seal은 유지한다.

- **XL scratch flush.** r39의 Posting scratch flush 59,861회·9.102초에는
  64행 묶음의 실제 B-tree 삽입이 포함된다. 한 spool transaction 안에서
  이미 packet당 한 SQL을 실행한다. inventory batch 사이에 잔여 행을 합쳐도
  실행 감소는 약 0.2% 이하다. 9.102초 전체를 추가 COMMIT으로 계상할 수 없다.
- **XL inventory Map readback.** 같은 r39의 완료된 34개 배치에서
  `NativeTextInventoryMapApply/2`는 wall 6.494초·thread CPU 6.211초,
  work reads 7,686,081회·work bytes 4,822,761,332B, work writes 0이었다.
  work는 물리 read bytes나 장치 I/O가 아니다. 이 단계의 CPU와 읽기 반복은
  물리 fsync와 구분된다. 단일 packed block owner의 실제 수와 SQL 행 비용을
  나눠 보고, 독립 seal을 유지하는 bounded alias witness가 제거할 범위를 정한다.
- **회귀 기대값.** f102에서 실패한 occurrence 테스트 4개는 f103의 B44
  이전 소스에서 동일하게 실패했다. 차이는 spool이 살아 있는 동안 보유하는
  `PacketEvidence` grant 8,208바이트와 일치한다. 거절된 packet의 자체
  grant와 spool 소유 grant를 분리하고 spool drop 뒤 0으로 복귀하는지 확인한다.

### B22 Large 실측

r31 `01c4bb4d…`와 r32 `58d69e24…`는 동일 corpus digest, Native config digest,
4,096 sources·seed42, 120초 dispatch, 명시 work cap을 사용했다. 각 pair의 소스·
두 바이너리 해시와 실행 artifact 해시를 대조했다. 다음 수치는 first full 요청의
최상위 Map과 그 하위 trace에 해당한다.

| 지표 | r31 Record `lr26` | r32 Record `lr27` |
| --- | ---: | ---: |
| terms / owners staged | 12,394 / 274 | 5,272 / 274 |
| 실제 write / file sync | 각각 12,668 | 각각 5,546 |
| 물리 설치 배치 | 50 | 22 |
| Catalog COMMIT (`SqliteWork/3`) | 63 | 28 |
| 최상위 Map wall / CPU | 27.557 / 18.543초 | 12.464 / 6.720초 |
| COMMIT wall / CPU 합계 | 8.756 / 3.718초 | 4.095 / 1.220초 |

물리 객체 수가 56.2% 줄었고, 정적 모델의 5,546개와 실제 write·file sync가
일치한다. r32의 Off·Record 최초 ACK는 46.419초·48.192초, 첫 검색은
95.888ms·103.202ms, 재오픈 검색은 56.732ms·44.830ms였다. 두 제품 실행의
producer exit는 0이었다. Record first/reopen은 677,649/957 events, drop/open 0이며
canonical profile을 검증했다. prebuilt 형식의 managed build binding은 `UNPROVEN`이다.

r32 Record에서 first full의 `lexical_build`는 wall 42.028초, CPU 28.047초였다.
이 안의 Posting은 22.009초, 최상위 Map은 12.464초다. 별도 source staging은
Record 15.071초와 Off 21.643초였고 첫 ACK의 server dispatch 구간에 포함되지 않는다.
r32 Off load1은 21.69–29.97, Record는 13.82–37.28이었다. 측정 중 호스트 전체
swap in/out은 각각 15.758/37.214GiB였다. syscall·배치·COMMIT 감소는 코드 변경의
직접 증거다. 호스트 상태가 달라 전체 wall 차이의 코드 기여율과 tracing overhead는
분리할 수 없다.

Source staging의 `lr27` 15.071초는 SDK 상한 32개를 적용한 128회 요청과
일치한다. 각 요청은 본문과 레코드를 하나의 내구 설치로 묶는다. 인코딩 합계
0.010초, Catalog lock wait 0, lock hold 7.521초, 물리 설치 7.052초였다.
32개 상한을 높이면 트랜잭션 반복은 줄일 수 있지만 8,192개의 내구 객체 설치는
남는다. 현재 120초 ACK 계약 밖의 시간으로, 이 측정에서 상한 변경을 요구하는
실패는 확인하지 못했다.

`lr24`는 producer 종료 후에도 Record 검증을 계속했다. 캡처와 제품 검증기가
동일한 authenticated raw child를 각각 엄격 파싱하던 경로를 한 번으로 줄였다.
제품 검증기가 raw custody와 엄격 파싱을 담당하며, 독립 호출자가 파싱 결과를
전달하면 raw와 대조한다. 캡처·제품 검증 Python 테스트 520개가 통과했다.
managed build receipt가 없는 prebuilt 경로의 테스트 2개는 제외됐다.
이 변경의 Large 검증 시간 재측정은 `NOT_RUN`이다.

`lr24` 첫 Full의 `build_prepare`는 86.583초다. 그 안의 lexical build는
wall 86.443초와 CPU 56.503초다. inventory prepare 20.785초 중 32개 물리
flush가 12.646초였고, inventory diff 9.866초와 inventory MapApply 3.671초가
이어졌다. SourceApply 앞의 owner prepare 8.683초 중 record get은 3.846초,
body get·UTF-8 확인은 4.002초였다. 해당 단계와 Lexical은 이미 같은 attempt
read cache를 사용한다. 현재 trace로 cache miss 절감량과 host 대기를 분리할 수 없다.

첫 Full의 cold inventory 32개 설치 배치는 각각 128 owner였다. 각 배치는
하나의 물리 설치와 reserve/confirm을 수행한다. B21은 cold 기본 창을 기존 hard
bound인 256으로 채운다. warm/명시 설정 상한 64, occurrence 1MiB, source 8MiB,
기존 사전 메모리 승인은 유지한다. 같은 fixture에서 바이트 한도가 먼저 차지
않으면 16배치·32 COMMIT까지 줄어든다. 배치별 live memory 증가와 실제 감소량은
고정 소스 `60d74310…`의 Large에서 확인한다.

## B23 — Inventory Diff의 packed DATA 재사용

`lr24` 첫 Full의 Diff 4,096회는 wall 9.866초와 CPU 8.300초였다. 그 안에서
posting edit 5.155초, inventory read 2.168초, occurrence read 1.858초였다.
count memo는 이미 selector 재검증을 1,142,420회 생략했다. 남은 중복은 동일
Diff에서 packed inventory cursor가 읽고 검증한 블록을 첫 count가 다시 해석하는
경로다. attempt read cache에 따라 실제 디스크 read 절감량은 달라진다.

새 경로는 같은 ref의 cursor가 검증된 DATA를 보유할 때 count에 재사용한다.
첫 count의 adapter `read_retained`와 객체 일치 확인을 수행한다. 전체 merge 및
token 합계가 일치하면 `CheckedBlock`과 grant를 기존 2슬롯 reader로 이동한다.
재사용 설정 0, 기존 cursor/CAS 검사, 오류 후 미채택을 검증 범위에 포함한다.
f74의 집중 packed Diff 회귀는 통과했다. Large 절감량 측정은 `NOT_RUN`이다.

## B24 — 읽기 전용 Catalog scope의 Record 이벤트 감소

`lr25`에서 확인한 빈 `lane22`를 줄이기 위해 상위 Catalog의 I/O 계측을 첫
write/sync까지 지연한다. Scratch의 기존 활성화 경로를 사용한다. 중첩 Catalog의
reentry 경계와 실행된 callback의 카운터 묶음, sync·거절 관측은 유지한다.
프로파일 Python 검증은 28개 통과했다. f74에서 read-only·실제 write·sync·checkpoint
회귀는 통과했고, 이전 VFS 두 테스트가 빈 scope의 lane22를 기대해 실패했다.
고정 fixture의 scalar arm은 이전에도 write 0건이었다. 기존 row/custody와 양수 write
counter 기대를 보존하고 empty scope 관측만 수정했다. `01c4bb4d…`의 f75에서
통합 집중 405개가 통과했다. Large 이벤트 수·ACK 재측정은 진행 중이다.

`r31`의 Off `lo27`은 최초 ACK79.470초, 최초 검색580.8ms, 재오픈 검색189.2ms였다.
증분·삭제·no-op·이전 pin·drain을 완료했고 producer exit0·117.782초였다. load1
18.14–23.42, swap 이동8.504GiB, 5초 간격 sampled daemon RSS 최대95.53MiB다.
이전 Off와 host 상태가 달라 wall 차이의 코드 기여분은 미측정이다.

`lo27ctl`과 `lr26ctl`의 보조 요약은 이전 실행 경로를 참조한다. 결과는 각 현재
`execution.json`과 artifact 해시, r31의 source/binary 해시를 직접 대조해 판정한다.
후속 r32 컨트롤러는 동일 run 경로를 생산·수집에 사용하고 source/binary/mode가
현재 실행과 일치하는지 검사한다.

같은 r31 pair의 Record `lr26`도 최초 ACK77.573초, 최초 검색385.3ms, 재오픈
검색205.0ms로 통과했다. 증분·삭제·no-op·이전 pin·drain과 product/canonical
검증을 완료했다. producer exit0·118.690초, 첫/reopen footer 706,957/1,125 events,
drop/open0이다. load1 22.50–36.64, swap 이동4.072GiB, sampled daemon RSS
최대89.72MiB다. Off·Record는 로컬 제품 검증이고 matched-host 비교는 `NOT_RUN`이다.

첫 Full의 cold inventory 물리 설치는 `lr24` 32×128개에서 `lr26` 16×256개로
줄었다. `lr26`의 최상위 Map은 terms 12,394개·owners 274개를 canonical report에
기록했고 실제 write/file sync도 각각 12,668개였다. 50개 물리 배치와 63회 COMMIT은
동일했다. Map wall/CPU는 27.557/18.543초, COMMIT wall/CPU는 8.756/3.718초다.
B22의 물리 노드 감소를 비교할 실행 기준이다.

첫 child 이벤트는 `lr24` 876,074에서 `lr26` 706,957로 19.3% 감소했다.
`lr26`의 `SqliteWork/22`는 18,718개이며 runtime maintenance의 zero-write scope는
21개다. 같은 유형의 `lr25` zero-write scope 10,406개와 대조된다. 이 이벤트 수와
별도로 ACK 차이는 host·코드가 함께 바뀐 구간이다.

별도로 Inventory scratch의 `UPDATE root=root-256`은 `WITHOUT ROWID`의
`(root,k)` PK를 실제로 이동한다. 현재 이 작업이 old/new 행 envelope를 모두
사전 예약한다. 같은 첫 Full의 inventory flush에서 managed reserve/install/confirm을
제외한 wall은 최대 약 2.19초다. 안정 키와 단일 packet 가시성으로 이 이동을 없애는
경우에도 마지막 충돌 재검사, 전역 cursor 거절, marker 뒤 optional memo 실패 동작과
추가 index 비용을 함께 검증해야 한다. 해당 스키마 변경은 보류한다.

## B03-B1 후보 자체의 검증 결함

이 절은 r4의 원 Large120 실패 원인과 별도다. B1의 공개 zero-reuse 설정이 block
cache에도 적용되도록 수리했고, mutable main의 helper 리팩터 중 컴파일 문제 이후
12개 파일의 외부 clean source에서만 영향 검증하도록 바꿨다.

`cc6b1e1`/f5는 Native109pass/1fail에서 멈췄다(182개 fail-fast 미실행,558개 제외).
원인은 decoder row key의 end offset이 뒤의2B vector selector까지 포함해 실제 존재하는
term을 찾지 못한 것이다. 기대값을 바꾸지 않고 key를 읽은 직후의 end를 저장하도록
수리했다. `36a883a`/f6는291pass/1fail이었고, 마지막 실패는 test port의
registered bytes가 전체 wire 길이가 아닌 fixture 오류였다. 실제 object-v1
framing/identity로 교정한 `642f74a`/f7은 Native292개·558개 제외를 통과했다.
f7 Kernel은 stale generic fixture로 FAILED, 같은 source의 f8은9pass/3fail이었다.
current MapBlock fault target, Inline의 cap35>32, Shared64 batch 내 pending dedup
reserve oracle을 수정한 `4b06b203e636b2e303652a3f55429b506c2fcd01`/f9는
Kernel12pass·65개 제외·125.827초, source/tool guard0으로 VERIFIED다. f10 strict
Clippy는 당시6개 lib 지적으로 FAILED였다. B1 소유2개를 수정했고 f16의 lib/tests
`--keep-going`에서 Native 소유 진단0을 확인했다. 전체는 다른 owner 진단33개로 FAILED다.
후속 mr5 Medium과 lo5/lr5 Large Off/Record 결과는 아래와 같이 모두 FAILED다.
이 후보 결함을 기존 r4 deadline의 원인으로 소급하지 않는다.

f11 Tools03의 old Record epoch는 PATH identity guard가 producer body0으로 거절했다.
15개 path에서 `st_dev` 33→30만 바뀌고 hash/inode/mtime 등은 동일함을 검사한 새
Tools03 invocation epoch를 고정했다. guard를 우회하지 않았다. f12는 입장 대기
prefix 뒤 PID/open FD가 없고 실행 body가 없어 NOT_RUN으로 종료됐다.

새 epoch의 f13은 source347에서 Native292pass, Kernel13 중11pass/2fail이었다.
두 실패의 raw JSON leaf 가정은 실제 public leaf/block reader 계약이 아니어서
TF/positions 검증으로 바꿨다. f15 source `68ea46a…`는 Native292pass, Kernel13 중
12pass/1fail이었다. 남은 실패는 내용이 같아도 다른 ref mapping에 같은 block id를
기대한 oracle 오류다. 동일 ordered ref mapping을 재구성하는 추가 Tree→Block roundtrip
검증으로 교정했다. physical map wire/ordered children 및 pin/GC 검증은 보존했다.
후보 `d8e2c52de9f0e058475aa25e3d746cdad3015672`의 f17 변경 transition test는
1pass·77개 제외·6.337초다. d8e2와68ea 차이는 이 Kernel test 파일1개이므로
f15 Native292+기존 lifecycle12와 f17 변경 case1의 **composite local proof**는
VERIFIED다. 같은 exact commit의 full suite나 제품 수용은 아니다.

### 실제 Large 진입과 후보 활성화의 불일치

실제 harness는 step995에서 첫 profile, step1059에서 첫 SourceAdvance Patch를 만든다.
f9까지의 `pack_owners` Bootstrap-only 조건은 이 full Large 경로에서 block을 켜지
못했다. 따라서 f9의 기능 검증은 원 Large120 수리의 활성화나 속도 향상을 입증하지
않는다. 공유 Main은 `Bootstrap` 또는 `Patch(delta.prior.owners.0.is_none())`에서
packing하도록 수정했고, Sparse의 nonempty prior leaf/old-ref bound는 유지했다.
profile-first 초기 Patch의 공통 pin/GC/rebuild를 다루는 Kernel case는 f17에서
통과했다. 후보 source의 전체 Kernel13 동시 실행은 NOT_RUN이며, 위 composite
proof 범위만 VERIFIED다.

seed42의 Large 원입력8,609,531B는 확인됐다. 약33,716 unique leaf2.94MB에서
12,288 block23.93MB로의 변화는 현재 코드로 계산한 **Large 추정**이며 제품 측정이 아니다.
물리 객체 수 감소가 예상되는 반면, block header/term과 cold 단일-term read의 bytes
증가 위험이 있다. 원 metric별 region bounds, E07 fixed mixed query SLO, work/memory
한도로 채택 판단한다. 원 Large120초·별도 ANN 조건을 완화하지 않는다. Medium의
fixed512MiB는 진단 입력이며 Large 수용의 hard gate가 아니다.

후보 source는 Main의13개 파일을 외부 안정 `s3`에 복사해 local snapshot으로 고정했다.
그 source freeze 당시 Root는 공유 Main HEAD/index를 바꾸지 않았다. 이후 다른 작업의
커밋까지 불변이라는 뜻은 아니다. `candidate-source-03.json`의 focused status는
VERIFIED다. `candidate-build-03.py`는 같은 controller env에서 normal r5 build와
Medium Record를 직렬 실행해 terminal을 남겼다. build exit0은 VERIFIED이며 Medium은
실제 full ACK 전 bytes cap에서 FAILED다. controller의 `pipeline_completed=true`와
전체 exit0은 제품 성공을 뜻하지 않는다: 내부 `medium_cli_exit_code=2`,
`medium_execution.exit_code=1`이다. lo5 Large Off도 아래와 같이 FAILED다.

### r5 후보 Medium의 비용 이동과 남은 거절

`d8e2c52d`의 mr5는 seed42/256 source/367,590B, fixed
1,000,000 reads·writes·mutations 및 536,870,912 bytes, Record에서 실제
`full_event_ack`를 실행했다. producer exit1·elapsed11.029초·outer timeout=false다.
`first_denial=bytes`: accepted usage는 686,015R/520,972W/536,866,122B/302,050M,
이어 scratch `edit_write.rs:79`의 14,133B charge가 거절됐다. `failed_stage=BuildPrepare`,
물리 write/fsync/directory-sync/barrier 실패0, full ACK·reopen·search는 미도달이다.
이 거절은 정확한 work cap 시행이며 테스트 fixture 오류나 host deadline이 아니다.

mr5 first child V5는 39,223 events, 7,684 spans, complete, health/orphan/dropped/
unwritten/open/lease 0이다. LexicalBuild lane2의 도달 prefix는 668,388R/514,386W/
532,360,166B/296,839M, wall3.299497초·thread CPU1.690297초다. SourceApply는
10,686R/3,387W/3,991,727B/3,608M, wall0.352170초·CPU0.197080초다.
하위 span은 모두 inclusive라 합산하지 않는다.

| mr4→mr5, 같은 Medium 입력의 Lexical 하위 | accepted bytes | mr5 span 결과·시간 |
| --- | --- | --- |
| InventoryPrepare lane2 | 261,371,848→321,302,214 (+59,930,366) | 260 ok, wall1.071781초·CPU0.518836초 |
| InventoryMapApply lane2 | 148,640,926→147,263,286 | 4 ok, wall1.098860초·CPU0.597989초 |
| InventoryDiff lane2 | 29,935,047→28,837,917 | 256 ok, wall0.364964초·CPU0.300533초 |
| Postings lane2 | 70,301,512→12,845,512 (실패 prefix) | lane37 error, lane38/39 미도달 |

Prepare의 lane34 tokenizer는256 ok, wall0.069646초·CPU0.058463초;
lane35 owner path는256 ok, wall0.086755초·CPU0.068366초다. 둘에는 work delta
counter가 없어 +59.93MB를 어느 세부 charge site에 나눌 수 없다. r4→mr5 managed
reserve input은5,230→1,635, install input은5,224→1,629, 실제 object write bytes는
3,298,069→3,566,935B다. InventoryDiff next-occurrence validation은3,083→512,
RAM reuse13,952→50,614, spill reuse34,091→0이다. B1이 객체·검증 호출을 줄인
사실과 Prepare logical bytes 증가가 동시에 관측됐다. 각 run의 work cap이 동일해
Lexical accepted bytes 총량은 거의 cap에 묶인다. 따라서 Postings prefix 감소를 전체
work 절감이나 Large 속도 향상으로 해석하지 않는다. 다른 소스·실행 환경의 wall/CPU
비율도 정식 성능 비교가 아니다.

l6ctl은 build/runtime PATH drift에서, l7ctl은 tool mount `st_dev` epoch 변화에서
각각 producer body0으로 거절됐다. lo5는 모든 tool metadata를 새 epoch에서 검사한
l8ctl로 시작했고, r5와의 PATH-only 차이는 별도 local runtime cohort로 선언했다.
가드를 완화하지 않았다. lo5의 직접 `execution.json`과 refusal은 source
`d8e2c52d` 및 r5 pair SHA를 기록한다. Large 원 입력 seed42/4,096 source,
reads/writes/mutations 각각67,108,864 및 bytes549,755,813,888, trace Off,
server dispatch120초·outer600초다. 실제 producer exit1/226.898901초,
`full_event_ack` incomplete wall124.435900초이며 `first_denial=deadline`,
`failed_stage=BuildPrepare`, checkpoint는 storage `object.rs:628`이다. accepted
usage는10,550,723R/8,042,018W/8,296,724,947B/4,545,263M이고 numeric cap,
memory, external-resource refusal은 없다. full ACK/search/reopen에 미도달했다.
같은 source에서 B1이 활성화돼도 원 Large120은 닫히지 않았다.

`l8ctl/terminal.json`의 요약 `execution.elapsed_seconds=145.286325`는 이전
`lo4/execution.json`을 읽은 controller copy bug다. 이 요약값은 lo5 결과로 쓰지 않는다.
`lo5/execution.json`의226.898901초와 lo5 refusal/CLI exit를 직접 판정한다.
원 terminal을 덮어쓰지 않고 controller 교정 기록을 별도로 남긴다. 이 local cohort는
PATH-only drift와 CLI `build_binding=UNPROVEN` 때문에 managed-fresh·matched-host
성능 비교 자격이 없다. 실패한 원120초 제품 조건만 관측했다.

같은 r5 pair/source의 lr5 Large Record도 producer exit1이다. server request는
120.654초, full ACK phase는127.697초 도달 prefix 뒤 deadline에서 거절됐다.
`first_denial=deadline`, `failed_stage=BuildPrepare`, checkpoint는 storage
`object.rs:593`; accepted usage는10,362,905R/7,918,927W/8,141,195,237B/
4,482,173M이다. full ACK/search/reopen은 미도달이다. Native child V5는
858,005 events written/4,698 dropped/7 open/0 leases로 **partial**이다. parser가
닫힌 span을 판독했어도 완전 캡처 또는 전체 작업 계수로 승격하지 않는다.

lr5 실패 request의 SourceApply는 wall15.897초/CPU4.269초, LexicalBuild는
wall103.728초/CPU42.419초다. Lexical의 선택된 accepted work prefix는
10,083,692R/7,815,076W/8,064,744,528B/4,400,049M이다. 하위
InventoryPrepare lane2는2,860 spans, wall41.232초/CPU14.563초,
4,979,364,938B; InventoryMap lane2는44 spans, wall31.917초/CPU14.817초,
2,250,442,352B; Diff lane2는2,815 spans, wall11.428초/CPU7.386초,
508,500,555B다. ManagedObjectInstall 132 spans는 wall54.302초/CPU18.740초로
Lexical 하위와 중첩한다. lane34/35는2,816 owners, 44 map batches×64이며
deadline 시점에1,280 owners가 남았다. Postings는 완료되지 않았다. 물리 진단의
23,630 writes/58,508,801B와 write/fsync 실패0은 도달 prefix만 나타낸다.
포함 관계인 span 시간을 합산하거나 r4와의 속도 비율·tracing overhead로 읽지 않는다.

### typed inventory confirmation fee의 charge-site 수리

mr5의 Prepare 비용 증가를 소스에서 재검토했다. `admit_inventory_confirmation`은
planned inventory `p/n`을 `optional_map_value`로 인증하고 `n`을 실제 installed
`refs`의 Object로 확인한 뒤에도, 각 행의 읽기와 PK 이동 사전 charge에
`MAX_MAP_VALUE_WIRE_BYTES=195`를 `p/n` 두 필드 모두에 적용했다. 이 행은 실제로
`p=None` 또는 Object, `n=Object`만 허용한다. Null은 1B로 보수적으로 charge하고
Object의 canonical wire는 33B다. 기존 `key+2×195+72` envelope를 인증 후
`key+(1 또는 33)+33+72`로 바꾸었다. 읽기 1회와 PK 이동 2회에 쓰이므로 planned
행당 accepted logical fee 차이는 prior Null 1,068B, prior Object 972B다.
실제 행 수와 남은 Postings 작업을 모르면 총 절감량·Medium 통과 여부를 확정할 수 없다.
원 cap을 늘리거나 charge를 생략하지 않았다.

같은 bounded SQL batch, pre-install `reserve_for_write`, confirmed-record scratch 합산,
물리 설치 뒤 count/ref 재검증 및 SQLite 하드 page cap을 유지한다. `p`가 Inline이거나
`n`이 Object가 아니면 기존 typed corruption을 유지한다. literal codec/fee 테스트는
Null/Object 20,884B와 Object/Object 20,980B를 고정 gold로 확인하고 각각 exact−1
예산의 거절에서 planned row가 Ready로 보이지 않음을 검증하도록 추가했다.
이 변경은 **code-level fee correction**이다. CPU/wall 개선이나 제품 수용의 실측이 아니다.

첫 frozen 후보 `c49b2a1fa0e3dc30d88e6189ef0cc0ddbac25bfa`는 Main에서 소유
15개 파일을 복사했다. f18은 새 테스트의 `check_eq!`에서 임시 encoded 값의 참조 수명
E0716으로 컴파일 FAILED, 실행 body0이다. 실제 생산자 결함 판정은 아니다. 기대값이나
제품 코드는 바꾸지 않고 encoded 값을 지역 변수에 묶은 test-only 교정을 새 frozen
`a6083fb22480786d5e8172057b6fbe291af26ee7`에 고정했다. f19 focused Native·Kernel
terminal은 Native exit0/lifecycle exit0이다. Nextest는 Native293pass/558 제외,
Kernel lifecycle13pass/65 제외를 실제 실행했다. 이 exact source의 영향 집중
scope는 VERIFIED다. 같은 source의 r6 normal cached pair build는 exit0,
mr6 Medium 진단 body는 FAILED다. Large120은 NOT_RUN이다.

### mr6: typed fee 절감의 실제 크기와 다음 거절

r6는 `a6083fb22480786d5e8172057b6fbe291af26ee7`의 정상 cached pair build
exit0이다. 같은 controller 환경의 mr6 Medium fixed512MiB Record는 producer
exit1/13.664초, outer timeout=false, full ACK 전 거절됐다. `first_denial=bytes`,
accepted 874,591R/573,176W/536,864,032B/350,737M 뒤 Catalog
`managed_install.rs:108`의 10,692B charge가 실패했다. numeric bytes cap 시행이며
memory/external-resource refusal, 물리 write/fsync/dir-sync/barrier 실패는 없다.
물리 진단은2,403 writes/3,946,025B다. r6 controller의 완료 flag는 제품 성공이 아니다.

mr6 child V5는45,923 events/8,713 spans, complete, health/orphan/dropped/unwritten/
open/lease0이다. mr5와 같은 Medium 입력에서 InventoryPrepare lane2 accepted bytes는
321,302,214→266,699,646B로 **54,602,568B 감소**했다. 이는 첫 인덱싱의 prior
Null/Object 51,126 planned 행×행당1,068B와 정확히 일치한다. 코드에서 특정한
generic195B fee의 제거가 실제 WorkBudget에 반영됐다는 근거다. MapApply의
147,263,286B와 Diff의28,837,917B는 그대로이며, Postings lane37 실패 prefix는
12,582,998→67,183,476B로 진행했다. 결과적으로 512MiB 전체 cap에서 더 뒤의
Catalog 설치 사전 charge가 거절됐다. LexicalBuild selected prefix는
856,964R/566,590W/532,358,076B/345,526M, wall5.745559초·CPU2.953458초다.
이 nested/실패-prefix 시간은 총 완료 시간이나 mr5 대비 속도 개선율이 아니다.
실제 Medium fixed 진단은 FAILED이며, B03-B2의 8KiB/512-term 실험은 같은
진단 cap으로 비교한다. 별도 Large 제품 cap은 변경하지 않는다.

### B03-B2 코드 후보와 아직 없는 제품 증거

다음 후보는 한 owner의 occurrence dictionary block을 기존 `QITXOC01` wire/reader
경로에서 4,096B/128행에서 **8,192B/512 canonical term**으로 확장한다. 같은
positions vector 공유, singleton/oversize leaf, nonempty-prior Patch의 unchanged
old ref 보존,64행 SQL flush·planned-confirmation row/ref binding은 유지한다.
`a6083fb2` 대비6개 파일 변경을 포함한 소유 입력15개를 외부 stable `s3`의
`878be6c9e4e33dd35b9c80cc5b450c2cf3f6f099`에 고정했다.

새 output policy(admission `[31;32]`)의 literal digest는
`b2c1df18b452485d69ef4e687b7a3519f07cfff76fe25f0a665d8706258d7d95`다.
직전 4KiB/128행 policy `eef15862…`와 leaf-only `559e0482…`는
`validate_instance`의 typed refusal oracle로 남겼다. 동일 wire magic만으로
다른 bounds의 old object를 현재 policy로 재해석하지 않는다. 테스트 소스에는
512행/8KiB·Full·cancel/memory 및 실제 FileObjectStore/SqliteCatalog의
260-selector 한 block·public 첫/마지막 selector·delta·pin/reopen case가 포함됐다.
기존 lifecycle case가 마지막 pin·GC를 다룬다. f20 Native는295개 통과·558개 제외로
VERIFIED다. Kernel은 새 test helper의 `StoredObject` deref를 통해 registration
children을 move한 E0507 컴파일 실패로 body0이었다(FAILED). 또한 큰 inventory의
contribution에는 occurrence ref 260개 대신 EntryMapBlock root 한 개만 연결된다.
Main oracle는 이 root의 인증된 positional children과 독립 260-key `QITXEM01`
golden을 비교하고 public 첫/마지막 selector reader를 유지하도록 교정했다.
제품 코드 불변·테스트2파일만 교정한 frozen `a60ac6576a0ca2cae306ee68a014b11c3736246f`의
f21은 Native 새 distinct-vector case1개 통과·853개 제외, Kernel14개 통과·65개 제외다.
f20 Native295개 통과·558개 제외와 합쳐 집중 composite local proof는 VERIFIED다.
정확 a60 소스의 전체 Native suite는 미실행이다. r7 정상 cached build는 완료됐지만
mr7 Medium은 536,869,205B 사용 후12,164B charge가 Catalog
`managed_install.rs:108`에서 fixed512MiB 진단 cap에 거절됐다. reads878,940/writes573,536/
mutations351,513, physical2147 writes·3,680,139B이며 I/O 오류는 없었다.
lo7 Large Off는 원120초 요청124.194초에서 Catalog `managed_install.rs:151` deadline으로
FAILED다(producer157.629초, full ACK 없음). reads15,477,290/writes11,733,540/
bytes11,341,756,074/mutations6,738,039이며 cap·memory·external 거절은 없었다.
Large Off 결과는 B2가 원120초 제품 목표를 해결하지 못했다는 증거다. Medium
fixed512MiB 실패는 별도 진단 결과다. seed42 투영
Medium512→256, Large12,288→4,096 block은 코드 기반 추정이며 설치 호출·CPU·wall·SLO
개선 실측이 아니다.

B3는 B2의 owner-local occurrence block이 성공적으로 공유되더라도 QITXEM01
inventory가 selector마다 같은 child ref를 반복 등록하는 중복을 겨냥한다. B2 r7
Medium/Off 실패만으로 이 중복이 남은 지배 비용이라고 단정할 수 없다. 새
QITXEM02는 정렬 row마다 고유 child slot을 저장하고 registration에는 first-use
고유 ref만 둔다. 기존 public key→ref mapping과 pin/GC authority는 보존한다.
Kernel 독립 fixture는 초기 증분에서 a/b의 기존 block을 재사용해10행/고유4 child,
중간 Tree 뒤 복귀할 때 새 leaf5개로10행/고유5 child가 되는 차이를 gold로 둔다.
정확 policy SHA는 old B2 `b2c1df18…` 재현 뒤 new `14c6d531…`로 산출했다.
19개 소유 입력/8개 변경의 external frozen `39f4a9c3bd82843aea2b390411d8ec531a0d3b2c`에서
f22 Native297개 통과·558개 제외(3.371초), Kernel14개 통과·65개 제외(114.062초)로
집중 회귀는 VERIFIED다. f23 lexical strict Clippy는 exit101/FAILED다. 전역38개
진단은 기존 타 owner33개와 Native 새 distinct-vector 테스트 helper5개(정수 나눗셈2,
산술 부작용3)로 분리된다. 테스트 helper 산술1파일만 고친
`996230439fbf187661c7462b018c113c8b3de1bd`의 f24 strict Clippy도 exit101/FAILED지만
진단33개는 모두 타 owner이고 Native 진단은0이다. f25 Native 변경 case1개는
통과·853개 제외/source guard0으로 새 소스의 기능 영향을 확인했다. f22 Native297개·
Kernel14개와 합친 집중 proof이며 Native298개 실행으로 합산하지 않는다. r8 정상 cached
build는 exit0이지만 mr8 Medium은 536,864,010B 사용 후9,988B charge가 Catalog
`managed_install.rs:108`에서 fixed512MiB 진단 cap에 거절됐다(producer15.888초·exit1).
reads892,017/writes446,724/mutations219,407, physical2364 writes·2,256,813B,
file sync2857/confirm2857(physical reuse493), barrier40·I/O 오류0이다. child trace는
49,370 events 완결·dropped/unwritten/open/lease0이다. lo8 Large Off도
producer159.174초·exit1, `full_event_ack`124.654초 미완료로 FAILED다.
`BuildPrepare`의 Storage `map/completed_group.rs:140` deadline이며 누적 사용은
15,381,374 reads/8,331,877 writes/9,933,434,999B/3,283,526 mutations이다.
numeric cap·memory·external resource 거절은 없었다. Medium의 512MiB와 달리
Large는 explicit 67M/512GiB 논리 funding으로 실행했다.

같은 `99623043…` binary pair의 lr8 Large Record도 full ACK125.721초 미완료,
producer185.890초·exit1로 FAILED다. 원인은 `BuildPrepare`의 Storage
`map/completed_group.rs:140` deadline이며 13,486,261R/7,747,424W/
9,054,086,583B/2,728,115M에서 numeric·memory·external 거절은 없었다.
물리31,630 writes·43,770,402B, I/O 오류0이다. child V5 footer는841,911 events,
dropped/unwritten/open/lease0으로 COMPLETE다. 이 수치는 제품 성공이나 정식
Off/Record overhead 비교가 아니다.

닫힌 child의 실패 LexicalBuild 한 span은 wall112.262초/CPU58.303초,
13,207,048R/7,643,573W/8,977,635,886B/2,645,991M이다. 선택된 하위 inclusive
단계는 Prepare wall20.516초/CPU11.430초·5,971,958,518B, Map
13.870초/6.490초·1,486,154,106B, Diff9.774초/7.388초·384,541,527B,
Postings51.408초/26.034초·1,093,627,763B다. 실패 Postings lane37 자체는
wall50.629초/CPU25.836초·1,093,365,205B다. ManagedObjectInstall interval을
중복 없이 합친 wall union은 실패 Lexical 안28.636초, lane37 안13.196초다.
Prepare lane34/35와 Diff는 각각4,096 owner span, Map은64 batch까지 도달했다.
Postings lane37은 실패한1 span이다. posting-edit17,949 span은 parent chain상
InventoryDiff 아래이며 lane37의 자식은0개다. 이 수를 첫64 posting map의 edit
수로 사용했던 해석을 철회한다. ManagedObjectInstall은 실패 Lexical 안230개
최외곽 span/CPU16.888초, lane37 안38개/CPU7.657초다. 두 집합 안에서 중첩된
ManagedObjectInstall은0개였다. 따라서 설치 interval 밖의 wall은 각각 약83.626초와
37.433초지만 원인을 단정할 수 없다. reserve31.524초·confirm55.870초는 처리 전체의
inclusive 집계이므로 위 union과 더하지 않는다. 상위·하위 inclusive 시간을 합산하거나
전체 wall에서 임의로 빼지 않는다.

**Postings51초의 직접 자식 재분해:** 실패 lane37의 wall50.629064초/CPU25.835966초
중 직접 자식 interval union은48.022905초이며 미계측 간격은2.606159초다.
아래 직접 자식끼리는 중첩하지 않는다. 표의 CPU가 없는 구간을 host wait로
치환하거나 wall−CPU를 전부 scheduler 경합으로 해석하지 않는다.

| lane37 직접 자식 | 수 | wall 초 | thread CPU 초 |
| --- | ---: | ---: | ---: |
| ManagedObjectConfirm | 38 | 23.641956 | 11.003578 |
| ManagedObjectInstall | 38 | 13.195895 | 7.657016 |
| ManagedObjectReserve | 38 | 9.816076 | 4.140055 |
| SqliteWork lane22 write envelope | 5,187 | 1.368979 | 미계측 |

38개 reserve/install/confirm triplet은 각각256 input slot을 처리했고 모두ok다.
이는9,728 registration input slot이며 고유 물리 write 수와 동일하지 않다.
confirm 내부 COMMIT은16.331305초/CPU6.000718초, reserve 내부 COMMIT은
2.975408초/CPU0.370181초다. 합계76 COMMIT19.306713초 중 checkpoint/backfill
lane25는39회10.571434초/CPU4.610249초이며 COMMIT에 포함된다. confirm의
preflight3.922416초, register/transfer2.969227초, protection/lifecycle0.383599초도
서로 다른 SQL phase다. reserve preflight는5.526690초, mutation은1.105664초다.
catalog lock **hold**33.303790초는 이 SQL을 감싼 시간이다. 실제 lock wait는
reserve0.153324초/confirm 약9µs로, hold를 lock contention으로 부르지 않는다.

lane37의 VFS lane22 요청량은118,343 xWrite calls/1,355,749,752B다. 직접
scratch envelope는10,176 calls/41,680,896B, Catalog confirm은103,095 calls/
1,170,958,816B, reserve는5,072 calls/143,110,040B다. checkpoint lane25의
87,055 main-DB writes/620,584,960B는 이 합계의 부분집합이다. 그중 confirm
38회 checkpoint는86,210 writes/615,071,744B이며 COMMIT 안에 들어 있다.
이들은 **VFS 요청** 계수다. 장치 물리 write, fsync 시간, WAL frame 수나 장기
reader 때문에 checkpoint가 반복됐다는 증거로 승격하지 않는다. 실제 I/O·admission
refusal 계수는0이다. `ManagedObjectInstall`의 object-file write 계수와 합치지 않는다.

같은76개 Catalog hold의 pager 전후 delta는 cache hits6,617,975/misses1,799,043/
writes168,321/spills0이다. 시작 시 읽어낸 설정은 page4,096B·cache49,152KiB
(48MiB)·autocheckpoint1,000pages다. miss는 cold read·eviction·B-tree access·host
paging을 구분하지 않는다. spill0과48MiB 설정만으로 cache 부족을 결론 내리거나
메모리 cap을 올리지 않는다.

따라서 현재 실측 주 대상은 catalog custody의 SQL·durable COMMIT과 physical
install이다. `completed_group.rs:140`은 이 작업 뒤 순회 중 deadline을 관측한
checkpoint이며 그 위치만으로 hash table 결함을 뜻하지 않는다. 기존 same-base
root dedup을 재구현하거나, 자동 checkpoint/FULL 내구성을 끄는 안은 제외한다.
빈 base의 CAS nibble 중복 변환과 known-size buffer 기하 재할당은 별도 소규모
소스 낭비로 확인해 제거했다. 이 두 수정의 영향 검증과 Large 개선량은 NOT_RUN이다.

추가 source 수리 B03-B4는 같은 transaction의 child proof window다. 개별 node의
child가64 이하면 전체 batch의 child가64를 넘더라도64개만 기억하던 capacity
shortcut을 제거했다. 기존의 total 기반 capacity와4,096 unique cap·포화 후 전체
재검증·원 grant를 그대로 사용한다. 어떤 실제 registration batch에 반복 ref가 몇
개인지는 기존 trace에 ref 식별자가 없어 산출하지 못했다. 단위 SQL oracle로64개
이후의 중복 인증 제거와 late corrupt/refusal rollback을 검증하고 제품 영향은 새
source에서 분리 판정한다. 큰 Posting Block 단순 확장은 query 전체 재검증과
증분 전체 재작성·4K 경계 cliff 때문에 NOT_SELECTED다.

lr8의 실행 중 periodic 자원 관측은 child RSS1,362표본/최대96,419,840B
(최대 표본 간격1,646.7ms), fixture allocated330표본/최대370,106,368B와 그 안의
scratch146,673,664B(최대 간격1,671ms·missing7)다.
aggregate process RSS·child process I/O는 이 Mac에서 unsupported이며 표본은 연속 peak를
보장하지 않는다. 따라서 E07 aggregate RSS gate는 NOT_RUN이다. disk traversal
observer wall35.230초, child RSS probe31.801초는
누적 관측 비용으로 request wall에서 빼거나 원인 비율로 해석하지 않는다.
`lr8/host-pressure-diagnostic.json`의 load37–38, swapin91,780/swapout40,124와
압축·해제 약105만 건은 **terminal 이후 17:45:19–29 UTC의 10초** 관측이다.
실행 중 host 병목의 직접 증거가 아니다. 실패 Lexical의 wall−CPU 약53.959초에는
대기·I/O·스케줄링이 섞여 있다. 이 local Record와 Off의 시간 차이로 tracing
overhead나 코드 speedup을 산출하지 않는다.

동일 seed42 Medium의 mr7→mr8 child trace에서 inclusive selected work bytes는
InventoryPrepare 265,135,902→265,135,902B(동일), InventoryMap
147,199,614→67,700,478B(−79,499,136), Diff28,293,989→16,305,857B(−11,988,132),
Postings69,624,440→180,638,964B(+111,014,524)다. LexicalBuild 전체는
532,363,153→532,358,034B(−5,119)다. 단계들은 inclusive이므로 합산하지 않는다.
물리 write bytes는 3,680,139→2,256,813B(−1,423,326)지만 write 횟수는
2147→2364(+217)다. 각 실행은 다른 시간·호스트 경합을 포함하므로 속도 개선율로
읽지 않는다. 관측상 물리 bytes는 줄었지만 제품 cap 실패는 해결되지 않았다.
신규 term의 `token_groups` live SELECT당4,096B는 코드 계약 대조에서 의도된
scratch page touch tariff로 확인됐다. 이 비용을 없애는 가격 변경은 하지 않는다.
Medium fixed cap의 누적 기여와 Large deadline의 지배 원인을 동일시할 근거도 없다.

중복 map build 제거를 새 B4로 추가하는 안은 제외한다. Storage
`map.rs:1880–2080`은 동일 base·edit count·fingerprint의 후보 root를 이미 공유하고,
재사용 전 양쪽 scratch cursor를 EOF까지 검증한다. 같은 최적화를 다시 구현해도
Large `BuildPrepare`의 확인된 신규 비용 단위를 제거하지 못한다.

Query 예산은 별도로 소스 점검했다. 실제 bench Text `read_need`는 64MiB이고,
64-slot `CheckedBlock`의 20,512B layout 예약과 `PreparedRows` backing을 함께
계산하면 2,625,536B다. 최대 payload 524,288B와 작은 `StoredObject` overhead를
합쳐 약 3.16MB charge이며 성장 peak는 steady보다 크지 않다. 이전의 약 1.8MB
추정은 layout과 backing의 동시 funding을 빠뜨렸다. Kernel은 registry ceiling과
held reservation을 대조하고 SDK는 need를 override하지 않는다. 4MiB 테스트 helper는
제품 sizing 증거가 아니며 더 작은 custom need는 `ResourceLimited`로 거절될 수 있다.
이 분석은 메모리 계약 적합성의 코드 산술로, 실제 RSS·query latency·retained-read
수용은 r7의 Medium/Off 실패만으로 확인되지 않았다.

## 2026-11-06 재감사 판정

확정된 직접 실패는 C17·51b70b34·0a97e858 Large의 단일 SourceAdvance dispatch120초 소진이다.
코드에서 확인한 구조적 비용은 작은 불변 객체의 물리 파일 단위 영속화와 여러 단계의
검증·map/SQL 처리다. 이것만으로 전체 지연의 지배율이나120초 불가능을 확정하지 않는다.
51b70b34의 원 managed 빌드는 terminal/receipt 없이 중단됐다. 해당 프로세스와
log FD 부재를 확인한 뒤 같은 target의 normal cached build를 재개해 exit0으로 마쳤다.
mr2와 최신 mr4는 모두512MiB 논리 work cap에서 실패했다. 같은 r4 pair의 원 Large
Off/Record도 dispatch120초 deadline으로 실패했다. 최신 결과는 바로 아래에 기록하고,
이후 mr2/lo2/lr2·C17 절은 해당 시점의 역사적 근거로 읽는다.

| 분류 | 확인한 내용 | 판단 한계 |
| --- | --- | --- |
| 직접 원인 | C17 SourceApply18.419초+완료하지 못한 Lexical100.595초 | 실패 prefix이며 seal/commit까지 끝난 전체 시간이 아님 |
| 구조적 증폭 | 배치 안에서도 새 객체마다 파일 생성·쓰기·file fsync·게시. directory sync/barrier만 공유 | API256 batch를 물리256객체의 단일 쓰기로 해석하지 않음 |
| 입력 규모 | seed42 Large4,096파일/8,609,531B. Medium256파일/367,590B | 파일 수16배, bytes23.42배. 동일 작업당 선형 시간은 미입증 |
| 호스트 조건 | Lexical wall100.629초, thread CPU29.479초, 차이71.150초 | scheduler·동기 I/O·blocking을 분해하지 못해 호스트만의 시간으로 부를 수 없음 |
| 부수 수리 | B02 계수·context mask·Python 검증기와 해당 테스트 | 관측·거절 정확도 개선이며 Large 속도 개선 증거가 아님 |
| 수용 공백 | 최신 source-bound Large120 ACK/search/reopen 없음 | 과거 다른 소스의 성공·테스트 수로 대체 불가 |

### 최신 r4 고정본: 정상 빌드와 세 실패 실행

외부 `~/.codex/qib6-_0r6mac7/s3/`의 고정 source는
`0a97e8589daa41b736cb6d59545b0116bb3b89c6`다. `r4/terminal.json`은
normal cached resume build exit0과 Catalog 집중37개 통과를 기록한다.
이 빌드는 managed-fresh가 아니다. 실행 세 개의 source, scale binary, daemon binary는
각 실행 전후 동일했다. 그러나 `l4ctl/m5ctl/l5ctl/terminal.json`의 build/capture
filtered environment에는 `PATH` 한 key의 drift가 있으며 `build_binding=UNPROVEN`이다.
첫 `m4ctl` 시도는 이 drift에서 body0으로 거절됐고 `mr4`를 만들지 않았다. 후속 세
실행은 실제 producer body를 실행했지만 source/pair 일치와 제품·canonical 수용은
서로 다른 판정이다. CLI exit2는 실패 producer/capture 자격 부족을 나타내며 빌드 실패로
해석하지 않는다.

| 실행 | 제품 상태와 경과 | 직접 거절 | raw/종료 근거 |
| --- | --- | --- | --- |
| `mr4` Medium fixed Record | producer exit1, outer timeout=false, 12.356839초; product `UNVERIFIED` | bytes cap512MiB: used536,865,857B, 다음10,600B 거절 | `mr4/execution.json`, `mr4/causal-timeline.jsonl.native-medium-children/first.jsonl`, `first.stderr` |
| `lo4` 원 Large Off | producer exit1, outer timeout=false, 145.286325초; product `UNVERIFIED` | `first_denial=deadline`, `failed_stage=BuildPrepare`; checkpoint는 `text.rs`의 token/key 처리 | `lo4/execution.json`, `lo4/stderr` |
| `lr4` 원 Large Record | producer exit1, outer timeout=false, 146.810698초; product `UNVERIFIED` | `first_denial=deadline`, `failed_stage=BuildPrepare`; checkpoint는 `managed_install::confirm`의 lane8 packet loop | `lr4/execution.json`, `lr4/causal-timeline.jsonl.native-large-children/first.jsonl`, `first.stderr` |

원 Large의 명시 한도는 reads/writes/mutations 각각67,108,864,
bytes549,755,813,888이고 단일 dispatch120초다. `lo4`의 거절 시 usage는
10,865,924R/6,884,981W/6,577,808,745B/4,331,086M,
`lr4`는11,227,154R/7,126,249W/6,791,074,318B/4,509,605M이다.
두 실행 모두 숫자 한도·memory·external resource 거절이 아니다. Off도 실패하므로
tracing만 제거해 해결되는 조건은 아니다. 두 실행은 도달 prefix가 다르므로
145.286/146.811 비율을 tracing overhead로 해석하지 않는다.

`mr4`의 닫힌 child는 V5 56,184 events, complete, dropped/unwritten/open/lease/health/
orphan0이다. WorkBudget의4계수는 **accepted logical work의 inclusive delta**이며
실패한10,600B charge와 물리 I/O는 포함하지 않는다. LexicalBuild lane2의2 span은
945,790R/582,069W/532,359,833B/396,516M, wall6.718217초/CPU4.884094초다.
SourceApply lane3은10,686R/3,387W/3,991,727B/3,608M,
wall0.304436초/CPU0.165732초다. Lexical의 bytes는 최종 used의99.16%다.
Prepare lane2 260개는261,371,848B, MapApply lane2 4개는148,640,926B,
Diff lane2 256개는29,935,047B, Postings lane2는70,301,512B다.
오류는 Postings lane37→Postings lane2→LexicalBuild→BuildPrepare로 전파됐다.
lane38/39는 미도달이다. 이 하위 계수는 Lexical 안에 중첩되므로 더하지 않는다.
물리 진단은5,230 writes/3,298,069B, failed confirm1,
write/fsync/directory-sync/barrier 실패0이다. 같은 cap 실패가 남아 있어
File get hash와 Catalog registration encode 수리가 제품 조건을 닫았다고 볼 수 없다.

`lr4`의 닫힌 first child는 V5 637,289 events, complete,
SHA256 `171b3978decc9fa29faac4a0ff6b247ed939e582f7388fd84572a21834002b04`,
dropped/unwritten/open/lease/health/orphan0이다. 실패 request wall120.152688초,
LexicalBuild lane2 2 span은 wall114.628386초/호출 thread CPU66.000876초,
accepted work10,947,993R/7,022,413W/6,714,625,437B/4,427,495M이다.
SourceApply lane3은 wall5.107407초/CPU3.680325초,
accepted work168,467R/52,751W/68,528,213B/56,564M이다.

| `lr4` Lexical 하위 | span/결과 | Inclusive wall 초 | 호출 thread CPU 초 | accepted work bytes |
| --- | ---: | ---: | ---: | ---: |
| InventoryPrepare lane2 | 2,730 ok | 45.074986 | 24.980680 | 3,754,808,912 |
| InventoryMapApply lane2 | 41 ok/1 error | 45.413534 | 26.987565 | 2,165,304,826 |
| InventoryDiff lane2 | 2,624 ok | 12.218451 | 9.881738 | 486,786,729 |
| Postings lane2/35–39 | 미도달 | 0 | 0 | 관측 span 없음 |

같은 순서의 나머지 accepted work는 Prepare
4,521,661R/3,150,630W/1,319,433M, MapApply
4,119,832R/3,050,320W/3,047,632M, Diff
610,726R/757,280W/0M이다. 모든 해당 span에4개 계수가 존재한다.
실패 ancestor는 ManagedObjectConfirm lane3→InventoryMapApply lane2→LexicalBuild→
BuildPrepare→request다. 마지막 confirm은 wall0.526133초이며 deadline 검출 위치가
전체 지연의 지배 함수라는 뜻은 아니다. child의 물리 진단은39,288 writes/42,915,388B,
failed confirm1, write/fsync/directory-sync/barrier 실패0이다. Lexical
wall−호출 스레드 CPU는48.627510초로 동기 I/O·blocking·scheduler 대기가 섞일 수 있다.
반면 Prepare45.075초/MapApply45.414초의 실제 실행 경로와 CPU도 확인됐다.
코드 단독 또는 호스트 단독 원인, 단계별 exclusive 기여율은 이 trace로 확정하지 않는다.
각 stage의 span·work delta는 상위 Lexical에 포함되고 sibling 집합도 합산하지 않는다.
완전한 진단 child는 실패 prefix의 증거이며 full ACK/search/reopen이나 정식
Off/Record 비교 성공의 증거가 아니다.

### 새 mr2: 시간 실패와 논리 작업량 실패의 분리

고정본 `51b70b3470ecbb58beebf757831eb53db85aade6`의 정상 cached bench pair는
Cargo exit0이다. 외부 `qib6-_0r6mac7/r2/normal-build-result.json`이 결과를 기록한다.
빌드의 admission758.451초와 compile39분39초는 제품 dispatch120초의 일부가 아니다.
원 managed-fresh 실행의 중단 기록과 prebuilt build_binding=UNPROVEN은 보존한다.

`mr2/execution.json`은 Medium fixed Record가23.202859초에 producer exit1로 끝났음을
기록한다. 외부 timeout이 아니며 full SourceAdvance가 BuildPrepare/NativeTextPostings에서
`Terminal(Refused(ResourceLimited))`가 됐다. reads963,417/writes588,655/
mutations401,727/bytes536,865,857을 사용한 뒤 bytes10,600 청구가 거절됐다.
남은5,055B보다 큰 청구였으며 거절된10,600B는 used에 더하지 않는다.

거절 위치 `Catalog::managed_install::charge_object`는 **마지막 관측 지점**이다.
해당 객체의 canonical registration JSON 길이 J와 child 수 E에 대해 각 reserve/confirm이
`4J+256E`를 청구한다. compact 저장 codec과 별개로 보수적 논리 work를 유지하는 계약과
독립 fee oracle가 있다. 이 위치만으로512MiB를 Catalog가 모두 썼다거나 잘못된 중복
과금이라고 판단하지 않는다. 객체별 row/64개 SQL packet과 최대256개 설치 batch를
확인했으며 이 경로의 전체 SourceAdvance 재직렬화·전체 operation 반복 protect는 찾지 못했다.

추가 증폭 후보는 TextSpool의 term_page 반환 term당4,096B와 confirmed-posting SQL
probe당4,096B, Prepare token_groups의 source별 신규 term당4,096B다. map scratch는
64-row query/INSERT당 기본 비용이므로 이를 term당4KiB와 섞지 않는다. 모든 비용은
요청 전체의 누적 work이며 실제 쓰기3,298,069B와 나눠 disk write amplification이라
부를 수 없다. stage별 work delta 또는 청구원 ledger 없이 주범 비율은 미확정이다.

first child V5 failure trace는71,097 events, loss/health/open/lease0이다. Diff256개는
요청18의 full build prefix에 속한다. prior0/next3,083/RAM13,952/spill34,091/equal0/
validation failure0이다. 재사용 반환48,043회와 실제 next 검증3,083회를 관측했고,
OccurrenceRead lane31은wall1.473913초/CPU0.616303초다. nested inclusive 시간은
합산하지 않는다. 이 Medium 실패에서 occurrence 재검증이 주원인이라는 근거는 없다.

**실행 순서 정정:** fixed Medium512MiB 성공은 original Large 실행의 제품 조건이 아니다.
Large는 원래 explicit reads/writes/mutations67,108,864·bytes549,755,813,888와 dispatch120초다.
mr2 실패를 유지하고 동일 binary의 Large Off를 독립 판정한다. Medium 재funding으로
실패를 덮거나 ANN4K의1M/512MiB·SDK30초/worker10초를 완화하지 않는다.
과거 C16/C17 Medium은 문서상 explicit funding이므로 원 숫자가 부재한 지금의 fixed
mr2와 직접 속도·비용 회귀 비교를 하지 않는다.

### 새 lo2: tracing Off에서도 원120초 실패 재현

같은 source/pair/filtered environment의 Large Off는 producer exit1, outer timeout=false,
elapsed178.514856초다. source와 양쪽 binary는 전후 동일하다. Full source staging은
33.003335초로 별도 요청이며 full_event_ack 관측129.481174초에는 서버 dispatch와
통신·상태확인·정리 시간이 포함된다. 이를 서버 deadline129초라고 해석하지 않는다.

직접 거절은 `first_denial=deadline`, `failed_stage=BuildPrepare`다. 당시 누적 work는
reads7,914,339/writes4,979,851/bytes4,715,789,654/mutations3,163,350이며 원 한도
67,108,864/67,108,864/549,755,813,888/67,108,864보다 작다.
denied_charge/denied_memory/external_resource_refusal은 없다. 거절 checkpoint는
managed_install.rs:817이며 그 함수가 전체 지연을 만들었다는 뜻은 아니다.

이 실행은 trace Off에서 실패했으므로 tracing overhead만 제거해 해결되는 실패가 아니다.
Off에는 Text 세부 phase가 없어 mr2의 작은 입력만으로 Large 실패 구간을 확정하지 않는다.
당시 계획된 동일 pair의 Large Record1회(`lr2`)로 단계·실패 ancestor를 진단했다. 두 실패
prefix의 도달 작업이 다르면 Off/Record elapsed 비율을 overhead라고 보고하지 않는다.

mr2의 세부 실패는 첫 changed-term page64개의 lane37 `entry_patch::apply`다.
lane35 두 span·lane36 한 span은 성공했고 wrapper prep/put/top-map인 lane38/39는
미도달이다. lane37은wall2.005342초/CPU0.930751초이며 상위 Postings2.006600초에
포함된다. 설치 packet4개가 각각256개의 물리 객체를 성공적으로 설치했고4번째
Catalog confirm이 누적 work bytes에서 거절됐다. 해당 물리 phase의 관측 실패는0이다.
이를 wrapper 또는 최종 top-map 비용이라고 부르지 않는다.

**mr2 당시 관측 공백:** 그 source의 SourceApply/Text 시간 span은 stage별 WorkUsage
delta를 발행하지 않았다. mr2의 work_bytes 한 건도 이전 요청의 member assembly에
속했다. 현재 `0a97e858`에서는 기존 span에4개 accepted delta를 발행하며,
위 `mr4/lr4` 첫 child에서 모두 관측했다. 부모 inclusive와 자식 delta를 합산하거나
실패한 청구를 accepted delta에 넣지 않는다. 계측은 성능 수리 자체가 아니다.

### 새 lr2와 실행 직후 호스트 관측

동일 pair의 Large Record도174.409383초에 producer exit1로 끝났으며 outer timeout은
없다. BuildPrepare의 deadline이고 work bytes2,635,718,937, denied charge/memory/
external resource refusal은 없다. source/pair 전후 동일이다. Off의 bytes4,715,789,654와
도달 작업이 다르므로174.409/178.515 비율을 tracing overhead로 해석하지 않는다.

child 전체의 quiescent 물리 진단은23,273 writes/23,073,002B, file sync23,273,
directory sync17,868, barrier257, reserve257/confirm257이며 failed confirm1이다.
물리 write/file sync/directory sync/barrier 실패는0이다. 누적 시간은 reserve26.945초,
confirm40.820초, write2.316초, file sync7.197초, directory sync0.882초, barrier6.192초다.
이는 child 전체 누적값이며 단계별·exclusive 시간 또는 syscall 기여율로 바로 합산하지 않는다.

실행 종료34초 뒤02:41:58–02:42:00 KST의 bounded `top -l 2 -s 1 -n 0` 관측에서
16CPU 호스트 load average53.19, CPU idle0–0.4%, physical memory47GiB 사용,
compressor24GiB와 unused117MiB를 확인했다. 두 번째 interval의 swap-in36,258/
swap-out34,420 pages도 관측했다. **호스트 경합과 메모리 압박은 실제 관측 사실**이다.
이 짧은 실행 직후 샘플로 요청 전체의 scheduler/I/O 대기 비율이나 조용한 호스트의
완료 시간을 계산하지 않는다. 코드가 효율적이라는 증명도 아니다. 동일 소스·고부하
조건에서 무변경 Large를 반복하지 않고 기존 failure trace로 다음 수리 owner를 고른다.

### lr2 오프라인 분석: 실제 도달 경로와 다음 코드 범위

기존 V5 parser로 닫힌 first child를 검증했다.451,717 events, complete,
health/drop/unwritten/open/lease0이다. 실패 요청은 id168/control/connection169,
wall120.097193초이며 BuildPrepare→LexicalBuild→InventoryMapApply lane2→
ManagedObjectConfirm(64 inputs)에서 deadline을 관측했다. trace의 완전성과 제품 성공은
다르다. full ACK/search/reopen은 미도달이고 canonical product 검증도 없다.

| 실패 요청의 관측 범위 | span 수 | Inclusive wall 초 | 호출 thread CPU 초 |
| --- | --- | --- | --- |
| SourceApply lane3 | 1 | 24.864602 | 6.292227 |
| LexicalBuild | 1 | 94.511617 | 32.011491 |
| InventoryPrepare lane2 | 1,040 | 40.306306 | 14.048999 |
| InventoryMapApply lane2 | 16(15ok/1error) | 32.479142 | 9.941968 |
| InventoryDiff lane2 | 960(all ok) | 16.059402 | 6.148245 |
| OccurrenceRead lane31 | 11,087 | 8.603131 | 3.782397 |

각 행은 해당 span의 inclusive 합이다. 부모·자식 또는 nested 같은 stage의 시간과
CPU를 합산해120초 기여율을 만들지 않는다. 특히 OccurrenceRead는 Diff 안에 있다.
최종 오류 MapApply는0.635719초이며 그 마지막64-input confirm은0.034997초다.
거절이 그 지점에서 감지됐다는 사실만으로 마지막 confirm이 지배 병목인 것은 아니다.

Diff 계수는 prior0/next11,087/RAM59,456/spill198,293/equal0/failure0이다.
재사용 반환257,749회가 이미 존재한다. 완료된 owner별 inventory 항목 수와 실제
토큰 그룹 방문 수는 동일하지 않다. 같은 body의 inventory reuse는 token traversal을
건너뛸 수 있으므로 계수 합계에4KiB를 곱해 token-group 비용 하한을 계산하지 않는다.

설치 aggregate의 write phase 호출은 가장 가까운 Text 조상별로 다음과 같다.

| Text 조상 | 관측 packet 수 | write phase 호출 수 |
| --- | --- | --- |
| InventoryPrepare lane2 | 16 | 2,203 |
| InventoryPrepare lane35 | 37 | 9,472 |
| InventoryMapApply lane2 | 16 | 1,024 |
| InventoryDiff lane2 | 0 | 0 |
| LexicalBuild 직접 하위 | 15 | 960 |

이것은 도달한 설치 phase의 호출 수이며 고유 ref·성공 게시 파일 수·device write 수·
단계별 실제 bytes delta가 아니다. 위 source-bound 호출 대조상 Prepare subtree의
설치 생산자는 OccurrenceWrites::flush이므로11,675개 write phase 호출은 그 경로에
귀속된다. 포맷 후보의 이득이나 query 비용을 검증하지 않고 전부 제거 가능하다고
계산하지 않는다. nearest Text별 reserve/confirm input 합은 미산출로 남긴다.

**lr2 당시 코드 후보 결정:** B03-A count 전달은 우선 수리에서 보류한다. B03-B owner별 bounded
occurrence block의 최소 codec/producer/reader·독립 fixture를 먼저 검토할 근거는 확보했다.
단 전체 포맷 채택은 exact payload dedup 손실·query read bytes·증분 locality를 반증하는
기존 계약을 통과해야 한다. Posting Block은 이 Large 실패가 Postings 이전이므로 우선
패킷에서 제외한다. Catalog의 기존 transaction-local child proof/allowance 수리를
같은 의미로 다시 구현하지 않는다.

B03-B의 추가 계약 결손도 보완한다. 현재 physical packet은 여러 file/ordinal을 담는다.
`(ref,canonical stored term)` selector를 쓰는 block에 서로 다른 owner의 동일 term과
다른 positions를 같이 넣으면 선택이 모호하다. **block 내용은 한 owner의 logical
inventory로 제한**하고 완성된 block들만 기존 물리 put_batch에서 함께 설치한다.
동일 bytes의 정확한 block alias는 허용하지만 cross-owner mixed-content block은 별도
selector/권위 계약 없이 허용하지 않는다. 이 보완은 설계 계약이다.

### 후속 코드 수리와 B03-B 계약 재검토

B03-B를 구현 전에 producer/reader/독립 oracle로 대조했다. 현재 FileObjectPort::get은
객체 전체를 읽고 wire와 decoded payload bytes를 청구한다. 두 term을 block으로 묶으면
cold 단일-term 조회에서도 다른 term의 payload를 읽는다. query-local cache는 첫 read를
없애지 못한다. 기존 leaf는 positions만으로 ref를 만들기 때문에 term/owner/surface 간
exact payload alias도 존재한다. block에 term key를 넣으면 이를 잃을 수 있다.

증분에서도 `previous == identified_leaf_ref`는 prior block과 항상 달라 동일 positions를
변경으로 오판한다. 이를 고치려면 prior block 전체를 term별로 인증·선택해 비교해야 한다.
이 결손은 포맷을 바로 제품 개선으로 인정하지 못하게 한다. 다만 **모든 cold 단일-term
read bytes가 단1B라도 증가하면 영구 반려**라는 조건은 원 제품 계약이 아니라 이 RCA가
자기 부과한 과도한 기준이었다. [원 RFC](../../rfc/oct-10-native-incremental-vnext.md)의
metric별 선고정 region bound와 고정 query mix/SLO, E07 warm lexical top20
p95≤100ms/p99≤500ms, 기존 work/memory budget이 채택 기준이다. cold read 증가는
측정·보고하고 해당 한도와 latency 안에서 판단한다. 결과를 본 뒤 threshold를 만드는
일은 허용하지 않는다. 원 Large120초, Medium fixed512MiB 진단, ANN4K의 별도
1M reads/512MiB·SDK30초/worker10초를 혼합하지 않는다. Large·ANN gate는 완화하지 않는다.

재검토된 B03-B1은 **새 current policy** 안에서 owner-local term→positions-vector
dictionary(최대4,096B/128 term rows)를 구현했다. 동일 positions는 block 내부
vector를 공유하고 singleton/oversize/변경된 delta term은 leaf로 둔다. `(ref,term)` count 선택은
완전 인증된 bounded layout에 묶고 ref-only scalar memo는 leaf에만 사용한다.
구 policy의 silent fallback/rewrite/drop은 없으며, 새 policy 안의 old/current
generation·reopen·pin을 보존해야 한다. 실제 구 refs/pins이 남은 cutover는 호환 실행
artifact를 보존하거나 거절해야 한다. Codec·producer·독립 oracle 합류 뒤 f15
Native292개와 기존 Kernel lifecycle12개, f17 변경 transition1개의 composite local
proof는 VERIFIED다. 같은 exact source의 전체 Kernel13은 NOT_RUN이다. 새 정상
cached pair build는 VERIFIED지만 mr5 Medium과 lo5 Large Off는 각각 FAILED다.
집중 body만으로 채택을 선언하지 않는다.

실제 코드에서는 다음 두 결손을 수리했다.

1. **File get의 같은 wire 이중 SHA:** `read_io::ReadWire`는 해당 module의 private
   bytes/reference를 가지며 길이·read_exact·EOF·digest 확인이 끝나야만 생성된다.
   FileObjectStore/FileObjectReader의 get은 이 reference-bound 값을 소비해 공통 body
   decoder로 간다. scratch/resident의 raw decode는 길이와 SHA를 유지한다. namespace,
   FD owner, allocation lease, work charge와 cancellation 순서는 보존한다. 파일 read의
   동일 wire hash는2회에서1회로 줄지만 Large 시간 절감률은 아직 미측정이다.
2. **인덱싱 단계의 work delta 누락:** Core WorkBudget의 공통 helper가 기존 WorkReads/
   WorkWrites/WorkBytes/WorkMutations를4개 모두 accepted usage 차이로 기록한다.
   underflow는 health만 남기고 부분 family를 발행하지 않는다. Kernel WorkStage가 이
   helper를 재사용하고 SourceApply가 WorkStage를 사용한다. Text는 LexicalBuild와
   Prepare/Map/Diff/Postings/Root의 owner lane2, Postings의 bounded35–39 step에 연결한다.
   per-object read/edit span에는 추가4계수를 붙이지 않는다. Off 직접 실행·원 product
   결과·grant를 보존하며 실패한 청구는 delta에 넣지 않는다. nested delta는 inclusive다.

실제 두 File get 경로에서 metadata 확인 뒤 memory-admission hook으로 동일 inode를
truncate/append하는4가지 분기를 결정적으로 검증했다. 이 테스트에서 처음 나온
unused-results compile 오류2개는 수정했고, 그 후 집중 배치는 다음처럼 통과했다.

- VERIFIED: `./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-core
  -p quanta-index-storage -p quanta-index-lexical --lib -E 'package(=quanta-index-core) |
  package(=quanta-index-storage) | (package(=quanta-index-lexical) & test(native::text))'
  --test-threads 2` →741 passed/602 excluded. 신규 Core5개와 실제 File short-read/EOF,
  기존 Native Text/손상/namespace/취소·메모리 oracle를 포함한다.
- VERIFIED: Kernel lib `cargow --lane test-workspace-lane check -p quanta-index-kernel
  --lib --locked --offline`; SourceApply 연결 컴파일까지다.
- VERIFIED: Hexagonal boundary와 Core/Kernel module snapshot, 영향 rustfmt/diff 검사.
- NOT_RUN: 강화한 Kernel source_trace integration body와 새 정상 pair의 실제 UDS4계수.
  해당 integration의 SDK→Lance dev graph는 이 test cache에 없어 독립 fresh 재빌드를
  시작하지 않고 정상 pair 큐와 분리해 남긴다. 기존 strict 전체 FAILED도 승격하지 않는다.

당시 소유 소스12개를 합류한 `86e6130e`의 `r3` 실행 계획은 이력이다. 이후
`0a97e858`의 `r4` normal cached pair가 exit0으로 끝났고 동일 binary의 `mr4/lo4/lr4`
제품 body도 실행됐다. 결과는 위와 같이 모두 실패이며 managed-fresh·canonical
qualification은 성립하지 않는다. 과거51b 실패를 새 patch의 결과로 바꾸지 않는다.

추가 가설인 inventory Block child dictionary는 기존 positional child ref의 중복을
줄일 수 있으나 현 Catalog는 SQL edge INSERT 전에 이미 sort/dedup한다. row 수만큼
edge SQL이 감소한다고 계산하면 안 된다. 새 codec/policy·old pin 계약과 기존 P1-A
소유 파일을 건드리므로 이번 수리와 병렬 구현하지 않는다.

### 단순 읽기 최적화만으로 닫을 수 없는 이유

C17 occurrence read lane31의 도달 span은 wall10.442초/CPU3.925초다.
그 값도 prior와 신규 검증을 함께 포함한다. 신규 validated-count 전달만 고쳐 전체
100.629초 Lexical 문제를 해결한다고 말할 근거가 없다. 이 수치는 전체4K 완료 효과의
상한이나 ETA가 아니라 관측된 prefix의 직접 읽기 비용이다.

Medium Prepare의 신규 객체3,083개는 source당 약12.04개이고 SQLite requested
write23,096,256B는 원문367,590B의 약62.83배다. 이는 해당 phase의 작업 증폭이지
device write amplification이나 최종 디스크 배율이 아니다. 객체 단위를 줄이는 축이
namespace/create·file sync·Catalog/GC 반복을 함께 건드릴 수 있다는 설계 근거다.
durability를 낮추거나 이미 검증된 payload인 척하는 변경의 근거는 아니다.

### 최신 코드와 과거 측정을 혼동하지 않는 결정

현재 EntryMap은 Small/Block/Tree를 갖고, Block은 최대512개 행을 별도 payload/key
상한 안에서 묶는다. layout은 Text policy digest에 포함된다. 이미 공유 트리에 있는
이 변경을 다시 구현하지 않으며 C17 수치로 그 개선 효과를 인정하지도 않는다.
Block은 inventory map 객체 수를 줄이는 축이고 term occurrence의 파일 단위 저장을
모두 없애는 구조는 아니다.

`r4`의 work delta와 물리 진단을 기준으로 occurrence 저장 단위 수리를 먼저 검토한다.
남은 비용이 occurrence 재검증이면 저장 권위와 수명을 가진 count 결과 전달을 검토한다.
전역 Storage container 개편 전에 owner-local Lexical occurrence positions block의
효과를 원 budget/latency와 독립 oracle로 측정한다.
Storage API를 유지할 수 있지만 ref-only count 캐시, 증분 ref fanout, query의 반복 block
읽기와 기존 positions dedup 손실을 함께 해결해야 한다. 이 비용이 개선을 상쇄하면 폐기한다.
구체적인 선택·소유·검증·중단 조건은 [확정 실행 순서](WORKPLAN.md#execution-plan)와
[B03-B](WORKPLAN.md#b03-b)가 소유한다. 전역 저장 포맷/GC 재설계는 기본안에서 제외한다.

현재 Native 경로의 object-kind별 물리 census는 미관측이다. legacy LexicalStore의
kind aggregate를 Native로 가져오지 않는다. 51b70b34와 동일한 현재 Text 호출 경로에서는
Prepare subtree의 설치가 OccurrenceWrites::flush의 put_batch로만 이어지고 inventory
map 설치는 별도 sibling이다. 이 고정본에서 관측한 Prepare의 physical packet은
occurrence 설치로 귀속할 수 있으므로 이를 위한 추가 trace scope는 불필요하다.
옛 C17 source byte는 부재하므로 이 증명을 C17 전체 계수의 새 해석으로 소급하지 않는다.

### 해결이 지연된 진행 방식의 원인

1. 최근 B02 패킷은 계측과 검증기 결함을 닫았지만 큰 비용을 제거할 B03 수리를
   선택하지 않은 채 조건부 상태로 남겼다. 회귀 통과를 제품 성능 해결 진척과 구분해야 했다.
2. C16/C17의 도달 작업과 호스트 조건이 달라 인과 비교를 닫지 못했다.
   이후 원본 source/binary/trace 캐시도 없어져 과거 비교를 그대로 재개할 수 없게 됐다.
3. 새 고정본의 중단된 fresh 빌드와 정상 cached 재개에 긴 시간이 들었다. 현재 build는
   완료됐으며 이는 제품 인덱싱120초와 별개인 검증 소요 시간이다.
4. 마지막 좁은 테스트를 긴 managed build 뒤의 같은 admission 큐에 넣어300초
   입장 대기 제한으로 종료124가 됐다. 테스트 본문은0개 실행됐으며 이는 인덱서 실패가
   아니라 검증 실행 순서의 오류다. 같은 대기를 반복 실행하지 않는다.

추가 계측이나 새 티켓을 늘리지 않고 완료된 r4 failure trace로 수리 범위를 제한한다.
기존 build를 반복하거나120초·work/durability 계약을 완화하지 않는다.

## 실패를 발생시킨 조건

Large는4,096개 소스의 full SourceAdvance다. 소스 staging은 별도 요청으로 먼저 끝난다.
서버는 queue admission을 끝낸 뒤120초 dispatch 예산을 만들며 요청 저장·실행 입장, SourceApply,
CoreRequired Text build, seal, publication commit이 그 예산을 공유한다.
클라이언트600초와 capture900초는 서버 예산을 늘리지 않는다.
제품 queue wait10초는 dispatch 예산 생성 전의 별도 한도이며 C17 BuildPrepare 실패와
현재 Cargo admission300초 대기를 서로 합산하지 않는다.

C17 Record는 SourceApply18.418817초 뒤 실패한 Lexical100.594692초를 실행했다.
두 직렬 단계가119.013509초를 사용했고 다음 설치 예약의 checkpoint에서 deadline이
관측됐다. 거절 감지 위치를 해당 예약 함수의 성능 병목으로 해석할 수 없다.

실패 시 논리 work는 reads3,439,760, writes2,105,887, bytes1,981,181,324,
mutations1,343,714였다. 각 cap은67,108,864,67,108,864,549,755,813,888,
67,108,864다. 관측된 물리 write/sync/barrier 실패는0이다.
이 카운터는 서로 다른 계수 단위이며 OS syscall 수나 RSS로 변환할 수 없다.

근거는 [C17 종료 기록](../oct-22-finalizing/VALIDATION.md#e07-fixed-work-decision)과
[실제 dispatch](../../../crates/quanta-index-searchd-runtime/src/native.rs),
[SourceAdvance orchestration](../../../crates/quanta-index-kernel/src/update.rs)이다.

## 비용 분해

| C17 Large의 도달 범위 | Wall 초 | 호출 스레드 CPU 초 |
| --- | ---: | ---: |
| SourceApply | 18.419 | 6.843 |
| Lexical, configure를 포함한2개 span | 100.629 | 29.479 |
| Prepare의 토큰화+SQL, lane34의768개 child | 1.079 | 0.433 |
| Prepare의 grouped read/occurrence, lane35의768개 child | 33.807 | 10.574 |
| Prepare namespace/create, 신규 쓰기9,374개 | 14.745 | 6.450 |
| Map external-child validation | 10.917 | 3.322 |
| occurrence read, lane31의8,799회 get | 10.442 | 3.925 |

Prepare·Map·Diff와 그 하위 비용은 Lexical에 중첩된다. 표의 행을 합산할 수 없다.
768개 child는 완료한 전체4K의 비율이 아니다. 8,799회 get은 신규·기존 객체 읽기가
공유하는 lane이며 신규 객체 재읽기만의 수로 확정하지 않는다.

실패 전230개 physical install packet과20,390개 입력이 성공했다.
전체 capture는418,380events,41,836,325B이고 loss/health/orphan/open span/lease가0이다.
이것은 정상적으로 관측된 실패 prefix의 증거다.

기록에 남은 외부 원본 투영 경로는
`~/Library/Caches/quanta-index/audits/nov03-e07-wal-policy/record-analysis-c17ir1-physical.json`이다.
C17 실행 소스는 `8e94ca5c0f5200f2f923fd237b9e4d8a83d059b4`였다.

## 구조적으로 비용이 커지는 경로

### 작은 객체의 영속화

Text는 파일별 term occurrence와 inventory/map을 불변 객체로 만든다.
고유 occurrence payload를 한 packet에 모아 `AttemptContext::put_batch`로 설치한다.
Storage는 객체별 namespace/currentness 확인, 임시 파일 생성, 쓰기, file sync,
게시와 retirement를 수행한 뒤 디렉터리 sync와 배치 barrier를 수행한다.

따라서256개를 한 API 호출로 전달해도256개 파일의 처리 비용은 발생할 수 있다.
단위 객체 수는 경로 검사와 inode·Catalog·sync 비용을 증폭한다.
필요한 내구성과 인증을 만족시키는 표현의 크기와 검증 결과 전달 방식은 설계 선택이다.

완료한 C17 Medium은256소스,367,590source bytes에서 Prepare 신규 객체3,083개,
동일 Prepare의 SQLite xWrite1,371회와23,096,256requested bytes를 관측했다.
이 숫자는 저장·Catalog 작업 증폭을 보인다. 모두 불필요한 bytes라는 증거는 아니다.

경로는 [OccurrenceWrites::flush](../../../crates/quanta-index-lexical/src/native/text.rs),
[put_batch](../../../crates/quanta-index-core/src/index/attempt.rs),
[install_prepared_batch_with_barrier](../../../crates/quanta-index-storage/src/object.rs)다.

### 설치 뒤의 occurrence 검증 읽기

`diff_inventory`는 contribution의 token 합계를 확인하고 posting edit를 만들기 위해
position count를 사용한다. 새 reference는 `validated_new_count`를 거쳐 최초 읽기에서
저장 객체의 전체 payload 형식을 검증한다. 이미 검증한 결과는 attempt-bound memo와
spill에서 재사용한다.

현재 install 결과는 등록된 reference이며 검증된 occurrence count 권한이 아니다.
생성 당시 배열 길이나 미검증 SQL 값을 곧바로 count로 쓰면 저장 객체 손상 감지가
약해진다. 새 객체의 저장·검증 결과를 재사용하려면 그 결과를 보증하는 생산자와
유효 수명, identity/payload 일치, 실패 시 무효화를 계약으로 정해야 한다.

경로는 [validated_new_count와 diff_inventory](../../../crates/quanta-index-lexical/src/native/text.rs),
[load_count](../../../crates/quanta-index-lexical/src/native/text_occurrence.rs),
[immutable object port](../../../crates/quanta-index-core/src/index/ports.rs)다.
신규/기존·고유/반복·memo hit/miss 분리가 추가 수리 선택의 선행 조건이다.

## 2026-11-06 추가 코드 대조

### 같은 읽기에서 두 번 해시하는 경로

`FileObjectStore::get`은 `read_wire` 뒤 `decode_stored_wire`를 부른다.
read_io::read_wire_with_owner는 EOF와 전체 wire digest를 검증하고,
decode_stored_wire는 같은 소유 buffer의 digest를 다시 계산한다.
read-only File adapter도 이 순서를 사용한다. 이는 소스로 확인한 중복 계산이다.

다만 decode_stored_wire는 scratch/resident map에서도 쓰이므로 공통 함수의 digest
검사를 삭제하면 안 된다. 수리한다면 reference와 불변 bytes를 결속한 private
read 결과만 검증된 layout decoder로 전달하고, 외부/scratch wire의 인증은 유지해야 한다.
이 중복의 실제 CPU 기여는 미측정이다. 작은 leaf에서의 미세 개선을 Large120 해결로
주장하지 않는다. 별도 포맷 개편 전 검토할 수 있는 좁은 후보로 남긴다.

### 이미 존재하는 캐시 안의 반복 탐색과 여러 순회

- changed body의 각 term은 prior identity 비교 전 encode/identify를 수행한다.
  unchanged면 설치를 건너뛰지만 이 비교 준비 비용은 발생한다.
- changed term은 prepare_inventory, needs_flush, push에서 최대256개 pending 목록을
  반복 탐색한다. RAM 확인 창은64개이며 창 밖은 attempt-local SQLite spill을 조회한다.
  신규 cache 부재가 아니라 bounded 탐색·spill 정책의 비용 후보이다.
- inventory compact plan/CAS 원본 결속, 결과 inventory diff, contribution token 합계와
  posting edit 생성은 별도 순회다. 목적이 다른 검사를 같은 중복으로 취급해 제거하지 않는다.

Prepare lane35 CPU가 작고 namespace/sync 또는 wall 대기가 크면 선형 탐색 가설의
우선순위를 내린다. lane31과 next 검증 호출이 작으면 first-validation 전달 후보도 내린다.
추가 전역 set/cache나 계수는 이 분석만으로 도입하지 않는다.

### inventory Block으로 posting 비용까지 해결됐다고 볼 수 없음

현재 `EntryMap::validate_for(Posting)`은 Block을 거절한다. Block은 inventory에만
적용되고 큰 posting map은 Tree 경로를 쓴다. NativeTextIndex::prepare는 모든 source
change와 잔여 inventory/contribution을 처리한 뒤 apply_postings, write_root를 수행한다.
Prepare 중 deadline 실패한 prefix에는 이 후반 작업이 끝났다는 증거가 없다.
`mr4`는 Postings lane37에서 실패했고 `lr4`는 MapApply에서 실패해 Postings에
도달하지 않았다. 이 미도달 범위를 완료한 전체 초기 구축으로 외삽하지 않는다.

Bootstrap과 Patch는 같은 NativeTextIndex::prepare를 사용하며 SourceInputCursor와
base 유무가 다르다. cold bootstrap에는 prior가 없으므로 unchanged/no-op 최적화의
성공을 초기 구축 속도의 증거로 바꾸지 않는다.

### 긴 빌드와 제품의 semantic 실행을 분리

Runtime/harness는 semantic crate를, semantic은 LanceDB/Lance를, LanceDB는
DataFusion을 무조건 의존한다. 현재 동일 정상 제품을 유지하면서 이 의존을 제거하는
feature rail은 없다. 따라서 Text 전용 fixture도 이 graph를 컴파일한다.
하지만 해당 fixture의 profile은 CoreRequired Text1개, provider_units0이고 semantic
설정이 없다. semantic provider는 config.semantic이 Some일 때만 등록한다.
Lance/DataFusion 컴파일을 제품 SourceAdvance의 embedding 실행 비용으로 해석하지 않는다.

resume02는 admission758.451초(약12분38초) 뒤 실제 compiler에 입장했다.
이는 Cargo 입장 대기이며 제품120초와 별개다. 기존 target의 cached artifact를
재사용하고 있으므로 fresh 전체 빌드를 다시 시작한 상태가 아니다. artifact row 수를
빌드 완료율이나 남은 시간으로 환산하지 않는다.

build stderr의 MallocStackLogging 경고는 원인이 미확정이다. 입력 manifest의
Malloc/DYLD/Rust profiler env key는 부재했고 resume script는 filtered env를 build와
제품 capture subprocess에 전달한다. 메시지만으로 malloc tracing 활성화나 제품
tracing overhead를 주장하지 않는다. 정식 Off/Record는 동일 launch env/관측 모드로 판정한다.

## 이미 존재하거나 수리한 비용 절감

| 항목 | 현재 코드에서 확인된 범위 |
| --- | --- |
| 설치 배치 | 최대256개, 고유 payload1MiB 제한, SQL/owner64 경계 |
| 중복 제거 | 배치 내 exact identity, occurrence 확인 참조와 검증 count memo/spill |
| 변경 생략 | unchanged occurrence와 COW root 재사용 |
| 저장 표현 | empty/singleton inline, Small/Block 등 기존 표현 |
| child 검증 | 배치 내 참조 short-circuit와 transaction 내부 exact set |
| 파일 생성 | nonreplacing fresh claim으로 중복 anchor 관측 제거 |
| allowance | 공유 트리에 별도 accounting-only 검증 변경 존재. 기존 담당의 hunk |
| checkpoint | 기존8-page 처리와 WAL batching. FULL/auto1000 계약 유지 |
| trace 소비 | Python의 단일 Operation-root 가정을 unique IO-summary root 선택으로 수리 |

일반적인 배치·dedup·캐시 추가를 새 결손으로 발급하지 않는다.
공유 파일에 이미 존재하는 allowance 변경을 이 RCA의 신규 수리로 복제하지 않는다.

## 호스트 영향과 아직 구분되지 않은 부분

C17 Lexical의 wall−호출 스레드 CPU는71.150초다. 스케줄 대기, 동기 I/O,
락·blocking과 다른 스레드의 작업이 섞일 수 있다. 완료 시 호스트는16 logical CPU에
load108.46이었고 swap 사용도 관측됐다. foreground Catalog mutex wait는129회 합계
59,792ns로, 그 mutex가 전체 지연의 주원인이라는 근거는 없다.

호스트 경합은 기여 가능성이 높은 조건이다. 그 기여율, device service time,
건강한 호스트에서의 전체4K 완료 시간은 현재 trace로 확정되지 않는다.
C16/C17의 진행 prefix와 환경이 달라 총시간이나 객체당 CPU로 수정 효과를 단정할 수 없다.
Medium과 Large의 파일 크기·term·객체 분포도 다르다.
현재 generator는 Medium80행, Large120행의 본문 loop와 각각3개 고정 행을 쓴다.
총 생성 행은21,248→503,808로 약23.71배다. hit/symbol의 누적 threshold도 다르다.
입력 근거는 `scale.rs`의 TIER_MEDIUM/TIER_LARGE와 generate_scoped_corpus/generate_file,
과거 bytes는 VALIDATION의 C13IO1과 C17 Medium 종료 기록이다.

## 측정·완료 판정에서 생긴 별도 문제

Python child verifier는 Operation root가 하나라고 가정했다. 실제 Native trace는
lifecycle과 Catalog 설정의 독립 root를 갖는다. Rust verifier는 이미 이를 허용했다.
Python도 complete IO summary를 가진 root 하나를 선택하고 그 아래의 성공한 boot를
확인하도록 수리했다. split/incomplete/duplicate summary와 실패한 lifecycle은 거절한다.

C17 Medium 생산자는26.892초에 종료0, 전체 ACK15.845초와 검색·재오픈을 완료했다.
기존 wrapper의 FAILED는 보존했고 같은 원본의 제품·canonical offline 검증이 통과했다.
487개 영향 테스트 통과·2개 skip과 고정 consumer의12개 테스트 통과는 이 소비자 수리의
근거이며 Large 성능 개선 근거로 사용하지 않는다.

### RR 후속 검증기 종료 순서 수리

종료 이벤트 뒤의 sync counter가 종료 시각과 동일한 timestamp를 가지면
Python이 받아들이는 반례를 확인했다. Rust는 Operation root가 아직 열려 있어야
그 counter를 수용한다. 기존 Python 회귀는 timestamp가 더 늦은 경우를 다뤘다.

canonical parser가 Native I/O summary의 세 key를 처리할 때 Operation root가 이미
종료됐는지 확인하도록 수리했다. summary reader는 같은 counter key 집합을 재사용한다.
writer 이벤트 순서가 보존되는 parser에서 거절하므로 timestamp가 같아도 판정할 수 있다.

지원 스키마2–5, first/reopen, 세 counter의24개 종료 후 반례를 모두 거절하고,
같은 범위의 종료 직전 동일 시각24개 입력은 허용한다. legacy schema1의 기존 계약도
독립 회귀로 확인했다. 기존 다중 Operation root와 손상 회귀까지 집중65개가 통과했다.
아래 영향 배치는1,024개 통과·2개 skip,21.57초로 끝났다.

```sh
.venv/bin/python -B -m pytest -p no:cacheprovider -q --tb=short \
  tools/ci/tests/test_causal_cost_timeline.py \
  tools/ci/tests/test_causal_cost_profile.py \
  tools/ci/tests/test_causal_cost_capture.py \
  tools/ci/tests/test_causal_cost_canonical.py
```

skip2개는 prebuilt 실행에 managed-build receipt가 없는 기존 조건이다.
Ruff E9/F와 scoped diff 검사도 통과했다. 이 수리는 offline 검증의 거절 계약을
닫았다. 이후 `lr4`는 다른 고정 소스의 Large 실패 prefix를 다시 측정했지만
scheduler/I/O 기여율과 원120초 수용은 여전히 미확정이다.

날짜별 계획과 상태를 계속 추가하면 다른 source의 실패·코드 수리·미실행 수용이
섞인다. 또한 원본을 휘발 가능한 캐시 경로로만 인용하면 캐시가 없어진 뒤 그대로
재검증할 수 없다. 당시 VERIFIED를 취소하는 사유와 현재 replay가 BLOCKED인 사유를
구분해야 한다. 이 폴더는 현재 판단과 후속 패킷을 소유하고 과거 실행은 이력으로 인용한다.

## r4 초기 판단과 당시 해결 방향

### B02에서 드러난 진단 context의 기존 계약 불일치

WorkBudget의 `scoped_diagnostic_context(None)`은 기존 context를 상속한다.
Lexical 호출부는 이를 명시 mask로 설명했으나 실제로는 Diff span 생성 실패 뒤에도
하위 read가 외부 context를 다시 사용할 수 있었다. B02가 새로 만든 회귀가 아니라
계수 회귀를 추가하면서 드러난 기존 seam이다.

inherit 계약을 바꾸지 않고 Core에 `scoped_diagnostic_override(Some/None)`을 추가했다.
None은 명시 mask, Some은 교체이며 기존 scoped request의 RAII 복원을 공유한다.
제품 요청 identity·deadline·권한·사용량은 그대로다. Core331개와 Text154개가 통과했고,
Diff admission 실패 oracle도 제품 결과·work 동등성과 하위 trace 차단을 확인했다.
이 수리는 관측의 귀속을 바로잡으며 인덱싱120초 비용 개선의 실측 근거는 아니다.

- 직접 실패 원인은 인덱싱을 포함한 단일 SourceAdvance 예산의 소진이다.
- 토큰화보다 객체 설치·검증 경로의 실측 비용이 크다.
- 구조적 작업 증폭은 확인됐으며, 안전하게 제거할 개별 작업은 계약과 계수로 입증해야 한다.
- 120초 초과의 불가피성, 호스트 안정화만으로의 통과, 최신 전체 소스의 수용은 미입증이다.
- 직전 감사에서는 Text·Storage·Native dispatch·kernel update의 주요 파일이 C17과 같았다.
  과거 C17 고정 컷이 없어 그 byte 비교를 갱신할 수 없다. 새 고정본의 수용은 별도로 필요하다.

[WORKPLAN](WORKPLAN.md)의 당시 B03-B1 계획은 확인된 r4 실패를 기준으로
owner-local occurrence block의 codec·producer·reader·정책과 profile-first 초기
Patch 활성화를 합류하고 원 예산·독립 oracle로 판정하는 것이었다. 이후 B2의
8,192B/512-term 확장과 B3 QITXEM02까지 구현·집중 검증했다. 당시 고정본은
Large120에 실패했다. 후속 r34 판정은 [현재 원장](README.md#현재-판정)에 있다.
현재 B02의6개 검증·재사용 계수 생산자와 소비자는 소유 회귀까지 합류했다.
최신 r4의 정상 cached build와 실제 Medium/원 Large Off·Record 캡처는 완료됐고
모두 위 실패로 기록했다. 과거 C17의8,799회 get을
신규 재검증만의 비용으로 재분류하거나 새 성능 수리의 효과로 해석하지 않는다.

## Root의 Legacy etcd 60초 SDK 검증

2026-11-09 KST의 고정 Legacy 입력은 1,421개 파일이다. 원 SDK I/O 제한은
60초이며 physical inventory 예약 한도는 30,000개다. Native 직접 발행의
512MiB·100만 work 한도 검증은 별도 경로다.

계측 실행의 SDK 요청은 61.569초에 서버 발행을 마쳤다. Native 발행
20.974초, File 발행 13.697초, Native changed-segment 검증 3.865초,
File seal 5.353초였다. 시간은 중첩 구간을 합산하지 않고 해석한다.

저장된 gram 경로 635,648행은 리프당 256행 제한 때문에 MapPage
2,523개로 나뉘었다. dictionary 압축 뒤 payload와 의존 수를 검사하는
전용 wide gram leaf를 추가했다. 새 leaf는 최대 1,024행·논리 참조
128개·의존 객체 128개·32KiB payload다. 일반 bootstrap은 256행을
사용하고 변경된 wide leaf의 delta는 기존 일반 형식으로 다시 작성한다.
새 reader는 기존 mode 0·1과 새 mode 2를 함께 읽는다.
기존 binary는 mode 2를 읽지 못하므로 새 상태를 발행한 뒤에는 해당
형식을 읽는 binary가 필요하다.

고정 빌드 `legacy-route-key-v5`의 원 SDK 실행 두 번은 발행
52.367초·53.127초, 활성화 ACK 103.774ms·59.733ms로 모두 종료 코드
0이었다. lexical·semantic·Hybrid 응답을 받았다. 같은 635,648행의
실제 MapPage는 632개로 줄었고 첫 실행 state는 10,951개 파일이었다.
소스·바이너리와 입력 바인딩은 외부 실행 묶음
`/Users/songmin/.codex/a88-gq0uxqug/etcd-native-goal-NMu4RbQ5/legacy-route-key-v5`
에 있다. 이는 해당 로컬 고정 빌드의 SDK 반복 통과이며 호스트 간 성능
비교를 수용한 결과는 아니다.

wide 형식을 포함한 별도 current-source Large 표준 테스트는
`binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`
1/1, 실행 39.265초로 통과했다. 서로 다른 두 프로세스의 전체 source와
순위 페이지를 비교했고 실행 전후 소스가 일치했다. 기록은 같은 외부
상위 경로의 `legacy-route-key-v5-large-current/nextest.log`에 있다.
Native Large120 SourceAdvance 검증과는 구분한다.

### File V3 합류 후 원 SDK 반복 결과

`native-packed-file-v3-product-v1`의 source-bound pair에서 Legacy Caddy는
publish 18.114/19.331초, etcd는 50.299/50.376초로 각각 두 번 통과했다.
원 SDK 60초와 물리 파일 30,000개 한도는 유지했다. 최종 state 파일은
Caddy 3,812개, etcd 10,625개였고 etcd gram MapPage는 632개였다.
File posting digest dictionary의 byte-string wire가 V3이며, FileAuthority
정책은 22다. 정책 21의 상태를 새 서버로 그대로 재오픈하는 수용은 없고
명시적인 재구축이 필요하다. 이 결과는 이전 V2 코호트의 etcd 1/2 통과와
구분한다. 같은 동결 입력의 표준 Large 두 번 재시작은 44.255초에 통과했다.

Native 경로는 같은 빌드에서도 실패했다. etcd는 BuildPrepare bytes
536,870,827 + 457, Caddy는 token group reads 1,000,000 + 1에서 거절됐다.
동일한 Caddy 첫 53개 inventory root에서 terminal packing은 Map 객체를
12,529개에서 7,294개로, map reads를 735,540회에서 463,538회로 줄였다.
이후 더 진행한 배치의 실패를 절감 효과가 없다는 근거로 삼지 않는다.
etcd의 같은 첫 배치에서는 map bytes 절감이 없어 별도의 byte 원인이 남는다.
BuildPrepare의 async 관측 경계에 WorkUsage delta가 빠진 것도 확인했으며,
이 누락을 보완하기 전 미귀속 bytes를 특정 producer의 비용으로 단정하지 않는다.

### Generic Hybrid의 독립 입력 실패와 exact 진단

shared `task.query` V4 정책으로 고정한 독립 Go 세 건의 실제 SDK는
publish·재오픈·세 lane 캡처를 마쳤지만 품질 비교에는 실패했다.
first-distinct-file MRR@10은 lexical 0.3667, Hybrid 0이었다.
라벨이 공개된 이 세 건은 이후 개발 입력이며 새 홀드아웃으로 재사용하지 않는다.

같은 generation·query·모델·후보 창을 bounded exact semantic SDK로 다시
검사했다. 세 건 모두 ANN/exact의 상위 100개 candidate ID가 100/100
일치했고 relevant file의 순위도 같았다. 이 세 건의 저하를 ANN 누락으로
설명할 수 없다. RRF 기여와 원 lane 순위의 계산 불일치도 확인되지 않았다.
약한 semantic 순위가 lexical 상위 후보를 밀어내는 현상은 남아 있다.
임베딩 품질 개선은 별도 모델·sealed generation으로 검증하며 경로명 boost나
공개된 세 건에 맞춘 fusion 가중치 변경을 수리로 채택하지 않는다.
새 source-disjoint 입력의 선정·라벨 검토는 순위 실행 전에 고정한다.

### Nested map·정확한 census·prepared union 합류 실행

`native-nested-census-union-local-product-v2`는 실행 전후 2,612개
Rust/Cargo 입력과 실제 실행 파일을 묶었다. Legacy 원 SDK 60초 반복은
Caddy publish 17.883712/17.985800초, etcd 49.282234/48.126540초로
4/4 종료 코드 0이었다. 입력 파일은 각각 590개/1,421개다. 새 Native
후보의 변경을 이전 Legacy 실행의 성능 효과로 소급하지 않는다.

같은 pair의 Native Off/Record는 두 코퍼스 모두 512MiB bytes에서
실패했다. etcd reads 540,263회, writes 340,030회, mutations 302,234회,
거절 bytes 536,870,852 + 296B였다. Caddy는 reads 540,332회,
writes 511,468회, mutations 510,514회, bytes 536,867,399 + 4,096B였다.
파일 30,000개·reads 1,000,000회·bytes 536,870,912B는 유지했다.

etcd BuildPrepare의 accepted bytes는 499,890,870B, 그중 LexicalBuild는
193,664,836B였다. 아직 개별 Literal/FileMap에 분리 귀속하지 못한
비용은 280,676,420B다. Caddy Text 내부의 InventoryPrepare는
222,688,824B, InventoryMapApply는 151,531,405B, InventoryDiff는
53,700,134B였다. 부모/자식의 중첩 bytes를 합산하지 않는다.

메모리 scope 누락이나 ASCII arena 미사용 가설은 실제 호출 경로와
lane 34 계측이 반박했다. etcd 86/94, Caddy 455/482 완료 소스에서
token mutations가 있으면서 token SQL writes가 0이었다. 나머지
비ASCII 8개/27개는 token SQL writes 89,040/132,957회를 발생시켰다.
정확한 ASCII census는 예약 메모리를 줄였으나 이 실행의 작업 바이트를
줄였다는 증거는 없다. 추가 순회 비용도 청구했다.

후속 후보는 Literal/FileMap prepare·seal 및 독립 collect/map 경계를
각각 계측하고, 같은 TokenCursor/case_chars의 canonical Unicode census로
선택적 cold arena를 구성한다. empty-base inventory Tree는 검증한 원본
OrdinalMapEdit를 전달해 두 번째 key/edit 할당을 제거한다. 원래 순서,
CAS, object-ref, EOF, 독립 seal 검증은 유지한다. 후보의 성능과 수용은
새 실제 SDK 결과가 나오기 전까지 `NOT_RUN`이다. arena가 살아 있는 동안
후속 필수 출력 메모리가 부족해질 수 있는 B35는 별도 미해결이다.

독립 N4는 source·query·정답·분모 4를 사전 고정했다. 두 프로필 모두
원 SDK 60초로 4/4 publish·재오픈·세 lane 캡처를 마쳤다. 같은 실행 파일,
입력, 자연어 query V4, 상위 100개 요청을 사용했다. 첫 검증기는 실제
후보가 100개 미만이면 잘못 거절했다. fsnotify의 lexical 62개와 dense
71개는 typed exact_exhausted 증명이 있는 완전한 짧은 창이었다.
검증기 V2는 그 증명과 returned/count/coverage를 대조한다. 점수식·라벨·
선정·분모는 그대로 두었고, 누락·부분·외부 후보는 계속 거부한다.

실제 N4의 first-distinct-file 지표는 다음과 같다.

| Profile/lane | MRR@10 | Recall@10 | nDCG@10 |
| --- | ---: | ---: | ---: |
| Shared lexical | 0.319444 | 0.75 | 0.469316 |
| Potion semantic | 0.535714 | 1.00 | 0.642840 |
| Potion Hybrid | 0.473214 | 1.00 | 0.592326 |
| Jina semantic | 0.406250 | 0.75 | 0.537301 |
| Jina Hybrid | 0.340278 | 1.00 | 0.543027 |

Potion Hybrid의 lexical 대비 MRR@10 차이는 +0.153770이며, 경로명 boost나
fusion 가중치를 바꾸지 않았다. 같은 네 건에서는 Jina가 Potion을 이기지
못했으므로 모델 크기나 코드 모델 표기만으로 품질 수리를 수용하지 않는다.
이전 독립 세 건의 실패는 그대로 남으며, N4 승리를 그 회귀 해결로 소급하지
않는다. N4는 4건의 exploratory 검증이며 기본 모델 교체나 광범위한 일반화
수용으로 확대하지 않는다. typed Go failure 평가는 별도 `NOT_RUN`이다.
고정 scorer와 원 캡처는 외부 `local-code-new-holdout-v1` 묶음에 있으며
점수는 `scorer-private/n4-score-v2.json`이다. 로컬 모델 프로세스는 종료했다.

### Canonical Unicode 후속과 Literal V3 후보

`native-canonical-fixed-member-cost-product-v4`는 canonical Unicode census와
empty inventory의 원본 edit 전달을 포함했다. 집중 Rust 회귀는 139/139였다.
Native etcd Off/Record는 bytes 536,870,887 + 208B에서 동일하게 실패했다.
reads 461,660회, writes 251,887회, mutations 325,286회였고 memory 거절은
없었다. Caddy도 bytes 536,870,606 + 471B로 실패했으며 reads 451,327회였다.

새 계측에서 etcd의 Literal prepare 148,986,323B, 독립 seal 125,308,093B,
FileMap prepare 31,901,514B, seal 1,598B를 확인했다. Literal의 독립
Changes::collect는 각각 66,434,502B였다. Caddy Text MapApply는
231,476,929B, InventoryPrepare는 142,555,417B였다. 비용은 중첩을
제거해 해석한다. 소스 커서는 owner key 순서이므로 완료 owner 94개를
manifest 앞 94개와 같은 파일이라고 추정하지 않는다.

같은 V4 Legacy 실행의 발행·세 route 응답은 끝났지만 후속 Python 출처
검증이 실패했다. 검증 체크아웃 복사에서 누락한 gitignored grammar
Cargo.lock 네 개가 원인이었다. 현재 grammar digest가 이 파일들도
포함하기 때문이다. 실패를 성공으로 바꾸거나 digest 검증을 완화하지
않았다. 후속 동결 입력에는 ignored grammar 파일과 Python consumer를
추가해 2,728개 입력을 기록했다. grammar producer·consumer 규칙의
구조적 관리는 기존 P07 오너에게 전달했다.

Literal V3는 같은 canonical 35B key를 owner dictionary로 저장한다.
물리 block의 V2/V3 실제 길이를 비교해 작은 wire를 선택하며 두 준비
비용을 모두 청구한다. V1/V2 persisted artifact 읽기, 독립 seal, 32KiB
block cap은 유지했다. `native-literal-v3-complete-input-product-v6`의
집중 Rust는 140/140 통과했다. 작은 wire 선택만으로 WorkBytes 절감이
보장되지는 않으므로 원 Native SDK의 순효과는 별도로 측정한다.
Scratch SQL의 기존 strict 9건은 경고 억제 없이 정리했고
`scratch-root-strict-v2`의 좁은 Clippy 실행이 통과했다.

V6의 실제 Native Off/Record도 bytes 한도에서 실패했다. 같은 Text 첫
flush 구간의 Literal prepare는 167,424,592B, seal은 116,453,410B로
V4 합계보다 9,581,762B 늘었다. 더 작은 저장 wire와 별개로 이중 인코딩이
작업량을 늘린 결과다. 이 후보를 성능 수리로 수용하지 않았다. 후속은
compact row 하나만 만들고 V2 길이를 계산하며, 실제 작은 형식을 한 번
발행한다. V2로 전환할 때의 재구성과 인코딩 작업도 청구한다.

Text의 후속 cold inventory는 512 term을 넘을 때 term별 map 대신
occurrence block·leaf별 upper-fence directory를 만든다. 원본 ordinal
SQL 스트림의 CAS·순서·개수·EOF와 저장 block의 term/ref를 모두 확인하고,
두 번째 원본 스트림과 재오픈한 directory 전체를 대조한다. warm delta는
기존 Tree로 전환하며 이전 directory도 읽을 수 있다. 이 범위의 집중
Rust는 `native-cold-directory-product-v8`에서 154/154 통과했다. 이후
약속된 map continuation이 빈 EOF로 바뀌는 오류의 거절 회귀를 추가했다.
최종 기존 V2·directory 합류 후보의 실제 SDK는 실행 후 별도 판정한다.

V6의 표준 Large 두 번 재시작은 1/1, 본문 85.339초로 통과했고 전후
2,728개 입력이 일치했다. Native Large120과 다른 검증이다. V6 Legacy
Caddy publish는 16.634683/50.992814초로 2/2 통과했지만 etcd는 첫 실행
49.098408초 통과 후 두 번째 SDK 60,000ms 읽기 timeout으로 실패했다.
반복 통과 판정은 다시 열었다. Caddy의 같은 binary에서 FileAuthority가
4.186→29.385초, seal이 2.896→8.760초로 늘었지만 이것만으로 etcd 실패
단계를 확정하지 않는다. 동일 V6 binary·입력·60초로 causal phase와
timeout events를 보존하는 별도 진단을 요청했다. 진단을 수용 실행으로
혼합하지 않는다.

실제 harness의 owner(index)는 manifest 순번을 big-endian stable ID로
인코딩한다. 따라서 이 고정 입력에서 owner-key 순서가 manifest 순서와
일치한다는 코드 근거를 확인했다. 95번째 source의 본문이 72번째와 같아
94개 준비 직후 첫 flush가 발생한다. 해당 94개는 2,111,222B, canonical
indexable runs 164,086개이며 token SQL writes는 0이었다. 남은 원문
6,554,995B는 아직 처리 비용을 측정하지 않았고 선형 외삽하지 않는다.

Literal 단일 compact 후보도 후속 정적 비용 감사에서 거절했다. 실제
harness의 연속 ordinal owner는 V2 prefix 압축이 잘 되는 입력이며,
discarded compact 후보와 V2 재구성이 작업량을 크게 늘릴 수 있었다.
검증 전 성능 단언을 통과시키려고 수수료나 테스트 기대값을 바꾸지 않았다.
`literal_blocks.rs`는 SHA가 V4 동결본과 같은 기존 V2로 복원했고
Literal/FileMap 단계 계측은 유지했다. Legacy File posting의 V3와는
서로 다른 형식이며 해당 수정은 유지했다. 최종 후속 후보는 기존 Literal
V2와 Text directory를 합친 `native-v2-cold-directory-product-v12`이다.
실행 전 감사에서 검증 체크아웃에 남은 V2 `finish` 호출 두 곳을 main과
동기화했다. directory 관련 일곱 파일과 continuation 거부 테스트 연결은
소스상 일치한다. V11은 공통 자원 admission 이전에 중단돼 실행 본문이
없으며, V12 Rust·실제 SDK는 아직 `NOT_RUN`이다. 공유 supplier의 실제
빌드 종료 후 동일 V6 Legacy 진단, V12 집중 Rust·제품 빌드 순으로 재개한다.
기존 오너의 최신 여덟 경로를 같은 검증 체크아웃에 합류한 뒤 main 전후와
체크아웃의 입력 2,730개가 모두 일치함을 확인했다. 동일 V6 진단에 쓰는
Python consumer·Cargo.lock·vendor 입력 1,133개는 V6와 그대로 일치한다.

Legacy의 기존 `WORK_RESERVED_FILES=30000`은 한 번에 보유하는 Work
allowance의 하드 한도다. 전체 상태 루트 한도 `MAX_FILES=200000`과
구분한다. V6 etcd 성공 실행의 물리 파일 10,625개는 관측치이며, 전체
루트의 3만 파일 하드 한도 검증으로 확대하지 않는다. Native의 파일
30,000·512MiB·읽기 1,000,000 및 SDK 60초 조건은 그대로다.

directory 후속 감사에서 512-term occurrence block을 64행씩 순회하면
최초 get 뒤 최대 일곱 번의 retained-read가 wire·decoded bytes를 다시
청구함을 확인했다. 이는 물리 I/O 재실행을 뜻하지 않는다. directory의
bulk 페이지는 이제 한 블록·leaf 경계에서 끝나며 실제 잔여 행만 선예약한다.
MapEntries는 이 경로에서만 최대 512행을 요청하고, 이전 페이지의 마지막
키를 별도 허가·byte 청구한 복사본으로 옮긴 뒤 소진된 내부 참조를 해제한다.
호출자가 보유한 행의 custody와 작은 페이지의 기존 retained-read는 유지한다.
512+2+leaf의 순서·값·EOF, bulk get 세 번·retained-read 0번, 64행 연속
페이지의 retained-read 일곱 번과 거부·취소 회귀를 추가했다. 독립 소스 리뷰와
format/diff 검사는 통과했으며 회귀 본문과 실제 성능은 여전히 `NOT_RUN`이다.
동시에 변경된 기존 오너의 harness 다섯 경로도 읽기 감사 후 합류했다.
SDK60·file30000 조건은 그대로이며, 합류 후 main 전후와 검증 체크아웃의
입력 2,730개가 다시 일치했다. B35 전체 메모리 admissibility 해결로 확대하지
않는다. 공유 supplier 종료 전 SDK 진단도 보류한다는 자원 조정 결과를 유지한다.

추가 정적 감사에서 다음 보완 경계를 확인했다. 이번 감사의 Rust·SDK·점수
재실행은 `NOT_RUN`이며, 아래 내용을 관측된 Product 장애로 확대하지 않는다.

- P2 읽기 일관성: `entry_map/directory.rs`의 stateless `scan`은 다음 packed
  block의 첫 key가 직전 upper fence를 넘는지 검사하지 않는다. 각 block은
  유효하고 fence가 정렬돼 있어도 `[a,c]`, `[b,d]`처럼 범위가 겹치면 역순
  결과를 만들 수 있다. cached `Cursor::advance`는 이를 거절한다. 현재
  확인한 생산 inventory 순회는 cached 경로이며, 두 reader의 동일 거절
  회귀와 stateless 경계 검사가 필요하다.
- P2 재시도 계약: `Cursor::advance`는 block 검증 전에 descriptor 상태를,
  `next_page`는 성공 반환 전에 term slot을 변경한다. 페이지 중간 거부 후
  같은 cursor를 재시도하면 행을 건너뛸 수 있다. 생산 호출은 `?`로 attempt를
  종료하므로 현재 Product 데이터 손실 근거는 없다. 새 회귀의 pre-row
  retained-read 거부 후 재시도만으로 일반 재시도 안전성을 보장하지 않는다.
  상태를 성공 시에만 반영하거나 오류 후 재사용 금지를 명확히 해야 한다.
- B35 조건부 메모리: optional arena는 lane35의 필수 key·payload 예약까지
  살아 있다. arena 승인만으로 이후 필수 출력 여유가 보장되지 않는다.
  동일 입력·동일 memory ceiling에서 전체 SQL 경로와 cold 경로의 완주를
  비교하는 회귀가 필요하다. 현재 SDK 실패는 WorkBytes이며 이 메모리
  경로의 실재 발생은 미검증이다. 필수 예약 실패 후에는 terminal denial을
  되돌려 fallback할 수 없으므로 첫 출력 effect 이전 admission이 경계다.
- Hybrid 품질: N3 한 건은 세 lane top100에 grade2 파일이 없고, 두 건은
  lexical 파일 순위 10→14, 1→28로 융합 후 하락했다. 후보 recall과 fusion을
  분리해야 한다. 일반 검색은 chunk candidate_id RRF 뒤 topK를 반환하고
  scorer는 그 뒤 파일 중복을 제거하므로 파일 검색 목표와 단위가 다르다.
  typed file identity로 lane rank·provenance를 합치는 별도 파일 계약을
  검토하되 이것만으로 후보 부재나 lexical 비퇴행 해결을 보장하지 않는다.
  N4의 탐색적 양수 평균은 N3 폐쇄 근거가 아니며, 후속 튜닝은 노출된
  N3·N4를 DEV로 두고 새 사전등록 blind holdout에서 검증해야 한다.

### Root follow-up: credit, file ranking, and the V6 causal diagnostic

- Native cold output now preflights a scoped memory credit before the first
  effect. Alias output transfers exact key/payload grants to the canonical
  encoders; nonalias output uses the canonical block split/JSON length census
  and supplier bounds through Native IO, Catalog, FileStore and inventory SQL.
  Unknown/insufficient credit drops the optional arena and selects original SQL
  replay before effects. Mandatory refusal remains terminal; limits are unchanged.
  This bounds tracked `WorkBudget` allocations, not SQLite internals or total RSS.
- Directory readers now reject overlapping block ranges consistently. Cursor
  descriptor/term progress commits only after validation/success. Regression
  source includes overlap and mid-page/next-descriptor refusal then retry.
- An explicit GenericFile contract fuses first-distinct `SourceFileKey` lane
  ranks before file topK, with original chunk witnesses and RRF k60. Ordinary
  chunk Hybrid and GoTrace ranking retain their contracts. Dense candidates are
  stabilized before the chunk cap, including ties at its boundary. No path
  relevance boost or model/weight switch is introduced.
- These new Rust changes are **NOT_RUN**. V13 has separate memory/supplier,
  Native/normalizer, Core Hybrid, contract/SDK/dispatcher and benchmark-bin rails
  before immutable products. The previous 154-body/V6 results do not qualify V13.
- The permitted V6 Legacy etcd causal diagnostic executed: SDK60 read timeout,
  exit2, outer wall64.949592s. Three ingest windows returned by3.698505s; the
  server backend returned at61.336804s, followed by peer cancellation. Nested
  timings include Native publish19.204s, File publish14.295s, Native seal
  transition3.132s and changed segments5.505s, File seal5.597s, finalization2.807s.
  These are not additive flat costs. Data-file/shard POSIX sync timings total
  about0.446s across10,138 calls each, so call count alone does not identify the
  timeout owner. Concurrent compiler activity and causal instrumentation make
  this a diagnostic, not a quiet-host performance acceptance.
- N5 fixes eight source/query-disjoint cases before outputs, Potion default,
  1,000 chunks per lane/100 files, and file metrics at10. This differs from old
  top100 DEV runs. Blind labels and the independent scorer are locked before
  outputs; ranker access is denied. Scorer/controller checks passed, but actual
  N5 SDK captures, paired-pool validity and quality scores remain **NOT_RUN**.

### Root structural follow-up: Legacy producer and canonical HybridFile capture

Legacy segment construction built RAW/FOLDED grams and positions, then derived
the same four relations from the original bodies again before writing. The seal
independently derives them from persisted bodies a third time. The producer now
emits the canonical `PositionsIndex` terms/postings directly, retaining decoder,
local-ID/order/size checks and the existing body/posting wire. Fixed Unicode,
repeated-term, position and CBOR gram/term goldens cover this change. The old
producer oracle remains test-only; persisted `paged_verify` is unchanged.

An independent source review checked the safety boundary: source-corpus batches
with `seal=false` are rejected before mutation; a delta base is read only from
committed sealed identity/manifest; final seal verifies every new segment after
that base. Publication-free legacy index mutation is refused. The measured
5.894s segment-build envelope includes both old passes; it is an upper bound,
not a measured saving. Current Rust/SDK performance is still **NOT_RUN**.

The first GenericFile implementation was available only through `file-profile`.
That did not change the standard `hybrid` benchmark route. The canonical runner
now has an explicit `hybrid_file` route, required separate `--file-top-k`, and
the unchanged NaturalLanguage chunk query/lane cap. It invokes the actual SDK
`.files()` route and publishes semantic data even when this is the sole route.
Ordinary chunk `hybrid` and lexical `natural_language_file` retain their contracts.
Raw file witnesses, SourceFileKey, pin, RRF score and both chunk windows are
preserved in the record and diagnostic; representatives retain source-span proof.

The Python driver/evaluator admit standalone capture of this explicit route and
verify its evidence. The canonical paired file evaluator does not yet support
its bounded chunk-lane contract; capture support is not paired qualification.
Unknown/partial/approximate or short lower-bound chunk pools cannot become a
scored result merely because the file cap is filled. The product now measures
file fusion as the final query stage. Shared window validation also rejects a
boolean substituted for an integer exhaustion count.

Completed-response hashing required a second consumer fix: nested RRF/raw scores
previously triggered the no-float canonical JSON refusal, while request IDs and
stage times made repetitions unequal. The explicit file route now normalizes
only its declared score fields to f64 bits and excludes its explanation from
the comparison digest. The complete original explanation remains in evidence.
All pin/window/file/rank/cap fields remain bound; unrelated floats still fail.
Existing response hash goldens are unchanged. Rust/Python share a fixed new
golden and mutation tests, with Rust execution pending the shared build slot.

Python verification: the broad retrieval benchmark regression rerun passed
591 tests; the file evidence/normalization suite passed 41 tests. The first broad
run caught one misplaced new branch in the historical diagnostic validator;
that was corrected before the passing rerun. These are Python/source checks,
not SDK or ranking-quality acceptance. N5 fixed-eight input preflight still
passes without changing its inputs/labels/scorer. Its actual SDK/scoring and
the original Native/Legacy/Large acceptance remain open.

### Root end-to-end audit: contract joins and repeated work

The audited publication path is SDK batch/digests/transport, dispatcher
preflight/journal, semantic and lexical publication, independent persisted-body
seal, durable finalization, then activation CAS. The query path is pinned read
view, lexical/dense collection, chunk or distinct-file RRF, source-proved
capture, window/model/hash validation, then evaluation. Publication seal and
activation checks remain independent of producer tokenization. No new
query-visible unsealed-publication defect was established by this source audit.

Three capture/evaluation joins required correction: `hybrid_file` was absent
from declared-model identity checks; its input/result JSON schemas lagged the
Python admission path; and a nonempty short file prefix from complete declared
chunk pools was projected to error with no candidates. Such a prefix must keep
its candidates as `capped`, with the original lower-bound window intact. It is
not generation exhaustion or independent proof of file-pool completeness.
Unsupported canonical paired file evaluation now refuses this route before
capture. The independent N5 scorer still has to validate the raw lane pools;
standalone capture support cannot substitute for that comparison.

Additional source-confirmed cost candidates, with no measured saving yet:

- GenericFile dense collection uses exact canonical Top-K and bypasses ANN.
  Its scan-work bound is sealed row count times dimension, not returned Top-K.
  Ordinary chunk semantic/hybrid may use ANN. `file_profile.rs` captures its raw
  semantic comparator through that ordinary SDK route, then calls `.files()`
  separately. These are not a guaranteed identical dense-pool contract. N5's
  independent scorer must reject a mismatch rather than report a fusion loss
  or win. Supply matching canonical raw pools before a qualified comparison;
  the existing work-bounded semantic API is not a drop-in replacement because
  it does not request canonical boundary ties. Do not discard canonical tie
  selection to obtain a faster but different contract.
- Staged publications above the 64MiB inline threshold hold one global commit
  mutex through decoding and the entire publication/seal/finalization call.
  Different repositories can therefore receive `CatalogBusy`. Any narrowing
  needs explicit decoded-input memory admission first; this source finding is
  not evidence that the measured etcd timeout used staged transport.
- Staged SDK transport serializes for length, identity and part emission in
  separate passes. Identity hashing already computes the byte count. Reuse that
  count or a bounded immutable wire view rather than allocate a full CBOR body;
  source/batch digest domains and replay binding must remain independent.
- Cold output credit refusal drops the completed arena and repeats tokenization
  into SQL. Its supplier bound always budgets 256 objects/1MiB payload, even for
  smaller shapes. A safe bound must include pending inputs and peak flush work;
  profile actual refusal frequency before changing the bound. Directory scans
  also fetch one descriptor per map scan; read amplification is not measured.

These source findings do not replace current-cohort Rust tests, Native/Legacy
SDK60, standard Large restarts or independent holdout quality acceptance. B14
posting normalization and P07 transport remain with their existing owners.

Audit verification: `pytest -q` over retrieval benchmark, HybridFile evidence
and default-decision modules passed 655 tests. Final integration then removed a
schema-only prohibition on an unused Semble profile: the common spec is valid
for standalone Quanta capture, while the paired command still refuses
HybridFile. The affected HybridFile/file-pair/model selection passed 18 tests
(575 deselected) after that final change. Ruff, Rust formatting and diff checks
passed. Rust test bodies and actual SDK/holdout runs are NOT_RUN on this cohort.

### Structural follow-through on the four audited costs

- Canonical raw pools now use the existing work-bounded semantic request with
  required `canonical_ties` request/response binding. True selects the same
  exact source/path/span/id order before Top-K as `.files()`; false retains the
  prior bounded semantic contract. The file diagnostic preserves its raw
  candidate/window fields and records mode and settled work. Its 10,000,000
  work-unit ceiling is unchanged. Ordinary semantic/hybrid remain unchanged.
  The two downstream dense-sampler literals in semantica were updated to false
  under a separate two-file claim after confirming no overlapping claim.
- The exact backend already used bounded Top-K sorting. The vector column is
  now removed after distance calculation and before that sort, then loaded for
  selected output rows only. Distance computation over the generation and tie
  key reads remain; this is a data-movement reduction, not an ANN substitution
  or removal of the exact scan's row-count-times-dimension cost.
- Cold object-install credit now uses the actual block shape plus pending
  inputs/payload and the maximum possible flush packet, rather than always
  requesting 256 objects/1MiB. Confirmation grants use the same physical input
  bound. Tests cover parent-credit refusal/admission/release and a large pending
  packet. On output-credit refusal, a smaller optional row/key packet plus
  scoped SQL-string credit can replay the validated arena through the existing
  token writer without normalizing the source again. Both grants precede the
  first reset/write. If either is unavailable, the original arena-drop/source
  replay remains; errors after SQL begins are terminal. Unicode/position and
  sorted wide-key byte-cap oracles cover the transfer in regression source.
- SDK transport reuses the identity walk's byte count for the inline/staged
  decision, removing one complete CBOR pass on the staged path. Staged commit
  admission is now repo-local with eight active slots and an aggregate nominal
  encoded-body-plus-retained-header byte ledger. Reservation precedes decode
  and lasts through publish/discard; a missing reservation refuses before
  publication. This ledger is not an allocator/RSS bound. The individual upload
  cap and Native 512MiB WorkBytes limit are unchanged.

The N5 v3 controller additionally refuses captures lacking canonical mode or
positive bounded settlement. Its fixed eight inputs, cap, model, labels and
independent score formula were preserved. Input preflight and synthetic
controller admission checks passed; actual SDK captures and scores are NOT_RUN.
The standard Large selector remains
`binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`,
which opens two OS-process instances after preparation. A different one-restart
helper is not a substitute for this requested scope.

### Follow-up review: publication cleanup, directory intervals and pool evidence

The follow-up review found three additional source defects. Their Rust behavior
proof is still pending the shared compiler slot; source completion is not SDK60
or quality acceptance.

- Directory random lookup/scan could select an overlapping packed descriptor
  without validating its lower boundary. Cursor iteration already retained the
  previous fence, but stateless resumed lookup did not. The shared interval
  check now uses a known predecessor during iteration, or a point/seek witness
  for a random block. Equality overlap and lookup misses also check the next
  interval. This adds constant random-lookup reads, with no new persisted
  objects or extra map I/O for the sequential cursor. Regressions cover strict
  and equality overlap, leaf boundaries, valid gaps, retry and cancellation.
- A staged publication could commit successfully and then lose its receipt to
  `CatalogBusy` when a different upload held the store operation lock during
  cleanup. Cleanup now has a separate Complete/Deferred/Failed disposition.
  Deferred identities have a bounded eight-entry process-local queue drained
  at later operation boundaries; current identities are skipped. Real cleanup
  errors remain visible and retained for retry. Restart preserves the body
  under the existing TTL/quota, not a durable queue promise. Before-commit
  cancellation/error retains the body. Required nullable wire status and SDK
  staged/direct binding keep cleanup separate from durable journal identity
  and activation. The three downstream outcome literals were migrated under a
  narrow claim; their owning Rust rail is not yet executed.
- A HybridFile response could omit a file from an exhausted bounded union, or
  relabel its cap, while passing the standalone evidence validator. The rank
  owner now supplies the bounded distinct-file count before truncation; the
  response echoes the requested file cap and checks the complete bounded
  prefix. Runner records also bind the independently declared cap. This does
  not claim global generation exhaustion. The old validator accepted the
  missing-winner fixture; a missing new argument was only an API failure, not
  behavioral RED.

The independent holdout scorer also compared complete endpoint-specific window
objects. Their lane labels and profile/cost/coverage traces intentionally differ,
so valid equal candidate prefixes could be rejected before scoring. The original
frozen scorer remains unchanged. A revised scorer and negative tests are now
separately frozen before any output: validate each route's window independently and bind
the entire ordered chunk pools by producer commitments, independently recomputed
from captured raw candidates. Candidate order/ID/source/path/score, count, pin,
exhaustion and RRF checks must remain. The score formula, labels, eight inputs,
model and caps are unchanged. N5 uses the fixed `potion-code-full-v2` profile;
it is not evidence for the CLI's different default `potion-code` profile.

The file response now carries SHA-256 commitments to both entire ordered chunk
prefixes after canonical ordering/truncation and before file projection. The
transcript binds a domain, generation pin, row count and every row's source
repository/path/candidate ID/original f32 bits. Independent Python reconstruction
checks nonrepresentative rows too. Source ownership remains independent of the
containing index pin; the helper admits distinct source repositories. JSON
roundtrip, signed zero and source/order/score mutations have fixed independent
goldens. These commitments prove bounded-prefix equality, not global recall.

Root verification completed: repository Python 3.12/Unicode17 batch 1,618 passed
before the final pool commitments; the affected evidence/schema/hash/merge batch
then passed 62 tests after them. The external scorer v2 synthetic suite passed
18 tests. Its RRF, metric, gold, inventory, plan, pair and original-record checks,
plus file ranking/witness reconstruction, were compared with v1 and are
unchanged. The new v4 controller passed fixed-eight input preflight plus two
positive/nine negative lock checks. No real holdout output existed at the new
pre-output freeze. This is validator proof only; actual Rust, SDK60, standard
Large and holdout scoring remain NOT_RUN because compiler admission is held by
the coordinated owning work. No Root waiter or temporary checkout is retained.

### Scorer follow-up: consume the complete producer evidence

The next read-only join review found three omissions in scorer v2. It checked
ordered candidate-pool equality but did not consume the newly exported file cap
and bounded file count; it locked controller source without comparing that SHA
to either controller receipt; and it relied on the controller for canonical
semantic mode and bounded-work settlement. These are evidence-validation defects,
not a reproduced change in file relevance or a defect in SDK response binding.
The old positive fixtures also omitted these fields, so they did not cover the
actual producer contract.

Scorer v3 now independently checks the response cap against policy, computes
the full distinct SourceFileKey union before truncation and binds its count,
checks both preflight/status controller SHA against the frozen controller, and
validates all four semantic canonical/work fields. Sixteen negative variants
failed against the pre-fix behavior; the repaired synthetic suite passes21 tests.
Root inspected the diff and verified unchanged RRF, metrics, gold, inventory,
plan, pair, original-record and score functions. File order/witness reconstruction
is unchanged apart from reusing the exact union already computed for its count.

Controller v5/scorer v3 and their exact three-code-file lock were frozen before
any real N5 output. Original v1/v2 artifacts, labels, policy, eight cases, model
and budgets remain unchanged. Fixed-eight input preflight passes with the new
lock. Current main upload/cleanup/SDK and cold/directory/B14 joins had no new
source-confirmed defect in the bounded follow-up review. Rust and real SDK
execution remain open: the quanta build lock has a live source5 all-features
build, while the host has live D42-SDK/P07 compiler processes. No Root long
waiter, copied product result or extra validation checkout was introduced.

### Resumed Rust execution: cleanup test setup failures

Current-source canonical focused execution passed15 contract,157 memory/storage,
195 Native,25 Hybrid core and55 Hybrid SDK/search-plane test bodies. Initial
compile failures were confined to unused return values in newly added tests;
binding or asserting those values resolved the executed compiler diagnostics.

The subsequent IPC run executed25 tests with21 passes and four failures. All
four reported `source upload part is empty or exceeds its byte bound` before
the intended postcommit cleanup transitions. Their fixture already contains a
full part-sized bundle; serializing its enclosing CBOR batch adds headers, so a
single manually constructed upload part violates the legitimate part bound.
The SDK roundtrip test already uses the canonical multipart iterator. Reusing
that iterator and checking every ACK's identity and next offset repairs the
four setup paths without shrinking the body, raising limits or changing the
cleanup assertions. Production upload rejection was correct. The repaired
tests remain unverified until an admitted rerun executes their cleanup bodies.

The rerun's60-second admission request expired with compiler/body count0 while
the existing XL/Mixed product owner held the live project lock. This is an
execution-scheduling boundary, not an IPC regression or evidence of SDK60
performance. SDK60, standard Large and N5 comparisons still require current
immutable products and their original acceptance conditions.

After the live owner released admission, the unchanged repaired source ran the
IPC scope successfully:25/25 bodies passed, including deferred cleanup under
contention, restart retention/queue bound, unrelated-reader drain and visible
I/O failure followed by retained-body retry. The multipart test-setup RCA is
therefore verified for that focused scope. This does not close SDK60 or runtime
product acceptance.

The persisted semantic integration target also exposed an execution-profile
mistake: it unconditionally imports debug-only `test_support`, while Root had
selected bench (release inheritance). Its E0432/body0 did not justify changing
the production cfg gate. The same frozen source passed the selected persisted
canonical-tie test1/1 with nextest's default test profile. The bounded semantic
SDK binding then passed1/1 with its recorded bench configuration.

Benchmark compilation next found two unused `u32` return values from
`validate_public_top_k` in `file_profile.rs`. The CLI now uses those validated
values for its subsequent checks and SDK requests. This is a compile repair;
the values, limits, refusals and ranking policy are unchanged. Source checks
pass; Rust revalidation awaits the existing C34 exclusive execution boundary.
No current immutable product build or SDK60/Large/N5 result exists yet.


<a id="native-final-executor-20261111"></a>
## 최종 immutable 후보와 실제 입장 결과

단일 실행기는 기존 D helper를 소비했고 main 생산 코드는 수정하지 않았다. 구
`source5`/build9 및 `22a8712e` 입력·실패는 원본에 남아 있다. 동결 후보는
`/private/tmp/qidx-xl-mixed-rca-z1a_82he/source-main-20261111-r2`의
`824629e4f72b835788bc31c7f1bed5715f6e4f9c`다. main HEAD는
`ade5fadc1a084fbb81eba831861e996081d80867`; 입력3,189개·vendor lock11개,
capture drift0이다. F3a의 입력2,740개와 내용 차이0이고 Core rustdoc을 포함한다.
manifest `BASE/rail-main-20261111-r2.json`의 SHA256은
`57e0b03c0d113cf6700e7c284c90795103167d39dc0572b1a1170729f01e262a`다.
여기서 BASE는 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다.

- **F1 VERIFIED:** `build_host_native_main.py <manifest> final-executor-2`를 Semantica
  `.venv/bin/python -B`로 실행했다. shared wait0/project lock 허용 뒤
  `./scripts/cargow --lane bench-lane build -p quanta-index-searchd-harness --bin scale_matrix
  -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --offline
  --profile bench --message-format=json-render-diagnostics`가 exit0이었다. registry
  dependency615·Cargo config2·tool8·runner7 결속, 외부 path dependency 거절을
  유지했다. cached bench build이며 fresh/release 증거는 아니다. terminal은
  `BASE/build-main-20261111-r2/terminal.json`; scale_matrix SHA256
  `1c1b292ae3b670f573ee7ae411d76d8128a35b1268ca6268ef1211faf18d42eb`,
  daemon SHA256 `0c33c355dc8eb7fb8b5295e07eb8961ad9f20e85c919605facd58031c9afba39`다.
- **F2 첫 입장 BLOCKED:** `host_native_main.py <manifest> final-executor-1`은
  `BOUND_CANDIDATE_READY` 뒤 11월11일 04:20:40 KST canonical exclusive wait0에서
  `CompileActivityBusyError`로 거절됐다. lock은
  `/Users/songmin/Library/Application Support/semantica-codegraph-v2/quanta-build-cli/host-coordination/compile-activity.lock`;
  receipt는 `BASE/products-main-20261111-r2/host-final-executor-1/status.json`이다.
  host_admitted=false·controller exit1, case body0이고 제품 기능 판정은 없다.

시스템 Python3.9의 `zip(strict=True)` 미지원과 Quanta venv의 QBC `blake3` 누락은
빌드 전 진입 오류였다. 기존 QBC venv로 해결했으며 helper/의존성은 수정하지 않았다.
build admission authority `a3e4743b…`와 후속 제품 admission `1c90680d…`는 서로
다른 epoch다. 상세 원본은 `BASE/native-final-executor-20261111-r2-result.json`이다.
기존338/335/27/6 회귀를 재실행하지 않았다. 다음 원 helper 명령은 후행 main SDK
변경을 source guard가 거절했으며, 이를 우회한 현 main 전체 수용으로 해석하지 않는다.

```sh
/Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/host_native_main.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2.json final-executor-2
```

<a id="native-final-executor-retry2-20261111"></a>
### wait0 재확인과 단일 canonical900초 예약 — 04:52 KST 종료

원 helper guard는 main SDK `binding.rs`, `error.rs`, `search.rs`,
`tests/builder_tests.rs`의 후행 HybridFile/GoTestTraceFile·rustdoc 변경을 거절했다.
Native scale의 `NativeUdsClient`와 겹치지 않는 차이였지만 source 일치로 숨기지
않았다. 별도 exclusive wait0도 lock busy로 BLOCKED였고 receipt는
`BASE/native-final-executor-wait0-retry2.json`이다. 승인된 claim은 동결
`824629e4` pair의 제품 수용이며 `current_main_equivalence=NOT_VERIFIED`다.
원 source/config/tool/binary와 7 helper의 `require_binding`/
`verify_native_main.py` 검사는 유지했다.

private execution-only `queued_native_final_r2.py`(SHA256
`cc17572c30b791c04e3130811fe61483fa879718200b5ee0b145c051df4ef7ad`,
canonical authority SHA256
`1c90680db39be94a5b5c73caeffdc26bb0c4d05eb79e45d07b559cedd868bc76`)
한 건이 operator lock 아래 canonical admission→execution 순으로 EX를 최대900초
기다린 뒤에만 원 resource admission/products leaf를 호출하도록 했다. case 판정,
timing preflight, 120초·예산·수명 기준은 그대로였다. 04:37:54–04:52:54 KST
EX 미획득으로 `CompileActivityBusyError`: host_admitted=false,
leaf_started=false, Large/XL/Mixed·Record body0이다. PID43963 종료/operator lock
해제, 전후 custody exit0을 확인했다. detached launcher는 OS exit를 별도 수집하지
않았으므로 terminal BLOCKED와 프로세스 종료만 주장한다. formal Off/Record
비교·release는 NOT_RUN이다. products 및 child terminal은 생성되지 않았다.

```sh
/Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/queued_native_final_r2.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2.json queued-final-executor-1 /private/tmp/qidx-xl-mixed-rca-z1a_82he/native-final-retry2-main-context.json
```

원본: `BASE/products-main-20261111-r2/host-queued-final-executor-1/terminal.json`,
`BASE/queued-final-executor-1*`. 뒤의 긴 예약도 이 거절을 성공으로 소급하지 않는다.

<a id="native-final-executor-7200-20261111"></a>
### canonical7200초 단일 예약: host 입장과 제품 preflight를 분리

같은 동결 pair·원7 helper를 유지하고 private `queued_native_frozen.py`만 대기
1..7200초로 파라미터화했다. controller SHA256
`8124ed875774ef0671f7185be7df81cbe4a866da55f919a63ed2cbc779a413b1`,
canonical authority SHA256
`1c90680db39be94a5b5c73caeffdc26bb0c4d05eb79e45d07b559cedd868bc76`다.
이번 supervisor는 OS exit를 수집한다. 앞선900초 시도에 소급하지 않는다.
`current_main_equivalence=NOT_VERIFIED`와 frozen binding guard도 유지했다.

- **입장 VERIFIED:** 05:08:27 KST 단일 예약에서 EX를179.9초 뒤 획득했고
  project resource lock도 허용됐다. 기존 작업·lock·dirty는 변경하지 않았다.
- **preflight BLOCKED/environment:** 05:11:29–05:21:29 KST의 원600초
  preflight 최종 load22.57959/한도8.0, 첫 관측23.228027, foreign Rust0이었다.
  입장 성공은 clean timing 증거가 아니다. `BASE/products-main-20261111-r2/terminal.json`은
  status=BLOCKED, runs=[], controller_completed=false,
  all_products_verified=false, reason=`Original timing preflight refused after both canonical locks`다.
  Large/XL/Mixed body0, Record0이며 formal timing/release는 NOT_RUN이다.
- **종료:** PID4855 05:21:33 KST OS exit1, supervisor 종료, operator lock 해제,
  전/입장/후/최종 frozen custody exit0, 재빌드0/main 생산 변경0이다. 비 Rust 작업도
  표본에 있었으나 단일 표본으로 높은 load의 유일한 원인은 확정할 수 없다.
  Native ACK·검색·재오픈·Mixed 구현 실패로 귀속하지 않는다.

```sh
/Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/queued_native_frozen.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2.json queued-final-executor-7200-1 /private/tmp/qidx-xl-mixed-rca-z1a_82he/native-final-retry2-main-context.json 7200
```

원본 결과 `BASE/native-final-7200-result.json`, controller
`BASE/products-main-20261111-r2/host-queued-final-executor-7200-1/terminal.json`,
preflight `BASE/products-main-20261111-r2/host-queued-final-executor-7200-1/timing-preflight.json`,
OS exit `BASE/queued-final-executor-7200-1-exit.json`이다. 후속 실행은 원
load<=8.0 gate를 필요로 한다. 기존 BLOCKED terminal은 덮어쓴 성공으로
해석하지 않는다.


<a id="native-final-controller-review-20261111"></a>
### Native 최종 실행기 코드 감사 — 2026-11-11

Native 소유5파일은 빌드 후보 `824629e4…`와 바이트가 일치했다. 준비 도중
오류는 `InventoryBatch::flush`가 batch·spool을 무효화하고, 준비 후 미완료
handle 및 물리 설치 receipt는 Drop에서 각각 무효화한다. 이전 focused 회귀의
소스 범위와 이번 제품 본문0 기록을 유지한다.

기존 private 실행기의 두 결함을 격리 subprocess로 재현했다.

- 원600초 preflight의 본문0 BLOCKED terminal도 기존 case 출력과 함께 일괄
  거절했다. 이 때문에 실제 7200초 시도의 다음 실행은 입장 검사에 도달하지
  못한다.
- 제품 leaf exit0 후 post-leaf custody를 실패시키면 terminal은 FAILED인데
  OS exit0·`all_products_verified=true`가 남았다.

BASE의 `queued_native_frozen_v2.py`에서 명시적인 `--resume-preflight`로
같은 source/manifest의 본문0 preflight 기록만 재개할 수 있게 했다. 이전
host terminal과 products terminal의 내용 및 case 디렉터리 부재를 확인한다.
operator lock과 canonical EX 아래에서 기록을 다시 검사하고 새 attempt의
`previous-products-terminal.json`으로 원본 바이트를 보존한 후 원 products
leaf를 실행한다. 새 terminal이 없으면 실패한다. leaf 종료값과 최종 실행기
종료값을 분리하고 후속 custody 오류에서는 성공 플래그를 false로 내린다.

- V2 SHA256 `71e58493c516bf712e50ec4a85214f78fc34c55d4c17fa0a285ec56f461499b1`.
- 실행기 회귀 7/7·exit0. 실제 이전 receipt의 사본으로 archival, 중복/불일치
  거절, 새 terminal 누락, 성공 leaf 이후 custody 오류를 검증했다. 실제 이전
  receipt는 읽기 검증을 통과했다. 원 pair custody도 다시 exit0이었다.
- 읽기 전용 호스트 검사 exit1. 1분 load25.84082, 한도8.0, foreign Rust3개.
  canonical load guard는 load가 한도 이상이면 거절한다. Large/XL/Mixed·Record는
  NOT_RUN·본문0이다. main 생산 수정과 재빌드는 각각0회다.

회귀 명령은 `/Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/test_queued_native_frozen_v2.py`다.
V2 실행기와 회귀는 감사 이력이다. 실제 재개는 아래
[V3 실행기](#native-final-controller-hardening-20261111)를 사용한다.

<a id="native-final-controller-hardening-20261111"></a>
### Native 실행기 session hardening — 2026-11-11

감사 범위는 이 채팅의 private 실행기, 실행 감독기, 재개 회귀 및 F2 기록이다.
main HEAD는 `ade5fadc`, 제품 pair는 동결 `824629e4…`다. Native 소유 5파일은
동결 소스와 바이트가 일치한다.

- **V2에서 도입한 재개 결함.** 본문0 기록을 새 attempt 아래로 이동한 뒤 leaf가
  terminal 없이 종료하면 동일 선행 시도를 지정해도 재개할 수 없었다. 격리
  subprocess에서 첫 시도 종료와 원본 바이트 보존, 두 번째 입장 전 거절을
  재현했다. V3는 원본을 선행 시도의 `products-terminal.json`에 원자적으로
  이동하고, 동일 source/manifest와 선행 terminal을 검증해 그 경로로도 재개한다.
- **V2에서 도입한 하위 판정 결함.** 조건 밖 `continue` 때문에 실제 존재하는
  자식 terminal을 모두 건너뛰었다. source 또는 상위 요약과 불일치하는
  자식으로 exit0이 재현됐다. 자식이 누락되면 거절됐지만 누락 위치가
  기록되지 않았다. V3는 보고된 순서, 각 자식의 존재·run/source/manifest,
  상위 요약과의 일치를 검증한다. case exit는 상위 실행 결과에서 읽는다.
- **Record 호출 복원.** 일치하는 Large/XL Off 실패 자식 중 producer 종료값
  또는 `timed_out=true`가 있는 첫 case만 Record를 호출한다. macOS capture의
  `exit_code=null, timed_out=true`가 빠진 정수 전용 조건은 fixture에서
  Record0으로 재현했다. 최종 실행기는 타임아웃에 Record 한 건, 종료값과
  타임아웃 표시가 모두 없는 시작 실패에 Record0을 기록했다.
- **회귀 보강.** 고정 receipt fixture로 바꿔 실행 중인 제품 경로의 최신
  terminal에 대한 의존을 제거했다. leaf 이후 custody 실패의 실제 하위
  종료값도 기록한다. 실행기 격리 subprocess 회귀 **16/16·exit0**이다.
  fixture가 QBC/leaf를 대체하므로 제품 본문 증거는 별도다.

V3 실행기 SHA256 `e804b1ec09354395c48f85251e47edf5e6f799abf2f9c2e235f326f59947f34d`.
회귀 파일 SHA256 `c2b528f635f8b8bf1c705ce5d4fae2117c205dbb1369b2e6281aa47254426ffe`.

```sh
/Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/test_queued_native_frozen_v3.py
```

실제 7200초 시도의 terminal은 읽기 검사를 통과했다. SHA256
`ae821cb15b0b229cbfe12f8162469394ac790f3cabf7132ffcf8725c2c1b7ce6`이고
3개 case가 모두 NOT_RUN이다. 원 pair custody는 exit0이었다. 실행 시
`queued_native_frozen_v3.py`에 기존 manifest/context와 새 attempt 이름,
승인된 wait seconds 및 `--resume-preflight queued-final-executor-7200-1`를
전달한다. 이전 실행 감독기는 이전 controller에 결속된 이력이다.

원 timing preflight의 읽기 전용 재확인은 load17.460938, 한도8.0으로
exit1·BLOCKED다. Large/XL/Mixed·Record는 NOT_RUN·본문0이고, 이번 보강으로
main 생산 코드나 빌드 pair를 갱신한 건수는 각각0이다.


<a id="native-final-f2-execution-20261111"></a>
### F2 최종 실행 계획 — 2026-11-11

1. 동결 `824629e4` pair와 V3 실행기 결속을 확인한다. 기존 case 및
   본문0 선행 terminal은 V3의 같은 source/manifest 검사로 소비한다.
2. 단일 canonical EX→project lock→원 timing preflight를 통과한 뒤 Large Off,
   XL Off, Mixed Off 순으로 실행한다. Large/XL의120초 ACK·검색·재오픈과
   delta/noop/delete·old view, Mixed의50QPS·1Hz·960초·pin600·renew120/maxgap180·
   settle60을 원 validator로 판정한다.
3. 상위 terminal과 세 자식의 run/source/manifest/요약 및 실제 종료값으로
   수용을 확정한다. 동소스 Large/XL Off 기능 실패가 관측되면 해당 Record
   한 건에서 phase36–39, VFS/Catalog, 설치, 호스트 CPU를 분리해 결함 owner를
   확인한다. main 수정은 확인된 해당 owner의 실패와 회귀에 결속한다.

실행 명령은 아래 한 건이다.

```sh
env -u QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS /Users/songmin/Documents/code-new/semantica-codegraph-v2/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/queued_native_frozen_v3.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2.json queued-final-executor-v3-20261111-1 /private/tmp/qidx-xl-mixed-rca-z1a_82he/native-final-retry2-main-context.json 7200 --resume-preflight queued-final-executor-7200-1
```

착수 시 pair custody는 exit0이었다. 읽기 전용 검사에서 foreign Rust가
1→0, load가80.71582→46.848633으로 변해 단일 예약을 시작했다. 실제 제품
입장은 예약 안의 canonical preflight로 판정한다. V3의 prior terminal 원본은
실제 입장 뒤 선행 시도의 `products-terminal.json`으로 이동한다.


#### V3 실행 결과 — 15:42 KST

- canonical admission lock 획득까지1778.1초. host EX와 project lock을
  획득했고 project lock은600.219초 후 반환됐다. 전/입장/후/최종 custody는
  모두 exit0이다.
- 실제 products terminal은 BLOCKED, runs=[], controller_completed=false다.
  원 timing preflight 600초 종료 시 load18.777832, 한도8.0, foreign Rust0이다.
  Large/XL/Mixed·Record는 NOT_RUN·본문0이다. main 생산 수정·재빌드는0회다.
- leaf exit1, controller terminal의 예정 exit1, 셸 도구의 관측 exit138이다.
  도구 종료값 차이의 원인은 확인하지 못했다. 실제 preflight와 case 부재,
  controller 종료·operator lock 해제는 따로 확인했다.
- V3는 선행 terminal 원본을
  `products-main-20261111-r2/host-queued-final-executor-7200-1/products-terminal.json`에
  보존했다. SHA256 `ae821cb15b0b229cbfe12f8162469394ac790f3cabf7132ffcf8725c2c1b7ce6`다.
  이번 products terminal의 SHA256은 `035fb205d420f964fabafa04db303aa1ef22bd8a38091a7a200828da0f9f8faa`이고 controller terminal은
  `products-main-20261111-r2/host-queued-final-executor-v3-20261111-1/terminal.json`이다.
  위 경로의 기준은 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다.

제품 수용의 잔여 조건은 원 canonical preflight의 clean 판정이다.


<a id="native-f2-clean-resume-uds-path-20261111"></a>
### F2 clean 감지 후 V3 단일 재개 — 17:02 KST

잠금 없이 원 canonical preflight를 관측했다. 17:02:30 KST에 load7.946289,
foreign Rust0·exit0을 확인한 뒤 `queued-final-executor-v3-20261111-1`을
선행으로 지정해 `queued-final-executor-v3-20261111-2`를 한 번 시작했다.
기존 Semantica venv 경로가 열리지 않은 첫 관측 실행은 launch 전 exit127이었다.
설치된 Python3.12.12의 QBC import와 blake3 1.0.8을 확인해 admission Python으로
사용했다. QBC SHA256은 기존 `1c90680d…`와 일치했고, 제품 Python은 build receipt의
경로를 그대로 사용했다.

- **호스트·project admission VERIFIED.** 세 case의 canonical preflight도 모두
  clean이었다. 네 custody 검사(before/admitted/after/final)는 각각 exit0이다.
- **capture 설정 FAILED/integration, 제품 본문 NOT_RUN.** Large/XL/Mixed의
  capture는 모두 exit2였다. `capture.stderr`의 원인은 해석된 Unix 소켓 경로
  112/109/112바이트가 macOS 한도103을 넘는다는 것이다. `execution.json`을 읽던
  제품 실행기의 후속 FileNotFoundError는 이 거절에 따른 결과다. case별 실제
  output root·execution·functional 판정은 모두 없다. 본문0, Record0이다.
- **종료 기록.** leaf exit1, controller terminal 예정 exit1, 부모 Python
  `subprocess.run`으로 관측한 process exit1이다. 이전 실행의 셸 도구 exit138과
  내부 exit1 차이는 원인이 미확인인 별도 이력이다. 이 시도는 operator lock 반환과
  controller PID 종료를 확인했다.
- **경로 수리 검증.** frozen capture의 `realpath`와
  `_preflight_native_socket_path`를 실제 호출했다. 짧은 물리 제품 root로
  디렉터리 alias를 연결한 격리 검증에서 Off/Record 5경로의 길이는68–74바이트였고
  실제 AF_UNIX bind가5/5 통과했다. 실행 전 검사에 이 경로 길이를 포함한다.
  과거 증거를 보존할 archive를 먼저 확보하고 짧은 물리 root를 기존 manifest의
  논리 products 경로에 연결하면 manifest/build/helper 바이트를 유지할 수 있다.
  이번에 승인된 단일 재개는 이미 종료돼 실제 products root는 그대로 두었다.

BASE는 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다.

- clean receipt `BASE/queued-final-executor-v3-20261111-2-readiness.json`.
- 명령·실측 OS exit `BASE/queued-final-executor-v3-20261111-2-exit.json`.
- controller terminal `BASE/products-main-20261111-r2/host-queued-final-executor-v3-20261111-2/terminal.json`.
- 제품 terminal `BASE/products-main-20261111-r2/terminal.json`, SHA256
  `ca3adcbf433151f2828e639f8b8753d146a0423fd754dcdc5e9844a8de2a8043`.
- 선행 원본 `BASE/products-main-20261111-r2/host-queued-final-executor-v3-20261111-1/products-terminal.json`,
  SHA256 `035fb205d420f964fabafa04db303aa1ef22bd8a38091a7a200828da0f9f8faa`.


<a id="native-f2-short-products-root-20261111"></a>
### F2 물리 제품 경로 복구 — 17:06 KST

BASE는 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다. 이전 controller 종료와
operator lock을 확인하고 기존 `BASE/products-main-20261111-r2`를 통째로
`BASE/products-main-20261111-r2-preserved-v3-2`에 rename했다. 80개 entry의 종류와
파일 SHA256은 이동 전후 일치한다. **기존 문서의 시도별 증거를 읽을 때는 기존
products prefix를 보존 경로로 바꾸고 나머지 상대 경로를 그대로 사용한다.**

manifest의 기존 logical products 경로는 지금 새 실디렉터리 `BASE/p3`를
가리킨다. 실제 `realpath`로 원 socket guard를 적용한 결과 Large Off/Record,
XL Off/Record, Mixed Off는 각각89/92/86/89/89바이트다. manifest SHA256
`57e0b03c0d113cf6700e7c284c90795103167d39dc0572b1a1170729f01e262a`와
source/build/원 helper는 기존 파일을 소비한다. 바이트 보존과 경로 매핑은
`BASE/native-f2-short-root-relocation.json`에 결속했다.

새 시도 `queued-final-executor-v3-20261111-3`은 비어 있는 제품 출력에서 시작한다.
원 timing guard의 clean을 읽기 전용으로 확인한 뒤 V3를 한 번 실행한다.


<a id="native-f2-prebuilt-physical-root-20261111"></a>
### F2 짧은 실경로 재결속 및 실제 Large 결과 — 17:19 KST

- **V3-3의 alias 경로는 BLOCKED/integration이다.** socket guard는 realpath를
  허용했지만 `HostMonitor.start`의 실제 `RawWriter`가 products 경로의
  symlink를 `O_NOFOLLOW`로 거절했다. EX·project·Large timing preflight는
  통과했다. Large/XL/Mixed 본문0, Record0, 실측 OS exit1이다. 이 증거는
  `BASE/p3`와 기존 logical alias에 보존했다. prior V3-2는
  `BASE/products-main-20261111-r2-preserved-v3-2`에 있다.
- **실경로와 재결속 VERIFIED.** `BASE/rail-main-20261111-r2-p4.json`의
  변경 필드는 products=`BASE/p4` 및 build=`BASE/build-native-f2-prebuilt-p4`
  두 개뿐이다. 메타데이터 inputs는 원 파일에서 rail hash만 바뀌었다.
  파생 terminal에 원 manifest/inputs/terminal SHA, 동일 runner·두 binary
  identity, compiler invocation0을 기록했다. `verify_native_main.py`의 원
  manifest 및 파생 manifest 검사 모두 exit0이다. 원 SHA256은
  `57e0b03c0d113cf6700e7c284c90795103167d39dc0572b1a1170729f01e262a`,
  파생 SHA256은
  `de468f176d630b253cc8b4b36630a067ba7752db91450e0dc9cdb5e48fdbcccc`다.
  Off/Record 다섯 경로는 원 socket guard86–92바이트와 격리 실제
  `HostMonitor.start/phase/finish` 및 AF_UNIX bind5/5를 통과했다.
- **V3-4 Large Off VERIFIED.** producer exit0·case exit0·
  PRODUCT_ARTIFACT_VERIFIED다. 전체 producer wall45.232초, full ACK27.281초,
  첫 검색43.854ms, 재오픈1303.108ms, 재오픈 후 검색50.152ms다.
  delta/noop/delete와 old pin, 삭제 후 재오픈 검증도 같은 artifact의 원
  validator를 통과했다. source는 동결824629e4이며 post dirty=false다.
  summary SHA256은
  `fabd87765fa5f0a366433b21bb9a697a327cc9b3cec1b85b570134a9ed552c76`다.
- **V3-4 F2 BLOCKED/environment.** XL의 개별 timing preflight는
  load11.762695/8.0, foreign Rust0으로 거절됐다. XL 및 Mixed 본문0,
  Record0이다. 실제 runs는 Large VERIFIED, XL BLOCKED 순서이며 Mixed는
  NOT_RUN이다. leaf/예정 controller/실측 OS exit가 모두1이고 전/입장/후/최종
  custody는 모두0이다. controller 종료와 operator lock 반환을 확인했다.
  원 formal timing·release는 NOT_RUN이다.

BASE는 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다. 원 build receipt와
실행 전용 provenance는 각각 `BASE/build-main-20261111-r2/terminal.json`,
`BASE/build-native-f2-prebuilt-p4/terminal.json`이다. 실측 명령 및 OS exit는
`BASE/queued-final-executor-v3-20261111-4-exit.json`, 실제 제품 terminal은
`BASE/p4/terminal.json`, Large artifact는
`BASE/p4/n2-large-off/artifact/summary.json`, XL gate는
`BASE/p4/n2-xl-off-control/host-preflight.json`이다.

현재 gate 흐름은 전체 sequence 전에만 clean을 기다리고, 각 case 앞의
별도 검사에서는 즉시 차단한다. 이번 기록은 다음 case의 clean 조건이
독립적으로 필요함을 보여준다. 동일 상태의 추가 실행은 만들지 않았다.


<a id="native-f2-continuation-review-20261111"></a>
### XL→Mixed 이어가기 정적 리뷰 — 2026-11-11

활성 단일 이어가기 `continuation-xl-mixed-20261111-1`은 원 실행기 SHA256
`98a280a33702c6aa772cb002ded82321870e89d98e16c07bc63ba8a159c39b21`로 읽기 전용 preflight를 관측 중이었다.
동결 source `824629e4…`와 P4 manifest `de468f17…`를 그대로 소비한다.
현재 제품 경로와 실행기 파일은 건드리지 않았다.

- **실패 우선순위.** 기존 `leaf`는 XL 실제 실패 뒤 Mixed가 차단되면
  집계를 BLOCKED로 바꿨다. 격리 회귀에서 기존판의 실패를 재현했다.
  보강판은 같은 입력에 FAILED를 반환하고 XL Record 결과를 별도 유지한다.
- **부분 재개와 원본 증거.** 기존 `bound_prefix`는 원 XL 차단 기록이 live에
  있어야 하므로 기록 이동 직후 종료와 XL 성공 이후 Mixed-only 재개를
  거절했다. 보강판은 기존 집계, Large 증거, 원 XL preflight의 명시 경로와
  SHA를 확인한다. 원 XL 차단 기록을 선행 시도 아래 보존하고 종료 후에도
  다시 검증한다. 검증된 XL은 지정한 이전 완료 terminal·실제 하위 결과와
  결속한 경우에만 재사용한다. 기존 case 본문이 있으면 재시작을 거절한다.
- **원본 artifact와 예산.** 새 성공 case에서 실제 execution, summary,
  tier manifest, host observations, child terminal을 다시 대조한다.
  source, pair hash, tier, seed, mode를 확인하고 최종 receipt에 SHA를 남긴다.
  project timeout은 최대3개 case의 기존 cooldown 600초와 manifest 실행
  예산, 기존 정리 여유600초를 합한5400초다. case별 원 기준은 동일하다.
  각 canonical probe는15초 timeout과 명시적 진단을 갖는다.

격리 보강 사본과 적용 전 diff는
`/private/tmp/qidx-xl-mixed-rca-z1a_82he/continuation-review`에 있다.
보강 실행기 SHA256 `d69b722210f033d4e253a63e6611faf143b02be2713f57690b8aea69ce387d72`, 회귀 파일 SHA256
`5fc980931ec367e8df9fb0b4e32728e52a1f308175a0788e3915fb37de9274ab`다. 회귀9/9·exit0, 실제 P4
Large prefix 읽기 검증 exit0, `git apply --check` exit0이다. 격리
회귀가 제품 본문을 대체하므로 제품 실행 증거로 사용하지 않는다. 같은
시점의 기존 단일 실행이 파일을 다시 읽을 수 있어 실행기 교체와 추가
예약은 하지 않았다. V3-4 aggregate BLOCKED 원본은 유지한다.


<a id="native-f2-continuation-promoted-20261111"></a>
#### 보강판 승격 — 17:56 KST

operator lock을 비차단 획득한 상태에서 이전 PID83339의 명령과 읽기 전용
대기, XL/Mixed/Record 본문0을 확인하고 해당 관측만 SIGTERM으로 종료했다.
이전 terminal은 host/project 입장 false, BLOCKED, 내부 `SystemExit(143)`을
기록했고 감독 프로세스의 관측 종료값은1이다. 원본 실행기/회귀는
`BASE/continuation-review/original`에 보존했다.

패치를 단일 실행기 경로에 적용하고 `test_continue_native_p4.py`의9/9·exit0을
재확인했다. 새 단일 시도 `continuation-xl-mixed-20261111-2`는
`--resume-from continuation-xl-mixed-20261111-1`로 시작했다. 기존 P4 Large
증거와 824629e4 pair를 사용하며, 입장 전 custody는 exit0이다.

```sh
/Users/songmin/.local/bin/python3.12 -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/continue_native_p4.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2-p4.json continuation-xl-mixed-20261111-2 --resume-from continuation-xl-mixed-20261111-1
```

새 시도의 첫 읽기 전용 검사 load24.024902/8.0·foreign Rust2로 BLOCKED다.
이 관측에는 EX/project lock이나 제품 본문 실행이 없다. 원 aggregate와
기존 Large 결과는 보존한다.

<a id="native-f2-acceptance-oracle-review-20261111"></a>
#### F2 판정 경계 재검토 — 2026-11-11

- **ACK 시간.** 동결 제품 validator는 phase 완료와 유한한 음이 아닌 wall time을
  요구한다. `full_event_ack.wall_ms <= 120000`은 자동 판정에 없다. 서버의
  120초 dispatch는 queue 이후 적용되고, harness는 600초 client 예산에서
  original-key 상태를 조회해 publication을 확정할 수 있다. F2의 120초 ACK는
  capture 성공과 함께 원본 summary의 해당 phase 실측값을 직접 확인한다.
  Large의27.281초·`immediate_ack`는 원본에서 확인했다.
- **검색과 Mixed.** producer는 재오픈된 새 daemon의 Latest를 최종 ACK view와
  대조하고 검색 결과를 검증한다. Mixed validator는 50QPS·960초, pin600초,
  마지막 pin 해제·floor 및 첫 해제 시도부터 60초 이내 정산을 요구한다.
- **비정상 중단 후 재개.** 보강 실행기는 정상 BLOCKED terminal에서 성공한 XL을
  재사용한다. XL 종료 뒤 Mixed 진행 중 상위 실행이 취소되면 case exit가 담긴
  완료된 BLOCKED 기록이 없을 수 있다. 기존 XL 산출물은 보존되지만 현재
  자동 재개는 그 경계를 거절한다. 실행기에서 case 종료 증거를 다음 case 전에
  저장하면 해당 중단에서도 검증된 XL을 재사용할 수 있다.


<a id="native-f2-continuation-ack-proof-20261111"></a>
#### 원 ACK 기준과 재개 증거

동결 source의 `validate_native_product_artifacts`에 P4 Large summary와 tier
manifest 원본을 다시 공급했다. 원 manifest의 tier/seed/예산·daemon SHA로
검증해 exit0이었다. full event ACK의 실제 wall은27281.457875ms,
completion은 `immediate_ack`, submit outcome은 `terminal_published`이고
status 재조회는0회다. 원120000ms wall 조건을 충족한다.

원 capture는 phase 완료와 wall 유효성을 검사한다. 서버 dispatch의120초
예산 및 client600초 timeout과 실제 ACK wall은 별도 관측값이다. XL 결과가
들어오면 `detail.phases.full_event_ack.wall_ms <= 120000` 및
`detail.publication_outcomes`의 실제 completion을 원본에서 확인한다.

이어가기는 이전에 완료된 continuation terminal에 결속된 XL을 재사용한다.
현재 실행기에는 case 직후 프로세스 exit를 독립적으로 발행하는 checkpoint가
없다. XL 완료 후 Mixed 도중 최종 leaf terminal 없이 종료하면 XL artifact를
보존해도 자동 이어가기는 막힌다. 이 경우 기존 body를 실행 대상으로 다시
만들지 않는다. 진행 중인 실행기 파일은 이번 관측에서 수정하지 않았다.

<a id="native-product-hot-path-audit-20261111"></a>
#### Native 제품 경로 정적 재감사 — 2026-11-11

main `ade5fadc1`과 동결 `824629e4`의 settlement/reclaim/retention,
SourceApply/attempt reads, FileObjectStore 및 Lexical 진입 파일을 직접 대조했다.
감사 대상 핵심 10개 파일은 바이트가 일치한다.

1. **Mixed 정산 확장성.** `observe_read_settlement`는 공유 Catalog mutex와
   DEFERRED snapshot을 유지한다. 차단 조건이 없을 때 두 등록 테이블의 모든
   객체를 순회하며 lifecycle kind를 point lookup하고, 전체 lifecycle을 다시
   순회하며 row integrity와 최대 11개 참조 테이블을 확인한다. 256행 cap은
   한 페이지 크기다. 동일 정산 RPC의 작업량과 mutex 점유는 전체 객체 수에
   비례하고, held worker는 예정 tick 기준 2초 예산으로 호출한다. 최종
   debt-zero 검사에 도달한 동일 후보 측정은 아직 없다.
   Catalog reclaim/settlement 담당에서 같은 snapshot의 검증을 페이지 단위
   집합 조회로 묶어 point lookup을 줄인다. 큰 catalog에서도 2초 안에 끝날
   근거가 부족하면 reclaim의 bounded 검증 결과를 durable 변경 경계에
   결속한다. orphan·누락 lifecycle·동시 publication은 기존처럼 정산을
   거절해야 한다. 정상 객체 수만 늘린 고정 debt fixture에서 SQL/hold-time
   곡선과 정산 결과를 검증한다.
2. **Mixed retention의 빈 writer transaction.** 20ms retention 설정은
   Work/Views/Custody 각 라운드 앞에서 `retain_exact_history_tail`을
   호출한다. head와 floor가 같으면 `write_floor`는 SQL UPDATE를 생략하지만,
   그 앞에서 공유 mutex와 `BEGIN IMMEDIATE`를 잡고 마지막 COMMIT까지 간다.
   이 설정의 이상적 호출 상한은 초당 50회다. 실제 rate와 지연 기여율은
   미측정이다. Catalog retention 담당에서 동일 snapshot에 head/floor를
   확인하는 read fast path를 두고, 변경 시에만 write transaction을 열어
   head/epoch/floor를 재확인한다. 기존 no-op writes0 회귀에 실제 writer
   reservation0과 다른 연결의 head advance 경쟁을 추가해 검증한다.
3. **XL ACK의 SourceApply→Lexical 반복 읽기.** SourceApply에서 body get과
   UTF-8 검증을 마친 뒤 Lexical cold 경로가 같은 body를 get·UTF-8 검증한다.
   최대 4MiB attempt window에는 source-record용 2MiB가 있다. 과거 XL 입력은
   약 113MB였다. window가 전체 body를 보유할 수 없으며 최신 physical
   hit/miss 분포는 없다. 기존 SourceApply/BuildPrepare span에서 실제 get,
   retained hit와 bytes를 분리한다. 반복 비용이 확인되면 같은 attempt의
   ref·length·admission 검증 결과를 소비 경계에 연결하되, 임의 ingress
   참조와 재시작 경로의 검증을 유지한다.

Posting은 bounded batch로 조회·쓰기하고 E의 중간 Catalog confirm+reserve는
이미 한 COMMIT이다. Catalog confirm의 최대 256개 input uniqueness 사전
검사는 최악 32,640 비교 뒤 authoritative preflight를 거친다. 이는 계측
후보로 두며 `RegisteredObject::clone`은 Arc 복제이므로 children 전체
복사로 계상하지 않는다. StageSources의 추가 복사와 identify/prepare 해시는
기존 staging 항목이며 SourceAdvance 120초 안에 포함하지 않는다.
r55의 publisher 준비→ACK 중앙 3.227초는 다른 소스의 부분 결과다.
이번 감사의 빌드와 제품 실행은 `NOT_RUN`이다.

<a id="native-product-hot-path-repair-20261111"></a>
#### Native 제품 경로 후속 수리

- **Retention.** 동일 head/floor를 DEFERRED 스냅샷에서 확인한다. 변화가
  필요할 때 IMMEDIATE 트랜잭션을 얻어 head·epoch·floor를 다시 읽고
  검증한다. 다른 WAL 연결이 실제로 writer 예약을 잡은 동안 기존 floor를
  반환하는 회귀를 기존 pin/단조성 테스트에 추가했다.
- **정산.** 이미 읽은 GC 페이지에서 LIVE row를 검증한다. 객체당 shape와
  body를 다시 찾던 SQL 두 문장을 없애고 공통 lifecycle digest 검증과
  기존 per-row charge를 유지한다. 실제 scan의 정상·budget 거절·손상 digest
  회귀를 확인했다. 등록 coverage 및 exact reference 조회는 선차감과
  오류 순서를 보존한다. 여전히 전체 객체 수에 비례하는 경로다.
- **XL.** 기존 `NativeSourceApply`와 `BuildPrepare`에 object-port get과
  retained 호출·성공·성공 wire bytes를 Record에서 집계한다. 논리 work
  fee와 물리 장치 I/O를 이 수치로 대체하지 않는다. Off에서 tally를 켜지
  않으며 body 검증·cache 한도는 유지한다. v5 timeline은 올바른 span의
  계측만 소비하며 완전한 capture의 부분 counter family를 거절한다.

집중 Rust nextest는 Catalog·Kernel·Searchd의 19/19·exit0이다. 테스트에는
두 번째 SQLite writer가 예약을 보유한 no-op, 실제 settlement scan의
손상/예산 거절, retained DATA의 identity·취소와 counter 결과가 포함됐다.
`native_source_rebase`의 `source_trace::` 통합 회귀는 3/3·exit0이다.
기존 테스트는 Literal의 중첩 BuildPrepare를 Kernel 최상위 단계와 함께
세었다. 최초 실행에서 이 중복 집계가 재현됐고, Kernel lane 3과 실제
부모 span으로 검증 범위를 고쳐 Off/Record work 동등성과 카운터를 확인했다.
`test_causal_cost_timeline.py`는 543/543·exit0이다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog -p quanta-index-kernel -p quanta-index-searchd --lib -E 'test(/exact_tail_policy_is_atomic_monotone_and_preserves_existing_pin|read_settlement_|attempt_reads::tests::retained_read_|trace_wire::tests::/)' --no-tests fail --test-threads 1 --no-fail-fast
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-kernel --test native_source_rebase -E 'test(/source_trace::/)' --no-tests fail --test-threads 1 --no-fail-fast
PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -m pytest tools/ci/tests/test_causal_cost_timeline.py -p no:cacheprovider -q
```

F2 동결 pair는 이 소스 변경과 다른 실행 입력이다. 원 120초 ACK·검색·재오픈,
Mixed pin·정산과 변경 후 성능 비교는 이 집중 회귀로 판정하지 않는다.

<a id="native-index-rca-tools-20261111"></a>
### Large 최초 인덱싱 RCA 및 재현 없이 확인하는 진단

P4 Large Off (`824629e4`, 4,096파일, 8,609,531B)의 `artifact/summary.json`과
`execution.json`에서 첫 staging 8.222280초, 최초 ACK 27.281458초, 첫 검색
43.854ms, 재오픈 1.303108초, 재오픈 검색 50.152ms를 확인했다. staging과 ACK
두 단계의 합은 35.503738초이고 이 중 ACK가 76.8%다. 120초 기준은 원 요청 ACK에
적용된다. staging과 phase observer 시간은 이 기준에 합산하지 않는다. 최초 ACK는
원 submit에서 `terminal_published`/`immediate_ack`로 완료됐고 status check는 0회다.

- **staging:** 원 harness는 128개씩 32회 동기 `StageSources`를 호출한다. Kernel은
  매 페이지에서 입력·UTF-8 검사, body/record identity와 payload 준비, custody
  설치 및 반환 identity 검증을 한다. 이 실행의 8.22초는 전체 호출의 벽시계다.
  준비·IPC·설치·COMMIT의 각 비중은 P4에서 관측하지 않았다.
- **ACK:** staging 완료 후 변경 목록을 만들어 SourceAdvance로 보낸다. SourceApply,
  BuildPrepare/검색 member, publication 단계가 이 경로에 있다. P4에는 daemon
  child trace가 없어 27.28초 중 각각의 기여도는 미관측이다. 원인 후보를 구분하는
  과거 XL Record (`6ffa`, 32,768파일, deadline 실패)에서는 SourceApply 30.381초,
  BuildPrepare 86.267초를 관측했다. 설치 18.980초·Catalog COMMIT 10.512초는
  InventoryPrepare 아래, Posting 14.989초는 Diff 아래였다. 부모·자식 시간은
  합산하지 않으며 이 XL 수치나 wall/CPU 차이를 P4의 비용 또는 host 경합으로
  직접 전용하지 않는다.

원 harness의 `full_stage_sources`가 Record일 때만 페이지 `begin`과 정상 `ack`를
별도 stderr marker로 남긴다. 순서·건수·경과 시간만 기록하고 Off는 추가 진단 clock과
stderr 쓰기를 하지 않는다. `ack`는 StageSources에서 참조를 받은 사건이다. 제품의
SourceAdvance ACK 또는 검색 가능 시점과 구분한다. 기존 qtrace는 새 이벤트 없이
요청 root와 자식 span으로 집계한다. 타임아웃 prefix도 읽고, 열린 span의 최종 wall과
누락된 계수는 `null`로 남긴다. 별도 capture의 시계는 정렬하지 않는다.

저장된 capture 디렉터리가 `$RUN`일 때 두 명령 모두 제품 재실행 없이 동작한다.

```sh
.venv/bin/python -m tools.benchmark.retrieval.causal_cost_diagnostic \
  native-staging --input "$RUN/stderr"
.venv/bin/python -m tools.benchmark.retrieval.causal_cost_diagnostic \
  native-ingest --input "$RUN/causal-timeline.jsonl.native-large-children/first.jsonl" --limit 5
```

`native-staging`의 `open_page`는 시작 marker 뒤 ACK를 관측하지 않은 페이지,
`slowest_ack`는 ACK가 남은 페이지 중 최장이다. 옛 실행이나 Off는 `NO_MARKERS`다.
`native-ingest`의 `requests[].stages`에서 SourceApply/3·BuildPrepare/3, inventory·posting,
`managed_object_install/*` 및 `sqlite_work/3` COMMIT의 wall union을 확인한다. 설치와
SQLite는 가장 가까운 단계별 wall union도 제공한다. CPU와 계수가 실제로 발행된 경우에만
합계를 내며 논리 WorkBudget과 object-port 호출/성공 wire bytes를 별도로 보여준다.
BuildPrepare만 보이는 요청에는 초기 설정도 포함될 수 있다. 최초 SourceAdvance를 볼 때는
`native_source_apply/3`가 관측된 요청을 확인하고, 가장 긴 요청만 보고 단정하지 않는다.
XL 입력은 경로의 `native-large-children`을 `native-xlarge-children`으로 바꾸고 파일이
기본 128MiB보다 크면 실제 capture 한도로 `--max-capture-bytes`를 명시한다.

실패 시 제품 `execution.json`의 `exit_code`·`timed_out`과 daemon `first.stderr`의
`native_work_refusal`을 먼저 확인한다. 이후 닫힌 비용 구간과 열린 작업을 분리하고,
`host-preflight.json` 및 Off와 동일 조건일 때만 경합·기록 비용 비교에 사용한다.
진단 출력만으로 SourceAdvance ACK, 제품 수용 또는 빌드 결속을 판정하지 않는다.
새 변경 후 실제 Record/Off 제품 검증 및 기록 오버헤드 비교는 별도 범위다.

**VERIFIED:** Python timeline·diagnostic 및 신규 회귀 묶음 605/605, Ruff lint/format,
Rust harness crate check와 해당 파일 rustfmt. 기존 `x1r` XL child trace를 도구로 다시
읽어 `PARTIAL_DIAGNOSTIC`, 요청 2건 중 열린 요청 1건(`44255`), 열린 SourceApply의
`wall_ns`/`union_ns=null`과 계수 미관측을 확인했다. P4의 기존 Off stderr는
`NO_MARKERS`, 마지막 수용 건수 `null`로 출력했다. XL 제품을 재실행하지 않았다.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1 ./scripts/cargow --lane test-workspace-lane \
  check --locked --offline -j 2 -p quanta-index-searchd-harness --lib
PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_causal_cost_native_ingest.py tools/ci/tests/test_causal_cost_native_staging.py \
  tools/ci/tests/test_causal_cost_diagnostic.py tools/ci/tests/test_causal_cost_timeline.py
```

**NOT_RUN:** 최신 소스의 Record 제품 실행, staging stderr의 실제 생산 캡처,
quiet-host Off/Record 오버헤드 및 제품 수용. Rust check는 실행 증거가 아니다.

<a id="native-run-triage-20261111"></a>
### 저장된 실행 폴더에서 실패 지점 확인

```sh
.venv/bin/python -m tools.benchmark.retrieval.causal_cost_diagnostic native-triage --run-dir "$RUN"
```

`$RUN`에는 실행 폴더나 대응 `-control` 폴더를 지정한다. 기본 출력은 `execution.json`,
호스트 사전 검사, SHA-256이 일치하는 제품 summary 또는 같은 소스의 실패 prefix를 읽는다.
daemon 거절은 실패한 게시의 전체 operation key가 일치할 때 해당 실패에 귀속한다.
staging 로그 끝이 잘렸으면 마지막 정상 ACK와 열린 페이지를 보존한다.

기존 결과에서 다음 세 판정을 확인했다.

- P4 Large Off는 `PRODUCT_VERIFIED`를 표시하고 staging 8.22초, 원 ACK 27.28초,
  첫 검색과 재오픈 검색을 보여준다. daemon trace는 `OFF`다.
- 기존 `x2r` XL은 `WORK_REFUSED`다. `full_event_ack` 안의 `BuildPrepare`가 작업
  deadline으로 거절됐고, 원 operation 2의 checkpoint는 당시 `native/text.rs:9413:16`이다.
  기록된 호스트 사전 부하는 30.58이다. checkpoint는 거절이 관측된 위치다.
- P4 XL의 `-control` 폴더는 `HOST_PREFLIGHT_BLOCKED`다. 부하 11.76이 한도 8.0을 넘었고
  producer 실행 기록이 없다.
- 기존 `x1r`의 `--trace`는 672,235개 기록, 미기록 1,548,050개와 열린 span 5개를
  표시했다. 마지막으로 보이는 요청 단계는 SourceApply였지만 같은 operation의 daemon
  거절은 BuildPrepare에서 발생했다. trace의 유실과 거절 위치를 함께 확인해야 한다.

요청 내부 비용이 필요할 때 `--trace`를 추가한다. 기본 재생 한도는 128MiB다. 더 큰 파일은
출력에 표시된 바이트 수를 확인해 `--max-capture-bytes`를 지정한다. `--json`으로
자동화에 연결할 수 있다. `native-ingest`는 개별 span group의 상세 값을 제공한다.
triage 명령의 종료 코드 0은 보고서 생성 성공을 나타낸다. 제품 상태는 출력의 `status`와
저장된 execution의 `product_status`에 있다.

**VERIFIED** — 기존 성공·실패·사전 차단 폴더를 실제 CLI로 확인했다. `x1r`은 daemon
trace를 재생했고 338,966,901바이트의 `x2r`은 기본 제한에서 `OVER_LIMIT`로 표시했다.
timeline·diagnostic·ingest·staging·triage 회귀 611개와 Ruff 검사를 통과했다.

<a id="native-cold-repeated-work-20261111"></a>
### 최초 게시·staging의 반복 작업 제거

- 최초 SourceAdvance의 owner·path 검증에서 빈 `MapRoot(None)`도 storage scalar
  lookup을 호출한다. 기존 구현은 빈 root의 부재를 반환하기 전에 키 길이의 두 배만큼
  임시 nibble 버퍼를 할당하고 채웠다. 공통 `lookup_key`가 기존 checkpoint, 키 길이
  검사와 논리 bytes 청구 직후 빈 root를 반환한다. 기존 nonempty 탐색과 SourceApply의
  scratch·충돌·경로 검증은 동일 경로를 사용한다. 빈 root에 대한 물리 객체 읽기는 기존에도 0이었다.
- 원 scale staging의 128개 페이지에서 본문 Vec를 파일마다 복제했다. 이미 corpus에 있는
  본문을 `stage_sources_borrowed`로 전달한다. 두 SDK 입구는 같은 encoder와 응답
  검사로 합류하고, 페이지별 경로·예산·ACK 검사는 기존대로 적용한다.
- triage는 summary와 `tier_manifest.json`의 원본 해시를 모두 `execution.json`과
  대조한다. manifest 누락·변조나 summary의 소스·tier 불일치에서 `ARTIFACT_MISMATCH`를
  반환하고 phase 값을 숨긴다. 실제 P4 artifact 복사본에서 manifest 끝에 한 바이트를
  추가했을 때 `PRODUCT_VERIFIED`에서 `ARTIFACT_MISMATCH`로 바뀌었다.

**VERIFIED** — Python 진단 회귀 613개, 실제 P4 정상·manifest 변조 확인, 변경 파일의
Ruff·rustfmt 및 `git diff --check`.

**BLOCKED** — Storage·SDK·harness 집중 nextest는 공유 build-test lock에서 15초와
30초 대기 후 exit124로 입장하지 못했다. 이 두 시도의 테스트 본문 실행은 0이다.
[후속 집중 실행](#native-path-and-mixed-diagnostics-20261111)에서 같은 범위를 검증했다.
재개 시 Nextest의 `-j`가 테스트 스레드 옵션이라는 점을 확인해 빌드 병렬도는
`CARGO_BUILD_JOBS=2`로 지정했다.

원 XL 제품 timing은 기존 이어가기 호스트 검사에서 **BLOCKED**다. 23:16 KST 기록의
1분 부하는 23.32, 한도는 8.0이며 다른 Rust 프로세스가 5개 있었다. 두 최적화의 시간
기여도와 변경 후 원 120초 수용은 미측정이다. 과거 XL의 BuildPrepare deadline은
이 두 제거 항목의 지배 비용을 입증하지 않는다.

<a id="native-path-and-mixed-diagnostics-20261111"></a>
### SourceApply SQL 준비 재사용과 Mixed 실패 진단

- SourceApply 경로 scratch는 각 path row의 `expected`/`next`를 UPDATE할 때마다 같은
  SQL을 다시 준비했다. 페이지 조회 두 문장과 UPDATE를 연결의 statement cache에서
  재사용한다. 원 행별 reserve, transaction, 파라미터, 오류 순서는 유지한다. 기존
  swap/duplicate 회귀에 다음 행의 `NULL` 바인딩 확인을 넣었다.
- 빈 map 배치 조회도 단건과 동일한 키 준비 경로를 쓴다. 원 결과 배열 및 key bytes
  청구와 물리 키 제한은 유지하고 임시 nibble backing만 생략한다. 기존 배치 회귀에서
  임시 메모리 거절 scope의 빈 root 결과와 원 논리 청구를 확인한다.
- 기존 Mixed `m2` (`18f6f89d`)는 마지막 pin이 해제됐지만 60초 동안 61회 polling 후에도
  정산 상태가 Pending이었다. 마지막 반환 상태의 head/floor는 318/318이며 target pin은
  없었다. 같은 target sequence 5의 stderr 표본에는 `UnretiredView`, `TerminalCustody`,
  `ReclaimQueue`가 있다. 마지막 로그 표본의 head는 310이고 `TerminalCustody`의
  oldest ready sequence는 27,342다. 표본의 마지막 로그와 마지막 polling 응답은
  별도 기록이다. 이 실행 이후 retention·reclaim이 변경돼 최신 제품에서의 재현은 미확인이다.

`native-triage --run-dir "$RUN"`에 Mixed 실패 주체, pin 해제, 정산 poll/head/floor,
같은 target sequence의 차단 표본이 표시된다. stderr 표본은 `execution.json`에 기록된
해시가 일치할 때만 사용한다. 제품 trace와 달리 Off 실행에서도 보관된 stderr를 읽는다.
이 실행 시점의 Record staging wall에는 페이지 시작/ACK stderr 출력 시간이 포함됐다.

**VERIFIED** — Python 회귀 615개와 실제 `m2` 폴더의 triage JSON, Ruff·rustfmt.
Kernel·Storage·SDK·harness 집중 nextest `6634330f-c486-4a37-a475-491188465bf8`은
7/7 통과, exit0이다. 공유 잠금이 풀린 후 이전에 막혔던 단건 lookup·borrowed SDK
wire/UDS·harness failure 기록과 이번 path UPDATE·배치 lookup 회귀를 함께 실행했다.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=75 ./scripts/cargow \
  --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel -p quanta-index-storage -p quanta-index-sdk \
  -p quanta-index-searchd-harness --lib \
  -E 'test(/path_claims_preserve_swap_identity|empty_scalar_lookup_keeps_key_admission|batch_lookup_matches_independent_values|late_stream_conflict_or_duplicate|borrowed_source_batch|actual_refusal_writer_preserves_child_raw_paths/)' \
  --no-tests fail --test-threads 1 --no-fail-fast
```

원 XL 120초·최신 Mixed 성능 수용과 이 최적화의 시간 기여도는 이 진단에 포함되지 않는다.

<a id="native-trace-custody-and-stage-time-20261112"></a>
### 최초 인덱싱 측정과 실행 기록 결속 재감사

- cold harness가 `full_event_ack` 전에 전체 `OwnerMutation` 배열을 만들었다. 기존 SDK의
  128행 페이지 입력으로 순서대로 공급한다. owner ordinal, object 참조와 ACK 경계는 동일하다.
- Record staging의 동기 stderr marker 쓰기를 phase `observer_ms`에 따로 계상한다. 페이지
  elapsed와 `wall_ms`는 측정된 marker 쓰기를 제외한다. 원본 harness span과 전체 실행 벽시계는
  실제로 경과한 구간을 유지한다. triage는 wall과 observer 시간을 같이 표시한다.
- daemon child trace는 파싱 상태와 실행 결속을 각각 표시한다. 실행 기록의 capture ID, 경로,
  크기, SHA-256이 모두 일치할 때만 `BOUND_*`를 표시한다. metadata가 없는 실패 기록은 footer가
  complete여도 `UNBOUND_DIAGNOSTIC`이며, 불일치 trace의 요청 비용은 표시하지 않는다.
- fixture의 operation key는 서로 다른 실행에서 재사용된다. 작업 거절을 `WORK_REFUSED`로
  귀속하려면 전체 key 일치와 실행에 기록된 stderr SHA-256 일치가 모두 필요하다. 기존 XL
  `x2r`의 해시 일치 harness stderr에서 `BuildPrepare` deadline을 다시 확인했다.

정적 감사에서 Mixed terminal custody의 낮은 순번 재방문·부분 해제·재시도 경로를 확인했다.
과거 m2는 현재 소스의 생산 결함 재현으로 분류할 수 없다. XL의 SourceApply→Lexical source
body 재읽기와 객체별 namespace 재검사는 현 코드에 남아 있다. 단일 최신 Record의 실제
get/retained bytes와 설치 phase가 각 비용의 기여도를 판단하는 다음 증거다.

**VERIFIED** — harness 집중 nextest `fe6a83f0-427a-40a2-8657-7b0c3d344052` 4/4,
Python timeline·diagnostic·triage 회귀 617/617, Ruff·rustfmt·diff 검사. 보관된 Large Record
`lr29`의 674,977개 이벤트를 실제로 재생해 `BOUND_COMPLETE_DIAGNOSTIC`과 요청 5개를
확인했다. 기존 실패 `x2r`는 결속된 harness stderr의 동일 작업에서 deadline을 재확인했다.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=60 ./scripts/cargow \
  --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-searchd-harness --lib \
  -E 'test(/stage_marker_time_is_charged|cold_mutations_preserve_owner_identity|actual_refusal_writer_preserves_child_raw_paths|native_record_child_receives_selected_output_cap/)' \
  --no-tests fail --test-threads 1 --no-fail-fast
```

**BLOCKED** — 기존 담당의 `continuation-xl-mixed-20261112-1`이 제품 입장 조건을 관측
중이다. 00:14 KST 표본은 1분 부하 16.06으로 한도 8.0을 넘었다. 최신 원 120초 XL과
Mixed 제품 수용은 아직 실행 증거가 없다.


<a id="native-f2-continuation-resume-20261112"></a>
#### 중단된 관측 재개 — 2026-11-12 00:13 KST

`continuation-xl-mixed-20261111-2`의 supervisor94825/controller94826은 종료됐다.
기록에는 마지막 읽기 전용 preflight만 있고 admission status, leaf terminal,
controller terminal 및 실측 process exit는 없다. 마지막 관측은
00:09:43 KST, load7.619141/8.0, foreign Rust2였다. 도구 세션 중단과
프로세스 종료를 확인했으나 해당 프로세스의 OS exit나 신호 번호는 알 수 없다.
operator lock은 비어 있었고 XL/Mixed/Record 본문은0회다.

동일 P4 manifest, 동결824629e4 pair, 보강 실행기 `d69b7222…`를 재사용한다.
실행 전 기존 aggregate와 Large summary의 SHA를 확인했다. 기존 관측
디렉터리를 보존하고 마지막 완료된 pre-admission 시도
`continuation-xl-mixed-20261111-1`에서 새 단일 관측
`continuation-xl-mixed-20261112-1`을 시작했다. PID97469의 입장 전 custody는
exit0이다. 독립 supervisor PID97462가 이 시도의 OS 종료값을
`BASE/continuation-xl-mixed-20261112-1-exit.json`에 기록한다.

<a id="native-ownership-structural-audit-20261112"></a>
### Native 소유권과 검증 책임의 구조 감사 — 2026-11-12

데몬의 `StageSources`는 이미 소유한 `Vec<SourceInput>`을 Kernel에 빌려주고, Kernel은
설치 패킷을 만들면서 본문과 경로를 다시 할당했다. Kernel의 배치 API를 소유형 입력 하나로
전환했다. 데몬은 역직렬화한 Vec을 넘기고 Kernel은 본문을 설치 패킷으로, 경로를 canonical
source record로 이동한다. work 청구는 논리 길이를 따르고, 이동되는 Vec·String의 실제
capacity는 원 예산의 temporary memory로 받는다. 기존 preflight·child-first 설치·객체
identity 확인과 SDK 요청/응답 형식은 유지한다. 이는 **staging**의 복사 제거다.

SourceApply와 Lexical의 본문 검증 중복은 별도다. SourceApply는 유효한 source cut을 만들기
위해 record·본문을 확인하고, Lexical은 현재 `SourceDelta`를 받아 다시 본문을 읽고 UTF-8을
확인한다. 이 경계의 후속 설계는 Kernel이 attempt·cut에 묶인 단일 검증 순회를 소유하고,
검증한 본문을 소비자가 처리할 때 함께 전달하는 계약이다. 소비자가 건너뛴 prior/next와
Lexical이 없는 profile도 publish 전에 완주해야 한다. 임의 staged ref·복구·취소·늦은 검증
실패에서 같은 보장을 증명해야 한다. staging 성공이나 단순 flag를 본문 검증 근거로 사용할
수 없다. 현 소스의 get/retained 계수로 시간 기여도를 확인한 뒤 이 경계를 통째로 바꾼다.

writer와 Catalog는 이미 중간 confirm+reserve를 같은 transaction으로 처리한다. 다음 packet을
같은 receipt로 넘기려면 취소·재시작·fence 계약을 바꿔야 한다. 현 소스에서 그 비용이 120초
결과를 좌우한다는 증거가 없다. Mixed의 pin 해제와 전역 `SettledAtSnapshot`은 다른 사건이다.
기존 수용 조건은 지속 발행 중 전역 부채가 비는 시점을 요구하므로, 최신 실행의 blocker
체류·custody 진행·전수 검사 도달 여부가 필요하다. 과거 m2의 표본만으로 원인을 합치지 않는다.

**VERIFIED** — 통합 nextest `43e4d174-30fc-4c6a-907f-f10797c9ac96` 4/4, exit0.
실제 UDS의 128개 수용과 129개 거절, 객체 identity·순서·중복 제거, 빈 source,
실제 capacity 예산 거절 시 미설치, 여러 페이지의 전체 adapter 게시와 재오픈을 확인했다.
작은 논리 길이의 입력도 보유한 capacity가 temporary grant보다 크면 거절된다.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=75 ./scripts/cargow \
  --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel -p quanta-index-searchd-runtime \
  --test native_source_batch --test native_bootstrap --test native_lifecycle --test native_index_uds \
  -E 'test(/batch_preserves_single_identity|native_uds_stage_sources_accepts_full|existing_corpus_installs_all_adapters|empty_source_advance_rebinds_real_filemap/)' \
  --no-tests fail --test-threads 1 --no-fail-fast
```

**BLOCKED** — 기존 동결 제품 실행은 00:37 KST 1분 부하 19.06으로 사전 검사에서 대기했다.
변경 후 staging 시간 절감 및 XL 120초·Mixed 제품 수용은 미측정이다.

<a id="native-source-body-single-read-20261112"></a>
## SourceApply와 Native Text의 본문 검증 합류

기존 SourceApply는 바뀐 next 본문을 읽어 ref·길이·UTF-8을 검증했다.
Native Text는 같은 본문을 다시 읽어 동일 조건을 검증한 뒤 토큰화했다.
원본 객체가 읽기 창에 남아도 두 번째 논리 read와 UTF-8 스캔은 발생했다.

- SourceApply는 event·authority, record·edge, path·frontier와 pinned prior
  본문을 검증한다. 이미 검사한 prior와 정확히 같은 next ref·길이는
  그 검사 결과를 사용한다.
- 새 next UTF-8 검증은 시도 내 ref·길이 표에 맡길 수 있다. Native Text는
  실제 객체를 읽어 검증하고 같은 borrow에서 토큰화한다. 성공한 검증만
  해당 시도의 표에 표시한다. 최대 32,768개 메타데이터 슬롯을 사전 예약하며,
  메모리 예약이나 슬롯이 부족하면 SourceApply에서 직접 검증한다.
- Kernel은 member 준비 직후 표에 남은 본문을 원 work budget으로 읽는다.
  재사용·제외·adapter 구성으로 본문을 읽지 않은 경우도 처리한다. 이 단계가
  완료돼야 work seal과 Catalog publish에 진입한다. 잘못된 본문을 늦게 발견하면
  시도 소유 frontier/map 객체가 이미 설치될 수 있다. 이후 처리는 기존 시도
  custody·정산 경로를 따른다. 기존 view를 유지하고 새 event 게시를 막는다.
- Record에서 eager·최종 확인은 기존 `native_source_owner_prepare/34`에 남는다.
  Lexical이 소비한 본문은 `native_text_inventory_prepare` 및 BuildPrepare
  object-port 집계에 포함된다. 34번 lane만으로 전체 본문 비용을 합산할 수 없다.

**VERIFIED — nextest `49ae13a7-6d33-41ca-aa28-36f234be52d2` 12/12, exit 0.**
cold 본문 두 개의 get+retained 합계가 각각 1회였고 두 문서 모두 검색 인덱스에
반영됐다. Text member 없는 시도에서 잘못된 UTF-8과 길이로 거절되며 최신 view를
유지했다. UTF-8 오류의 terminal custody 해제와 재호출 결과 0도 확인했다.
취소·trace 계수, 배치 경계, Mixed 업데이트·재오픈과 UDS 게시 회귀를 같이 실행했다.
이전 실행 `acc0a203-56b3-45d8-8b63-b6af04616b60`의 11/12 실패는 cold fixture가
이전 inventory block을 세던 검증 문제였다. 현재 cold의 packed occurrence 두 개와
별도 설치 두 회를 검증하며 warm의 64개 inventory block 확인 및 중간 실패 주입을
유지했다.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=75 ./scripts/cargow \
  --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel -p quanta-index-lexical -p quanta-index-searchd-runtime \
  --lib --test native_lifecycle --test native_source_rebase --test native_source_batch \
  --test native_index_uds --no-tests fail --test-threads 1 --no-fail-fast \
  -E 'test(source_body_validation) | test(direct_staged_record_cannot_publish_false_utf8_admission) | test(utf8_stamp_with_invalid_bytes_refuses_before_token_writes) | test(source_body_keeps_original_cancellation) | test(native_text_byte_bounded_batch_confirmed_prefix_mixed_update_and_reopen) | test(native_uds_source_lifecycle_status_old_view_and_reopen) | test(native_uds_stage_sources_accepts_full_object_batch_and_refuses_overflow) | test(source_trace_preserves_publication_cost) | test(source_preflight_failures_and_early_cancellation) | test(source_advance_async_build_reports_four_work_axes) | binary(native_source_batch)'
```

**BLOCKED — 기존 F2 timing 입장.** 01:31:30 KST 1분 부하 21.68, 한도 8.0,
foreign Rust 3개였다. 해당 이어가기 디렉터리에는 readiness만 있다. 기존 P4
`824629e4` pair의 Large ACK 27.281458초와 검색·재오픈 기록은 이 변경 이전의
증거다. 현재 변경을 포함한 pair의 XL·Mixed 제품 수용과 Off/Record 차이는
**NOT_RUN**이다. 원 120초, ACK·검색·재오픈과 Mixed pin·정산 기준을 사용한다.

### 유예 본문 검증 중 취소와 재개

**VERIFIED — nextest `1b6d0766-9a4a-43d6-92ae-c3b753bfaa59` 4/4, exit 0.**
Text member 없는 SourceAdvance에서 map 설치 뒤 두 번째 본문 GET에 취소를
주입했다. 원 요청은 Pending이며 head는 이전 view였다. 같은 operation을 재개하면
두 본문을 다시 읽고 두 owner를 게시한다. terminal custody 해제 후 재호출은
0건이고 같은 operation의 재조회는 동일 Published view를 돌려줬다.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=75 ./scripts/cargow \
  --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel --test native_lifecycle --no-tests fail --test-threads 1 \
  -E 'test(source_body_validation) | test(native_text_byte_bounded_batch_confirmed_prefix_mixed_update_and_reopen)'
```

01:41:06 KST 기존 F2 준비 상태는 **BLOCKED**였다. 1분 부하 23.80, 한도 8.0이며
새 XL·Mixed 제품 결과가 없었다.


<a id="native-f2-nov12-boundary-consumed"></a>
#### 11월 12일 실행 경계 인수 — 2026-11-12 04:02 KST

Native 전환 DISPATCH·NC-F와 C0/P09 DISPATCH의 계획 간 경계를 읽었다.
F1/F2는 기존 동결 `824629e4` pair의 원 Large/XL/Mixed만 소유한다.
NC-A/B/C/D의 새 소스와 F3의 합류·벤치·SDK60 수용은 각 작성자가 맡는다.
공유 main HEAD는 `ade5fadc1a084fbb81eba831861e996081d80867`이고 다른 작성자의
다수 dirty hunk가 있다. 해당 변경을 이 동결 후보의 증거로 승격하지 않는다.

기존 supervisor97462/controller97469가 살아 있고 동일 continuation의 control에는
readiness만 있다. XL/Mixed 본문과 admission terminal은 아직 없다. 새 실행은 만들지
않았다. C0 전용 대기기 PID96800도 살아 있으며 C34 기록은
`canonical_host_admission_released`다. canonical host-coordination의 external claim은
이번 표본에서 없었다. 이는 현재 QBC 잠금의 가용성을 증명하지 않는다. 실행 입장은
기존 canonical EX→project lock→quiet-host case gate를 그대로 소비한다.
Java PTA·C0/P09의 실측 실행권은 그 입장 권위가 판단한다.

**BLOCKED** — 기존 환경 입장 대기다. Large는 기존 검증 결과를 재사용하고,
XL/Mixed는 **NOT_RUN**이다. 최종 terminal과 OS exit, 잠금 반환은 종료 시 이 원장에
추가한다. 새 소스의 영향 판정과 후속 후보 수용은 NC-F가 소비한다.


<a id="native-f2-authority-drift-20261112"></a>
#### canonical 입장 도구 변경으로 본문 전 중단·동일 pair 재개 — 2026-11-12 07:52 KST

`continuation-xl-mixed-20261112-1`은 07:50:58 KST에 load7.970703/8.0,
foreign Rust0으로 quiet-host 조건을 충족했다. 이후 canonical 모듈의 바이트 대조에서
`RuntimeError('Canonical admission authority changed during loading')`로 거절됐다.
실행 시작 시 권위 SHA는 `1c90680db39be94a5b5c73caeffdc26bb0c4d05eb79e45d07b559cedd868bc76`,
현재 SHA는 `d04f3a6a6e20876b15cf2374a047c7b9cb8eb7d4b2ff0a46f9d75f924ba9a953`다.
긴 입장 대기 중 live canonical 도구가 바뀐 것이 직접 원인이다. 원 pair와 동결 제품
소스의 변경 증거는 없다. 현재 canonical EX 함수는 admission/activity 두 flock과
external claim 검사를 수행하고 finally에서 잠금을 반환한다.

**BLOCKED** — `host_admitted=false`, `project_leaf_started=false`, controller exit1,
독립 supervisor의 실측 OS exit1이다. supervisor97462/controller97469가 종료했고
operator lock의 nonblocking 취득·반환을 확인했다. project leaf와 XL/Mixed/Record
본문은0회다. 해당 시도는 compile-activity와 project 실행권을 취득하지 않았다.
기존 P4 aggregate와 Large summary SHA, 실행기 `d69b7222…`가 그대로다.

이 실패를 보존하고 현재 canonical 도구를 새 입장 입력으로 소비해
`continuation-xl-mixed-20261112-2 --resume-from continuation-xl-mixed-20261112-1`을
한 번 시작했다. 실행기·manifest·pair·제품 예산을 재사용했다. supervisor46579,
controller46580, 입장 전 custody exit0이다. 최초 readiness는 load13.934082/8.0,
foreign Rust1로 **BLOCKED**다. 새 OS 종료값 경로는
`BASE/continuation-xl-mixed-20261112-2-exit.json`이다. Large 재실행과 제품 재빌드는0회다.


<a id="native-f2-xl-mixed-terminal-20261112"></a>
#### 동결824629e4 XL·Mixed 실본문 종료와 실행권 반환 — 2026-11-12 08:14 KST

**FAILED** — `continuation-xl-mixed-20261112-2`가 원 pair·원 예산으로
XL Off→Mixed Off→XL Record를 각1회 실행하고 종료했다. 제품 재빌드와 Large 재실행은0회다.
컨트롤러 시작·입장 직후·종료 후 custody는 모두 exit0, canonical EX와 project leaf는 실제
입장했다. leaf exit1, controller exit1, 독립 supervisor의 실측 OS exit1이 일치한다.
원본 P4 aggregate의 과거 BLOCKED와 기존 Large VERIFIED 영수증은 그대로 보존했다.
현재 main의 NC-A/B/C/D 변경과 새 출하 후보에는 이 결과를 전이하지 않는다.

| 본문 | producer exit / capture exit / case exit | 실행 결과 |
| --- | --- | --- |
| XL Off | 1 / 2 / 1 | full_event_ack incomplete 137953.937542ms, completion=null, original-key status=refused |
| Mixed Off | 1 / 2 / 1 | 699.667611s 뒤 held_read settlement 한도 실패, 960초 epoch 미완료 |
| XL Record | 1 / 2 / 1 | full_event_ack incomplete 139539.724542ms, completion=null, original-key status=refused |

세 본문 모두 timed_out=false, source_revision 전후 일치, dirty_after=false, pair SHA 전후
일치다. capture의 `build_binding=UNPROVEN` 표시는 남아 있으며, 별도 controller의 동결
custody exit0과 이전 pair 공급 증거로 그 입력을 확인했다. 제품 수용 상태는 모두 FAILED다.

XL Off는 clean gate load5.782227/8.0, foreign Rust0으로 시작했고 실행 표본에서도
foreign Rust0, 관측 load 최대7.810546875였다. daemon의 key sequence2와 결속된 최초
거절은 `deadline`, failed_stage=BuildPrepare,
`crates/quanta-index-storage/src/object.rs:906:36`의 NamespaceValidation checkpoint다.
usage reads3929261, writes13630047, bytes12116438332, mutations18192257은 명시 한도
내다. full_stage_sources는91943.36ms였다. ACK는 완결되지 않았고 원120000ms를 넘겼다.
first_search·reopen은 도달하지 못했다. 거절 checkpoint를 누적 지연의 단독 원인으로
단정할 수 없다. XL Record도 같은 deadline·BuildPrepare·sequence2에서 실패했고
checkpoint는 `object.rs:1914:12`의 decoded bytes charge였다. 해당 코호트의 full-stage
설치/읽기 경로를 변경한 후보에 동일 원 rail 검증이 필요하다.

Mixed의 초기 FULL ACK는23232.51ms, immediate_ack/terminal_published/status checks0이다.
보유 시간은600.261438958초, 실제 최대 renewal gap은121062.573459ms로180000ms 안이다.
pin release 확인은 최초 release 요청 후91.63025ms, confirmed_last_pin=true,
last_status=Released다. settlement는60회 poll 뒤 Pending, settled_ms=null,
head=exact_floor=275, remaining_target_pins=false다. daemon은
Pending(TerminalCustody)→Pending(ReclaimQueue)→Pending(UnretiredView)를 관측했고
마지막 로그는 TerminalCustody/head272/oldest_ready26629다. custody pages/rows와
operation sequence는 증가했다. 이 표본은 진행이 있는 잔여 debt이며 pin 해제 실패로
분류할 근거가 없다. 최종 전역 settlement 완료 증거는 없다.

Mixed의 다른 원 합격선도 충족하지 못했다. warmup publisher는 scheduled60 중
published17·missed_late43, measure는 중단 시 scheduled601 중 published255·missed346다.
query planned48000 중 due33015, queue-full10911, enqueued22104,
completed22040 중 failed21841이다. 첫 query 실패는 task27 broad_top20의 IPC Read timeout,
latency2001.780667ms·queue1608.361667ms다. 부분 결과의 first_failure_owner=held_read는
최종 중단 owner를 뜻하며 가장 이른 query 실패와 구분한다. 50QPS·1Hz·60+900초 수용과
무실패 query는 FAILED다. Mixed 실행 중 foreign Rust 표본4건도 있어 timing 환경은
`foreign_rust_observed`다. query·publisher·settlement의 직접 실패는 기록하되, 이 실행에서
환경 기여와 제품 비용을 분리한 비교 성능 수치는 주장하지 않는다.

실행 종료 시 supervisor46579/controller46580 및 case capture PID49912/54938이 없었고,
해당 BASE를 사용하는 잔여 제품 프로세스도 없었다. resource_admission의 실제 release는
held_ns1200676379208로 기록됐다. operator lock의 nonblocking 취득·반환도 확인했다.
project lock은 후속 다른 PID가 사용 중이므로 전역 idle로 표시하지 않는다. 이 시도의
EX·project lease는 context 종료와 leaf release 이후 반환됐고, 타 작업 잠금은 그대로다.

원본 위치는 `BASE=/private/tmp/qidx-xl-mixed-rca-z1a_82he`다.

| 종료 증거 | SHA-256 |
| --- | --- |
| p4/host-continuation-xl-mixed-20261112-2/terminal.json | acd2323eff62f7c987117776dabdc743598e40b4879b3f8d422bc35496f29cf7 |
| p4/host-continuation-xl-mixed-20261112-2/leaf-terminal.json | 4b9ba415d87445c392943805f0c086b911c89c11f16c9b43e01a82f652041e6f |
| continuation-xl-mixed-20261112-2-exit.json | 4745fe8084aa9a7e1188abeb338c84a2862cb8948465bcf593d68285e046efcb |
| p4/n2-xl-off/execution.json | 30e01c212fc912748092366a3f09a0ef8d96018feed40d3b750a6d9d96274f93 |
| p4/n2-mixed-off/execution.json | 077b25b96f9188a96d70433962a37f02010a48be3405badcd47cceb00b149f47 |
| p4/n2-xl-record/execution.json | a30146d2538076112f5ecb24b7abcfeac85b59ae4b03be6f93089af5a1901309 |

소스 인계는 NC-F가 이 terminal과 원 실패를 소비해 영향 writer를 정하는 경계다.
XL은 BuildPrepare의 설치·검증 비용, Mixed는 NC-C의 read-session 요청/기한·대기와
Catalog의 retention·global settlement를 각각 대조해야 한다. 기존 dirty writer가
진행 중인 같은 기능을 재작성하지 않았다. 관련 입력이 바뀐 후보에서만 동일 rail을
재검증한다. 현재 후보의 XL ACK120초와 Mixed 전체 수용은 잔여 FAILED다.

오프라인 진단은 현재 도구의 다음 명령으로 실행했고 각각 exit0이다.

```sh
.venv/bin/python -B -m tools.benchmark.retrieval.causal_cost_diagnostic native-triage --run-dir /private/tmp/qidx-xl-mixed-rca-z1a_82he/p4/n2-xl-off
.venv/bin/python -B -m tools.benchmark.retrieval.causal_cost_diagnostic native-triage --run-dir /private/tmp/qidx-xl-mixed-rca-z1a_82he/p4/n2-mixed-off
```


<a id="native-f2-xl-record-offline-diagnostic-20261112"></a>
#### XL Record 오프라인 trace 진단과 NC-F 소비 요청 — 2026-11-12

다음 canonical 진단 명령은 exit0으로 끝났다. 최초 `--include-trace` 호출은 지원하지 않는
옵션으로 exit2였고 실제 공개 옵션 `--trace`로 교정했다. 기본 읽기 상한에서는
OVER_LIMIT을 표시하므로 실제496630837bytes를 수용하는 명시 상한536870912로 읽었다.
새 제품 본문·빌드·캡처를 만들지 않았다.

```sh
.venv/bin/python -B -m tools.benchmark.retrieval.causal_cost_diagnostic native-triage --run-dir /private/tmp/qidx-xl-mixed-rca-z1a_82he/p4/n2-xl-record --trace --max-capture-bytes 536870912
```

parser는 COMPLETE_DIAGNOSTIC, events4911466, dropped0, unwritten0, open_spans0,
truncated_tail=false를 반환했다. run binding은 UNBOUND이므로 최종 trace 상태는
UNBOUND_DIAGNOSTIC이다. 실패 capture의 execution에는 유효한 child trace 결속이 없다.
따라서 다음 값은 별도 오프라인 관측으로만 소비한다. Off 시간 분해나 정식 동시 비교를
증명하지 않는다.

- trace 경로는 `BASE/p4/n2-xl-record/causal-timeline.jsonl.native-xlarge-children/first.jsonl`이다.
- SHA-256은 `26a7afc2b8a53455432a2e8c6a07c579fc8c511701a8388f0124daaf97b45de1`, capture_id는 `3b64db5cad77bda6ce3c8a8a81dd04ac`다.
- 실패 request38346의 inclusive wall union은 BuildPrepare87807.04ms, LexicalBuild87806.37ms, NativeTextInventoryPrepare/lane2 50694.43ms, lane35 37365.68ms, SourceApply29297.95ms다. 겹치는 stage의 값을 합산하지 않는다.
- NC-F는 이 관측을 기존 E03 Text inventory·prepared install writer에게 전달할 소스 소비 요청으로 사용한다. 거절 마지막 checkpoint만 최적화 대상으로 삼는 결론은 보류한다. 해당 writer의 관련 변경과 같은 제품 rail 결과를 결속해야 한다.
- Mixed는 NC-C의 shared read-session 요청 정산·queue/timeout과 Catalog retention의 terminal custody 진행·settlement를 각각 소비한다. 최종 중단 owner, 최초 query 실패와 외부 Rust 관측을 한 원인으로 합치지 않는다.

**VERIFIED** — 이 시도의 terminal·실측 OS exit·source/pair custody·프로세스 종료와 실행권
반환 확인, 위 offline 진단 및 `git diff --check -- docs/plans/nov-5-indexing-bugbash/RCA.md`
exit0이다. **FAILED** — 원 후보의 XL ACK120초·ACK/search/reopen과 Mixed50QPS·1Hz·전체
60+900초·60초 settlement 수용이다. 현행 NC 후보의 동일 조건 재검증은 관련 source 변경과
NC-F 합류에 달려 있다. 같은 실패 입력의 추가 제품 재시도는 만들지 않았다.


<a id="native-f2-session-hardening-20261112"></a>
#### 이 채팅의 continuation·회귀·종료 증거 hardening — 2026-11-12

리뷰 범위는 이 채팅이 작성·보강한 `BASE/continue_native_p4.py`,
`BASE/continuation-review/test_continue_native_p4.py`, 단일 supervisor의 실측 종료 기록과
이 원장의 F1/F2 결과다. BASE는 `/private/tmp/qidx-xl-mixed-rca-z1a_82he`다.
공유 main HEAD는 `ade5fadc1a084fbb81eba831861e996081d80867`이며 다수 타 작성자의
dirty·untracked가 있다. 이 채팅의 앞선 원문 일부는 compacted history로만 남아 있어,
Rust 제품 변경 전체의 작성자를 이 채팅으로 귀속할 근거는 없다. 해당 hunk는 범위 밖이다.
continuation의 이전 수정은 `continuation-review/original`과 patch, 이번 수정 전 바이트는
`session-hardening-20261112/before` 및 `before-identities.json`으로 대조했다.
현재 index에서 RCA에 staged 변경은 없다.

| 사용자 의도와 소비 경계 | 확인된 결함·귀속 | canonical 보완과 증명 |
| --- | --- | --- |
| 같은 pair의 실제 XL 결과만 이어받기 | introduced — 이전 continuation hardening의 raw hash/summary tier 검사만으로 Large를 XL로 재표기한 fixture를 수용했다. 이전 positive resume 회귀가 이 잘못된 fixture를 사용했다. | `continue_native_p4.py:239`에서 build-bound Python으로 frozen `validate_native_product_artifacts`를 직접 호출한다. 원 tier manifest·corpus 크기·mutation 수·publication·phase·Mixed 계약을 기존 권위에 위임한다. 재표기한 Large와 미완료 ACK를 거절하고 실제 Large 원본을 수용했다. |
| 원120초 FULL ACK 수용 | pre-existing in original continuation — generic capture가 completed ACK의 실제120초 wall 상한을 강제하지 않았고 continuation도 검사하지 않았다. 앞선 수동 판정은 이 누락을 자동화하지 못했다. | `continue_native_p4.py:260`에서 canonical 본문 검증 후 wall<=120000ms를 적용한다. 120000은 수용, 120000.001은 거절했다. |
| 다음 case·receipt 오류에도 실측 종료 보존 | pre-existing in original continuation — OS case exit는 row 반환 후 메모리에만 있었으며 terminal 파싱 실패 시 잃었다. | `continue_native_p4.py:353`에서 wait 종료 직후, child terminal 읽기 전에 case별 process checkpoint를 atomic rename으로 발행한다. 실제 exit7 뒤 terminal 부재, XL exit0 뒤 Mixed 자식 SIGTERM(-15)·terminal 부재에서 각각 종료값과 실패 prefix를 확인했다. |
| 재실행 가능한 회귀가 실제 실행기 검사 | introduced — fixture가 현재 XL control을 복사해 실본문 실행 후9개 중2개가 오류였다. 로컬 shadow runner도 실제 실행기보다 먼저 import될 수 있었다. | 원본 refusal을 `original_xl_evidence`로 찾고, 실제 BASE runner를 먼저 import한다. positive carry는 명시적인 validator 계약 stub으로 sequencing만 확인한다. raw product의 타당성은 실제 Large·tier 재표기 거절·미완료 ACK 거절 회귀로 따로 검증한다. |
| 제품 실패·입장·실행권의 원본 보존 | satisfied — 이전 결과의 OS1·leaf1·제품 FAILED와 미결 수용 기준을 확인했다. | 모든 기존 P4 terminal, manifest, 실측 exit 파일의 SHA가 이번 수정 전과 같다. frozen source/pair와 build-bound runner 입력 검사도 exit0이다. |

RED에서 Large→XL 재표기 수용과120000.001ms ACK 수용을 각각 assertion failure로
재현했다. exit7 프로세스를 실제 실행한 뒤 checkpoint 부재도 확인했다. 기준9개 회귀의
7pass/2error는 잘못 선택한 post-run fixture 경로가 원인이었다. 수정 후 아래 명령은
16/16, exit0이다. 테스트는 격리된 임시 디렉터리를 사용하며 제품 daemon을 시작하지 않는다.

```sh
/Users/songmin/.local/bin/python3.12 -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/continuation-review/test_continue_native_p4.py
/Users/songmin/Documents/code-new/quanta-index/.venv/bin/python -B /private/tmp/qidx-xl-mixed-rca-z1a_82he/verify_native_main.py /private/tmp/qidx-xl-mixed-rca-z1a_82he/rail-main-20261111-r2-p4.json
git diff --check -- docs/plans/nov-5-indexing-bugbash/RCA.md
```

실제 active runner의 canonical-validator 실패 검사, ACK wall 조건, process checkpoint를
메모리상의 변형에서 각각 제거해 해당 회귀를 실행했다. 변형3개 모두 testsRun1/failures1/
errors0으로 필수 조건 제거를 탐지했고 실험 명령은 exit0이었다. 이는 owner-local 보호
조건의 검증이다. 새 Rust 제품·성능 수용 실행은 NOT_RUN이다.

| 변경 단위 | SHA-256 |
| --- | --- |
| continue_native_p4.py before | d69b722210f033d4e253a63e6611faf143b02be2713f57690b8aea69ce387d72 |
| continue_native_p4.py after | 7e8ee9b00068fd3021660542436c5c7a8d48290a503b395abd77f0d940c52cd2 |
| continuation-review/test_continue_native_p4.py after | 80a3d3b42e89a4ddee3b404ce34e85038f82058bc6128437c0e4670595c64ab3 |

**VERIFIED** — 이 채팅의 실행기·회귀 보강,16개 owner-local 회귀,3개 필수 조건 제거
탐지, 원 source/pair custody와 기존 종료 증거의 바이트 보존이다. **FAILED** — 이전 원
후보의 XL ACK120초와 Mixed 전체 제품 수용이다. 다음 제품 실행은 관련 소스를 보완한
NC-F 후보와 동일 rail에 결속해야 한다. 이번 작업은 기존 body를 다시 실행하지 않았다.
case checkpoint는 완료한 OS wait 결과를 보존한다. SIGKILL·전원 단절 이전의 미관측
종료값과 강제 종료 후 자동 재개는 이번 검증 범위에서 증명하지 않았다. 상태는 partial이다.


<a id="native-f2-session-hardening-second-pass-20261112"></a>
#### 직전 continuation 보완분 재감사 — 2026-11-12

HEAD `ade5fadc1a084fbb81eba831861e996081d80867`에서 실행기
`7e8ee9b00068fd3021660542436c5c7a8d48290a503b395abd77f0d940c52cd2`와
회귀 `80a3d3b42e89a4ddee3b404ce34e85038f82058bc6128437c0e4670595c64ab3`를
재확인했다. 앞 절의 canonical validator, 원 ACK120초, case OS exit 발행 순서,
original refusal archive, 실제 runner import와 parent 검증 연결에 추가 결함은
없었고 코드·테스트 변경도 없었다. 동일 회귀16/16·exit0(2.962초),
`verify_native_main.py` exit0/`BOUND_CANDIDATE_READY 824629e4…`, 기존 P4
terminal·manifest·OS exit12개 SHA 일치다. 이는 실행기 회귀 증거다. 기존
XL/Mixed 제품 수용은 FAILED; 강제 종료 후 자동 재개와 새 후보 제품 재검증은
NOT_RUN이다. 전체 제품 마감은 partial이다.
