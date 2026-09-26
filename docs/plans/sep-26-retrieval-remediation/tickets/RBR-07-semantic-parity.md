# RBR-07 — Full-vector parity와 exact/ANN 원인 분리

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현: pinned Python full-vector reference와 Rust parity test, `crates/quanta-index-semantic/tests/exact_ann_decomposition.rs`의 경계 oracle은 코드에서 확인. 과거 실모델 통과는 현 source proof가 아니다.
- 검증: 이 감사의 pinned asset/digest, ignored parity rail, owning Rust/semantic integration 및 외부 per-query exact-vs-served delta는 `NOT_RUN`. `run.py`의 조건부 same-model T15 verdict는 임의 `pass` 요약만으로 통과할 수 있어 proof-integrity `FAILED`.
- 잔여: frozen vectors·tokenizer/model/정밀도/truncation binding에서 256차원 전수 parity와 255/256·short/full ANN 분해를 재실행한다. T15는 summary JSON 대신 raw vectors·입력·tolerance·asset/source/실행 context를 재도출/검증해야 한다. wrong-file/wrong-span/미반환을 구분하지 못하면 unresolved로 둔다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. full-vector parity·exact/ANN harness 코드 관측; 현 소스 재실행·외부 per-query 분해는 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/03.
- 성격: 검증 공백. 현재 semantic 실패를 특정 모델/ANN 결함으로 확정하지 않는다.

## 파일·함수

- [model2vec.rs](../../../../crates/quanta-index-embed/src/model2vec.rs): `encode`, ignored pinned-model test.
- [semantic route](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs): query embedding/model gate와 projection.
- [search.rs](../../../../crates/quanta-index-semantic/src/search.rs): `run_vector_query`, `run_lane`.
- [vector_index.rs](../../../../crates/quanta-index-semantic/src/vector_index.rs): `plan_for_rows_v1`, `LoadedApproximateIndexV1::apply`, existing exact-cosine test helpers.
- 벤치 diagnostics/profile 및 외부 reference fixture 생성 명령. 대용량 vectors/model은 외부에 둔다.

## 실험 순서

1. 모델 파일 hash 외에 encoder/tokenizer/library/정밀도/normalization/truncation까지 manifest에 고정한다. native Semble query default max_length=512와 corpus None, Quanta None을 기록한다. controlled parity에서는 명시적으로 같은 정책을 선택한다.
2. identifiers, qualified names, punctuation, Unicode, 빈/tokenless 입력, 긴 입력, batch permutation을 pinned Python reference와 Rust에서 비교한다. 256차원 **전체**, vector norm, pairwise similarity/order를 검증한다. 작성 당시의 한 문장 8성분 테스트만으로는 전체 parity라고 취급하지 않는다.
3. tolerance는 dtype/quantization 근거와 독립 fixtures로 먼저 고정한다. 불일치 후 임의 확대하지 않는다. zero/tokenless 결과는 양쪽 계약을 명시하고 유효한 zero vector로 가장하지 않는다.
4. 동일 frozen corpus/query vectors·제약·k에 exact exhaustive cosine oracle과 실제 선택된 dense lane을 적용한다. 255/256 row 경계 및 충분한 ANN result count에서도 recall 차이를 측정한다.
5. wrong-file / right-file-wrong-span / candidate retrieved-but-not-returned를 가능한 trace 단계로 나눈다. 미관측 단계는 unresolved로 남긴다.
6. 한 요소씩 수정한다: parity mismatch면 encoder 계약, exact는 맞고 ANN만 놓치면 index effort, exact도 틀리면 입력 representation/모델 실험. 모델 교체는 자동 결론이 아니다.

## 테스트·완료

- pinned asset mutation, model-ID mismatch, reorder, finite/dimension/norm, near-tie tolerance fixtures.
- exact/ANN scoped filters, ties, k 경계, short result exact completion, cancellation/deadline.
- ignored asset-dependent test는 전용 실행 rail과 실제 결과가 있어야 parity `VERIFIED`. 기본 테스트가 skip했다고 성공으로 세지 않는다.
- 출력: full-vector parity report, ANN-vs-exact per-query delta, 원인별 관측/미관측 목록, 수정 또는 유지 결정.

공통 [TEST-PLAN](TEST-PLAN.md)의 embed focused + semantic integration 적용. 이번 짧은 과거 질의의 실패를 긴 입력 truncation 차이로 설명하지 않는다.
