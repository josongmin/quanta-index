# I02 · Native 심볼 정의 탐색

우선순위 **P1**. 정의 탐색을 약속하는 제품 경로의 필수 기능이다.

## 현황

구 Quanta에는 별도의 심볼 경로가 있고 현행 Native Text는 `symbol.has.name`을 거절한다. [10.2 원장](../../nov-20-native-cutover-closeout/tickets/10-native-ide-agent-gaps.md#102--심볼-정의-검색의-native-대체-또는-퇴역)은 사용자 결정 전까지 **잠정 (2) 명시적 거절 유지**다. D3 레인은 비어 있지 않은 뷰에서 SDK 사전 거절·raw 데몬 거절과 세션 지속을 실제 데몬 테스트로 `VERIFIED`했다. 현재 [SDK 오류 분류](../../../../crates/quanta-index-sdk/src/native_uds.rs#L3627)에는 `UnsupportedCombo` 수리가 보이나, 그 변경의 최종 동일 후보 수용과 퇴역 결정은 별개다. [구 Symbol 라우트](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs#L538) · [거절 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/symbol_refusal.rs)

## 작업 범위

- [10.2a](../../nov-20-native-cutover-closeout/tickets/10-native-ide-agent-gaps.md#102--심볼-정의-검색의-native-대체-또는-퇴역)에서 Native 정의 검색 제공 또는 의도적 퇴역을 결정한다. 제공한다면 기존 구조·심볼 생산자가 검증한 정의 이름·종류·범위와 Source 신원을 인수하고 repo·revision에 결속된 공개 조회를 연결한다. 퇴역한다면 공개 거절·대체 경로와 영향받는 호출자를 04/05/06에 기록한다.
- 일반 문자열 매치와 심볼 정의를 구별한다. 의미 기반 참조·rename은 기존 분석 경로의 심볼 해석을 사용한다.

## 완료 조건

- [ ] 제공을 택하면 동명 심볼·다른 namespace 또는 repo·정의가 삭제된 DELTA에서 심볼 식별과 원문 범위가 고정 골든과 일치한다.
- [ ] 실제 SDK·CLI 또는 기존 소비자가 결정된 경로를 호출한다. 제공 시 메타데이터가 없는 언어·뷰의 정책을 확인하고, 퇴역 시 명시적 거절·안내·대체 경로와 04/05/06 이관을 확인한다.

## 의존·인계

생산자 권위와 기존 소비자 목록은 바로 조사한다. [S01](S01-corpus-reconciliation.md)의 Source 정책과 [Q02](Q02-query-scope.md)의 범위 필터를 재사용한다. 기존 구조 검색 오너와 중복 엔진을 만들지 않도록 05의 실제 호출에 연결하고 공용 변경은 [V03](V03-signed-release.md)에 인계한다.
