# 전체 구조 개선 실행 프롬프트

아래 블록을 quanta-index 구현을 맡을 다른 에이전트에게 그대로 전달한다.

```text
당신은 `/Users/songmin/Documents/code-new/quanta-index`의 quanta-index 구조 개선 전체 구현 owner다.

목표는 bugbash finding을 개별 patch로 덮는 것이 아니라, 구조 개선 최종안의 W0–W7과 C1–C4를 실제 코드·테스트·검증으로 완료하는 것이다. 계획을 다시 작성하거나 일부 P1만 고친 뒤 끝내지 마라. 독립 진행 가능한 작업은 계속 수행하고, 외부 자격 증명·production corpus·다른 repo처럼 실제로 없는 입력만 명시적 blocker로 남겨라.

## 1. 시작 시 읽을 authority

다음 순서로 전체를 읽어라.

1. `AGENTS.md`
2. `AGENT_CORE.md`
3. `AGENT_PLAYBOOK.md`
4. `AGENT_REFERENCE.md`
5. `docs/bugbash/sep-16/findings.md`
6. `docs/bugbash/sep-16/structural-remediation-plan.md`
7. `docs/bugbash/sep-16/test-plan.md`
8. `tools/ci/test-authority.toml`
9. `Justfile`

충돌 시 repo authority의 우선순위를 따른다. 이 프롬프트는 구현 범위와 종료 조건을 구체화한다. 문서의 OL/IT/G0/W/C ID는 계획 ID이며 기존 test 함수나 실행 완료를 뜻하지 않는다.

## 2. 시작 상태와 ownership

감사 기준 HEAD는 `4914156f4191daa3e12998bdb38f2b821a057fdd`였다. 당시 tracked product source는 clean이고 `docs/bugbash/`만 untracked였다. 이 상태는 과거 snapshot이므로 시작 즉시 다음을 다시 확인한다.

- `git rev-parse HEAD`
- `git status --short`
- branch/upstream과 현재 diff
- `docs/bugbash/sep-16/`의 findings/구조안/test plan/실행 prompt 존재와 digest
- 실행 중인 관련 cargo/searchd/benchmark process

현재 HEAD가 달라졌으면 reset하지 말고 새 HEAD와 감사 HEAD의 관련 diff를 읽어 finding/계획의 유효성을 다시 판정한다. user-owned dirty/untracked 파일을 삭제·원복·덮어쓰지 않는다. 특히 `docs/bugbash/`는 이 작업의 입력이다. 겹치는 dirty product 파일이 있으면 독립 파일부터 진행하고 충돌 지점만 blocker로 보고한다.

현재 workspace에서 작업하는 것을 기본으로 한다. 별도 worktree를 만들 경우 untracked bugbash 문서가 자동으로 따라가지 않음을 고려해 원본 절대 경로를 read-only authority로 사용하고, 산출물을 두 worktree에 분산시키지 않는다.

코드·테스트·문서·manifest·Justfile·CI authority 변경과 새 storage crate/dependency 추가는 이 작업에 포함된다. commit, push, PR 생성, 배포, production state-root migration/삭제는 별도 요청 없이는 하지 않는다. 임시 state와 artifact는 repo 밖에 둔다. production state-root를 probe나 test에 사용하지 않는다.

## 3. 절대 조건

- Tantivy의 inverted index/BM25와 LanceDB/Arrow를 유지한다. 자체 inverted index나 자체 ANN 엔진을 만들지 않는다.
- core/contract는 vendor-neutral로 유지하고 SQLite/Tantivy/Lance 세부사항은 adapter에 가둔다.
- 공개 authority는 하나만 둔다. old file authority와 new catalog dual-write, query-time legacy fallback, silent semantic downgrade를 남기지 않는다.
- bytes durability → catalog publish → activation을 분리하고 순서를 지킨다. DB transaction이 backend 파일을 원자화한다고 가정하지 않는다.
- exact source identity, batch body hash, logical generation, physical artifact ID, model/ANN profile, activation/health/aux epoch를 별도 타입으로 다룬다.
- query는 route가 요구하는 dependency vector를 한 번 resolve/pin한다. 필요 없는 history/runtime/RepoMap readiness로 plain lexical을 차단하지 않는다.
- active/rollback pin과 resident cache handle을 구분한다. pin/activate/retire/attach/quarantine의 원자적 전환점과 lock order를 구현한다.
- budget 초과, missing ANN, integrity failure, unsupported capability를 성공·empty result·fallback으로 바꾸지 않는다.
- 외부 embedding 호출의 exactly-once 과금은 provider support 없이 보장하지 않는다. at-most-one committed application과 durable receipt만 보장한다.
- incremental이라고 부르려면 unchanged foreground bytes의 full-copy가 없어야 한다. compaction/index training 비용은 별도 계측한다.
- 단순 파일 존재, row count, seal marker만으로 integrity를 통과시키지 않는다. incremental commitment는 independent full-rebuild oracle과 대조한다.
- async timeout이나 `spawn_blocking` abort만으로 physical cancellation 완료를 주장하지 않는다. 실제 worker slot/bytes 점유를 계상한다.
- migration은 별도 vNext state root의 offline freeze/export/import를 기본으로 한다. production root에서 시험하지 않는다.
- `unsafe`, `#[allow]`, default substitution, heuristic success, open-ended compatibility shim을 추가하지 않는다.
- 테스트를 구현 문자열 grep이나 implementation mirror로 만들지 않는다. 외부 관찰 가능한 invariant와 independent oracle을 검증한다.

## 4. 진행 기록

`docs/bugbash/sep-16/implementation-progress.md`를 작업 ledger로 만든다. 기존 파일이 있으면 이어서 쓴다. 최소 필드:

- frozen HEAD/base/dirty fingerprint와 user-owned paths
- W0–W7, C1–C4 상태: `planned | in_progress | implemented_not_run | passed | failed | blocked | not_applicable`
- QI-BB-001–032 상태와 owning W/OL/IT
- DA-01–12 상태
- G0-L/S/C/R의 command, source/config digest, 결과, 선택/기각 사유
- 변경 파일·삭제한 legacy owner·남은 compatibility path
- 실행 command, selected/executed/pass/fail/ignored, 포함 범위, 제외 범위, failure class, raw artifact path
- external blocker와 필요한 owner/input

ledger는 진행 증거다. green test 없이 finding을 `passed`로 바꾸지 않는다. source가 이동하면 링크와 owner를 갱신하되 finding의 과거 증거를 지우지 않는다.

## 5. 실행 순서

### Phase 0 — source 재검증과 W0 gate

1. findings의 reachable path와 현재 source를 재대조한다. 이미 고쳐진 finding은 exact diff와 test evidence로 표시한다.
2. 현재 broken scan experiment를 current seal contract에 맞게 고친다. 기능 truth가 통과한 뒤 baseline을 캡처한다.
3. G0-L을 실제 Tantivy로 구현·실행한다: pinned base read, add/delete/merge, old/new 동시 read, GC/crash open, immutable file inventory, new-written bytes, scoring/statistics 보존.
4. G0-S를 실제 LanceDB 0.30.0으로 구현·실행한다: old-base branch, vector/membership 결속, ANN delete/refill, pinned version cleanup/restart, 사용 API 목록.
5. G0-C를 실제 SQLite workload로 구현·실행한다: linked engine version, effective pragmas, commit/queue/WAL, busy/deadline, crash/export/restore. WAL-reset fix가 포함된 version만 사용한다.
6. G0-R을 실제 runtime으로 구현·실행한다: native operation cancellation points, canceled single-flight waiter, slot occupancy, frame decode peak, graceful shutdown drain.

각 gate를 ADR/evidence로 남긴다. API 문서 조사나 toy compile만으로 PASS하지 않는다. gate가 실패하면 해당 lane의 W3 전환을 차단하고 이유를 남겨라. 자동으로 base+delta custom layout이나 process isolation을 선택하지 말고 그 후보도 별도 probe를 통과시켜라. 막히지 않은 W1/W5 등은 계속 진행한다.

### Phase 1 — W1 contract

contract-base/contract/core/SDK에서 다음을 한 번에 고정한다.

- producer/stream/session/sequence와 operation key/body hash/replay floor
- source/logical/physical/profile/epoch identity
- resolved read token과 route-specific dependency vector
- query budget, result exactness/exhaustiveness/continuation 의미
- `top_k=1..=10_000`, 0/10,001 typed refusal, internal k+1 분리
- embedding/ANN profile과 normalization contract
- old/new wire consumer inventory와 version mismatch behavior

공개 DTO를 바꾸면 SDK/server/producer contract를 같은 change set에서 맞춘다. `just rust-public-api`, `just rust-fuzz-smoke` 대상이다. legacy/v2 payload를 하나 더 추가하지 말고 canonical surface로 cut한다.

### Phase 2 — W2 catalog/lifecycle, C1

G0-C가 통과한 단일 storage adapter crate를 추가한다. 최소 구현:

- operation/session journal, replay lookup, durable receipt
- snapshot/artifact/dependency/activation/health/aux records
- invisible staging epoch와 short publish transaction
- lifecycle gate와 `scope gate -> catalog transaction` lock order
- logical pin, `Live -> Deleting`, attach rejection, async reap scheduling
- prepare/publish/activate/recover state machine과 stale-attempt fencing
- active-first descriptor boot와 inactive quarantine

먼저 한 corpus의 `publish -> acquire -> query -> restart -> replay` vertical slice를 연결해 C1을 통과한다. 이 과정에서도 old/new authority dual-write는 금지한다. 기존 physical writer를 잠시 쓰면 신형 state root 안의 단일 backend implementation이어야 하며 C2 삭제 owner와 deadline을 ledger에 기록한다.

### Phase 3 — W3 native snapshots/GC, C2

G0-L/S가 통과한 방식만 구현한다.

- Tantivy immutable segment snapshot과 query-required text block manifest
- Lance exact dataset/membership version과 ANN artifact/coverage
- canonical input inventory 및 incremental commitment
- streamed semantic derivation/Arrow write와 batch/resource cap
- shared artifact reference, actual physical bytes, pin-aware GC
- immutable compaction replacement와 expected-old artifact CAS
- on-read integrity, cold-open validation, bounded scrub

full/delta/clear/delete/reorder/restart의 incremental root와 independent full rebuild root/query 결과를 비교한다. same-count content mutation도 검출한다. native ANN delete/filter/refill 결과를 exact live-row oracle과 비교한다.

### Phase 4 — W4 read/query

- single-flight `SnapshotRegistry`와 bounded resident bytes/FD/cold-open concurrency
- flight-owned cancellation과 waiter 독립성
- route-specific `QueryReadView`
- common logical/physical query plan
- bounded exact count/projection/regex/result encoding
- `scope_top_k` pushdown 및 arbitrary ID scope cap
- exact/at-least/unknown/exhausted/window 의미 수정
- history/RepoMap에만 keyset page token. 범용 cursor store와 disk spill engine은 만들지 않는다.

semantic existing cache와 별도 lexical authority cache를 중첩하지 않는다. 모든 route가 common validator/plan/read view를 사용한 뒤 old reopen/route-specific limit path를 삭제한다.

### Phase 5 — W5 IPC/runtime/resource

- connection별 bounded read와 pre-worker FD/frame byte reservation
- query/ingest/control/maintenance별 queue·permit·fairness와 GC 최소 진행 예산
- cooperative deadline/cancellation, claimed mutation의 status 조회
- shared runtime ownership과 bounded blocking worker pool
- live socket unlink 방지, single-instance lease, mode/owner/peer policy
- bounded diagnostics와 metric exporter
- writer/cache/query/embedding/Arrow/WAL/compaction을 합친 process envelope
- graceful shutdown/drain fixture

slow peer, long native query, 정상 query, ingest, GC를 동시에 실행해 head-of-line blocking과 permit deadlock이 없는지 확인한다.

### Phase 6 — W6 retrieval/domain quality, C3

- immutable learned embedding profile과 versioned/checksummed atomic cache
- hash embedder는 dev/test로 격리. 이번 범위에서 신규 lexical-only deployment mode를 만들지 않는다.
- true hybrid independent dense recall과 explicit lexical-scoped rerank 분리
- canonical HybridSeed/ranker/metrics, dense failure의 fail-closed
- original-query execution trace 기반 explain과 exact presence
- history relevance/recency total order와 keyset page
- Unicode/text semantics registry와 offsets
- indexed/bounded RepoMap 및 versioned auxiliary state
- cache/sample/regex/vector/response retention·memory cap

production learned profile의 held-out relevance와 exact-vector baseline을 사용한다. credential/model/corpus가 없으면 IT-16만 blocked로 남기고 stub 결과를 production quality로 승격하지 않는다. 그 외 owner-local/integration은 계속 완료한다.

### Phase 7 — W7 cutover/delete/qualification, C4

- 별도 vNext root용 offline exporter/importer/rebuild 도구와 dry-run/fault tests
- frozen high-water, source/domain roots/counts/query/replay 비교 receipt
- server/SDK/producer wire version cut과 old-version explicit refusal
- read-only smoke 전 rollback과 writes-open 이후 rollback/roll-forward 경계
- old Ledger whole-snapshot persistence, file activation authority, query reopen, full-copy writer, legacy HybridSeed, snippet explain 등 구조 계획 §11의 모든 legacy 사용처 삭제
- README/architecture와 현재 broken doc path 2개 수정
- test-authority/Justfile/CI recipe 등록 완성

production state를 실제 migration하거나 삭제하지 않는다. migration 도구와 isolated fixture 증거를 완성한다. 외부 producer roundtrip을 실행할 수 없으면 exact missing input과 command를 blocker로 남기고 local C4 evidence를 끝낸다.

## 6. 테스트 의무

`docs/bugbash/sep-16/test-plan.md`를 그대로 구현한다.

- OL-01–14 owner-local 묶음을 구현한다.
- IT-01–16 integration 시나리오를 구현한다.
- QI-BB-001–032의 최소 OL/IT 연결을 모두 채운다.
- DA-01–12 반례를 해당 IT/owner test로 고정한다.
- process crash/lease/restart는 실제 child process로, backend commit/open/delete는 실제 Tantivy/Lance/SQLite로 검증한다.
- concurrency 순서는 barrier/failpoint handshake로 제어한다. sleep만으로 interleaving을 가정하지 않는다.
- expected IDs/errors/roots는 independent oracle/golden으로 고정한다.
- zero selected, ignored-only, early-return/skipped fixture는 PASS가 아니다.

새 integration target을 추가하면 `tools/ci/test-authority.toml`과 executable CI rail에 등록한다. P0/P1 invariant에는 independent positive/negative/recovery/consumer target을 배정한다. 같은 target 하나를 네 역할에 반복하지 않는다. `Justfile`의 selected recipe도 함께 갱신한다.

현재 확인된 선택 rail 공백:

- `test-integration`은 현재 선택된 7개 target뿐이다. semantic persisted/model/SCv2와 신규 storage/search-plane target을 추가해야 한다.
- `test-daemon`에는 현재 `composite_generation_authority_restart`, `semantic_boot_report`가 없다. 필요한 새 case를 구현한 뒤 aggregate recipe에 편입한다.

## 7. 검증 순서와 명령

항상 source-first → 가장 좁은 owner test → owner target 전체 → integration → aggregate 순서로 실행한다. Rust front door는 `just` 또는 `./scripts/cargow`다. bare `cargo`를 기본으로 쓰지 않는다.

Focused owner 형식:

`./scripts/cargow --lane test-fast-lane test --locked --all-features -p <owner> --lib -- --list`
`./scripts/cargow --lane test-fast-lane test --locked --all-features -p <owner> --lib <exact_test_name> -- --exact`
`./scripts/cargow --lane test-integration-lane test --locked --all-features -p <owner> --test <target>`

현재 빠져 있는 실제 target은 recipe 편입 전에도 명시적으로 실행한다.

`./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test persisted_semantic`
`./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test semantic_generation_lifecycle_model`
`./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test scv2_persisted_scenarios`
`./scripts/cargow --lane test-daemon-lane test --locked --all-features -p quanta-index-searchd-runtime --test composite_generation_authority_restart`
`./scripts/cargow --lane test-daemon-lane test --locked --all-features -p quanta-index-searchd-runtime --test semantic_boot_report`

Checkpoint/최종 rail:

- `just fmt-check`
- `just rust-profile test-fast`
- `just rust-profile test-integration`
- `just rust-profile test-daemon`
- `just rust-test-authority`
- `just rust-ignored-test-policy`
- `just rust-public-api` for contract/SDK public changes
- `just rust-fuzz-smoke` for IPC/DTO/error changes
- `just rust-hexagonal` and `just rust-cargo-modules` for crate/module boundaries
- `just rust-profile validate-shared-surface`
- `just rust-verify-hellgate-fast`
- `just rust-verify-hellgate-broad`
- `just rust-bench-dsl-truth`
- corrected exact-source warm/cold/load/scale/ops/relevance gates
- `just rust-profile verify-rust` at final source
- `just lint-doc-paths`
- `just lint-root-hygiene`

`just rust-verify-hellgate-cross-repo <producer_root>`는 검증 대상 `QUANTA_INDEX_SEARCHD_BIN`, producer exact HEAD/dirty digest/features를 고정하고 실행한다. recipe 내부 external Cargo 실행은 producer repo의 authority도 따라야 한다.

동일 호스트의 큰 Rust build/link와 authority performance runner를 병렬 실행하지 않는다. warm/cold authority 측정도 직렬화한다. 빠른 toy/20-sample cold smoke를 production p99 증거로 쓰지 않는다.

## 8. 실패 처리

- 실패한 test를 삭제·ignore·완화하거나 broader retry로 숨기지 않는다.
- 먼저 product defect, test defect, fixture/setup, environment/toolchain, external blocker로 분류한다.
- product defect면 root cause owner에서 고치고 focused → owner → integration 순서로 다시 실행한다.
- test defect면 independent oracle과 실제 계약을 기준으로 고친다. 구현 결과에 맞춰 expected를 복사하지 않는다.
- environment/external blocker면 exact command, 최초 실패 step, error, missing input, 독립적으로 완료한 범위를 ledger에 남긴다.
- gate 실패 시 관련 finding을 완료 처리하지 않는다. 다른 lane은 계속 진행한다.
- partial green, compile success, test catalog 등록, stale artifact를 campaign 완료로 승격하지 않는다.

## 9. 완료 정의

다음이 모두 충족돼야 작업이 완료다.

1. QI-BB-001–032 각각 구현과 owner-local/integration evidence가 있다.
2. DA-01–12 반례가 regression으로 고정돼 있다.
3. G0-L/S/C/R 선택이 실제 probe evidence와 ADR로 닫혔다.
4. C1–C4가 같은 최종 source에서 통과했다.
5. OL-01–14와 IT-01–16의 applicable 항목이 실제 실행됐다.
6. 변경 P0/P1 invariant의 positive/negative/recovery/consumer 및 PR/merge/nightly mapping이 authority catalog에 있다.
7. 구조 계획 §11의 legacy path가 source/manifest/consumer에서 삭제됐다. 장기 shim/dual-write/fallback이 없다.
8. final exact HEAD에서 correctness, crash/replay, resource, migration, performance, relevance evidence가 서로 구분돼 있다.
9. docs/README/Justfile/test authority가 구현과 일치하고 repo doc lint가 통과한다.
10. 외부/production 실행이 남으면 code-complete와 release-complete를 분리하고 필요한 owner/input을 명시한다.

## 10. 최종 보고 형식

한국어로 짧고 기술적으로 보고한다.

- exact HEAD/base/branch와 dirty ownership
- 구현 완료/미완료를 W0–W7, C1–C4, 32 findings 기준으로 표기
- 핵심 구조 변경과 실제 삭제한 legacy 경로
- test 결과: command, selected/executed/pass/fail/ignored, 포함/제외, failure class
- performance/quality: source/corpus/model/host/threshold와 실제 수치
- migration/rollback 증거
- 외부 blocker, review/merge/deploy/production activation의 미실행 경계

최종 응답에서 “SOTA”, “production-ready”, “완료”를 수치·회복·consumer 증거 없이 쓰지 마라. 구현과 테스트를 끝낸 뒤 결과를 보고하라. 계획 재진술로 종료하지 마라.
```

## 전달 시 참고

- 이 프롬프트는 코드·테스트·마이그레이션 도구와 isolated verification 구현을 승인한다.
- commit/push/PR/deploy/production migration 또는 실제 production data 삭제는 승인하지 않는다.
- 다른 에이전트가 시작할 때 source가 바뀌었으면 이 문서의 HEAD를 현재 상태로 오해하지 않고 재검증해야 한다.
