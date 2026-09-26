# RBR-05 — Symbol route와 공통 published-unit 증명

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현: `published_units.rs` typed registry, SDK `symbol` route와 `record.rs::prove_hit`의 published-unit 증명 경로 확인. `prove_hit`의 **scored byte span은 full-line projection**이므로 indexed span 품질은 RBR-06 미해결.
- 검증: 과거 live SDK 기록은 현 source의 forged/stale/timeout/no-answer·동명이인 proof가 아니다. 현재 SDK rail/receipt는 `NOT_RUN`.
- 잔여: RBR-06 span 스키마와 맞춰 producer→validator를 고정하고 현 daemon route·negative fixtures를 재실행한다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. typed registry·symbol route 코드 관측; 현 소스 SDK proof 미발급. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/02/04.
- 성격: 심볼 기능을 실제 평가까지 연결하는 필수 작업.

## 파일·함수

- [main.rs](../../../../benchmarks/retrieval/src/main.rs): `KNOWN_ROUTES`, route/profile admission.
- [sdk.rs](../../../../benchmarks/retrieval/src/sdk.rs): `query_route`, `RankedHit`/outcome conversion, route inventory.
- [record.rs](../../../../benchmarks/retrieval/src/record.rs): `prove_hit`, record assembly.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs), `tests/sdk_roundtrip.rs`.
- 신설 `benchmarks/retrieval/src/published_units.rs` 또는 같은 역할의 단일 registry module.
- 기존 SDK [SymbolQueryBuilder](../../../../crates/quanta-index-sdk/src/symbol.rs) 재사용.

## 작업

1. chunk/symbol의 typed published-unit registry를 만든다. ID, path, original source SHA, source span, producer identity가 원본 authority다. symbol ID를 chunk ID로 위장하지 않는다.
2. `symbol` route를 public SDK로 실행하고 generation binding·request budget·typed failure 규칙을 기존 route와 동일하게 적용한다. RBR-02의 명시적 input policy를 사용한다.
3. `SymbolCandidate`를 증명 가능한 결과로 변환한다. snippet 문자열은 reference 이름일 수 있으므로 원문 definition bytes인 척하지 않는다. returned span과 published authority를 대조한다.
4. `prove_hit`는 종류별 published unit을 검증한 뒤 같은 source-byte/context accounting으로 record를 만든다. unanchored 응답은 종류별 authority가 증명할 때만 허용한다.
5. symbol route와 chunk route를 별도 결과로 보고한다. gold span으로 결과를 잘라 precision을 인위적으로 높이지 않는다. 라인 확장 평가 span은 RBR-06과 동일하게 표시한다.
6. Python route schema는 이미 nonempty string을 허용하므로 불필요한 enum/schema fork를 만들지 않는다. 실제 Rust allowlist, capture provenance, validators 및 inventory만 필요한 범위에서 수정한다.

## 테스트·합격

- 실제 daemon publish/query/record/replay에서 symbol route 성공과 chunk routes 보존.
- missing registry entry, chunk/symbol collision, forged ID, wrong path/SHA/span, stale generation 거부.
- 동명이인, namespaced symbol, no-answer, deadline/transport/provider errors, partial window 처리.
- source bytes에서 context/token count를 독립 재계산; snippet이나 gold로 대체하지 않음.
- query route 추가가 cold/warmup/measured schedule 및 result matrix를 누락시키지 않음.

공통 [TEST-PLAN](TEST-PLAN.md)의 contract + SDK 적용. 기능 연결만으로 exact-name ranking 개선을 주장하지 않는다.
