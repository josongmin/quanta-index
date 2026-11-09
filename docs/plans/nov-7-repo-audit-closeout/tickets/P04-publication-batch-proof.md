# P04 — 게시 배치의 daemon 계산 횟수

구현과 동작 회귀는 `VERIFIED`다. 같은 불변 요청의 검증을 게시·Pending 복구가
재사용하는 계약은 [ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md)에
합쳤다. 집중 회귀와 실제 SDK 2/2·8개 named crash cut의 범위는
[실행 기록](../../../ARCHIVE-INDEX.md#2026-11-08-november-audit-ticket-closeout)에 있다.

## 직접 계측 `VERIFIED`

2026-11-08에 `main@7a16f48a2`와 게시·복구 변경분을 담은 분리본에서
`./scripts/cargow`로 debug daemon을 빌드했다. 기존 SDK crash fixture를 사용해
실제 계산 함수에서 아래 값을 관측했다. probe는 분리본에만 적용했다.

| daemon 요청 | canonical wire digest | source-event payload 해시 | footprint |
| --- | ---: | ---: | ---: |
| 신규 게시 | 1 | 1 | 1 |
| `after_semantic_seal` 중단 직전 게시 | 1 | 1 | 1 |
| 재시작 뒤 같은 Pending 게시 복구 | 1 | 1 | 1 |

`p04_daemon_counts_new_publish_and_pending_recovery`는 복구 후 게시와 검색까지
통과했다. 선언 token을 유지한 body 변경, footprint 정책 불일치, 잘못된 batch shape,
semantic admission 거절에 대한 기존 Core·dispatcher 회귀도 4/4 통과했다.
계측 1건과 거절 회귀 4건을 포함해 P04 선택·실행·통과는 각각 5건이다.
