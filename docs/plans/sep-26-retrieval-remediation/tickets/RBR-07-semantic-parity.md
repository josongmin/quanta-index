# RBR-07 — Full-vector parity와 exact/ANN 원인 분리

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현 갱신: `7cefac4a10a06ed56b6f5b9f42b3726468b1f198` + 공유 dirty에서 `model2vec/parity_fixture.rs`에 inference 전 strict typed validator를 추가했다. required/duplicate/unknown fields, schema/profile/model ID/revision/library/asset pins, 명시적 null policy, canonical 9-input 순서, full-width finite vectors, norms와 full pairwise triangle의 독립 재계산 정합성을 검증한다. asset-free 독립 basis-vector oracle과 omission/partial/reorder/forged/nonfinite/overflow/zero 반례 4 tests를 추가했다. Rust unit-layer norm 검증을 별도로 추가하고 기존 component `0.002`·cosine `0.005` tolerance는 유지했다. 단 compile/test 통과 전에는 구현 검증을 `NOT_RUN`으로 둔다.
- reference artifact는 **schema 2** 하나만 수용한다. producer가 local path를 model ID로 쓰지 않고 canonical `minishlab/potion-code-16M-v2` + exact revision을 기록한다. 과거 schema 1 reference는 거부 후 pinned venv에서 재생성한다. 이 reference schema는 RBR-12/T15의 fail-closed raw proof envelope schema 1과 별개이며, validator/양성 inference만으로 T15 producer custody를 승인하지 않는다. generator는 zero/nonfinite/incomplete output을 거부하며 permutation 검사도 `python -O`에서 생략되지 않는 명시적 오류로 처리한다.
- 현재 검증: 외부 `/private/tmp/qi-rbr07-validator.2MFS28/`에서 pinned 실제 model2vec reference schema 2(9×256)를 재생성했다. asset-free validator 4 tests 통과 뒤 같은 binary의 실제 정상 fixture rail 정확히 1 test 통과, invalid 8종(이전 7 mutants + `max_length` key 누락)과 원본 schema 1은 각각 정확히 1 failed/exit 101로 거부했다. `validator-native-pre-capture-receipt.json`에 source/dirty·asset/dependency·command/log/binary SHA를 보존했다. 초기 병렬 core/wire compile 중단과 inherited contract `large_enum_variant` Clippy 실패도 raw log로 남겼으며 이 결과는 parity 성공으로 세지 않는다. capture helper 포함 최종 binary 재검증은 진행 중이다.
- 후속 raw producer: test-only `model2vec/parity_capture.rs` + optional `QUANTA_INDEX_PARITY_CAPTURE`가 모든 assertions 성공 뒤에만 native raw/unit full vectors, norms, direction cosine triangle, reverse permutation, fixed tolerance, reference/asset/model identities를 기록한다. absolute external target·`create_new`·repo 및 symlink-parent 차단·256 KiB bound를 적용하고 write/flush 실패는 test 실패다. source/env/terminal custody는 self-report하지 않는다. 별도 terminal wrapper와 T15 consumer 비교/identity 검증 전에는 raw capture만으로 same-model claim을 승인하지 않는다.

### 이전 source의 결함 재현 — 보존된 감사 기록

- 관측 source: `604149ed3f6033e24a834ebaa86a596f7b8ed82d` + 공유 dirty. reference generator의 model2vec 0.9.0·weights/tokenizer/config 정확한 pin 거부와 Rust full-vector/exact-ANN harness는 존재한다. 제품 encoder/ANN 결함을 확정한 것은 아니다.
- **FAILED — fixture 검증기:** `model2vec.rs::full_vector_parity_against_pinned_reference`는 JSON indexing의 missing→Null 때문에 policy 부재를 명시적 `max_length=null`과 구별하지 않는다. schema version·norms를 읽지 않고, pairwise 행 수/삼각형 폭·canonical 9-input 목록/순서를 강제하지 않는다. 코드 주석의 norm/전체 pairwise 검증 주장은 현재 구현보다 강하다.
- 재현: 외부 `/private/tmp/qi-rbr-source-audit.p3VfSl/`에서 실제 pinned 세 asset의 SHA를 재확인하고 ignored test를 각 실행마다 정확히 한 건 선택했다. 정상 9×256 fixture는 1 passed(exit 0); `pairwise_cosine_upper=[]`, schema/policy/norms 동시 삭제, 각 필드 개별 삭제 세 건, norms=999, adversarial input 축소+empty pairwise **7종 invalid fixture도 각각 1 passed(exit 0)**여서 거부 계약 **FAILED**다. 축소 input+유효 pairwise 대조군은 cosine 차이로 실패했으며 canonical coverage의 거부 증거가 아니다. 정상 fixture 통과를 fail-closed parity 자격 또는 T15 양성 proof로 승격하지 않는다. 원본 fixture/모델은 수정하지 않았다. 정확한 명령·입력/로그 digest는 `parity-audit.json`·`parity-mutation-audit.json`을 따른다.
- 잔여 코드: inference 전에 typed/strict fixture validator를 추가한다. schema/profile/library/asset pins, required policy key와 명시적 null, canonical adversarial input 집합·순서, finite 256-dimension vectors, norms의 수량·값·벡터와의 정합성, 모든 pairwise 행/폭·유한 값·독립 재계산을 검증한다. missing/reorder/subset/partial/forged 반례는 asset-free test로 실행하고 required inventory에 등록한다. fp16 reference norm과 Rust L2 norm은 별개 계약으로 검증하며 기존 tolerance를 사후 확대하지 않는다.
- 잔여 proof/실험: validator 수정 뒤 actual-model parity를 재발급하고 외부 frozen query/corpus에서 exact-vs-served per-query delta를 분해한다. 이전 synthetic exact/ANN 5 passed는 외부 결과의 증거가 아니다. T15 raw producer·terminal 실행 custody는 RBR-12에서 별도 구현; same-model claim이 false면 T15는 `NOT_APPLICABLE`, true면 현재 양성 proof는 `NOT_RUN`이며 verdict는 fail-closed한다.

- 우선순위: **P0 proof-integrity**. 원 리뷰 계약의 fail-open 차단 기준을 적용한다. 차단 범위는 이 fixture rail의 완전한 parity 증거 승인이지 전체 제품 실행/일반 pair가 아니다. fixture validator 수정 → 실제 asset 재검증 → 외부 원인 분해. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/03; validator 수정 자체는 즉시 가능하다.
- 성격: 재현된 proof-integrity 결함 + 미실행 원인 실험. 현재 semantic 실패를 특정 모델/ANN 결함으로 확정하지 않는다.

## 파일·함수

- [model2vec.rs](../../../../crates/quanta-index-embed/src/model2vec.rs): `encode`, ignored pinned-model test.
- [parity_fixture.rs](../../../../crates/quanta-index-embed/src/model2vec/parity_fixture.rs): required typed schema 2 + canonical input/numeric evidence validator와 asset-free 반례.
- [parity_capture.rs](../../../../crates/quanta-index-embed/src/model2vec/parity_capture.rs): optional bounded write-once external raw artifact, capture safety tests.
- [parity_reference.py](../../../../tools/benchmark/retrieval/parity_reference.py): exact model2vec 0.9.0·asset pin 검증 뒤 canonical schema 2 reference 생성.
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

- 필수 mutant: schema/library/policy/normalization/norms 누락, null 대 누락, 입력 제거·추가·순열, 빈/잘린/길어진 pairwise triangle, vector/norm 숫자 변조·nonfinite·dimension/row count 불일치. 정상 shape validator와 양성 actual-model rail을 각각 검증한다.
- pinned asset mutation, model-ID mismatch, reorder, finite/dimension/norm, near-tie tolerance fixtures.
- exact/ANN scoped filters, ties, k 경계, short result exact completion, cancellation/deadline.
- ignored asset-dependent test는 전용 실행 rail과 실제 결과가 있어야 parity `VERIFIED`. 기본 테스트가 skip했다고 성공으로 세지 않는다.
- 출력: full-vector parity report, ANN-vs-exact per-query delta, 원인별 관측/미관측 목록, 수정 또는 유지 결정.

공통 [TEST-PLAN](TEST-PLAN.md)의 embed focused + semantic integration 적용. 이번 짧은 과거 질의의 실패를 긴 입력 truncation 차이로 설명하지 않는다.
