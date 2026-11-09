# E07 웨이브 이력

> 2026-11-05 KST에 보존한 이력이다. 본문의 날짜별 현재·다음·잔여는 당시 기록이다.
> 현행 Native 인덱싱 RCA와 신규 작업은 [bugbash SSOT](../../plans/nov-5-indexing-bugbash/README.md)가 소유한다.

<a id="e07-closeout-plan-20261030"></a>
## E07 — 이 채팅의 남은 작업 종료 순서 (2026-10-30)

현재 상태는 [README E07](../../plans/oct-22-finalizing/README.md#e07)만 소유한다. 이 절은 실행 순서이며
새 티켓이나 상태 원장이 아니다. 수리 전 검증 코호트는90c163e8/2879다. sync 통합 후보b4806510은 미수용 후 철회했고, 선택한 WAL frame 소스는5c6d0f8a/2881이다(3a050c37의 SQL 테스트 정수 타입만 수정).
operation 참조 정규화·관측기·기존 비용 수리를 다시 구현하지 않는다. 과거 초기
materialization watchdog 실패와 선택 소스의 원4K 수용을 혼동하지 않는다.
[선택 소스 제품 영수증](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-frame-product-20261031)을 재사용하며,
새 실패의 exact 코드 hunk가 확정되기 전 추정 수리를 배정하지 않는다.
다른 채팅과의 수정·실행 경계는
[E07 비중복 경계](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-nonoverlap-boundary-20261030)가 소유한다.
아래 Root는 E07/SEARCH 합류자이며 다른 에픽의 Root 역할을 인수하지 않는다.
외부 검증 helper 수리와 E07 문서 정합화는 완료했다.
측정 후 선택한 WAL frame 쓰기만 vendor의 bounded buffer/원 sync-point 함수 재사용·controlled VFS 판별·독립 Runtime 회귀로 수정한다. 기존 타 owner hunk와 sync/ABI/trace 계약은 유지한다.

### 병렬 준비와 재사용

10/30 소스/계약·빌드 실패·DoD 대조는 완료했다. 같은 코호트에서는 이 감사를
반복하지 않는다. 외부 helper의 중복1800s 제거와 모의 lifecycle4개 검증도
완료했다. 준비본 `oct30-e07-owned-closeout-20261030`을 재작성하지 않고
완료한 빌드/config 단계는 재사용하고 아래 실패 분기를 이어간다. trace/운영은 해당 후행 profile 입력만
준비하며, 준비 완료를 제품 실행 통과로 세지 않는다.

실제 타채팅 상태/최근 코드 경로 대조로 확정한 제외 목록은
[현재 owner별 제외 경계](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-live-owner-exclusion-20261030)를 따른다.
GC·DELTA·QE·Provider·L1–L4·Chunk·W2의 기존 작업은 이 채팅 TODO가 아니다.
초기 처리량의 좁은 RCA→확정 hunk 수리/영향 검증→원4K 수용은 위 영수증을
따른다. 과거90c163e8의 prebuilt Record와 canonical 분석은 이미 있으므로
재실행하지 않는다. Off3778/Record3725의 초기 watchdog 실패와
CoverageSave/COMMIT 비용을 기준으로 선택한 수정은 controlled WAL의4KiB 이하
header+page를 실제 xWrite 하나로 묶는 것이다. 원
walWriteToLog의 sync-point와 native write/부모 sync·pending/retirement를
유지하고8KiB/default VFS는 원래 경로로 둔다. sync 후보의 추가 ABI/증표는 철회했다. 초기128 fixture나 동일 소스 Off/Record를 반복하지 않는다. 독립 fixture에 Semantica 전체 완료를 선행조건으로 붙이지 않는다.

| 독점 준비 lane | 산출물 | 수정·실행 경계 |
| --- | --- | --- |
| Root / 빌드·소스 결속 | 완료한 source/command 대조·기본 bench/config/원4K Off와 외부1800s 수리를 재사용. 선택 binary의 Record와 두 daemon epoch의 canonical 완전성을 확인 | `scripts/cargow`의 기존 admission/7200s 빌드 제한을 유지한다. helper를 다시 구현하지 않으며 old-source RCA·철회한 sync 후보·선택 WAL 결과를 분리한다 |
| Source / 계약·오류 경계 감사 | 완료한 Catalog hot3의 producer→consumer·digest·old-format refusal·회귀 연결 대조 재사용. 새 실패에 관련된 exact hunk만 후속 확인 | 확인된 결함 전에는 읽기 전용. 실제 결함이 나오면 현재 preimage·기존 owner를 대조해 이 채팅의 충돌 없는 exact hunk만 배정. 외부 owner hunk는 인계 결과 소비 |
| Observation / trace·운영 입력 | 기존 capture/parser·메모리/공간 authority·Large/XL/mixed/fault 입력과 독립 gold 준비 | 기존 도구·fixture를 재사용한다. 파일/FD/capacity 생산 코드와 E01/E02 공급자는 기존 owner 유지 |

준비는 Root 내부 lane만 병렬화한다. peer·외부 채팅 메시지나 새 채팅 생성은
없다. 빌드·제품·성능·fault의 heavy 실행은 같은 host admission 큐 하나에서
직렬화하며, 실행 중 다른 lane은 읽기 전용/오프라인 준비만 진행한다.

### 단일 실행 큐와 합격 조건

| 순서 | 실제 할 일 | 다음 단계로 넘길 조건 / 실패 시 처리 |
| --- | --- | --- |
| 1. 기본 bench 빌드 | 동일 `bench-lane`의 완료된 Cargo 캐시를 재사용해 `quanta-index-searchd-runtime --test native_product_process --no-run`을 기본 bench/locked/offline/all-features로 한 번 완료 | terminal0·`Finished bench`·정확한 test executable와 embedded daemon·source/config/profile 결속. 빌드 실패는 제품 실패로 세지 않는다 |
| 2. 원4K 제품 | 같은 바이너리의 config 거절1개 후 원4096×64 Off 본문1회. Source·초기 materialization·Q64/K10·recall≥.95·score 오차≤1e-4·cold reopen·fixed delta·path/overlay 원 oracle 전체를 실행 | 원 Source30s/1Mreads/512MiB, worker10s/lease2s/batch1/max-dispatches2, watchdog1200s 유지. 정확한 완료 영수증과 source/binary/config 사후 결속까지 있어야 통과 |
| 실패 분기: 좁은 RCA→코드 수리 | 실패를 Source/worker 예산·lease/aggregate watchdog/품질/재오픈·증분/수명·자원으로 분류. 필요한 경우 현재 소스의 bounded Record1회와 기존 계측으로 해당 구간만 분해 | 새 원인과 exact producer/caller hunk가 확인된 경우만 원 owner 수리. 고정 gold와 필요한 영향 회귀를 한 번 묶고 새 소스에서 실패했던 제품 gate를 재검증. 동일 실패의 무수정 반복 금지 |
| 3. trace·성능 | 원4K 기능 성공 뒤 같은 source/binary/input/caps의 complete Record 확인. 정식 Large는 기존 capture front door로 Off/Record≥3쌍 직렬 교대 | footer/health/loss0·부모/자식 귀속·결과/논리 work 동등성 및 phase paired median 증가≤5%. 경합 진단·128 prefix·서로 다른 소스의 결과를 정식 쌍으로 합치지 않는다 |
| 4. 자원·운영 | 기존 Large/XL, fixed delta/no-op와 N/H/M·batch 축, 원60+900s mixed/churn, long pin·space pressure·cancel/panic/process-death·ENOSPC/short-write/fsync/checked-close·shutdown을 기존 fixture로 직렬 수용 | RSS≤2GiB·allocated scratch≤4GiB·physical≤64GiB, 마지막 pin 뒤60s 이내 debt0·shutdown5s, old-view 정확성·accepted publication 유실0 및 원 latency/freshness/served-rate oracle. 기존 통과 범위는 재사용하고 변경 영향/최종 profile의 공백만 실행 |
| 5. 종료 인계 | E07의 원 DoD별 source·실행·성능·운영 판정을 대조해 E08에 인계 | genuine E01–06 공급/제품 결과와 E08 A→전환→B/full-suite·CI/release는 기존 owner가 결합. 이 채팅이 해당 코드를 다시 구현하거나 별도 ANN 큐를 만들지 않는다 |

표의1–2와 선택 소스의 Record→두 capture canonical은 기존 영수증을 재사용한다.
새 raw가 생긴 경우에만 그 raw를 한 번 분석한다. 이후 **정식 관측/성능(3)·자원/운영(4)**은
각 gate의 입력·실행 범위로 판정하고 마지막에 E08이 결과를 소비한다.
확정되지 않은 후속 코드 수리는 조건부 분기이며 열린 구현 티켓으로 세지 않는다.
자원3항목의 기존 false/null·sampled 증거는 전체 수명 peak를 증명하지 않는다.
[자원 authority 경계](E07-performance-operations.md#e07-resource-authority)의
RSS·all-writable-root 근거/강제 수단을 먼저 확정하고 실제 누락된 접점만 수리한다.
전용 Linux authority/quiet profile 부재는 해당 정식 수용만 BLOCKED이며 로컬
제품·fault 코드 확인의 전역 차단 조건이 아니다. 새 sampler/ledger를 선제 배정하지 않는다.

### 전일 빌드 반복을 막는 조건

- 외부 helper의1800s 제한은 제품 SLA가 아니다. 기존 `cargow`의7200s 빌드
  supervisor보다 먼저 컴파일을 끊던 외부 중복 제한은 제거 완료했다.
  준비본에서 기존 supervisor 하나가 빌드를 감독한다. admission·그룹 cleanup은 유지하며
  `QUANTA_INDEX_RESOURCE_ADMISSION=0`이나 중첩 admission을 사용하지 않는다.
- 빌드 완료 전 실행 파일을 선택하지 않는다. 전일 실패 로그/결과는 보존하고,
  다음 결과는 외부 새 폴더에 둔다. 공유 캐시를 지우거나 cold lane으로 옮기지 않는다.
- 기능 회귀용 Catalog opt-level0 설정은 제품 빌드에 전달하지 않는다. Source30s와
  worker/watchdog·내구성·입력 크기·oracle은 빌드 감독 변경과 무관하게 유지한다.
- 현재 source가 같으므로 기존 Catalog289+수정2·생산 strict·monitor12를 재사용한다.
  dirty/untracked 자체를 재실행 사유로 삼지 않는다. 새 실제 변경이나 최종 통합
  계약이 요구할 때만 해당 검증을 갱신하며 단일291개 green으로 표시하지 않는다.
- 새 raw는 canonical parser로 한 번 분석해 작은 결과를 재사용한다. 과거 이미
  분석한 raw 전체를 다시 읽거나, 포함 관계의 wall/CPU를 합산·차감해 원인을 만들지 않는다.

로컬에서 가능한 기능·RCA·fault를 먼저 닫는다. 정식 source binding/quiet 조건과
Linux aarch642CPU/6GiB·SSD·cgroup authority가 없는 항목만 BLOCKED로 남긴다.
Mac sampled RSS를 continuous peak로 승격하거나, 전용 호스트 부재를 독립 코드
수리의 전역 blocker로 삼지 않는다. 환경 입력 없이 해당 최종 DoD를 면제하지도 않는다.

아래 W1 범용 SEARCH/PHYSICAL 표는 기존 생산 코드 owner의 배경 계약이다.
E07 제품 코드를 자동으로 다시 배정하거나 별도 원4K/성능 실행을 시작하는
지시가 아니다. 이번 실행은 위 E07 비중복 경계와 단일 큐를 따른다.

**이 채팅 종료 조건:** 원4K 전체 수용과 이 변경의 추가 확정 P0–P2 미해결0,
기존 E07 DoD의 trace/자원/운영 판정 및 외부 의존 인계 완료. 코드 합류만으로
전체 완료를 선언하지 않는다. 상태 갱신은 README E07, 실행 사실은 VALIDATION,
정확한 수정 권한은 OWNERSHIP에만 기록한다.
필수 NOT_RUN/BLOCKED가 남으면 전체 마감은 미완료이며 로컬 코드 검증 통과와
구분한다. 전용 호스트를 다시 요구하는 대신 가능한 로컬 범위를 먼저 닫는다.

<a id="e07-remaining-rca-plan-20261031"></a>
### E07 잔여 RCA와 실행 계획 — 2026-10-31

현재 상태는 README E07 한 행이 소유한다. 이 절은 기존 gate의 원인·입력·종료
조건을 구체화하며 새 티켓/runner/상태 원장을 만들지 않는다. 선택 소스
5c6d0f8a의 WAL 수리·영향240/240·원4K Off/Record·두 canonical은 재사용한다.

| 닫히지 않은 이유 | 코드/실행 근거 | 필요한 조치 |
| --- | --- | --- |
| 원4K 기능과 정식 성능의 증거 단위가 다름 | Off1030.42s/Record1163.71s는 한 쌍이며 foreign cargo/rustc max가0/0 대2/12다. 원4K는4096×64 vector-row oracle, Large는4096파일×평균120행 profile이다 | 두 실행 시간 차이를 observer 결함/개선율로 판정하지 않는다. Large의 같은 binary/input/caps·고정 phase 교대≥3쌍을 별도로 판정 |
| 정식 capture의 source/profile 입력이 준비되지 않음 | `causal_cost_capture.py:647–651`은 clean checkout와 정확한 HEAD를 요구한다. 기본 funding은 Large/XL을 거절하며 Large의 token mutation 최소치1011712는 기본1000000보다 크다 | 공유 dirty tree의 원HEAD와 선택 소스를 혼합하지 않는다. 완전한 source cut·관측 빌드·명시4축 limits를 실행 전에 고정 |
| 자원 계약과 현재 계측의 의미가 다름 | capture는 `continuous_rss_peak=false`,`allocated_disk_peak=null`을 생산하고 profile validator도 이 진단 계약을 검사한다. sampled RSS/cgroup charged memory는 전 수명 합산 RSS 상한 증명이 아니다 | RSS 상계/강제와 전체 writable-root allocated-block 권위를 먼저 검증한다. 선택한 권위가 현재 producer→consumer에 없는 경우만 그 접점을 수리 |
| bounded 기능 gold로 운영 profile을 닫을 수 없음 | 원4K·240개 영향 회귀와 과거 fault/pin 증거는960초 mixed·600초 제품 pin·전체 비용/latency/freshness 분포를 실행하지 않았다 | 완료 gold를 재사용하고, 변경 영향 공백과 원 운영 배치만 별도로 수용. 과거 source의 pass를 현재 전체 운영 green으로 합산하지 않음 |

현재 관측 비용은 CoverageSave290.658s(중첩 Catalog hold215.988s/COMMIT188.114s),
outside-phase SQLite295.726s다. 이를 합산해 critical path나 새 P0–P2로 만들지
않는다. 추가 production hunk는 미확정이다. 원30s/1Mreads/512MiB·worker10s와
watchdog1200s는 유지한다. Large/mixed의 명시4축은 그 profile의 전체 작업 입력이며
원4K 요청 예산을 완화하는 수단이 아니다.

**기준의 적정성:** 실제 상한·정확성·운영 부하를 따로 검증하는 요구는 유지한다.
원 계약은 보수적 상계/강제 증거를 허용하므로 정확한 연속 RSS 시계열·새 per-FD
ledger·추가 tracing pipeline을 새 필수 구현으로 만들지 않는다. 현 로컬에서
RSS/공간 권위가 입증되지 않은 경우 그 두 gate만 BLOCKED로 확정하며, 무수정
표본 실행을 늘려 통과로 만들지 않는다. 2CPU/6GiB 운영 profile와 RFC의
100K-file/8-core/32GiB IDE profile도 별개다.

#### 병렬 준비 → 필요한 코드만 합류

| 내부 준비 lane | 먼저 고정할 산출물 | 코드 진입 조건/완료 조건 |
| --- | --- | --- |
| Source/build | 원HEAD·선택 파일 hash·Cargo/lock/vendor/C include를 포함한 완전한 build closure의 외부 task-owned 고정본, clean source revision, Large/XL seed42·4축 limits·trace cap·시간·host/cache 조건 | 아래 격리 Git 절차의 manifest parity와 canonical source capture를 먼저 통과한다. 소스/limits/host 중 하나라도 미고정이면 formal arm 시작0. 최소 mutation 수만으로4축 전체가 충분하다고 판단하지 않음 |
| Resource contract | harness 시작 전부터 마지막 child/FD 정리까지의 소유 범위, RSS 단위/상계 근거·migration 차단, scratch/state/temp/log/trace/WAL/pin/retired/deleted-open FD의 공간 권위와2/4/64GiB cap | 각 상한의 근거·단위·누락 범위를 기존 E07 resource 절에 결정한다. 권위가 입증되면 아래 소유 경계의 후보 함수만 연다. 입증이 안 되면 필요한 입력을 명시해 해당 gate BLOCKED로 확정; 미정 방법을 구현 task로 발주하지 않음 |
| Operations impact / 비용축 대조 | 8-path WAL diff→실제 fault/pin/cancel/recovery caller→독립 gold를 매핑하고,240개/새 byte·sync·오류 gold의 포함 관계와 아래 비용축 coverage를 기존 VALIDATION에 기록 | 기존 gold가 덮으면 재실행하지 않는다. 변경 WAL partial-write/after-sync의 미덮인 case만 추가/실행. process-kill trace test를 publication recovery proof로 세지 않음. E01/E02/PUB/GC와 E08 full-suite는 기존 담당 유지 |

**격리 Git source cut의 구체 절차:**

1. 공유 checkout의 HEAD·tracked tree·선택2881 source의 path/hash/mode와 Cargo가
   해석한 내부/vendor/외부 dependency closure를 읽는다. 복사 전후 같은 inventory를
   대조하고 변경 경로가 있으면 중단한다. shared stash/reset/add/commit은 하지 않는다.
2. checkout 밖 task-owned 새 디렉터리에 실제 source bytes/modes를 복사한다.
   필요한 dirty/untracked vendor·C include도 포함한다. target/cache/.venv/log/receipt와
   중첩 Git metadata는 source copy에 넣지 않는다. gitlink나 원본 source로 향하는
   symlink로 의존성을 대신하지 않으며 필요한 dependency bytes는 기존 build custody로 결속한다.
3. 그 격리본에서만 SHA-1 Git local revision을 만든다. 이 revision은 검증용 snapshot이며
   공유 branch의 commit/push·release가 아니다. 원HEAD90a0ec34·source5c6d0f8a의
   manifest와 새 revision의 관계를 외부 출력에 기록한다. 2881 source의 exact parity를
   확인하고, 이후 자원 코드가 바뀌면 변경 경로를 명시해 successor cut을 새로 고정한다.
4. 격리본에서 기존 `scanner_source_identity.capture(cwd,new_HEAD)`가 요구하는 전체
   tracked tree/mode·clean index·untracked/ignored input0을 확인한다. manifest·호출부·
   build/trace 출력은 격리 source 밖에 둔다. 정식 `source_revision`에는 new_HEAD를
   사용하며 과거4K receipt의 HEAD/digest는 덮어쓰지 않는다. 고정본 폐기는 자기
   출력 범위에서만 하며 증거가 참조하는 동안 보존한다. build 성공만으로 이 단계가
   통과했다고 세지 않는다.

**비용축 coverage는 전체 E07 분모에 포함한다:**

| 비용축/입력 | 재사용할 실제 경로 | 이 채팅의 직접 처리와 입력 공백 |
| --- | --- | --- |
| full·1파일 delta·no-op·delete·유한 churn | `scale/native_index.rs::measure_index_native_tier`와 기존 capture/profile | 현재 producer의 fixed mutation count와 phase/result를 수용. 단일 delta를1/100/1000파일 batch 전체 proof로 확대하지 않음 |
| 1Hz 변경·50QPS·600초 pin·settlement | `scale/native_index/mixed_load.rs::run`과 기존960초 profile | 실행/비용 판정만 수행. 새 mixed runner·pin issuer·renew/release 로직을 구현하지 않음 |
| N/H/M·공통 term·history·1/100/1000파일 독립 batch | 기존 E03/E04/Kernel/harness의 실제 fixture와 source-bound 비용 결과를 먼저 대조 | 현재 Native scale의 고정 delta만으로 독립 축 전체를 수용할 수 없음. 해당 입력/독립 gold/계수 접점을 특정하기 전 새 harness 코드를 발주하지 않음. 미실행은 NOT_RUN, 필요한 입력 부재는 BLOCKED로 범위를 명시 |
| 실제 dependency fanout·branch/watcher/model/policy 변경 | 해당 source/producer와 E01/E02/E04/E05/E06의 기존 입력/결과 소비 | Native detail의 `derived_fanout`은 명시적으로 unobserved다. 별도 producer·watcher·embedding 정책을 이 채팅에서 재구현하지 않음. source-bound 입력/골드/계수가 없으면 해당 축 미수용을 유지 |

이 비용축은 Large/XL 또는960초 mixed 통과로 자동 완료되지 않는다. 기존 결과의
실제 source/caller/oracle가 닫는 범위만 소비한다. 누락된 계수 adapter가 확인되면
기존 owner의 exact hunk를 대조한 뒤 한 writer가 보완하며, 전체 공급자 구현 부재를
새 E07 production rewrite로 바꾸지 않는다.

Resource 접점의 코드가 필요한 경우 기존 관측 경로에서 증거의 method·scope·상한·
source/binary/config/epoch를 함께 결속한다. persisted artifact 계약 변경이 필요하면
생산자와 validator를 같이 변경하고, 내부 IR/collector/issuer를 이중화하지 않는다.
독립 음성 gold는 missing/forged method, sampled-to-peak 승격, 잘못된 단위,
over-cap, 누락 root/child, migration, 첫 실행/재오픈 불일치를 거절해야 한다.
권위가 없는 동안 false/null 진단 표시는 유지한다. 이 조건부 접점을 현재 확정
제품 결함 수로 세지 않는다. 정확한 후보 함수·기존 테스트·한 writer 규칙은
[조건부 자원 hunk 경계](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-conditional-resource-hunks-20261031)를 따른다.

#### Root 단일 실행 큐

1. **정적 합류와 좁은 검증.** 실제 코드 변경이 있으면 format/경계와 해당
   producer→consumer의 독립 테스트를 한 묶음으로 검증한다. 현재 WAL240개·원4K·
   canonical은 source가 그대로면 재사용한다. authority 조사로 실행 불가가 확인된
   gate는 필요한 입력을 명시해 BLOCKED로 종료하고 독립 기능 검증을 진행한다.
2. **관측 빌드1회.** 고정한 clean source에서 기존 라이브러리
   `causal_cost_build.build_scale(cwd,out_root,backend="index-native")`를 외부
   task-owned 호출부에서 한 번 사용한다. 이 함수가 cargow로 scale_matrix+daemon을
   빌드하며 독립 CLI가 아니다. 빌드 종료 뒤 host/cache 조건을 준비하고, 완료한 두
   바이너리의 exact SHA/path와 원 observed-build receipt를 후속 모든 arm에 재사용한다.
   arm마다 managed build를 반복하지 않는다. prebuilt의 UNPROVEN/diagnostic 표시를
   덮어쓰지 않고 원 빌드→source→각 arm의 transitive custody를 별도 대조한다.
3. **변경 영향 기능 gate.** 첫 Large Off의 독립 full ACK/search/reopen 결과를
   확인한다. 실행 전 exact source/binary/seed/4축 limits/trace cap/quiet host/cache/
   phase/noise 판정까지 고정됐고 다음 Record와 교대 순서를 지킬 때만 이를 첫 paired
   Off arm으로 함께 사용한다. 하나라도 빠지면 functional Off로만 기록하고 정식 쌍은
   적격 조건에서 별도 시작한다. XL은 독립 profile1회로 수용한다. 필요한 물리 fault는
   변경 영향이 확인된 case만 기존 fixture로 직렬 실행하고, 과거 ANN3개/600초
   timeout 배치를 반복하지 않는다. accepted publication 유실0·held old-view gold·
   실패 후 재오픈·원FD/charge/pin 환급과 기존 shutdown oracle를 유지한다.
4. **정식 observer gate.** 첫 Large가 통과하고 적격 환경인 경우 같은 binary로
   Off/Record≥3쌍을 직렬 교대한다. 기존22개 고정 phase(선택 churn phase는 별도)의
   wall과 work/output parity를 비교한다. 각 phase paired ratio 중앙값 증가≤5%,
   loss0이며 짧은 phase의 절대 차이·분산도 보존한다. 원4K 한 쌍과 경합 arm을
   섞지 않는다. 기존 capture/profile와 사전 고정 수동 판정 경로를 사용한다.
   Record의 출력 cap도 실행 전에 고정한다(현재 지원 상한1GiB). partial/drop이면
   그 capture 검사는 FAILED로 남기고 실제 event 생산량/집계 경계를 RCA한다.
   이미 분석한 원4K raw를 다시 파싱하거나 trace 일부를 잘라 완전성을 만들지 않는다.
5. **운영 gate1회와 인계.** Large full ACK 뒤 기존 mixed 경로로60초 warmup+
   900초50QPS·동시4/queue64·1Hz 변경·600초 제품 pin/120초 renewal을 수용한다.
   마지막 release 뒤60초 debt0·5초 shutdown, latency/freshness·실패율·원 자원
   cap과 위 독립 비용축 coverage를 각각 판정한다. 90초 opt-in pressure나20회 churn은 이 원960초 oracle를
   대신하지 않는다. E07 결과/미수용 입력을 기존 VALIDATION에 정리하고 E08이
   소비하도록 한다. 다른 채팅에 메시지를 보내거나 작업을 재배정하지 않는다.

source/binary/config drift·불완전 custody·noise로 비교 자격이 없으면 해당 판정은
BLOCKED/미실행 NOT_RUN이며, 실행된 제품/정확성/유실/상한 검사가 실패하면 FAILED다.
개별 command/validator의 실제 FAILED/timeout·prebuilt diagnostic 상태는 보존한다.
자격 입력이 무효여서 formal gate가 BLOCKED인 것과 개별 실행 결과를 합치지 않는다.
실패 시 해당 arm에서 원인과 exact hunk를 특정한 뒤 다음 비싼 실행을 결정한다.
새 고정본·환경·코드 수리 없이 같은 실패를 반복하지 않는다. 로컬 기능 결과와
quiet Linux/전체 자원/출시 qualification을 분리해 이 계획을 유한한 gate로 닫는다.

**10-31 실행 후 재개 조건:** Large의 실제 실패를 독립 세 gate로 분리한다.
후속 packet-local 입력 중복 제거와102/102는 완료됐지만 source091f19a1의
Large 재수용도 서버120s 기한에서 FAILED다. 따라서 아래 표의 재사용/비용 접점
전체가 해결됐다고 보지 않는다. 같은 조건의 추가 Large·XL·mixed 반복은 배정하지
않으며,실패한 Catalog/maintenance/초기-build 경계의 다음 exact 수리 후 재개한다.
현재 상태는 README E07,source별 실행은 VALIDATION의 기존 E07 절에만 갱신한다.

**코드 재감사에 따른 조사 범위 정정:** packet 간 확인 cache와 Storage의 batch 내
digest dedup은 이미 있다. 이를 새로 구현할 결손으로 배정하지 않는다.
Large 실패는 fixture가 선택한 maintenance20ms·1단계/round·2s·512KiB reserve가
켜진 조건이다. Native 운영 기본값의 일반 초기 구축 결과로 확대하지 않는다.
20ms 자체는60초 debt settlement의 필수 수용 조건이 아니지만, 실패 후 설정만
바꾼 결과로 기존 gate를 닫지 않는다. 이10-31 감사에서는 추가 exact hunk가
미선정이었다. 후속11-02의 count-only reader 선택과 실행은 아래 재개 절을 따른다.
캐시 포화 이후 재확인과 checkpoint 경쟁의 물리 비용은 계속 구분한다.
[10-31 근거와 재사용할 완료 구현](../../plans/oct-22-finalizing/VALIDATION.md#e07-code-reaudit-20261031).

| 기존 소유 경계 | 다음 exact 조사/수리 대상 | 원 계약·종료 조건 |
| --- | --- | --- |
| SEARCH/초기 lexical | packet-local 입력 제거·count-only reader·기존 cache를 재사용한다. 후속 진단 샘플은 `prepare_inventory`의 occurrence put_batch→기존 파일 read/설치 경로를 가리켰다. term별 positions SQL 페이지화는 현재 근거로 배정하지 않는다. 확인 창 밖 실제 재등장·새 파일/기존 파일 분리 없이 cache miss나 reserve 수를 물리 절감량으로 바꾸지 않는다 | 추가 SEARCH 단독 코드 결함은 미확정이다. 기존 attempt-local 확인/원 map·출력 identity·취소·메모리 admission을 유지하고 다른 PHYSICAL hunk를 선점하지 않는다. 실제 다음 수리가 SEARCH 접점도 바꿀 때만 독립 gold와 한 writer를 추가 |
| Catalog/PHYSICAL·기존 Native GC | 해당 두 배치 분기의 read→sync 원 FD 재사용과 private reader core는 후속 사용자 지시에 따라 이 채팅의 한 writer가 반영했다. 재구현하지 않고 현재 path/inode·namespace·FD/capacity·retirement·환급 계약의 남은 독립 회귀와 owning 증거를 합류한다. Catalog reserve/confirm·잠금/checkpoint와 신규/재확인 비용은 [source별 Record](../../plans/oct-22-finalizing/VALIDATION.md#e07-leaf-attribution-20261102)로 분리한다 | 새 writer를 만들지 않는다. 기존 owning gold에 one-open·path 교체·실패 후 미confirm·실제 lease 반환을 보완하고 원래 read/full-wire·per-file/directory sync/full barrier를 유지한다. 후보 절감량은 아직 미입증이다. GC 재예약은 재구현하지 않고 GC 정지·sync 제거·cap/60초 settlement 변경으로 통과시키지 않음 |
| 공용 Native Root | `native_server_admission`120s→IPC 새 요청→Submit WorkBudget와 harness600s client I/O의 분리 구현은 확인됐다. 이는 deadline 전달 누락으로 확정된 코드 결손이 아니다. `managed_install.rs:464`는 만료 감지 charge 위치다 | E07이 parent/API를 선점하거나120s를 임의600s로 늘리지 않는다. 원4K30s/1M/512MiB는 그대로다. 기한 계약 변경이 필요하면 별도 사전 결정과 새 profile로 남기며 성능 수리와 구분 |
| E07 Root | 코드 일치 심볼 원본과 실제 process/phase에 묶인 successor 샘플·canonical 실패 trace·수리된 child drain의 실제 검증을 재사용한다. old-product matcher 실패와 새 진단 cohort를 구분하고 NM 함수 후보/짧은 관측을 전체 CPU·기존 바이너리 개선율로 승격하지 않는다 | 아래 잔여 A의 독립 회귀·B의 owning/strict 수용→source/binary 결속 Large1회→성공 시 XL→원mixed. 동일 실패 재실행이나 별도 원4K/물리 구현 큐는 없다. formal quiet/RSS/all-root 권위는 별도 BLOCKED |

이는 새 네 티켓/별도 owner 배정이 아니다. 기존 함수·수정권과 실제 실행 결과를
연결한 다음 작업 순서다. 부족한 공급 비용축과 E08 전체 통합은 기존 담당 범위로
유지한다. 현재 trace의 상위 span·동기화·Catalog 합계는 중첩이 있어 더해 critical
path나 절감률을 만들지 않는다. 추가 제품 hunk는 이 근거로 특정한 뒤 합류한다.

**11-02 코드 우선 재개:** 위 표의 조사에서 SEARCH의 다음 hunk를 특정했다.
`validated_occurrence_count`가 전체 위치 배열을 materialize한 뒤 길이만 반환한다.
기존 object/JSON preflight를 공유하는 count-only reader와 그 caller만 수리하며,
캐시 정책·물리 installer·GC cadence·120s 기한은 그대로다. Kernel attempt DATA
cache도 이미 있으므로 first64 miss를 물리 read로 계산하지 않는다. 병렬 tool hunk는
기존 canonical capture reader의 `closed-maintenance` projection이며 새 수집기가 아니다.
정적 합류 뒤 Text/occurrence·기존 Kernel3개를 한 nextest 선택으로 검증하고,
strict와 기존 Python owning batch를 수용한다. 이어서 수리 source/binary에 묶인
로컬 Large1회로 기존 실패를 판정한다. 실패하면 XL/mixed는 진행하지 않고 실제
원인을 유지한다. [진행·실행 상태](../../plans/oct-22-finalizing/VALIDATION.md#e07-count-reader-20261102)는 README E07에 합류한다.

실제 합류 결과는 Python50/50·Rust110/110·strict·로컬 제품 빌드 VERIFIED다.
동일5f604bcb/2881 Large Off1회는159.202s/producer1로 FAILED이며 원인은
BuildPrepare의 서버 deadline이다. 수치 cap·외부 watchdog 초과가 아니다.
같은 바이너리의 bounded Record1회도 deadline으로 실패했지만 schema5
581714events/loss0의 완전한 실패 trace를 얻었다. 실패한 full build는 pre-Postings
inventory에 머물렀고,lexical CPU·Catalog COMMIT/대기와 maintenance를 분리했다.
후속 진단은 이 inventory의 occurrence 설치→기존 파일 검증/재open 접점을
가리켰다. [함수 RCA와 기존 PHYSICAL 인계](../../plans/oct-22-finalizing/VALIDATION.md#e07-leaf-attribution-20261102)를
소비하며,근거 없는 positions SQL/cache 수리는 배정하지 않는다.
그 실행에서 확인한 child 검증기의 단일 operation-root 가정은 E07 경계에서
수리·검증을 마쳤다. 독립 Catalog 설정 root를 허용하면서 IO summary를 한 root에
묶는 gold와 strict가 통과했다. 이를 기존 deadline 해결이나 제품 green으로
해석하지 않는다. 후속 원 FD 재사용 코드도 반영됐으며,다음은 아래 잔여 A의
회귀 공백 보완과 B의 owning proof다.
기존 Record의 checkpoint 결과 분석은 과거 source 범위를 유지하며 현재 source의
병목 시간으로 승격하지 않는다.
Catalog/GC·Native parent의 새 hunk가 확인되면 기존 owner가 처리한다. 계측을 추가한
실행으로 Off 수용이나 개선율을 만들지 않으며,XL/mixed는 Large 해결 뒤다.


<a id="e07-residual-packets-20261102"></a>
### E07 코드 재대조 후 잔여 패킷 — 2026-11-02

현재 코드/작성된 회귀와 실행 증거는 [최신 실행](../../plans/oct-22-finalizing/VALIDATION.md#e07-read-owner-proof-20261102)으로
구분한다. 이 절은 추가 구현/검증의 계약이며 새 상태 원장이 아니다. 이미 반영된
read-owner 생산 코드와 회귀6개는 재구현하지 않는다. A 소스 보완과 B의 최종
Storage166·Native3·Storage strict는 완료됐다. C의 고정 소스 Large는 deadline으로 실패했고
현재 바이너리 Record RCA에서 초기 occurrence 설치의 반복 확인 비용을 확인했다.
XL/mixed는 선행 실패로 NOT_RUN이다. 아래 D는 같은 기존 SEARCH 경계에서 합류됐고
Native200·lexical strict·기존 Kernel lifecycle5가 통과했다. 별도 ignored4K engine 검증은 NOT_RUN이다.
통계 fixture를 포함한 최종201과 strict도 통과했다. source b97d8191의 Large Off155.925s·
Record154.773s는 모두 deadline FAILED다. inventory 내부 원인은 아직 분리되지 않아
추가 count memo 대신 아래 E의 bounded phase 계측을 합류했다. 최종265/265·
Python451·owning strict와 e6fa916f 정상 빌드는 완료됐으며 LP3 canonical Record도
COMPLETE다. Large는 다시 deadline FAILED여서 C를 닫지 않는다. 아래 E는 완료
구현/진단으로 재사용하고,후속 잔여는 기존 C의 Storage/Catalog/VFS owner 수리다.

| 패킷 | 유일 writer·수정 경계 | 실제 잔여·독립 완료 조건 | 선행/합류 |
| --- | --- | --- | --- |
| A — 회귀 공백2곳 | 기존 test writer1,`crates/quanta-index-storage/src/object/retained_read_tests.rs`만 | 기존 two-identity 테스트의 barrier에서 첫 owner의 원 dev/ino·live1,둘째 owner의 live0·각 open1을 확인. 별도 실제 batch loser 회귀는 test-only `ObjectWriteIoPort::begin`→ticket `created`에서 받은 canonical target에 matching wire를 O_EXCL/0600 게시해 초기 exists=false 뒤 실제 publish(false)를 강제한다. 원래 winner inode의 read1·같은 FD barrier·loser temp/FD cancel1·winner 보존·confirm1·종료 후 lease0을 확인. barrier 실패 변형은 confirm0/FD·lease0. 새 제품 seam/fault flag나 helper 직접 호출로 분기를 건너뛰지 않는다 | 현재 생산 코드/기존 테스트5개 재사용. 두 보완은 같은 파일이므로 한 writer가 연속 수행. 단위 fake-port 증거를 실제 Common capacity 제품 수용으로 승격하지 않음 |
| B — owning·strict 합류 | Root 실행1큐. 컴파일/lint/행동 실패가 드러난 경우 위3파일 exact hunk만 수리 | Storage library+`native_object_namespace`와 기존 실제 managed-store/IO 회귀를 한 영향 묶음으로 실행. 기존 `admitted_managed_store_installs_scalar_batch_and_replay_with_real_catalog`·`retired_visible_create_recovery_releases_reader_and_settles_once`·`no_replace_loser_cancels_only_its_temp_and_keeps_charged_winner`를 재사용한다. nonzero bodies·원 오류/예산·close/refund 결과와 storage all-target strict를 분리 기록 | A 소스 합류 후. 선언/정적 parse를 테스트 성공으로 기록하지 않음. 같은 package/selector를 다른 채팅·다른 폴더로 중복 실행하지 않음 |
| C — 정책 수리→제품·운영 수용 | 기존 E07 Root 합류·제품1큐와 아래 C1–C3의 disjoint 내부 writer | LP3 source e6fa916f/capture93f7321a의 [확정 RCA](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-reclaim-policy-rca-20261102)를 입력으로 WAL threshold/cooldown·typed defer·GC 우선 진행을 수리한다. 실제 수정·검증·실패 후 분기는 [C0–C7 실행계획](E07-WAVES.md#e07-wal-policy-execution-20261103)만 따른다. A/B/D/E와 기존 indexing/FD/VFS 구현은 재사용한다 | 계약 먼저→C1∥C2∥C3→Root C4→영향 검증 C5→Large C6→남은 운영 C7. quiet paired overhead·전체 수명 RSS/all-root allocated authority 부재는 BLOCKED로 유지. 새 티켓/SSOT/제품 실행 큐 없음 |
| D — 확인 완료 occurrence의 attempt 내 재사용 | 생산 writer1:`text.rs`의 기존 hot-cache/OccurrenceWrites.flush/prepare_inventory 접점과 `text_spool.rs`의 private set. 테스트 writer1:새 독립 child2개만. Root 등록/합류1회 | 기존 admitted TextSpool에 exact ObjectRef 확인 집합을 추가한다. positive max64는 resident hot windows,0은 전체 uncached로 명시한다. batch<=64 scratch 쓰기 공간을 원 put 전에 예약하고,실제 put 성공과 전체 ordinal/identity 검증 뒤에만 집합에 표시한다. 원 AttemptRef·GC 보호·첫 설치의 reserve/install/sync/confirm을 유지한다. 다른 attempt·pending/실패 출력·prior leaf는 hit가 아니다. 전체 RAM 집합·새 DB/serializer·상한 확대·실패 fallback을 만들지 않는다 | C의 c0e11cce 실제 실패/Record 뒤 선택. 독립 >64 재등장·0 경로·정확한 결과/실패/취소/공간·fence 회귀→lexical owning/strict와 기존 lifecycle law→새 source C. 첫64 count-read cache와 Kernel/Storage dedup은 재구현하지 않음 |

| E — inventory 내부 비용의 실제 귀속 | Core stage/CPU·searchd wire/recorder prefix writer1,Python closed schema/CPU·gold writer1,lexical coarse producer/trace gold writer1. Root가 계약/검증 합류 | 고정 stage3개 `NativeTextInventoryPrepare`/`NativeTextInventoryMapApply`/`NativeTextInventoryDiff`와 wire `native_text_inventory_prepare`/`native_text_inventory_map_apply`/`native_text_inventory_diff`. owner 준비·최대64-owner map 적용·owner diff의 동기 span만 추가한다. 기존 observe_attempt·원 WorkBudget/실패/lease를 유지한다. per-token event·새 DTO·cache/상한 변경은 없다. 실제 cap 실패에서 확인한 lost-begin 뒤 short-end 출력은 첫 미기록 이후 prefix latch로 수리하며 drain/health/손실·원 cap을 보존한다. 기존 wire ID는 보존하고 calling-thread CPU는 새 동기 stage만 whitelist한다 | D source 실제 실패 뒤 선정. 계측·cap 수리의 최종 영향265/265·Python451·lexical/searchd all-target strict·Core library strict는 통과했다. 기존 Core test strict4건은 별도 실패이며 새 source e6fa916f 빌드 및 LP3 canonical COMPLETE도 완료됐다. LP3 Large deadline FAILED는 그대로이며 C의 기존 owner 인계로 이어진다. closed stage/CPU·부모/실패·Off 동등 oracle의 집중 검증과 strict를 Root1큐로 모은 뒤 새 source Record1회. 상위 lexical span과 child 시간은 중첩이며 합산하지 않는다. 계측만으로 동일 Off를 반복하지 않음 |

B의 실행 계약 명령은 다음이며 실제 terminal·범위는 위 VALIDATION에 기록한다. 기존 warm target 사용과
resource admission은 Root가 한 곳에서 설정한다. 테스트 병렬성은 repo 기본을
유지하며 전역 jobs1/test-threads1을 추가하지 않는다.

```sh
./scripts/cargow --lane bench-lane nextest run --cargo-profile bench --locked --offline --all-features -p quanta-index-storage --lib --test native_object_namespace
./scripts/cargow --lane bench-lane nextest run --cargo-profile bench --locked --offline --all-features -p quanta-index-searchd-runtime --lib -E 'test(admitted_managed_store_installs_scalar_batch_and_replay_with_real_catalog) | test(retired_visible_create_recovery_releases_reader_and_settles_once) | test(no_replace_loser_cancels_only_its_temp_and_keeps_charged_winner)'
./scripts/cargow --lane bench-lane clippy --profile bench --locked --offline --all-features -p quanta-index-storage --all-targets -- -D warnings
```

원래 병렬 경계는 A의 테스트 소스 보완과 Root의 문서/입력 대조였다. D는 생산 private
접점1writer·독립 test child1writer·read-only 적대검토와 Root 문서/입력 대조를 병렬로
진행한다. 공용 parent 등록은 Root만 한다. 완료된 생산 코드·
회귀를 새 writer에게 다시 나누거나 A의 같은 파일을 두 writer가 동시에 고치지
않는다. B/C는 같은 Root 큐에서 이어 실행한다. Catalog/GC cadence·producer·
DELTA·SDK·E01/E02·E08·출시 작업은 기존 배정에서 제외하지 않으며 이 채팅에
중복 발주하지 않는다. 다른 채팅의 실시간 활성 상태는 이 repo-only 감사에서
조회하지 않았으므로 해당 담당이 현재 실행 중이라는 주장도 하지 않는다.

위 넓은 library 실행에서 드러난 `maintenance_stats.rs`의 기존 test1은 Root가
unsealed source-event의 성공 기대를 원 InvalidContract 거절로 교정한다. 생산
계약과 통계 불변 oracle는 유지하며,Native200과 이1개를 같은 최종 영향 선택에
넣는다. File 대형 compaction suite는 이 Native 변경의 재실행 대상으로 삼지 않는다.

<a id="e07-wal-policy-execution-20261103"></a>
### E07 C — 남은 코드부터 닫는 실행계획 (2026-11-03 KST)

이 절은 기존 C의 실행 계약이다. 현재 판정은 [README E07](../../plans/oct-22-finalizing/README.md#e07),
근거는 [LP3 RCA](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-reclaim-policy-rca-20261102), 독점 파일은
[C writer map](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-wal-policy-writers-20261103)이 소유한다.
A/B FD 재사용·D 확인 참조 spill·E 계측/recorder는 완료한 source별 결과를 소비한다.
11-03 C1/C2/C3와 Root config/pump 연결 소스는 반영했다. 실행 판정은 README/VALIDATION의 최신 owning 결과를 따른다.

**수리할 실제 결함은 세 접점이다.** (1) GC reserve512KiB를 WAL 물리 크기
임계값으로 재사용하고 GC round마다 TRUNCATE하는 정책 결합, (2) 선택적 정리가
GC보다 먼저 예산을 쓰거나 busy로 GC를 건너뛰는 순서, (3) 제품 Catalog의
busy_timeout2s가 round_timeout2s와 같아 긴 SQLite reader가 pump를 점유하는 경로다.
LP3의701회와 foreground auto49회·잠금 겹침은 원인 근거이며120s 통과 보장은 아니다.

#### C0 — Root가 한 번 고정할 계약

- 기존 `MaintenanceConfig`→`NativeReclaimConfig`에 Runtime-local
  `NativeWalReclaimPolicy { trigger_bytes, min_interval }` 하나를 연결한다.
  JSON의 maintenance 안에 명시 `wal_reclaim { trigger_bytes, min_interval_ms }`를
  두고 실제 constructor/config fixture를 함께 갱신한다. GC reserve를 implicit
  fallback으로 사용하지 않는다. maintenance 생략=자동 유지보수 off 의미는 보존한다.
  자동 WAL 상태는 기존 pump 스레드가 단조시계로 소유하고 첫 due는 시작+min_interval이다.
  due 전에는 WAL 검사를 위한 stat/Catalog lock/SQL을 실행하지 않고, 실제 선택적
  시도 직전에 다음 due를 예약한다. busy/미funded도
  cooldown을 적용한다. stop/cancel 뒤 새 시도는 없다.
- Catalog의 기존 `checkpoint_wal_for_reclaim` 결과를
  `Result<WalReclaimOutcome, IndexError>`로 직접 교체한다. 값은
  `BelowThreshold`(WAL 없음 포함), `DeferredUnfunded`, `DeferredBusy`, `Completed`다.
  bool wrapper/두 번째 checkpoint API는 만들지 않는다. 첫 reads1과 복합 charge의
  미funded 모두 cancel/deadline을 재검사한 뒤 defer한다. 실제 VFS 용량 거절은
  `ResourceLimited`, 취소/deadline/storage는 원 오류다. SQLite busy/locked 및
  PRAGMA busy 행만 `DeferredBusy`이며 native IO 거절과 혼동하지 않는다.
- **자동 경로는 GC-first다.** 현재 round의 recovery/GC를 먼저 시도한다. 성공 또는
  ResourceLimited 뒤 원 요청이 유효하고 WAL due일 때만 같은 MaintenanceControl·물리
  scope·남은 원 WorkBudget으로 선택적 TRUNCATE를 한 번 실행한다. 후자는 WAL 점유로
  다음 GC가 계속 거절되는 것을 완화할 조건부 복구이며 현재 GC 실패는 유지한다.
  GC의 cancel/storage 오류 뒤에는 새 WAL 작업을 하지 않는다. GC 실패를 새 예산으로 재시도하거나 WAL 성공으로
  덮지 않는다. 실제 부족/취소에서 GC 성공을 보장하지 않는다. `prepare_discard`와
  명시적 `reclaim_step`의 사전 공간 회복은 보존하며 busy/미funded 결과를 명시 소비한다.
  기존 `finish_reclaim_round`는 원 ResourceLimited를 round 진단에 남긴 뒤 pump를
  계속해 다음 주기에 재시도한다. 이 wrapper를 삭제하거나 모든 용량 거절을 pump
  fatal로 만들지 않는다. Catalog의 실제 거절을 optional defer로 재분류하지 않고
  durable debt/원 cursor와 실패 진단을 보존하는 것이 경계다.
- 선택적 TRUNCATE만 Catalog mutex 아래 비대기 busy 처리한다. 원 connection의
  busy timeout을 보존하고 일시적으로0을 적용한 뒤 성공/실패 모두 checked restore한다.
  restore 실패는 오류이며 성공 결과로 숨기지 않는다. 일반 Catalog writer의2s 정책과
  SQLite auto1000/FULL/fullfsync는 유지한다. 이는 busy 대기 제거이며 진행 중인
  backfill/fsync 선점이나 모든 syscall의2s 상한을 보장하지 않는다.
- threshold는 물리 WAL high-water이고 미checkpoint debt나 장치 여유량이 아니다.
  실제 Catalog lock 안에서 stat·원 max_io/4·atomic WorkBudget·VFS funding을 검사한다.
  config 검증은 `0 < trigger_bytes <= control_need.io_bytes/4`와 양수/overflow 없는
  min_interval을 요구한다. allowance/논리 사용량을 실제 disk pressure로 사용하지 않는다.
  NativeResourcePlan IO는 동시 admission 추정치이며 별도 소비량 원장으로 합산하지 않는다.

#### C1–C4 — 코드 병렬 패킷과 합류

| 패킷 | 단일 산출물 | 독립 oracle / 합류 조건 |
| --- | --- | --- |
| C0 / Root | current HEAD·dirty 함수·기존 owner와 위 signature/config/호출 순서 확정 | 같은 semantic hunk가 바뀌었으면 기존 변경을 먼저 합성한다. 다른 채팅 실시간 상태는 추정하지 않는다 |
| C1 / 정책 writer | Runtime-local 순수 policy/state 모듈. due·cooldown·checked Instant 연산만; IO/SQLite/Core 새 타입 없음 | fake clock의 최초 due·직전/정각 경계·긴 round·busy/미funded 후 재시도·stop. 같은 상태 수명에서 시도 수≤`1+floor(elapsed/min_interval)`. reserve→confirm 사이 절대 checkpoint 없음 같은 불가능한 보장은 요구하지 않음 |
| C2 / Catalog writer | 기존 checkpoint 함수·정확한 결과 분류·비대기 busy scope/복구와 기존 Catalog oracle 보완 | rows7/11·old reader sum7/current18·reopen·미funded checkpoint IO0 보존. 실제2s connection 설정에서도 긴 reader가 busy 대기를 하지 않고 원 timeout이 복구됨. Catalog 분류/cancel/deadline oracle를 담당하며 실제 VFS ENOSPC/short-write/fsync는 Root C4/C5 통합에 귀속 |
| C3 / fixture writer | 기존 physical_mixed_pressure의 독립 child에서 실제 reader·GC·작업 예산을 검사. 제품 pin/공간 압박·복구는 기존 gold 재사용 | fixture/oracle를 먼저 작성하고 C0의 실제 API가 합류한 뒤 테스트 body 연결. helper로 GC 결과를 계산하거나 가상 API/새 product fault seam을 만들지 않음 |
| C4 / Root integrator | parent export·config와 모든 literal·pump 상태/GC-first·manual/discard caller·harness JSON을 한 번 연결 | C1/C2 반환 후만 공용 caller를 수정. maintenance off·head/cursor/recovery·stop/join·renewal을 유지. 성공 GC 뒤 reads 소진은 WAL `DeferredUnfunded`; 실제 GC 실패는 그대로. near-capacity에서 허위 성공/유실 없이 명시 복구 경로가 유지됨 |

실행 DAG는 **C0 → C1∥C2∥C3 → C4 → C5 → C6 → C7**다. Root+내부3슬롯에서
소스를 먼저 끝내고 worker는 Root에게만 변경/정적 결과/미해결 접점을 반환한다.
worker별 Rust/제품 실행·peer 대화·다른 채팅 조회/메시지·새 채팅/티켓은 없다.
공용 parent와 source/test 합류·heavy 실행은 Root 한 큐다. 전역 jobs1은 추가하지 않는다.

#### C5 — 한 영향 배치, 필요한 짧은 정책 비교

1. 변경 파일 format/whitespace·actual caller inventory·module boundary를 먼저 확인한다.
   Catalog의 기존 checkpoint3회귀, C1 순수 정책, Runtime maintenance/renewal·native pump,
   영향받은 VFS checkpoint/fault·manual/discard 회귀를 선택해 canonical `./scripts/cargow`
   한 큐로 실행한다. 변경 Catalog/Runtime/harness의 strict를 함께 확인한다. 기존 Core
   test lint4건은 기존 owner의 별도 실패이며 이 수리의 성공/실패 분모에 섞지 않는다.
2. reader가 살아 있어도 **funded GC는 먼저 진전**하고 optional busy가2s를 기다리지
   않아야 한다. 마지막 pin 해제 뒤 원60s debt0·shutdown5s·원 publication/old-view·
   fail-closed/refund gold를 보존한다. 진짜 ENOSPC에서는 즉시 GC 성공을 요구하지 않고
   durable debt 보존→원 복구 후 재시도를 확인한다. GC-first가 사전 WAL headroom을
   회복하지 않는 tradeoff를 실제 near-capacity 회귀로 검사한다.
3. 첫 비교 후보는 **trigger8MiB/min_interval1s**다. 4KiB page의 auto1000-frame
   약4,120,032B(3.93MiB)보다 높고 현재 IO64MiB/4=16MiB 이하다. 같은 binary·고정 짧은 Catalog workload에서
   busy reader/해제·threshold 교차·WAL 재사용을 확인한다. 필요할 때만8MiB/2s를
   두 번째 후보로 사용한다. 후보는 운영 보장이 아니며≤2개 후 한 값을 고정한다.
   시도 수·Catalog wait·WAL 물리 peak·GC 진전·총 비용을 함께 비교하고 전체 Large
   파라미터 grid는 돌리지 않는다. auto off 또는 hard WAL growth cap은 이 수리에
   포함하지 않는다. 기존 oversized WAL의 defer와 auto checkpoint를 보존한다.

#### C6 — 제품 재실행과 실패 시 다음 수리

- 영향 배치가 통과한 source/Cargo/config를 고정하고 normal bound bench/daemon을
  한 번 빌드한다. Large42 **Off1회**로 전체 ACK/search/reopen·원 cap을 판정한다.
  helper 전체 wall을 dispatch120s에서 빼지 않는다. source/ACK phase와 거절 권위를
  따로 기록한다. prebuilt build-binding UNPROVEN을 qualified build로 바꾸지 않는다.
- Large 실패 시 같은 변경 source의 **Record1회**로 남은 비용을 분리한다. 시도 수가
  여전히 폭주하면 config/실제 caller의 우회부터 수리한다. 시도 경합이 줄었는데도
  실패하면 남은 Prepare/Diff CPU·Storage apply_stream scratch/root/install·foreground
  auto checkpoint를 같은 critical path로 다시 귀속하고 확인된 한 hunk만 수리한다.
  unchanged Off 반복·cache/count memo 확대·모든 ResourceLimited 무시는 해법이 아니다.
- Large 성공 뒤에만 XL42→원60+900s mixed를 실행한다. 원 ANN4K의30s/1Mreads/512MiB와
  Large의4096파일은 다른 gate다. server120s/client600s·maintenance20ms/1step/2s·
  reserve512KiB·원 work/memory/custody/FULL 계약을 완화하지 않는다. 기존 원4K proof는
  재사용하되 변경이 직접 닿은 maintenance/trace 수명만 필요한 범위로 재검증한다.

**C6 실제 후속 — Native observer 선택:** C1 정책 이후 Large Off/Record는
BuildPrepare120s에서 FAILED다. 별도 Native sampler가 Off에도 주기적 RSS100ms/
disk500ms 관측을 수행했다. 현재 C6의 추가 코드 hunk는 기존 Python capture CLI에
`--native-resource-sampling off|periodic`를 명시하고 기존 Disabled/Enabled Rust
observer 및 nullable resource validator를 그대로 소비하는 것이다. 기본 periodic과
mixed의 periodic 필수는 유지한다. off는 별도 관측 없는 기능/오버헤드 진단 입력이며
resource peak/운영 수용은 NOT_RUN/BLOCKED다. 같은 Rust/Cargo closure·바이너리·
입력/원 cap에서 sampler만 끈 비교로 실제 observer 기여를 확인한다. 주기 관측의
누적 wall을 요청 지연에서 빼거나 이 설정만으로120s 성공을 예측하지 않는다.
현재 실제 결과는 README E07/VALIDATION을 따르며 새 티켓/collector는 없다.

**C6 현재 수리 후보 — 보호 중인 객체의 GC 큐 churn:** sampler-off Record
C1UR1은 FAILED이며, maintenance ancestry 아래 WAL sync3,818회/24.360s가
관측됐다. managed COMMIT lane3과 별도이며 parent-sync21.778s는 중첩이다.
현재 reserve/confirm은 같은 transaction의 intent/attempt custody로 보호한 객체를
GC 큐에도 넣고, GC는 reference가 있으면 dequeue와 FULL commit을 수행한다.
이 분기의 실제 실행 횟수는 trace에 없으므로3,818회를 모두 해당 분기로 단정하지 않는다.
다음 exact 수리는 managed Attempt/Ingress의 초기 queue 등록을 보호 해제 시점으로
옮길 수 있는지 확인하는 것이다. 모든 abort/terminal/ingress transfer/recovery의
최종 보호 해제가 같은 transaction에서 기존 release/enqueue를 소비해야 한다.
그 증명이 없는 경로는 변경하지 않는다. lifecycle·기존 queue digest 검증·rollback·
reclaiming conflict·원 work 청구는 보존한다. Bootstrap·공개 GC API와20ms cadence,
reserve512KiB·FULL·auto1000은 그대로 둔다. 독립 oracle은 보호 중 queue 부재,
reserve-only 및 confirm 후 crash/reopen, 마지막 custody 해제 후 queue와 실제 회수,
공유 child 보존, malformed queue/late failure의 무변경이다. Catalog 코드·독립
회귀는 disjoint writer로 나누고 Root가 영향 검증과 변경 source Large를 한 번 합류한다.
source audit만으로 구현 가능/성능 수용을 선언하지 않는다.

추가 합류는 ingress admission의 보호 이전이다. 원 transfer를 유지하고 ingress
정리 시 같은 transaction의 operation custody와 digest를 확인한 객체만 기존
adopt로 queue를 제거한다. 확인되지 않은 intent는 release/enqueue를 유지한다.
확정 ingress 객체가 항상 intent를 가진다는 가정은 사용하지 않는다. 새 조회와
최악 normalization을 추가 선청구하며 원 cap을 바꾸지 않는다. 다른 보호 해제는
여전히 객체를 재등록할 수 있어 전역 protected queue=0을 완료 조건으로 삼지 않는다.

**C6 종료한 비교 — 기존 writer cache16MiB:** C2UR1에서 maintenance WAL writes0/
sync16·lexical Catalog wait0.023s를 확인해 GC 추가 패킷은 발급하지 않는다.
남은 전경 Catalog miss594,510과 COMMIT38.807s를 근거로 기존 harness의 명시
`catalog_writer_cache_kib`만8192→16384로 비교한다. 새 cache/knob/ledger는 없다.
Native 기존 admission에서384MiB capacity·320MiB execution floor를 유지하고
16MiB connection lease를 먼저 예약한다. 잔여48MiB이며 RSS cap의 증거는 아니다.
일반/mixed config digest가 모두 바뀌는 cache variant로 구분한다. 원 request/
work/IO/cadence/FULL/auto1000을 유지한다. 동일 입력의 완료된 첫5 MapApply prefix의
miss·CPU/commit 관측과 Large 결과로 판단하며, 단순히 실행 시간이 달랐다는
이유로 개선을 선언하지 않는다. C3 Off/Record도 원120s에서 FAILED이며 완료된 첫5
MapApply의 miss는483666→468958(약3% 감소),각14 COMMIT과 WAL 쓰기는 거의 같다.
host epoch와 진행량이 달라 wall/CPU 차이를 cache 효과로 판정하지 않는다.
효과가 입증되지 않아 이 한 hunk를8192로 복원했다. GC 구조 수리는 유지한다.
제품/RSS·overhead/XL·mixed 수용은 별도이며 field 변경을 구현 완료로 세지 않는다.

남은 전경 비용은 기존 trace의 managed input 계수와 closed phase를 먼저 대조한다.
종료 직전5s wall stack은 Prepare의 occurrence install→reserve/confirm→COMMIT 경로
후보를 보여주며 MapApply 전체나 CPU 비율의 증거가 아니다. reserve와 confirm의
두 durable 경계,FULL/auto1000,각 native write의 root/allowance/retarget 검증은 유지한다.
추가 cache 확대·autocheckpoint 해제·배치 상한 상향·동기화 생략을 수리로 발급하지 않는다.

**C6 확정 코드 — 같은 window의 sparse occurrence packet 합류:** 기존
ManagedObjectInputs의 성공 reserve/confirm 쌍을 canonical trace에서 대조했다.
C2 Prepare368쌍/5299입력(평균14.4),64미만364개;C3는559쌍/7274입력(평균13.0),
64미만555개다. 두 실행의 첫5 MapApply는1789/1791/1760/1716/1728입력으로 같고
이미 대부분256개인 내부 배치다. 이 수리는 Prepare의 파일별 residual만 대상으로 한다.

- 기존 InventoryBatch가 OccurrenceWrites 하나를 소유한다. token table reset과
  독립인 owned key/payload/MemoryLease를64행 안에서 보유하고,64행 또는 window
  경계에서만 기존 put_batch를 호출한다. MapApply256·공개 batch64·메모리/work cap은
  바꾸지 않는다. reserve→물리 write/sync→confirm의 두 durable 경계는 그대로다.
- pending 행마다 ordinal을 보존하고,설치 전에 해당 ordinal의 edit placeholder를
  전부 선승인한다. 전체 반환 cardinality/identity를 검사한 뒤에만 confirmed spill/
  cache와 각 ordinal의 edit를 채운다. map cursor를 읽기 전에 residual을 확정한다.
  changed=false의 base pop은 해당 파일의 pending이 없을 때만 안전하며 독립 회귀로
  검사한다. 다른 attempt/취소/누락 ordinal·잘못된 출력은 거절하고 attempt를 폐기한다.
- window residual도 기존 NativeTextInventoryPrepare span 안에서 관측한다. 이 stage의
  span 수는 파일 수가 아니며 파일별 token preparation과 residual 확정 구간을 포함한다.
  이전/이후 비교는 성공한 동일 input prefix·ManagedObjectInputs·commit 수를 사용한다.
- 소스/독립 회귀를 먼저 합류한 뒤 lexical owning+strict를 한 큐로 검증한다. 소스가
  바뀌었으므로 normal bound pair를 새로 빌드하고 Large Off1회,실패 시 같은 pair의
  Record1회만 실행한다. 감소시킬 실제 packet 수와 기능 gate 모두 확인하며 성능
  성공이나120s 통과를 미리 선언하지 않는다. XL/mixed는 Large 성공 뒤 원 순서다.

이 패킷은 영향 회귀·strict·구조 검증과 실제 효과까지 확인했다. C4 Large
Off174.444s/Record173.077s는 원120s에서 FAILED다. 동일 첫5 MapApply까지
Prepare 입력4779개를 유지하며 reserve/confirm 쌍324→131(59.6% 감소)을 확인했다.
현재 남은 C6은 MapApply47.031s/CPU35.077s와 Diff31.165s/CPU22.765s의 원 생산 경로다.
실패/서로 다른 진행량의 전체 합으로 성능을 비교하거나 완료율을 외삽하지 않는다.
정적 조사상 bulk empty-root build·완전 CAS/cursor 소비·최종 reachable DAG만
중복 없이 설치하는 경로는 이미 있다. 이를 새로 구현하지 않는다. 다음 읽기 감사는
`text.rs::diff_inventory/MapEntries/validated_new_count`,
`text/entry_patch.rs::apply`,기존 `storage/map.rs::apply_stream/install_reachable_objects`
사이의 실제 반복 read/metadata·작업 단위를 분리한다.64개 resident count와 기존
confirmed spill의 경계는 후보일 뿐,기여도가 미측정인 새 count memo/codec·format
교체를 구현 패킷으로 승격하지 않는다. 새 코드가 확정되기 전 동일 Large 반복0이다.

**C6 현재 측정 코드 — Diff의 누락된 하위 귀속:** C4의 기존 SQL lane을
재투영하면 Diff의 lane22합0.438s이고 write admission0.178s라31.165s를 SQL
쓰기로 설명하지 못한다. MapApply는 COMMIT21.025s/전체47.031s,실제 metadata
preflight/mutation합 약4.43s다. 합산은 각 범위 안에서만 하며 부모와 중첩해 더하지 않는다.
새 알고리즘을 추측하는 대신 기존 단일 observe helper를 stage 매개변수로 일반화한다.
추가하는3stage는 `NativeTextInventoryRead`(lane30 cursor next),
`NativeTextOccurrenceRead`(31 object port get+전체 count 검증;resident memo hit 제외),
`NativeTextPostingEdit`(32 key/row admission,33 bounded SQL flush)다. 같은 원
WorkBudget·attempt·원 오류를 전달하고 Off는 clock/event 없이 원 closure를 호출한다.
CPU는 부모 Diff/Build에 포함되며 하위 합계를 부모에 더하지 않는다. 원64행 flush는
Diff 아래,최종 residual flush는 Diff가 닫힌 뒤 Build 아래인 현재 순서를 유지한다.
Core stage→실제 lexical port→trace wire→schema/canonical CPU 소비와 독립 parity
fixture를 함께 연결하며 새 collector/cache/schema version·제품/storage format은 없다.
OccurrenceRead는 port 내부 DATA cache를 포함할 수 있어 span 수를 FD/device read 수나
고유 reference 수로 해석하지 않는다. 물리 I/O는 기존 별도 object/VFS counter로 판정한다.

이 변경은 C4 실패를 진단하기 위한 observer derivative다. 영향/Off fast-path·wire/
canonical 검증과 normal pair build 후 Record1회만 사용한다. C5 Off·새 제품 수용은
NOT_RUN으로 남기며 기존 C4 Off를 새 binary의 수용으로 승격하지 않는다. 실패한
제품 의미가 같은 Off를 반복해 개선을 만드는 큐는 없다. 실제 알고리즘 수리가 선정되면
원 C6의 변경 source Off→실패 시 같은 pair Record→성공 시XL/mixed 순서로 돌아간다.

정적 감사에서 추가로 확인한 caller 후보는 canonical `scoped_reads`의 연결 범위다.
Kernel query에는 연결됐지만 update의 NativeText prepare에는 원 map port가 전달된다.
새 cache를 만드는 후보가 아니라 기존512slot/1MiB DATA window를 같은 funded
LexicalBuild 내부에서만 빌릴 후보이며 C5의 InventoryRead 비용 확인 전 채택하지 않는다.
빈 bootstrap에 불필요한 box grant를 만들거나 Kernel 전체 prepare에 걸쳐 다른 stage/
retry/메모리 lender로 DATA를 넘기면 안 된다. hit는 원 logical reads/bytes·decode·취소를
청구하지만 물리 namespace/path를 매번 재인증하지는 않는다. 같은 attempt/pin의
immutable DATA 수명 계약과 별도 live authority 확인을 전제로 하며 새 보장을 만들지 않는다.
OccurrenceRead 우세라면 이 caller 연결로 해결되지 않는다. 이미 있는 bulk builder,
resident count·spill,retirement scan hint나 FD 재사용을 다시 구현하지 않는다.

**C6 C5 귀속 후 선정한 두 코드 패킷:** observer derivative C5R1은 원120s에서
실패했지만 canonical1580035events COMPLETE,loss/unwritten/open/lease/health/orphan0이다.
같은512closed OK Diff29.6486s/CPU22.2907s 중 InventoryRead144394회19.4642s/
CPU15.3675s,OccurrenceRead112046회8.81925s/CPU5.97774s다. posting push0.1379s와
flush0.9463s는 별도다. 하위 값은 부모에 포함되며 물리 I/O·unique ref 수가 아니다.
이를 근거로 다음 둘을 한 코드 배치로 합류한다.

- **기존 map read window caller 연결:** SourceInputCursor의 첫 change와 no-op return
  뒤에만 canonical `maps.scoped_reads`를1회 호출한다. 같은 attempt·원 WorkBudget의
  maps port만 바꾸며 source cursor는 원 port를 유지한다. DATA·box는 changed branch/
  LexicalBuild 안에서 소멸하고 NativeTextRoot install 전에 해제된다. 빈 bootstrap과
  no-op patch는 factory0,미지원 backend None은 원 경로다. Storage cache 재구현은0이다.
- **기존 confirmed row의 검증된 count:** resident64를 늘리지 않고 기존 scratch row에
  nullable validated_count1개만 둔다. same prepared attempt·확인된 ref만 첫 object get+
  full preflight 성공 뒤 count를 저장한다. Missing/disabled/unprepared와 prior side는
  원 검증 경로다. ref/type/1..8192·fence·취소 검사를 유지하고 SQL UPDATE 전에 원
  work/capacity를 예약한다. SQL/FULL/admission 오류는 attempt를 끝내며 fallback·부분
  재시도는 없다. 별도 digest/cache/table/영속 format 또는 상한 증가는 없다.

Root는 `text.rs` 생산 접합과 공용 mount를,독립 worker는 기존 spool/API·SQL oracle,
map-window child oracle,count-spill child oracle를 각각 독점한다. 코드가 모두 합류한
뒤 static/strict→owning→기존 Native Text UDS gold→normal pair build를 단일 큐로
실행한다. 신규 count의 cold SQL UPDATE 비용 때문에 순이득은 아직 NOT_RUN이다.
새 알고리즘 배치는 원 순서대로 Large Off1회→실패 시 같은 pair Record1회다.
MapApply45.5958s/CPU33.6584s와 COMMIT120/20.1019s는 여전히 별도 병목이다.
이 두 수정만으로 Large 통과를 예고하거나 원 C7 수용을 닫지 않는다.

**C6 후속 코드 — physical unique64 occurrence packet (2026-11-03):**
C6R1에서 Prepare453span/26.605s 중 COMMIT340회/13.406s가 관측됐고,
physical reserve/confirm170쌍의 입력6140개 중64개를 채운 묶음은3개였다.
원인은 먼저 논리 편집64개로 packet을 닫고 그 안에서 물리 중복을 제거하는 순서다.
첫5window의 물리 입력4779개/131쌍은 같은 물리64개 상한에서 최소77쌍이라는
수리 근거다. 이 값은 실행시간 예측이나 전체120s 성공 보장이 아니다.

- RAM에는 고유 payload 최대64개만 보관한다. 같은 ref의 payload가 다르면 즉시
  거절한다. 중복 term/owner 편집은 기존 inventory SQL의 typed Planned partition으로
  흘리며 기존64행 buffer·4GiB scratch·원 WorkBudget을 유지한다.
- 기존 root 정수의 Ready(0..63)/Planned(0..63) 태그를 폐쇄적으로 인코딩한다.
  새 table/index·공개 API·영속 객체 format·cache·Catalog/VFS 변경은 없다.
- 물리 put 전에 모든 fanout 행·정확 참조/개수·attempt·PK 충돌·promotion work/capacity를
  선승인한다. 전체 물리 반환을 검증한 뒤 opaque receipt가 해당 refs만 Ready로 전환한다.
  행/참조 drift와 SQL/FULL/취소 실패는 attempt 종료이며 부분 재시도/성공 fallback은 없다.
- global/root inventory cursor 모두 첫 결과 전에 전체 Planned/invalid partition 부재를
  확인한다. 늦은 오류에서 앞쪽 Ready prefix만 게시할 수 없다. prior 검증·count spill·
  map window caller·CAS/EOF·FULL·원 상한은 그대로다.
- producer/SQL/고정 oracle를 합류했으며 영향 strict→owning→실제 NativeText UDS→구조→
  normal pair build를 Root 한 큐로 수행한다. 새 Large/XL/mixed는 아직 NOT_RUN이다.
  기존 C6 Off/Record 실패를 새 코드 성능으로 승격하지 않는다. 검증 후 변경 pair
  Large Off1회→실패 시 Record1회→성공 시 XL/원960s mixed 순서를 유지한다.

**C6 진단 드롭 후 수리 — row event 증폭 제거:** 0f70 source의 C6PR1은
외부 helper의 trace cap 누락으로 PARTIAL이고,기존1GiB cap을 복원한 C6PR2도
1522event dropped/open1로 INVALID다. 둘 다 전체 RCA/overhead 수용에 쓰지 않는다.
앞의 완전하게 닫힌 동일5window는4779inputs의 Prepare131→77쌍을 확인하지만
원120s 제품 실패를 닫지는 않는다. canonical parser의 owner-span 거절을 우회하지 않는다.

관측 수리는 Root의 `read_diff_inventory_entry`와 Diff posting push 접점만 소유한다.
기존 `MapPageCursor::is_empty`를 재사용해 buffered next·EOF·빈 inline에는 clock/event를
만들지 않고 실제 page fill 시도만 Read span으로 감싼다. posting의 저비용 row push는
coarse Diff 안에 두고 SQL flush span은 유지한다. 실제 occurrence port read·CPU·원
WorkBudget/취소/오류는 그대로다. 새 queue/collector/schema·buffer 상한 확대는 없다.
129행의 fixed oracle는 page3/flush2/row-push0과 Off/Record 결과·work·error·port parent
동등성을 검사한다. named stage/lane의 과거 capture decoder는 유지한다.

이는 observer derivative다. scoped fmt/strict·집중 trace oracle·정상 pair 빌드 후
proper1GiB cap Record1회로 loss/open/lease/health0인지 확인한다. business ABI/포맷과
module graph가 바뀌지 않아 이미 통과한0f70 UDS3·경계 검사는 재실행하지 않는다.
이전155/155를 새 source 전체 owning 결과로 승격하지 않고 새 Off/제품 수용은
NOT_RUN으로 둔다. loss를0으로 만든 뒤에만 남은 MapApply 저장 증폭 수리를 선정한다.
외부 참조의 sorted block/locality 원칙은
[Tantivy SSTable 설계](https://github.com/quickwit-oss/tantivy/blob/main/sstable/README.md)와
부합하지만 해당 문서도 기본 block의 get tradeoff를 명시한다. 이는 새 compact
format의 성능 증거가 아니며 실제 byte bound/lookup·delta·GC/custody 계약 없이
format/전체 engine 교체를 발급하지 않는다.

<a id="e07-entry-map-rca-plan-20261103"></a>
**C6 후속 RCA·구현 결정 — F0 먼저, F1–F3은 조건부 후보:**
이 패킷은 원 DoD가 지정한 필수 저장 형식이 아니라120s 실패에 대해 선택한
성능 개선 후보이다. R0/F1–F3의 부재를 기존 WAL/FD/window/count/confirmation 수리의
미구현으로 해석하지 않는다. 기존 제품 코드와 마지막 실행 이후 실제 변경의
대조는 [현행 범위 감사](../../plans/oct-22-finalizing/VALIDATION.md#e07-actual-code-scope-audit-20261103)를 따른다.
드롭 없는 C6BR1은16MapApply65.494s/CPU50.732s,28117map inputs/224commits,
SQLite쓰기요청2.023GB를 보였다. 원120s는 FAILED다. 직접 거절은 BuildPrepare
deadline이며 numeric cap·메모리·외부 취소가 아니다. Catalog maintenance의59.004s는
maintenance가 foreground 뒤에서 기다린 시간이다. foreground wait는0.187s이므로
이를 GC가 foreground를59s 막은 원인으로 해석하지 않는다. 부모/하위 span은 합산하지 않는다.

기존 generic map에는 sorted bootstrap/COW·terminal inline·최종 reachable DAG 필터·
256개 설치/중복 제거·complete CAS/EOF가 이미 있다. 이를 다시 만들거나 batch/cap을
늘리지 않는다. Small8 증설은 모든 Copy root·metadata·child 한도를 넓히고,전역 map
codec 교체는 Storage/query/GC까지 영향을 넓힌다. Native EntryMap의 bounded block은
더 좁은 후보지만 실제 root 적합률·byte 크기·작은 delta 비용 없이 최종안으로 확정하지 않는다.

| 순서 | 실제 산출물·재사용 경계 | 결정·완료 조건 |
| --- | --- | --- |
| F0 입력 계수·계약 선택 | 기존 Large42 generator와 canonical TokenCursor/case_chars로 파일별 raw/folded unique term keys를 계수한다. 기존 harness의 test module만 사용하고 별도 tokenizer/CLI/제품 실행은 만들지 않는다. 고정4096files·원문 bytes8609531·corpus digest와 독립 표본 산술 gold를 대조한다 | Small8 적합 수,후보512rows/16KiB payload 적합 수,root 최대 rows/key/payload·wire 상한을 기록한다. 실제 encoded bytes·서로 다른 value refs·기존 reachable node/new-file 수·성능은 이 census로 증명하지 않는다. 적합률이 낮으면 F1 착수 전에 후보를 재검토한다 |
| F1 표현·codec·읽기 | Root가 `EntryMap`의 단일 closed representation과 object/map borrower signature·byte layout를 먼저 고정한다. 기존 Small8 우선,다음 후보 block,나머지 Generic. private codec은 정렬 keys와 같은 순서의 registered children을 저장하며 별도 child index를 넣지 않는다. 기존 object port/WorkBudget를 사용한다 | 독립 byte/order/count/child gold,중복·trailing·malformed·size/ref·메모리·취소 거절,lookup/scan. 최종 encoder와 F0 model을 같은 입력에서 대조한다. 정상 shape 초과만 Generic이며 admission/storage/corruption 오류는 그대로 전파한다 |
| F2 생산·CAS·custody | `entry_patch.rs`의 기존 full original merge/CAS·per-root/global digest·EOF를 보존한다. 최종 내용의 count/bytes로 표현을 결정하며 prior 형태에 따라 Generic을 유지하는 shortcut을 넣지 않는다. 완전 검증 뒤 기존 `AttemptContext::put_batch`로64roots 이하를 설치한다. payload·children·ordinal/output backing을 원 budget에서 선승인한다 | 모든 출력 identity를 사전 식별값과 비교한 후에만 roots를 반환한다. 같은 최종 ordered(key,ObjectRef) mapping의 full/delta 표현·block ref 동일성,Small↔block↔Generic,late conflict/changed stream/미소비 EOF/mixed ordinal/wrong later output/funding 거절에서 published prefix0. source provenance가 다른 전체 Text artifact의 동일성을 주장하지 않는다. 새 Catalog/GC authority 없음 |
| F3 실제 소비·artifact 결속 | Root가 `text.rs`,`text_overlay.rs`,`contribution.rs`의 membership/lookup/scan/MapEntries·Diff·query·inline_posting·child/reopen 접점을 함께 연결한다. 이번 Block 생산은 inventory에 한정하며 posting Small/Tree와32B singleton/8B tree inline은 유지한다. Block-for-posting은 typed corruption으로 거절한다 | wrapper→block→leaf의 registered children/순서/ref 검증과 pin/GC gold. layout를 output policy에 결속하고 실제 producer/config/reader의(schema,format,policy) tuple을 맞춘다. 외부 설정 format 값만으로 새 codec 식별이 보장된다고 가정하지 않는다. 비호환 입력은 기존 profile gate에서 reservation/provider 전에 거절;legacy reader/shim0 |
| 합류·판정 | 공용 계약 고정 뒤 codec leaf·producer leaf·독립 oracle/caller 준비를 disjoint writer로 병렬 진행한다. 공용 parent/type/profile/config/mount는 Root 한 writer,worker는 Root에게만 결과 반환·Cargo0·타채팅 연락0 | 한 큐에서 static→strict/영향 body→NativeText full/delta/noop/delete/query/reopen/pin/fault→정상 pair build. 변경 pair Large Off1회,실패 시 같은 pair proper1GiB Record1회. 통과 후 XL/원960s mixed. 변경 없는 실패 재실행0 |

**F0 결과 — 표현 gate VERIFIED:** 고정 Large42의4096/4096 roots가 후보에 들어간다
(Small8=0,Generic 필요=0). raw/folded unique rows 합계1146516,key bytes4926506,
root 최대286rows/key28B다. 후속 positional-child 계약으로 재결속한 모델은 payload
최대2014B/wire 최대11150B이며,2/2를 다시 실행해 확인했다. 이전2582B/11722B는
별도 child index를 가정했던 F0 이력으로만 남긴다.
따라서 F1–F3의 bounded 구현을 진행할 표현 근거는 확보했다. 실제 encoder/쓰기 절감/
120s·delta/query gate는 아직 NOT_RUN이며,다른 corpus나 posting root의 적합률로
일반화하지 않는다. F1–F3은 여전히 NOT_IMPLEMENTED다.

**Root가 먼저 고정할 내부 API:** 기존 `EntryMap::{lookup,scan}`과 `MapEntries::new`는
동일 context의 `&dyn ImmutableObjectPort`와 `&dyn PersistentMapPort`를 함께 빌린다.
기존 key/after/limit/WorkBudget·반환 계약은 유지하고 새 public port/DTO를 만들지
않는다. block reader의 parser scratch/returned page 수명과 registered children의
순서·중복·행 대응 규칙을 codec leaf 및 독립 oracle writer가 같은 명세로 소비한다.

**문서상 고정한 byte 계약:** payload=`12 + Σ(2 + key_bytes)`이며 rows≤512와
payload≤16384를 함께 만족해야 한다. registered children은 행당 하나씩 같은
순서로 두고 중복 ref도 보존한다. child index/dictionary는 만들지 않는다. 현재
object wire는16B header+payload+32B×rows로 상한32784B다. **16KiB는 전체 wire
상한이 아니다.** 아래 R0에서 실제 private ABI와 이 계약을 맞춘 뒤 병렬 구현을
시작한다. 현재 codec의 실측값은 아니다. 준비64roots의 payload·refs·scratch 합계도
원 예산에 포함한다. 새 cache/port/allocator/worker·전체 engine 교체는 발급하지 않는다.

**반증·중단 조건:** F0 Go는 표현 가능성만 판정하며 비용·성능 gate는 보류한다.
saved trace의 aggregate28117inputs를 eligible root별 reachable/new-object 수로 임의
배분하지 않는다. F0 적합률은 후보 구현의 근거일 뿐120s 예측이 아니다. 실제
writer의 managed inputs/commits/requested bytes와 작은 delta의 block 재작성/조회
비용을 기존 Generic과 같은 입력·예산에서 대조한다. 64-row page lookup/scan의
read/decode 예산도 독립 oracle로 고정한다. full은 줄어도 delta/query가
원 계약을 깨면 이 후보를 수용하지 않는다. 최종 gate는4096files 전체 게시·검색과
원 deadline/예산이며 Map16 실패 prefix를 전체 진행률·예상속도로 외삽하지 않는다.
최신 F0 결과와 구현 상태는 [검증 기록](../../plans/oct-22-finalizing/VALIDATION.md#e07-entry-map-plan-audit-20261103),
단일 현재 상태는 [README E07](../../plans/oct-22-finalizing/README.md#e07)이 소유한다.

<a id="e07-entry-map-contract-parallel-tests-20261103"></a>
**C8CR1 후 선택한 쓰기 횟수 수리:** [고정8-frame 계약](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-append-batch-20261103)을
기존 E07 controlled WAL pack owner 안에서 구현한다. iFirst==0의 새 append만
최대8프레임을 한 실제 walWriteToLog에 합친다. 기존 root/allowance/FD 확인은
그대로 실행하며 FULL/동기화 경계/덮어쓰기/foreign 및 큰 page 경로는 유지한다.
구현 A·독립 C oracle C·실제 VFS fixture B가 간섭 없이 병렬이고 Root만 test
mount와 정적 합류 후 Catalog strict→Runtime strict→WAL/VFS owning→Catalog/
lexical/Kernel/UDS 영향→정상 pair→변경 Large Off/필요 Record를 실행한다.
CPU sample이나 상위 span 합으로 실제 절감 시간을 추정하지 않는다. BlockPages와
작은 Arc/lock 후보는 현재 선택한 필수 작업이 아니다.

C9WR1은 첫64-input Map의 동일 물리 바이트에서 WAL2424→305calls를 확인했지만
전체 Map spill158543pages가 남았다. 다음 단일 비교는 기존 cache를48MiB로
배분하며 총384MiB/floor320MiB·원120s/FULL은 유지한다. static admission의16MiB
여유는 RSS cap이 아니다. [source와 재사용 경계](../../plans/oct-22-finalizing/VALIDATION.md#e07-cache48-budget-variant-20261103)를
동결한 뒤 변경 strict/정상 pair→Large Off1/진단 Record1 순으로 실행한다. 현재
Block layout의 고정 prefix pager/spill/write-count와 실제 수용을 보고 유지/철회한다.
옛 Generic16MiB의3% 결과를 이번 source 결과로 승격하지 않는다.

**C10 실행 후 token-stream checkpoint — 2026-11-03.** 원120s Large Off143.328s와
Record146.979s는 둘 다 FAILED다. Record Map spill0/WAL calls25741과 C9의
spill158543/WAL calls336046은 입력 prefix가 달라 paired speedup이 아니다.
Prepare2405개/51.199s·calling-thread CPU30.043s를 근거로 기존 TextSpool
token INSERT를 최대64행 SQL로 묶고,`term,pos`의 한 ordered cursor에서 bounded
term group을 방문하도록 실제 full/delta 공통 prepare caller를 이관했다.
`token_read_active`는 live cursor 동안 token reset/INSERT와 재진입을 거절한다.
callback의 inventory/confirmed scratch 작업은 원 connection·attempt·WorkBudget과
기존 admission/put/confirm을 쓴다. `journal_mode=OFF` scratch에서 오류가 나면
attempt 전체를 폐기한다. 중간 prefix에 대한 rollback 보장은 새로 주장하지 않는다.
lane2 전체 Prepare 아래 lane34 tokenize/spool과 lane35 term-loop가 중첩되며
Off 양쪽 진단 None은 직접 경로다. span 합계로 exclusive 비용을 계산하지 않는다.
이전170/171 실패는128그룹에257회 읽기가 필요한데 fixture가100회로 제한된 것이었다.
테스트만 기존1024-read packet budget으로 연결했고96B 메모리 상한은 유지한다.
수정96cf1f2의 strict·lexical/reader/spool/trace171·Kernel4·UDS5·hexagonal은
VERIFIED다. 구조/정상 pair도 통과했다. 그 뒤 같은 source의
영향 회귀/정상 pair→Large Off1,실패 시 Record1→성공 시 XL/mixed 순서를 따른다.
120s 수용·RSS peak·quiet overhead는 미수용이다.
[현재 source·검증](../../plans/oct-22-finalizing/VALIDATION.md#e07-token-stream-20261103).

**C11 이후 실제 순서 — 2026-11-04.** token R4 영향180개/정상 pair는 통과했으나
원120s Large는 Off145.324s/Record141.878s로 FAILED다. Record3325/4096 owner 준비,
단일-cursor token SQL1.103s,Prepare+Map main admission16.328s를 확인했다. 아직 실제
연속 run 비율이나 새 성능 수용은 없다. [checkpoint R0](../../plans/oct-22-finalizing/VALIDATION.md#e07-checkpoint-batch-contract-20261104)의
max8 adjacent write를 구현했고 C UBSan 및 f591a0e3의 영향565개·strict·구조·정상
pair가 통과했다. C12 Off155.630s/Record164.032s는 원120s FAILED다. 첫 Map의 동일
요청 바이트에서 main xWrite44.54% 감소는 확인해 유지한다. host load72.76과 swap
37543.38M의 시점 관측이 있어 현재 전체 timing 비교는 BLOCKED다. 다음은 충분히
조용한 로컬 현재 pair/원 caps의1회 실행이며,실패하면 해당 Record로 새 코드 수리를
선택한다. 과거 미완료 설명에서 완료된 구현을 다시 배정하지 않는다. 실패/interrupt는
pending buffer를 버리고 원 WAL을 유지하며 sync/backfill 순서와 원120s는 유지한다.
기존 quiet overhead·전체 자원·UI의 별도 미수용을 코드 수용으로 바꾸지 않는다.

**256 envelope 후속 — 2026-11-04.** [실제 계약·실행](../../plans/oct-22-finalizing/VALIDATION.md#e07-install-envelope-contract-20261104). 호스트와 코드 RCA를 분리했다. 같은 입력/바이트7개 Map 구간에서 CPU와 CPU 외 시간이 모두 증가했다. Core/producer/Storage 접합,독립256 oracle,spool joint 예약/confirmed64 SQL 코드를 합류했다. 기존64 상한 테스트2건은256 계약으로 정정하고257 거절·처음/마지막 ordinal·전체 출력 검사를 유지한다. Catalog child-rich 선형 prefix 조회는 fixed sorted256으로 수리한다. 소스/정적 합류→영향 strict와 Core/Storage/Catalog/lexical/Kernel/UDS→경계·모듈·정상 pair→변경 Large Off1,필요 Record1 순서다. 변경 없는 C/VFS 회귀는 원 source 증거를 유지하며 새 실행으로 세지 않는다. 새 Large 전까지 원120s FAILED,quiet overhead/전체 자원 수용은 별도다.

**C13 최종 상태 — 2026-11-04.** 위 256 패킷은 R7에서 합류·동결했다. 동일 소스의
Core3/Storage64/Catalog301 원 근거와 새 Lexical181/Kernel4/UDS5(총558),관련
strict·경계·모듈·정상 test=false pair는 VERIFIED다. 실제 같은1507입력의 설치
쌍24→6,6841입력110→29를 확인했다. C13 Off/Record는 원120s BuildPrepare
deadline FAILED이며 host paging을 별도 기록했다. Quiet 비교 BLOCKED다. 현재
동결본의 안정된 로컬 수용이 다음 조건이며 새 wire/Map256/read-cache 구현은
측정 근거 부족으로 배정하지 않는다. 이번 범위의 추가 확정 코드 결함은 없고,
다른 owner·완료 구현을 재발급하지 않는다. [정확 범위·미수용](../../plans/oct-22-finalizing/VALIDATION.md#e07-install-envelope-contract-20261104).

**후속 요청 단위 RCA — 2026-11-04.** [현재 원장](../../plans/oct-22-finalizing/VALIDATION.md#e07-sourceadvance-rca-20261104).
R7의 `안정된 로컬 수용만 남음`은 그 패킷 마감 시점의 판단이다. 같은120s의
SourceApply17.638s→실패 Lexical101.679s와 COMMIT33.945s/중간 물리 설치35.808s를
확인했다. pending-before-confirmed 음성 조회를 제거하고 원 물리·inventory·환급과
추가 읽기65회 oracle을 포함한181개와 owning all-target strict가 VERIFIED이며
변경 제품 NOT_RUN이다. 다음 코드는 COMMIT CPU/native sync와 물리 설치의
validation/write/sync/rename/directory/barrier 비용에 근거해 고른다. 원120s·내구성·
자원 계약을 유지하며 새 Map256/wire/cache 패킷이나 다른 owner 작업은 발급하지 않는다.

**Namespace/CPU 후속 수리 — 2026-11-04.** [R3 현재 원장](../../plans/oct-22-finalizing/VALIDATION.md#e07-namespace-cpu-repair-20261104).
관리형 namespace 중복 stat2회 제거·변조12경우 oracle과 managed/SQL CPU→V5
parser/Perfetto consumer가 합류했다. Core production strict·Python772는 해당
R1/R2 소스에서 VERIFIED이고 R3 owning all-target strict·집중276개·hexagonal도
VERIFIED다. 넓은 Runtime unit 재빌드는 본문 전 취소하고 변경하지 않은 Catalog
SQL은 재실행하지 않는다. 정상 pair는 VERIFIED,실제 C14NO1 Off 원120s는
FAILED다. C14NR1 Record도 FAILED이며 새 CPU attribution은 complete child trace로
확보했다. 후속 [Block/shard 수리](../../plans/oct-22-finalizing/VALIDATION.md#e07-block-cursor-repair-20261104)는
코드 합류를 끝내고 Root의 Storage+Lexical strict→focused owning→hexagonal
한 큐에서254/254·strict·hexagonal을 통과했다. 실제 Kernel wrapper의
read_retained 누락을 이어서 수리했고 production strict는 VERIFIED다.
Kernel caller17/17·경계·정상 pair도 VERIFIED이나 수정본 C15 Off/Record도
원120s FAILED다. Root 실행은 모두 종료했다. 전체 타깃은
변경하지 않은 Source rebase lint2건으로 FAILED,prior R4/R5/R6도 body0으로 보존한다.
원 COMMIT33.945s 중30.705s는 겹치지 않는 sync/settlement/checkpoint envelope이며
이를 CPU/host/device 비율로 바꾸지 않는다. FULL/parent sync·auto-checkpoint·
제품 한도·다른 owner 범위는 유지한다. 후속 C15 Off/Record는 FAILED이고 원120s는 미해결이다.
다음 단일 코드는 같은 물리 설치 묶음의 child/path/create/publish/retire/settle
비용 분할이다. 구체 도달 비용 없이 새 cache/window/바인더 API를 병렬 재발급하지 않는다.
11-05 [같은 설치 묶음 계측](../../plans/oct-22-finalizing/VALIDATION.md#e07-physical-attribution-20261105)은
Core/V5 계약→Storage producer ∥ Python consumer→Root canonical/직접 소비자 접합→
정적 검토→한 영향 strict/body/경계 배치→정상 pair/원 Large Record 순서다.
구체 비용이 드러난 다음 그 owner의 실제 중복만 수리한다. 한도·sync·credit·
custody를 완화하거나 계측 이벤트를 객체마다 발행하지 않는다.
계측/집중 검증/C16 wire와 fresh-claim Runtime strict·74/74·hexagonal은 VERIFIED다.
초기 admission 차단 뒤 공유23입력 drift를 보존하고 clean external8e94ca5c의 정상
pair를 빌드했다:0/872.034s·drift0. 원 Large Off C17IO1/Record C17IR1은 모두
120s BuildPrepare deadline으로 FAILED다. Record418,380events 완전성은 VERIFIED,
Lexical 두 span wall100.629s/CPU29.479s다. 차이71.150s를 pure host/device 시간으로
귀속하거나 partial 결과를 전체 구축으로 외삽하지 않는다. Root 실행/분석은 종료했다.
child live 검사·C checkpoint는 유지하고 full-bound 선행 생성은 maintenance/종료
거절 순서 반례로 제외했다. [현재 실행 순서와 수정 분기](../../plans/oct-22-finalizing/VALIDATION.md#e07-fixed-work-decision)는
초기 요청의 offline selector와 C16·C17 별도 clean checkout을 완료했다. 실제 V5
파서를 포함한 집중20개와 subtest7개는 VERIFIED다. 다음 순서는 기존 Medium 동일
작업 비교 → 근거가 지목한 owner의 수리 → 현재 고정 소스 원 Large120s다. 비교
launcher가 두 arm을 단일 lease에 연결하며 비교 증거 검증과 경합 거절을 포함한
집중37개 테스트는 VERIFIED다. 분석기 SHA와 Prepare 전체 하위 단계 작업량을
고정하고 실제 V5·canonical 검증기의 합성256-source 성공 경로를 확인했다.
실제 launch admission은 외부 Rust2개와 load44.987/상한8로 BLOCKED이고
Medium 비교는 NOT_RUN이다. 현재 공유 입력62/2,822차이는 최종 수용 시
재고정하며 추가 allowance 변경과 구현을 중복하지 않는다.
후속 [Medium 단독 진단·검증기 수리](../../plans/oct-22-finalizing/VALIDATION.md#e07-medium-child-root-20261105)는
C17 생산자0/26.892s·전체256 ACK15.845s를 확인했다. 정상인 독립 Operation
root2개를 거절하던 Python 검증기를 고쳤고 관련487개 통과·2개 skip 및 같은
원본의 offline 제품/canonical 검증은 VERIFIED다. 고정 consumer a41830d를
비교 실행기에 연결했다. C16/C17 효과 비교는 NOT_RUN,원 Large120s는 FAILED다.
후속 [인덱싱·호스트 RCA](../../plans/oct-22-finalizing/VALIDATION.md#e07-indexing-versus-host-rca-20261105)는
SourceApply18.419s 뒤 Lexical100.595s에서 원 예산이 소진됨을 확인했다.
토큰화보다 작은 객체 설치·검증 비용이 크며71.150s 비CPU 구간의 scheduler/I/O
분리는 미측정이다. 이미 있는 배치·dedup·검증 count 재사용은 재구현하지 않는다.
XL·mixed·overhead·운영 수용은 각각 별도 gate다.

**C7BR1 후 Catalog exact-set 계약:** child 검증 결과는 같은 실제 locked transaction
안에서만 재사용한다. 기존 ChildWindow 안의 exact open addressing만 바꾸며 작은
입력(max children/input≤64)은 원64slots를 유지한다. 큰 입력은 전체 child 수를
기존4096 상한으로 제한한 후 power-of-two slots≤8192·stored refs≤4096로 원
Temporary 예산에서 DATA 생성 전에 승인한다. 실패한 child는 기억하지 않는다.
full이면 미저장 ref는 기존 전체 metadata/edge/GC 검증을 수행한다. 메모리 거절을
잡아서 작은 캐시로 우회하지 않는다. billing plan과 실제 reserve/confirm이 동일
선택·삽입 순서를 사용하고 계획 DATA/grant는 실제 proof 생성 전에 해제한다.
독립 tests는 충돌 exact equality,>64 unique/repeat,새 transaction 재검증,late
corruption/rollback,작은4KiB envelope와 원 grant 거절/refund를 확인한다.
동결 후 Catalog strict/owning→lexical/Kernel/UDS 영향→정상 pair→변경 Large 순으로
Root가 합쳐 실행한다. Map49.782s 전부를 이 캐시의 예상 절감으로 계산하지 않는다.

#### R0 계약 먼저 → Root+3 병렬 구현 → 단일 검증 큐

이 절은 앞 F1–F3의 구체 실행 계약이다. **후속 실제 구현은 반영됐고 검증 중이다.**
R0 closed EntryMap/object-aware API·policy,inventory-only Block codec,기존 CAS/EOF
생산자와 모든 output identity 검사,실제 reader/overlay·Kernel/UDS oracle를 합류했다.
고정 source a51042e의 strict와 lexical133/133이 통과했고 후속 Kernel/UDS·제품 큐를
실행 중이다. 최종 판정은 [구현·검증 checkpoint](../../plans/oct-22-finalizing/VALIDATION.md#e07-inventory-block-implementation-20261103)가
소유한다. 아래는 그 구현의 계약/배정이며 과거 NOT_IMPLEMENTED 문구는 계획 시점의
상태다. 새 codec의120s 성능 성공은 아직 주장하지 않는다.
writer별 유일 파일은 [수정권 표](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-entry-map-exclusive-packets-20261103)가
소유한다. 새 사용자 채팅이나 별도 실행 큐를 만들지 않는다.

**R0 — Root 단독,병렬 source 작성 전 고정할 것:**

| 계약 | 고정 내용·오류 의미 |
| --- | --- |
| 단일 표현 | 기존 `EntryMap` owner를 `Small(SmallMap) / Block(ObjectRef) / Tree(ObjectRef)`의 closed Copy enum으로 교체한다. `native_serde_enum!`의 단일 현행 wire에서 tags0/1/2를 사용한다. default/빈 MapRoot는 Small0. Tree(None)·Small/Block 동시 필드는 표현하지 않는다. 별도 V2 IR/legacy reader 없음 |
| 적용 역할·결정성 | private `EntryMapUse::{Inventory,Posting}`를 기존 `entry_patch::apply`의 명시 인자로 둔다. Inventory는 Small8(all keys≤32)→Block fit→Generic,Posting은 기존 Small8→Generic이다. 같은 역할·policy·최종 ordered(key,ObjectRef) mapping에서 prior·편집 분할·full/delta로 선택을 바꾸지 않는다. 같은 CAS/merge를 재사용하며 별도 posting/inventory builder를 복제하지 않는다 |
| Block payload | magic/version8B=`QITXEM01`,row_count u16 BE,reserved u16 BE=0의12B header. 각 row는 key_len u16 BE와 key bytes. rows1..512,key≤1024,payload≤16384. 전체 key는 byte order로 strictly increasing. child index/dictionary0 |
| child/wire | `children.len()==rows`,child[i]는 row[i]의 value ref. 중복 refs는 그 순서대로 보존하며 같은 payload에 다른 child 순서를 canonical 동일체로 취급하지 않는다. wire bytes=`16+payload_len+32*rows`≤32784. Catalog의 기존 물리 edge sort/dedup은 재사용하며 객체 identity의 ordered children을 바꾸지 않는다 |
| key domain | codec은 byte-key map이며 token marker/UTF-8/owner-key 도메인을 새로 강제하지 않는다. 기존 map의 빈 binary key 의미도 바꾸지 않는다. inventory의 `check_term_key`와 posting의 owner decoding은 기존 실제 consumer가 수행한다. 합법이지만 Block key1024/shape를 넘는 입력은 Generic으로 가며 기존 map key4096 제한은 유지한다 |
| 실제 API | `EntryMap::lookup(&self,objects,maps,key,budget)->Result<Option<ObjectRef>,IndexError>`, `scan(&self,objects,maps,after,limit,budget)->Result<MapPage,IndexError>`, `MapEntries::new(objects,maps,root)`를 기존 port 타입으로 변경한다. `validate_for(EntryMapUse)`는 posting의 Block을 거절한다. `inline_map`은 Small에만 Some,`tree_base`는 Tree에만 Some-root이며 Block을 Tree로 추정하지 않는다. 공개 Core/port/SDK 타입 추가0 |
| codec/producer 접합 | private `BlockDraft::new(budget)`, `push(key,value,budget)->Result<bool,IndexError>`, `stage(self,&mut StagedObjectBatch,budget)->Result<usize,IndexError>`를 고정한다. false는 합법 shape 초과뿐이며 전체 원본 검증은 계속한다. stage는 기존 StagedPayload/Batch에 DATA를 옮길 뿐 설치하지 않는다. 새 public DTO/설치 권위 없음 |
| 읽기·수명 | 실제 object get의 canonical wire/digest 검증을 빌리고 requested ref·registration bytes·child count·header·모든 key/order/trailing 및 Small로 표현 가능한 비정규 Block을 **첫 hit/page 전** 검증·거절한다. late corruption을 앞부분 성공으로 숨기지 않는다. StoredObject가 원 DATA lease를 보유하고 반환 MapPage는 기존 page/key lease를 갖는다. parse/page·stage 입력/출력 backing은 원 budget에서 선승인하고 DATA보다 먼저 grant를 해제하지 않는다 |
| policy/profile | `TextRecipe.output_policy`에 positional EntryMap layout를 식별하는 고정 marker를 넣는다. schema/format은 기존 injected descriptor owner가 소유하며 Root가 실제 producer/config/reader의 정확 tuple을 대조한다. 구 policy 또는 등록 descriptor와 다른 format은 기존 profile/reader gate에서 reservation/build/provider 전에 typed refuse해야 한다. 외부 format 값이 codec 변경을 자동 탐지한다고 가정하거나 두 번째 format issuer를 만들지 않는다 |
| 노출·실패 | Conflict/Cancelled/ResourceLimited/CorruptObject/storage는 전파한다. 메모리·취소·손상을 Generic 성공이나 EOF로 바꾸지 않는다. 모든 original CAS/root ordinal/digest/EOF 및 모든 batch output ref 확인 뒤에만 결과 roots를 반환한다. 실패 시 published/returned prefix0. 기존 attempt에 귀속된 미게시 물리 객체는 가능하며 기존 abort/GC가 회수한다 |

**inventory-only 선택 근거:** singleton posting의 owner key는 정확히32B여서 현재
Small/inline 경로를 유지한다. 위험은 df≥9다. 기존 Generic posting은8B df+tree ref를
inline으로 저장하지만 Block posting은 wrapper를 추가해야 한다. 그 비용 이득은
측정하지 않았으므로 이번 변경에서 제외한다. F0가 계수한 것은 파일 inventory뿐이다.
공통 producer의 역할 인자·read validation만 명시하고 posting 알고리즘/marker를
새로 만들지 않는다. 이 결정으로 저장 표현의 적용 범위까지 R0에서 고정한다.

R0 산출물은 실제 parent type/signature·module visibility·policy/caller 수정점과
독립 oracle가 소비할 fixed wire 명세다. 임시 성공 body/미구현 fallback을 production에
넣어 컴파일을 통과시키지 않는다. 소스가 미완성인 준비 단계는 NOT_RUN이며 정적
계약 검토를 컴파일/행동 성공으로 표시하지 않는다. 계약 변경이 필요하면 Root가
영향 worker만 멈추고 변경 preimage/API를 함께 갱신한 뒤 재개한다.

**두 번의 병렬 wave:** 가용4slots를 Root+내부 worker3개로 쓴다. 슬롯을 넘는 작업은
이전 leaf가 고정된 뒤 같은 worker가 이어받으며,일을 늘리려고 공용 파일을 쪼개
여러 writer에게 동시에 배정하지 않는다.

| wave | Root | worker A | worker B | worker C | 합류 조건 |
| --- | --- | --- | --- | --- | --- |
| P1 | EntryMap parent·실제 caller/MapEntries·policy/profile 연결 | private block codec/reader/Draft | 기존 entry_patch의 merge/CAS/complete stream·staging/모든 output identity | 독립 BTreeMap/readable-object fixture·고정 byte/malformed/custody oracle | R0 API만 소비,leaf source 완성·static 리뷰 뒤 Root가 한 번 합류. 아직 Cargo/runtime 없음 |
| P2 | P1 통합·parent mount·공용 fixture signature·최종 dirty 대조 | 실제 Kernel pin/reopen/GC 전이 테스트 | 실제 UDS block query·구 policy/format 거절 테스트 | 실패·메모리 반환·tiny-delta/page 비용 oracle와 기존 gold 누락 보완 | actual codec·producer 경로를 밟는지 정적 확인. Root 통합 뒤 같은 source에서 단일 검증 시작 |

worker는 자기 allowlist 밖 source·Cargo/lock·parent를 수정하지 않고 Root에게만
handoff한다. peer/다른 채팅 연락·조회·재배정·직접 Cargo/product 실행0. handoff에는
변경 함수,사용한 확정 API,독립 expected oracle,남은 실제 접점과 NOT_RUN 범위를
적는다. shared main의 관련 semantic hunk가 바뀌면 해당 배정만 재대조하고,무관한
dirty 파일이나 파일명 overlap만으로 전체 작업을 중단하지 않는다.

**테스트는 아래 반증 조건을 먼저 작성한다.** 각 행은 필수 coverage이며 테스트
개수를 채우라는 지시가 아니다. 구현 출력으로 expected bytes/결과를 생성하지 않는다.

| coverage | 독립 oracle·필수 실패 입력 | 실제 관측·회귀 금지 |
| --- | --- | --- |
| C1 codec·형상 | hand-authored header/row/children/identity gold;Small8/9,1 long key33,rows512/513,payload16383/16384/16385,key32/33/1024/1025,빈 binary key/반복 child refs. malformed magic/version/reserved/count/길이·truncation/order/중복/trailing·missing/extra child | 최종 encoder와 F0 formula 대조. 손상된 마지막 행은 첫 lookup/page도 거절. 물리 child swap은 실제 store의 digest 검증으로 거절하며 codec이 별도 기대값 없이 semantic swap을 알아낸다고 주장하지 않음 |
| C2 query/page | BTreeMap과 hit/miss·after exclusive·limit1/64/4096·empty/EOF를 비교;limit0/4097 거절. inventory key와 posting owner domain의 기존 gold 유지 | 모든 행을 page 경계에서 중복/유실 없이 반환. page/backing lease가 clone/cursor/drop까지 유지되고 취소·메모리 거절에서 반환. 실제 읽기·decode 비용 계수,무제한 새 cache0 |
| C3 canonical merge | 기존 shared_entry_maps의6개 laws 재사용. Small↔Block↔Generic,empty와64 mixed ordinal,동일 최종 mapping의 full/one-edit/multi-edit·다른 prior 경로 비교 | 표현·Block bytes/children/ref 동일성. Generic4096의1-edit는 unrelated prior scan0을 유지한다. count>512를 이유 없이 full scan하는 변경 금지 |
| C4 whole failure | 마지막 CAS/외래 ordinal/중복·역순 key/바뀐 root cursor/global stream/EOF cancel/조기 consumer,short result·wrong last output identity,각 준비·output grant 거절·중간 취소 | compact 설치 전 원본 stream 완전 검증,모든 반환 identity 검증,반환/게시 prefix0. 실패 후 원 view/pin 유지·원 attempt 정리와 grant 환급을 확인. 실제 I/O 실패는 원 typed cause |
| C5 실제 lifecycle | `native_lifecycle/text_entry_maps.rs`의 실제 FileObjectStore+Catalog를 확장. tiny-inline→Small8→Block→강제 Generic→Block→empty,정확 old pin·cold reopen·마지막 pin 해제/GC | 기존9→Tree 기대를 지우기만 하지 않고9→Block gold와 실제 Generic boundary를 추가한다. wrapper→block→leaf·source child를 물리 확인. Generic pin 회귀도 유지 |
| C6 실제 UDS·policy | >8 eligible inventory와 df1/8/9 posting을 보장하는 source-derived corpus,고정 BM25/phrase/regex/DSL/path/overlay·delta/noop/delete gold;old policy·descriptor format mismatch 각각;위조 Block-for-posting | inventory는 실제 Block,posting은 기존 Small/Generic임을 확인. 반환 결과와 cold reopen/selected view,거절 시 reservation/build/provider calls0. 기존 소규모 BM25와 Literal same-kind 테스트를 NativeText block/정책 증거로 대체하지 않음 |
| C7 비용·lifetime | 같은 mapping의 bounded block/기존 Generic fixture에서 full·tiny delta·64-row scan/lookup의 WorkUsage/managed input/wire/commit 계수 비교,lease deny/drop 실패점 | 행 수와 물리 객체/새 파일 수,SQLite 요청 bytes와 media bytes를 구분한다. full 절감만으로 delta/query 회귀를 수용하지 않는다. sampled RSS를 whole-life peak로 승격하지 않음 |
| C8 실제 fault·운영 | 변경 경로에 기존 ENOSPC/short-write/fsync/cancel/process-death oracle를 재사용하고 실제 block 객체/게시 접점을 밟도록 입력 보완 | 기존 authority/fault 주입 재개발0. 이전 source의 통과나 짧은 compaction을 현재 block·지속 운영 성공으로 합산하지 않음 |

기존 lower 증거의 정확한 경계는 Storage
`registration_sharing_keeps_ordered_repeated_children_and_slots_until_drop`,
`scalar_and_batch_wire_denial_precede_durable_reservation_and_io`,
`managed_scalar_barrier_failure_retains_ticket_without_confirmation`와 Runtime
`native_object_write_io::tests::kernel_short_write_keeps_full_charge_until_cancel_closes_and_syncs`다.
이를 Block 제품 수용으로 승격하지 않는다. P2의 실제 block fixture에서 기존 IO fault
포트를 빌려 마지막 write/sync·취소를 주입하고 old view/pin/미게시 상태를 확인한다.
실제 OS ENOSPC/FUSE/process-death 입력이나 환경이 없으면 해당 범위는 BLOCKED 또는
NOT_RUN으로 남긴다. ANN seal·offline migration fault를 NativeText 경로로 계산하지 않는다.
query 행은 지원 capability에만 고정 gold를 적용하며,미지원 capability는 기존 typed
refusal을 유지한다. 이 codec 변경을 핑계로 새 query 기능을 활성화하지 않는다.

policy 음성의 기대 오류도 기존 권위대로 구분한다. build의 구 policy/등록 밖 format은
`InvalidInput`,query의 descriptor format mismatch는 `SnapshotUnavailable`,구 policy는
reader validation의 `InvalidInput`이다. 이미 받은 member의 stale input policy는
`pinned_root`의 `Conflict`이며 artifact object get 전 거절한다. `begin_input` 등 사전
준비까지 호출0이라고 확대하지 않고,피검 단계 이후의 install/reservation/build/provider
미호출을 관측한다. 오류를 하나의 임의 generic refusal로 통일하지 않는다.

**Root 실행 순서·stop rule:**

1. P1/P2 source 완료 후 scoped rustfmt·diff/모듈/hexagonal·소비자 검색·독립 oracle
   리뷰를 먼저 한다. 기존 성공 fixture가 실제 block을 밟는지 확인하고 원 golden을
   약화하지 않는다. 전체 source가 맞기 전 반복 compile/product0.
2. 단일 source에서 아래 strict와 owning body를 실행한다. selector 발견0·compile만
   성공·본문0은 통과가 아니다. 실패는 원인 클래스/실제 body 수를 기록하고 관련
   source/oracle를 수리한 경우에만 해당 범위를 재실행한다. 전역 jobs/test-thread=1을
   강제하지 않고 같은 lane의 검증·제품 실행만 Root가 직렬화한다.
   C1–C8별 필수 case가 실제 실행 목록에 있는지도 대조한다. 기존 테스트만 통과하고
   새 block/policy/fault module이 비어 있는 상태는 통과가 아니다. P2-B의 필수 body는
   `text_entry_maps::block_queries_preserve_fixed_gold_across_delta_delete_noop_and_reopen`,
   `text_entry_maps::old_layout_policy_and_foreign_format_refuse_before_native_text_work`,
   `text_entry_maps::block_install_fault_keeps_old_view_and_original_pin`으로 이름을 고정한다.
3. 실제 UDS/Kernel pin-GC/fault 통과 후 정상 daemon/harness pair를 build한다.
   source/input/config/profile/binary를 기존 capture로 결속한다. 다음은 변경 pair
   Large Off1회→deadline 실패 시 proper1GiB Record1회다. trace loss/partial이면
   진단 입력 불충분을 기록하고 숫자를 성능 근거로 해석하지 않는다.
4. Large 전체4096게시·검색/원120s와 독립 gold가 통과한 뒤 XL→원60+900s mixed를
   실행한다. 별도 Kernel ignored4K와 같은 목적의 전체4096제품 실행을 무조건
   이중 수행하지 않는다. 누락된 비용 oracle가 있을 때만 해당 기존 rail을 선택한다.
5. 정식 overhead는 성공한 같은 pair의 quiet≥3쌍/paired median≤5%·loss0이다.
   host 경합·whole-life 자원 authority·UI의 기존 BLOCKED는 별도 유지한다. 코드
   회귀·제품·운영·release를 한 완료율로 합산하지 않는다. 성능 실패가 계속되면
   같은 실행 반복 대신 새 trace의 변경 후 주비용을 RCA하고 다음 단일 수리를 고른다.

아래는 **향후 실행할 명령**이며 이번 계획에서 실행됐다는 뜻이 아니다. 기존
bench-lane target/admission을 사용하고 `cargow` 바깥에 resource wrapper를 중첩하지
않는다. 외부 일회성 산출물은 checkout 밖에 두고 결과는 기존 VALIDATION만 갱신한다.

```sh
./scripts/cargow --lane bench-lane clippy --profile bench --locked --offline --all-features -p quanta-index-lexical -p quanta-index-searchd-runtime -p quanta-index-searchd-harness --all-targets -- -D warnings
./scripts/cargow --lane bench-lane clippy --profile bench --locked --offline --all-features -p quanta-index-kernel --test native_lifecycle -- -D warnings
./scripts/cargow --lane bench-lane nextest run --cargo-profile bench --locked --offline --all-features -p quanta-index-lexical --lib --no-fail-fast -E 'test(native::text::tests) or test(native::text_reader::tests)'
./scripts/cargow --lane bench-lane nextest run --cargo-profile bench --locked --offline --all-features -p quanta-index-kernel --test native_lifecycle --no-fail-fast -E 'test(text_entry_maps::) or test(native_text_statistics_follow_delta_delete_noop_and_pinned_history) or test(native_text_fixed_live_set_churn_preserves_current_and_pinned_gold)'
./scripts/cargow --lane bench-lane nextest run --cargo-profile bench --locked --offline --all-features -p quanta-index-searchd-runtime --test native_index_uds --no-fail-fast -E 'test(text_entry_maps::) or test(native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold) or test(native_uds_text_query_uses_selected_reader_need_below_interactive_envelope) or test(retained_text_session_survives_floor_and_releases_original_catalog_pin)'
./scripts/cargow --lane bench-lane build --profile bench --locked --offline --all-features -p quanta-index-searchd-harness --bin scale_matrix -p quanta-index-searchd-runtime --bin quanta-index-searchd
```

#### C7 — E07 전체 잔여를 누락 없이 마감

| 잔여 수용 | 같은 기존 실행/owner에서 할 일 | 완료 판정 |
| --- | --- | --- |
| 지속 운영·fault | mixed의 고정 query/freshness·600s pin·renew/release/debt/cleanup과 변경 경로의 ENOSPC/short-write/fsync/checked-close/cancel/process-death를 기존 gold와 대조 | 실제 body/독립 oracle. 기존 source별 회귀는 영향 범위만 재검증; 작은3회 compaction을 steady state로 대체하지 않음 |
| tracing overhead | 같은 binary/input/caps의 quiet Large Off/Record≥3쌍, 고정 phase paired median 증가≤5%·loss0 | local 기능/진단은 최대한 수행하되 host 경합이면 정식 판정 BLOCKED. LP3의 완전한 실패 trace는 overhead 수용 아님 |
| 실제 Perfetto UI | 기존 viewer에서 source-bound capture의 track/flow/오류 표시를 확인 | 11-03 실제 접근은 Work settings의 local browser disabled로 BLOCKED. raw canonical 검증·nlist 진단을 UI 확인으로 대체하지 않으며 설정을 임의 변경하지 않음 |
| aggregate 자원 | 기존 producer→profile의 전체 process RSS2GiB/scratch allocated4GiB/physical64GiB·모든 root/retired/deleted-open/worker 범위를 실제 authority와 대조 | under-cap sample은 continuous peak 통과가 아님. authority 부재는 BLOCKED; 새 sampler/ledger를 근거 없이 개발하지 않음 |
| 비용축·E08 인계 | [고정 E07 DoD](E07-performance-operations.md#dod)의 N/H/M·fanout/history·term/batch/branch/watch/model/policy를 기존 feature owner 입력·독립 gold·producer/detail/profile 결과와 대조 | 미수용 축을 성공으로 합산하지 않음. 새 코드가 필요하면 실제 누락 caller/계수 hunk만 같은 owner에 귀속. E01/E02/SDK/CI/release를 이 채팅에 중복 구현하지 않음 |

C의 정책 코드·영향 회귀·Large/XL/mixed와 E07의 정식 성능/자원 수용은 각각
판정한다. C만 끝나도 E01–E08 전체 완료가 아니다. 종료 시 README E07·VALIDATION을
최종 source와 맞추고 기존 E08에 결과를 인계한다. 필수 FAILED/BLOCKED/NOT_RUN을
감춘 완료 선언이나 새 미구현 후보의 반복 발급은 하지 않는다.
