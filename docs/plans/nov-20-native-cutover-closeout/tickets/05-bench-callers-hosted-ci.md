# 05 · 벤치·공개 호출자와 hosted CI 전환

**잔여 인계 · 11월 22일 01:34 KST.** Native warm/cold 코드·공식 레지스트리 및 Harness 외부 프로세스 이관은 인수한다. 작성 패킷은 기존 S caller의 Metadata10·Structural2 실제 SDK/API/MCP/CLI 및 gold, Harness의 승인 A/B provider·입력과 Legacy scale 38건 disposition, Hosted의 승인 old-Q 공급이다. 실행은 집중 Native DSL 본문→공식 33행 warm **200**·cold **20**, 유료 A/B, 동일 후보 hosted job을 각각 기존 권위에 남긴다. [실제 호출](#정식-dsl과-실제-제품-호출) · [CI 입력](#hosted-공급)

**담당** 기존 Harness/벤치, Semantica/SDK caller, CI 공급 오너. Root/F가 공용 recipe·registry·CI 합류와 실제 실행을 담당한다.

**11월 22일 현행 상태.** Harness의 `legacy-bench`과 인프로세스 Harness·scale writer를 제거했다. 공식 warm/cold·scale 및 DSL scenario truth는 Native/공용 fixture를 사용한다. 이전 기준과 비교할 때는 고정 외부 old-Q 실행 파일을 사용한다. old-Q 호환 테스트가 남아 있는 CircleCI 공급을 유지했다. 이 변경의 Hosted 실행과 공식 Linux DSL 실측은 **NOT_RUN**이다. [현행 Harness Cargo](../../../../crates/quanta-index-searchd-harness/Cargo.toml) · [Hosted 공급](#hosted-공급)

## OpenAI A/B와 외부 비교

- [ ] **05.1** **A/B 외부 프로세스 이관·선택 회귀 VERIFIED** `src/bin/relevance_openai_ab.rs` → `run_openai_semantic_ab_report` → `relevance/process.rs::run_openai_ab` 연결을 로컬 `main`에서 확인했다. 두 arm의 Lexical·Semantic corpus/query, 고정 generation/retained identity, 요청별 표본·최댓값·distinct miss의 실제 대조는 05.5에서 확인한다. [바이너리 진입점](../../../../crates/quanta-index-searchd-harness/src/bin/relevance_openai_ab.rs#L38) · [A/B 구현](../../../../crates/quanta-index-searchd-harness/src/relevance/report.rs#L654) · [외부 프로세스 helper](../../../../crates/quanta-index-searchd-harness/src/relevance/process.rs#L235)
- [ ] **05.2** **A/B 공용 등록·선택 본문 VERIFIED** Root가 A/B의 `legacy-bench` 필수 등록을 제거했다. NoFeatures의 A/B 본문과 기존 JSON/Python 캡처·`benchctl` 검증을 완료했다. 이관된 concurrency/freshness/open-loop/relevance/tail 본문의 수용은 해당 벤치의 원 등록을 따른다. [Harness 등록](../../../../crates/quanta-index-searchd-harness/Cargo.toml#L87) · [기존 캡처](../../../../tools/benchmark/relevance_openai_ab_capture.py#L1083)

Harness 오너의 두 외부 프로세스·SDK 경로를 인수했다. `cargow metadata --no-deps --no-default-features`에서 `relevance_openai_ab` 타깃의 필수 기능은 0개, Harness NoFeatures normal/build 트리 1,967행에서 Tantivy와 Runtime crate는 0행이다. `just rust-test-authority`도 exit 0이다. 현재 로컬 `main`의 `d8a490565`에서 NoFeatures 컴파일과 선택 Rust 본문 20건은 **VERIFIED**다. 승인 입력과 고정 데몬을 사용하는 05.5 유료 A/B 실측은 **BLOCKED**다. [실행 기록](#검증)

## 정식 DSL과 실제 제품 호출

- [ ] **05.3** **Native 12행 유지·본문 대기** 공식 33행에서 Native 진단 21행을 재사용한다. **RuntimeMetadata 10·Structural 2**는 `SearchDslV1` 게시·조회와 독립 gold에 결속했다. 실제 API/MCP·SDK·CLI 및 Semantica 생산·소비 경로의 수락은 아래 호출과 본문 영수증을 대조한다. 기존 History·Lexical·Semantic·Symbol·RepoMap·seed/cluster/explain·diagnostic·구 DTO의 미분류 실제 호출은 기존 공급 목록에서 확인한다. [공개 경계](../../nov-12-native-cutover/TANTIVY-RETIREMENT.md#브레이킹-변경과-유지할-계약)
- [ ] **05.4** **Root의 생산자 전환.** 행별 판정·gold·집중 공개 본문을 인수해 `scenarios.rs`, warm/cold 실행기·collector, `registry.toml`·`Justfile`·CI 변경을 06 삭제 후보에 함께 합류한다. 현재 공식 Legacy 33행은 전환 전 비교 입력으로 고정한다. 최종 공식 권위는 05.6의 동일 후보 실측을 통과한 결과에 부여한다. [warm 실행기](../../../../crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs) · [cold 실행기](../../../../crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs) · [공식 등록](../../../../tools/benchmark/registry.toml#L617) · [06 합류](06-tantivy-retirement.md#작성-순서)

11월 21일 현재 Q 작업 트리에 Native DSL 게시와 retained 조회가 연결돼 있다. SDK는 Source에 결속된 projection을 게시하고 `SearchDslV1`을 UDS로 보낸다. Runtime은 기존 read ticket 아래 Structural·RuntimeMetadata를 조회한다. Q의 12행 독립 gold와 다른 view cursor 거절 회귀가 소스에 있다. 실제 CLI 프로세스 회귀는 같은 12행을 사용해 `native publish-dsl`의 `Published` view와 12개 조회의 SDK·CLI 페이지를 대조한다. [SDK 게시·조회](../../../../crates/quanta-index-sdk/src/native_uds/dsl.rs#L269) · [Runtime retained 조회](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/dsl.rs#L44) · [CLI 프로세스 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/dsl.rs#L336)

Root는 [01](01-q-regressions.md)의 Q·Catalog·Runtime 후속 종료 뒤 동일 소스와 외장 캐시를 사용하는 직렬 작업에서 `just rust-test-native-dsl-cli`를 실행한다. 정식 release CLI 빌드의 Cargo 결과에서 실제 실행 파일 하나를 찾아 절대 경로와 SHA-256을 Rust에 전달한다. `native_index_uds`의 `read_sessions::dsl::native_dsl_twelve_retained_rows_match_pinned_cli_v1` 하나를 선택한다. 테스트는 private 임시 사본을 실행하고 전후 SHA를 검증한다. 실행 파일 선택·공백 경로·호출 실패 전파의 격리 recipe 검사는 **5/5 VERIFIED**다. 실제 Q 컴포넌트 회귀, S 실제 API/MCP·SDK·CLI 호출자별 수용, 공식 33행 warm/cold recipe 전환은 각각 **NOT_RUN**이다. [실행 recipe](../../../../Justfile#L350)

### 05.3 호출자와 독립 gold

사용자 판정에 따라 RuntimeMetadata 10행과 Structural 2행을 모두 Native에 유지한다. 공개 조회 계약은 정확한 retained view의 `SearchDslV1`이며, SDK 생산자는 같은 Scope와 Source에 결속한 projection을 게시한다. CLI는 `native publish-dsl`, `native runtime-metadata`, `native structural`을 사용한다. [Q SDK](../../../../crates/quanta-index-sdk/src/native_uds/dsl.rs) · [CLI](../../../../crates/quanta-index-searchctl/src/native/dsl.rs)

| 공식 행 ID | 판정 | 독립 gold |
| --- | --- | --- |
| `runtime.dirty_no.sourcegraph` | 유지 | `e2e-1-src/clean.rs` |
| `runtime.changed.sourcegraph` | 유지 | `e2e-3-src/changed.rs`, `e2e-4-src/stale.rs` |
| `runtime.stale.sourcegraph` | 유지 | fixture 후보 1–9 전체, candidate ID 오름차순 |
| `runtime.snapshot.sourcegraph` | 유지 | `e2e-3-src/changed.rs`, `e2e-9-src/snap.rs` |
| `runtime.meta_owner.sourcegraph` | 유지 | `e2e-5-src/owner.rs`부터 `e2e-8-src/surface.rs`까지 네 facet 후보 |
| `runtime.meta_service.sourcegraph` | 유지 | 동일 네 facet 후보 |
| `runtime.meta_layer.sourcegraph` | 유지 | 동일 네 facet 후보 |
| `runtime.meta_surface.sourcegraph` | 유지 | 동일 네 facet 후보 |
| `runtime.affected.sourcegraph` | 유지 | `e2e-3-src/changed.rs` |
| `runtime.invalidated_by.sourcegraph` | 유지 | `e2e-3-src/changed.rs` |
| `structural.tree_match.native` | 유지 | `e2e-5-src/tree.rs`, `name`의 파일 기준 UTF-8 바이트 `14..33`, 2행 |
| `structural.tree_match.sourcegraph` | 유지 | 같은 후보·캡처·범위 |

Q의 기존 UDS gold는 실제 Source·projection 게시 후 독립 기댓값을 일반 SDK, Original admission SDK, CLI 라이브러리 응답과 대조한다. 기존 cursor 회귀는 같은 Source 재인증으로 이전 카탈로그를 거절하고, 원 view의 continuation과 응답 backing 반환을 검증한다. [Gold와 selector](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/dsl.rs)

- Semantica Structural은 `SearchBuilderV2::execute_native_structural_with_original_v3` → 검색 어댑터 → Lane 4 → source-bound 포트 → Q retained session으로 호출한다. 호출자가 발급한 키·budget·memory와 Native/Sourcegraph 문법을 전달한다. 기존 연결 회귀는 동일 Sourcegraph 요청이 정확한 view에서 캡처를 반환하고 세션을 해제하는지 검증한다. [SDK 호출](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder_execute_axis.rs) · [포트와 호출 회귀](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_query_ports/native_connector_v3.rs)
- 제품 SDK의 `execute_published_native_structural_v3`는 게시 읽기 lease와 Original을 이용한다. Native Python 청크는 선언부터 본문까지 하나의 Source span을 Lexical 증거와 Structural AST에 전달한다. 기존 Linux 서명 설치 본문에서 Native와 Sourcegraph 문법으로 선언 식별자 `4..20`와 반환 정수 `35..37`, exact view, 세션 해제를 확인한다. [SDK 진입점](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/runtime_executor.rs#L1402) · [설치 본문](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/query_handlers/search_handler.rs#L683)
- RuntimeMetadata 운영 공급은 `ensure_native_indexed_with_runtime_metadata_v3`에서 내구 FULL ACK 이후 서명 설치 peer와 같은 Original을 빌려 게시한다. 배포자가 실제 카탈로그 및 dirty 사실을 제공한다. `execute_published_native_runtime_metadata_v3`는 RepoState가 선택한 Acked Source operation·view와 일치하는 카탈로그 receipt를 확인하고 그 exact catalog view에서 조회한다. 같은 Linux 서명 설치 본문에 실제 Structural 결과의 chunk ID를 공급해 게시·조회·해제를 대조하는 단언을 추가했다. [공급 및 조회 권위](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder/index_owner_env_authority/aggregate_publication_authority/native_runtime_metadata_v1.rs)
- Structural은 원 Source 게시의 SourceCut·profile과 현재 CatalogView의 digest를 대조한다. 검증된 exact view에서만 projection과 세션을 연다. 같은 SourceCut의 다른 profile은 Structural lookup 전에 거절하고 기존 Lexical profile 판정을 유지한다. 같은 DSL 게시 경로는 다른 profile의 terminal view를 거절한다. [Exact Structural 조립](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_query_ports/native_structural_v1.rs#L38) · [프로필 불일치 회귀](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_query_ports/native_connector_v3.rs#L569)
- 현행 MCP `search`는 Symbol, `search_code`는 Hybrid로 dispatch한다. 두 등록 입력과 generated SDK 어댑터에서 이 12행을 호출하는 MCP 경로는 확인되지 않았다. Semantica의 기존 full-loop 회귀는 `GenerationPin` 기반 Structural·RuntimeMetadata DTO를 사용한다. 그 회귀는 이 Native 12행의 본문 분모에 포함하지 않는다. [MCP Symbol](/Users/songmin/Documents/code-new/semantica-codegraph-v2/apps/mcp/mcp/handlers/search.py#L50) · [MCP Hybrid](/Users/songmin/Documents/code-new/semantica-codegraph-v2/apps/mcp/mcp/handlers/usecase_ai_tools.py#L613) · [기존 full-loop 호출](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/source_bound_index_sdk_full_loop_contract/assertions.rs#L1330)

Q SDK 게시 준비는 dirty 중복과 카탈로그 digest를 검사한다. Q 데몬은 Submit·Resume 공용 커밋 경계에서 현재 Source의 corpus certificate·세대와 모든 카탈로그/dirty chunk ID를 검증한다. 기존 UDS 두 본문에 잘못된 세대와 외부 chunk ID의 `Refused(InvalidInput)` 영수증, Resume 일치, 최신 view 불변을 추가했다. [게시 검증](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/dsl.rs) · [기존 UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/dsl.rs#L280)

페이지 요청마다 retained view·projection·cursor를 재검증한다. 일치하는 범위의 corpus 한 건을 기존 resource ledger로 보관하며, 보관 한도를 넘거나 다른 범위를 읽으면 Source에서 다시 검증한다. RuntimeMetadata 참조는 기존 chunk map에서 검사한다. 실제 warm/cold 시간은 기존 실행기가 비교한다. [corpus 재사용](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/dsl.rs)

기존 05 큐의 정규 32본문은 Q daemon 2(독립 gold 12행 포함), Q SDK 9, S runtime 17, S retrieval-kernel 4다. 별도 1본문씩 있는 기존 Linux 서명 설치 suite와 `just rust-test-native-dsl-cli`도 같은 입력 갱신에 결속했다. 현행 통과 영수증은 정규 **0/32**, 서명 설치 **0/1**, release CLI **0/1**이다. 기존 큐가 사용하는 외장 볼륨 이탈로 본문 입장을 대기 중이다. Rust 파싱, `git diff --check`, Q `just rust-test-authority`, Linux 설치 suite `--resolve-only`는 통과했다. [기존 05 큐](/Users/songmin/.codex/artifacts/semantica-runtime-queue-01a1f9da/QUEUE.md#L35) · [입력과 선택자](/Users/songmin/.codex/artifacts/semantica-runtime-queue-01a1f9da/inbox/01a1f99e-db8d-70b3-8f79-8321a3a2749a-native-dsl-gold-137fba74-original-callers.md) · [외장 볼륨 이탈 기록](/Users/songmin/.codex/artifacts/semantica-runtime-queue-01a1f9da/receipts/c35-infra-interrupted-volume-loss.json)

11월 21일 cold 수집기가 조립 프로세스의 무출력 성공·깨진 JSON을 통과시키거나, 실패 전에 쓴 일부 출력으로 기존 결과를 덮는 세 가지 경우를 재현했다. 같은 디렉터리의 임시 경로에 조립하고 기존 `BenchArtifactV1` 검증기로 검사한 cold 결과만 원자적으로 게시한다. 신규 회귀는 수정 전 **3 FAILED**, 수정 후 DSL·CI·`benchctl` 범위 **219/219 VERIFIED · exit 0**이다. 실제 공식 Native 측정은 **NOT_RUN**이다. [cold 산출물 게시](../../../../tools/benchmark/run_dsl_cold_matrix.py#L210)

Root는 05.3이 넘긴 Metadata 10·Structural 2를 기존 Native warm/cold 실행기에 연결했다. 각 pass는 별도 데몬과 Source 및 projection 게시를 준비하고, 공개 SDK에서 exact view에 대해 질의한다. 조립기는 33행 전부와 실행 파일·fixture·설정·exact query·관측 페이지를 대조한다. 누락 retained 행과 다른 view 회귀는 수정 전 **2 FAILED**, 수정 후 수집기·비교기 Python **85/85 VERIFIED · exit 0**이다. Rust 집중 본문과 실제 SDK 시간은 기존 큐에서 확인한다. 공식 Linux 비교와 CI 권위 전환은 **NOT_RUN**이다. [Native retained 실행기](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_retained.rs) · [33행 조립기](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_artifact.rs)

공식 DSL 비교기는 기준선과 현행 결과 양쪽의 `qualification=diagnostic`을 거절한다. 같은 출처·설정인 진단 결과를 통과시키던 회귀 두 건을 재현했다. 수리 후 DSL 수집·비교, capability, `benchctl` Python **200/200 VERIFIED · exit 0**이다. [비교기](../../../../tools/benchmark/compare_dsl_bench.py#L411)

공식 warm/cold 기준선 발행에서 cold 교체 뒤 디렉터리 동기화가 실패하면 warm만 복원되고 새 cold가 남았다. 두 발행이 교차하면 뒤 발행의 실패 복원이 앞 발행의 성공을 덮을 수도 있었다. 배타 발행 표식을 획득한 뒤 이전 파일을 읽고 두 교체를 기록한 뒤 디렉터리를 동기화하도록 변경했다. 기존 pair와 최초 발행의 동기화 실패, 선행 발행 직후 후속 발행 실패의 회귀가 수정 전 **3 FAILED**, 수정 후 **3/3 VERIFIED**다. `test_benchctl.py`와 `test_compare_dsl_bench.py` 전체는 **170 PASS · 2 FAIL**이며 두 실패는 동시 P04 변경 중 발생한 `P04_WITNESS_FILES` import 오류다. 해당 오너 수정 후 실패한 두 선택만 재검증해 **2/2 VERIFIED · exit 0**을 확인했다. 공식 Native warm/cold 실측은 **NOT_RUN**이다. [기준선 발행](../../../../tools/benchmark/benchctl.py#L432) · [실패 주입](../../../../tools/ci/tests/test_benchctl.py#L854)

05.4 Native DSL 자원 계측은 Text·History 재오픈·retained가 프로세스를 회수하기 전에 Linux `VmHWM`을 읽는다. 구 비교 실행과 동일한 파서를 사용한다. 조립기는 데몬을 사용한 모든 보고서에 유효한 값이 있을 때만 최대치를 데몬 RSS로 기록한다. SDK 로컬 오류는 데몬 값 없이 유지하고, 누락된 보고서가 있으면 조립 프로세스 범위를 명시한다. 실제 Rust 파서·집계 로직의 격리 시험은 **2/2 VERIFIED · exit 0**이며, 전체 Harness 타깃·33행 RSS 회귀·Linux 실제 계측은 **NOT_RUN**이다. 공식 Native 실행기 입장과 warm/cold 권위 전환도 남아 있다. [공용 Linux 관측](../../../../crates/quanta-index-searchd-harness/src/artifact.rs#L635) · [집계](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_artifact.rs#L708)

05.4 공식 `dsl-authority`의 registry와 `rust-bench-dsl-refresh`를 기존 Native 33행 수집기로 연결했다. 같은 동결 checkout의 warm/cold 실행기, 데몬, Published History fixture를 `--dsl-native-plan`의 경로·SHA로 고정한다. Quiet Linux preflight 아래 두 실행을 직렬로 수집하고 각각 절대 출력 경로·HEAD·표본 하한·계측 범위·입력 결속을 검증한 뒤 현행 결과 pair를 게시한다. Rust 조립기는 33행 gold와 행별 warm 200/cold 20 이상, 모든 데몬 보고서의 실제 RSS, Linux host를 요구한다. Native 기준선·비교는 이 권위 계약을 요구하며 old-Q 비교는 기존 별도 용도로 유지한다. 현행 제어·registry 집중 **42/42**, 수집기 **16/16**, 비교기 **102/102 VERIFIED**다. 승인 빌드의 실물 plan 공급, 전체 Rust 타깃 및 canonical Linux warm/cold는 **NOT_RUN**이다. [registry](../../../../tools/benchmark/registry.toml#L78) · [결과 게시](../../../../tools/benchmark/benchctl.py#L764) · [운영 입력](../../../../tools/benchmark/README.md#dsl-warmcold-gate)

기존 History 실명 producer를 `just rust-prepare-native-dsl-history`에 연결했다. 승인 daemon SHA와 clean Q HEAD를 확인하고, 실제 warm 조회와 fresh daemon 조회를 마친 뒤 `source_git_head`를 포함한 fixture를 게시한다. 공식 warm이 cold 표본 입장 조건에 항상 걸리던 결함도 수리했다. 두 모드는 HEAD 누락·불일치를 실행기 기동 전에 거절한다. 회귀는 수정 전 **5 FAILED**, 수정 후 수집기·비교기 **122/122**, `benchctl` DSL 선택 **25/25 VERIFIED**다. Rust 파일시스템 헬퍼 격리 검사 **2/2**, Just 선택자 dry run 및 Rust 구문 검사도 통과했다. 승인 Linux 입력의 실제 fixture 생산과 공식 warm/cold 실측은 **NOT_RUN**이다. [History producer](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history/dsl_fixture.rs) · [두 모드 수집기](../../../../tools/benchmark/run_dsl_cold_matrix.py#L435) · [실행 경로](../../../../tools/benchmark/README.md#dsl-warmcold-gate)

집중 Rust 명령 `cargow --lane test-workspace-lane nextest run --release --locked --offline --no-default-features -p quanta-index-searchd-harness --lib`의 `dsl_artifact::tests::`, `dsl_retained::tests::`, `native_dsl_roster_` 선택은 첫 420초와 후속 900초 모두 공용 빌드 잠금에서 **BLOCKED · exit 124 · 본문 0**이다. TEXT의 선행 빌드에서 확인한 `LanguageCode::new` 오류 변환은 기존 `anyhow::Error::msg` 패턴으로 수리한 현행 소스를 인수한다. `rustfmt --check`, `just rust-module-cycles` 및 변경 파일의 `git diff --check`는 **VERIFIED**다.

공식 warm/cold 임시 결과에서 `concurrency=0` 또는 Native matrix가 빠진 Legacy 형식이 현행 결과 쌍을 덮어쓰던 경로를 재현했다. 기존 BenchArtifact 검증기와 Native matrix 조건을 게시 전에 적용한다. 현재 파일에 같은 실행의 staged SHA를 다시 대조하고, 기준선 승격은 읽은 바이트에 그 SHA를 요구한다. 성능 회귀 시 유효한 측정 결과를 보존한다. 게시 반례는 수정 전 **2 FAILED**, 수정 후 기능 관련 `benchctl`·artifact 검증 **78/78 VERIFIED**다. 전체 두 Python 파일은 **140 PASS · 4 FAIL**이며 네 실패는 별도 Native capture의 evidence-root symlink loop 처리다. 공식 Linux 실측은 **NOT_RUN**이다. [게시 검사](../../../../tools/benchmark/benchctl.py#L769) · [승격 검사](../../../../tools/benchmark/benchctl.py#L357)

21:38 KST 05.4 공식 `benchctl compare`에서 warm 비교가 끝난 뒤 다른 정상 발행이 baseline 또는 현행 warm/cold pair를 교체하면, 두 세대 결과를 한 번의 통과로 처리하던 문제를 수리했다. 두 비교를 시작하기 전에 네 입력의 파일 신원을 잡고 각 비교가 끝난 뒤 발행 표식과 파일 신원을 대조한다. 교체 감지 시 해당 결과를 거절하고 다음 비교를 시작하지 않는다. 실제 pair 게시를 끼워 넣은 회귀는 수정 전 **2 FAILED**, 수정 후 `test_benchctl.py`·`test_compare_dsl_bench.py`·`test_benchmark_manifest.py` **213/213 VERIFIED · exit 0**이다. Canonical Linux warm/cold 실측은 **NOT_RUN**이다. [비교 입력 검사](../../../../tools/benchmark/benchctl.py#L577) · [교체 회귀](../../../../tools/ci/tests/test_benchctl.py#L246)

21:57 KST `just rust-bench-dsl-warm`과 `just rust-bench-dsl-cold`를 제거했다. 두 Legacy 실행이 공식 결과 경로를 따로 덮을 수 있었다. `just rust-bench-dsl-compare`는 `benchctl compare dsl-authority`를 호출해 기존 pair 동시 교체 검사를 적용한다. 등록 회귀는 수정 전 **2 FAILED**, 수정 후 `test_benchctl.py`·`test_compare_dsl_bench.py`·`test_benchmark_manifest.py` **215/215 VERIFIED · exit 0**이다. 정식 Linux warm/cold 측정은 **NOT_RUN**이다.

11월 21일 `run_dsl_cold_matrix.py`의 작업 트리 Legacy 자동 빌드를 제거했다. Native 실행 입력 또는 보존 Legacy 실행 파일·checkout·commit·SHA와 명시 출력이 필요하다. 출력 경로만 지정한 회귀는 수정 전 자동 빌드와 실행에 들어가 **1 FAILED**, 수정 후 기존 결과를 유지하고 subprocess 실행 전에 거절한다. DSL 비교·Native 수집기·`benchctl`·manifest **237/237 VERIFIED · exit 0**이다. 정식 Linux DSL 실측은 **NOT_RUN**이다. [cold 실행기](../../../../tools/benchmark/run_dsl_cold_matrix.py) · [회귀](../../../../tools/ci/tests/test_compare_dsl_bench.py)

같은 변경에서 현행 Rust warm/cold 실행기의 Legacy 분기와 전용 소스 2개를 제거했다. Native 조회·History·retained·아티팩트 조립 인자는 유지하며 이전 입력은 기존 usage 오류로 거절한다. 두 Rust 파일의 `rustfmt --check --edition 2024`와 diff 검사는 통과했다. Rust 컴파일과 실제 SDK는 **NOT_RUN**이다. [warm 실행기](../../../../crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs) · [cold 실행기](../../../../crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs)

11월 21일 사용처를 대조해 `bench_support`의 Legacy 인프로세스 준비·질의와 `snippet`의 Legacy 실행·전용 테스트를 제거했다. 공식 snippet 실행은 Native SDK 관측을 같은 소스 기반 oracle로 판정한다. 고정 old-Q 비교는 외부 프로세스 SDK를 사용한다. Rustfmt·diff 및 `cargo metadata --no-deps --locked --offline`의 warm/cold/snippet 타깃 등록을 확인했다. Rust 본문은 **NOT_RUN**이다. [DSL 보조 코드](../../../../crates/quanta-index-searchd-harness/src/bench_support.rs) · [snippet](../../../../crates/quanta-index-searchd-harness/src/snippet.rs)

11월 22일 `searchd-harness-runtime-custody`의 세 본문을 `native_product_process::native_lifecycle`로 옮기고 기존 Legacy 파일·Cargo·CI 등록을 제거했다. Native DSL Text 실행은 기존 데몬과 루트를 함께 소유하는 세션을 사용한다. 명시 종료는 프로세스를 회수한 후 소유 루트를 삭제하고, drop도 같은 순서를 지킨다. 빌린 루트는 보존한다. 새 테스트는 Cargo가 등록한 실제 Native 데몬에서 공개 SDK로 Workspace를 만들고 exact view를 읽은 뒤 각 불변식을 검증한다. 테스트 본문은 **NOT_RUN**이다. [Native 세션](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/lifecycle.rs) · [실물 프로세스 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/native_lifecycle.rs)

동일 caller 상태 루트의 두 번째 기동이 앞 실행의 `create_new` 설정 파일에 충돌했다. 세션마다 짧은 별도 임시 경로에 소켓·설정·로그를 두고 자식 종료 뒤 정리한다. 같은 `state`를 열어 첫 기동의 exact view를 두 번째 기동에서 읽는 회귀를 기존 타깃에 추가했다. Rust 포맷·타깃 등록은 **VERIFIED**다. 현행 NoFeatures 재실행은 `test-workspace-lane`에서 `native_product_process`의 `test(/^native_lifecycle::/)` 4개를 선택했다. 공유 Cargo 잠금을 1,200초 기다린 뒤 **BLOCKED · exit 124 · 실행 0**으로 종료했다. [재시작 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/native_lifecycle.rs)

11월 22일 Legacy scale의 무호출 래퍼 6개를 제거했다. 소스·저장소가 digest에 반영되는지와 tier manifest 스키마를 검사하는 두 본문은 Native scale이 쓰는 공용 provenance 모듈로 옮겼다. 기존 단언을 유지했고 Legacy 전용 38건은 기존 오너의 본문 분류에 남아 있다. Rustfmt·diff·test authority 검사는 통과했다. Rust 컴파일 및 본문은 **NOT_RUN**이다. [공용 provenance 회귀](../../../../crates/quanta-index-searchd-harness/src/scale/provenance.rs#L121)

## 검증

11월 22일 유지 RuntimeMetadata 10행의 Native warm/cold 실행기와 집계기는 후보 ID·순서 외에 같은 fixture의 저장소, 경로, 행 범위와 snippet을 검증한다. 이전 검사는 후보 ID를 보존한 잘못된 응답 필드를 수용했다. 다섯 필드를 각각 바꾼 Rust 회귀를 추가했다. 응답 검증은 SDK 경과 시간을 읽은 뒤 수행한다. Rust 포맷 검사 **VERIFIED**, 신규 Rust 본문은 **NOT_RUN**이다. [검증 코드](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_retained.rs) · [회귀](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_artifact.rs)

Structural 2행도 전체 매치 범위와 소스 저장소를 고정 fixture에 대조한다. 후보 ID와 metavariable 좌표가 올바르더라도 저장소 또는 전체 범위가 달라지면 실행기와 집계기가 거절한다. 기존 변조 회귀에 두 경우를 추가했다. Rust 본문은 **NOT_RUN**이다. [검증 코드](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_retained.rs#L113) · [회귀](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_artifact.rs)

- [ ] **05.5** 고정 executable·원 checkout·요청/부하·provider 설정 및 승인 실행 입력으로 A/B 두 arm의 실제 결과를 비교한다. Source 게시/조회, 요청별 표본·최댓값·distinct miss·retained identity와 원 JSON을 기존 캡처에서 대조한다. 활성 provider 입력이 빠지면 해당 A/B 실측을 `BLOCKED`로 둔다. [A/B 실측과 입력 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md)

외부 데몬 A/B 캡처는 승인 입력의 데몬 실행 파일 SHA를 실행 전후에 대조한다. 결과에는 같은 G1 Source manifest, 별도 root incarnation, 각 arm의 lexical·semantic head와 정상 범위의 게시 HTTP 요청 수가 있어야 한다. `source_evidence` 누락·변조·잘못된 데몬을 수용하던 실패 10건과 크기 검사 전에 sidecar 전체를 읽던 실패 1건을 재현했다. 기존 보관본과 pin 누락·실행 중 데몬 교체를 포함한 캡처 및 `benchctl` Python **127/127 VERIFIED**다. 실제 provider A/B는 **NOT_RUN**이다. [캡처 검증기](../../../../tools/benchmark/relevance_openai_ab_capture.py#L491)

캡처의 OpenAI 설정에 전체 최대 배치 크기와 보존된 다중 입력 요청의 토큰 추정치를 대조한다. provider가 허용하는 단일 대형 입력은 수용한다. 캐시 비활성 실행은 캐시 관측 0과 실제 HTTP 요청을 요구한다. 요청·실패·재시도 수도 Rust 수집기와 같은 관계로 검사한다. 새 회귀 8건의 변경 전 결과는 7 실패·1 통과였고, 수정 후 전체 캡처·`benchctl`은 **135/135 VERIFIED**였다. [계측 검증](../../../../tools/benchmark/relevance_openai_ab_capture.py#L221)

OpenAI 데몬은 정식 설정 파서가 `QUANTA_INDEX_EMBED_REQUEST_SAMPLE_METRICS=true`를 받았을 때 상세 계측을 노출한다. 요청 수·누락 수·최댓값과 최근 256개 표본을 같은 스냅샷에 결속한다. 고정 `telemetry.rs`를 보존 NoFeatures Core·Contract와 직접 링크한 Rust 본문은 **4/4 VERIFIED**다. 256개 상한, 전체 최대치, 동시 기록 중 스냅샷 일관성, 기존 오류 계측을 확인했다. 설정·조립 회귀, 전체 데몬 빌드, 실물 SDK scrape와 유료 A/B 본문은 **NOT_RUN**이다. [계측 생산자](../../../../crates/quanta-index-embed/src/telemetry.rs#L108) · [데몬 등록](../../../../crates/quanta-index-searchd/src/app/runtime.rs#L1074)

별도 직접 링크 실행에서 현행 계측 Rust, 기존 `ObservabilityScrape`, 공개 metrics DTO의 JSON 직렬화·역직렬화와 현행 Harness 디코더를 이어 확인했다. 요청 257건 → 표본 256건·탈락 1건, 탈락한 최대치 보존, 설정 배치 한도 위반 거절을 확인했다. opt-in 플래그가 켜진 실제 데몬과 SDK 연결은 **NOT_RUN**이다. [Harness 디코더](../../../../crates/quanta-index-searchd-harness/src/relevance/process.rs#L451)

외부 데몬 A/B는 승인된 요청당 최대 비용과 총비용 상한이 모두 있어야 시작한다. Source·조회 HTTP의 동시 요청·입력 바이트·비용을 같은 원장이 제한한다. 두 단계의 성공 요청과 재시도를 각각 검산하고, 고정 판정 13건은 러너와 캡처가 같은 TSV에서 가져온다. [유료 실행 입력](../../../../tools/benchmark/README.md#archived-openai-ab-diagnostic) · [판정표](../../../../crates/quanta-index-searchd-harness/src/relevance/semantic_judgments.tsv)

캡처·`benchctl`·테스트 정책 Python **356/356 VERIFIED**. 호환 보존 의존성에 현행 원장·HTTP 전송·워커·계측 코드를 직접 링크해 Rust **72건**, Harness 계측 검증 **4건**을 실행해 통과했다. `cargow nextest`의 통합 SDK·러너 검증은 공유 잠금 대기 시간 초과로 **BLOCKED**이며 해당 실행의 테스트 본문은 0건이다. 승인 provider 입력과 계측을 지원하는 고정 데몬 지정이 없어 유료 A/B는 **BLOCKED**다. 실제 호출은 **NOT_RUN**이다.

현재 TSV를 사용하는 A/B는 `--print-inputs`가 출력하는 설정·Source 배치 승인 해시와 데몬 SHA를 요구한다. 러너는 데몬 시작 전에 설정 기본값과 SDK 배치 바이트의 해시를 확인한다. 캡처는 결과 게시 전에 두 해시를 다시 대조한다. 모의 캡처는 내부 해시도 다시 계산한 설정 기본값 변조와 형식이 올바른 다른 Source 배치 해시를 모두 거절했다. HTTP 워커는 전송 직전 취소·만료를 확인하며, 두 경우 모두 모의 HTTP 0회와 차감 0을 확인했다. [실행 입력 계약](../../../../tools/benchmark/README.md#archived-openai-ab-diagnostic)

이 변경의 `.venv/bin/python -m pytest -q` 캡처·`benchctl`·테스트 정책 범위 **352/352 VERIFIED**. 보존 의존성 직접 링크에서 승인 endpoint·원장·HTTP·워커·계측 **74건 통과, 1건 제외**, 승인 해시 골든·SDK Source 배치 결정성 **2/2 VERIFIED**. `cargow nextest`는 공유 잠금 대기 180초 후 124로 종료해 **BLOCKED**이며 통합 테스트 본문은 0건이다. 유료 A/B는 승인 입력과 데몬 고정값이 없어 **BLOCKED**이며 실제 호출은 **NOT_RUN**이다.

보존 in-process 실행은 사전 거부한다. `validate-frozen-openai-ab`는 기존 세 sidecar를 원 checkout과 grant로 검증한다. 현재 유료 캡처는 비밀키 없이 고정 러너의 `--print-inputs`를 실행해 설정·Source 승인 해시를 확인하고, 본 실행에만 키를 전달한다. 출력은 Linux/macOS의 원자적 교체 금지 rename으로 한 번 게시한다. 모의 실행에서 동시 생성된 빈 디렉터리·파일·심볼릭 링크를 보존했다. 구 러너 실행 재현 2건은 수정 후 프로세스 시작 전에 거절된다. [운영 계약](../../../../tools/benchmark/README.md#archived-openai-ab-diagnostic)

범위 한정 합류 후보는 `codex/openai-ab-closeout`의 `ff4df01cdd084dceafbe48a4784c807391f7dfd1`이다. 같은 clean checkout에서 `python -m pytest -q`로 `test_relevance_openai_ab_capture.py`, `test_benchctl.py`, `test_benchmark_manifest.py`, `test_check_test_authority.py`를 실행해 **355/355 VERIFIED**다. 보존 의존성 직접 링크는 원장·HTTP·워커·계측 74건, 비밀키 없는 설정 1건, 승인 해시와 Source 배치 2건을 통과했다. `cargow tree --locked --offline -p quanta-index-searchd-harness --no-default-features -e normal,build`의 Tantivy·Legacy Runtime 의존은 0개다.

공유 작업 트리와 clean 후보의 정식 `cargow nextest`는 각각 공유 잠금 대기 360초와 300초 후 exit 124로 끝났다. 두 실행 모두 Rust 테스트 본문 0건으로 **BLOCKED**다. 공유 작업 트리에서 진행 중인 Root 실행과 공용 파일 인수 뒤 아래 선택자를 사용한다. 유료 A/B는 승인 입력과 고정 데몬 공급이 없어 **BLOCKED**이고 실제 호출은 **NOT_RUN**이다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features \
    -p quanta-index-searchd-harness -p quanta-index-embed -p quanta-index-searchd \
    --lib --bin relevance_openai_ab --no-tests fail \
    -E 'test(openai_ab) | test(approved_openai) | test(interruption_at_http_dispatch) | test(openai_egress_refuses_wrong_endpoint) | test(openai_cost_ceiling_is_shared) | test(openai_retry_ceiling_stops)'
```

후속 점검에서 같은 건수의 다른 질의는 이전 미리보기 승인을 통과하고 결과 검증에서만 거절됐다. 후속 커밋 `0dec0baf7f73d5a1aee7bb72cb033486d9500f40`은 설정·Source 배치·판정표를 같은 승인 해시에 결속하고, 미리보기의 판정표 해시를 원 checkout과 대조한 뒤 키를 전달한다. Rust 러너는 새 staging 디렉터리를 확보한 뒤에만 실행하며, JSON은 배타 생성한다. 기존 승인 형식으로 보존 산출물을 검증할 수 있다. 이 SHA에서 캡처·`benchctl`·registry·테스트 정책 Python **357/357 VERIFIED**이며, TSV와 Rust 해시 대조·승인 해시 골든·기존 staging 거절·JSON 무교체 보조 Rust **4/4 VERIFIED**다. [실행 계약](../../../../tools/benchmark/README.md#archived-openai-ab-diagnostic)

`0dec0baf7`의 공식 Rust 실행은 공유 잠금에 입장한 뒤 **FAILED · exit 101 · 본문 0**이었다. 격리 후보에 IPC의 `connect_unix_probe` 함수·재공개가 빠져 있었고, CLI 단위 테스트가 `args.pop()` 반환값을 소비하지 않았다. `d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1`에 기존 IPC 헬퍼를 인수하고 제거한 플래그를 테스트에서 검증하도록 수정했다. 이 clean 후보에서도 같은 Python 범위 **357/357 VERIFIED**다. 위 Rust 선택자에 `test(refusal_writer_publishes_complete_json_once_without_replacement)`와 `test(recover_native_settlement_flag_requires_one_recovery_command)`를 추가한 첫 재검증은 공유 잠금 대기 300초 뒤 **BLOCKED · exit 124 · 본문 0**이었다.

같은 `d8a490565` clean 후보에서 위 두 선택자를 포함한 `./scripts/cargow --lane test-workspace-lane nextest run`이 공유 락을 획득해 **VERIFIED · exit 0 · 20/20 통과**했다. Nextest 실행 ID는 `8463e49f-290c-41d3-a7eb-0ab723cf350a`다. 4개 바이너리에서 Source·조회 비용 공유, 재시도 상한, 승인 endpoint 불일치와 전송 직전 취소 거절, 변조 증거와 기존 staging 거절, JSON 재게시 거절을 실행했다. 선택 범위 밖 523건은 건너뛰었다. 로컬 `main`을 `df090e56a`에서 검증한 `d8a490565`로 fast-forward했고, 대상 작업 파일 36개와 기존 스테이징을 보존했다.

후속 구조 점검에서 전송 작업자의 첫 취소 검사 뒤 비용 원장 락을 기다리는 동안 취소되면, 락을 획득한 뒤에도 전송과 비용 차감이 진행되는 경우를 확인했다. Core 원장이 락 획득 후 차감 직전에 OpenAI 요청의 취소·남은 시간을 확인한다. HTTP 전송은 락을 놓은 뒤 실행한다. [비용 원장](../../../../crates/quanta-index-core/src/domains/semantic/admission.rs#L405) · [OpenAI 전송](../../../../crates/quanta-index-embed/src/openai.rs#L1108)

같은 공유 락에서 Rust 1.92 `rustc --test`로 이전 Core 원장 소스와 신규 회귀를 링크했다. 동일 interleaving에서 수정 전 **FAILED · exit 101 · 모의 전송 1회·차감 1**, 수정 후 취소 오류와 모의 전송·차감 각각 0을 확인해 **2/2 VERIFIED · exit 0**이었다. 공식 `nextest` 실행 `cacad54b-587e-4fae-bbb3-b96856bc5d1e`에서도 Core·Embed 두 바이너리의 본문이 **6/6 VERIFIED · exit 0**이었다. `d8a490565` 기준 공유 작업 트리에서 수리한 원장·OpenAI 소스의 blob은 실행 전후 각각 `46edd4f5d5aa30e10c77955bb0cebeb85de7ac06`, `ebd7f2a980a996b1caa92eff44a4d2ff6bc54db8`였다. [취소 회귀](../../../../crates/quanta-index-core/src/domains/semantic/admission.rs#L878)

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features \
    -p quanta-index-core -p quanta-index-embed --lib --no-tests fail \
    -E 'test(http_admission) | test(approved_openai) | test(interruption_at_http_dispatch) | test(an_attempts_timeout_is_capped_by_the_budget_remainder)'
```

05.5 유료 A/B는 **BLOCKED**다. 승인 입력 해시와 비용 승인값, 고정 데몬 경로와 SHA-256, API 키가 공급되지 않았다. 실제 유료 호출은 **NOT_RUN**이다.

- [ ] **05.6** 같은 Linux에서 고정 비교용 old-Q와 Native의 유지 시나리오를 warm **200**·cold **20** 이상 실행한다. 공개 SDK gold·정확한 오류·고정 source/fixture/daemon, 원 표본과 검증기 음성 회귀를 각 결과에 결속한다. [DSL DoD](../../nov-12-native-cutover/tickets/NC-F-integration.md#dsl-정식-검증-연결)

## Hosted 공급

- [ ] **05.7** 승인 Linux x86-64 old-Q ELF `ce64d9f6…` (136,709,176 B)를 기존 CircleCI GitHub App 프로젝트에서 승인된 runner 경로 또는 HTTPS URL로 공급한다. App에 승인 입력이 연결되기 전까지 hosted 공급은 `BLOCKED`다. [공용 CI](../../../../.circleci/config.yml#L148) · [F 공급 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md)
  - 보존 ELF의 직접 경로와 loopback HTTPS는 각각 exit 0으로 같은 실측 크기·SHA를 기록했다. `BASH_ENV`가 일반 파일이 아니거나 전송이 HTTP로 전환되면 공급 성공 기록을 남기지 않는다. F15 배선을 포함한 `d8a490565` 기반 작업 트리의 로컬 권위·coverage 회귀 **221/221**, `just rust-test-authority`, CircleCI 설정 검증은 모두 exit 0이다.
  - CircleCI App 프로젝트 `circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4`의 파이프라인 정의 `38e6fc99-c983-4c03-bb02-da8d53bad4ae`에서 06.4 후보의 전체 SHA와 pipeline·workflow·job ID를 기록한다. 각 job의 `test-authority/legacy-searchd-input.txt`에서 공급 방식·실측 크기·SHA를 확인한다. PR coverage는 App의 PR base SHA를 사용한다.
  - 승인 공급을 쓰는 job은 PR/main `verify-rust-tests`의 workspace all-features nextest, PR `verify-pr-coverage`의 workspace all-features `llvm-cov nextest`, 수동 `heavy-correctness`의 workspace all-features nextest와 NoFeatures 두 단계다. NoFeatures는 `process_readiness_owner_v1` 전체와 `f15_file_authority`의 `test(=delta_delete_noop_and_cold_open_match_independent_full_build) | test(=nontext_source_stays_path_only_across_cold_delta_noop_and_delete) | test(=damaged_file_bodies_stay_quarantined_after_restart)`를 실행한다. 각각 `test-authority/no-default-nextest.log`와 `test-authority/f15-no-default-nextest.log`의 실명·exit를 확인한다. [CI 실행 단계](../../../../.circleci/config.yml) · [F15 로컬 원 결과](04-maintained-test-migration.md#f15-nofeatures-filebody-restart)
  - 현재 App의 `QUANTA_INDEX_LEGACY_SEARCHD_BIN`·`QUANTA_INDEX_LEGACY_SEARCHD_ARTIFACT_URL` 등록은 0건이다. 최신 App pipeline은 이전 `ea5137d`에서 종료됐다. 06.4 최종 SHA 미확정으로 세 job의 최종 hosted 본문 및 exit는 **NOT_RUN**이다. [06.4 후보](06-tantivy-retirement.md#검증)
  - **11월 22일 08:27 KST 재확인.** 현재 셸에는 `QUANTA_INDEX_LEGACY_SEARCHD_BIN`·`QUANTA_INDEX_LEGACY_SEARCHD_SHA256`·artifact URL 변수가 없고 CircleCI CLI 인증 설정도 없다. App 파이프라인과 project env API는 비인증 조회로 열리지 않아 위 0건 이후의 공급 변경을 새로 입증할 수 없다. 로컬 F15 **3/3**과 연결된 Native FileBody **7/7**은 source-match 상태로 통과했지만 hosted Linux 실행으로 승격하지 않는다. 승인 runner 경로 또는 HTTPS URL이 공급되기 전 05.7은 계속 **BLOCKED**, hosted 본문은 **NOT_RUN**이다.
