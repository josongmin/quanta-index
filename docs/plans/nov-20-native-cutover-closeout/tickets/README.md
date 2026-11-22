# 11월 20일 Native 전환 코드 작업과 잔여 티켓

**11월 22일 정적 수리와 일괄 입장.** 삭제 Lexical 원 ID 91개 중 옛 adapter point/full 비교 1개를 퇴역했고 현재 후보에서 유지 이관 90개의 실제 출하 증거를 받아야 한다. DATA·FileBody 핵심 경로와 CURRENT 재개 검사를 수리하고 실제 공개 SDK·CLI 선택자를 확인했다. 구 Root PID 50363은 입장 전 exit 143, 본문 0/26이다. 신규 런타임은 현행 후보를 고정한 뒤 기존 Root 한 큐로 실행한다. [Root 일괄 실행](LANES.md#정적-수리-후-일괄-실행--11월-22일) · [출하 검증](06-tantivy-retirement.md)

**11월 22일 02:34 KST.** 삭제 Lexical 91개를 엔진 표현 퇴역 8개, 유지 계약 이관 83개로 분리했다. 이관은 DATA format-2 1개 **VERIFIED**, 82개 결과 대기다. Tantivy workspace 잔존 0과 검사기 회귀 8건은 통과했다. [04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**11월 22일 ss 후속 실행.** R5·Lexical/TEXT·Runtime·R25·CURRENT·P10/P11의 기존 담당 채팅 8곳에 후속 범위를 전달했다. Root 공용 합류와 PID 50363, 실제 실행은 기존 단일 큐를 사용한다. [지정 범위와 인수 조건](LANES.md#11월-22일-ss-후속-배정)

**11월 22일 01:42 KST 코드 검증.** Runtime·Lexical·Harness의 전체 타깃/전체 feature 오프라인 컴파일과 `just rust-check-tantivy-retirement`가 **VERIFIED · exit 0**이다. Cargo 그래프·lock·소스·vendor의 Tantivy 잔여는 **0**이고, 삭제한 Lexical 엔진 테스트 91건은 승인된 퇴역으로 판정했다. 별도 `cargo deny`는 의존성·manifest 정책 오류로 **FAILED · exit 3**이다. 실제 Linux release·서명 설치·rollback/restore-forward는 **NOT_RUN**이다. 기존 Root PID **50363**은 01:45 KST 조회 시 살아 있고 상태 파일은 `WAITING_Q_ADMISSION`, 단계 결과 **0/26**이다. [최신 레인 계획](LANES.md#지금-병렬로-준비할-작업) · [Tantivy](06-tantivy-retirement.md) · [Root](LANES.md#root-단일-실행-큐)

병렬 작성자의 경계·시작 파일·인계·종료 조건은 [레인별 작업 계획](LANES.md#지금-병렬로-준비할-작업)이 관리한다. 공용 Cargo/lock·recipe·test/proof authority는 Root/F 한 작성자가 합류하고, 실제 실행은 기존 단일 큐에서 진행한다. 각 항목의 원 증거는 [종료 표](LANES.md#티켓별-종료-표)를 참조한다.

## 기존 오너 인수와 실행

| ID | 남은 검증·인수 | 작성과 실행 |
| --- | --- | --- |
| [01](01-q-regressions.md) | 캐시 결속 제어 **VERIFIED**. 현행 Q 27+3·Catalog 19와 26 유지 본문 **NOT_RUN** | Root/F, 기존 단일 큐 |
| [02](02-semantica-runtime-sdk.md) | 승인 전 공급 lease 제어 **VERIFIED**. R25 후속 ELF·8 MiB Native 2·TS/JS·Java 및 WorkStorage 본문 | 기존 S SDK/Runtime 오너, Root 결과 인수 |
| [03](03-signed-published-cp2.md) | 기존 로컬 후속의 용량 입장과 실제 서명 설치·공개 SDK/CLI·CP2 원 결과 | Root/F 실행, 기존 D/H/S 공급 |
| [04](04-maintained-test-migration.md) | DATA Source 방문 종료 검증 코드 반영. 현행 old-Q 장애 4개와 F15 Native symlink 등 유지 본문 수용 | Root/F 실행, 기존 작성자 수리 |
| [05](05-bench-callers-hosted-ci.md) | 외부 프로세스 A/B 이관 결과 인수, 실제 A/B 계측 수용, Native DSL 집중 본문·공식 Linux warm/cold 및 승인 old-Q hosted 공급 | 기존 Bench·caller·CI 오너, Root 공용 등록 |
| [06](06-tantivy-retirement.md) | Tantivy 소스·workspace 의존성 검사 통과. SCV2 독립 V9 ACK 코드 결선. 유지 이관 90개 출하 증거, release compiler·서명 번들과 P10/P11 수용 | Root/F 단일 합류, 기존 S와 D 수리 |
| [07](07-product-qualification.md) | 최종 SDK60·Large, LanceDB ANN, signed Hybrid·독립 평가·E07 | 기존 TEXT/B/D와 F 인수·검증 |
| [08](08-release-activation.md) | 격리 후보 T, 통합 A → E06 전환 → 통합 B·정식 P11 | 기존 E06·E08·P11 오너, F 증거 인수 |

- **Root의 순서.** 01의 현행 Q 본문을 03의 paired 입력에 인계한다. 02의 프레임 수리·새 ELF·현행 SDK 결과는 기존 Semantica 오너에게서 인수한다. 06의 최종 후보를 07과 08에 넘긴다. CP2·History·WorkStorage·ANN의 추가 코드는 기존 오너가 첫 실패와 영향 경로를 확인해 해당 티켓에서만 수리한다.
- **확인된 재사용 범위.** B의 빈 결과 수리는 `9d72edb4`에서 UDS 2/2·CLI 1/1·Lexical 4/4를 통과했고 현재 작업 트리에 선별 합류했다. R56 동결 macOS release는 Tantivy 0, 양쪽 Record, Large 4,096파일과 두 OS 재시작을 통과했다. Runtime291은 Source25, D3 저널 1/1은 해당 순수 저널 본문 범위다. 현행 입력의 전체 통과 판정은 각 티켓에 둔다. [F 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md) · [TEXT 원장](../../nov-12-native-cutover/tickets/NC-TEXT-build-final.md)

제품 분모와 전환 조건은 [Native DoD](../../nov-12-native-cutover/DOD.md)와 [Tantivy 퇴역 계획](../../nov-12-native-cutover/TANTIVY-RETIREMENT.md)을 따른다. 11월 20일 기록의 상태는 마지막 확인 시각 기준이며, 실행 전 현행 HEAD·관련 dirty hunk·원 오너·terminal을 원장에서 다시 확인한다.
