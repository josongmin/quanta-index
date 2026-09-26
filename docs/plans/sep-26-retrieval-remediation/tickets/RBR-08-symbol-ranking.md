# RBR-08 — 근거 기반 exact-name 심볼 ranking

## 현행 판정 — 2026-09-26, `af640562` + dirty overlay

- 구현/검증: `symbol` route는 있으나 source-bound 오순위 진입 사례, exact-name ranker 변경, 영향받은 lexical/page/reopen proof는 `NOT_RUN`. 제품 기본값 변경도 없다.
- 잔여 결정: 정의 정답이 후보에 **있는데도** 참조/부분일치/동명이인 아래 놓이는 독립 사례를 먼저 수집한다. 사례가 없으면 이유와 raw probe로 **유지 결정**; 있으면 한 후보만 development에서 시험하고 schema 양 ingest 경로·pagination·reopen·holdout guard를 증명한다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P2. 진입 probe·변경 또는 유지 결정 `NOT_RUN`; 정책을 임의 승격하지 않는다. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-05/06.
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
5. development에서 Hit@1/MRR 효과를 측정해 한 후보를 선택하고 실험 profile에만 둔다. 기본 정책 승격은 RBR-12의 단일 최종 holdout 판정 이후다. fusion weight나 범용 BM25 parameter까지 동시에 조정하지 않는다.

## 테스트·합격

- exact vs token-partial, local vs container-only, 동일 이름 다른 namespace, qualified name, Unicode/case, definitions vs references.
- stable ties, repeated runs, 여러 페이지 연결 시 중복/누락 없음, wrong-generation/config cursor 거부.
- insert/upsert/replace/reopen이 동일 정렬을 제공하며 legacy schema는 typed refusal/rebuild.
- 독립 fixtures 전부 통과. 최종 holdout에서는 사전 선언한 rank 효과와 qualified NDCG/recall guard를 [TEST-PLAN](TEST-PLAN.md) 기준으로 함께 판정해야 기본 정책을 바꿀 수 있다.
- 진입 조건이 없으면 trace/실험 근거와 함께 유지 결정으로 종료한다. 실행하지 않은 ranker를 구현 완료로 쓰지 않는다.
