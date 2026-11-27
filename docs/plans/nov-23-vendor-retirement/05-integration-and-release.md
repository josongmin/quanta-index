# 포크 0 후보의 제품·출하 인수

Q는 Quanta Index, S는 Semantica Codegraph다. [검증 기록](10-execution-work-breakdown.md)에 소스·실행 결과가 있고, [SQLite 계약](02-sqlite-and-storage-limits.md)에 현재 지원 경계가 있다. 기존 제품·서명 원장에서 같은 후보의 실행을 인수한다.

## 현재 남은 코드

2026-11-26 KST, Q `28784da26` 작업 트리 기준이다. 소스에 반영한 항목과 남은 실행을 같은 행에서 추적한다.

| 우선순위 · 상태 | 담당 · 수정할 코드 | 완료 조건 |
| --- | --- | --- |
| P0 · writer 입장 연결 완료, 서명 설치 입력 대기 | D 설치·Q Runtime. `install-native`와 Python Native 활성 switch는 deployment lock 아래 읽기 검증 뒤, selection 게시 직전에 SHA로 고정한 실제 데몬의 `preflight-native-writer`를 실행한다. 새 명령은 기존 StateRoot lease와 XFS quota verifier, FD 상한, 쓰기 격리를 사용한다. [설치 런처](../../../tools/deployment/launcher.py) · [Runtime](../../../crates/quanta-index-searchd-runtime/src/native_product.rs) · [systemd](../../../tools/deployment/systemd/quanta-state-root-quota-verifier@.service) | D에서 승인 호스트의 verifier·XFS project·설정·Root identity를 서명 readback에 결속한다. 이전·새 Root의 합산 64 GiB와 새 Root의 첫 게시→조회→재시작을 판정한다. C-04의 서명 RestoreForward CAS는 기존 원장에서 검증한다. [D 원장](../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md) |
| P0 · 제품 XFS 하네스와 집중 본문 확인 | Q Runtime·Harness. 공통 제품 프로필에서 XFS project와 verifier, Root 안의 `SQLITE_TMPDIR`, 4,096 FD, 1 GiB scratch 및 전용 UDS 디렉터리를 준비한다. ReleaseProcess, P04/P10과 Native index scale에서 사용한다. 데몬 진입점은 fixture의 제품 preflight를 거절한다. [공통 프로필](../../../crates/quanta-index-searchd-harness/src/product_xfs.rs) · [P04](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/read_view_lifetime.rs) · [P10](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/native_boot.rs) · [scale](../../../crates/quanta-index-searchd-harness/src/scale/native_index.rs) | 이전 로컬 후보에서 P03, P04 기본·retention, RepoMap 구 wire와 Small scale을 확인했다. 현재 로컬 후보의 P10은 설치 중복 ELF 해시를 제거하고 테스트 watchdog을 분리한 뒤 통과했다. 정식 P03–P10은 동일 release ELF 및 등록된 호스트에서 판정한다. [집중 검증](10-execution-work-breakdown.md#11월-26일-후속-결함-검증) · [P10 재검증](10-execution-work-breakdown.md#p10-설치-타임아웃과-x86-linux-재검증) |
| P0 · macOS 오프라인 로컬 검증 완료 | Q Runtime·Storage. admitted inventory와 Source·History directory metadata에 동일한 Darwin held-FD batch를 사용하고 보유 Root의 보호 Catalog reader를 연결했다. [admitted inventory](../../../crates/quanta-index-searchd-runtime/src/publication_inventory/admitted.rs) · [metadata](../../../crates/quanta-index-searchd-runtime/src/publication_file_capacity/read_only_tree.rs) · [오프라인 지원](02-sqlite-and-storage-limits.md#macos-오프라인-잔여) | Runtime 10개와 로컬 Legacy Source·History CLI 각 1개가 통과했다. 승인된 이전 설치·History와 RepoMap 운영 쓰기 절체는 같은 서명 Linux 후보에서 판정한다. |
| P1 · Cargo 검사 연결 및 정책 정리 완료 | Q Cargo/CI. Runtime·Semantic의 Legacy feature를 명시적 opt-in으로 유지한다. `thiserror`를 통합하고 호환이 갈리는 중복만 버전별로 기록했다. contract fuzz의 zstd 계열을 갱신했고 독립 quota verifier를 포함한 전체 locked workspace에 deny·freshness를 적용한다. [정책](../../../deny.toml) · [검사 대상](../../../scripts/rust-workspaces.sh) · [검증](10-execution-work-breakdown.md#cargo-정책) | 공급 검사와 정책 검사는 통과했다. 표준 제품과 Legacy 제품의 서명 설치·호환 본문은 각 인수 원장에서 판정한다. |
| P1 · S warm DELTA 구현 진행 | S SDK·Store·ItemIndex의 INC‑01/S02. 공동 C1 발급과 Source 결속은 연결됐다. V1 호환 pin의 전체 투영, Store V4 warm prepare/HEAD 전수 검증, ItemIndex 유지 경로 재검증을 담당 원장에서 줄인다. [INC‑01](../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25) · [S02](../nov-24-native-enhancement/tickets/S02-single-edit-delta.md) | 한 파일 편집의 무관 본문 작업 0 B와 전체 경로 열거 0회를 계측하고 독립 FULL·재시작·SDK/Watch 결과를 대조한다. |

Native index scale과 RepoMap 구 wire의 제품 UDS 테스트는 공통 XFS 프로필을 쓴다. 일반 workspace 실행은 `cargow` 자동 pin을 식별해 개발 fixture를 선택한다. 릴리스 witness나 명시 제품 ELF가 있으면 quota 입장이 필수다. 독립 corpus, DSL cold/warm 및 snippet의 제품 XFS 검증은 별도다. [RepoMap wire](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/retired_repomap_wire.rs) · [corpus](../../../crates/quanta-index-searchd-harness/src/scale/native_index/corpus.rs) · [DSL](../../../crates/quanta-index-searchd-harness/src/scale/native_index/dsl_history.rs) · [snippet](../../../crates/quanta-index-searchd-harness/src/snippet.rs)

현행 Q에서 Product4K 후기 병목의 수정 위치는 새 실행의 후기 Record를 보고 확정한다. 이전 후보의 30초·1,200초 실패는 보존한다. signed History·P11, 독립 qrels, 전체 커버리지는 [설치·검증 잔여](10-execution-work-breakdown.md#제품-증거와-잔여)에서 추적한다.

## 실행 순서

| 우선순위 | 담당 경계 | 완료 조건 |
| --- | --- | --- |
| P0 · 설치 입력 | Q Runtime/Catalog·설치 D/S/H | Linux quota 배치와 이전 Root의 오프라인 이관 입력을 준비한다. 승인 이전 Root·실행물·History와 현재 후보의 target/config/policy를 확보한다. |
| P1 · SQLite 공급자·핵심 I/O 절체 완료 | Q Runtime/Catalog·Storage | Linux stock writer, macOS Catalog 보호 읽기와 표준 Cargo 그래프를 반영했다. [남은 코드](#현재-남은-코드)에서 설치와 CLI를 인수한다. |
| P1 · 병렬 검증 | Cargo/CI, TEXT/B, INC, F-EVAL, parser/codec 소유자 | 의존성 실패 원인, 초기 게시/DELTA/품질 기준선과 비교용 데이터, 공식 parser 배포 입력, 기존 Q/S wire·Root를 준비한다. 설치 실행은 이미 등록된 소유자와 레인으로 인수한다. |
| P2 · 하나의 후보 합류 | Cargo/CI·Root/F | 포크 0인 Runtime recipe·launcher·installer, daemon/CLI/SDK·코퍼스/모델을 같은 Q/S 소스에서 고정한다. 실제 compiler·shipment 검사와 설치 SHA를 연결한다. |
| P3 · 출하 | D/S/H, Root/F, E07/E08/P11 | 같은 서명 설치에서 운영·복구·품질을 닫고 실제 P10/P11·벤치 결과를 기존 원장에 결속한다. |

## Cargo와 실행물 증거

- [Cargo](../../../Cargo.toml), [표준 데몬 recipe](../../../Justfile#L201), [릴리스 검사기](../../../tools/ci/check-tantivy-retirement.py), [설치 런처](../../../tools/deployment/launcher.py)에서 같은 SQLite 공급자와 target/features를 확인한다. 지원 profile의 locked metadata/tree, 실제 Cargo compiler stream, daemon/CLI 락과 공급자 해시, source closure, Python 공급물 및 배치 입력에서 원래 Q 포크가 0이어야 한다. S의 별도 SQLite 0.30.0과 프로세스 경계는 별개로 확인하며 Q의 13개 분모에 넣지 않는다.
- 전체 locked workspace에 deny를 적용한다. 이행 중 발생한 중복·default feature와 advisory의 조치는 [Cargo 검증 기록](10-execution-work-breakdown.md#cargo-정책)에 남겼다. 독립 fuzz의 라이선스 예외와 독립 LanceDB 오라클의 유지보수 예외는 제품 공급자 판정과 별도다.
- 기존 격리 release 캡처와 shard 검사를 사용한다. `just rust-capture-release-build daemon|cli <외부 기록 디렉터리>`의 compiler receipt, 실제 설치 ELF/manifest 선택, signed readback, P10/P11이 같은 소스와 바이너리 SHA로 연결돼야 한다. 영향 root/nested/fuzz workspace의 잠금과 `cargo deny`도 별도로 판정한다. [원 출하 원장](../nov-20-native-cutover-closeout/tickets/08-release-activation.md) · [P11 계약](../../operator/p11-operational-proof.md)

## 구·신 설치와 복구

| 실행 조합 | 검증할 결과 |
| --- | --- |
| 이전 Q/S + 이전 승인 Root | Source/Text/Semantic/History·Graph/Path의 사용 중인 기능, 마지막 ACK와 실제 Catalog head·retention 기준선을 기록한다. |
| 새 Q/S + 새 quota Root | Catalog/Scratch/Native/Lance 동시 할당, initial FULL→DELTA→조회→History→재시작, 실제 quota 거절과 같은 작업 회복을 실행한다. 사용 중인 SDK/CLI의 구·신 버전을 같은 프로필에서 확인한다. |
| 새 Q/S + 이전 승인 Root 복사본 | 호환 프로필은 기존 선택과 DB/WAL/봉인/History를 대조하며 cold reopen한다. 이관 프로필은 이전 보호 read/export → 검증된 새 Root → 같은 Source/head/History/ACK 결속을 확인한다. macOS 오프라인 실행과 실제 Linux 서명 실행은 각각 필요하다. |
| 이전 S → 새 Q / 새 S → 이전 Q | 지원 조합의 조회·게시를 실제 SDK로 실행한다. 종료할 조합은 Stage·Catalog·Source·공개 head의 변화 없이 첫 쓰기 전에 거절한다. |

1. 활성화 전에 기존 publisher를 멈추고 들어온 작업을 정산한다. 승인된 이전 실행물·selection·Root identity·checkpoint와 마지막 ACK/frontier를 일치시킨다.
2. 새 후보의 첫 durable 게시 이전에는 승인된 이전 선택으로 돌아가는 흐름을 시험한다. 게시 이후에는 ACK 응답과 별도로 durable commit이 있을 수 있으므로 새 쓰기를 차단하고 실제 Catalog operation을 정산한다. 최신 frontier와 History를 보존한 restore-forward를 실행한다.
3. D/S/H의 동일 세대 서명 Q/S, Native selection/root·BOOT·config/policy, SDK/CLI·CP2·History와 새 PID를 Root/F가 인수한다. 이전 승인 바이너리, 실제 이전 Root/History, signed readback 또는 P11 관측·승인 입력이 빠진 실행은 `BLOCKED`로 남긴다. 설치·복구의 최종 본문은 `NOT_RUN`이다. [서명 공급](../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md) · [운영 런북](../../operator/state-cutover-runbook.md)

## 초기 게시와 Product4K

- TEXT/B 소유자의 [제품4K 원장](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md)과 [제품 수용](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md)을 유지한다. 예전 30초 SourceAdvance 기한 실패와 1,200초 owner materialization 실패는 각각 기록한 후보에 귀속된다. 현재 후보에서 최초 실패 단계를 같은 30초·읽기 100만·1,200초·단일 슬롯으로 다시 확인한다. 진단 Record는 같은 operation과 채택한 구간의 닫힌 trace를 남기고, 정식 통과는 quiet host의 Off에서 판정한다.
- 초기 실패면 [Text 준비/쓰기](../../../crates/quanta-index-lexical/src/native/text/term_dictionary_write.rs)와 적용, 후기 실패면 [Runtime pump/worker](../../../crates/quanta-index-searchd-runtime/src/native_pump.rs)·[worker](../../../crates/quanta-index-searchd-runtime/src/native_worker.rs)·[Kernel](../../../crates/quanta-index-kernel/src/update.rs)·Catalog의 queue→grant→prepare→publish→settle 중 실제 지배 구간을 수정한다. 같은 입력·요청으로 ACK, old pin, 재시작을 다시 확인한다.
- Caddy/etcd SDK60은 cold Off 각 2회와 Record 각 1회, Large/XL은 원 파일 분모와 두 PID 재시작을 받는다. 전체 Product4K와 ANN4K는 4,096×64/Q64/K10, Recall@10 ≥0.95와 독립 exact, overlay·교체/삭제·재시작을 기존 제품 프로필로 확인한다. Semantic lib의 ANN4K는 이미 통과한 범위다. signed 제품 본문은 별도다.

## Warm DELTA

- Facts·disk는 검증된 predecessor와 같은 Store V4 변경 행에서 미게시 C1 target을 발급하고 Source manifest·durable pointer와 복구에 결속한다. Watch의 signed predecessor 비교와 작업량 계측도 연결됐다. 다음 코드 경계는 V1 호환 pin 전체 투영, warm Store의 준비·HEAD 물리 검증, ItemIndex의 유지 파일 재검증이다. [Source V2 원장](../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25) · [S02 인계](../nov-24-native-enhancement/tickets/S02-single-edit-delta.md)
- S Runtime/SDK의 E02 receipt와 V9/V5/pending carrier를 같은 Source cut·attempt·Q operation에 결속한다. Q [apply_source](../../../crates/quanta-index-kernel/src/source.rs)와 [봉인·replay](../../../crates/quanta-index-kernel/src/update.rs)의 호출은 실제 실패 재현 때만 수정한다. binding 전 Submit 0, page ACK 유실·128→129 경계·seal 직후 종료·fence 변경에서 같은 prefix와 Source CAS 1회, V9 ACK 1개, Q 공개 중복 0건이다. 실제 Q Submit 응답 유실 회귀는 격리본의 hook·selector를 합류한 뒤 기존 소유 레인에서 인수한다. [설치 입력과 본문 판정](../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25)
- 공유 CAS의 불변 custody와 손상 거절이 확보된 뒤 V4 준비/commit·ItemIndex 전체 순회를 줄인다. 한 파일 실제 편집·추가·삭제·rename·no-op을 같은 설치의 초기 FULL→첫 V2 DELTA와 독립 FULL에 대조한다. warm 한 편집에서 전체 경로 열거 0회와 무관 본문 read/copy/hash/write 각 0 B를 요구한다. 재시작 뒤 Source/Path/Text·독립 Graph 결과, view/owner와 SDK·Watch를 다시 대조한다. 설치 본문·비용 결과는 `NOT_RUN`이다. [제품 판정](../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md)

## 품질·운영·나머지 공급자

- F-EVAL은 같은 signed SDK view의 lexical/dense/Hybrid 후보와 순위, 같은 코퍼스·DEV/EVAL·독립 qrels로 판정한다. [Native 품질 DoD](../nov-12-native-cutover/DOD.md)의 표본과 파일 MRR/Recall/무정답 기준을 사용한다. 정답이 pool 밖이면 [ANN 라우팅](../../../crates/quanta-index-semantic/src/native/embedding/ann_query.rs)과 lexical plan을, pool 안이면 [실제 출하 RRF](../../../crates/quanta-index-contract/src/native_protocol/read_session/ranking.rs)를 독립 정답과 대조한다. 동일 입력의 외부 엔진 실측이 있을 때 업계 비교를 판정한다.
- E07은 원 60초 warmup+900초 open-loop 50 QPS, 동시 4·대기 64·≥10,000 attempts·class당 ≥1,000, 1초 churn이다. HybridFile p95≤100ms/p99≤500ms, freshness p95≤1s/p99≤5s, RSS≤2 GiB, scratch≤4 GiB, physical≤64 GiB, pin 해제 뒤 60초 회수와 5초 shutdown을 원 분모에서 판정한다. 같은 호스트·키·입력·품질 기준의 AB/BA 5쌍에서 submit→ACK와 조회 p95의 비율 중앙값은 각각 ≤1.05다. [E07 원장](../oct-22-finalizing/E07-performance-operations.md)
- [공식 parser 설치](../../../tools/ci/tree-sitter-registry-consumer/INSTALL.md)는 지원 target의 실제 공급물·descriptor, Rust/Python 소비 경로와 외부 코퍼스의 파일·심볼·byte/span·검색 결과 및 실행 비용을 남긴다. [비교 도구](../../../tools/ci/compare_tree_sitter_symbol_records.py)를 같은 corpus manifest로 실행한다. 로컬 소스 퇴역 본문과 출하 설치를 구분한다.
- Q/S CBOR·Unicode는 이전 승인 Root와 S Lockfile/receipt/Graph/PTA의 실제 old writer→new reader, 허용 신→구, Unicode 17 ID·NFC·digest, 손상·거절·정산을 같은 후보에서 확인한다. 기존 Q i686 IPC peer-image 실패는 x86_64 Linux 커널의 ELF32 exact 재실행에서 통과했다. IPC suite와 후속 fixture 검증은 재검증 기록을 따른다. Unicode 벤치 fixture도 배포 source closure에 포함한다. [공급자 검증](10-execution-work-breakdown.md#코덱과-parser) · [i686 재검증](10-execution-work-breakdown.md#p10-설치-타임아웃과-x86-linux-재검증)
- 기존 [Tantivy 대체와 old-Q](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md), [RepoMap 이전 Required Root](../nov-23-repomap-retirement/README.md), [E06/E08/P11](../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 제품 조건을 같은 설치로 인수한다. RepoMap 제거가 TEXT+Semantic Product4K의 선행 조건이 아니다.

## 완료 판정

- [x] 대상 13개 vendor 원본, SQLite patch와 private feature/FFI를 퇴역했다. 지원 그래프와 Linux/macOS 데몬 컴파일, 집중 런타임 회귀를 확인했다.
- [ ] 출하 프로필의 컴파일러·설치 실행물, 이전 승인 Q/S·Root·History와 새 selection을 같은 서명 후보에 결속했다.
- [ ] 실제 Linux no-quota 이관, macOS 오프라인 설치 검증, 데이터가 있는 History 및 최신 frontier restore-forward를 해당 서명 후보에서 실행했다.
- [ ] Cargo 정책, 각 엔진 레인, 제품/운영 임계값과 적용 대상 조합을 원장에 기록했다. 실제 결과와 line/branch coverage는 별도 분모다. `FAILED`·`BLOCKED`·`NOT_RUN`이 남은 필수 조건은 각 상태로 유지한다.
