# 04 · 같은 제품 4K와 ANN4K의 수용

**담당과 실행 경로.** [이 채팅 B](README.md#채팅별-실행권과-재사용-범위)가 두 본문의 실행 및 입력 결합을 맡는다. 동결 Text Large의 단일 slow PASS 로그는 00의 판독 회귀에만 인수한다. 기존 B 실행기와 [07.5 제품 수용 티켓](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#ann과-hybrid)을 사용한다. [01](01-attribution.md)에 적힌 기존 순서대로 prefix와 ANN이 직렬 실행된다. `product_after_prefix.py`는 prefix가 초기 게시를 확인했고 ANN 실행이 terminal에 도달하면 같은 `retry.py PRODUCT4K`로 이어진다. 이 조건에서 ANN terminal에는 실패도 포함되므로 두 결과를 각각 읽는다. prefix가 게시 전에 실패하면 해당 원인을 고친 뒤 기존 경로를 재개한다.

## 고정한 실행과 판정

| 실행 | 정확한 본문과 통과 조건 |
| --- | --- |
| 제품 4K · tracing Off | `quanta-index-searchd-runtime --test native_product_process`의 `ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall`. 단일 초기 SourceAdvance의 100만 읽기와 fixture의 1,200초를 유지한다. 4,096 source 게시·materialization, 초기 검색, 첫 재시작, 교체·삭제 후 검색, 미저장 overlay의 검색과 종료 후 durable 결과, 두 번째 재시작 후 원본·변경 view 결과와 Lance 파일셋 확인까지 본문 1 PASS·exit 0. [제품 진입점](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L1642) · [마지막 재시작 확인](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L2199) |
| 독립 ANN4K | `quanta-index-semantic --lib`의 `native::embedding::tests::ann_product_tests::ann4k_tests::native_ann4k_selected_file_recall_matches_independent_exact`. 4,096×64·Q64·K10, 전체와 비어 있지 않은 각 필터 구간 Recall@10 ≥0.95, 빈 구간 0건 및 raw·파일 결과의 기존 oracle을 통과. 본문 1 PASS·exit 0. [독립 exact 비교](../../../crates/quanta-index-semantic/src/native/embedding/tests/ann_product_tests/ann4k_tests.rs#L397) · [필터별 판정](../../../crates/quanta-index-semantic/src/native/embedding/tests/ann_product_tests/ann4k_tests.rs#L796) |

**점수표를 읽는 기준.** 비어 있지 않은 각 파일 구간은 Q64×K10이므로 최소 608/640이다. raw는 구간마다 고정 질의 1개에서 독립 정답 집합과 정확히 일치해야 한다(10/10). 빈 구간은 분모 0으로 기록하고 hit 0·exhaustive를 확인한다. 원 제품 watchdog은 프로필 구성 후 시작하며 Nextest 전체 wall time과 다르다. 제품 공개 hit에는 owner·vector ID·score·선택적 chunk가 담긴다. source inventory 대조, 선택 경로 검사와 재시작 후 seal 검증의 범위를 구별한다. [raw·파일 판정식](../../../crates/quanta-index-semantic/src/native/embedding/tests/ann_product_tests/ann4k_tests.rs#L1047) · [제품 watchdog 시작](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L1739) · [공개 hit 계약](../../../crates/quanta-index-contract/src/native_protocol.rs#L1204)

## 실패별 후속 작업

- **초기 게시 거절.** 실제 거절 axis·stage·누적 사용량과 남은 기한을 [01](01-attribution.md)의 닫힌 구간에 대조한다. 읽기 한도나 파일 수를 바꾸거나 여러 SourceAdvance로 분할하면 원 제품 실패 재검증이 성립하지 않는다.
- **게시 이후 watchdog.** 원 실행 로그의 실패 phase·seed, SDK 왕복과 provider 관측, 남은 기한으로 원인을 좁힌다. 128 완료 prefix는 후반 오버레이·두 번째 재시작을 실행하지 않는다. 수정한 뒤 기존 제품 본문을 처음부터 재검증한다. [기존 watchdog 기록](../nov-12-native-cutover/tickets/NC-B-semantic.md#L687)
- **ANN 품질 실패.** 기존 LanceDB 실행의 `correlated_gt10pct`는 적격 2,048개에서 raw 8/10·파일 506/640이었다. 같은 seed·필터에서 exact 상위 owner가 속하는 리전과 8개 리전의 실제 방문·미처리 사유, 리전 내부 후보, 1,024 owner 한도 도달 여부, 재채점과 파일별 중복 제거 결과를 대조해 최초 누락 지점을 찾는다. 이 fixture의 비어 있지 않은 구간은 8리전을 유지하고 라우터 상한도 8이다. 9번째 리전 제외 사례를 이 실패의 원인으로 인계하지 않는다. 이 경로의 Semantic 수리와 동일 ANN 본문 재검증도 기존 B 범위다. [선택/탐색 손실을 구분하는 회귀](../../../crates/quanta-index-semantic/src/native/embedding/tests/ann_product_tests/ann4k_tests.rs#L581)

## 종료 시 인계할 값

각 실행에 대해 run ID, full selector, 실제 본문 수·exit, 제품의 마지막 성공 phase, ANN 구간별 분자/분모를 기록한다. 원 실행기의 `source_before`, `resolution_before`, 실행 중 `source_drift`·`resolution_drift`를 사용해 두 실행의 공통 소스·의존성 일치를 별도로 판정한다. 변경 중 얻은 PASS도 실행 사실로 남기고, 공통 입력 불일치가 있으면 동일 후보 수용 여부를 별도로 적는다. 이전 통과 결과를 재사용할 때도 같은 비교를 적용하고, 변경의 영향을 받은 본문만 기존 큐에서 다시 실행한다.

Prefix 게시가 확인되고 ANN4K와 제품 4K가 모두 종료되면 `product_after_prefix.py`가 각 실행의 terminal과 공통 입력 대조를 `quality-pair.json`에 기록한다. 단독 `retry.py` 실행은 각 실행 기록을 남긴다. 최종 판정은 두 본문과 결합 결과를 [00의 종료 기준](00-final-audit.md)으로 대조한다.

원 실행 호스트에서 비교할 기록은 아래와 같다. 출력 원본은 캐시에 보존하고 두 본문의 최종 결과와 후속 작업을 기존 07.5에 한 번 반영한다. Root/F는 이 결과와 07.5에 별도로 명시된 reclaim·fallback 등 출하 증거를 결합한다.

| `~/Library/Caches/quanta-index/` 아래 위치 | 용도 |
| --- | --- |
| `ann4k-product-retry-qz01qq7p/PRODUCT4K.log` | 초기 SourceAdvance 거절, exit 100 |
| `ann4k-resume-20261117/ANN4K.log`, `ann4k-lancedb-e1a28m08/ANN4K.log` | 이전 ANN 통과와 후속 LanceDB 품질 실패 비교 |
| `ann4k-owner-budget-repair-uls30q9w/{ANN4K,PRODUCT4K}/state.json` | 수정 후보의 terminal과 공통 입력 대조. 결합 결과는 같은 경로의 `quality-pair.json`에 있다 |

## 기존 실행기 결과

- **ANN4K · VERIFIED.** `2a4843ca-a052-44e2-b98e-12d96e2073b3`에서 1 PASS·exit 0. 비어 있지 않은 필터 6개가 각각 파일 640/640·raw 10/10을 기록했다. 빈 필터는 0건이다.
- **제품 4K · FAILED.** `492227b4-a0d2-4863-9433-e69d6d85a674`에서 1 FAIL·exit 100. 초기 4,096개 소스는 18.258초에 게시됐다. 이후 materialization의 1,200초 공통 watchdog이 만료됐다. 진행 폴링은 완료 3,077건을 관측했고, 실패 직후의 비원자적 공개 상태 조회에서는 완료 3,490건, 대기 603건, claim 3건이었다. Provider는 고유 행 3,520개와 질의 0개를 기록했다. 초기 검색과 후속 증분·overlay·두 번의 재시작 검증은 이 본문에서 실행되지 않았다.
- **공통 입력.** 원 비교 함수로 시작 시점 공통 소스 1,071개와 resolver 입력 67개를 대조했으며 차이가 없었다. 제품 종료 기록에는 소스 경로 drift 204개와 resolver 경로 drift 2개가 있다. 결합 상태는 `quality_incomplete`다.
