# Native 단일 엔진 수용 기준



종료는 구현, 제품 실행, 기본값 전환, 퇴역을 모두 포함한다. 현재 소스의 Tantivy 제거와 전체 타깃 컴파일은 [06.2–06.3](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md) 범위에서 끝났고, Linux release·서명 번들·복구·제품 실행은 남았다. 이 문서의 추가 기준은 기존 [R01–R40](../oct-22-finalizing/DOD.md)과 [E07 운영 사양](../oct-22-finalizing/E07-performance-operations.md)에 더해 적용한다. 아래 수치는 시험 전 고정하는 기준이다. 관측 결과는 각 현행 오너 원장에서 판정한다.

## 제품 및 상태 계약

- [ ] 설치된 Semantica 생산자·SDK·`searchd`로 `FULL → 동일 Native view 읽기 → 재시작 1 → DELTA/delete/no-op·ACK 유실/재시도 → 재시작 2`를 실행한다. 재시작마다 독립 source oracle, 활성 view·operation과 게시 영수증의 일치를 확인한다. ACK 유실 뒤 동일 작업은 기존 영수증을 반환하고 새 게시가 0건이다. [기존 P09 exact](../nov-12-bugbash-c0-p09/tickets/B05-final-verification.md)의 입력이 일치하면 해당 DELTA 단언의 영수증을 재사용하고, 이 시나리오의 추가 view·delete/no-op·재시작을 확인한다. Large 재시작 시험은 이 생산자 결속의 대체 증거가 아니다.
- [ ] 일반 SDK·`searchctl`·런처·API/MCP·벤치의 실제 제품 진입점을 Native로 실행한다. 소비자별 유지 계약에서 view·정렬·페이지·필터·출처를 확인한다. 언어·저장소·파일 범위 등은 실제 호출을 기준으로 유지 여부를 기록하고, 퇴역 또는 미지원 조건은 wire에서 typed 오류를 반환한다. Legacy 소켓·상태 루트 없이 필수 호출을 완료한다.
- [ ] 같은 비운영자 신원·소켓·설정에서 허가된 조회가 성공하고 발행·활성화·관리가 거절된다. 별도 Control 권한과 유효한 이전 view pin의 정상 호출을 확인한다. 다른 workspace·만료된 pin·승인되지 않은 instance·취소·기한 만료와 구 버전 조합을 실제 IPC로 검증한다. 거절된 쓰기의 상태 변경은 0건이다.
- [ ] 파일 Hybrid는 read session의 같은 view·ticket에서 한 요청·한 누적 예산·기한·파일 적격 조건으로 두 reader를 실행한다. 두 차선 사이 업데이트, 요청 취소·timeout·오류 후 조회 정산, provider 미완료 시 coverage·traversal 전달을 검증한다. session pin은 명시 Release·TTL·서비스 drain에서 해제하며 조회 중 renew/release와 재시도를 시험한다. Text에 없는 dense-only 파일, 중복 벡터·필터·동점·rename·다른 producer를 독립 파일 oracle과 대조한다. 부분·근사 결과의 전역 exact 완료 표시는 0건이다.
- [ ] 제품 전환은 별도 물리 루트에서 수행한다. Legacy 쓰기 차단·미결 작업 정산 후 원본 manifest와 producer event high-water를 고정하고 두 루트의 원본 입력을 독립 대조한다. Legacy→Native 호출에서 대상 operation·게시 operation·SourceCut digest를 모두 요구하고 BOOT 영수증과 대조한다. 값 누락·불일치, 이전 선택 CAS 불일치, 중단 시 잘못된 head 발행은 0건이다. [현재 전환 API](../../../tools/deployment/launcher.py#L788)
- [ ] 전환 시 두 저장소의 source high-water·replay floor·root 신원과 Native 영속 게시 기준점을 기록한다. 복귀 시 생산자 admission을 닫고 daemon을 정지·join한 뒤, deployment lock 안에서 같은 Catalog의 typed offline 일관 읽기와 Semantica outbox·전달 영수증을 대조한다. 게시 기준점 이후 증가와 두 저장소의 미결·미전달 작업이 각각 0건이어야 복귀한다. 증거가 없거나 불일치하면 거절한다. 생산자 영속화 직후·Catalog 커밋 직후 ACK 유실·재시작에서 각각 확인한다. restore-forward·outbox 재생은 갱신 유실 0건, 중복 논리 게시 0건과 이전 view 정확성을 확인한다. [현재 서빙 잠금](../../../tools/deployment/launcher.py#L963) · [운영 복구 계약](../../operator/state-cutover-runbook.md#rollback-boundary)
- [ ] 기존 E08의 first-FULL 3개·Facts 2개 durable cut을 출하 후보의 봉인 파일→Catalog COMMIT→ACK 경계에 연결한다. Catalog WAL/FULL 설정과 참조 전 파일·디렉터리 내구성 순서를 확인한다. 프로세스 종료, ENOSPC·short-write·fsync 실패, 재오픈에서 이전 또는 새 view가 완전하게 읽히고 같은 operation이 원 영수증으로 정산되는지 검증한다. 프로세스 종료 시험과 전원 손실 내구성의 증거 범위를 각각 기록한다. [기존 5개 durable cut](../oct-22-finalizing/DOD.md#final-dod) · [Catalog 설정](../../../crates/quanta-index-catalog/src/connection.rs#L465) · [RFC 발행 순서](../../rfc/oct-10-native-incremental-vnext.md#81-one-authority-and-one-linearization-point)

## 기존 고정 성능 및 자원 기준

| 구간 | 기존 조건 | 통과 증거 |
| --- | --- | --- |
| Native Caddy / etcd 원 SDK60 | 고정 Caddy 590파일·etcd 1,421파일. 실제 SDK I/O 제한 **60초**, 물리 파일 **30,000**, Native 논리 work bytes **536,870,912**, reads/writes/mutations **각 1,000,000**. Off와 Record 모두 원 입력·제한으로 실행. | Full ACK·검색·재오픈·source digest 일치. 최초 거절, SDK I/O, staging, publish, reopen 시간을 별도 필드로 기록. Record에는 두 프로세스의 완전한 trace와 누락 0. [원 입력·결과](../nov-5-indexing-bugbash/WORKPLAN.md#root-integration-follow-up--fixed-sdk60-and-independent-file-holdout) |
| Legacy Caddy / etcd 비교 | 이전 릴리스의 동일 원 입력, SDK I/O 60초·물리 Work 예약 30,000개 유지. 각 2회. | 원 SDK 결과·쿼리·재오픈을 남긴다. Native 완료와 Legacy 비교 결과는 각 실행에 귀속한다. Legacy Work 예약과 전체 상태 루트 파일 수를 따로 기록한다. [기존 반복 측정](../nov-5-indexing-bugbash/RCA.md#file-v3-합류-후-원-sdk-반복-결과) |
| 표준 Large 재시작 | 삭제 전 Legacy 기준 회귀는 `process_readiness_owner_v1`의 `e2e_process_readiness::binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`였다. 준비 SDK 제한 300초, source 4,096개, 이후 독립 데몬 두 번. | Native 최적화 실행은 원 두 OS 재시작과 소스 해시·owner·저장소별 및 전역 4,096개 순위를 **VERIFIED**, 1/1·320.116초에 확인했다. 이 고정 입력은 Literal V3와 packed seek까지 포함하며 뒤이은 inventory 변경 전이다. 파일 30,000, 원 Large funding, 서버 120초를 유지했다. 출하 관련 입력이 바뀌면 영향 범위를 재검증하고, 전체 테스트 시간을 SDK60 ACK 수치로 사용하지 않는다. [실행 결과](tickets/NC-F-integration.md#f--original-sdk60-and-native-buildprepare-budget--2026-11-14) · [Native 회귀](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/large_restart.rs) |
| Native Large / XL | seed 42, 각각 4,096 / 32,768파일, 원 서버 dispatch **120초**. Large의 별도 논리 funding은 R/W/M 각 67,108,864 및 549,755,813,888B. | 원 full ACK·검색·재오픈·delta/no-op/delete·이전 view·같은 작업 복구. SDK60의 512 MiB 진단 한도, 외부 controller timeout, staging과 서버 dispatch는 각각 적용 대상과 시계를 유지한다. [원 Large 조건](../nov-5-indexing-bugbash/README.md#native-large-funding-boundary) · [F2 실행 계약](../nov-5-indexing-bugbash/RCA.md#native-final-f2-execution-20261111) |
| Native Mixed | Linux aarch64 전용 2 CPU / 6 GiB / local SSD. 60초 warmup+900초 open-loop, **50 QPS**, **1 Hz** ≤16 KiB replace/delete/restore, 동시 4·대기 64, ≥10,000 attempts 및 query class별 ≥1,000. | 예정 발행부터 decoded top20까지 검색 p95≤100ms/p99≤500ms, staging→ACK→검색 freshness p95≤1s/p99≤5s. pin 600초, lease 300초, renew 120초/최대 간격 180초, 해제 후 60초 내 reclaimable debt 0, shutdown grace 5초. 초과·실패·timeout·대기 포함. [운영 사양](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles) |
| 물리 자원·관측 | 원 운영 프로필 전체 process RSS ≤2 GiB, allocated scratch ≤4 GiB, physical ≤64 GiB. 정식 Large Off/Record 직렬 교대 ≥3쌍. | 실제 high-water에 WAL·scratch·보호/퇴역 객체·FD를 포함한다. Record 고정 phase별 paired ratio 중앙값 증가 ≤5%, drop 0. 논리 바이트나 샘플 최대값은 연속 물리 peak 증거가 아니다. [자원 및 관측 DoD](../oct-22-finalizing/E07-performance-operations.md#dod) |
| Native 원 ANN4K 및 제품 SLO | 4096×64/Q64/K10, Recall@10 ≥0.95, 원 1M reads/512 MiB, SDK 30초/worker 10초. Native/IDE의 RFC 12.3/12.5는 별도 하드웨어·기능·비교 조건을 적용한다. | 실제 ANN/정확 검색과 독립 gold, 원 예산 및 기존 Native/IDE 필수 운영 기준. 원 ANN4K의 통과를 대형 ANN이나 실제 SDK60의 통과로 이전하지 않는다. [원 profile 구분](../oct-22-finalizing/DOD.md#existing-profile-separation) · [RFC SLO](../../rfc/oct-10-native-incremental-vnext.md#123-latency-stability-and-qualification-envelope) |

## 이번 이전의 추가 승격 기준

이 표는 Native 단일 엔진 전환에서 추가로 적용할 제안 기준이다. 기존 테스트 수나 합격선의 대체로 사용하지 않는다.

| 판정 | 새 기준 | 실패 시 |
| --- | --- | --- |
| SDK60 반복 | Caddy·etcd 각각 cold Off 2회 + Record 1회. 각 호출에 원 SDK I/O 제한을 적용하고 submit 시작→full ACK는 60초 이내다. 검색·재오픈·source digest도 일치하며 staging·ACK·검색·재오픈 시간은 따로 기록한다. | 한도 거절·timeout·identity 불일치는 해당 회차 실패다. 요청별 최초 거절, 단계별 물리 파일과 논리 작업 계수를 확인하고 담당자에게 넘긴다. |
| Native LanceDB ANN | 원 ANN 프로필에서 Native 관리 LanceDB 리전의 실행을 확인한다. 같은 세대·필터·모델의 독립 exact 파일 oracle에서 Recall@10 ≥0.95를 전체와 각 필터 선택도 구간에서 충족한다. Oracle 상위 집합 크기는 적격 파일 수와 10 중 작은 값이다. 적격 0건은 빈 결과를 확인한다. | 정확도·쓰기 증폭·재오픈·유지보수 중 하나라도 원 게이트 미달이면 승격을 멈춘다. |
| 같은 기능의 성능 회귀 | 동일 입력·품질·하드웨어의 frozen baseline/candidate를 교대로 각 5회 실행한다. submit→ACK와 회당 조회 p95의 짝별 candidate/baseline 비율 중앙값이 각각 ≤1.05다. peak RSS/공간은 기존 절대 상한을 적용한다. | baseline과 candidate가 다른 기능을 반환하거나 호스트 조건이 다르면 해당 비교는 수용하지 않는다. 모든 필수 절대 성능 기준을 따로 통과해야 한다. |
| Native Hybrid 조회 | 같은 960초 운영 프로필에서 HybridFile은 예정 요청→decoded top20 p95≤100ms/p99≤500ms. 원 lexical/Mixed는 별도 통과한다. 후보·재정렬 수, 입력 토큰, 기한, query mix, 모델 및 캐시 조건은 실행 전에 고정한다. | 대기·timeout·거절을 분모에서 제거하지 않는다. 초과하면 재정렬 또는 후보 깊이부터 통제 실험하고, 제한을 낮출 때는 품질을 같은 실험으로 다시 검증한다. |
| Native Hybrid 품질 | 새 독립 평가에서 같은 세대의 Native 파일 lexical 대비 **파일 MRR@10 +0.02 이상**, 저장소 단위 쌍별 95% 하한 **>0**, Recall@10 차이의 95% 하한 **≥−0.01**. 식별자 구간 MRR@10 차이와 무정답 구간 오탐률 차이는 각각 **−0.01 이상**, **+0.01 이하**. 무정답 오탐 판정 임계값은 DEV에서 고정한다. | 부족한 사례/저장소 수, 미판정 상위 10개, qrels 노출 또는 경로 신원 불일치는 승격을 멈춘다. 8건 진단 점수는 새 검증을 대체할 수 없다. |

성능 비교는 같은 CPU·메모리·스토리지의 canonical exclusive 입장과 quiet-host 검사를 사용한다. cold마다 새 상태 루트를 사용하고 OS 캐시·warmup 조건은 고정한다. submit 시간은 단조 시계로 공개 SDK 호출부터 게시 ACK 수신까지 잰다. 조회는 open-loop 예정 시각부터 클라이언트 디코드까지이며 대기와 경로상 제공자 비용을 포함한다. 60초 warmup 뒤 900초 측정에서 원 E07 최소 표본인 회당 10,000 attempts와 각 query class 1,000을 적용한다. 회별 nearest-rank SLO에서 거절·timeout·미완료는 기한 위반으로 계산한다. 성공 응답 지연과 성공률은 별도로 보고한다. 외부 경합·입력 위반으로 무효인 회차와 제품 실패를 기존 판정 규칙으로 구분하고 원 terminal을 보존한다. [E07 운영 조건](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles) · [The Tail at Scale](https://research.google/pubs/the-tail-at-scale/)

### Native LanceDB ANN 검증

- 독립 전수 oracle로 `0 < 적격 비율 ≤1%`, `1% 초과~10%`, `10% 초과`와 빈 적격 집합을 각각 시험한다. 필터와 벡터 거리가 상관된 경우, 같은 owner의 다중 벡터, 9개 이상 region에서 centroid 상위 8개 밖에 정답이 있는 경우를 포함한다. exact 경로의 oracle 일치, 선택 region의 exact 결과, 선택 region의 ANN 결과를 대조해 region 선택 손실과 내부 탐색 손실을 분리한다. [현재 region 선택](../../../crates/quanta-index-semantic/src/native/embedding/ann_query.rs#L156)
- 같은 recipe·seed·정렬 입력의 region serial/병렬 구축에서 region seal과 재오픈 순위를 대조한다. old pin을 든 채 DELTA/delete·병합·재시작하고 해제 후 원 reclaim 기한을 확인한다. region descriptor 스캔·open I/O, graph 방문·재채점, old/new region 공존과 병합 부채를 읽기·파일·RSS·공간 상한에 모두 포함한다. [기존 ANN 수명과 원4K](../oct-22-finalizing/E04-semantic-ann.md#original-4k-gate) · [Qdrant Optimizer](https://qdrant.tech/documentation/ops-optimization/optimizer/)

### 독립 홀드아웃 실행과 판정

표본 규모는 독립 파일럿의 쌍별 분산·저장소 상관으로 산정한다. 최소 답변 가능 **100건·저장소 12개**, 저장소 층별 **2개**, 식별자·hard negative 및 무정답은 각각 **30건·저장소 4개**를 별도 확보한다. 사전 산정 규모가 더 크면 그 값을 적용한다. 지표의 점 추정과 신뢰구간에는 같은 저장소 내 query family 평균·저장소 동등 가중을 적용한다. 층별 10,000회 복원추출과 canonical seed hash를 기록한다. [기존 cluster bootstrap](../../../tools/benchmark/retrieval/evaluator.py#L3308)

N3/N4/N5는 DEV에 사용한다. 기존 split 검증으로 저장소·fork/유사 소스·query family 중복을 검사한다. EVAL 실행 전에 저장소·수정 전 commit·파일 universe digest·사례·분모·모델·후보 깊이·랭커·scorer 버전을 동결한다. 상위 K 파일 합집합은 검색 경로와 순위를 가리고 판정하고 qrels를 같은 파일 universe에 결속한다. MRR은 상위 10개 판정 완료가 필요하다. Recall은 독립 source 심사와 후보 합집합에서 확인한 양성 범위를 분모로 표기한다. EVAL 결과로 후보를 조정하면 다음 승격에는 새 독립 EVAL을 사용한다. [기존 split 검증](../../../tools/benchmark/corpus_binding.py#L352) · [NIST qrels](https://trec.nist.gov/data/reljudge_eng.html)

동일 Native SDK·바이너리·모드·소켓·루트·view에 lexical과 Hybrid를 결속한다. Semantic projection owner는 현재 contribution의 `source_owner`·source ref와 같은 view의 source map·record를 인증해 canonical source-file `OwnerKey`로 변환한다. source-free projection은 파일 후보에서 제외한다. 두 projection owner가 파일 하나를 가리키는 경우, 다른 source producer의 동일 경로, stale/foreign source ref를 회귀로 확인한다. 이 변환과 필터 적용 후 파일별 최고 벡터·중복 제거·후보 창을 순서대로 적용한다. [현재 source 결속 계약](../../../crates/quanta-index-contract/src/index.rs#L285) · [두 projection→한 source 회귀](../../../crates/quanta-index-semantic/src/native/embedding/ann_directory/source_reverse/behavior_tests.rs#L497)

실제 질의·적격 조건·정렬된 파일 후보 풀 해시를 캡처하고 Hybrid의 lexical 풀을 단독 lexical의 같은 깊이와 대조한다. 기준 RRF의 파일 신원·동점·순위를 독립 계산으로 재현한다. 두 풀에 없는 양성은 후보 문제, 풀에 있으나 융합 뒤 밀린 양성은 순위 문제로 분류한다. 경로명·저장소명으로 관련도를 보정하지 않는다. [기존 후보 검증 코드](../../../benchmarks/retrieval/src/file_profile.rs#L117)

기존 판정기에 Native SDK 캡처→qrels→쌍별 MRR/Recall·무정답→저장소 CI→최종 Native 판정을 연결한다. DEV에서 각 경로의 무정답 판정 규칙을 동결하고 오류·timeout·부분 결과에 abstain 성공 점수를 주지 않는다. 검증기는 누락된 질의, 바뀐 입력/경로와 미완료 qrels를 거절한다. 기존 `replayed_no_default_decision`은 재생 범위의 증거로 기록한다. [기존 재생 판정](../../../tools/benchmark/retrieval/decision.py#L682)

## 전환 및 퇴역 판정

Tantivy 우선 전환은 [실행 계획](TANTIVY-RETIREMENT.md)의 유지·퇴역 호출 목록, 설치·복구, 성능 및 전체 의존성 0건 조건으로 판정한다. 전체 삭제를 반영한 후보를 먼저 검증하고 동일 바이너리·설정으로 기본 전환한다. 코드 퇴역과 전체 Native 제품 수용은 별도 판정이다.

### Tantivy 우선 퇴역 T 단계

T는 Tantivy 제거 후보의 독립 판정이며 아래 통합 A/B와 구분한다. T의 완료에는 유지할 lexical·History·호출자 이전, 격리된 실제 설치 후보의 게시·검색·재시작·복구, 최종 후보의 SDK60·Large·독립 Hybrid 및 해당 기능의 기존 성능/회귀 기준이 필요하다. 전체 workspace·release 그래프·코드·lock·vendor·후보 번들에서 Tantivy 계열은 0건이어야 한다. Native 기본 호출과 Legacy 거절도 그 격리된 설치 후보에서 리허설한다. 전역 기본 선택의 E06 활성화는 통합 A 이후에만 수행한다.

Native LanceDB 전환은 기존 Semantic 오너가 맡는다. T에서는 Tantivy 의존성 제거와 그에 해당하는 제품 범위를 검증한다. 통합 A/B에서는 아래 Native 출하 기준을 적용한다. [최종 구조·실행 계획](TANTIVY-RETIREMENT.md#구조-감사와-최종-실행-계획--2026-11-15)

### 전체 Native 통합 A/B

| 결정 | 필요한 증거 |
| --- | --- |
| 로컬 기능 검증 | Native 상태 전이·SDK 응답·인덱스 품질의 실제 본문과 독립 oracle. 로컬 기능은 Linux 운영 수치와 따로 기록한다. |
| 통합 A 및 전환 허용 | 출하 예정 Native 전용 빌드에서 제품·상태 계약 리허설, SDK60·표준 Large, 원 E07 및 추가 게이트, LanceDB ANN·Native Hybrid, 아래 출하 그래프·번들·실행 경로를 모두 검증한다. 검증한 바이너리·설정을 고정한 채 기본 선택을 전환한다. |
| 통합 B 및 퇴역 완료 | 실제 기본 호출자의 Native 선택, 설치 제품 재시작·재생·복구, 해당 출하본의 hosted CI·API/module·전체 workspace 수용과 이전 루트의 보존/정리 상태를 확인한다. 미결 게시나 필수 검증이 남으면 완료를 보류한다. |

P06 SDK, P10 상태 이전, P11 정확한 저장소 쌍·배포·활성화·복귀의 기존 실행 권위를 사용한다. 결과에는 실제 daemon mode, 소켓/IPC 버전, 루트 marker, BOOT/게시 영수증과 바이너리 신원을 담는다. `serve-native` 명시 실행과 기본 호출자 검증은 각각 기록한다. 등록이 `staged`이거나 실제 설치 타깃·번들·독립 관측 근거가 빠진 범위는 **BLOCKED**다. [Proof authority](../../../tools/ci/proof-authority.toml#L21) · [E08 릴리스 입력](../oct-22-finalizing/E08-integration-release.md#release-inputs)

P07의 SDK·Runtime·공개 SDK/CLI Published와 설치 복구는 각 원 suite·입력·source의 실제 본문으로 수용한다. 현행 목록과 중복을 제거한 분모는 [F 실행 패킷](tickets/NC-F-integration.md#p07-final-actions-20261115)을 따른다. 컴파일·등록 외에 원 요청 oracle의 실제 본문이 필요하다. 공개 candidate·query/restart·late peak의 수용, [D의 서명 설치 복구](tickets/NC-D-boot.md#current-work-packets-20261115), 정식 P11은 별도 범위로 검증한다. 아래 제품 합격선을 적용한다.

## 최종 제거 및 증거 체크리스트

- [ ] 통합 A 전 `./scripts/cargow tree --locked --offline -p quanta-index-searchd-runtime --edges normal,build`와 출하 SDK/CLI 패키지의 동일 검사를 실제 release recipe의 target·features로 실행한다. `tantivy`·`tantivy-fst` 및 Tantivy 계열 패키지가 0건이어야 한다. 출하 데몬의 LanceDB 의존성을 Native Semantic까지 추적한다. 그래프를 실제 출하 바이너리·번들 해시에 결속하고 번들에서 별도 인덱스 실행 파일과 Legacy 모드 설정을 검사한다. 격리한 이전 릴리스·비교 도구는 별도 그래프로 확인한다.
- [x] Legacy lexical writer/reader·History, 공유 `tantivy-fst` matcher, `legacy-tantivy`/`legacy-bench`, 공용 manifest·lockfile·vendor patch를 현재 소스에서 제거했다. 전체 타깃 컴파일과 workspace 검사는 [06.3 원장](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md) 범위에서 통과했다. 이는 설치 후보의 실제 release/번들 판정이 아니다.
- [ ] Native 정규식·오류와 exact/ANN build·search·restore/reopen을 새 Native 루트에서 실제 실행한다. Native Catalog가 LanceDB 리전 파일셋을 관리하는지 확인하고 Legacy 경로 fallback 0건을 검증한다. 제거된 Legacy Text/File sidecar·History·writer/head/GC 및 semantic generation adapter가 출하 실행 경로에 없는지 대조한다. 공개 wire·module/API·벤치 기본값·CI inventory를 실제 사용 경로와 맞춘다. [Native LanceDB 세션](../../../crates/quanta-index-semantic/src/native/ann_session.rs)
- [ ] 전환 후 authoritative head와 발행·보존 writer가 각각 하나다. 이전 읽기·pin 및 승인된 rollback 보존 범위를 정리하고 외부 비교나 이전 릴리스의 보관을 운영 제품과 분리한다.
- [ ] 정식 수용은 source와 dirty 입력·toolchain·feature·모델·manifest·config·실행 바이너리 해시를 연결한다. canonical host admission과 해당 quiet-host 기준을 통과하고 실제 본문·종료값을 원 검증기로 확인한다. F2는 부모/자식 terminal에서 `case_exit=0`, `status=VERIFIED`, `product_verified=true`를 확인한다. 준비 완료·과거 pair·부분 실행·테스트 본문 0건은 합격 증거가 아니다.
- [ ] 통합 A → E06 기본값 전환 → 통합 B 순서가 실제 설치 제품에서 완료된다. `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`은 각각 기록하고, 필수 스코프에 `FAILED`·`BLOCKED`·`NOT_RUN`이 남아 있으면 퇴역 완료를 선언하지 않는다.
