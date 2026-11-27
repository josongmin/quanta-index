# E07 — 성능·관측·운영 수용

현재 Native 초기 인덱싱 성능 RCA와 신규 작업은
[11월5일 bugbash SSOT](../nov-5-indexing-bugbash/README.md)가 소유한다.
[현행 RCA 원장](../nov-5-indexing-bugbash/README.md)와
[계약·구현·검증 계획](../nov-5-indexing-bugbash/WORKPLAN.md)을 따른다.

상위 상태·다른 담당 범위는 [에픽 원장](README.md#e07),
원본 실행 결과는 [VALIDATION](VALIDATION.md),
전체 제품 종료 조건은 [R01–R40 DoD](DOD.md)를 따른다.
검색 benchmark/ranking은 [기존 SEARCH 기록](VALIDATION.md#e07-search-benchmark-rca-20261105)의 담당 범위다.

아래는 기존 고정 수용 계약이다. 날짜별 E07 계획·후보·진행 상태는
[원문 복구](../../ARCHIVE-INDEX.md#nov-5-e07-preimages)에서 확인한다.

<a id="rca-solution-20261028"></a>
## 이전 RCA

[10월28일 당시 RCA](../../ARCHIVE-INDEX.md#nov-5-e07-preimages)는 이력이다.
[현행 RCA 원장](../nov-5-indexing-bugbash/README.md)에서 현재 원인과 미확정 항목을 확인한다.

<a id="e07-resource-authority"></a>
## 자원 권위

- 원 cap·grant·거절·lifetime을 적용한다. Large 운영 profile의 논리 work 한도는
  실제 RSS·allocated disk 상한이 아니다. 원4K의 1M reads/512MiB·30초 및
  worker 10초는 독립 gate이며 Large·Mixed 설정으로 바꾸지 않는다.
- 정식 resource high-water는 startup부터 drain·child 종료까지 완결된 동일
  source/binary·process identity·관측 창에 결속한다. Linux qualified owner는
  명시 위임된 cgroup v2 parent의 identity를 검증하고 workload 전부터 empty/reap까지
  관측한다. 불완전 ownership, PID/TID 재사용·이동, 누락된 live sample은 거절한다.
  kernel `memory.peak`는 cgroup memory 값이며 aggregate process RSS의 별칭이 아니다.
- process RSS는 PID/start-time별 harness/daemon/worker를 합산하고 동일 TGID는 한 번만
  센다. 공유 page는 중복될 수 있다. 주기적 RSS·비원자적 descendant census와
  500ms allocated-block traversal의 표본 최대는 continuous peak가 아니다.
  전체 writable root의 allocated-disk peak도 별도 authority가 필요하다.
  `false`, `null`, missing/error를 0으로 바꿔 cap 통과를 만들지 않는다.
- Object I/O의 reserve/confirm은 도달한 객체 항목 수이며 replay·기존 파일 확인도
  포함한다. unique 객체, 새 파일 생성, device write와 구분한다. 반환 write bytes와
  reserve/confirm·file/directory/barrier elapsed는 transaction·WAL·GC 전체가 아니며
  겹친 span의 합은 phase wall time이 아니다. 부모/자식 시간도 합산하지 않는다.
- 실제 마지막 ReadSession ticket release와 renewal, 같은 Catalog authority의
  bounded `SettledAtSnapshot`을 원 release 시각부터 60초 안에 대조한다. ViewRef 보관,
  과거 view 검색, 빈 reclaim page·forward cursor 끝, 물리 FD quiescence는 debt0이 아니다.
  보호 중 객체는 reclaimable debt에서 제외하되 물리 allocation에는 포함한다.
  상태 불완전·예산 초과는 `Incomplete`이며 성공이 아니다.
- 정식 capture는 같은 bench/daemon·source·binary SHA·input·caps·host/cache의
  Off/Record 직렬 교대 Large 3쌍 이상과 완전한 parent/first/reopen child trace,
  footer/loss·product 결과를 요구한다. fixed phase별 paired ratio 중앙값 증가 ≤5%와
  drop 0을 판정한다. 다른 clock/capture, 부분 진단, noisy host, failed ACK 또는
  diagnostic timing을 정식 overhead·제품 성공으로 사용하지 않는다.

[기존 자원·ReadSession 세부 원문](../../ARCHIVE-INDEX.md#oct-22-validation-and-w2-preimages)과
[E07 측정 이력](../../ARCHIVE-INDEX.md#nov-5-e07-preimages)은 source별 복구 자료다.

## 비용 계수와 maintenance 설정 해석

- `NativeTextIndex::plan/prepare`는 Bootstrap/Patch를 같은 builder에서 처리하고
  `InventoryReuse`·`InventoryBatch`는 prepare당 하나다. 확인 occurrence는 packet
  flush 뒤에도 재사용하며 기본 창은 첫 64 identity를 보존한다. 창 밖 재등장 비용은
  실측 전 추정하지 않는다. `OccurrenceWrites::flush`의 입력 감소는 하위
  `FileObjectStore::install_batch`의 물리 reserve/confirm/sync 감소를 증명하지 않는다.
- `source_mutation_count`는 fixture shape 검사다. 실제 mutation/fanout 측정이 아니다.
  `managed_install` deadline 기록은 만료 감지 위치이고 해당 줄의 독점 비용이 아니다.
  client 600초 I/O와 server 120초 dispatch는 서로 다른 기한이다.
- Native `maintenance`는 명시 운영 설정이고 생략하면 자동 drain이 꺼진다.
  Large/XL fixture는 20ms 간격·라운드당 한 단계·2초 round budget·512KiB 물리
  reserve를 선택한다. 재예약이 있으므로 실제 50Hz 실행이나 운영 기본값으로 해석하지
  않는다. 정책이 다른 실행으로 과거 실패를 소급 통과시키지 않는다.
- capture의 source/build·clock·operation·phase 귀속과 원 collector의 제외 범위를
  함께 기록한다. Catalog lock/commit, object I/O, producer/daemon 비용을 구분하며
  span 정의만으로 전체 경로를 관측했다고 주장하지 않는다. 동일 scope와 완전한
  trace에서만 비교하고 오류·중단·누락은 별도 판정한다.

## Fixed profiles and oracles

- 기존 P00–P09 trace/recorder/exporter·제품 ReadSession/renew/release/settlement를 재사용한다. 실패·timeout·reopen stop/join/start에서도 결과가 남고 observer 비용/누락을 분리해야 한다.
- Large=4096파일(seed42,16×256,평균120행), XL=32768파일의 독립 functional ACK/search/reopen profile을 유지한다. debug 결과와 bench/release throughput은 구분한다.
- 기존 Large 운영 profile: Linux aarch64 전용2CPU/6GiB·local SSD, 같은 bench binary/caps·논리 work와 실제 RSS/공간을 분리한다. host/storage 경합은 qualified timing이 아니다.
- 원960초 배치:60초 warmup+900초 open-loop50QPS, 동시4/대기64·초과도 실패,1초마다≤16KiB 파일 replace/delete/restore. ≥10,000 attempts/각 query class≥1,000,old-view witness와 실패/timeout·scheduled queue wait 포함.
- 실제 ReadSession pin600초,300000ms lease·120초 renewal/gap≤180초. 마지막 borrower 뒤 release·60초 이내 settlement. 기존 full/mixed의 cap·retention 선택은 원4K 요청 예산을 변경하지 않는다.
- 정식 Off/Record는 같은 binary/input/caps의 직렬 교대 Large≥3쌍·각 고정 phase paired ratio 중앙값 증가≤5%·drop0. noise/partial capture로 PASS를 만들지 않는다.


## DoD

- [ ] E01–06의 실제 제품 성공 뒤 Large/XL ACK/current·old search/reopen·같은 operation 복구를 독립 gold로 수용.
- [ ] warm lexical 예정 발행→decoded top20 p95≤100ms/p99≤500ms, one-file staging→ACK→검색 freshness p95≤1s/p99≤5s를 원 운영 profile에서 수용.
- [ ] aggregate 모든 process RSS≤2GiB·allocated scratch≤4GiB·physical≤64GiB, 실제 high-water·orphan/retired/WAL/FD/pin과 누락 없는 비용 회계. sampled maximum은 continuous peak가 아님.
- [ ] pin 해제 후60초 안 reclaimable debt0·shutdown grace5초·지속 churn/debt/drain. 보호 중 객체는 공간 회계에 포함.
- [ ] ENOSPC/short-write/fsync/checked-close·cancel/panic/process-death·long pin·space pressure에서 accepted publication 유실0, old-view 정확성·bounded cleanup.
- [ ] initial build·fixed delta/no-op·N/H/M·fanout/history·공통 term·1/100/1000-file batch·branch/watcher/model/policy 변경을 독립 축으로 계수. 숨은 전수 scan과 maintenance debt를 포함.
- [ ] [RFC 12.3/12.5절](../../rfc/oct-10-native-incremental-vnext.md#123-latency-stability-and-qualification-envelope)의 원 native/IDE profile을 실행 전에 고정하고 latency/quality/freshness·served rate·전체 비용을 함께 E08에 인계.

기존 사용자 유예(continuous RSS/전체 roots allocated peak·delegated Linux 등)는 실행 순서 유예이며 최종 DoD 면제가 아니다. 입력 부재는 BLOCKED/미실행은 NOT_RUN으로 남긴다. [원문 복구](../../ARCHIVE-INDEX.md#oct-22-validation-and-w2-preimages)는 당시 source의 세부 증거이며 결과를 본 뒤 같은 실행의 합격선을 바꾸지 않는다.
