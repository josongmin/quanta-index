# 검증·합격 조건

[최종 계획](README.md)의 제안 변경을 위한 acceptance matrix다.
아래 새 시나리오는 계획 수립 시 모두 `NOT_RUN`이다. 기존 ADR에 기록된 과거 실행까지
미실행으로 바꾸는 표가 아니다. 실제 작업에서는 실행한 항목만 `VERIFIED`/`FAILED`로
갱신하고, 필요한 입력이 없으면 `BLOCKED`, 실행하지 않았으면 `NOT_RUN`으로 남긴다.

## 모델과정책

| ID | 독립 입력/경계 | 합격 조건 |
| --- | --- | --- |
| A01 | 같은 차원의 모델 A→B; 빈 delta·부분 delta; SearchCorpus와 직접 adapter | 기존 `SEM_MODEL_MISMATCH`; provider 호출 0, 신규 event 예약 0, target 생성 0 |
| A02 | 동일 모델의 revision/공통 정책 변경, 필수 계약 누락 | 변경 필드별 typed refusal; 크기·차원 일치만으로 통과하지 않음 |
| A03 | A→A delta, A→B 전체 교체, 재시작 | 정상 구축과 query 계약 일치; 완료된 원 publication replay는 재임베딩하지 않음 |
| A04 | 다른 query/document prefix를 요구하는 고정 모델 계약 | 허용된 비대칭 조합 성공, 잘못된 조합 거절; digest 전체 동일성만을 oracle로 쓰지 않음 |

모델 호출을 세는 독립 test double과 storage/event 관측으로 부작용 부재를 검사한다.
모델을 바꾸지 않는 fixture만 통과한 결과는 A01 증거가 아니다.

## 발행과이력

| ID | 독립 입력/경계 | 합격 조건 |
| --- | --- | --- |
| B01 | admission 이후 SDK 연결 종료, 서버 완료, 실제 daemon 재시작, 원 이벤트 replay | 원 publication/receipt 복구, 새 target retarget/re-embedding 없음 |
| B02 | activation commit 이후 ACK 차단; 같은 candidate를 다른 요청도 시도 | 현재 상태 복구와 원 ACK/해당 요청의 성공 귀속을 구분 |
| B03 | 후속 활성화·rollback·ABA·incarnation 교체·stale writer | 현재 head만으로 원 요청 성공을 단정하지 않음; 잘못된 candidate 활성화 없음 |
| B04 | 동일 이벤트에 다른 body, 오래 지연된 재전송 | 충돌 거절; payload mismatch와 timeout을 같은 결과로 축약하지 않음 |
| B05 | events/streams/roots/envelope cap 직전·경계·초과; rollover 중 crash | 참조·pending 보존, 안전한 전환 또는 거절; 반쪽 catalog 상태 없음 |
| B06 | 구 epoch 만료 후 replay, 역순 완료와 sequence 공백 | 구 요청 재실행 없음; high-water가 미완료 이벤트를 덮지 않음 |

Activation 거절 테스트는 commit 후 ACK 유실을 대신하지 않는다. Scripted peer의 응답
검사와 실제 daemon/storage commit·프로세스 재시작은 별도 scope로 기록한다.
각 durable 전이 전후에 crash cut을 두고 receipt/event/head가 허용된 조합인지 확인한다.
SIGKILL 복구는 전원 장애 내구성과 다르다. 후자까지 주장하려면 fsync·directory persist·
rename 경계와 해당 저장소 fault 모델을 별도로 검증한다.

## 증분검증과저장형식

| ID | 독립 입력/경계 | 합격 조건 |
| --- | --- | --- |
| C01 | exact event replay / 내용이 같은 새 이벤트 / semantic만 불변인 delta | 각 의미에 맞는 receipt/event 동작, 불필요한 provider 호출·계산량을 각각 계측 |
| C02 | 변경 없는 bucket, 한 bucket 변경, 정책/검증기 버전 변경, cache eviction·재시작 | 정확한 증거만 재사용; miss는 독립 검증; 전체 inventory와 삭제 정확성 유지 |
| C03 | 같은 크기·mtime 복원 변조, 누락 객체/행, 중복 ID, 잘못된 root·posting·정책 | cache hit에서도 거절; 예상 결과를 producer 출력의 자체 digest만으로 만들지 않음 |
| C04 | 편중 source key, 페이지 크기 경계, split/merge/delete/빈 root, 잘못된 child/count/range | 페이지·scratch·깊이 한도 준수; 누락·중복·과도한 decode typed refusal |
| C05 | 연속 delta·삭제·재시작·rollback·GC와 독립 전체 재구축 | 논리 record/embedding 집합, 삭제, source↔posting 관계, 정확 검색 결과 일치 |
| C06 | Lance compaction/delete/remap, 공유 페이지 GC, 새 형식 전환 중 crash·구 reader | row/index coverage 정확성, 보유 root에서 완전 복구, 미지원 형식 거절 |

Fixture는 고정 source·독립 기대값·고정 vector를 사용한다. 실제 provider 호출은 별도
통합 검증이며 그 비결정성을 storage parity 실패와 혼동하지 않는다. 논리 commitment는
계약이 canonical일 때 비교한다. physical file layout/native row address/page root의
동일성은 기본 합격 조건이 아니다. ANN은 별도 gold와 사전 recall/tolerance 기준으로 판정한다.

## 읽기수명과동시성

| ID | 강제 실행 순서 | 합격 조건 |
| --- | --- | --- |
| D01 | 획득 성공·pin=1 확인 → 의도한 panic → unwind | 지정 panic 도달/내용 확인, pin=0 복귀, 이후 GC 가능 |
| D02 | acquire·activation·retire·GC·republish를 barrier로 교차 | pin 보유 객체 보존, retired 신규 획득 차단, shared reachable 객체 보존 |
| D03 | cancellation/drop, 재시작, 허용된 process 소유 범위 | 누수·deadlock 없음; 단일 프로세스 보호를 다중 프로세스 보장으로 확대하지 않음 |

임의 sleep이나 많은 반복만으로 경쟁 조건 도달을 추측하지 않는다. 획득/퇴역의 전후를
제어하는 barrier·상태 관측으로 반례 스케줄을 실행한다. native/다중 프로세스 경계는 실제
경계에서 검증하고 in-memory 모델 검증과 구분한다.

## 자원과중단

| ID | 입력/장애 | 합격 조건 |
| --- | --- | --- |
| R01 | 요청별로는 작은 동시 ingest/query/maintenance가 합계 상한을 초과 | 할당 전 공유 예약, 제한된 queue/backpressure 또는 typed refusal; 누락·중복 charge 없음 |
| R02 | provider timeout·retry/backoff·다음 window에서 예산 소진 | 같은 deadline 소비; 추가 호출 정지; staged 상태와 원 이벤트로 복구 |
| R03 | staging/compaction/journal 쓰기 실패·disk full, pin으로 GC 보류 | 새 generation 오활성화 없음; 기존 활성 세대 조회·원 결과 복구 가능 |
| R04 | 반복 예산 소진 후 dependency 회복·실행 기회 제공 | checkpoint부터 전진하거나 지원 예산으로 완료; 무한 처음부터 retry를 진행 보장으로 인정하지 않음 |

실행 profile은 측정 전에 `B_wait/B_exec/B_recover`, 동시성·queue, 회계/RSS/디스크 ceiling,
최소 재개 단위와 interrupt 불가능 구간을 지정한다. 엔진 내부 soft limit과 cooperative
deadline의 초과 가능 범위를 숨기지 않는다. RSS 샘플링에는 최대 관측 간격을 함께 기록한다.
샘플 최대값은 실제 최대값의 증명이 아니며 hard RSS 보장은 별도 enforcement가 필요하다.

## 성능과운영

| ID | 변화시키는 축 | 확인할 결과 |
| --- | --- | --- |
| P01 | 같은 Δ에서 N 증가; 같은 N에서 Δ 증가 | semantic 계산량, byte 인증량, root/inventory 순회량의 증가율 |
| P02 | 같은 논리 N·Δ에서 반복 갱신 H와 file/fragment 수 F 증가 | directory entries/hardlinks/manifest bytes/row aggregation/maintenance debt 증가와 상한 |
| P03 | cold/warm, cache eviction, 프로세스 restart, no-op 3종 | 캐시 편향 없는 시간·작업량, 현재 바이트 인증 보존 |
| P04 | 동시 query/ingest/compaction 및 pin 보유 | ingest p50/p95/p99, query tail·처리량·ANN 품질, 메모리·디스크·I/O 증폭 |

측정 시각을 구분한다: 요청 입장, publication receipt 확정, activation 확정, 실제 query
가시성. Seal 지연과 activation 지연을 분리하고 사용자에게 보이는 전체 갱신 지연도 측정한다.
중첩 span을 exclusive 비용처럼 더하지 않는다. CPU·read/write bytes·fsync·queue/provider
대기와 wall time을 구분한다.

과거 baseline과 비교하려면 source/입력/config/의존성/binary/실행 환경을 해당 계약대로
결속하고 사전 합격 기준, 반복 수, 순서와 warmup을 정한다. host 간섭과 실패 표본도 보존한다.
단일 instrumented XL을 정식 속도 개선이나 회귀 없음의 증거로 사용하지 않는다.
기존 `703d0e68` 결과는 비용 분해 출발점이며 새 변경의 수치가 아니다.

합격 숫자는 이 문서에서 추정하지 않는다. 각 profile에서 지연·throughput·품질·RSS·disk·
증폭 허용값을 실행 전에 고정해야 정식 비교가 가능하다. 없이 실행한 측정은 diagnostic이다.

## 실행 규칙과 완료 판정

Rust는 `./scripts/cargow` 또는 `Justfile`의 기존 owner recipe를 사용한다.
먼저 가장 좁은 결정적 회귀를 실행하고, 변경 표면에 맞는 계약 검사를 연결한다.

| 변경 표면 | 추가 확인 |
| --- | --- |
| public contract/SDK | `just rust-public-api`와 해당 producer/consumer 회귀 |
| crate/port 경계 | `just rust-hexagonal`, `just rust-cargo-modules` |
| activation/generation/query pin/state root/shared ingress | `just rust-profile test-daemon` 및 기존 owner 시나리오 |
| persisted/wire format | 구형/미지원 입력 거절, 전환·crash/restart, 완전한 reader/writer 통합 |
| 정식 성능·설치·pair | 기존 qualification 계약의 실제 실행; focused 테스트로 대체하지 않음 |

이 명령들은 앞으로 구현할 변경의 실행 계약이며 이번 문서 작성에서 실행했다는 의미가 아니다.
정확한 package/test selector는 구현 시 현재 catalog와 owner recipe에서 확정한다.
공유 자원의 빌드·대형 벤치는 직렬화한다. 재사용 가능한 동일 입력의 증거는 반복하지 않는다.

완료 기록에는 변경과 소유 범위, command, 관측 결과, 미실행 경계를 남긴다. Code 작성,
focused 회귀, storage/daemon, Semantica 통합, qualified benchmark를 각각 판정한다.
새 per-run log/snapshot은 저장소에 추가하지 않고 필요한 경우 checkout 밖에 보관한다.
