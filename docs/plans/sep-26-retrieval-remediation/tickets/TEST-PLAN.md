# RBR 공통 검증·완료 계약

상태: 테스트 설계 작성. 아래 후속 구현 검증은 `NOT_RUN`이다. 기존 [RB TEST-PLAN](../../sep-23-retrieval-bench/tickets/TEST-PLAN.md)의 입력 격리·replay·성능 자격을 약화하지 않는다.

## 1. 공통 oracle와 부정 테스트

- 기대값은 고정 fixture의 독립 선언 span, 수작업 계산, canonical public contract, pinned reference encoder, exhaustive cosine 또는 관측 가능한 호출 횟수에서 얻는다. 구현 결과를 정답으로 복사하지 않는다.
- 누락·잘못된 타입·unknown field·중복·순서변경·stale source·wrong generation/model·위조 digest·partial/timeout/interruption을 거부한다. 미관측 값은 0이나 성공이 아니다.
- query planning과 symbol producer는 evaluator-only gold/category/holdout labels를 읽을 수 없다. 기존 격리를 유지하고 접근 시도에 대한 부정 테스트를 추가한다.
- 각 테스트 추가와 같은 변경에서 `proof-required-tests.json`의 해당 실제 수집 identity를 갱신한다. inventory equality를 subset 허용으로 바꾸지 않는다.

## 2. 티켓별 필수 검증

| 티켓 | 독립 oracle / 핵심 반례 | 최소 owning rail |
| --- | --- | --- |
| 00 | 실제 pytest/nextest collection; 새 계약 파일 변경 시 closure 거부; profile 위조 | Python proof/receipt tests + contract |
| 01 | SDK 응답과 sidecar 필드 대조; executed-but-empty lane; request 혼합; missing stage | contract + SDK |
| 02 | DSL AND 보존; literal escaping; sentence/identifier paired fixture; gold 비접근 | contract + SDK |
| 03 | pinned reference 함수 출력과 lane 호출 spy; alpha endpoint도 dual execution | Python adapter + 실제 pinned Semble 개발 캡처 |
| 04 | 5개 언어의 수작업 definition spans; symbol-only 변경의 digest 변경; combined replacement | contract + storage + SDK |
| 05 | forged symbol ID/path/span; wrong generation; no-answer/timeout; line-expanded context | contract + SDK |
| 06 | UTF-8/CRLF/긴 줄/겹친 span의 손계산; rank와 density의 독립 계산 | chunking contract + Python scorer |
| 07 | full vectors + exhaustive cosine; 255/256 boundary; short/full ANN result | embed focused + semantic integration |
| 08 | 정확 이름/동명이인/부분일치; stable tie; multi-page no duplication/omission | lexical fixtures + storage |
| 09 | 동일 후보 fixture에서 fusion 불변식; sparse filters; deadlines; actual lane cost | semantic/storage + daemon |
| 10 | fresh/delta/reopen/replay/tombstone/fault/restart의 최종 row set | semantic integration + daemon |
| 11 | 실제 프로세스 트리와 positive-RSS descendants; zero-RSS 연결 노드 | Python sampler + platform별 owner check |
| 12 | raw receipts/records로 fresh-process verdict 재도출; wrong-source/admission 거부 | contract proof + SDK proof + pair/replay |

## 3. 실행 명령

레포 루트에서 실행한다. `<...>`는 새 외부 경로/실제 spec으로 교체할 placeholder다.

```sh
python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -p no:cacheprovider
python3 -m pytest tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_portable_proof.py -q -p no:cacheprovider
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
3. 정확성 fixture는 전부 일치해야 한다. 핵심 primary quality는 기존 density-aware metric을 유지하며 exact-span Recall@10, rank-only MRR/Hit@1, context bytes를 보조로 보고한다. no-answer는 false-positive/abstention을 별도로 보고한다. top-10만 수집했으면 @20을 측정했다고 쓰지 않는다.
4. 개선 선택 기본 규칙: development에서 후보를 고른 후 holdout에서 primary quality 차이의 paired 95% CI 하한이 0 이상이고, 선언된 strata의 recall이 악화되지 않아야 한다. 속도 개선은 같은 조건에서 warm p95 차이 CI 상한이 0 미만이어야 한다. 기존 protocol의 표본/quiet-host 최소조건도 충족해야 한다. 다른 margin을 쓸 경우 결과를 보기 **전에** manifest에 근거와 함께 고정한다.
5. 표본 부족·CI 불확실은 개선 증명이 아니다. holdout 실패 후 같은 holdout으로 재튜닝하지 않는다. 개발 baseline은 허용하지만 qualified 주장은 admission/receipt 조건까지 통과해야 한다.
6. 튜닝 후보는 티켓의 유한 matrix를 사용한다. 확대가 필요하면 사유와 추가 예산을 먼저 기록하고 새 실험으로 분리한다. 결과를 보고 유리한 질의/레포를 제거하지 않는다.

## 5. 증거 묶음과 완료

각 결과에 full HEAD, 관련 dirty paths/source hash, 입력·모델·parser·dependency·설정·binary hash, host/runtime, 정확한 명령, raw 출력/종료 상태, 산출물 경로/SHA-256, covered/excluded scope를 남긴다. schema/config/policy 변경은 옛 증거 재사용을 무효화한다.

개발 작업의 종료와 `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`의 종료를 분리한다. 외부 독립 입력이 없으면 해당 qualification만 `BLOCKED` 또는 `NOT_RUN`으로 남기고, 개발 티켓에 가짜 승인·정답·quiet-host를 만드는 작업을 추가하지 않는다.
