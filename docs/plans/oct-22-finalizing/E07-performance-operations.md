# E07 — 성능·관측·운영 수용

현재 Native 초기 인덱싱 성능 RCA와 신규 작업은
[11월5일 bugbash SSOT](../nov-5-indexing-bugbash/README.md)가 소유한다.
[RCA](../nov-5-indexing-bugbash/RCA.md)와
[계약·구현·검증 계획](../nov-5-indexing-bugbash/WORKPLAN.md)을 따른다.

상위 상태·다른 담당 범위는 [에픽 원장](README.md#e07),
원본 실행 결과는 [VALIDATION](VALIDATION.md),
전체 제품 종료 조건은 [R01–R40 DoD](DOD.md)를 따른다.
검색 benchmark/ranking은 [기존 SEARCH 기록](VALIDATION.md#e07-search-benchmark-rca-20261105)의 담당 범위다.

아래는 기존 고정 수용 계약이다. 날짜별 E07 계획·후보·진행 상태는
[보존 본문](../../history/nov-5-indexing-bugbash/E07-performance-operations.md)으로 이관했다.

<a id="rca-solution-20261028"></a>
## 이전 RCA

[10월28일 당시 RCA](../../history/nov-5-indexing-bugbash/E07-performance-operations.md#rca-solution-20261028)는 이력이다.
[현행 RCA](../nov-5-indexing-bugbash/RCA.md)에서 현재 원인과 미확정 항목을 확인한다.

<a id="e07-resource-authority"></a>
## 자원 권위

[기존 자원 권위 계약](../../history/nov-5-indexing-bugbash/E07-performance-operations.md#e07-resource-authority)의
원 cap·grant·거절·lifetime을 적용한다. 새 RCA 문서가 이 권한을 대체하지 않는다.

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

기존 사용자 유예(continuous RSS/전체 roots allocated peak·delegated Linux 등)는 실행 순서 유예이며 최종 DoD 면제가 아니다. 입력 부재는 BLOCKED/미실행은 NOT_RUN으로 남긴다. 기존 [세부 운영 사양](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-09-cost-observability.md)의 cap/clock/capture/오류/settlement 계약을 보존하며 결과를 본 뒤 같은 실행의 합격선을 바꾸지 않는다.
