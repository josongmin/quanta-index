# 4K SourceAdvance 경로 인덱스 작업 패킷

**목표**는 4,096개 파일을 한 번의 `SourceAdvance`로 게시하고, 같은 제품 프로필의 100만 논리 읽기·1,200초 안에 오버레이 종료와 두 번째 재시작까지 검증하는 것이다. 원 ANN4K의 필터별 검색 품질도 같은 후보에서 확인한다. **07.5의 B는 이 채팅**(`01a1cbf6-46b6-7590-8550-c818539e9f2d`)이다. [실패 RCA](../../bugbash/nov-20/findings.md#L121) · [공식 채팅 배정표](../nov-20-native-cutover-closeout/tickets/LANES.md#11월-21일-작업-채팅-배정) · [기존 제품 수용 티켓 07.5](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#ann과-hybrid)

## 현재 인계점

초기 SourceAdvance의 4,096개 게시와 prefix 128개 완료를 확인했다. 이전 ANN4K는 1 PASS, 마지막 제품은 materialization watchdog에서 1 FAIL이다. Catalog 집중 회귀 7/7 이후 immutable operation 조회 기한을 연결했고 후기 trace에는 점유 전용 판독을 추가했다. 새 후기 Record 한 건은 admission 대기 중 디스크가 가득 차 본문 전에 종료됐다. [현재 상태와 코드 단위 검증](04-product-acceptance.md#현재-코드와-실행-판정). 02·03은 초기 게시 실패가 재현되고 경로 비용이 원인으로 확인될 때 다시 검토한다.

## 채팅별 실행권과 재사용 범위

2026-11-21 KST에 동일 체크아웃에서 최근 갱신된 비보관 사용자 채팅 17개(이 채팅 포함)의 최근 작업 본문, 공식 담당표, 기존 controller 상태와 Text Large terminal을 대조했다. 아래의 조건부 작업은 새 채팅 배정이나 착수 통지가 아니다.

| 실제 채팅 | 확인한 작업 | 이 패킷에서의 처리 |
| --- | --- | --- |
| **Native 전환 B · 자체 ANN·파일 신원** (이 채팅) | 07.5의 prefix·ANN4K·제품 4K와 독립 exact oracle. 확인 시 prefix·ANN의 기록 PID는 종료, 제품 continuation은 실행 중이고 제품 본문은 미시작 | **이 채팅에 남김.** 00의 실행기 판독·입력 결합, 01의 기존 대기 복구·계측 판독, 04의 두 본문 및 필요한 Semantic 수리를 한 큐가 소유한다. 동일 selector 또는 캐시 디렉터리를 인수해 실행하는 다른 채팅은 확인되지 않았다. |
| **Tantivy 퇴역 · Native Text SDK60 TDD** (`01a1d915-3719-76b0-8d1d-dcf3213cba72`) | 07.3·07.4 Caddy·etcd SDK와 Text Large. 동결 Main Large는 4,096파일·두 재시작 **1/1 PASS, 172.802초**. 후속 quiet Off·출하 후보 검증은 이 채팅 담당. [07.3·07.4 기록](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#sdk60와-large) | Large terminal을 slow PASS 판독 회귀에 재사용한다. 기존 TEXT 테스트를 07.5 큐에서 다시 실행하지 않는다. 02의 파일 간 배치나 03의 경로 블록 구현을 현재 맡아 진행 중이라는 증거는 없으며 둘 다 **보류**한다. 필요해지면 기존 TEXT 작성 경로 한 곳만 사용한다. |
| **Tantivy 퇴역 · Lexical 유지 회귀 이관** / **F15 세션 검증과 로컬 핀 보완** | NoFeatures의 파일 검증, scrub 격리, 세션·재시작과 기존 F15 증거. [04 유지 회귀](../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md) | F15 결과가 필요할 때 두 채팅의 기존 fixture·terminal을 인수한다. 같은 Lexical crate를 사용해도 이번 `PathWrites` 쓰기 배치나 ANN 제품 큐의 작업은 별도다. |
| **증분** 및 기존 Store·SDK·E02 채팅 | Source V2 변경 증명, 세대별 본문 CAS, DELTA 게시·ACK, 단일 편집 비용과 Q Literal seal. [증분 배정표](../../bugbash/nov-20-native-delta-cost/tickets/README.md) | 해당 DELTA·Literal 회귀는 기존 큐에 둔다. 01의 대상은 4,096개 파일을 최초 FULL로 게시하는 B operation이다. |
| **검색 벤치마크 지표 확인** / **07 Hybrid · 서명 설치 증거 소비 연결** | Root는 Native 전환·공유 후보 통합, Hybrid는 07.1·07.2의 서명 설치·캡처·RunStore 검증. [레인별 인계](../nov-20-native-cutover-closeout/tickets/LANES.md) | B가 두 본문과 공통 입력 결과를 기존 07.5에 한 번 기록하고 Root/F가 인수한다. 서명 설치·Hybrid 캡처·최종 출하 판정은 기존 담당에게 남긴다. |

**02·03 착수 경계.** 현재 Tree에는 빈 루트 패킹이 있고, `emit_merge`는 여전히 파일 단위로 잔여 배치를 제출한다. 01에서 현재 패킹으로 게시되면 04로 간다. scratch 또는 등록 객체 비용이 실제 실패를 설명할 때만 해당 패킷을 열고, 수정 시점에 기존 TEXT의 같은 함수·포맷 변경을 다시 대조한다. Catalog는 trace에서 설치·자식 검증 비용이 확인될 때만 해당 코드의 기존 담당이 관여한다. [파일별 flush](../../../crates/quanta-index-lexical/src/native/text_path.rs#L476) · [기존 패킹](../../../crates/quanta-index-lexical/src/native/text_path.rs#L575)

### 11월 22일 병렬 작업 전달

| 담당 | 전달한 작업 | 산출물을 받는 곳 |
| --- | --- | --- |
| **Tantivy 퇴역 · Native Text SDK60 TDD** | 경로 scratch 수집과 맵 설치를 현행 계측으로 구분 가능한지 확인한다. 빠진 필드가 있으면 Lexical 생산자에 필요한 계측을 보완하고 집중 회귀로 결과·오류·예산의 동등성을 검증한다. | 기존 TEXT 문서의 계측 위치·필드·회귀 결과를 [01](01-attribution.md)에서 소비한다. |
| **검색 벤치마크 지표 확인** | Root/F의 07.5 출하 증거 인수만 맡는다. 실제 reclaim, Legacy semantic fallback 0, 설치 신원의 기존 생산자와 증거 위치를 정리한다. | Root/F 원장에서 B의 두 테스트 결과를 받아 출하 증거와 결합한다. |
| **이 채팅 B** | 00·01·04의 실행기 판정, prefix, ANN4K·제품 4K, 공통 입력 대조를 소유한다. | B의 두 본문과 공통 입력 판정을 공식 07.5에 한 번 기록한다. |

TEXT와 Root/F의 기존 채팅으로 각각 한 번 전달했다. TEXT가 계측 확인에 착수한다고 응답했다. Root/F는 앱의 전송 성공까지 확인했다. 산출물 수용 여부는 각 담당의 본문과 원 결과로 판정한다.

## 착수 순서

**먼저 [00 · 최종 감사와 필수 작업](00-final-audit.md)을 읽는다.** 실행기에서 재현한 slow PASS 오판, 후속 실행기의 종료 상태 누락, 독립 retry 간 입력 대조 누락의 조치와 완료 기준이 있다. 01·04가 필수 경로이고 02·03은 측정 결과로 선택한다.

| 패킷 | 착수 조건과 담당 | 넘기는 결과 |
| --- | --- | --- |
| [01 · 실행 복구와 비용 분리](01-attribution.md) | 먼저 진행. B 실행기 담당, TEXT·Catalog는 필요한 계측만 보완 | 현재 패킹으로 게시되는지, 어떤 단계의 작업을 줄여야 하는지, 판단 가능한 trace 범위 |
| [02 · 파일 간 scratch 쓰기 배치](02-path-write-batching.md) | 보류. 01에서 파일별 scratch 제출 비용을 줄여야 함이 확인되면 기존 TEXT 작성자와 범위 대조 | 동일 Tree root·조회 결과, 배치 수·논리 읽기·비용의 전후 비교 |
| [03 · 경로 posting 블록](03-path-posting-blocks.md) | 보류. 01 또는 02 이후 경로 맵 설치·자식 검증이 여전히 실패를 설명할 때만 기존 TEXT 작성 경로에서 검토 | 원본 스캔과 일치하는 경로 reader/writer, 포맷 전환·재시작·비용 비교 |
| [04 · 제품·ANN 수용](04-product-acceptance.md) | 현행 패킹 또는 선택한 수정이 기존 실행기에 반영될 때. B 담당 | 동일 제품 본문과 ANN 본문의 각각의 결과, 공통 입력 대조, 기존 07.5 갱신 |

01에서 현행 패킹이 초기 게시를 통과하면 04로 간다. 전체 제품이 후속 단계에서 실패하면 그 단계에서 다시 원인을 좁힌다. 02와 03을 동시에 구현하면 비용 변화의 원인을 구별할 수 없다. 경로 밖에서 비용이 지배하면 해당 생산자에 넘기고 두 패킷은 보류한다. 최종 실행 결과는 기존 수용 티켓에 기록한다.

## 시작 시점의 증거 · 2026-11-21

| 범위 | 판정과 확인된 사실 |
| --- | --- |
| 제품 4K | **FAILED**. 초기 게시 전에 999,999회 사용 후 4회 읽기 청구가 거절됐다. 기한은 1,147.861초 남았다. 과거 다른 실행은 초기 게시까지 성공했다. [실패 구분](../../bugbash/nov-20/findings.md#L123) |
| 경로 Tree | 빈 루트용 nested packing은 이미 구현됐다. 최신 제품 입력에서의 효과는 **NOT_RUN**. 기존 구현을 02·03에서 다시 만들지 않는다. [패킹 선택](../../../crates/quanta-index-lexical/src/native/text_path.rs#L575) |
| ANN4K | 이전 후보의 6개 비어 있지 않은 구간은 각각 640/640이었다. 후속 LanceDB 실행은 `correlated_gt10pct`에서 raw 8/10, 파일 506/640으로 **FAILED**. 수정 후보의 전체 ANN4K는 **NOT_RUN**. [이전 결과](../nov-12-native-cutover/tickets/NC-B-semantic.md#L620) · [원본 로그 위치](04-product-acceptance.md) |
| 실행 복구 | prefix controller가 `ENOSPC`로 preflight와 상태 쓰기에 실패했다. 확인 당시 prefix·ANN 대기 PID는 종료됐고 prefix trace는 비어 있었다. 제품 continuation은 살아 있었다. 01에서 다시 확인한다. |
