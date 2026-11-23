# 07 · 엔진 SourceAdvance·Text·Product4K 후속

상태 **NOT_RUN**. 기존 포크 후보에서 먼저 엔진 실패를 진단·수정하고 같은 엔진 수정을 담은 순정 후보에서 다시 검증한다. 4K 진단·수정·수용은 기존 [B 원장](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md), 원본 Caddy·etcd SDK60과 Text Large는 기존 [TEXT 원장](../nov-12-native-cutover/tickets/NC-TEXT-build-final.md), 최종 제품 인수는 [07.5](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md)가 소유한다.

## 확정된 시작점과 비교 범위

| 범위 | 관측 사실 | 해석 한계 |
| --- | --- | --- |
| 기존 B Product4K | 초기 Source 4,096개를 18.258초에 게시했으나 owner materialization의 1,200초 제한에서 **FAILED 1/1·exit 100**. 진행 폴링의 완료 3,077건과 실패 직후 비원자적 상태 조회의 완료 3,490건·대기 603건·claim 3건은 서로 다른 관측이다. | 검색·overlay·두 번의 재시작은 이 실행에서 **NOT_RUN**. 128-owner prefix의 `CoverageSave`·WAL 비중을 후기 4K의 원인으로 확정할 수 없다. [기존 결과](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#기존-실행기-결과) |
| 별도 Root NoFeatures Product4K | 초기 `SourceAdvance`가 30.0018초에 `DeadlineExceeded`; operation Pending, provider 호출 0. `BuildPrepare`에서 취소를 관측했다. | 위 B 실행과 후보·실행이 달라 같은 병목이라는 증거가 없다. 최초 취소 줄은 `NativeTextPostings` 내부 비용 귀속을 입증하지 않는다. [원장](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#ann-재시작-후속-실행) |
| 독립 ANN4K | 이전 고정 입력은 1 PASS. | 포크 없는 현 후보의 ANN4K·tracing Off Product4K·후기 Record는 **NOT_RUN**. 기존 PASS는 새 후보의 검색 품질 증거가 아니다. [정식 oracle](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#고정한-실행과-판정) |
| Text 원본 SDK | 과거 동결 후보에서 Caddy·etcd Record와 4,096파일·두 재시작 Large 통과가 있었다. 이후 소스·빌드 입력이 바뀌었다. | R34 빌드 custody 수리의 compiler/test/SDK terminal과 원 512 MiB·60초·quiet Off는 아직 열려 있다. [TEXT 최신 경계](../nov-12-native-cutover/tickets/NC-TEXT-build-final.md) |

위 실행은 최초 실패 진단에 사용한다. 전환 비교의 기준선은 **엔진 수리를 포함한 포크 유지 후보의 실제 결과**다. 순정 후보와 같은 코퍼스·설정·계측 Off/Record 조건을 적용하고, 후보별 Q/S revision·Cargo lock·resolver·출하 feature/target·서명 설치 입력을 고정한다. 실행마다 source/config/binary hash, 정확한 selector, 본문 수·exit, 첫 실패 phase, Work 사용량·요청 기한, trace 완전성, 제품 입출력을 기록한다. 원본 결과의 재사용은 실제 변경 영향 경로와 입력 일치를 증명한 경우에 한한다. [기준선 비교](05-integration-and-release.md#엔진-기준선과-포크-전환-비교)

## 1. 현 후보의 최초 실패 경계 판정

1. B의 기존 `product_after_prefix.py` / `retry.py PRODUCT4K`와 단일 공용 Rust 실행 잠금, 기존 quiet-host admission을 사용한다. 기존 controller의 PID·child·terminal·잠금을 먼저 확인하고 미접수일 때 **한 건** 접수한다. Record 출력과 테스트 temp/state 볼륨의 공간을 캡처 상한과 함께 확인한다. 공간 부족·호스트 경합으로 본문이 시작되지 않으면 **BLOCKED/NOT_RUN**으로 분리한다. 새 후기 Record가 `ENOSPC` 때문에 본문 0건이었던 이력을 성공 증거로 승격하지 않는다. [B 입장](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#materialization-계측--성공-owner의-전-구간을-연결).
2. 기존 제품 fixture의 단일 초기 `SourceAdvance` **30초·읽기 100만**, `4,096×64/Q64/K10`, 전체 **1,200초**, `batch_limit=1`, `max_in_flight=1`을 유지한다. 테스트의 요청 예산은 [`budget()`](../../../crates/quanta-index-searchd-runtime/tests/native_product_process.rs#L1139), 본문은 [`product_native_ann_quality_4096x64_reopens_at_fixed_recall`](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L1923)이다. 진단 Record에는 기존 `QIDX_NATIVE_ANN_TRACE_DIR`와 명시적 파일 상한을 사용하고, 정식 수용은 tracing Off로 따로 실행한다.
3. 첫 daemon의 동일 qtrace 클록에서 `native-ingest`의 요청 `start_ns`·`end_ns`와 `closed-workers --window-ns START END`를 맞춘다. `--occupancy-only`는 닫힌 worker가 창에 걸친 경우의 관측 점유와 미분류 시간을 보여 주며 최소 완결 표본이 필요한 단계별 비용 보고를 대신하지 않는다. drop·truncation·열린 span·후기 표본 부족은 원인 미확정으로 표시한다. [`causal_cost_diagnostic.py`](../../../tools/benchmark/retrieval/causal_cost_diagnostic.py#L1109), [`causal_cost_worker_scopes.py`](../../../tools/benchmark/retrieval/causal_cost_worker_scopes.py#L147).
4. 초기 게시가 거절되면 아래 **SourceAdvance/Text** 분기, 게시 뒤 owner 완료가 기한을 넘으면 **owner pipeline** 분기로 간다. 게시·materialization이 통과하면 실제 첫 실패 phase인 검색·증분·overlay·재시작을 해당 daemon의 계측으로 진단한다. 첫 daemon trace가 이후 재시작 daemon의 시간을 입증하지 않는다. 원 본문을 분할하거나 예산을 늘려 통과시키지 않는다.

## 2. SourceAdvance/Text 분기

`SourceAdvance`가 먼저 실패한 **현 후보의 동일 operation**에서 durable Pending/terminal과 최초 거절 axis, 남은 기한·R/W/B/M, `BuildPrepare → LexicalBuild → NativeTextPostings`의 닫힌 wall·Work를 대조한다. [`NativeTextPostings` lane 39](../../../crates/quanta-index-lexical/src/native/text.rs#L13797)는 `term_dictionary_write::prepare(root.terms, ...)`와 `context.apply_stream(...)`을 하나의 span에 넣는다. 기존 prefix의 `map_apply/0`은 그 안의 자식 구간이다. 별도 Root의 30초 실패를 prefix 수치로 설명하지 않는다.

| 현 후보 동일 캡처의 결과 | 정확한 수정 접점과 채택 조건 |
| --- | --- |
| lane 39 전의 `BuildPrepare`나 `LexicalBuild`가 지배 | 해당 단계의 실제 반복 조회·복사·정렬·물리 쓰기를 owning 함수까지 추적하고 원 Work·객체 수·결과를 보존하는 최소 변경을 한다. Text의 기존 owner별 계산·cold term 경로는 [TEXT 원장](../nov-12-native-cutover/tickets/NC-TEXT-build-final.md)의 현행 소스 해시와 대조한다. 감으로 전체 토큰 파이프라인을 재설계하지 않는다. |
| lane 39가 지배하고 자식 시간만으로 구별 불가 | [`apply_postings`](../../../crates/quanta-index-lexical/src/native/text.rs#L13452)의 lane 39 안에서 [`term_dictionary_write::prepare`](../../../crates/quanta-index-lexical/src/native/text/term_dictionary_write.rs#L39)와 `context.apply_stream`의 두 닫힌 진단 자식을 분리한다. Core 진단 이벤트·daemon 인코더·Python ingest의 동작과 기존 trace 읽기를 함께 확인한다. 동일 요청의 합계 시간·Work와 drop을 대조한 뒤 큰 자식만 수정한다. |
| `apply_stream`이 지배 | 실제 [`context.apply_stream`](../../../crates/quanta-index-lexical/src/native/text.rs#L13804) 호출에서 term/owner 맵의 재열거·선택 page 읽기·쓰기 횟수를 계수한다. 인증된 page/owner 재사용 또는 불필요한 복사 제거가 측정될 때만 적용한다. 변경 전후 동일 posting·TF·positions·old view·재오픈과 원 청구량을 검사한다. |
| prepare가 지배 | term dictionary의 key/owner 검증, 기존 cursor·cache 조회, 봉인 바이트를 각각 측정한다. 기존 R30/R34 후속의 실제 코드와 원본 코퍼스에 맞춰 하나의 반복 비용을 줄인다. 충돌·순서·영속 readback·취소에서 같은 결과를 요구한다. |

진단 변경은 [`diagnostic_trace.rs`](../../../crates/quanta-index-core/src/diagnostic_trace.rs#L822), [`trace_recorder.rs`](../../../crates/quanta-index-searchd/src/app/trace_recorder.rs#L785), [`causal_cost_native_ingest.py`](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L249)에 필요한 필드만 연결한다. 기존 stage 번호/요금 또는 SDK 예산을 조용히 바꾸지 않는다. 먼저 같은 실패를 집중 회귀로 재현하고, 수정 뒤 같은 본문과 원본 Caddy·etcd를 재실행한다.

## 3. owner pipeline 분기

초기 게시가 성공한 현 후보의 **같은 캡처**에서 owner별 `ready 발견 → assignment/enqueue → grant → worker 실행 → Catalog 게시 → pump 정산`을 연결한다. operation·claim fence·dispatch ordinal과 결과 `ViewRef`로 재시도를 별도 표본으로 세고, worker 종료와 공개 `WorkStatus` 완료를 구별한다. 이미 있는 [`native_work_diagnostic.rs`](../../../crates/quanta-index-searchd-runtime/src/native_work_diagnostic.rs#L1129), [`native_pump.rs`](../../../crates/quanta-index-searchd-runtime/src/native_pump.rs#L934), [`native_worker.rs`](../../../crates/quanta-index-searchd-runtime/src/native_worker.rs#L1042), [`Kernel::execute_owned_work`](../../../crates/quanta-index-kernel/src/update.rs#L2241), [`Catalog::prepared_work`/`stage_work`](../../../crates/quanta-index-catalog/src/native/authority.rs#L4355)를 사용한다. Worker의 중복 `status()` 조회 제거는 이미 구현된 후보이므로 재도입하지 않고 현 후보에서 실제 효과를 측정한다.

후기 창에서 backlog·완료율·재시도, queue, provider, reserve/install/confirm, `CoverageSave`, Catalog lock, WAL sync/backfill, ANN seal, CAS·정산의 **겹침을 제거한 시간**을 구한다. 초기 128-owner prefix의 Coverage/WAL 관측과 603 Pending은 진단 우선순위일 뿐 후기 병목의 증명이 아니다. 같은 capture에서 다음 중 **지배하는 단일 슬롯 비용 한 곳**을 고른다.

| 실측 지배 구간 | 최소 코드 변경과 회귀 |
| --- | --- |
| ready 이후 슬롯 공백·정산 | [`native_pump.rs`](../../../crates/quanta-index-searchd-runtime/src/native_pump.rs#L1177)와 Catalog [`owned_work.rs`](../../../crates/quanta-index-catalog/src/native/authority/owned_work.rs#L740)의 지배 조회만 줄인다. 완료 재조회는 해당 grant의 원자 게시·park 영수증이 전달될 때만 제거한다. due-retry, ACK 유실, 재시작, quiescence를 유지한다. |
| `CoverageSave`/설치·동기화 | [`Kernel` coverage 저장](../../../crates/quanta-index-kernel/src/update.rs#L4415)의 COW·reserve/install/confirm, Catalog publish와 실제 WAL sync를 분리한다. 객체/commit 수 또는 bytes가 감소하는 최소 호출만 수정한다. 동일 coverage root, 영수증, crash 중단·재개를 검증한다. |
| WAL backfill/main sync | [`connection.rs`](../../../crates/quanta-index-catalog/src/connection.rs#L114)의 WAL·reader pin·reclaim 한도와 실제 I/O를 대조한다. checkpoint 분리가 WAL sync 자체를 줄이지 못하면 채택하지 않는다. `WAL/FULL`·내구성·기존 Root 용량 정책을 보존한다. |
| provider/준비 | [`Kernel` artifact 재사용](../../../crates/quanta-index-kernel/src/update.rs#L4152)의 owner·input·instance·recipe·prior 결속을 유지하고 중복 provider 호출·직렬화·저장을 줄인다. stale prior, claim 만료, stage 뒤 종료·재개를 검증한다. |
| ANN seal/carry | [`ann_materialization.rs`](../../../crates/quanta-index-semantic/src/native/embedding/ann_materialization.rs#L196)와 [`consolidation.rs`](../../../crates/quanta-index-semantic/src/native/embedding/ann_directory/consolidation.rs#L650)의 누적 쓰기와 검색 가시성을 먼저 측정한다. 게시 임계 경로에서 seal을 빼는 설계는 미인덱싱 범위의 정확한 검색, 필터·삭제·교체·old view, 독립 ANN4K를 증명한 별도 작업에서만 채택한다. |

원 fixture는 단일 scope 슬롯이다. `batch_limit`/`max_in_flight` 상향, 새 준비 큐, claim 선점 병렬화, 한도 증가로 기존 실패를 가리지 않는다. 기존 B 문서의 제한된 준비·순서 게시 설계는 별도 자원 프로필을 갖춘 후속 구조 작업이다. [B 판단표](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#측정으로-고를-단일-슬롯-수정).

## 4. 실행과 완료 판정

| 순서 | 기존 명령·selector | 판정 |
| --- | --- | --- |
| 영향 집중 회귀 | `just rust-test-native-ann-read`, `just rust-test-native-ann-runtime`, `just rust-test-native-ann-daemon-smoke`; 변경 경계의 Catalog/Kernel 회귀는 `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline`에 정확한 `-p`, `--lib`/`--test`, `-E 'test(=...)'`를 붙인다. 예: [`owned_entry_validation_honors_deadline_when_catalog_is_held`](../../../crates/quanta-index-catalog/src/native/authority.rs#L6841), [`pre_execution_lookup_cancellation_keeps_its_error_after_a_prior_denial`](../../../crates/quanta-index-kernel/tests/native_projection.rs#L877). | 실제 본문·exit와 fixed gold를 기록한다. 집중 PASS는 제품 PASS가 아니다. |
| 독립 ANN4K | `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib --no-tests fail --test-threads 1 -E 'test(=native::embedding::tests::ann_product_tests::ann4k_tests::native_ann4k_selected_file_recall_matches_independent_exact)'` | 전체 및 비어 있지 않은 각 필터 Recall@10 ≥0.95, raw·파일 독립 gold, 1 PASS·exit 0. [본문](../../../crates/quanta-index-semantic/src/native/embedding/tests/ann_product_tests/ann4k_tests.rs#L397) |
| 정식 Product4K | 기존 B controller의 `retry.py PRODUCT4K`로 tracing Off [`ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall`](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L1923)를 한 번 실행한다. 직접 선택자가 필요하면 `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process --no-tests fail --test-threads 1 -E 'test(=ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall)'`를 기존 admission 안에서만 사용한다. | 단일 게시, owner 완료, 검색/Recall, 첫 재시작, 교체·삭제, overlay 종료·durable 결과, 두 번째 재시작·Lance 파일셋까지 1 PASS·exit 0. ANN4K와 같은 source/resolver 입력 및 실행 중 drift 0을 확인한다. |
| 원본 Text SDK60 | 기존 TEXT release/SDK runner의 **Caddy 590·etcd 1,421** Record와 corpus당 quiet Off 두 회. 파일 30,000·Work 512 MiB·R/W/M 각 100만·SDK 60초를 유지한다. 성공한 Record는 `python -m tools.benchmark.retrieval.causal_cost_diagnostic native-corpus-record --run-dir <output-root>`로 최초/재오픈 trace를 확인한다. | 각 corpus FULL ACK, 검색·재오픈, 완전한 trace와 같은 daemon의 quiet Off. Product4K의 30초 SourceAdvance·1,200초 전체 fixture와 **별개**다. [TEXT 원 계약](../nov-12-native-cutover/tickets/NC-TEXT-build-final.md#original-input-and-product-qualification-checks) |
| 같은 daemon Large | `QIDX_NATIVE_LARGE_RELEASE_BIN`/`QIDX_NATIVE_LARGE_RELEASE_SHA256`을 해당 TEXT daemon에 고정하고 `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --test native_product_process --no-tests fail -E 'test(=large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks)' --test-threads 1` | 4,096파일·두 프로세스 재시작 1 PASS. 별도 큰 Work/기한을 사용하는 본문이며 SDK60을 대체하지 않는다. [본문](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/large_restart.rs#L446) |

후보가 처음 실패한 단계와 실제 수정 영향 범위를 B/TEXT/07.5에 기록한다. 새 의존성 그래프의 SQLite·Lance 동작 차이는 [02](02-sqlite-and-storage-limits.md)·[03](03-lance.md)의 원자 게시·취소·복구·파일셋 검사와 함께 평가한다. 기존 통과 후보와 포크 없는 후보의 latency·Work·RSS·물리 사용량 비교는 같은 입력·운영 조건에서 수행하고 [E07](../oct-22-finalizing/E07-performance-operations.md)의 50 QPS, p95/p99, freshness·용량·Off/Record 조건을 별도로 닫는다. 모든 실행이 끝나기 전에는 코드 수정·컴파일·진단 PASS를 최종 제품 수용으로 기록하지 않는다.
