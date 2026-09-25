# RBR-08 — 근거 기반 exact-name 심볼 ranking

- 우선순위: P2. 실험/조건부 구현: `NOT_RUN`. 선행: RBR-05/06.
- 진입 조건: source-bound symbol route에서 정답 후보가 존재하지만 부분일치/참조/동명이인보다 낮게 배치되는 재현 사례.

## 파일·함수

- [lexical/adapter.rs](../../../../crates/quanta-index-lexical/src/adapter.rs): `UpsertSymbol`와 `ReplaceLexicalScope` 두 생산 경로.
- [schema.rs](../../../../crates/quanta-index-lexical/src/schema.rs), `lib.rs::SchemaFields`, [documents.rs](../../../../crates/quanta-index-lexical/src/documents.rs) `add_symbol_fields`.
- [searcher/port.rs](../../../../crates/quanta-index-lexical/src/searcher/port.rs): `search_symbols_constrained`; compile/restrictions.
- [searcher/paging.rs](../../../../crates/quanta-index-lexical/src/searcher/paging.rs): `collect_ranked_page`, cursor/order 계약.
- owning lexical fixtures, `tests/tantivy_smoke.rs`.

## 유한 후보와 작업

1. baseline과 exact local-name/qualified-name 우선 정책 한 가지를 비교한다. definition intent가 명시된 symbol route에 한정한다. generic lexical이나 모든 NL 입력에 무조건 적용하지 않는다.
2. exact-name 필드가 필요하면 canonical field를 schema에 추가하고 **두 symbol ingest 경로 모두** 같은 값으로 채운다. normalization/case/qualified separator 규칙을 명시한다.
3. 기존 on-disk index의 schema mismatch/rebuild 처리와 generation/cursor 호환성 거부를 함께 구현한다. 옛 인덱스를 새 필드가 있는 것처럼 읽지 않는다.
4. ranking은 pagination 전에 적용한다. 페이지 수집 후 재정렬이나 서로 다른 route의 score를 임의 비교하지 않는다.
5. quality gain이 없거나 contract/latency 손실이 있으면 기본 정책을 유지한다. fusion weight나 범용 BM25 parameter까지 동시에 조정하지 않는다.

## 테스트·합격

- exact vs token-partial, local vs container-only, 동일 이름 다른 namespace, qualified name, Unicode/case, definitions vs references.
- stable ties, repeated runs, 여러 페이지 연결 시 중복/누락 없음, wrong-generation/config cursor 거부.
- insert/upsert/replace/reopen이 동일 정렬을 제공하며 legacy schema는 typed refusal/rebuild.
- 독립 fixtures 전부 통과 + [TEST-PLAN](TEST-PLAN.md)의 quality gate를 만족해야 기본 정책 변경.
- 진입 조건이 없으면 trace/실험 근거와 함께 유지 결정으로 종료한다. 실행하지 않은 ranker를 구현 완료로 쓰지 않는다.
