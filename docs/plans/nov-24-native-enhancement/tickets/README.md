# Native 코드 검색 보강 작업 티켓

[보강 계획](../readme.md)의 코드 저장소 최소 계약과 후속 기능을 **6개 에픽, 16개 작업**으로 나눈다. 이 디렉터리는 실행 범위와 인계 조건을 정의한다. 2026년 11월 25일 잔여 작업 대조는 Quanta Index `28784da26` 기반 작업 트리와 기존 담당 원장을 기준으로 한다. 집중 회귀는 각각의 실행 후보에 귀속한다. 새 오너나 별도 실행 큐를 만들 필요가 없는 항목은 기존 오너에게 인계한다. [코드 감사와 합격선](../readme.md#실행-순서와-증거-기록) · [기존 오너와 실행 순서](../../nov-20-native-cutover-closeout/tickets/LANES.md) · [현재 검색 공백 원장 09](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md)

## 에픽별 작업

| 에픽 | 티켓 | 우선순위와 소유 경계 | 착수·합류 | 코드와 검증 상태 |
| --- | --- | --- | --- | --- |
| **Q · 공개 검색** | [Q01 기본 CodeSearch](Q01-sdk-codesearch.md) | **P0**. SDK·CLI 기본 계약, 파일 결과 식별·대표 위치, Runtime 공개 진입점과 세션 수명. 기존 09.1·05. | Root/F가 작업 트리 변경과 기존 로컬 결과를 V03 후보에 합친다. | Q01 단독 구현 잔여 없음. 집중 40/41 후 수정한 CLI exact 1/1 통과. V03 공용 API·설치 수용은 `NOT_RUN`. [코드와 실행](Q01-sdk-codesearch.md#후속-수리와-로컬-검증) |
| | [Q02 검색 범위](Q02-query-scope.md) | **P0**. 인증된 Source의 repo·revision·path·lang 필터, top‑k 전 적용. 기존 09.1c. | 필터 음성 회귀는 즉시. 생산자 결속은 S01 인계 후. | repo·lang 사전 필터 구현. 생산자 범위·정확 뷰의 설치본 수용은 `NOT_RUN`. |
| | [Q03 regex 입장](Q03-regex-budget.md) | **P0**. 본문 predicate 후보·예산과 오류 계약. 기존 09.2·07. | 4,097파일 정답과 경계 fixture는 즉시. 목표 범위와 입장 정책은 09.2에 기록. | 512-read 제한 Literal 및 Unicode 메모리 집중 회귀 통과. 두 레이아웃 모두 4,097파일 Source 전수 검사를 통과했다. 선행 실행은 양쪽 모두 첫 SDK 검색 시간 초과로 0/2였다. 수리 뒤 LayeredV2·TreeV1 로컬 공개 SDK·CLI **2/2**, Lexical **14/14**, 혼합 Source **3/3** 통과. 승인 Linux 설치본은 V03. |
| **S · 저장소 수집·증분** | [S01 코퍼스·Watch 재대조](S01-corpus-reconciliation.md) | **P0**. Source 수집과 파일 집합·freshness 권위. | 즉시. 필터용 메타데이터를 Q02에, 최종 수용을 V03에 인계. | SDK 전체 집합 재대조 구현. Watch 유실·오프라인·HEAD·ignore 변경의 제품 수용은 `NOT_RUN`. |
| | [S02 한 파일 DELTA](S02-single-edit-delta.md) | **P0**. Semantica 생산자의 V2 발급·Store·ItemIndex 전체 작업 제거. 기존 INC‑01·03. | 기존 오너에서 즉시 진행. Q Text의 물리 비용은 P02 소관. | 준비된 C1의 V2 발급이 구현됐다. 발급 header·Store·ItemIndex의 전체 방문과 설치 SDK·Watch 비용 검증이 남았다. |
| **D · 스토리지·운영** | [D01 내구성·복구](D01-durable-recovery.md) | **P0**. 객체 설치→Catalog→ACK, 서명 설치본의 중단·재시도. 기존 03·08. | 장애 fixture와 복구 관측은 즉시. 정식 설치 증거는 Q/S·SQL 후보가 준비된 뒤 인계. | 원자 게시와 ACK 재정산 구현. 로컬 staged ACK 유실·강제 종료 복구 1/1 통과. 서명 설치본 복구 `NOT_RUN`. |
| | [D02 pin·scrub·재빌드](D02-pin-scrub-rebuild.md) | **P0 수용**. 읽기 pin 해제와 회수, 손상 범위 보고·원본 재빌드. | pin 오류·손상 fixture는 D01과 병렬. 공용 저장 계층을 바꾸면 D01과 순차 합류. | 희소 해제·경합 회귀 통과. 펌프·Layered 실제 데몬 파일 회수·watchdog 후속 3/3 통과. 명시적 뷰의 scrub 영속 진행·재시작과 실패·경합 CAS는 로컬에서 각각 1/1 통과했다. CAS는 안정 Q와 현행 Catalog 4파일의 격리 실행이며 작업트리 정식 합류가 남았다. 자동 전수 감사·FULL 안전 교체와 부하 중 60초 회수가 남았다. |
| **I · IDE·에이전트** | [I01 원문 매치 전수](I01-match-occurrences.md) | **P1**, 전체 매치를 전제로 한 편집에는 필수. 매치 단위 커서·Source 신원. | 독립 골든은 즉시. 공개 결과는 Q01·Q02의 신원 계약 인수 후. | Source 결속 커서와 공개 Runtime·SDK·CLI가 현행 작업 트리에 있다. 요청·응답 82, 세션 19. SDK codec **2/2**와 로컬 공개 전수 조회 **3/3** 통과. 서명 Linux 설치본은 `NOT_RUN`. |
| | [I02 심볼 정의](I02-symbol-definitions.md) | **P1**, 해당 기능 출하 시 필수. 기존 심볼 생산자와 정의 검색 소비자. | 기존 심볼 생산자·소비자 대조는 즉시. Q02·S01 메타데이터를 재사용. | 심볼 질의의 명시적 거절 회귀 확인. Native 정의 검색 구현 또는 계약 퇴역 결정이 남았다. |
| | [I03 미저장 오버레이](I03-overlay-identity.md) | **P1**, 해당 기능 출하 시 필수. repo별 오버레이, 저장 경합·재시작. | 두 repo fixture는 즉시. repo 신원과 공용 reader 패치는 Q02·V03과 합류. | repo·path 격리 및 Latest NFC/NFD 1/1 통과. Unicode 재시작과 공식 API·설치본 수용이 남았다. |
| **P · 비용·품질** | [P01 순위 페이지 비용](P01-ranked-pagination.md) | **P1**. 파일 순위·정확 건수와 고정 뷰 페이지 비용. | 계측·골든은 즉시. 공용 평가 루프는 Q03 뒤 합류. | main 작업 트리의 순위 재사용·A→B 교체·저예산 동등성 **4/4 통과**. 격리 후보의 제품 예산 검증 **1/1 통과**. 릴리스 규모 계측은 열린 상태다. |
| | [P02 Layered DELTA 비용](P02-layered-delta-cost.md) | **P1 계측**. Q 내부 루트·파티션·객체 재사용 비용. | S02와 병렬 계측. 객체 저장 계약 변경은 D01 소유. | 4 KiB 빈 루트와 두 segment 엔진 회귀 2/2 통과. FULL→PATCH·고정 뷰·재시작과 16개 레이어 compaction의 제품 프로세스 회귀 2/2 통과. 설치 물리 I/O는 `NOT_RUN`. |
| | [P03 식별자 랭킹](P03-identifier-ranking.md) | **P2**. 평가 입력, 랭킹·무정답 품질. 기존 평가 원장. | 독립 평가 준비는 즉시. 튜닝은 Q01·Q03의 골든 이후. | 구 순위 호환은 Q01에서 추적. 독립 식별자 품질 평가는 `NOT_RUN`. |
| **V · 검증·출하** | [V01 기존 회귀 이관](V01-maintained-regressions.md) | **P0**. 현행 04·05·06의 유지·퇴역 원장과 실행 결과 인수. | 원 테스트 대응표는 즉시. 구현·정책 변경 뒤 실제 대체 테스트를 기존 큐에서 실행. | 퇴역 정적 검사 통과 이력. 유지 회귀 적격 증거 0/128, migration 검사는 `FAILED` 이력. |
| | [V02 Zoekt 짝 비교](V02-zoekt-comparison.md) | **비교 작업**. 기준과 출하 필수 여부는 09.3a. | 동일 기능·정답·입력 준비는 즉시. 비교 실행은 정확성 수용과 후보 고정 후. | 고정 Zoekt로 etcd 1,421파일의 독립 정답 **4/4** 통과. Native 로컬 debug FULL은 SDK 60초 기한 초과. 재현·RCA는 기존 TEXT 07.3에서 수행한다. V02의 Linux 짝 정확성·성능은 `NOT_RUN`. |
| | [V03 단일 후보·서명 출하](V03-signed-release.md) | **P0**. Root/F의 공용 변경 합류, 같은 Q/S 후보의 출하 계약. 기존 03–08. | 입력과 기존 결과 인수는 즉시. 정식 실행은 P0 결과 합류 뒤 한 번 준비. | 정식 후보 입력과 서명 설치·활성화 수용이 남았다. 기존 집중 결과는 해당 후보 범위에 유지. |

## 병렬 착수와 충돌 경계

- **지금 병렬로 준비할 일.** Q는 공개 질의·필터 골든과 4,097파일 fixture를, S는 수집 대조와 기존 한 파일 비용 회귀를, D는 장애·회수 fixture를 준비한다. I/P는 독립 골든과 계측을, V는 이관 대응표와 비교 입력을 준비한다. 같은 제품 proof가 이미 실행 중이면 원 terminal과 입력을 인수한다.
- **Q 공용 경계.** 공개 SDK·CLI 조립은 Q01, typed 범위와 검증된 Source 필터는 Q02, lexical 후보·예산은 Q03이 맡는다. `text_query.rs` 평가 루프의 구조 변경은 Q03이 먼저 합류한 뒤 P01이 반영한다. I03은 오버레이 처리 블록, P02는 `text_segment`의 빌드 경계를 소유한다. 공용 `native.rs`·wire·SDK 요청형에서 패치가 겹치면 기존 Root/F가 단일 합류 담당이 된다.
- **S/D 공용 경계.** S01은 원본 파일 집합·신원과 재스캔, S02는 Source 지속화·증분 발급과 비용을 맡는다. D01은 객체의 내구 설치와 원자 게시, D02는 pin 해제·회수·scrub과 재빌드 수용을 맡는다. 같은 Catalog·ticket 경계를 바꿔야 하면 해당 오너가 변경을 합치고 두 회귀가 같은 후보를 사용한다.
- **기존 원장이 결정한다.** 남은 기본 검색 호환 계약과 4,096 후보 예산의 제품 수용은 09.1·09.2에, Zoekt 수용 기준은 09.3에 적는다. `rare common`만 있는 질의는 본문 source-predicate의 DF 가드를 검증하지 않으므로 Q03이 regex 또는 `file.contains`를 넣은 fixture로 기존 09.2 담당에게 인계한다. [09의 결정과 합격선](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md)

## 중복 제거한 잔여 작업

| 경계 | 기존 담당이 할 일 | 공유할 결과 |
| --- | --- | --- |
| **P0 · Q 공개 검색** | **Q02** 기존 담당은 S01의 인증 메타데이터와 repo·revision·path·lang 제약 및 커서의 결속을 맡는다. Top-k 전 적용과 복원된 Source의 필터 동작을 검증한다. | V03은 Q01 확장 frontdoor·실제 CLI 프로세스, **Q03** 로컬 4,097파일 두 레이아웃, I01 전수 매치의 기존 결과를 인수한다. 공용 wire·등록은 Root/F가 합친다. |
| **P0 · 수집·증분** | **S01** Source/Watch 담당은 이벤트 유실·오프라인 변경·HEAD/ignore 전환 뒤 독립 manifest로 재대조한다. **S02** 기존 INC-01/03 담당은 V2 발급 header·Store·ItemIndex의 전체 순회를 줄이고, 설치 SDK·Watch에서 한 파일 비용과 FULL 동등성을 검증한다. | 같은 Watch 실행의 최신성·파일 집합·비용은 한 번 수집한다. Q02는 Source 신원을, D01은 같은 operation 신원을 인수한다. Q 내부 DELTA 비용은 P02에 둔다. |
| **P0 · 내구성·운영** | **D01** 기존 03/08 담당은 서명 설치의 ACK 유실·중단·복구를 수용한다. **D02** Runtime·Catalog 담당은 durable 진행 상태를 자동 순회에 연결하고, 원본 FULL 검증·CAS 교체와 부하 중 60초 회수를 확인한다. | D01 로컬 강제 종료 1/1, D02 페이지 재개·격리·회수는 기존 결과를 사용한다. 같은 설치 시나리오의 핀·물리 회수는 D02/07.9에서 판정한다. |
| **P0 · 출하 검증** | **V01** 기존 04/05/06 담당은 적격 Linux 유지 128본문과 Tantivy 이관 증거를 채운다. **V03** Root/F·D/E08은 S/Q 재동결과 승인을 거쳐 서명 설치·복구·활성화를 결속한다. 현재 유지 본문 0/128, 활성 pin의 S/Q Source 경로 부재다. | RepoMap 05·P11, History/BOOT, SDK60·Large·ANN4K·Hybrid·E07은 각 원 담당 영수증을 같은 후보에서 인수한다. |
| **비교 · etcd/Zoekt** | **TEXT 07.3**만 Native etcd FULL 타임아웃을 기존 60초 입력으로 재현하고, 실패 시 게시 단계를 계측한다. **V02** 기존 Bench 담당은 같은 코퍼스·후보의 양쪽 파일 집합을 독립 정답과 대조한 뒤 Linux 짝 비용을 측정한다. | 로컬 Zoekt 4/4와 Native debug 기한 초과 기록을 인수한다. 출하 기준 적용은 09.3a 결정에 따른다. |
| **해당 기능 출하 시 · IDE** | **I01** 공개 전수 매치 로컬 3/3을 승인 후보에 결속한다. **I02** 기존 심볼 생산자·TEXT 담당은 정의 검색 제공 여부를 결정한 뒤 구현 또는 호출자 이관을 맡는다. **I03** Runtime overlay 담당은 Unicode 재시작·rename/delete·저장 경합을 공식 API에서 수용한다. | I01의 기존 전수 매치와 I03의 Latest NFC/NFD 양성은 원 실행을 인수한다. |
| **P1/P2 · 비용·품질** | **P01** 기존 TEXT 담당은 페이지 재사용의 릴리스 규모 지연·메모리와 예산 fallback을 확인한다. **P02** Q TEXT는 Layered DELTA 루트·포스팅·compaction의 쓰기·sync·피크 디스크를 잰다. **P03** 기존 F-EVAL은 독립 식별자 정답·무정답으로 품질을 측정한다. | 효과를 측정하는 데 필요한 입력과 계측을 P02·S02, P03·V02에서 공유한다. 승인 입력·바이너리·선택자·오라클이 같은 본문은 기존 큐에서 한 번 실행한다. |

## 합류·완료 판정

1. **최소 기능 수용**은 [필수 계약 10개](../readme.md#코드-저장소-최소-동작-체크리스트)에 결속한다. 수집·재대조는 S01, 최초 FULL·디스크 재오픈과 ACK·복구는 D01/V03, 증분은 S02/D01, 검색·필터·대표 결과는 Q01–Q03, 보존·회수와 손상 복구는 D02가 담당한다. 비용은 P01/P02와 기존 V03 프로필에 인계한다.
2. **P0 합류**는 Q01–Q03, S01–S02, D01–D02의 결정·실행과 V01의 적격 증거를 V03에 모은다. Source 메타데이터 인계, Q/S·Cargo·SQL·서명 실행본은 기존 조립 담당이 한 후보로 결속한다. I01–I03은 해당 IDE 기능을 출하할 때 그 기능의 필수 계약으로 적용한다. P01–P03의 개선 실측과 V02의 상대 비교는 기존 출하 조건 및 09.3 결정에 따라 인수한다.
3. **최종 기록**은 변경 범위·원래 오너·공개 호출자·selector·실제 실행 결과를 포함한다. `VERIFIED`는 해당 입력과 실행으로 입증한 범위에 붙인다. 실행이 없거나 입장에 막힌 경우는 각각 `NOT_RUN`·`BLOCKED`로 유지한다. [전환 DoD](../../nov-12-native-cutover/DOD.md) · [제품 검증](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md) · [활성화](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)
