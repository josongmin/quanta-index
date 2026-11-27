# 11월 20일 Native 전환 코드 작업과 잔여 티켓

**11월 23일 코드 재감사.** Q ANN 코덱 합류와 derive 정리는 끝났다. 이번 S/Q 출하에서 RepoMap을 제거하고 신규 DELTA 최적화는 보류한다. 이전 `Required` 복구, Source-only 절체와 새 후보의 유지 계약은 [RepoMap 퇴역 계획](../../nov-23-repomap-retirement/README.md)의 기준을 사용한다. 기존 A/Store의 V6 잔여 코드는 독립 FULL 발행·빈 registry 복구 및 memo CAS 실물 회수다. H, R25, CURRENT, 정식 128건과 P09–P11은 각 설치와 실행 결과를 기존 큐에서 인수한다. [파일별 경계와 실행 상태](LANES.md#코드별-소유와-수용)

같은 Q/S 후보의 대체 본문 **126개**와 유지 old-Q **2개**, 출하 번들의 Tantivy **0**, 같은 P10 ELF의 P11 **2건**, 서명 Linux 복구 **3건**, SDK60·Large·ANN4K/whole SDK·Hybrid의 제품 실측을 기존 실행에 연결한다. 이전 동결본의 PASS와 진행 중인 요청은 각 원 입력에 귀속한다. [이관 실행](04-maintained-test-migration.md) · [퇴역 판정](06-tantivy-retirement.md) · [제품 수용](07-product-qualification.md)

11월 22일의 예비 퇴역 분류와 이전 큐 상태는 이력으로 보관한다. 현재 판정은 아래 티켓과 실행 원장을 따른다. [원문 복구](../../../ARCHIVE-INDEX.md#2026-11-23-native-closeout-history-compaction)

병렬 작성자의 경계·인계·종료 조건은 [레인별 작업 계획](LANES.md#현행-구현과-수용-순서)에서 관리한다. 공용 Cargo/lock·recipe·test/proof authority는 Root/F 한 작성자가 합류하고, 실제 실행은 기존 단일 큐에서 진행한다. 각 항목의 원 증거는 [종료 표](LANES.md#티켓별-종료-표)를 참조한다.

## 기존 오너 인수와 실행

| ID | 남은 검증·인수 | 작성과 실행 |
| --- | --- | --- |
| [01](01-q-regressions.md) | derive 정책 **0건 · exit 0**. Catalog 저장 코덱·영수증 **2/2 VERIFIED**, Runtime 물리 프로파일·실제 데몬 설정 거절·재열기 **3/3 VERIFIED**. 후기 제품 4K **0/1 FAIL**(owner work 1,200초), Linux FUSE **BLOCKED**. 이전 동결 NoFeatures **26/26**, Lexical·UDS·Catalog **49/49** | Root/F와 각 파일 오너, 기존 단일 큐 |
| [02](02-semantica-runtime-sdk.md) | R25 **v34 검토용 제어** 인수. 동결 S/Q/GQLang·Cargo, 승인 Linux ELF와 **8 MiB SDK10** 본문이 남음 | 기존 S SDK/Runtime 오너, Root 결과 인수 |
| [03](03-signed-published-cp2.md) | H 로컬은 이전 Q pin에서 production **exit 101**. 수정된 paired 후보를 확보했고 production·CLI·두 replay/DATA 타깃 결과는 대기 중이다. | [03.7](03-signed-published-cp2.md#history-03-7) 서명 History 보류. D3 핀 **3/3**, 새 서명 설치 **BLOCKED**, 실제 공개 SDK/CLI·CP2·복구는 기존 03에서 인수한다. |
| [04](04-maintained-test-migration.md) | **04.2 제품·runner 코드와 선택 본문 VERIFIED · 105/105**. 원 v5 104건과 Linux 파생 1건에 귀속한다. 재개 명령·기록 순서 검증과 제어 회귀 **51/51 PASS**를 확인했다. 고정 macOS old-Q SDK22 **22/22**와 Source·Symbol **2/2**, 동결 Q의 Lexical Boot **5/5**, old-Q F15 **3/3**, Native FileBody·G1 **8/8**을 인수했다. 출하 원장은 `NOT_RUN`이며 최종 clean Q의 대체 126개와 Linux old-Q 유지 2개를 수집한다. | [Runtime 이관 ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#completed-runtime-migration) · [Lexical 코드](04-maintained-test-migration.md#lexical-코드와-검증-범위) · [old-Q SDK 코드](04-maintained-test-migration.md#legacy-sdk-custody), Root/F 기존 큐 |
| [05](05-bench-callers-hosted-ci.md) | 기존 DSL gold **12/12**, 회귀 **15/32**, release CLI **1/1**. 실제 MCP 유지 호출의 대조, 최종 Linux 공식 DSL·유료 A/B·hosted가 남음 | 기존 Bench·caller·CI 오너, Root 공용 등록 |
| [06](06-tantivy-retirement.md) | 마지막 Q 소스·workspace 의존성 검사는 통과. 이관 증거 부족으로 전체 **exit 1**. 06.4의 실제 release compiler·번들 Tantivy 0과 06.5 설치 복구가 남음 | Root/F 단일 합류, 기존 S와 D 공급 |
| [07](07-product-qualification.md) | 로컬 Lexical 다세대·재시작 회귀 통과. 최종 SDK60·Large·Off/Mixed와 기준본 비교, 수정 후보의 Product4K·ANN, 서명 Hybrid·독립 평가·E07이 남음 | 기존 TEXT/B/D와 F 인수·검증 |
| [08](08-release-activation.md) | P11 발급 수리 합류, 정식 선행 조건 **20건**, 동일 P10 ELF의 P11 **0/2**. 격리 후보 T·통합 A → E06 전환 → 통합 B·운영 P11 | 기존 E06·E08·P11 오너, F 증거 인수 |
| [09](09-native-search-gaps.md) | 기본 `.text()`·CLI `code-search`의 Native 연결과 후보가 제한된 고빈도 본문 검색이 구현됐다. 4,097파일 Layered·Tree 공개 SDK·CLI **2/2 VERIFIED**. 최종 후보의 공개 검색 골든·취소·비용 검증은 09.1·09.2의 08.1 전환 게이트다. Zoekt 비교 조건과 페이지 순회 비용은 09.3·09.4에서 추적한다. | 기존 SDK·caller·searchctl·TEXT·Bench 오너, 07 결정 기록, F 인수 |
| [10](10-native-ide-agent-gaps.md) | P1 IDE·에이전트 공백. 원문 매치 위치 커서, 심볼 정의의 Native 대체 또는 퇴역, 오버레이의 `(repo, path)` 충돌 키. 실행 **NOT_RUN** | 기존 TEXT·SDK·Runtime 오너, Semantica 심볼 생산자 |

- **Root의 순서.** 01의 현행 Q 본문을 03의 paired 입력에 인계한다. 02의 프레임 수리·새 ELF·현행 SDK 결과는 기존 Semantica 오너에게서 인수한다. 06의 최종 후보를 07과 08에 넘긴다. CP2·History·WorkStorage·ANN의 추가 코드는 기존 오너가 첫 실패와 영향 경로를 확인해 해당 티켓에서만 수리한다.
- **확인된 재사용 범위.** B의 빈 결과 수리는 `9d72edb4`에서 UDS 2/2·CLI 1/1·Lexical 4/4를 통과했고 현재 작업 트리에 선별 합류했다. R56 동결 macOS release는 Tantivy 0, 양쪽 Record, Large 4,096파일과 두 OS 재시작을 통과했다. Runtime291은 Source25, D3 저널 1/1은 해당 순수 저널 본문 범위다. 현행 입력의 전체 통과 판정은 각 티켓에 둔다. [F 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md) · [TEXT 원장](../../nov-12-native-cutover/tickets/NC-TEXT-build-final.md)

제품 분모와 전환 조건은 [Native DoD](../../nov-12-native-cutover/DOD.md)와 [Tantivy 퇴역 계획](06-tantivy-retirement.md)을 따른다. 11월 20일 기록의 상태는 마지막 확인 시각 기준이며, 실행 전 현행 HEAD·관련 dirty hunk·원 오너·terminal을 원장에서 다시 확인한다.
