# RBR-07 — Full-vector parity와 exact/ANN 원인 분리

> Archive status: `Historical work packet`. Accepted decisions are owned by [SEP-26-002](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md). Current unfinished work is tracked only in [GAP-REGISTER.md](GAP-REGISTER.md). This file is retained for implementation and evidence history.


## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현 갱신: 마지막 관측 `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e` + 공유 dirty. `model2vec/parity_fixture.rs`에 inference 전 strict typed validator를 추가했다. required/duplicate/unknown fields, schema/profile/model ID/revision/library/asset pins, 명시적 null policy, canonical 9-input 순서, full-width finite vectors, norms와 full pairwise triangle의 독립 재계산 정합성을 검증한다. asset-free 독립 basis-vector oracle과 omission/partial/reorder/forged/nonfinite/overflow/zero 반례 4 tests + capture safety 2 tests를 추가했다. Rust unit-layer norm 검증을 별도로 추가하고 기존 component `0.002`·cosine `0.005` tolerance는 유지했다. triangle index는 checked arithmetic으로 처리하며 lint suppression을 추가하지 않았다.
- reference artifact는 **schema 2** 하나만 수용한다. producer가 local path를 model ID로 쓰지 않고 canonical `minishlab/potion-code-16M-v2` + exact revision을 기록한다. 과거 schema 1 reference는 거부 후 pinned venv에서 재생성한다. 이 reference schema는 RBR-12/T15의 fail-closed raw proof envelope schema 1과 별개이며, validator/양성 inference만으로 T15 producer custody를 승인하지 않는다. generator는 zero/nonfinite/incomplete output을 거부하며 permutation 검사도 `python -O`에서 생략되지 않는 명시적 오류로 처리한다.
- **VERIFIED — local binary behavior:** 외부 `/private/tmp/qi-rbr07-validator.2MFS28/receipt.json`의 마지막 실행에서 asset-free 6 tests 통과, actual pinned schema 2 reference 9×256 정상 rail 정확히 1 passed, optional actual raw capture rail 정확히 1 passed. invalid 8종(이전 7 mutants + `max_length` key 누락)과 원본 schema 1은 각각 정확히 1 failed/exit 101로 거부했다. 동일 실행 binary SHA `a7b5e32c0617f62657801d4cae21f4d3ae5ef66f41b76fbb2523f7d97f4ac7a5`; 전체 embed lib 76 passed/3 ignored, all-target Clippy·scoped format 각각 exit 0. generator Ruff도 exit 0. 원본 fixture/모델은 수정하지 않았다.
- **NOT_RUN — frozen-source/T15 qualification:** 최종 audit의 HEAD와 own/source-config 8 inputs는 pre/post 동일했지만 contract ingest/observation와 core lexical/semantic outbound/stream 5 dependency source가 병렬 편집으로 바뀌었다. 따라서 binary의 위 동작 결과를 단일 frozen-source receipt 또는 전체 제품 자격으로 승격하지 않는다. 초기 병렬 core/wire compile 중단과 이전 Clippy 실패도 별도 raw logs에 보존했다. 최종 `receipt.json` SHA `1bb22e2a0948da65bd7452a186d07757aea0cc30acda881bd4689ad287902d21`; `artifacts.sha256` SHA `a755023f2d708e5ba2b00f7cb0531b91c1ba4cd0d77b370be6f3227b503b2022`. 이 로그/입력/환경/binary receipt는 local diagnostic이며 raw source closure/terminal 자격 증거를 대신하지 않는다.
- 후속 raw producer: test-only `model2vec/parity_capture.rs` + optional `QUANTA_INDEX_PARITY_CAPTURE`가 모든 assertions 성공 뒤에만 native raw/unit full vectors, norms, direction cosine triangle, reverse permutation, fixed tolerance, reference/asset/model identities를 기록한다. absolute external target·`create_new`·repo 및 symlink-parent 차단·256 KiB bound를 적용하고 write/flush 실패는 test 실패다. source/env/terminal custody는 self-report하지 않는다. 별도 terminal wrapper와 T15 consumer 비교/identity 검증 전에는 raw capture만으로 same-model claim을 승인하지 않는다.
- 실제 raw artifact: `/private/tmp/qi-rbr07-validator.2MFS28/capture-yfucfeox/native.json`, 158333 bytes, SHA `c0d3da5bafd37c3ac3cafe2a88bee57a6b5695cfa511000ee9f9a79223f2cab6`. independent Python reference `/private/tmp/qi-rbr07-validator.2MFS28/reference-v2.json`, SHA `af59eb3be514988fb8f3dd32fd76e6af036fcdfb766606ce6225abcee126f0cf`. Capture `schema_version=1, kind=model2vec-native-parity-capture, reference_schema_version=2`는 raw output format이며 T15 proof envelope와 동일 schema가 아니다.
- 잔여: 고정 source/dependencies에서 동일 owning rails 재발급 → 별도 T15 reference/native terminal producer 및 replay comparator → 외부 frozen query/corpus exact-vs-served decomposition. Same-model claim 자동 활성화, ANN effort 변경, 모델 교체는 이 수정 범위가 아니다. 제품 encoder/ANN defect는 여전히 확정되지 않았다.

### 외부 dense RCA producer — local 실행 검증 회수

- **VERIFIED — owner-local 실행 동작:** `quanta-index-semantic`의 기존 `proof` feature에 `ann_proof.rs`와 `quanta-index-ann-proof` CLI를 추가했다. `build.rs`의 proof-only storage-free preflight는 기존 generation/scope/vector/window validators를 재사용한다. production ANN policy·index effort·ranker·default는 변경하지 않는다. 최종 scoped 4 unit tests(4 passed/0 failed/0 ignored), `--features proof --lib --tests --bin quanta-index-ann-proof` Clippy 및 scoped fmt는 각각 exit 0이다. CLI 실제 build/512-row 실행도 exit 0이며 아래 raw receipt의 제한된 범위에서 검증한다.
- 입력은 absolute external JSON과 기존에 없는 absolute external state 두 개다. recursive duplicate-key 거부, exact required query fields(선택 제약은 explicit null), strict integer counts/k, 동일 full typed batch와 vectors/query/filters/k, finite/normalization/dimension/unique-ID 검증을 저장 전에 수행한다. 입력 read는 `take(8 MiB + 1)`로 allocation 전에 제한한다. semantic rows 16,384·queries 16·k 128·dimension 4,096 및 raw output 32 MiB 상한을 적용하며 typed batch serialization expansion도 저장 전에 bounded counting writer로 계산한다. state는 exclusive `create_dir`, repo 내부/기존 target은 거부한다.
- 실제 production builder와 sealed `open_proven` search API를 사용한다. 별도 f64 exhaustive cosine oracle은 모든 eligible input f32 vector를 순회한다. exact order는 score descending/embedding ID ascending, served order는 production 순서 그대로다. 모든 physical semantic columns와 full vectors를 input과 대조하며 corpus/input/full-row SHA, raw scores와 같은 row의 독립 score error, exact/served counts·strict/tie-aware overlap·sealed dense contract·before/after/counter deltas를 기록한다. ANN/exact/fallback 실행 여부는 실제 counter로 구분한다. 임의 pass tolerance·overall pass flag는 없다.
- 외부 owning 결과: `/private/tmp/qi-rbr07-ann-proof.iHL7OL/receipt.json` SHA `28e29fa515c51197c5edb03fa07c5ce3ddde1375ac16698e54cd3777c3ce47ae`. input SHA `30de38f12904c5f17d4e7d59aa1fb19fdc281977931f694c0c450164`(synthetic 512×8 vectors/4 queries), frozen CLI SHA `85a664a74f070610691819cc3affc12b29373942f259c56fcbff22e62ee858b1`, raw `positive.stdout` SHA `9ea1458fa1a38cca090157b43153b28b1097e6c483839e580679b035090b8f5c`. Independent Python 3.13 replay는 exact f64 arithmetic, 모든 semantic physical row, f32 vector bits, raw original JSON hash bytes, filters/identity/ties/score errors/counters를 다시 계산했다. eligible 512/256/256/0, exact 및 served 10/10/10/0, strict/tie-aware overlap 10/10/10/0, 네 순서 모두 일치. 최대 same-row score error 4.55e-8/8.51e-8/6.56e-8/null. sealed `ivf_hnsw_sq`에서 각 query ANN counter +1, exact 및 completion +0였다.
- **VERIFIED — native refusal:** bool/float count, count mismatch, bool/zero/wrong-width vectors, duplicate query/embedding IDs, recursive duplicate JSON key, nonfinite, required null key omission, unnormalized vector, 8 MiB 초과, typed projected-output 초과 총 **14 invalid fixtures**는 각각 exit nonzero/state 미생성으로 거부됐다. 같은 정상 입력을 기존 state에 재실행한 경우도 거부됐으며 state 파일별 SHA는 전후 동일했다. 초기 compiler/Clippy 실패와 Python 3.9 replay incompatibility는 별도 로그/receipt에 보존했고 성공으로 계산하지 않았다.
- 실제 frozen 후보 code corpus + pinned 256-dimension model vectors는 별도 lane `/private/tmp/qi-rbr07-real-ann.DH0pzi/`에서 같은 frozen binary로 실행했다. 담당 lane의 초기 실제 결과는 406 rows/88 files/4 queries에서 exit 0 및 independent replay exit 0; eligible 406/406/8/0, overlap 10/10/8/0, 순서 모두 일치, 최대 score error 약 9.32e-8다. raw SHA `491252efa571e9f9bb25396e097004b7a9ed2e0e50a2570461ae874cc6b62565`. 이 subset은 후보 파일 bytes의 약 47.8%/완전 커버 파일 59개이며 전체 코드 코퍼스 품질·독립 gold 자격이 아니다. 해당 담당 lane 최종 consumer-negative receipt와 자세한 scope는 중앙 통합에서 별도 회수한다.
- **NOT_RUN — clean-source/continuous-custody 및 corpus-wide qualification:** scoped final checks에서 contract/core/semantic source SHA는 전후 동일했으나 HEAD는 병렬 writer의 `bc794630...`→`349090ca...`로 변경됐고, 부모 task의 unidentified read-surface/input-identity event도 continuous custody에서 제외한다. CLI binary는 마지막 test-only redundant-clone 수정 이전 build(ann proof SHA `d9ac6c6f...`)이며 최종 source SHA `c8fbf9e4...`의 차이는 `#[cfg(test)]` fixture ownership 이동 한 줄뿐이다. 이 결과를 source-independent ANN 자격, quiet-host latency, 모델 품질, installed process 또는 완전한 source closure로 승격하지 않는다. Membership raw는 포함되지만 별도 independent membership oracle은 이번 범위에서 제외한다. served-output/counter 직접 함수 mutation은 아직 별도 coverage gap이며 reachable 결함으로 확인된 것은 아니다.

### Owning verification rails

- Asset-free validator/safe-capture 6 tests: `./scripts/cargow test -p quanta-index-embed --lib --locked model2vec::parity_ -- --nocapture`.
- Actual model: pinned absolute `QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR`·`QUANTA_INDEX_PARITY_REFERENCE`와 optional external `QUANTA_INDEX_PARITY_CAPTURE`를 지정하여 `model2vec::tests::full_vector_parity_against_pinned_reference --ignored --exact --nocapture` 정확히 한 건 실행. 최종 audit는 한 번 빌드한 같은 test binary를 모든 정상/mutant에 적용하고 각 command와 binary SHA를 기록했다.
- Full local lib 및 lint: `./scripts/cargow test -p quanta-index-embed --lib --locked`, `./scripts/cargow clippy -p quanta-index-embed --all-targets --locked -- -D warnings`, `./scripts/cargow fmt -p quanta-index-embed -- --check`.
- 기존 `proof-required-tests.json` Rust inventory는 retrieval-bench crate만 수집한다. embed 6 tests를 그 목록에 넣지 않는다. 이 owning rail의 actual collection/execution과 T15 exact native test terminal custody는 별도 producer에서 검증한다. `9 raw input cases`와 `1 selected native test` count를 혼동하지 않는다.

### 이전 source의 결함 재현 — 보존된 감사 기록

- 관측 source: `604149ed3f6033e24a834ebaa86a596f7b8ed82d` + 공유 dirty. reference generator의 model2vec 0.9.0·weights/tokenizer/config 정확한 pin 거부와 Rust full-vector/exact-ANN harness는 존재한다. 제품 encoder/ANN 결함을 확정한 것은 아니다.
- **FAILED — fixture 검증기:** `model2vec.rs::full_vector_parity_against_pinned_reference`는 JSON indexing의 missing→Null 때문에 policy 부재를 명시적 `max_length=null`과 구별하지 않는다. schema version·norms를 읽지 않고, pairwise 행 수/삼각형 폭·canonical 9-input 목록/순서를 강제하지 않는다. 코드 주석의 norm/전체 pairwise 검증 주장은 현재 구현보다 강하다.
- 재현: 외부 `/private/tmp/qi-rbr-source-audit.p3VfSl/`에서 실제 pinned 세 asset의 SHA를 재확인하고 ignored test를 각 실행마다 정확히 한 건 선택했다. 정상 9×256 fixture는 1 passed(exit 0); `pairwise_cosine_upper=[]`, schema/policy/norms 동시 삭제, 각 필드 개별 삭제 세 건, norms=999, adversarial input 축소+empty pairwise **7종 invalid fixture도 각각 1 passed(exit 0)**여서 거부 계약 **FAILED**다. 축소 input+유효 pairwise 대조군은 cosine 차이로 실패했으며 canonical coverage의 거부 증거가 아니다. 정상 fixture 통과를 fail-closed parity 자격 또는 T15 양성 proof로 승격하지 않는다. 원본 fixture/모델은 수정하지 않았다. 정확한 명령·입력/로그 digest는 `parity-audit.json`·`parity-mutation-audit.json`을 따른다.
- 잔여 코드: inference 전에 typed/strict fixture validator를 추가한다. schema/profile/library/asset pins, required policy key와 명시적 null, canonical adversarial input 집합·순서, finite 256-dimension vectors, norms의 수량·값·벡터와의 정합성, 모든 pairwise 행/폭·유한 값·독립 재계산을 검증한다. missing/reorder/subset/partial/forged 반례는 asset-free test로 실행하고 required inventory에 등록한다. fp16 reference norm과 Rust L2 norm은 별개 계약으로 검증하며 기존 tolerance를 사후 확대하지 않는다.
- 잔여 proof/실험: validator 수정 뒤 actual-model parity를 재발급하고 외부 frozen query/corpus에서 exact-vs-served per-query delta를 분해한다. 이전 synthetic exact/ANN 5 passed는 외부 결과의 증거가 아니다. T15 raw producer·terminal 실행 custody는 RBR-12에서 별도 구현; same-model claim이 false면 T15는 `NOT_APPLICABLE`, true면 현재 양성 proof는 `NOT_RUN`이며 verdict는 fail-closed한다.

- 우선순위: **P0 proof-integrity**. 원 리뷰 계약의 fail-open 차단 기준을 적용한다. 차단 범위는 이 fixture rail의 완전한 parity 증거 승인이지 전체 제품 실행/일반 pair가 아니다. fixture validator 수정 → 실제 asset 재검증 → 외부 원인 분해. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/03; validator 수정 자체는 즉시 가능하다.
- 성격: 재현된 proof-integrity 결함 + 미실행 원인 실험. 현재 semantic 실패를 특정 모델/ANN 결함으로 확정하지 않는다.

## 파일·함수

최종 manual typed fixture rail: `/private/tmp/qi-rbr07-manual-deserialize.xE4bW9/receipt.json` SHA-256 `ee0572805ab6d41179038e3094f252510f3e9fcb60d8ad56c8781c263822fc4b`; 소유 입력 전후 동일, asset-free6·actual pinned 정상1·invalid8+schema1 거부 및 Clippy/fmt 회수. 조건부 exporter와 external exact-vs-served proof-only CLI는 구현됐고 위 local 실행 검증도 회수했다. `vector_index.rs::exact_top_k` cfg(test) helper를 oracle로 재사용하지 않는다. external corpus-wide RCA 및 최종 T15 custody는 별도 종료한다.

- [model2vec.rs](../../../../crates/quanta-index-embed/src/model2vec.rs): `encode`, ignored pinned-model test.
- [parity_fixture.rs](../../../../crates/quanta-index-embed/src/model2vec/parity_fixture.rs): required typed schema 2 + canonical input/numeric evidence validator와 asset-free 반례.
- [parity_capture.rs](../../../../crates/quanta-index-embed/src/model2vec/parity_capture.rs): optional bounded write-once external raw artifact, capture safety tests.
- [parity_reference.py](../../../../tools/benchmark/retrieval/parity_reference.py): exact model2vec 0.9.0·asset pin 검증 뒤 canonical schema 2 reference 생성.
- `crates/quanta-index-semantic/src/ann_proof.rs`: duplicate-safe typed input admission, full-row corpus oracle와 independent exact-vs-served raw producer.
- `crates/quanta-index-semantic/src/bin/quanta-index-ann-proof.rs`: bounded external input read + exclusive fresh-state CLI; `Cargo.toml`/`lib.rs`는 proof feature 등록만 추가.
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
