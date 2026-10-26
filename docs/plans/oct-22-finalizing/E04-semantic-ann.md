# E04 — Semantic native incremental·ANN 제품

상태는 [SSOT E04](README.md#e04). 원 요구 R18–R22, 종전 W2-13을 소유한다. 기존 SEARCH의 confirmed failure 수리와 단일 제품 큐를 유지하며 Catalog/VFS/Common/FD는 PHYSICAL 경계다.

## Reuse and authority

NativeSemantic directory·bounded region/delta/member COW·Lance file-set·embedding/provider·selected source/projection 인증·Kernel materialization/CAS·SDK/query/overlay route를 재사용한다. semantic desired/covered/searchable와 ANN approximate/exact/capped/incomplete를 구분한다.

모델/recipe/input/owner/member binding·provider 호출 전 compatibility·stale job/delete/ABA fencing·durable unchanged-owner reuse·held-old view pin을 유지한다. mask/delete/replacement는 top-k/refill 전에 적용한다. 직접 source-path/lexical eligibility의 권한을 확장자나 private callback으로 대체하지 않는다.

## Original 4K gate

기존 `product_native_ann_quality_4096x64_reopens_at_fixed_recall`를 동일 bench daemon/SDK로 수용한다.

- 4096×64 vectors,64 queries,K10,recall≥0.95. 실제 UDS/provider·selected membership·독립 exact score/coverage/traversal·cold reopen을 포함.
- 원 per-request1M reads/512MiB·SDK30s·worker 총 assignment lifetime10s와 memory/IO/work/candidate/routing 상한을 유지. claim/grant/retry가 원 deadline을 재시작하지 않으며 watchdog1200s도 원 요청을 연장하지 않는다.
- 이전 SourceAdvance 통과 뒤 worker deadline 실패, 이후 수리의 owning pass, 새 수리 소스의 미실행은 별도 source 결과다.
- Pending/status/작은64-owner/legacy ann_matrix 통과는 원4K 양성 수용이 아니다. Unknown은 원 key로 관측하며 재제출/예산 갱신하지 않는다.

## Shared provider admission

Source materialization과 query는 같은 provider·attempt registry를 사용한다.
registry의 physical inflight 상한은 검증된 service execution capacity의
`provider_units`와 일치해야 한다. `provider.concurrency=1`은 각 owner/query
호출의 fanout이고 서비스 총량이 아니다. Interactive/Enrichment가 각각 원 grant를
보유한 동시 호출을 이 fanout 값으로 거절하면 안 된다. 각 HTTP attempt의 원 grant
custody·취소 후 physical join·shutdown drain은 기존 owner가 계속 보장한다.

provider의 typed `ServerOverloaded`/`RequestCancelled`는 Native
`ResourceLimited`/`Cancelled`로 전파한다. 원 요청의 취소/deadline이 먼저 관측되면
그 결과를 우선하고, 관련 없는 provider 오류는 기존 Storage 진단을 보존한다.
Source·query 실제 bridge 회귀와 기존 pool/custody/signals 회귀를 한 영향 배치로
검증한다. 변경하지 않은 daemon materialization/query/scope/reopen fixture의
RED→GREEN은 해당 기능 수용이며 원4K/Q64·quiet 성능 수용을 대체하지 않는다.

## Code closeout plan — 2026-10-22

이 계획은 E04 구현 절단과 검증 순서다. 현재 상태는 [README E04](README.md#e04),
source별 결과는 [VALIDATION의 단계 진단](VALIDATION.md#e04-bounded-phase-discriminator--2026-10-22)에만 기록한다.
완료된 original-path/batch lookup·completion mark·completed-group index·resident wire
custody 수리는 재사용한다. 128-owner 경합 Record 진단의 시간 분포를4K 성능 보장으로
외삽하지 않는다. 설치 그룹8개 자체가 전부 불필요하다는 결론도 내리지 않는다.

### 1. 실제 의존 관계와 최소 변경 계약 고정

최초 RCA baseline의 일반 owner8회 설치는 아래 순서였다. C1 수리 뒤7회가
관측됐고,후속 metadata/COW 결합의 구현·증거는 아래 해당 절에서 관리한다.
`stage_work` 같은 추가 Catalog transaction은 설치 횟수에 포함하지 않는다.

| 최초 baseline 설치 그룹 | 실제 producer/caller | 최초 C1 절단 |
| --- | --- | --- |
| 1. Immutable request | Kernel `submit_authorized` ingress/install/admit | 원 operation identity·recovery 경계 유지 |
| 2. Owner artifact | Registry owner build → NativeSemantic | 원 artifact 인증·`stage_work` 재시작 경계 유지 |
| 3. Owner member + owner coverage | Kernel `save_materialized_owner_metadata` | 이미2객체 batch; 재개발하지 않음 |
| 4. Selected owners + missing roots | Kernel materialize → 원 resident pair/map installer | 이미 공용 설치 그룹; CAS/EOF/old roots 유지 |
| 5. ANN delta map | NativeSemantic `put_validated_delta` → 기존 map installer | 새 directory가 참조하는 선행 결과로 유지 |
| 6. ANN directory | `ann_materialization::prepare` → `AnnDirectory::save` | 아래7과 합칠 우선 대상 |
| 7. CoverageBinding | Kernel materialize → `save_coverage` | directory의 실제 identity를 child로 묶어6과 child-first batch |
| 8. Final coverage map | Kernel materialize → 기존 map installer | 설치된 coverage를 참조; 원 CAS 전 선행 설치 유지 |

seal/carry owner의16/17그룹은 별도 분기다. 일반 owner의8→7을 그대로 적용했다고
가정하지 않고 실제 branch의 설치/파일 graph까지 확인한다.

### 2. C1 — prepared searchable 결과 계약과 실제 batch 연결

**아래 C1은 이미 구현된 원 계약이며 신규 TODO가 아니다.** directory + CoverageBinding의2회 설치를 기존1회 batch로 합치는 계약이다.
임시 이름이나 최종 Rust 서명은 구현 시 기존 타입을 재사용해 정하지만 계약은 다음과 같다.

- Core `IndexOwnerBuildPort::prepare_searchable`의 단일 내부 반환 계약이 이미 설치된
  root 또는 아직 설치하지 않은 bounded immutable object 준비 결과를 표현한다.
  준비 결과는 canonical payload/children·identity·원 allocation lease를 보유하며
  공개 searchable reference나 publication authority를 발급하지 않는다.
- 기존 `ObjectBatchInput`·identity 계산·`AttemptContext::put_batch`·admitted payload/batch
  helper를 재사용한다. 공용으로 이동할 helper는 한 번 이동하고 consumer별 복제하지 않는다.
  기존 반환 계약과 나란한 v2 port·fallback engine·새 serialization format을 만들지 않는다.
- Semantic `AnnDirectory`의 기존 검증/codec/children 조립을 한 경로로 유지하면서,
  `ann_materialization::prepare`가 directory 준비 결과를 Kernel로 이동한다.
  MAX_DIRECTORY_BYTES·capacity-before-allocation·lease 수명과 모든 early-return을 유지한다.
  directory/coverage/batch backing의 동시 peak를 계산해 원 memory funding 안에 맞추고,
  payload를 옮길 때 복제하거나 lease를 먼저 해제하지 않는다.
- Kernel `registry.rs`의 실제 erased producer/consumer를 같은 변경으로 갱신한다.
  `update.rs::materialize`는 준비된 directory identity로 CoverageBinding을 인코딩하고
  `[directory, coverage]` 순서로 원 child-first batch에 넘긴다. 반환 길이·순서·identity를
  검증한 뒤 final coverage map과 원 `commit_transition`에 연결한다.
- `update.rs`의 나머지 Source/Projection/reconfigure searchable caller와 non-ANN/default
  owner-map 반환도 같은 계약을 소비한다. 이 경로는 필요한 준비 결과를 설치한 후 기존
  coverage 처리로 진행한다. definition-only API·미소비 준비 결과를 남기지 않는다.
- directory가 설치되기 전 root를 read/Catalog에 노출하지 않는다. batch가 일부 설치 후
  실패해도 partial searchable/coverage를 게시하지 않고,같은 attempt의 원 recovery/GC
  경로가 처리한다. 기존 prepared-work 재사용·old pin과 예산을 유지한다.

이 절단은 SEARCH의 Core/Kernel/Semantic producer→consumer 변경이다. Storage installer,
Catalog/VFS/Common/FD의 physical body를 새로 구현하거나 다른 owner의 본문을 덮지 않는다.
원 batch가 이 child 관계/환급/실패 원인을 만족하지 못하는 실제 결손이 나오면 해당 exact
hunk를 OWNERSHIP 경계로 인계한다. 후속 metadata/COW 절단은 기존 Storage
builder/installer를 공유하는 명시 facet 추가로 한정하며 아래 구현 절을 따른다. 별도 writer/ledger/전역 pending cache로 우회하지 않는다.

**코드 완료 조건:** 두 metadata 객체의 wire/reference/child graph가 원 gold와 같고,
실제 ordinary materialization에서 해당2회가1회로 결합되며,모든 caller가 새 계약을
소비한다. 8→7은 검증할 구조적 목표이며 wall-time/4K 통과 약속이 아니다. 더 작은 숫자를
맞추려고 request 저장·artifact/stage_work·map installer·최종 CAS를 합치지 않는다.

### 3. C2 — 같은 코드 배치에서 실패·수명 회귀 구현

기존 Core/Kernel/Semantic/Storage oracle를 확장해 다음 동작을 고정한다.

- 독립 canonical wire/digest/children과 전후 selected view·old view가 동일하다.
  이미 설치된 root/default/non-ANN,새 directory,empty directory와 seal/carry를 포함한다.
- prepare/encoding/메모리 거절은 install 전에 실패하며,data allocation이 lease보다 먼저
  해제된다. producer 실패·취소·batch reserve/install/confirm 실패는 original primary cause를
  보존하고 성공 prefix나 새 root를 반환하지 않는다.
- batch의 잘못된 길이·반환 순서·identity는 거절한다. 늦은 map/CAS conflict에서도
  old selected view가 유지되고 staged objects는 기존 attempt cleanup에 남는다.
- 원 operation/claim으로 재시작할 때 prepared artifact 재사용·중복 provider 호출 방지·
  unchanged-owner/ABA fencing을 보존한다. 성공을 위해 fresh key나 예산을 만들지 않는다.
- ordinary owner는 metadata batch 감소를 실제 install/sink 호출로 확인한다.
  seal owner는 원 region/member/file-set graph와 독립 query/retirement gold를 유지한다.

회귀 코드를 실제 producer 변경과 함께 끝낸다. 테스트를 통과시키기 위한 production
wire/oracle/recall/예산 변경은 하지 않는다.

### C1/C2 implementation join — 2026-10-22

구현은 Core `SearchablePreparation::{existing,object,install_parent}`로 고정했다.
ANN producer의 원 payload/lease를 이동하고,Kernel의 세 actual caller는 provisional
identity로 coverage를 인코딩한 뒤 원2객체 child-first batch의 길이/순서/identity를
확인한다. attempt 불일치/취소/parent child 누락도 설치 전에 거절한다. 기존 root는
원 scalar 경로이며 같은 wire/children을 유지한다. `install`은 직접 포트 consumer와
기존 Semantic 회귀의 단독 설치 경로다. 별도 IR/codec/ledger/physical writer는 없다.

C2는 실제 FileObjectStore/Catalog/Kernel에서 같은 operation·누적 WorkBudget로
재개하고 staged artifact 재사용/old pin을 확인하는 회귀를 포함한다. 임의 storage
오류의 Kernel public outcome은 원래 `Unknown`이고 원 Catalog 상태는 Pending이다.
internal install 오류를 직접 Err로 노출하도록 제품 계약을 바꾸지 않는다. 최초 배치의
Semantic fixture10건은 batch 구현 부재,Kernel1건은 이 Unknown 계약을 잘못 가정한
새 회귀 문제였으며 fixture/기대값만 수리했다. source 적용과 execution 수용은 분리하며
현재 결과는 [VALIDATION](VALIDATION.md#e04-prepared-searchable-metadata-batch--2026-10-22)을 따른다.

### SourceAdvance discriminator after C1

C1/C2는 owning194/194·Projection/회복13/13·영향 strict와 실제128-owner의
7/15/16 설치그룹으로 확인했다. 당시 원4K는 SourceAdvance의30.003s에서
Unknown/Pending으로 실패했으며 ANN materialization에 도달하지 않았다.
수치 한도 안의 관측469,792,598bytes/685,391reads가 완주 비용은 아니다.
경합 실패만으로30초 제품 조건을 바꾸거나 C1을 원인으로 판정하지 않는다.

후속 Source/SourceMap/BuildPrepare의 기존 Off 실패 단계 연결과 Source-only
4096 capture는 구현·집중 검증을 마쳤다. 해당 capture의 SourceAdvance는
27.735796167s에 끝났고 원 operation의 durable Published receipt를 확인했다.
이후 원4K 진단도 SourceAdvance를 통과해 아래 logical ready 결손에 도달했다.
이 과거 실패만으로 별도 재개발 티켓을 발급하지 않는다. 최신 실행 결과는 아래
마감 경계에서 구분하며, 원30s 수용 조건은 그대로 유지한다.
기존 bulk builder/open-path 재사용을 중복 구현하지 않는다.
ignored 진단은 기존 phase fixture setup을 공유하고 원 SourceAdvance·30s/10s·
수치 조건을 유지한 채 동일 원 operation key의 durable Published receipt 확인에서 끝난다. 원 ANN4K/Q64
수용을 대체하지 않는다. 단계별 실제 비용을 확인한 뒤 해당 SEARCH hunk만
수리하고,physical 하위 body나 수용 상한을 추측으로 바꾸지 않는다.

### Current code closeout boundary

C1/C2·Source/Build Off 실패 단계의 집중 검증과128-owner 설치 그룹 감소,
Source-only4096의 원30s 안 Published/동일key receipt는 각 기록된 source에서
확인했다. 이후 원4K 실행에서 아래 logical ready 결손이 발견돼 수리했다.
해당 최신 수리는 고정clock·Catalog/Kernel/실제pump53/53과 영향 strict를 통과했다.

제품2초 lease를200ms로 줄이던 integration fixture는 원2초로 복원했다.
provider를2250ms 붙잡아 원 lease를 실제로 넘긴 뒤에도 동일 fence·최초key 성공·
reopen·추가provider 없음이라는 기존 gold를 유지해 검증했다. 짧은 cadence/만료는
고정clock 회귀로 남아 있다. 원4K의30s/총10s·수치 상한은 바꾸지 않는다.

아래 초기 pending 편집 개선 전 ready 수리 source의 원4K 진단은 SourceAdvance30.004s의 deadline 취소로
FAILED(0/1)다. Trace Off였고 원key는 Pending,daemon은 정상 종료했다.
738782reads/508338400bytes는 수치 상한 안의 중간 사용량이며 완주 비용이 아니다.
해당 진단의 ANN materialization/Q64/reopen/change는 NOT_RUN이다. 초기 pending
편집 개선의 실제 호출/수명 회귀와 최신 quiet admission 상태는 [SSOT](README.md#e04)를
따른다. quiet admission 뒤 같은 원 gate로 판정한다.
이 실패는 ready 수리의 제품 검증에 도달하지 않았으며 그 수리의 회귀로 단정하지 않는다.
경합 override 실행은 진단이며 정식
성능/Off·Record 수용이 아니다. 새 bulk builder·worker·물리 ledger 재개발은
추가하지 않고,추가 실패는 source에 연결된 actual hunk가 확인될 때만 수리한다.
초기 pending 공유와 watchdog 수리 후 고정2085파일의 원4K 경합 진단도
SourceAdvance30.001s 취소로 FAILED0/1이다. 원key Pending/daemon exit0이며
ANN materialization/Q64/reopen/delta/path/overlay에는 도달하지 못했다.
별도 Source 단계 Record 진단은 완전trace6825건에서 Source 적용16.366s
(map7.662s 포함),lexical12.111s(postings6.072s 포함)를 관측한 뒤 취소됐다.
관측한553303reads/423048351bytes는 상한 안의 중간 사용량이고 완주 비용이 아니다.
이 경합 Off/Record 두 실행은 정식 overhead 비교나 새 구조 결함의 증거가 아니다.
최종 정상 admission은 외부Rust15/load60.24>8로 BLOCKED다. 같은 조건의
경합 반복 대신 정상 admission의 원gate를 수용하고,실제 실패가 지속될 때
확인된 Source/Text hunk만 수리한다. 물리 owner 본문/원 예산은 유지한다.
전체 E04는 미완료이며 세부 명령/이력은
[VALIDATION](VALIDATION.md#e04-current-source-4k-and-phase-rca--2026-10-23)을 따른다.

### RCA repair priority — 2026-10-23

최신 원 Off4K `3ae910f1…`는 SourceAdvance의 Published 경계를 통과했으며,
이후 ANN Compaction의 원 총10초 assignment deadline에서 FAILED다. 아래 Source
RCA는 앞선 실패 source의 근거다. 현재 다음 수리는 아래 bounded carry 절을 따른다.
SDK30s나 worker10s를 늘리지 않고 phase별 실패를 구분한다.

현재30초 실패는 ANN 계산 전 Source/Text/초기 pending의 동기 게시 경로다.
기존 trace의 COMMIT31회6.646s 중 계측 native sync는0.817s이며 Catalog lock
대기는51µs다. 별도 짧은 요청 worker sample은 Source215/245·map191/247·
postings217/242에서 파일 syscall을 관측했다. 샘플 비율은 CPU/전체 시간 비율이
아니다. profile의 Rust 심볼 제거 때문에 개별 함수의 정확한 비용은 미확정이다.
Source/Text가 완료된 진단도 pending-leaf의 기존 취소 checkpoint에서30초가 끝나
취소 줄 자체를 결함으로 해석하지 않는다. [RCA 근거](VALIDATION.md#rca-drill-down-foreground-metadata-io--2026-10-23)를 따른다.

수리 순서는 기존 owner 안에서만 고정한다.

1. Source/Text와 기존 physical admission의 반복 open/path 검증 비용을 줄일 실제
   hunk를 찾는다. 동일 attempt의 bounded 검증 custody를 재사용할 때에도 root/FD
   incarnation·child/recipe·원 예산·원인·before-I/O 거절을 유지한다. 검증 생략이나
   전역 cache,두 번째 serializer/writer로 대체하지 않는다.
2. foreground COMMIT의 자동 WAL checkpoint와 VFS 비용을 분리한다. 기본 hook의
   동기 실행은 source에서 확인했지만 checkpoint/admission별 실측 비중은 미확정이다.
   정책을 변경하려면 기존 funded maintenance의 retention-disabled·공간 상한·복구
   경로까지 먼저 연결/검증한다. auto-checkpoint만 끄거나 FULL/fullfsync를 낮추지 않는다.
3. 확인한 hunk와 독립 owning 회귀를 먼저 닫고 원4K를 한 번 수용한다. 호스트 경합을
   유일 원인으로 두거나 ANN metadata 최적화만으로 앞선 Source/Text 실패가 해결됐다고
   처리하지 않는다. 원30s/10s·수치 한도·절대1200s는 유지한다.

### Attempt read reuse implementation — 2026-10-23

RCA의 반복 물리 읽기를 Kernel의 단일 `prepare_and_commit` attempt 안에서 줄인다.
private `AttemptObjectReads`가 원 `ImmutableObjectPort`를 감싸고 SourceAdvance와
SourceResync에서만 활성화된다. miss는 원 포트의 인증/읽기/오류를 그대로 소비한다.
hit는 이미 읽은 `StoredObject`의 Arc와 원 allocation lease만 공유한다. payload,
child graph,wire,writer,Catalog 권한을 새로 만들지 않는다. 모든 쓰기는 원 포트로 전달한다.

직접 주소 슬롯은 입력 크기에 비례하되 최대32768개,보유 DATA는4MiB로 제한한다.
테이블은 원 budget의 임시 메모리를 예약한 뒤 생성하고 DATA/테이블을 lease보다 먼저
해제한다. hash 충돌은 full identity 비교 뒤 eviction이며 잘못된 hit가 아니다.
hit도 원 read/wire/decode 논리 비용과 취소/deadline을 유지한다. table lock은 대기하지
않고 resource 거절한다. 재시도·재시작마다 새 window이며 persisted/global cache가 아니다.
Materialize/Projection/Reconfigure는 기존 읽기 경로다.

SourceResync 호출부까지 포함한 최종 Source/rebase/resync/lifecycle 집중 배치는
48/48을 통과했다. 이후 추가한 table-memory 거절을 포함한 private 읽기 회귀는
9/9 통과했다. 두 결과의 중복8개를 별도 테스트 수로 더하지 않는다.
해당 배치의 생산 Kernel strict·hexagonal·영향 서식/공백 검사는 통과했다. 원4K 성능 개선과
전체 E04 수용은 이 로컬 결과로 완료 처리하지 않는다. WAL auto-checkpoint나
PHYSICAL 본문은 변경하지 않았다.

읽기 재사용을 포함한 고정2087파일 `42d5d11e…`의 원4K는 **FAILED0/1**이다.
SourceAdvance30.0022325s에서 취소됐고 원 operation2는 Pending/fence9223,
daemon exit0이었다. 부분 사용량은795058reads/505303191bytes이며 수치·메모리
거절은 없었다. 현재 cache 수리를 원30s 수용으로 처리하지 않는다.

### Bounded work-action spill — 2026-10-23

Source와 Projection의 owner Create 생성은 기존 `WorkSpool`에 최대64행으로
묶어 전달한다. private `WorkActionBatch`는 backing 메모리를 먼저 예약하고
마지막 잔여 행까지 manifest seal 전에 flush한다. canonical
WorkAction codec/key/digest·중복 제거·child 검증·Catalog publication은 원 경로다.

이전 scalar 경로의 행마다 SQL prepare·scratch 용량 PRAGMA를 반복하던 비용을
하나의 bounded reservation과 multi-row INSERT로 바꾼다. 준비된 행·payload·SQL
문자열의 메모리를 원 budget에 선예약하며 거절 시 publication하지 않는다.
논리 mutations/bytes는 scalar와 동일하다. 새 queue/worker·wire·writer나 영속
버퍼를 만들지 않으며 원30s/10s·1Mreads/512MiB 조건을 유지한다.

집중 배치55/55에서 Source65-owner의 실제 생성/flush/ready-work 경계와
Projection·Semantic Projection·Source rebase/resync를 확인했다. 64/65/129행의
독립 canonical commitment·중복 제거·scalar 논리 비용 일치,64행의 SQLite
statement run1회,미삽입 메모리 거절을 포함한다. strict가 지적한 큰 값의 인수
전달과 함수 중간 const 선언을 수정한 뒤 다시55/55·Kernel strict를 통과했다.
이후 아래 포트 비용 분리의 최종 변경 검증 결과는
[VALIDATION](VALIDATION.md#e04-attempt-read-reuse-and-work-spill--2026-10-23)에 기록한다.
이 집중 결과는 원4K·정상 host 성능·E04 전체 완료를 의미하지 않는다.

추가 추상화 감사에서 Kernel이 FileObjectStore의 읽기 비용을 복제하던 결손을
확인했다. Core `ImmutableObjectPort::read_retained` 한 계약으로 비용 권한을
원 포트에 남긴다. 기본 구현은 원 `get`이며,FileObjectStore만 자체 read/wire/decode
비용과 취소를 보존해 같은 attempt의 원 DATA를 공유한다. Kernel은 포트 반환
identity를 재확인한다. 다른 포트의 읽기8회 비용을1회로 바꾸거나 원 수치 거절을
우회하지 않는 회귀를 추가했다. Storage의 이 narrow 비용 facet 외에 기존 파일
open/검증·writer·Catalog/VFS/Common 본문은 변경하지 않았다.

후속 Source·Projection·재설정의 Retire 생산도 같은 window에 연결했다. 별도
scalar lazy 생산 경로를 제거했고 window는 scratch/budget 포트만 받는다.
Source129-owner 재설정/폐기와 기존 영향 배치 **58/58**,생산 strict를 통과했다.
실제 버퍼가63행에서 scratch를 열지 않고64행에서 SQL1회,65번째 잔여 행을 seal 전
flush하는지와 최종 메모리 환급도 고정했다. 이 변경은 아래 원4K 실행의 고정
바이너리와 별도 source이며,해당4K 통과로 합산하지 않는다.

### Bounded foreground ANN carry — 2026-10-23

원4K `3ae910f1…`는 Source Published 뒤 materialization에서 FAILED0/1이다.
공개 work 관측1944/4096 완료 후 ResourceLimited를 읽었다. Off 진단의 네
거절은 Compaction에서 first_denial=deadline,수치·메모리 거절 없음이었다.
body938.12s로1200s watchdog 전이며 SDK30s 실패가 아니다. map clear/managed
reserve의 checkpoint는 만료가 드러난 위치이고 해당 함수의 결함을 뜻하지 않는다.

consolidation은 fan-in8만 제한해64행 delta가64+128+256+512…를 연쇄 흡수한다.
이는 원10초 작업 안에 수천 owner의 인증/선택/COW 설치를 끌어올 수 있다.
새 정책은 같은 attempt의 일반 foreground carry를 기존64행 window×8=512행으로
제한한다. 이미 승인된 더 큰 delta는 단독 seal할 수 있고 추가 run은 합치지 않는다.
초과 live run은 그대로 유지한다. region/cell·query/routing·메모리/수치 상한·
원 claim/deadline·회복 경로는 유지하고,capacity 부족은 기존 거절을 따른다.
새 background worker·대체 writer·예산 갱신은 없다.

실제1024-member/real Lance fixture에서 기존 fan-in-only 코드는512행 bounded
gold 대신1024행을 반환해 RED1/1을 재현했다. 수리 source의 Semantic 전체 lib와
Kernel 영향 배치는 f52479bb…의287/287,생산4개 crate strict를 통과했다. 새512행 정책은10초 시간 보장이나 원4K
통과 증거가 아니다. 남는 region 수와 검색 비용이 늘 수 있으므로 동일 원 Q64/
recall/reopen/delta/path/overlay와1200초 수용을 다시 판정한다.

그 뒤 정적 감사에서 원 region cap에서도512행을 강제하면 기존 capacity carry를
막는 수용 범위 축소를 확인했다. 일반 carry는512행을 유지하되,원 region cap의
pressure 분기는 기존 region/cell 상한 안에서 fitting larger run을 합칠 수 있게
보완했다. 원 work/memory/총10초 거절은 그대로다. 같은1024-member fixture에
full-capacity/기존 범위 보존 case를 추가해 최종287/287을 통과했다. 실행한 f52479bb…의
바이너리/원 두 파일은 별도 고정했고 이 후속 source의 green으로 사용하지 않는다.

f52479bb…의 원4K는 Source Published 후 공개 완료1961/4096에서 전체1200초
경계에 도달해 FAILED다(body1239.12s에는 별도30s 실패 관측/종료가 포함됨).
그 실행에는 앞선 granted Compaction10초 만료를 관측하지 않았지만,새 정책의
전체 수용을 의미하지 않는다. Q64/recall/reopen/delta/path/overlay는 NOT_RUN.
두 경합 실행을 속도 비교로 사용하지 않는다. 마지막 정상 admission은 외부Rust3/
load18.91>8로 BLOCKED다. 현재 남은 실질 조건은 전체 처리량/제품 gate다.
같은 경합 조건의 반복 대신 고정 profile과 원 비용 조건에서 실제 원인을 좁힌다.

### Implemented code cut — metadata and dependent COW installation

materialize의 원 durable 그룹 두 경계를 기존 installer 안에서 결합했다.
Core ObjectMapBatchPort는 최대2개 metadata와 최대2개 short-key COW root를
받고, KernelServices의 명시 composition이 같은 backend의 object/map/batch
세 facet를 함께 연결한다. Native와 실제 owner/projection fixture를 연결했다.
owner member/coverage+owners/missing, directory/coverage+final coverage map을
각각 같은 child-first walk에 넣는다. Catalog 최종 CAS와 stage_work는 그대로다.
일반 backend의 독립 port 계약은 유지하며 선택한 batch의 오류를 재시도하지 않는다.

- metadata의 provisional identity와 원 payload/lease를 유지해 COW 준비에 전달한다.
  기존 FileObjectStore bank가 opaque metadata와 dependent nodes를 child-first로
  설치하고 원 ref ordering/roots를 반환한다. 새 serializer/writer/Source issuer는 없다.
- Object/Map이 같은 backend라는 사실은 Native composition에서 명시한다. 독립 port를
  포인터/identity hint로 추정해 연결하지 않는다. 임의 backend의 동작을 조용히 바꾸거나
  실패한 atomic bank를 다른 실행 모드로 재시도하지 않는다.
- 모든 original-base CAS·cursor EOF를 확인한 뒤에만 설치한다. metadata/node의 중복,
  capacity-before-allocation·동시 peak·취소/실패/후속 CAS와 원 attempt cleanup을 유지한다.
- 원 두 실제 caller를 연결했다. FileObjectStore의 독립 기존 경로와 비교해 같은
  wire/child graph·roots·old root에서 두 설치 그룹을 하나로 줄이는 회귀가 통과했다.
  두 번째 CAS 실패·취소·중복/미참조 객체도 검증했다. 후속 memory/refund와 확대
  영향 배치462/462도 통과했다. 실제 Native128은1/1 통과했고 ordinary126개의
  reserve/confirm 각5회,seal/carry 각13·14회로 감소를 관측했다. 완전 trace·유실0·
  입력변경0이다. Record 경합 진단이므로 wall-time 개선률/trace overhead를 추론하지
  않는다. 같은 조건의 원Off4K는 공개 완료2635/4096에서 원1200초 만료로 FAILED다.
  Core 실행 메서드 이동 후 영향413/413·strict와 module snapshot도 통과했다.
  이동 후 current-source quiet-host 원4K 수용은 별도 경계이며 아직 NOT_RUN이다.

이 절단은 기존 E04와 PHYSICAL bank의 좁은 합류이며 새 티켓 큐가 아니다.
정식 Off/Record·quiet-host 비교·E07 운영·E08 CI/release는 별도 기존 수용 경계다.

### Empty publication work stream

Owner-only materialization publishes an authenticated empty work-action stream.
Catalog now skips the temporary duplicate-key table only for declared count0,
still consumes EOF and checks count/digest before completing the original live
claim. A hidden action and malformed empty digest must roll back head, work and
receipt under the existing oracle. Nonempty action/completion collisions retain
the original table and guards. This removes two TEMP-schema DDL statements per
ordinary owner completion without changing a durable transaction or deadline.
Post-edit source6b3e541c... is VERIFIED226/226 across Catalog native and Kernel
lifecycle/semantic-projection owning, with production/test strict and no input
drift. The pre-edit source2fdeb2a4... originalOff4K FAILED1649/4096 at the original
1200s watchdog under contention; its daemon/test binary custody is intact.
Post-edit original4K and quality qualification are NOT_RUN. No speedup or clean
host regression is inferred from these different-source/contention results.

### Owned materialization entry validation

`prepare_and_commit` used to validate the full owned request, then the same
managed claim/key in a second clock-writing transaction before pin/build. Core
WorkClaimPort now takes the full UpdateRequest at its existing materialization
boundary. Catalog keeps exact-body validation first, then live claim/key checks
under one original transaction and clock sample. Terminal replay still uses
validate_owned_work at its separate entry; replay alone does not pass the live
materialization check. Manual work retains its prior route. Both real Kernel
callers consume this single contract; no second validator/serializer is added.

The real owned-reopen/completion fixture observed20 SQLite COMMITs before this
change. Its independently observed cost guard was fixed at<=19 before editing
production and failed20>19 with one executed body. Original request,artifact,
stage_work,coverage and final CAS phases remain. Same-rail GREEN and impact
226/226 pass at source7b75f0e9.... Final source8e5d7216... passes diagnostic4/4,
actual owned1/1,Runtime production/test strict and Core/Kernel module snapshots.
Actual Native128 passes1/1 with complete37280-event trace,zero loss/open spans,
no source drift. Kernel Catalog API uses SqliteWork/lane14;managed COMMIT stays3,
so their768/1314 observed intervals no longer alias. Ordinary install groups
remain5,with seal/carry13/14. WAL sync reduction is not observed in this sample.
The51 owned API baseline lines match the official render;unrelated shared
whole-Core snapshot drift remains. Original4K/quality and quiet timing remain
unqualified;no wall-time or fsync improvement follows from the COMMIT budget.

### Discovered logical ready-queue defect

실패 단계 연결 후 원4K 진단은 SourceAdvance를 통과했지만,이미 시작한 재시도
assignment가 오래된 신규 backlog 뒤로 밀리는 결손을 확인해 의도 중단했다.
종료 신호로 생긴 IPC/143 결과를 자연 watchdog/종료 버그로 해석하지 않는다.
고정 ManualClock 회귀는 유효한 원 publication과 Catalog reopen 뒤 기존 코드가
due retry 대신 신규 행을 선택하는 것을 재현했다.

수리는 `Catalog::ready_owned_work_impl`의 논리 선택 한 곳이다. due인 기존
assignment를 원 쿼리로 먼저 읽고 남은 page 슬롯만 신규 작업으로 채운다.
각 lane의 timestamp/work 순서·읽기 전용 discovery·원 absolute deadline·
max_dispatches·lease·backoff를 보존한다. 이미 deadline이 지난 것은 먼저
선택해 기존 ResourceLimited 차단으로 끝낸다. 새 queue/claim/writer나 physical
body 변경은 없다. Catalog/Kernel/실제 pump 회귀·strict 뒤 원4K를 판정하며,
이 ordering 수리 자체를 전체 처리율 개선이나 원4K 통과로 쓰지 않는다.

### Initial pending coverage edit production

SourceLocal의 prior owners와 missing이 모두 빈 root이면,검증된 새 owner는
cached/stale member가 없는 pending이다. Kernel은 이 경우 두 ordinal에 같은
편집을 복사하지 않고 한 원 stream을 EOF까지 소비해 만든 immutable root를 두
필드가 공유한다. `PendingCoverageBatch`는 기존 참조·covered/searchable·missing=false가
섞이는 것을 backing 할당 전에 거절한다. nonempty coverage·Projection·materialize는
기존 독립 precondition/두 root 경로를 유지한다. 이후 materialize가 두 root를 분리해도
old view는 원 공유 root를 보존한다.

Storage의 동일 stream fingerprint/검증·root 재사용은 이미 구현돼 있다. 이 변경은
그 재사용을 재개발하지 않고 Kernel의 중복 edit 생산·scratch 쓰기/읽기·검증을
제거한다. 저장 codec/digest/children·Catalog claim/CAS·physical installer와
원30s/총10s·수치 상한은 바꾸지 않는다. 실제 한 root/한 edit 호출과65-owner batch
경계·materialize 이후 분리/old view를 함께 검증하며,이 국소 감소를 원4K 또는
전체 throughput 수용으로 대체하지 않는다.

### Managed attempt protection SQL packet

Source/Posting의 기존 `confirm_attempt` lane8은 같은 operation/fence와 보호
INSERT를 object마다 반복했다. 이 부분만 동일 transaction 내부의 최대64행 SQL로
묶는다. 원256-object 설치 상한, registration/child 인증, object별 reclaim,
논리 work charge, row digest, CAS와 commit/fullfsync는 유지한다. SQL·bind·
key/digest backing은 생성 전에 기존 temporary memory로 승인하고, packet 종료에
해제한다. 기존 scalar 직렬화의 transient 비용을 새 retained backing으로 중복 예약하지 않는다.

고정 비용 oracle는65개 object의 초기 confirm과 replay 각각 보호 SQL2회,
모든 digest/confirmed 상태 일치다. 마지막65번째 보호 INSERT fault는 첫64행,
등록과 lifecycle 변경까지 같은 transaction에서 rollback하고 원 reservation을
보존해야 한다. 기존4KiB child-window oracle도 같은 한도로 통과해야 한다.
이 SQL 호출 감소는 전체 Source30s 또는 원4K throughput 수용을 뜻하지 않는다.

### Atomic owned entry observation

`WorkClaimPort::validate_owned_work` returns the operation status observed in the
same transaction as exact retained-request and current managed-claim validation.
Unknown/pending work needs the live original claim; terminal work permits only
its original retained replay, including after work GC/restart. This observation
is consumed by Kernel submission before any await and never serves as an
execution grant. The existing durable-body validation before provider/build and
fresh publication CAS remain mandatory. Preserve DigestConflict precedence over
generic workspace/claim-shape errors and reject expired/inactive assignments
before ingress birth. The real owned-reopen SQL fixture bounds COMMITs at18
(previously19), without reducing dispatch/lease/deadline/durability limits.

### Atomic materialization pin

Execute and rebase validate the current request/managed claim and issue the
original read-session pin in one Catalog transaction/clock sample. The ordinary
read path and this method reuse the same pin issuer: same view/root validation,
OS session lock/inode/fence, pin digest, expiry and rollback. Pin seal failure
must roll back the pin, global counter and clock floor together. No new request,
serializer, physical ledger, issuer or execution budget is created.

Keep `prepared_work` at its existing boundary after profile/coverage checks and
immediately before provider work. Moving its validation into the earlier pin
transaction could miss a lease expiring during those reads. A read pin never
grants execution authority. On CAS rebase reacquire the validated latest view;
provider output reuse still comes from the durable original work row. The
original owned-reopen SQL upper bound becomes17,down from18,without removing
the late pre-provider check or publication's fresh claim/CAS.

### Atomic owned ingress seal and admission

The original `admit_owned` boundary seals its confirmed immutable request root
inside the existing admission transaction. The standalone `seal_inputs` and
this path share the same private seal implementation: exact ingress fence,
confirmed request registration and matching prior seal remain required.
Ordinary admission retains its explicit sealed-ingress precondition. Owned
pure identity and frozen live-request checks, clock floor, current epoch,
source authority, fence allocation, custody transfer and ingress deletion all
remain in their original authorities. Existing operation replay occurs before
requiring a new ingress. Failure commits neither the new seal nor admission;
the confirmed open ingress remains available for retry. No object write,
physical confirmation, serializer or publication CAS is deferred or bypassed.

The real owned restart/provider/receipt fixture tightens COMMIT<=17 to<=16.
This is the owning fixture's count ceiling,not a measured total for every ANN
daemon operation; its adapter/layout and the daemon ANN layout differ.
Catalog rollback tests cover both the seal write and final ingress deletion,
including clock/fence and custody rollback. Rejections retain unconfirmed-root,
stale-fence, conflicting-seal, expired-claim, altered-body and cancellation
behavior. A real object-store fixture supplies an unsealed confirmed ingress
to the public owned-admission port and then tests durable request recovery.
These checks do not replace the original4K or quiet-host performance oracle.

### Source build measurement boundaries

The SourceLocal coverage path records five coarse stages: `native_coverage_scan`,
`native_coverage_map_apply`, `native_searchable_prepare`, `native_coverage_save`
and final `native_member_assemble`. Scanning includes bounded leaf/work-action
spooling; map application includes its final leaf flush. Every result, including
an error, closes the reached stage with accepted `work_reads`, `work_writes`,
`work_bytes`, `work_mutations` deltas. These are logical WorkBudget charges,
not physical I/O, memory high-water, CPU time or failed requested charge counts.
The existing first-denial/failed-stage diagnostic retains that separate role.

Off must not read the diagnostic clock, emit events, reset a budget, replace an
error or change scan-cursor lifetime. No span is emitted per owner; stages occur
only at these actual execution boundaries. Existing Catalog/physical envelopes
may share an operation parent; correlate by the same operation and time bounds,
never by temporal overlap alone or by adding overlapping inclusive durations.
No CPU/scheduler attribution is claimed without a matching profiler sample.

The next run first validates the helper's accepted/refused work and Off behavior,
recorder/schema/replay and actual Kernel source/materialization paths. A new
Record Source4096 run then selects the next repair from the actual failed inner
stage. Off/Record overhead acceptance requires the same source/config/input,
trace completeness and admitted quiet-host conditions; a contended diagnostic
pair is not comparative performance proof. The original30s/10s/1M/512MiB and
all recall/watchdog/dispatch/durability policies remain fixed.

### Profiler and durability attribution

A stack sample is attributable only after checking the executed binary and
matching code addresses/bytes against its symbols. Profile names alone do not
establish layout identity; a bin test harness or differently linked sidecar is
invalid. Keep the product executable unchanged, retain inherited Cargo jobserver
FDs in a compiler pass-through, and compare every executable/code-data section
before resolving an older address. Count actual sampling ticks, not requested
seconds divided by nominal interval. Mark a sampled/contended run diagnostic.

Managed COMMIT and native sync intervals are nested. Native sync lane11/12/13
ends before subsequent Common capacity settlement/parent sync; it is not the
whole physical durability cost. Lanes15/16/17 observe the following unchanged
AfterSync adapter dispatch for WAL/main/other files. They include lock, inode/root
checks, parent sync and settlement, not isolated parent-fsync/device time. Native
failure and post-sync refusal have separate terminal outcomes; dispatch errors
remain unchanged and Off/foreign scopes read no diagnostic clock. Lane14 stays
the Kernel Catalog API envelope. ResizeTicket currently requires authenticated
final-width **and parent sync**. A hot parent-sync stack alone cannot authorize
skipping that contract. Any coalescing must reuse the PHYSICAL create/resize/
replacement/failure custody and retain root/FD/name checks; SEARCH does not add
a second physical authority or relax FULL/WAL/fsync. Keep the original4K oracle
separate from a128-owner trace and from quiet-host Off/Record qualification.

### Synchronous Catalog diagnostic coverage

Budgetless Catalog methods originally selected `scoped(guard,None)`,while
`lock_native_budget` forwarded the original request context. A complete footer
therefore did not establish complete Catalog coverage. Historical aggregate
labels remain captured subsets,not all native IO within an owner lifetime.

The separate Core `CatalogDiagnosticPort` and thread-confined lexical scope now
carry observation only. The Catalog scopes the original context by its exact
instance and current thread,restores nesting/unwind and masks explicit Off.
Budget-aware calls retain their own context,including explicit Off. The normal
inactive Off branch does not allocate,clone context,read a clock or emit events.
Original lock,clock,claim,funding,pin and durability policies remain unchanged.

Kernel's synchronous budgetless submit/validation,pin,prepared-work/stage,
recovery and refusal calls select this context without holding a scope across
await. The original ReadTicket retains only diagnostic DATA for validate/renew/
release/retry/drop; its one Catalog supplies both capabilities. The composite
trait preserves exact data-pointer and session Arc identity checks. It does not
introduce a second reader,pin issuer or authority. Budgeted held-read settlement
retains its existing explicit request context. Public tickets created without
a work context do not inherit another request's observation.

Real IO tests must establish native/post-sync events for legacy calls while
results,work,errors and custody stay unchanged. The actual128-owner capture must
also show previously absent post-publication ticket-release sync under the
original owner,normal footer,zero health/loss/open spans and matched native/post
intervals. Coverage is the instrumented Kernel execution lifetime,not startup,
pre-grant scheduling,all process IO,CPU or observer-overhead qualification.
At source5a43de1e… these checks passed:390 owning,34 real VFS and3 actual daemon
cases; complete42265-event capture,all five legacy calls and post-publication
release covered128/128,zero health/loss/open spans. This closes the identified
context propagation omission,not all-process observation or performance DoD.
See [source-bound results](VALIDATION.md#e04-synchronous-catalog-context-20261024).

Lane21 now isolates the existing parent sync within the unchanged AfterSync
envelope. Optional original-owner diagnostic DATA changes no physical authority
or settlement policy; SHM/marker/safe-file callers select None. Actual34 VFS
cases pass. A first contended128-owner run fails at100 observed completions,
Finalize/ResourceLimited; the subsequent failure-diagnostic-enabled run passes
128. Preserve both outcomes: the first exact refusal cause remains unresolved.
The existing bounded failure/renewal observer is now enabled identically for
Off/Record product fixtures,so a recurrence retains WorkBudget denial location.

The initial captures identify parent sync as96.50–98.32% of post-sync elapsed;
their passing capture records2468 parent calls/3.581689s with zero invalid trace
state. Later contended captures vary,so those values are source/run-bound.
This is elapsed call-boundary time,not device service or quiet-host performance.
Any physical coalescing remains the original PHYSICAL writer's exact hunk and
requires same-FD durability completion plus original parent/name/device/failure
proof. Configuration flags or SQLITE_OK alone do not prove a full barrier. No
parent sync,root check,lease,deadline or request budget is omitted or relaxed.
At source395833d3… the original4K body was executed once as a contended
functional diagnostic: Source Published29.032s,deadline exhausted at2916/4096
observed completed inputs. No Q64/reopen/delta/path/overlay phase was reached.
The final9ms IPC timeout consumed the original remaining deadline and is not
an independent transport threshold. The earlier100/128 ResourceLimited remains
unresolved rather than being relabeled as this watchdog failure.

The successful-run diagnostic relay now retains existing first-denial/renewal
records after confirmed shutdown. The full4K run also exposed saturation of all
eight renewal slots by fast WorkFenceLost observations. Their count is now
limited to two within the same eight-record total,leaving room for important
later observations; all original result/renewal/cancel/retry behavior remains.
Neither missing samples nor fast fence loss establishes that a lease expired.
Current8482caf0… strict,22 diagnostic/renewal regressions and3 actual daemon
cases pass. Its normal original4K admission is blocked (foreign Rust8/load13.78),
so no later4K success is claimed. Latest capture43614 events has zero invalid
state; parent sync2464 calls/6.451172s is97.78% of post-sync. This does not prove
that physical coalescing is safe or host contention alone caused the4K failure.
See [source-bound execution and RCA](VALIDATION.md#e04-successful-attempt-diagnostics-20261024).

### Parallel source audit — 2026-10-24

The initial read-only audit reads the dirty tree at HEAD c32cd191…; it does not promote
the earlier8482caf0… focused results into a current full-product pass. Four
disjoint reads cover Catalog/worker scheduling,physical durability,quality/
diagnostic oracles,and Kernel/Semantic materialization. No Rust build,test or
benchmark is executed during the audit. Findings and their repair contracts:

| Class / existing owner | Confirmed source behavior | Smallest repair and decisive oracle |
| --- | --- | --- |
| P1 / Catalog scan + Runtime pump integration | `ready_owned_work_impl` checks its WorkBudget before a blocking Catalog lock. Empty due/open results can return successfully after the original scan deadline; this does not bound mutex wait. | Use existing `lock_native_budget`,check the original budget after SQL/commit before success,and carry a scoped `Ready(items)` or `RetryDeadline` outcome. Only the actual first local deadline refusal can produce `RetryDeadline`; reads/bytes/memory/storage failures retain their original error. Hold the original Catalog lock beyond the scan deadline; both empty/nonempty discovery must refuse without dispatching or terminating a healthy pump. A deadline check does not preempt an already-running SQL/OS call. |
| P2 / Catalog grant result + Core/Kernel/Runtime consumer | `begin_owned_work_impl` returns `None` for terminal,active,not-due,expired-lease and exhausted-policy cases. Worker records all of them as ResourceLimited; pump correctly treats start `Ok(None)` as no dispatch. | Replace the ambiguous Option with one canonical `Granted`, `Deferred(reason)` or `Terminal(status)` decision. Preserve fence mismatch as WorkFenceLost and malformed managed Pending/Unknown as Storage. Integrate typed start/skip and completion/quiescence consumption once; skip must not invent a ResourceLimited completion or new persisted work state. Test each branch and keep original deadline/dispatch accounting. |

Final counter-audit narrows the earlier fixture findings: rank order is already
an explicit NativeSemanticResult contract and is enforced before both durable
and overlay responses (`native_semantic_result.rs::validate_order`). A reversed
public result is therefore not shown to pass the real product path. An
independent fixture assertion is optional defense in depth,not another P2
product blocker. Do not compare approximate results position-by-position with
the exact golden; preserve the existing recall contract.

Provider multiplicity is diagnostic cost,not a global exactly-once contract.
HTTP max_retries=0 does not remove the original owned max_dispatches=2: a
failure before durable `stage_work` can rebuild the provider output. The
readiness probe may also issue query-0 before Q64. Retain the already-existing
no-corpus-reembedding-after-reopen assertion. Report unique/total/duplicate
calls by phase when measuring cost; do not add total==4096 or one-call-per-query
as new hard acceptance conditions.

The two confirmed repairs share the original Catalog/Runtime decision boundary
and must have one integration writer. The diagnostic cost fixture can proceed
independently; it is not a new correctness gate. Physical work owns only its
existing adapter/FileMutation/vendor hook. Agree the typed decision contracts
before parallel body edits; do not duplicate public SDK APIs,issuers,queues,
ledgers or serializers. Use the existing WorkBudget first-denial state via a
typed read-only accessor for the fresh scan budget,immediately on its failing
return. Never parse Debug text or infer the original error from a later
`remaining()==0`; an unrelated error may precede that later expiry. This local
classification is not the terminal cause of an enclosing product operation.

Core WorkClaimPort -> Catalog -> Kernel -> pump owns discovery outcomes.
Core WorkClaimPort -> Catalog -> Kernel -> worker -> pump owns grant/start
outcomes. Persisted/wire formats and original work/operation identities stay
unchanged. Empty/nonempty held-lock expiry,nonretryable quota/storage failure,
every defer/terminal/fence branch and skip/cancel/quiescence are the decisive
focused checks; they run in one batch after these producers/consumers compile.

The two producing/consuming repairs are now implemented in one canonical
process-local contract. `OwnedReadyOutcome` distinguishes Ready from an actual
fresh scan Deadline retry; `OwnedWorkBegin` distinguishes Granted, Deferred and
terminal work states. The terminal type excludes Pending/Claimed by construction.
Catalog uses the existing budget-aware mutex acquisition and checks the budget
again after SQL/commit. Core/Kernel/Runtime forward these outcomes without wire
or persisted changes. A skip waits for worker quiescence and produces no fake
completion. Actual pre-grant ResourceLimited/WorkFenceLost errors retain their
existing nonfatal start semantics; the narrower discovery rule does not replace
that policy. First-grant deadline exhaustion may block work while the operation
is still Unknown,so its diagnostic says WorkBlocked rather than pretending that
the operation returned Refused.

The existing fixture now reports bounded WorkStatus RPC count/wall time and
phase-local provider unique/total/duplicate counts on success and failure.
Its samples remain bounded; RPC wall time includes waiting and cannot be read
as polling CPU cost. The selected implementation batch passes strict,83 owning,
67 Kernel lifecycle,7 fixture and3 actual-daemon cases at source8b524b5b….
The subsequent WorkBlocked label/real pre-grant deadline regression has a
separate validation cohort; do not merge cohorts into a full-workspace claim.
See [implementation,source results and cost attribution](VALIDATION.md#e04-owned-decision-repair-20261024).

The follow-up stop audit finds a separate P2: the scan request did not carry
the manager's stop authority,and claim acquisition still used an unbounded
Catalog mutex lock. Both now consume the same original stop Arc and10s scan
budget through the canonical Core/Kernel/Catalog acquire method. No fresh
deadline is issued between discovery and acquire. A checkpoint before acquire
commit rolls back an observed stop/expiry; after commit,the existing stop check
prevents dispatch and leaves the durable claim recoverable. Only the scan's
typed Deadline is retryable; unrelated resource/storage/cancellation failures
remain errors. Control-memory binding follows each actual reservation and is
cleared when it ends. This bounds mutex waiting,not an in-progress SQLite/OS
call or the whole service join. Provider drain's5s grace begins after pump and
worker joins,so a provider-grace breach is not the reason for this repair.
Held-mutex stop/expiry,precommit and postcommit recovery,and pump classification
are the focused regression contract. Final source8693965b… passes selected
strict and214 combined cases,including all14 actual pump lifecycle cases.

Those actual pump cases exposed a separate foreground memory defect: one
changed owner reserved64 coverage leaf slots(48256 bytes) before the first
push. Source Applied and Projection now use their existing sealed mutation
count to reserve only min(count,64) slots. Snapshot cardinality is unknown and
retains default64; count>=64 preserves the original flush/group boundary.
The cursor rejects excess rows before yielding them,and a mutation pushes at
most one leaf. No implicit Vec growth or memory-authority change is introduced.
The1-row leaf reservation is754 bytes on this host; this is not an RSS estimate.
The original64KiB fixture and its publication/worker/root oracles remain intact.
See [failure,first denial,repair and final214-case result](VALIDATION.md#e04-bounded-coverage-closure-20261024).

The follow-up control audit found another bounded-request defect: WorkStatus,
manual ClaimWork and manual RenewWork discarded the original caller budget
before the Catalog mutex. They now borrow that same WorkBudget through the
canonical Core/Kernel/Runtime path. WorkStatus and ReadyWork recheck after the
read transaction/guard ends. Manual claim/renew check before COMMIT: cancellation
rolls back the fence/lease change; a successful COMMIT returns its actual durable
grant even if cancellation arrived during COMMIT. No synthetic unknown result,
new deadline,wire format or extra issuer is introduced. Managed grant renewal
retains its separate frozen assignment contract. Real held-lock,precommit rollback,
commit-hook cancellation and Kernel forwarding cases are the integration oracle.
See [current control-boundary validation](VALIDATION.md#e04-control-budget-closure-20261024).
This is a cancellation/occupancy repair,not a proved4K throughput improvement.
Control source448 passes222 owning/3 daemon cases. IPC source8b63 replaces idle
accept sleep with listener readiness while preserving shutdown cadence,and
passes76 IPC/17 service cases. Final source08b915b0… changes only the readiness
fixture: minimum500ms after an incomplete64-status page,up to1s without progress,
reset on actual completion,and capped by the original deadline. The prior50ms
reset trial still produced16766 status reads; the final128 diagnosis uses3972,
retains exact128/128 receipts,and produces a complete82641-event capture.
Its strict/5 fixture/actual128 checks pass. This is a bounded observer policy,
not a claim about CPU/RSS or paired throughput. Original4K and E07 stay open.
The adjacent public Status/ReadView legacy entrypoints are recorded under the
existing [E06 service boundary](E06-sdk-service-cutover.md#native-control-request-boundary),
not duplicated in this E04 implementation lane.

**Physical performance candidate,not a proved correctness defect:** the pinned
Rust1.92 Apple `File::sync_all` directly calls `fcntl(F_FULLFSYNC)`; held-parent
sync therefore adds a potentially second full barrier after native SQLite sync.
SQLite itself can fall back from native F_FULLFSYNC to fsync. That fallback
alone does not prove a current durability bug: current parent full-sync follows
native sync and its failure prevents settlement. An initial audit claim to the
contrary was rejected after tracing through the pinned standard library.

A candidate protocol is authenticated held-parent fsync before the original
native full barrier,then settlement only after same-native-FD raw full-barrier
success. It requires a per-attempt handle/inode/parent/sync witness and explicit
fallback failure handling; PRAGMA/readback or SQLITE_OK alone cannot supply it.
The current sync order remains until same-writer/FD/name/root and parent/native
failure,truncate,close/restart oracles prove the new protocol. The candidate is
not an approved immediate production edit. The initial hooks lacked raw syscall
outcomes; the later diagnostic probe records same-file/default-call provenance
and raw/fallback outcomes without granting settlement authority. A production
single-attempt proof and restart/fault equivalence are still required. Further
Catalog commit coalescing is also conditional on proof
that original reservation/receipt/ACK boundaries permit it; prior19->16 owning
COMMIT work is reused,not reopened from a generic batching suggestion. No parent sync is
silently omitted. The6.451172s parent observation is97.78% of post-sync only;
neither a safe speedup nor dominance over total4K latency is established.
See [Apple's fcntl contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).

**Do not turn unproved hypotheses into implementation tickets.** Existing
maintenance visits one region per ordinary publication; the first live member
can end that visit. Foreground carry is already bounded at512 rows. Kernel
materialization uses the original Semantic prepare/COW/Lance path. No second
incremental indexer or unbounded ordinary-region scan was found in these reads.
Terminal due=i64::MAX parking rejects the completed-history scan hypothesis.
Renewal's shared Catalog mutex and repeated assignment reads are cost candidates,
but current fast fence-loss records do not establish starvation. Readiness RPC
count/wall time is now measured in the bounded diagnostic fixture; its observer
effect and CPU contribution remain unqualified.

The raw native sync observer preserves the production order and syscall/error
semantics. Source96578416… passes155 parser cases,85 owning cases,2 Kernel trace
cases,3 actual-daemon cases and selected strict. Its128 trace is complete with
46342 events and2602 observed default full-sync successes,no fallback or loss.
The same source's4K SourceAdvance-only diagnostic passes at21.662s with the
original30s/1M/512MiB request contract. Source and owner costs differ: the
21.617s server Source request contains source apply5.965s,build prepare12.930s
and publication1.330s; lexical8.007s and coverage-map3.839s are nested in build
prepare. All137 native sync observations in that request report default full
success. These inclusive phases are not additive CPU costs,and neither the
Source-only pass nor contended128 timing qualifies full4K/observer overhead.
The test syscall oracle now injects fcntl per call instead of modifying SQLite's
global syscall table; its post-edit focused validation has a separate source.
See [raw observation and source-bound execution](VALIDATION.md#e04-native-sync-observation-20261024).

The earlier original4K at sourcecf6a3314… is FAILED0/1: Source Published21.655s
passes the original30s request,but the shared1200s watchdog expires during
initial materialization at observed2719/4096. The subsequent non-atomic public
snapshot reports2739 completed/1355 pending/2 claimed; it is not the readiness
count or temporal first-failure proof. Q64/reopen/delta/path/overlay are NOT_RUN.
That cohort superseded source395833d3…'s2916/4096;the current source74f80886…
subsequently failed at2326/4096 with Source Published26.050s and query0.
See [the current source-bound result](VALIDATION.md#e04-prepared-delta-closeout-20261024).
These contended cohorts do not establish a throughput trend. The cf6 readiness fraction implies
roughly1.5x its observed average materialization rate is needed even before
reserving time for later quality phases. This is planning arithmetic
under the same-work-distribution assumption,not a benchmark forecast or proof
that any one repair supplies the gain. The fixed30s/1M request contract and
1200s whole-fixture gate remain product requirements. After code fixes,record
grant reasons and readiness RPC counts alongside materialization/seal/sync
phases in one decisive capture. Canonical Catalog lock/guard observations now
add `catalog_lock_wait` and `catalog_lock_hold` to the existing closed stage
vocabulary. Wait covers entry through acquisition and its budget validation;
hold starts at scoped guard construction and finishes after actual mutex release,
before VFS scope cleanup. Both use the original context as siblings, preserving
original errors, panic/Aborted and RAII release. Normal Off adds no clock/event/
allocation calls, but its fixed guard footprint and branch are not zero overhead.
Direct measured attribution is recorded in
[Catalog/control validation](VALIDATION.md#e04-e06-catalog-control-closure-20261024);
RPC wall time alone is not mutex occupancy. Bounded8-record samples
and a64KiB stderr prefix are diagnostic samples; their absence/truncation cannot
certify that no failures occurred. Reuse existing diagnostics rather than adding
a second tracing framework. Formal Off/Record acceptance remains E07.

### Bounded install correction after the current original4K failure

The d07ba483 correction skips an identical clock UPDATE only after authenticating the
durable floor and preserving all original error checks. It does not make lease,
pin or prepared-work decisions advisory. ANN file chunks use the original
attempt/installer/custody path in groups of at most64 entries and1MiB payload;
singleton keeps scalar no-copy installation. All additional copies and output
metadata remain funded by the original budget. Manifest file/chunk order,
hashes,child union and descriptor identity do not change. Partial child failure
or cancellation leaves no descriptor and retains existing attempt reclaim.

Neither the equal-ms frequency nor the full4K benefit was established. That
source d07ba483… passes strict,61 owning and3 actual-daemon cases;its128 capture
is complete/lossless. Worker install pairs are649 versus657 in the earlier128
cohort. This is a work-count reduction,not a paired speedup or sufficient proof
for another expensive full gate. Existing
C1/C2 batches and durable stage_work remain. The latest failure,source/command,
candidate boundaries and execution status are maintained in
[the existing validation ledger](VALIDATION.md#e04-original4k-and-bounded-installs-20261024).
The later terminal no-op,final tail+descriptor and non-seal delta cuts have
their own [current-source validation](VALIDATION.md#e04-prepared-delta-closeout-20261024);
the d07 work counts do not qualify them.

### Native sync mutation classes

Before a physical durability redesign,the same Handle now tracks diagnostic
mutation attempts through its successful settlement. Native xSync spans carry
one fixed overwrite/growth/truncate/create/no_mutation/unclassified counter.
The class lookup precedes native timing and is absent in Off/foreign scopes;
fixed enum/branch overhead remains unqualified. A class and even a successful
raw full-sync result do not grant stable-parent or settlement authority.
No production barrier is removed. Trace v4 and its native-profile consumer keep
the prior v1-v3 vocabulary/rejection contracts. Source df419812… passes strict,
76 owning and one actual128 daemon case;its v4 capture is COMPLETE/lossless.
Observed worker overwrite-parent cost is5.184s of32.829s. Even removing all of
it at zero replacement cost yields only an idealized1.188 ratio in this cohort,
not a4K forecast or a sufficient1.5x repair. Exact source-bound results and
the conditional ingress-fusion/recovery limits are maintained in
[the existing ledger](VALIDATION.md#e04-native-sync-mutation-classes-20261024).
Target actual COMMIT/full-sync multiplicity and preserve the canonical durable
boundaries. Parent credits remain a cross-layer candidate,without measured
eligible hit rate or safety proof;do not implement them merely because most
observed native syncs carry the overwrite class.

### Non-seal delta and coverage installation contract

The implemented source74f80886 ordinary-owner cut joins the ANN delta COW installation with
the already joined searchable descriptor/coverage installation. The current
non-seal path does not read the new delta bytes between these boundaries.
Artifact installation and durable stage_work remain earlier;seal paths keep
their current physical installation before reading/sealing the delta.

One canonical ObjectMapBatchPort gains prepare_one_map with an Arc receiver.
Its consuming PreparedObjectMapBatch owns the original backend,ResidentStage,
prepared root,AttemptRef and allocation grants. Preparation checks the original
base and computes COW DATA without installation. The token's root is only a
provisional encoding input,not a readable root or publication authority.
Consumption checks the same attempt and sink scope,then the additional original
coverage-base precondition before physical I/O. At most two short-key roots and
two opaque DATA objects use the existing child-first installer and original
reserve→physical sync→confirm custody. No serializer,issuer or staged-read
fallback is introduced. Drop/cancel before installation releases DATA grants
without creating storage intents;partial installation retains original custody.

Kernel passes its explicitly linked object/map Arc through prepare_searchable;
AttemptContext constructors do not gain another capability field. Semantic
uses the token only for a real,nonempty,non-seal delta edit. SearchablePreparation
owns it and prepare_parent returns one owned SearchableParentPreparation that
preserves the token. The final coverage map consumes that same preparation.
Absent composition retains the existing separate ports;an error from a selected
joined backend is returned without retrying through another path.

Independent qualification compares roots,canonical object/descriptor bytes,
child closure and selected search results with the original separate sequence;
counts actual reserve/confirm groups rather than API calls;checks both base
conditions before I/O;and covers wrong attempts,cancellation,partial/funding/
confirm failure,reopen/replay and memory release. Stage lifetime and peak
admission remain under the original caps. One group per eligible ordinary
owner is the candidate saving,not a measured speedup or a promise of4K closure.
Implementation and source-bound results stay in the existing E04 ledger.

### 4. C3 — 코드 마감 후 검증을 한 배치로 수행

1. **정적 마감:** source/dirty hunk·전체 trait caller·반환값 소비·allocation/drop·오류 전파·
   child-first 순서를 검토하고 편집 파일 format/diff·hexagonal/module 검사를 한다.
   Core/Kernel parent와 실제 Semantic producer 합류는 한 writer가 직렬 처리한다.
2. **영향 owning 한 배치:** Core 준비 계약,Kernel materialize/async coverage와 actual
   Catalog/Storage lifecycle,Semantic ordinary/carry/retirement,Storage map/실패 회귀를
   `./scripts/cargow`의 원 feature/profile에서 묶어 실행한다. 이어 영향 production/test
   strict Clippy를 수행한다. 필요한 batch 실패를 수리한 후에만 다음 단계로 간다.
3. **비용 판별 한 번:** 기존 ignored128-owner fixture와 완전 trace로 실제 ordinary 그룹
   감소·원 lease/오류/coverage를 확인한다. `complete` footer·유실0·미종료0을 검사하고
   Record 경합 결과를 observer overhead 또는 quiet 성능 비교로 승격하지 않는다.
4. **원4K 한 번:** 변경 source가 안정되고 owning이 통과한 뒤 원4K를 실행한다.
   진단 로그만 바뀐 경우를 처리율 수리로 세지 않는다. 앞 비용 capture에서 수리의
   효과가 확인되거나 원4K로만 판별할 구체 가설이 있을 때 진행한다.128 통과만으로
   같은20분 실패를 반복하지 않는다.
   SourceAdvance→4096 completion→Q64 recall≥0.95→reopen→delta/path/overlay를 전부
   원1M/512MiB·SDK30s·assignment10s·watchdog1200s로 수용한다. 변경 없는 재실행 대신
   새 실패의 actual phase/cost를 수리한다. 경합 override는 진단 범위로만 기록한다.
   source/materialization/cold reopen과 selected path/delta/overlay는 같은 원 절대
   deadline을 공유한다. 단계 전환에서 새1200s를 발급하지 않으며 이미 만료됐으면
   다음 작업을 시작하기 전에 거절한다. 실패 뒤의 진단/정리는 별도 bounded 관측이고
   그 시간이나 결과를 quality 성공으로 합치지 않는다.
5. **기존 남은 수용:** 독립 filtered exact gold·delete/replacement/churn·old-pin/fault/cancel
   oracle의 실제 빈칸만 채운다. PHYSICAL 기존 결과와 동일 범위를 중복 실행하지 않는다.
   지속 부하/RSS/disk·정식 Off/Record는 E07,전체 CI/release는 E08 경계로 인계한다.

**기존 C1/C2·metadata/COW 구현과 수리된 scan 예산·grant 이유 계약을 재사용한다.**
확정된 두 코드 결함과 비용 관측 보완의 영향 검증은 source별로 기록했다. 순위 검사는
이미 실제 응답 경로에 있고 provider 전체 exactly-once는 요구하지 않는다. Physical barrier 변경은 별도 증명 전
후보이며 확정 수리로 세지 않는다. 영향 검증,128 진단,원4K/품질 수용과 전체 E04 DoD는
별도 판정한다. 새 worker·ANN engine·일반
transaction framework·공개 SDK API/화면·batch_limit/max_in_flight 또는 예산 확대는 이번 계획에 없다.
외부 작업 채팅 조회·메시지 없이 현재 SEARCH 경계에서 진행한다. 내부 병렬 작업은
Catalog clock,Semantic ANN fileset,읽기 전용 lease/내구성 감사로 경로를 분리하고,
부모 한 곳에서 결과를 통합한다. agent 간 직접 통신과 중복 쓰기를 하지 않으며
공유 소스를 고정한 뒤 부모만 Rust/actual-daemon 검증을 직렬 수행한다.

### Transaction-local Catalog validation reuse

The original owned request validator now returns its authenticated live work
and assignment rows to same-transaction consumers. Pin and outer owned status
still perform the original pure live-claim/operation/scope checks. Terminal
and manual pin fallback keep the previous validation order. Completed
publication rechecks the retained request and operation,then uses the same
live rows and parks the assignment only after writing Completed. Ordinary
Superseded/manual parking and cross-transaction revalidation remain unchanged.
No new issuer,serializer,ledger,clock cache,work cap or durable-write omission
is introduced.

Atd38aee2a,the executed connection-local SQL oracle observes work/owned/request
SELECT1/1/2 for Unknown/Pending owned validation,pin and completion separately.
Catalog strict34/34 owning and actual128/query-scope/reopen2/2 pass. The terminal
pin rejection and its priority over a corrupt assignment row are explicit
gold. Three later assertion line wraps have explicit format-only provenance.
See [the source-scoped execution](VALIDATION.md#e04-tx-validation-reuse-20261024).
This corrects repeated authentication/decoding work;it removes no install or
sync boundary and does not qualify the original4K gate or paired timing.

### Current Source deadline and observer preparation

E07's bounded Record/stream/schema-branch work is implemented with focused
proof; [source-scoped results](VALIDATION.md#e07-bounded-record-stream-replay-20261025)
retain Rust95/Python658 and complete lossless actual128 evidence separately.
The earlier original4K Record atcbba5a25 fails SourceSubmit's original30s limit,
leaving original sequence2 Pending and no provider call. Its2.56MB COMPLETE
trace observes that product failure; it does not close materialization or Q64.
SourceSubmit Catalog hold15.767s includes managed COMMIT5.756s. Whole-process
reserve/confirm40.8s includes input staging and cannot be charged wholly to30s.

The narrow subsequent storage scratch mark completion changes per-object SQL
to admitted64-row conditional UPDATE packets, preserving each object's charge,
state/refusal and commit-before-install. Atf0dca8d6, storage strict, map77/77 and
Kernel source-batch1/1 pass. Source30s benefit is unmeasured; original4K on that
source is NOT_RUN. The existing4096 Source-only diagnostic failed under the
unchanged Source30s limit in both stripped and symbolized diagnostic builds.
The two2s samples resolve VFS per-write root/lock revalidation and SQLite cache
spill/checkpoint paths; they do not quantify whole-request CPU or establish a
safe validation omission. Lower-VFS write-call/requested-byte/error aggregates
are connected to the existing Record scope. Source062fe720… passes strict,
Rust82/82 and Python678 with2 existing skips; the historical128 full parsed
result SHA is unchanged. This verifies observation, not the Source30s fix.
The new Source-only run is FAILED0/1 with the unchanged30s limit and provider0.
Its complete capture measures111775 lower SQLite write calls and241854244
requested bytes inside SourceSubmit (WAL78526/main21733/other11516), lower
write failures0. Outside staging/cleanup is excluded. This is a measured write
amplification target, not a cache-spill count or a solved deadline.
The subsequent code cut strengthens five private reservation contracts to the
original Transaction and changes only the FK-free conditional provisional
INSERT to OR ROLLBACK. Existing named UPSERT and all child/protection/FK logic
remain. Final sourcec39a67e9… passes Catalog/runtime all-target strict and106/106
owning bodies, including late-constraint rollback and child-first replay/reopen.
The existing SQL observer retains its fixed256 count with the new statement.
At the same c39a67e9 source, the existing4096 Source-only body is VERIFIED1/1,
Published22.384457125s with unchanged30s caps. The original full4K is FAILED0/1
at Source30.000992375s/Pending/provider0; Q64/reopen/delta/path/overlay remain
NOT_RUN. Its3.14MB schema4 capture is COMPLETE/loss0. Preflight observed foreign
Rust30/load23.574 versus Source-only2/load12.413, so no causal latency or tracing
overhead ratio is valid. See the existing VALIDATION subsection for exact scope.
The next narrow cut batches the existing canonical fence counter UPDATE only
inside the same exclusive transaction, with an exact-used final CAS before
success. Sourceae051807 passes Catalog all-target strict and74/74 focused
bodies, including independent scalar256 versus final1 UPDATE, unchanged final
counter/generation and rollback/reopen gold. Its original4K is FAILED0/1:
Source18.810s Published, then one owner Blocked(ResourceLimited), public snapshot
2041/4096 completed, provider2048 unique/2049 total/1 duplicate. The original
watchdog still had about285s remaining. Q64/reopen/delta/path/overlay are NOT_RUN.
WorkBudget first_denial/failed_stage are absent; private quota, lower publication
refusal and prior lease/fence rotation must be distinguished before another run.
The616881711-byte capture is canonical COMPLETE/loss0. The failed worker has
118 successful child spans, including final Catalog reserve/COMMIT; first
external physical admission refusal and the final coverage-save stage are now
recorded in source7b56a113: strict green, budget/native-file-I/O23/23 and
Kernel materialization8/8 VERIFIED with source drift0. No full4K
completion, host-causal speedup or tracing overhead acceptance is asserted.
The following recovery cut connects the original physical authority at the
existing installer replay boundary for retired visible safe Creates. It retains
reader exclusion for live writers, original wire validation and Catalog/sync
ordering. This code-backed recovery seam is not yet the measured full4K cause;
source332c3ec8 passes strict/boundary checks and owning97/97 plus Kernel8/8.
The one original default-Off4K is FAILED0/1 at Source30.0017715s/Pending/
provider0, BuildPrepare cancellation, below numerical caps, external refusal
None; hostforeignRust2/load20.753 is not quiet. Worker and later quality stages
are NOT_RUN; the retired-create repair is not proved as this failure's cause.
The subsequent255493d6 Python-only terminal fold passes684/2 existing skips
and the full588MiB old/new public-result digest equality. It does not change
the Source/worker binary or qualify the original4K product gate. No new cache,
unsafe/vendor status bridge or durability bypass is introduced.
The existing4096 Source-only diagnostic now has255 source symbolized
samples. R2 proves both stack collection and independent1s CPU/I/O windows
inside their respective canonical lexical/postings stages. Source still FAILS
at30.002143209s with BuildPrepare cancellation, below numerical limits and no
external refusal. Whole Source Catalog hold16.245967708s and COMMIT6.838002293s
overlap. The postings stack reaches final top-map DAG installation, managed
confirm, SQLite pager spill/subjournal/WAL and original per-write root/allowance
checks. This does not establish whole-stage cost shares or observer overhead.
Normal preflight is BLOCKED atforeignRust16/load35.15625; the explicit override
is diagnosis only. Original4K/Q64/reopen/delta/path/overlay remain open.

The historical c39 Source-only capture now bounds the complete serial
pre-postings inventory, including all token INSERTs, at1.157187708s of its
22.384457125s Published request. Postings is6.636377834s, with contained Catalog
hold4.960253460s and COMMIT2.313156499s; inclusive values overlap. This is the
same historical run, not current332/255 timing. In initial Source preparation
the terms root stays empty until the final top apply, so scalar-to-batch old
wrapper lookup cannot reduce map-node reads for this fixture. Neither that
incremental-only candidate nor the unmeasured token INSERT patch is accepted
as a Source30s repair. Current R2 confirms the spill/write path; a safe reduction
must preserve Catalog replay, child-first and durability contracts. Existing
Retained memory authority can outlive execution, but a connection cache policy
must retain its lease through actual checked close and cannot treat advisory
cache_size as a hard memory bound. No cache increase or guard removal is applied.
The later26d99ebf fee census avoids a discarded metadata Vec without changing
WorkUsage, SQL or product budgets; strict/hexagonal and Catalog252/252 pass.
Its Source30s/full4K effect is NOT_RUN and is not inferred from those regressions.
The following39e9a830 cut addresses the reached confirm protection path with
cached single-row OR IGNORE statements, retaining64-row preparation/reclaim
ordering and the original outer transaction, FKs, budgets and durability. Its
strict/hexagonal and Catalog252/252 are VERIFIED; Runtime VFS is FAILED42/43
on the new discriminator's pending WAL custody after raw connection close.
The focused9d0a6eb0 probe confirms pins0/no uncertain close, but does not reach
the scalar arm. At86d2f5bb only the Runtime test changes to use the existing
Catalog owner through checked close/reopen. Runtime43/43, Kernel8/8 and strict/
hexagonal pass, drift0; Catalog252 remains the unchanged39e9a830 proof. The
64-target insertion-only comparison passes98calls/262400bytes versus0/0, with
failures0 and exact reopened digests/FKs/quiescence. COMMIT is outside this
scope, so full-transaction write savings or Source speedup are not inferred.
One original default-bench Off4K at86d2f5bb is FAILED0/1 with unchanged limits:
SourceAdvance Published25.455199041s within30s, then3119/4096 observed completed
at the1200s watchdog. A later non-atomic snapshot has3140 Completed/956 Pending/
0 Blocked; provider3146 unique/0 duplicates and daemon exit0. Original Q64,
reopen/delta/path/overlay are NOT_RUN. Source drift0; normal preflight is
BLOCKEDforeignRust2/load14.39502, so no timing comparison is inferred.
The nexta5e4e974 code cut changes prepared_work's clock read to the existing
advisory validator while stage/final publication still persist/revalidate time,
claim and artifact. Strict/hexagonal, Catalog253/253 and Kernel8/8 pass, drift0.
The existing128-owner Record diagnosis passes29.44s with all128 worker roots
successful and a COMPLETE/loss0 trace; its128 prepared calls have0 native writes.
Full4K benefit is unmeasured. API trace lanes23/24 separate prepared/stage calls.
No unchanged original4K retry or limit relaxation is assigned.
After a fresh normal preflight passesforeignRust0/load7.953125<=8, Root runs
one original default-Off4K ata5e4 without contention override. Its reached
SourceAdvance is Published16.8077325s within30s. All4096 exact materialization
receipts are observed in1143.915s, but initial Q64 seed48 exhausts the original
shared1200s deadline(IPC Read timed out after11ms). Body FAILED0/1,1211.53s,
provider4096 unique/0 duplicate rows,48 unique/49 total queries; daemon exits0
and2106-input drift0. Full Q64/reopen/delta/path/overlay are unqualified. Root
next uses one explicitly bounded4K-source/128-completed-prefix Record diagnosis
without changing the original quality fixture, product limits or funding.

That471ae8d6 diagnostic is VERIFIED1/1(90.13s body), source2106 drift0; clean
preflight and default bench Record. Source16.175s is Published;129 exact public
Completed receipts are observed in50.143s. The closed45.7MB schema4 capture is
COMPLETE/loss0 and contains199ok/1shutdown-aborted worker roots, not200 accepted
completion receipts. Its early4K costs inform the next repair; original Q64,
reopen/delta/path/overlay and sustained/RSS/fault qualification remain open.


The following092f5788 cut fixes same-tx Catalog operation-read reuse and opt-in
same-callback capacity detail/normal-stop relay. Strict/hexagonal, VFS49/49,
Kernel8/8 and relay2/2 are VERIFIED at092f(2107-input drift0); Catalog254/254 is
reused at806f with identical Catalog source. Those are focused behavior checks,
not current original4K or paired timing acceptance. Full4K is NOT_RUN for this
cut; the last original run remains a5e4 FAILED at initial Q64 seed48 after all
4096 materializations. The next work must reduce proved product costs and retain
30s/1M/512MiB, owned10s/dispatch2/lease2s/fanout1 and the shared1200s oracle.

The following80e62276 cut removes bounded namespace/key and ANN delta duplicate
work while preserving per-call namespace/member authentication and transaction
boundaries. Catalog255/255, Semantic234/234, Kernel8/8, owning strict/hexagonal and
the original Runtime192/12 filtered-exact gold1/1 are VERIFIED, source drift0.
The same-source partial4K diagnosis observes Source Published16.500s and131 exact
Completed in50.119s, with a COMPLETE/loss0 capture and bounded OS windows. These
contended observations establish neither a latency improvement nor full4K
quality. The last original full4K remains a5e4 FAILED at Q64 seed48;80e6 full4K
is NOT_RUN. [Current implementation, commands and remaining gates](VALIDATION.md#e07-catalog-ann-cost-cut-20261025).

### Selected artifact DATA and causal phase contexts — 2026-10-25

The current code borrows only the artifact already authenticated by the original
object port inside one pinned `materialize` call. Matching identity delegates to
that port's `read_retained`; other identities keep `get`. No extra table, memory
reservation, FD retention or cross-pin cache is introduced. Logical read/byte
fees, cancellation and adapter/member validation remain. Source's prior bounded
attempt window is a separate existing path. The owning caller witness compares
physical reads against the original-port baseline while requiring equal WorkUsage.

BuildPrepare, Finalize, SemanticPrepare and NativeCoverageSave now temporarily
parent actual child work through the original WorkBudget. The request is restored
on return/error/unwind/future drop; deadline, grants, ledgers, ports and attempt
stay canonical. Off avoids diagnostic request cloning and clock reads; zero CPU
overhead is not claimed. The Catalog owned-ready lookup consumes the canonical full row validator once
in the normal same-transaction path; corrupt cross-key rows keep their original
second identity lookup and error priority. Shadow, claim and root checks remain.

The edits are focused VERIFIED: Core307/Catalog256/Kernel118 atba31b8a5;
final Kernel strict/lifecycle9 and actual daemon filtered gold1 at67741542.
Only the new lifecycle test's setup changes between these sources; library proof
is reused explicitly. The same-source partial4K capture is VERIFIED1/1:Source18.232s Published,
128 exact Completed/51.711s, COMPLETE/loss0 and actual managed ancestry under all
four phase kinds. Original4K admission is BLOCKED(foreign Rust5/load29.469727>8),
so its body is NOT_RUN. This does not close original4K/quality gates. [Current closeout scope](VALIDATION.md#e07-selected-artifact-phase-parenting-20261025).


The subsequent314666fa Runtime observer-only repair passes strict/VFS48,
short-write/actual ANN last-pin2 and UDS fixed-cap1. The existing managed ANN
seal real-ENOSPC witness now passes1/1 on a disposable64MiB HFS+ mount with
same-operation recovery and confirmed detach. The symbolized diagnostic partial4K
also passes1/1(Source17.847s,128 exact Completed/55.578s, COMPLETE/loss0), and
actual daemon UUID-bound function frames distinguish read/check/close and
checkpoint/sync paths. Default artifacts are restored by Cargo; current314
independent filtered192/12 gold passes1/1. Following the earlier admission block,
one normal-admitted314 original4K Off body is FAILED0/1 in initial materialization:
4020/4096 exact receipts at1162.509s,Source17.246s Published,provider4034unique/
0duplicates/0queries. The watchdog expires before Q64; subsequent quality/reopen/
delta/path/overlay remain NOT_RUN. New source-backed cost RCA precedes any retry;
every fault, paired overhead and steady-state bounds also remain open.
[Current source and proof](VALIDATION.md#e07-sync-observer-reentry-20261025).

The following5510c600 Catalog stage_work-only update keeps authenticated shadows
and durable boundaries while removing unchanged indexed columns from SET.
Focused strict/Kernel73 plus existing skip1/daemon filtered gold1 and corrected
Catalog owning1 are VERIFIED; the other256 are explicitly reused from ec9b's
identical production body. A same5510 Record prefix also passes128 exact
completions with COMPLETE/health/loss0. Full original4K on this source is not
yet qualified; the prior full4K failure at314 remains retained before Q64.
After two normal-admission blocks, the5510 whole4K Record RCA starts at
2026-10-25T02:17:05Z with foreign Rust0/load7.351562<8 and no override.
Source4096 is Published in18.232688792s, but the original1200s body is
FAILED0/1(1217.26s body): public3556/4096 at1160.546s before Q64. Provider
3576unique has0duplicates/0queries. Closed-trace replay is COMPLETE/healthy/loss0. Catalog hold828.183s and
native+post sync union499.929s dominate observed costs; intervals overlap.
The different-source Record/older Off pair is not observer-overhead qualification. No limit or durability change is
made to turn the older failure into a pass.
Followingb62c47c9 repairs managed diagnostic parenting only: strict, owning1
and actual128 phase diagnosis1 pass. All1094 actual managed calls have one
child Catalog COMMIT/lock/BEGIN/VFS scope, with COMPLETE/healthy/loss0.
It does not qualify full4K or tracing overhead.
[Narrow cost cut and proof boundaries](VALIDATION.md#e07-stage-work-payload-only-20261025).

Current31aec3e7 changes only the recorder fromb62: strict/recorder22 and
actual128 diagnosis/three scoped pairs are VERIFIED. It does not change the
materialization algorithm or qualify original4K. The first original4K Off body after5510 is terminal FAILED0/1(1210.32s).
All4096 public receipts arrive1084.670s; initial Q64, cold-reopen Q64 without
re-embedding and the one-owner delta completion1.019s pass. The shared1200s
expires at incremental Q64 seed43; remaining quality/path/overlay do not pass.
Source17.506786541s Published, corpus provider4096unique/0duplicates and source/
binary drift0 are observed. This is not paired speedup or full4K acceptance.
The next52296745 code batch implements one-call map DATA reuse and retained
routing descriptor reads under the original pin, exact logical charges and
memory/cancel limits. Owning strict and focused132/132 are VERIFIED. The first current-source original4K admission is blocked by foreign Rust2.
After those identified processes terminate, fresh normal admission at
2026-10-25T04:28:55Z(foreign0/load5.656738) starts the one original body. It is
FAILED0/1(1210.51s): Source15.487306375s Published;4096/4096 materialization
completes1163.968s, then selected source inventory expires with4ms IPC budget
under the shared1200s. Provider4096unique/0duplicates/1query; daemon exit0,
source/binary drift0. Current Q64/reopen/delta/path/overlay are NOT_RUN. The
query optimization's full-body effect is not measured because the failure
precedes Q64; historical31 quality cannot qualify522. No unchanged retry is
assigned.
[Current query code and exact scope](VALIDATION.md#e04-query-scoped-data-20261025).
[Current source and original-body queue](VALIDATION.md#e07-trace-writer-allocation-20261025).


<a id="deferred-owner-install-contract"></a>
### Deferred owner install contract — focused verified, original4K pending

The current full result makes materialization the required next source target.
Ordinary owners still install four managed groups: immutable request, artifact,
Finalize metadata+owners/missing COW, then descriptor/delta/coverage COW. Query
DATA reuse cannot remove those worker groups. Closed5510 attributes3519 ordinary
workers to this shape; its costs are not a current-source speed prediction.

The implementation extends the existing FileObjectStore ResidentStage and
canonical ObjectMapBatchPort. Core supplies one same-attempt prepared owner
scope; Kernel passes its stage-aware facets to the existing single Semantic
preparation and consumes its original token:

- `stage_work` keeps its installed artifact frontier. Prepare the original two
  metadata DATA and owners/missing original-base edits without installation.
  The token owns original payload/wire allocation leases and attempt identity.
- Lend only stage-aware canonical object/map reads for these provisional refs;
  use existing stored-wire decoder, radix lookup/scan/batch lookup and selected
  member/recipe/children validators. A provisional reference is never passed to
  the ordinary backend as if installed, published, pinned or registered.
- Nonseal SourceLocal Semantic prepares once, adds delta to the same resident
  stage, then appends directory/coverage DATA and the original coverage edit.
  Bound this route explicitly to at most4 DATA/4 independent roots; the original
  two-root API is not sufficient and must not silently accept a larger packet.
- Before first durable reserve, validate all original-base preconditions and
  complete child→parent reachability. Install through the existing walker and
  its <=256-object/64MiB physical split limits. Preserve reserve→physical sync→
  confirm and final Catalog CAS, ordinal/reference checks, fences and deadlines.
  Four→three is an ordinary install-group target, not a guaranteed one-COMMIT
  count or promised speedup.
- Seal, projected reverse-map edits, retirement unlink and other early writes
  flush through the same canonical stage before the first real write. Staged
  structural diff/batch/scan/lookup reuse the original algorithms; no Semantic
  preflight or second execution is added. Do not
  retry a failed selected backend on another engine or fabricate installed refs.
- Token drop performs no I/O; late partial physical failure remains in original
  attempt custody/reclaim. Keep artifact reuse, old/current roots, ABA/delete,
  cancellation/memory/error priority and no publication on failed installation.

The Core/Storage/Kernel implementation is written. Sourcea646 strict and
focused139/139 including actual UDS192 are VERIFIED at that source. Its next
normally admitted original4K is terminal FAILED0/1(body984.14s), not a deadline:
Source14.672099875s,4096/4096 materialization881.921s, initial/cold/delta Q64 and
selected path checks pass before the unsaved overlay Fast query fails. The
unquoted `row-0` is a valid public Keyword token sequence; Native incorrectly
assumed one token. The same E04 boundary now repairs Native execution and
preflight rather than quoting or weakening the fixture. Source4322046b strict
and focused157/157 pass, including NativeText140, real UDS overlay8, managed
fault/last-pin6, fixed-cap pressure1, independent filtered gold1 and actual
isolated-volume ENOSPC1. The one unchanged-budget contention-override original4K
is FAILED0/1(body1209.84s) at the shared1200s overlay deadline, with source/binary
drift0. Source4096 publishes15.334886083s;4096ready1037.647s, initial/cold/deltaQ64
and selected path checks pass. The old Keyword InvalidInput no longer appears;
the whole overlay sweep and remaining final checks are not accepted. After the
previous foreign build ends, a fresh normal attempt is again BLOCKED/body0 by
new foreign Rust4/load9.756348>8. No Root Rust/SUT remains live. Continue only
after normal host admission; do not repeat the unchanged contended body or
raise product limits. These contended timings do not qualify performance.
[Current RCA and source](VALIDATION.md#e04-native-keyword-sequence-20261025).

Explicit Prepared/Transferred/Installed completion distinguishes transferred
owner metadata from an early flush. Kernel requires exact selected refs/roots;
Core checks bounded output ordinals. Original artifact/stage_work frontier,
physical-base CAS, all installer barriers and publication remain. No duplicate
E03 ticket or new runner/engine is created. Root owns the common contract,
preflight accounting, documents and serial verification; exclusive Native
query and Runtime owning-test writers report only to Root.

### Current query diagnosis handoff — 2026-10-25

The later normal-admitted432 body starts at07:31:07.581691Z with foreign Rust0/
load3.952148. It reaches Source14.343913208s,4096ready1038.464s and initial/
cold/deltaQ64 plus selected paths, then is externally interrupted. Its process
has stopped, no harness summary/result exists, and it supplies neither final
PASS nor deadline FAIL. External output:
`/private/tmp/qi-e04-keyword-sequence-original4k-off-20261025-u0h5c3zt`.
Do not restart this unchanged Off merely because its terminal handle is gone.

The actual residual is phase attribution, so the existing SEARCH/E07 boundary
now adds bounded query observation to the original runtime/Kernel/provider/
NativeText paths. It preserves the original WorkBudget identity and all fixed
fixture, quality and product limits. The integrated source7a0828f7 strict/focused274/274 and Python702/2existing skips
are VERIFIED. Its original4K Record is FAILED0/1 before query at Source30s in the supported
contended diagnostic mode; no full4K or throughput result is claimed.
One source-bound original4K Record follows only after decisive checks, under
normal admission or the user's existing supported diagnostic contention mode.
[Implementation and current evidence](VALIDATION.md#e07-query-phase-observability-20261025).

The final8387c986 differs from product7a only in two reusable integration-test
paths. Its owning Runtime product-target strict and actual-daemon UDS Record
regression1/1 pass. This supplies the missing small On ancestry/result witness,
not4K performance. Normal-only admission09:16:00.113374Z is BLOCKED/body0 at
foreign Rust3/load17.783691>8; no unchanged contended body follows. The fixed
Source30s and global1200s remain. [Final test and next gate](VALIDATION.md#e07-actual-uds-query-trace-20261025).
The next two consecutive goal continuations also fail normal admission, latest
09:30:13.932041Z with foreign Rust3/load32.417480>8. No body is restarted under
contention. The full goal is BLOCKED on this repeated external-state condition,
not complete; source-bound query RCA and original4K acceptance remain required.


### Code-first continuation — 2026-10-25

Host admission blocks performance execution, not independent source remediation.
The user's latest direction prioritizes remaining code and batches validation.
Source1624aff0 now removes repeated immutable BM25 average/IDF derivation by a
fixed64-slot per-query scalar table, initialized at the original first hit. The
original eager stats/tf/dl and finite-score checks, score addition order, posting
EOF/authenticity/declared-DF validation and logical work charges remain. No
cross-request corpus/vector authority or second engine/cache service is added.
Independent fixed common/rare formula bit-gold, term-order/results/usage and
corruption-before-boolean-miss cases are added. This is a structural CPU-work
reduction, not a measured critical-path cause, full4K speedup or acceptance.

The query refusal observer is connected through the original Runtime transport,
ANN diagnostic relay and existing actual daemon witness. Original4K body/config/
quality and limits remain; only the relay whitelist/fixed relay test changes in
ann_quality.rs. Initial lexical143/143,diagnostic9/9 and UDS42/42 are VERIFIED.
The actual witness exposed postterminal cancellation sampling noise; the final
code reserves6 of8 slots for other errors and distinguishes a real preterminal
refusal. Final eed03da5 Runtime all-target strict,diagnostic10/10,UDS42/42 and
product3/3 are VERIFIED with source drift0. NativeText143/143 stays on its
unchanged earlier1624 lexical source, not a single-source198-test batch. The
inspected6-path static review finds no confirmed P0-P2 defect; this is not
original4K or full performance qualification.
[Current implementation and results](VALIDATION.md#e07-code-first-remaining-20261025).


### User-requested current-source original4K — 2026-10-26

Current source38f70c96 compiles in the original bench/all-feature profile and
executes the unchanged original4096x64/Q64/K10/recall.95 gate through the real
daemon. Under the explicitly requested supported contended diagnosis it is
FAILED0/1,1073.76s: Source4096 Published25.423311s,then owner materialization
Blocked(ResourceLimited) with2097/4096 completion bindings observed. The bounded
worker records identify original total10s deadline expiry,not numeric work
exhaustion; shared1200s has not expired at the reported failure boundary.
Initial578405827B capture is COMPLETE/loss0; initial query/reopen/delta/path/
overlay are NOT_RUN. Canonical source and binary digests match. Current result
and the Catalog/durable-metadata cost and worker identity limitation are in
[the exact execution record](VALIDATION.md#e07-user-requested-original4k-20261026).
No source-only phase or complete failure trace closes the original full gate.

### Original grant/trace connection

The existing bounded refusal path now retains the original grant/claim and
actual worker capture/span; pregrant and unavailable trace remain explicitly
null. It preserves the original deadline/cancel/settlement and success path.
Final affected regressions21/21 and Runtime library strict Clippy are VERIFIED;
full-target strict retains separate existing Core/risk-suite failures. This is
RCA instrumentation, not the original4K performance repair or acceptance.
[Implementation and current-source validation](VALIDATION.md#e07-owned-grant-trace-correlation-20261026).

## DoD

The two scoped oracle gaps identified in the current source inventory now have
focused passing witnesses without adding product features. The public lexical-scope branch returns authenticated finite
exact ranking before ANN graph traversal;the existing two-owner product scope
and selected-owner boundary tests already prove its binding/refusal behavior.
A 192/12 narrow/broad independent exact top-k/score witness passes ataa75b355,
keeps ExactExhausted and does not claim graph filtered-approximate recall.
Existing original4K quality/cold-reopen/replace/delete/path/overlay and actual
managed ANN ENOSPC recovery are reused. At4f1b79f9,the managed last-pin witness
preserves the unshared searchable child closure's actual inodes while pinned
and verifies their absence after original terminal-work/history/reclaim;current
gamma search remains unchanged. Generic source retention and pin counts do not
substitute for this oracle. [Source-specific results and repaired fixture gaps](VALIDATION.md#e04-filtered-last-pin-oracles-20261024)
do not close the broader4K/churn/operating requirements below or create a new
implementation ticket queue.

- [ ] genuine complete affected output의 desired/covered/missing·exact small-scope cosine gold·certified chunk/projection delta가 일치.
- [ ] 원4K의 full publication→ANN/Q64→reopen→delta/path/overlay를 끝까지 통과.
- [ ] 실제 selected eligible corpus/filter에 대한 independent exact top-k·recall, narrow/broad·상관·삭제/churn·history retention을 고정 profile로 수용.
- [ ] full materialization/ANN seal·file installation/checked-close·same CAS·old-pin/fault/cancel/last-release를 PHYSICAL 원 oracle로 수용.
- [ ] 지속 업데이트의 region bounds·maintenance/quality debt·실제 total resources와 cached/uncached freshness를 E07에 전달.

source/phase RCA는 기존 trace와 필요한 좁은 실행으로 판별한다. observed deadline checkpoint를 지배 비용으로 단정하거나 새 engine/worker/ledger·budget 확대를 필수 해법으로 만들지 않는다. 세부 unchanged profile/layout/고정 gold는 [보존 Semantic 사양](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-13-ann-product-integration.md)에 있다.
