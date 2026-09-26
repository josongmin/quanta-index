# RBR 공통 검증·완료 계약

상태: 공통 완료 계약. 최신 감사 source는 `604149ed` + 공유 dirty이며 티켓별 구현·실행 판정은 [CURRENT-AUDIT.md](CURRENT-AUDIT.md)의 최신 절을 따른다. actual pinned parity 정상 fixture의 local 통과만으로 RBR-07의 malformed fixture 거부 실패를 덮지 않는다. Rust/SDK exact collection, clean-source proof와 qualified final pair는 미발급이다. 두 bare-symbol 탐색 pair의 `PAIR_VALID=pass`는 이 게이트를 대체하지 않는다. 이 문서 변경도 retrieval source closure 입력이다. 기존 [RB TEST-PLAN](../../sep-23-retrieval-bench/tickets/TEST-PLAN.md)의 입력 격리·replay·성능 자격을 약화하지 않는다.

## 1. 공통 oracle와 부정 테스트

- 기대값은 고정 fixture의 독립 선언 span, 수작업 계산, canonical public contract, pinned reference encoder, exhaustive cosine 또는 관측 가능한 호출 횟수에서 얻는다. 구현 결과를 정답으로 복사하지 않는다.
- 누락·잘못된 타입·unknown field·중복·순서변경·stale source·wrong generation/model·위조 digest·partial/timeout/interruption을 거부한다. 미관측 값은 0이나 성공이 아니다.
- query planning과 symbol producer는 evaluator-only gold/category/holdout labels를 읽을 수 없다. 기존 격리를 유지하고 접근 시도에 대한 부정 테스트를 추가한다.
- 각 테스트 추가와 같은 변경에서 `proof-required-tests.json`의 해당 실제 수집 identity를 갱신한다. inventory equality를 subset 허용으로 바꾸지 않는다.

## 2. 티켓별 필수 검증

| 티켓 | 독립 oracle / 핵심 반례 | 최소 owning rail |
| --- | --- | --- |
| 00 | 실제 pytest/nextest collection; 새 계약 파일 변경 시 closure 거부; profile 위조 | Python proof/receipt tests + contract |
| 01 | SDK/sidecar 필드 대조; executed-but-empty lane; missing stage; 실제 server observation on/off config/binary identity·결과 동등성. runner sidecar off는 server off가 아님 | contract + SDK + bounded overhead |
| 02 | DSL AND 보존; literal escaping; sentence/identifier paired fixture; gold 비접근 | contract + SDK |
| 03 | pinned reference 함수 출력과 lane 호출 spy; alpha endpoint도 dual execution | Python adapter + 실제 pinned Semble 개발 캡처 |
| 04 | 5개 언어의 수작업 definition spans; symbol-only 변경의 digest 변경; combined replacement; unsupported admitted 파일별 path+SHA/skip reason과 partial/full capability 판정 | contract + storage + SDK |
| 05 | forged symbol ID/path/span; wrong generation; no-answer/timeout; line-expanded context | contract + SDK |
| 06 | UTF-8/CRLF/긴 줄/겹친 span의 손계산; rank와 density의 독립 계산 | chunking contract + Python scorer |
| 07 | strict schema/required policy·명시적 null/canonical input·norm·pairwise triangle 검증과 omission/subset/reorder/forgery 거부; full vectors + exhaustive cosine; 255/256·short/full ANN; 조건부 T15 raw/asset/실행 custody | asset-free validator + actual asset embed + semantic integration + proof replay |
| 08 | 정확 이름/동명이인/부분일치; stable tie; multi-page no duplication/omission | lexical fixtures + storage |
| 09 | 동일 후보 fixture에서 fusion 불변식; sparse filters; deadlines; actual lane cost | semantic/storage + daemon |
| 10 | fresh/delta/reopen/replay/tombstone/fault/restart row set; transient observation의 request/repo/revision/batch/generation 혼합·누락·partial/replay 거부; durable receipt timing 비혼입 | semantic integration + 실제 SDK/daemon |
| 11 | 실제 프로세스 트리와 positive-RSS descendants; zero-RSS 연결 노드 | Python sampler + platform별 owner check |
| 12 | cross-suite dev/holdout file·definition·query-family 누수 거부; raw receipts/records로 fresh-process verdict 재도출; wrong-source/admission 및 조건부 T15/T16 `pass` 요약 위조 거부 | contract proof + SDK proof + pair/replay |

## 3. 실행 명령

레포 루트에서 실행한다. `<...>`는 새 외부 경로/실제 spec으로 교체할 placeholder다.

```sh
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_portable_proof.py -q -p no:cacheprovider
just retrieval-contract-local
just rust-profile test-integration-storage
just rust-profile test-integration-semantic
just retrieval-contract-proof <fresh-external-contract-root>
just retrieval-sdk-proof <fresh-external-sdk-root>
just retrieval-pair <frozen-external-spec>
just retrieval-verdict <repo> <suite> <run-manifest> <fresh-external-verdict>
```

영향받는 surface만 선택하여 점차 확장한다. 단일 crate 예외 rail은 `./scripts/cargow test -p <crate>`다. public SDK/contract 변경은 `just rust-public-api`; IPC DTO/decoder 변경은 `just rust-fuzz-smoke`; activation/generation/query-pin 변경은 `just rust-profile test-daemon` 및 owning scenario를 추가한다. facade/module boundary 변경은 `just rust-hexagonal`과 `just rust-cargo-modules`를 추가한다.

`retrieval-sdk-proof`에는 output 인자가 필수다. proof rail은 정확한 clean-source closure를 요구한다. dirty local 성공을 receipt로 변환하거나 check를 끄지 않는다.

## 4. 실험 고정과 선택 규칙

1. RBR-12에서 튜닝 전에 task family, split, candidate matrix, primary/secondary metric, 실행 순서·seed, 반복·표본 수, 시간 상한, 판단 규칙을 외부 experiment manifest에 고정한다. 원본 질의 동일성과 내부 변환 차이를 별개로 기록한다.
2. native-default 제품 비교와 controlled mechanism 비교를 합산하지 않는다. lexical, semantic, hybrid, symbol을 별도로 보고하고 공통 지원 범위를 명시한다.
3. 정확성 fixture는 전부 일치해야 한다. **qualified quality의 primary는 기존 RB TEST-PLAN §2.6의 graded density-aware NDCG@10**으로 유지한다. 독립 adjudicated gold가 없으면 이 값을 qualified primary로 계산하거나 대체하지 않는다. exact-span Recall@10, rank-only MRR/Hit@1, context bytes와 no-answer false-positive/abstention은 보조 지표다. top-10만 수집했으면 @20을 측정했다고 쓰지 않는다.
4. RBR-08/09/10의 효과 지표를 각각 개발 실험 전에 고정한다: symbol Hit@1/MRR, 동일 조건 warm query p95, fresh-root time-to-searchable. 공통 품질 guard는 qualified primary NDCG@10과 선언된 strata의 exact-span recall이다. 기능을 새로 여는 RBR-02/05와 청킹 RBR-06도 효과·guard·지원 task 범위를 미리 명시한다. 결과를 보고 primary나 margin을 바꾸지 않는다.
5. 후보 선택은 development에서 끝낸다. 동시에 변경된 query/chunk/rank/fetch/ingest 정책의 **최종 조합 한 개**와 기준 조합을 잠근 뒤 holdout을 한 번 연다. holdout에서 NDCG 차이의 query-paired 95% CI 하한이 사전 선언한 비열등 margin 이상이어야 하고, 채택을 주장하는 효과 지표의 CI가 사전 선언한 개선 기준을 충족해야 한다. 기본 margin은 0이다. 각 strata guard, quiet-host 및 기존 표본 floor도 충족해야 한다. 다중 후보를 holdout에서 고르지 않는다. 한 조합으로 시험한 변경은 묶음으로 채택하거나 기본 조합을 유지한다. 개별 효과를 주장하려면 development ablation과 별도 독립 holdout이 필요하다.
6. 품질 interval은 query/repo를 단위로, warm latency interval은 query·fresh-root의 반복 구조를 보존하여 재표본한다. 1,000개 반복 시간을 1,000개 독립 질의로 취급하지 않는다. ingest 시간은 fresh root를 단위로 계산한다. interval 방법·seed·반복 수·계층 처리·결측/실패 취급을 결과 보기 전에 고정한다. 실패/timeout/partial을 좋은 지연 샘플에서 제외하고 속도 개선으로 계산하지 않는다.
7. 표본 부족·CI 불확실은 개선 증명이 아니다. holdout 실패 후 같은 holdout으로 재튜닝하지 않는다. 필요한 후속 튜닝은 개발 데이터에서 수행하고, 새로 독립 고정한 holdout으로 별도 실험을 만든다. 개발 baseline은 허용하지만 qualified 주장은 admission/receipt 조건까지 통과해야 한다.
8. 튜닝 후보는 티켓의 유한 matrix를 사용한다. 확대가 필요하면 사유와 추가 예산을 먼저 기록하고 새 실험으로 분리한다. 결과를 보고 유리한 질의/레포를 제거하지 않는다.

## 5. 증거 묶음과 완료

각 결과에 full HEAD, 관련 dirty paths/source hash, 입력·모델·parser·dependency·설정·binary hash, host/runtime, 정확한 명령, raw 출력/종료 상태, 산출물 경로/SHA-256, covered/excluded scope를 남긴다. schema/config/policy 변경은 옛 증거 재사용을 무효화한다. 새 티켓 디렉터리가 source closure에 편입되면 이 문서의 사후 상태 변경도 receipt를 무효화한다. 최종 상태·결과표는 레포 외부의 digest-bound closeout artifact에 기록한다.

조건부 T15(동일 모델)·T16(증분)의 `pass`/count JSON과 그 SHA-256은 독립 증거가 아니다. T15는 pinned 양쪽 모델·토크나이저·정밀도·입력·raw 전체 벡터·사전 선언 tolerance·실행 context로, T16은 동일 source/generation에서 before/after raw row-set·rename/delete/restart·실행 context로 재도출해야 한다. 임의 command 문자열 또는 새 digest를 붙인 요약만으로 조건부 claim을 승인하지 않는다. 현재 `run.py`는 raw 양성 프로토콜이 없어 두 조건부 claim을 fail-closed로 거부한다. 과거 summary-only 승인은 역사적 결함이며, 현 양성 proof는 `NOT_RUN`이다. [현재 감사](CURRENT-AUDIT.md)를 따른다.

`same_model=false`/`incremental=false`이면 각각 T15/T16은 `NOT_APPLICABLE`다. claim을 true로 여는 경우에만 해당 양성 protocol이 필수다. 이 조건을 일반 pair의 무조건 필수 gate로 늘리거나 protocol 없이 claim을 true로 설정하지 않는다.

개발 작업의 종료와 `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`의 종료를 분리한다. 외부 독립 입력이 없으면 해당 qualification만 `BLOCKED` 또는 `NOT_RUN`으로 남기고, 개발 티켓에 가짜 승인·정답·quiet-host를 만드는 작업을 추가하지 않는다.
