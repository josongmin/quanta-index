# Native 증분 갱신 비용 · 2026-11-20

| 원장 | 남은 범위 |
| --- | --- |
| [INC-01 · 제품 코드와 소유 회귀](INC-01-source-closure.md#현재-남은-제품-코드--2026-11-26) | Runtime V1 핀·workspace·revision, Store warm prepare/commit, ItemIndex QE·Evidence·seal·V9가 남았다. Query exact-member의 C1 연결과 workspace 생성의 중복 순회 두 곳 제거는 합류했고 실행 검증이 남았다. |
| [INC-03 · 제품 검증](INC-03-product-acceptance.md#현행-판정--2026-11-26) | 기존 SDK·Watch에서 동일 operation의 Source CAS·Q ACK·G2, 독립 FULL parity, warm 전수 열거 0회와 무관 본문 0 B. |
| [E07 · 제품 성능](../../../plans/oct-22-finalizing/E07-performance-operations.md) | 기존 Large·XL·Mixed의 전체 증분 시간, 검색 지연, RSS와 용량을 판정한다. |

완료된 Facts 인증 조회, durable V2 게시, Store rollback 선택 조회와 Query 입장 계약은 [Native Source ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#native-source-commitment-and-selected-reads)에 기록한다. INC-02의 변경 블록 게시·봉인 및 로컬 회귀는 [Native Literal ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#native-literal-changed-directory-publication)에 기록한다.

## 실행 근거

- INC-01 소유 QBC는 C1 subset 수정 후 Kernel 9/9·Wire 1/1을 기록했다. 최신 C1 exact-member 조합은 수정 후 Wire 2/2·Kernel pre-key 9/9이며 모두 exit 0이다. Runtime workspace와 Store의 현행 본문 영수증은 대기 중이다. [소유 회귀 영수증](INC-01-source-closure.md#소유-회귀-영수증) · [SDK 실행 이력](INC-01-source-closure.md#남은-실행-검증)
- 11월 22일의 S `5dd6acd003b`·Q `4d2ee79a1`과 Q 변경분에서는 retrieval 가드 244개와 SQLite 19개 조건이 통과했다. Product selector 5개 해석은 성공했고 SDK 통합 QBC는 호스트 컴파일 잠금에서 exit 75였다.
- 11월 23일의 Q Kernel 첫 페이지 영속성·재개·fence는 2/2, 동결 Store V4 및 inventory 전이는 각각 1/1이었다. [Store·Wire 이력](INC-01-source-closure.md#코드-단위-현황과-검증) · [제품 실행 이력](INC-03-product-acceptance.md#2026-11-22-현행-제품-인수-상태)

구현 담당과 경계는 [E01 Source·Query](../../../plans/oct-22-finalizing/E01-producer-generation.md), [E02 게시·복구](../../../plans/oct-22-finalizing/E02-publication-recovery.md), [INC-01 잔여 표](INC-01-source-closure.md#현재-남은-제품-코드--2026-11-26)에서 확인한다.
