# E03 — Lexical native incremental·초기 구축

상태는 [SSOT E03](README.md#e03). 원 요구 R14–R17, 종전 W2-11과 기존 lexical 구조/초기 구축 잔여를 소유한다.

## Implementation boundary

기존 NativeText/paged source/postings·COW map·literal/token/BM25/regex planner·bounded query·SDK route를 재사용한다. 기존 Tantivy/Lance를 다른 이름으로 감싸거나 엔진을 다시 만들지 않는다. 지원 capability는 actual route/typed refusal inventory로 정하고 source 정의만으로 parity를 판정하지 않는다.

고정 delta/no-op에 관계없는 전체 corpus extraction/hash/inventory/seal 회계가 남아 있는지 caller→storage까지 비용을 추적한다. 확인된 비용만 producer/owner에서 수리한다. 초기 bulk preparation·sort/spill·group commit·bounded compaction 실험은 기존 durability/GC 경계를 유지하고 동일 입력으로 검증한다.

## DoD

- [ ] current/held-old view의 byte-literal·필수 token/BM25/regex/filter/ranking을 독립 expected gold로 수용. 미지원은 정확한 capability 거절.
- [ ] changed file의 전체 contribution을 포함해 rename/delete/recreate/no-op·통계/visibility·동일 위치 term 교체의 증분 결과가 cold build와 일치.
- [ ] full bootstrap·warm reopen·journal continuity·gap catch-up/Resync가 실제 daemon에서 완결.
- [ ] 고정 D/K를 두고 N/H/M을 독립 증가시켜 [RFC 12절](../../rfc/oct-10-native-incremental-vnext.md#12-structural-and-sustainable-performance-contracts)의 CPU/bytes/objects/catalog/allocation bounds를 실제 비용으로 검증.
- [ ] corpus-sized Vec/inline manifest·숨은 전수 scan·매 tiny delta 즉시 전역 compaction 없이 bounded construction/scratch와 지속 churn/debt 수용.
- [ ] 원 profile의 파일/s·MiB/s·증분 latency와 실제 제품 gold를 E07에 전달. elapsed만 낮아지고 실패/누락이 늘어난 결과는 거절.

성능 실험의 계수·하드웨어·profile은 실행 전에 고정하고 구현과 독립된 gold를 유지한다. 원 lexical/ADR 사양은 [RFC 9절](../../rfc/oct-10-native-incremental-vnext.md#9-concrete-index-work-packages)와 [보존 lexical 계약](../../history/oct-10-index-closeout/LEXICAL-STORE-CONTRACT.md)을 따른다. 기존 구현 완료와 전체 운영/성능 수용을 구분한다.
