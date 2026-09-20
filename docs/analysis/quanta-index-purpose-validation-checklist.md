# quanta-index 목적 적합성 감사 체크리스트

이 문서는 `quanta-index`가 의도한 역할을 실제로 수행하는지 감사하기 위한
실행 기준이다. 구현 계획이나 테스트 수 집계가 아니라, 현재 source snapshot에서
다음 제품 명제를 증명하는 데 사용한다.

> Semantica/Quanta producer가 발행한 검색 사실을 검증하고 generation 단위로
> materialize·seal·activate한 뒤, lexical/semantic/hybrid 및 보조 검색 surface를
> 일관되고 복구 가능한 로컬 search plane으로 제공한다.

현재 감사 authority는 다음 순서로 해석한다.

1. current contract/runtime source와 실제 실행 receipt
2. [Justfile](../../Justfile), [test authority catalog](../../tools/ci/test-authority.toml),
   [wire inventory](../../tools/ci/inventory/wire-surface.toml)
3. [README](../../README.md)의 현재 owner/non-goal 설명
4. [search product quality measurement matrix](../plans/jun-7-search-product-quality/MEASUREMENT_MATRIX.md)
5. [sep-16 test plan](../bugbash/sep-16/test-plan.md)과 historical plan/SSOT

낮은 순위 문서가 current source와 충돌하면 current source와 실행 결과를 따른다.

## 1. 감사가 답해야 하는 질문

1. producer와 search plane의 truth ownership이 섞이지 않는가?
2. 잘못되거나 불완전한 publish가 active 검색 상태를 오염시키지 않는가?
3. lexical과 semantic이 동일한 composite generation으로 조회되는가?
4. active 전환, pin, rollback, restart, GC 중에도 generation consistency가 유지되는가?
5. 각 query route가 advertised semantics와 실제 backend 실행을 일치시키는가?
6. semantic/hybrid가 단순히 응답하는 수준을 넘어 측정 가능한 relevance를 내는가?
7. 오류·미준비·과부하·손상을 빈 성공이나 silent fallback으로 숨기지 않는가?
8. 실제 daemon process가 종료, 장애, 재시작, 취소, 동시성 상황에서 안전한가?
9. 운영자가 readiness, provenance, metrics, quarantine, 실패 원인을 판별할 수 있는가?
10. 현재 producer, public SDK, wire format과 exact-head에서 호환되는가?
11. 정해진 corpus·host·model 조건에서 용량과 latency 한계를 만족하는가?
12. 문서와 배포 주장이 현재 실행 경로와 일치하는가?

하나라도 근거 없이 추정해야 한다면 해당 범위는 `PASS`가 아니다.

## 2. 판정 언어

모든 체크는 아래 상태 중 하나만 사용한다.

| 상태 | 의미 |
| --- | --- |
| `PASS` | 현재 frozen source에서 요구된 proof가 실행됐고 oracle을 만족함 |
| `FAIL` | 요구된 동작과 다른 결과가 재현됨 |
| `BLOCKED` | 필수 host, provider, producer checkout, credential 또는 fixture가 없어 실행 불가 |
| `NOT_RUN` | 실행 가능하지만 아직 실행하지 않음 |
| `N/A` | 감사 범위 밖임이 owner 결정과 근거로 확인됨 |

금지하는 판정:

- `부분 PASS`, `대체로 PASS`, `코드상 PASS`
- test binary exit `0`만 보고 selected/executed test 수를 확인하지 않은 `PASS`
- unit/fake adapter 결과를 실제 Tantivy/LanceDB/SQLite/UDS/process 증거로 승격
- macOS 결과를 Linux production 성능이나 Linux credential 동작의 증거로 승격
- 과거 clean HEAD의 receipt를 현재 dirty worktree의 증거로 재사용
- artifact가 없는데 검사가 absence-pass라는 이유로 품질·성능을 `PASS`
- timeout을 cancellation 성공으로 간주하거나 process kill을 graceful drain으로 간주
- deterministic hash embedder 결과를 real-provider semantic quality로 간주

## 3. 우선순위와 최종 verdict

| 등급 | 정의 | 예시 |
| --- | --- | --- |
| `P0` | 검색 truth 또는 durable authority를 손상시키는 문제 | wrong generation serve, partial activation, committed data loss |
| `P1` | 핵심 유즈케이스 또는 production process 안전성을 깨는 문제 | wire incompatibility, broken restart, missing drain, silent downgrade |
| `P2` | 운영·진단·유지보수에 지속적인 위험을 만드는 문제 | incomplete CLI, missing exporter, misleading docs |
| `P3` | 핵심 목적을 깨지 않는 품질·편의 문제 | help formatting, low-risk diagnostics |

최종 verdict는 다음 중 하나다.

- `PURPOSE_GREEN`: 모든 mandatory `P0/P1` 체크가 `PASS`이고 `BLOCKED` 및
  `NOT_RUN`이 없으며, exact-source artifact와 외부 경계 receipt가 유효하다.
- `PURPOSE_RED`: mandatory `P0/P1` 체크 중 하나 이상이 `FAIL`이다.
- `PURPOSE_BLOCKED`: mandatory `P0/P1` 체크가 환경 또는 외부 authority 때문에
  `BLOCKED`다. 실행한 부분은 별도 PASS/FAIL로 유지한다.
- `PURPOSE_INCOMPLETE`: mandatory 체크가 `NOT_RUN`이다.

`P2/P3` 실패가 있어도 핵심 correctness verdict는 분리할 수 있지만,
`PRODUCTION_READY`라는 표현은 모든 mandatory production/ops/scale 체크까지
통과하기 전에는 금지한다.

기본적으로 shipped surface의 모든 `P0/P1` row가 mandatory다. 특정 provider,
syntax 또는 deployment surface가 실제 제품에서 비활성이라면 product owner가
비활성 근거와 consumer 영향이 없다는 증거를 남긴 경우에만 `N/A`로 바꿀 수 있다.
`P2/P3`도 `PRODUCTION_READY` closeout에서는 unresolved risk 목록에 반드시 남긴다.

## 4. 증거 계층

| 코드 | 증거 | 허용되는 주장 |
| --- | --- | --- |
| `S` | source/static guard | 구조, dependency, API shape, 등록 상태 |
| `U` | owner-local unit/property test | 순수 정책과 owner invariant |
| `A` | real-adapter integration | 실제 SQLite/Tantivy/LanceDB commit/open/query 의미 |
| `D` | in-process full runtime | composition과 SDK/UDS route 연결 |
| `P` | 별도 OS process | signal, crash, lease, restart, FD/socket lifecycle |
| `F` | fault/concurrency probe | crash point, corruption, race, cancellation |
| `Q` | provenanced quality/perf artifact | relevance, ANN recall, latency, RSS, QPS |
| `X` | external producer/provider | cross-repo wire와 real-provider 동작 |

각 체크의 `필수 증거`보다 낮은 계층만 있으면 `NOT_RUN` 또는 `BLOCKED`다.

## 5. 감사 snapshot 동결

감사 시작 전에 아래 값을 receipt 상단에 기록한다.

```text
audit_id:
started_at_utc:
auditor:
repo_root:
git_head:
branch:
upstream_head:
merge_base:
worktree_clean: true|false
dirty_paths:
dirty_diff_sha256:
rust_toolchain:
os_arch:
host_cpu_mem:
state_root:
socket_root:
build_lane:
semantic_provider:
model_revision:
producer_root:
producer_head:
producer_dirty_digest:
daemon_binary_sha256:
```

### G0 — snapshot과 증거 무결성

- [ ] `G0-01 P0 S` `git rev-parse HEAD`, branch, upstream, merge-base를 기록했다.
- [ ] `G0-02 P0 S` `git status --short`와 `git diff --stat`을 기록했다.
- [ ] `G0-03 P0 S` dirty 상태면 대상 path와 diff digest를 기록하고 clean HEAD
  receipt와 분리했다.
- [ ] `G0-04 P1 S` build/test가 `just` 또는 `./scripts/cargow`를 통해 실행됐다.
- [ ] `G0-05 P1 S` 각 command에 시작·종료 시각, exit code, selected/executed/
  ignored 수와 log/artifact 경로가 있다.
- [ ] `G0-06 P1 S` parallel 실행이 동일 Cargo lane, state root, socket root 또는
  perf host를 공유하지 않았다.
- [ ] `G0-07 P1 S` benchmark artifact가 schema 2이고 full 40-char HEAD,
  corpus/config/model/host/RSS provenance를 가진다.
- [ ] `G0-08 P1 S` prior receipt를 사용했다면 exact source가 동일하거나 delta
  proof로 범위를 제한했다.

기본 정적 명령:

```sh
git rev-parse HEAD
git status --short
git diff --stat
git diff --check
just rust-test-authority
just rust-ignored-test-policy
just rust-hexagonal
just rust-bench-artifacts
```

`just rust-bench-artifacts`는 artifact 부재를 기본 허용한다. 제품 품질이나 Linux
perf closeout에서는 다음과 같이 모든 fresh family 존재까지 강제해야 한다.

```sh
python3 tools/ci/lint/check-bench-artifacts.py --require
```

## 6. 목적 및 책임 경계

### G1 — owner model과 아키텍처

- [ ] `G1-01 P0 S` producer가 source/graph/search fact를 만들고 search plane은
  이를 임의 재해석하거나 raw source를 별도 authority로 사용하지 않는다.
- [ ] `G1-02 P0 S` semantic public ingest는 typed semantic sources를 받으며,
  producer-authored completed vector를 public truth로 수용하지 않는다.
- [ ] `G1-03 P0 S` search-plane/core가 Tantivy, LanceDB, SQLite, UDS 같은 concrete
  dependency를 직접 소유하지 않고 port 방향을 유지한다.
- [ ] `G1-04 P1 S` runtime composition root만 concrete adapters와 process resources를
  조립한다.
- [ ] `G1-05 P1 S` query/control/ingest plane의 opcode와 owner가 중복되지 않는다.
- [ ] `G1-06 P1 S` history, runtime, structural, RepoMap의 readiness가 unrelated
  lexical/semantic readiness를 오염시키지 않는다.
- [ ] `G1-07 P1 S` Sourcegraph syntax는 명시된 subset translator이며 전체 product
  parity 또는 reverse translation으로 오인되지 않는다.
- [ ] `G1-08 P2 S` crate 이름과 public facade가 backend/transport 이름이 아니라
  purpose boundary를 유지한다.

필수 receipt:

```sh
just rust-hexagonal
just rust-cargo-modules
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## 7. Public contract와 wire

### G2 — schema, compatibility, fail-closed decode

- [ ] `G2-01 P0 S/U` request/response DTO의 required field가 manual serialize와
  deserialize 양쪽에서 동일하다.
- [ ] `G2-02 P0 U` missing, duplicate, unknown, wrong-type, oversized field가 typed
  error로 거부되고 default 성공으로 바뀌지 않는다.
- [ ] `G2-03 P0 U/F` query/control/ingest CBOR decoder가 arbitrary bytes에서 panic,
  unbounded allocation, hang을 만들지 않는다.
- [ ] `G2-04 P0 S` 모든 opcode와 on-disk format version이 wire inventory에 있다.
- [ ] `G2-05 P0 X` producer HEAD가 발행한 payload를 current daemon이 decode하고,
  current SDK receipt validation을 통과한다.
- [ ] `G2-06 P1 X` rolling version skew를 지원한다고 주장한다면 old producer/new
  daemon, new producer/old daemon 조합의 expected accept/refuse matrix가 있다.
- [ ] `G2-07 P1 U` request ID, repo/revision/generation identity, digest 및 scope가
  response/receipt에서 원 요청과 결속된다.
- [ ] `G2-08 P1 U` public API baseline 변경은 intentional breaking change 또는
  명시적 migration decision과 연결된다.
- [ ] `G2-09 P1 D` 16 MiB 직전 frame은 정상 처리되고 초과/partial/truncated frame은
  bounded typed refusal 뒤 다음 connection을 정상 처리한다.
- [ ] `G2-10 P1 S/U` error code와 repair payload는 stable하고 machine-readable하며
  generic string 또는 silent rewrite로 축소되지 않는다.

필수 receipt:

```sh
just rust-public-api
just rust-wire-inventory
just rust-fuzz-build
just rust-fuzz-smoke 60
just rust-profile validate-shared-surface
```

Public contract 또는 wire가 dirty면 `G2-05` 전까지 전체 목적 verdict는
`PURPOSE_INCOMPLETE`다.

## 8. Ingest와 durable authority

### G3 — publish validation, idempotency, atomicity

- [ ] `G3-01 P0 A/D` malformed identity, invalid base, wrong model/profile, wrong
  scope, digest mismatch를 adapter mutation 전에 거부한다.
- [ ] `G3-02 P0 A/D` full, delta, replace, tombstone, clear-family가 independent
  golden corpus의 최종 row/root와 일치한다.
- [ ] `G3-03 P0 A/P` commit 전 crash는 committed receipt나 visible generation을
  만들지 않는다.
- [ ] `G3-04 P0 A/P` commit 후 ack 유실 뒤 same-body retry는 동일 receipt를
  반환하고 중복 row/vector를 만들지 않는다.
- [ ] `G3-05 P0 A/P` 같은 idempotency key의 다른 body는 typed conflict다.
- [ ] `G3-06 P0 A` SQLite transaction은 batch를 전부 적용하거나 전혀 적용하지
  않으며 durable sequence가 단조 증가한다.
- [ ] `G3-07 P0 A` receipt의 inserted/deleted/replaced/scope counts가 실제 committed
  mutation과 일치한다.
- [ ] `G3-08 P0 A` lexical·semantic 한쪽만 seal된 상태가 public composite
  generation으로 노출되지 않는다.
- [ ] `G3-09 P1 A` replay retention floor 이후 replay는 expired/retired/conflict를
  구분하고 임의 재적용하지 않는다.
- [ ] `G3-10 P1 F` disk full, DB busy/locked, provider error, native seal failure가
  성공 receipt나 ready 상태로 변환되지 않는다.
- [ ] `G3-11 P1 A` semantic replace/tombstone scope가 receipt, generation plan,
  persisted rows 및 SDK validation에서 동일하다.
- [ ] `G3-12 P1 A` large semantic batch의 vector residency가 configured cap 안에
  있고 전체 batch를 중복 상주시키지 않는다.

주요 rail:

```sh
just rust-profile test-integration
just rust-profile test-daemon
./scripts/cargow test -p quanta-index-catalog --test idempotency --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_ingest_idempotency --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism --all-features --locked
```

## 9. Generation lifecycle

### G4 — seal, activation, pin, rollback, retention, GC

- [ ] `G4-01 P0 A` seal proof가 lexical/semantic identity, source digest, model
  profile, scope manifest와 결속된다.
- [ ] `G4-02 P0 D` publish/seal만으로 active generation이 바뀌지 않는다.
- [ ] `G4-03 P0 D` activate는 expected-active CAS를 사용하며 stale writer를
  거부한다.
- [ ] `G4-04 P0 D` lexical과 semantic 양쪽 proof가 유효할 때만 composite active
  pointer가 이동한다.
- [ ] `G4-05 P0 D/P` activation 성공 응답 이후 current/status/query가 모두 같은
  generation을 관찰한다.
- [ ] `G4-06 P0 F` activation과 query가 경합해도 한 응답에서 generation이
  혼합되지 않는다.
- [ ] `G4-07 P0 F` pinned old generation은 query 종료 전 retire/GC/compaction에서
  물리 삭제되지 않는다.
- [ ] `G4-08 P0 P` restart가 durable active generation을 복원하고 첫 query가
  검증된 handle을 사용한다.
- [ ] `G4-09 P0 D/P` rollback도 새 activation과 동일한 proof/CAS/readiness 기준을
  적용한다.
- [ ] `G4-10 P0 F` active artifact 손상은 bind 또는 query 전에 fail-closed되고,
  다른 generation으로 silent fallback하지 않는다.
- [ ] `G4-11 P1 F` inactive artifact 손상은 active serving을 중단하지 않고
  quarantine provenance를 남긴다.
- [ ] `G4-12 P1 F` GC는 durable authority 갱신과 snapshot/pin fence 이후 마지막
  reference가 사라진 bytes만 삭제한다.
- [ ] `G4-13 P1 D` retention cap 거부는 typed reason과 operator remediation을
  제공하며 unrelated repo/pair를 임의 eviction하지 않는다.
- [ ] `G4-14 P1 F` compaction/remap 중 old reader는 old physical identity를 끝까지
  사용하고 new reader만 CAS된 mapping을 본다.

주요 rail:

```sh
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_generation_activation_concurrency --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_snapshot_registry --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_physical_gc --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_boot_quarantine --all-features --locked
```

## 10. 검색 기능 정확성

### G5 — lexical, semantic, hybrid 및 보조 route

- [ ] `G5-01 P0 D` text exact/keyword/phrase/regex가 independent golden ID와
  snippet/offset을 반환한다.
- [ ] `G5-02 P0 D` symbol search가 symbol identity, path, range, language filter를
  보존한다.
- [ ] `G5-03 P0 D` native와 지원되는 Sourcegraph syntax twin이 같은 normalized
  meaning과 result set을 만든다.
- [ ] `G5-04 P0 D` unsupported/ambiguous syntax는 repair payload가 있는 typed
  refusal이며 query를 임의 완화하지 않는다.
- [ ] `G5-05 P0 A/D` semantic exact mode 결과가 independent exhaustive cosine
  oracle과 일치한다.
- [ ] `G5-06 P0 A/Q` ANN mode가 recall floor, exact returned score, full-page 및
  live-row membership 기준을 만족한다.
- [ ] `G5-07 P0 D` semantic lexical-scope rerank가 scope 밖 candidate를 반환하지
  않는다.
- [ ] `G5-08 P0 D` hybrid는 lexical/dense independent union 후 RRF를 적용해
  semantic-only hit을 보존한다.
- [ ] `G5-09 P0 D` dense lane 실패를 lexical-only 성공으로 downgrade하지 않는다.
- [ ] `G5-10 P1 D` tie, dedup, owner collapse, top-k, cursor 순서가 restart와 ingest
  sequence에 대해 deterministic하다.
- [ ] `G5-11 P1 D` explain이 실제 query의 lexical score, dense score, lane rank,
  fusion contribution과 candidate identity를 재현한다.
- [ ] `G5-12 P1 D` history relevance/recency와 epoch-bound cursor가 mutation 중에도
  중복·누락 없는 page union을 만든다.
- [ ] `G5-13 P1 D` runtime metadata query가 declared overlay epoch와 source
  dependency를 지킨다.
- [ ] `G5-14 P1 D` structural query가 producer-authored fact만 사용하고 raw source
  parser fallback을 하지 않는다.
- [ ] `G5-15 P1 D` RepoMap neighborhood와 cluster membership이 stable key,
  deterministic order, tamper-resistant pagination을 지킨다.
- [ ] `G5-16 P1 D` count/distinct/top-k 결과가 public cap, internal probe,
  exhaustiveness를 혼동하지 않는다.
- [ ] `G5-17 P1 D` 정상 empty result와 `NOT_READY`, unsupported, budget exceeded를
  구분한다.

주요 rail:

```sh
just rust-verify-hellgate-fast
just rust-verify-hellgate-broad
just rust-bench-dsl-truth
just rust-test-e2e
```

## 11. 검색 제품 품질

### G6 — relevance, snippets, explainability, repairability

- [ ] `G6-01 P1 Q` judged corpus가 query intent, relevant IDs, hard negatives,
  route family와 corpus digest를 고정한다.
- [ ] `G6-02 P1 Q` lexical/semantic/hybrid 각각 MRR@10, NDCG@10, Recall@20 및
  top-1/top-k containment threshold를 만족한다.
- [ ] `G6-03 P1 Q/X` real-provider semantic 평가가 provider/model revision,
  request shaping, cache state와 비용을 기록한다.
- [ ] `G6-04 P1 Q` hash provider 결과는 deterministic development rail로만
  표시되고 learned semantic quality claim에 사용되지 않는다.
- [ ] `G6-05 P1 Q` snippet은 hit-centered, bounded, deterministic하며 highlight
  offset이 실제 matched bytes/codepoints와 일치한다.
- [ ] `G6-06 P1 Q` ambiguity error는 supported alternatives와 docs anchor를
  제공하되 자동 rewrite하지 않는다.
- [ ] `G6-07 P1 Q` UI contract가 opaque/debug string parsing 없이 candidate,
  provenance, error, highlight를 렌더링할 수 있다.
- [ ] `G6-08 P2 Q/X` Sourcegraph 경쟁 claim은 동일 overlap subset과 corpus에서
  실행한 artifact가 있을 때만 작성한다.
- [ ] `G6-09 P1 S/Q` aggregate quality summary가 개별 dimension의 FAIL/BLOCKED를
  덮어쓰지 않는다.

필수 rail:

```sh
just rust-verify-quality-relevance
just rust-verify-quality-ambiguity
just rust-verify-quality-snippet
just rust-verify-quality-ops
just rust-verify-quality-ui
just rust-verify-quality-all
```

Release/production semantic claim에는 추가로 다음이 필요하다.

```sh
just rust-capture-quality-relevance-openai-ab
```

이 rail은 현재 advisory이므로 실행 성공만으로 real-provider threshold가 생기지는
않는다. 감사 receipt에 승인된 threshold와 비교 결과를 별도로 기록한다.

## 12. SDK, IPC와 consumer 여정

### G7 — public front door

- [ ] `G7-01 P0 D` SDK connect → publish → seal → activate → query → status의
  실제 UDS roundtrip이 동작한다.
- [ ] `G7-02 P0 D` SDK가 request ID, digest, receipt scope/count, activation ack를
  독립적으로 검증한다.
- [ ] `G7-03 P1 D` query-only consumer가 불필요한 ingest/control socket 부재로
  실패하지 않거나, 세 socket requirement가 명시적 제품 계약이다.
- [ ] `G7-04 P1 D` connection refusal, EOF, malformed response, timeout, overload가
  서로 구분되는 typed SDK error다.
- [ ] `G7-05 P1 D` searchctl의 모든 documented query가 SDK와 동일한 semantics와
  generation provenance를 보인다.
- [ ] `G7-06 P1 P` daemon이 준비되기 전 socket 존재만으로 ready를 선언하지 않고
  protocol handshake/readiness가 성공해야 한다.
- [ ] `G7-07 P2 D` `--help`와 `--version`은 성공 exit이고 usage text가 실제
  subcommand/options와 일치한다.
- [ ] `G7-08 P2 S/D` CLI의 read-only와 mutating command가 정확히 분류되고
  operator 권한 경계가 문서화된다.
- [ ] `G7-09 P2 S` publish/activate/rollback을 SDK-only로 유지할지 admin CLI에
  노출할지 owner decision이 있다.

필수 rail:

```sh
just rust-profile test-cli-smoke
./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor --all-features --locked -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test end_to_end --all-features --locked
```

## 13. 장애, 복구와 cancellation

### G8 — corruption, crash, restart

- [ ] `G8-01 P0 P/F` lexical seal의 manifest, sidecar, segment missing/truncation/
  checksum mismatch가 typed corruption으로 검출된다.
- [ ] `G8-02 P0 P/F` semantic manifest, Lance table/index, membership, model profile
  drift가 typed corruption으로 검출된다.
- [ ] `G8-03 P0 P/F` catalog row digest 또는 DB corruption은 authority 복구를
  추정하지 않고 startup/query를 fail-closed한다.
- [ ] `G8-04 P0 P/F` write/fsync/rename/catalog-commit 각 crash point 뒤 restart가
  old active 또는 fully committed new state만 노출한다.
- [ ] `G8-05 P1 P/F` half-sealed generation은 자동 public activation되지 않고
  replace/rebuild 또는 명시적 repair를 요구한다.
- [ ] `G8-06 P1 P/F` quarantine은 exact artifact, reason, generation, observed-at
  provenance를 제공한다.
- [ ] `G8-07 P1 P/F` discard가 active/pinned/shared artifact를 삭제하지 않는다.
- [ ] `G8-08 P1 P` same state root 두 번째 daemon은 live socket을 unlink하거나
  authority를 탈취하지 못한다.

### G9 — shutdown, timeout, cancellation, concurrency

- [ ] `G9-01 P1 P` 실제 release daemon에 SIGINT를 보내면 accept 중단 → in-flight
  drain → socket/lease 정리 → bounded exit 순서가 관찰된다.
- [ ] `G9-02 P1 P` SIGTERM도 동일 semantics를 가지며 default process kill을
  graceful drain으로 오인하지 않는다.
- [ ] `G9-03 P1 P/F` forced kill 뒤 restart가 lock/socket/catalog를 복구한다.
- [ ] `G9-04 P1 F` peer disconnect와 deadline이 queue/dispatch permits를 반환한다.
- [ ] `G9-05 P1 F` cancellation 후 Tantivy/Lance/OpenAI background work의 잔존량과
  최대 생존 시간이 bounded하다.
- [ ] `G9-06 P1 F/Q` 1/8/32 client와 slow client 혼합에서 정상 client가 head-of-line
  blocking으로 starvation되지 않는다.
- [ ] `G9-07 P1 F` query/control/ingest queue 포화가 명시적 overload/deadline으로
  응답하고 daemon 전체 deadlock을 만들지 않는다.
- [ ] `G9-08 P1 F` long query, activation, ingest, GC, quarantine 교차 실행이
  deadlock, use-after-delete, mixed generation을 만들지 않는다.
- [ ] `G9-09 P1 F` OpenAI worker thread와 Lance blocking task에 process-global
  admission 또는 실측 가능한 상한이 있다.
- [ ] `G9-10 P2 S/D` explicit Cancel RPC가 없다면 caller-visible cancellation의
  보장과 non-guarantee가 SDK/ops 문서에 명시된다.

현재 `SearchdBinaryProcess::stop`의 kill/wait만으로 `G9-01/02`를 통과시킬 수
없다. signal handler가 production binary에 연결된 별도 process test가 필요하다.

## 14. 운영, 보안, 관측성

### G10 — operator가 상태를 판별하고 안전하게 운용할 수 있는가

- [ ] `G10-01 P1 D` readiness가 active generation, 각 required backend proof,
  not-ready reason을 machine-readable하게 제공한다.
- [ ] `G10-02 P1 D` doctor가 socket, state root, catalog, active artifacts,
  provider/profile, quarantine 상태를 실제 probe한다.
- [ ] `G10-03 P1 D` metrics가 request count/error/timeout/overload/latency,
  ingest/provider/cache/GC/quarantine를 route·generation을 폭발시키지 않는
  bounded cardinality로 제공한다.
- [ ] `G10-04 P1 P` log에 request ID, typed code, generation 및 operation context가
  있으며 secret/source payload를 노출하지 않는다.
- [ ] `G10-05 P1 P` socket directory와 state root의 owner/mode가 startup에서
  검증되고 insecure path는 거부된다.
- [ ] `G10-06 P1 P` 지원 OS에서 peer credential/group 정책이 실제 local socket
  client로 검증된다.
- [ ] `G10-07 P1 S/P` TLS/authz 부재가 UDS filesystem boundary에 의존한다는
  threat model 및 deployment precondition으로 명시된다.
- [ ] `G10-08 P1 P` disk full, inode exhaustion, read-only FS, file descriptor
  exhaustion에서 기존 active query가 가능한 범위와 실패 mode가 확인된다.
- [ ] `G10-09 P2 S/P` production metrics/log exporter의 owner가 이 repo 또는 외부
  platform 중 하나로 지정되고 실제 scrape/shipping receipt가 있다.
- [ ] `G10-10 P2 D` quarantine list/detail/discard와 retention refusal에 operator가
  실행 가능한 remediation을 제공한다.

주요 rail:

```sh
just rust-verify-quality-ops
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos --all-features --locked
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_boot_quarantine --all-features --locked
```

## 15. 성능과 용량

### G11 — declared tier에서의 생산 제약

- [ ] `G11-01 P1 Q` small/medium/large/xlarge tier가 docs 수, bytes, vector dim,
  active/inactive generations, query mix와 concurrency로 고정돼 있다.
- [ ] `G11-02 P1 Q` full ingest, one-file delta, seal, activate, cold open, warm query,
  GC의 wall time을 분리 측정한다.
- [ ] `G11-03 P1 Q` peak RSS, disk amplification, index bytes, open FD/thread/task
  수를 기록한다.
- [ ] `G11-04 P1 Q` route별 p50/p95/p99, QPS, error/timeout count를 golden behavior
  검증 후 기록한다.
- [ ] `G11-05 P1 Q` ANN build/query는 recall과 latency를 같은 artifact에서 보되,
  recall 미달을 빠른 latency로 상쇄하지 않는다.
- [ ] `G11-06 P1 Q` 1/8/32 concurrency에서 page-max slow client를 섞고 HOL ratio,
  saturation point, overload behavior를 기록한다.
- [ ] `G11-07 P1 Q` retention metadata walk, open-cache, regex cache, embedding cache,
  metrics label/sample count가 corpus/generation 증가에도 bounded하다.
- [ ] `G11-08 P1 Q` baseline/current가 동일 corpus/config/model/host class이며 stale
  HEAD나 contended host 결과를 blocking proof로 사용하지 않는다.
- [ ] `G11-09 P1 Q` production claim은 canonical quiet Linux host에서 반복 측정한
  blocking threshold를 만족한다.
- [ ] `G11-10 P2 Q` local macOS/advisory latency는 correctness 검증과 개발 추세로만
  사용한다.

현재 제공 rail:

```sh
just rust-verify-quality-scale
just rust-verify-quality-tail
just rust-verify-quality-ann
just rust-verify-quality-concurrency 16
just rust-bench-dsl-refresh 20
```

`scale`의 medium/large/xlarge 및 route latency는 현재 local rail에서 advisory일 수
있다. production closeout에는 canonical Linux runner의 `--require` artifact 검사가
추가돼야 한다.

## 16. Cross-repo와 실제 provider

### G12 — producer/consumer/provider 통합

- [ ] `G12-01 P0 X` producer HEAD, dirty digest, enabled features를 기록한다.
- [ ] `G12-02 P0 X` 검증 대상 release daemon binary의 source HEAD와 SHA-256을
  기록한다.
- [ ] `G12-03 P0 X` 실제 producer가 full/delta/semantic replace/tombstone payload를
  발행하고 current daemon의 receipt를 검증한다.
- [ ] `G12-04 P0 X` producer finalize와 daemon activation 사이 failure/retry가
  partial public state를 만들지 않는다.
- [ ] `G12-05 P1 X` producer query caller가 text/symbol/semantic/hybrid를 실제
  roundtrip하고 expected generation과 IDs를 확인한다.
- [ ] `G12-06 P1 X` producer와 daemon의 deploy/cutover 순서, minimum compatible
  version, rollback 순서가 명시된다.
- [ ] `G12-07 P1 X` real embedding provider가 pinned model revision, dimension,
  normalization, retry/timeout/batch/cost policy를 지킨다.
- [ ] `G12-08 P1 X` provider 429/5xx/timeout/partial response가 bounded retry 후
  typed failure이며 ready generation을 만들지 않는다.
- [ ] `G12-09 P1 X` credential은 artifact/log/cache key에 포함되지 않는다.

Cross-repo rail:

```sh
just rust-profile release-daemon-fresh
export QUANTA_INDEX_SEARCHD_BIN="<verified release binary>"
just rust-verify-hellgate-cross-repo "<semantica-codegraph-v2 root>"
```

외부 repo의 Cargo 실행 규칙도 함께 지켜야 한다. 이 rail이 producer 전체
publish/activate/query matrix를 모두 포함하지 않으면 부족한 scenario를 receipt에
명시한다.

## 17. 문서, 배포와 지원 가능성

### G13 — 실행 주장과 운영 인계

- [ ] `G13-01 P1 S/P` README의 current snapshot HEAD와 핵심 runtime 주장이 현재
  source/process probe와 일치한다.
- [ ] `G13-02 P1 S` archived/partially-superseded/planned 문서를 current SSOT로
  인용하지 않는다.
- [ ] `G13-03 P1 S` public query, control, ingest route와 CLI/SDK examples가 실제
  contract와 동기화돼 있다.
- [ ] `G13-04 P1 S/P` service owner, supported OS/arch, state/socket paths,
  permissions, resource limits, startup/shutdown/restart 절차가 있다.
- [ ] `G13-05 P1 P` deploy manifest 또는 외부 platform 정의가 health check,
  graceful timeout, restart policy, persistent state, secret injection을 실제
  binary 동작과 맞춘다.
- [ ] `G13-06 P1 P` backup/restore 또는 rebuild-from-producer 정책이 RPO/RTO와
  함께 process-level drill로 검증됐다.
- [ ] `G13-07 P1 S/P` wire/catalog/index format upgrade와 rollback runbook이 있다.
- [ ] `G13-08 P2 S` non-goal과 deferred feature가 현재 구현 기능처럼 서술되지
  않는다.
- [ ] `G13-09 P2 S` known limits에는 no HTTP, UDS auth model, observability owner,
  provider requirements, scale threshold, cancellation semantics가 포함된다.

## 18. 표준 실행 순서

무거운 rail을 먼저 실행하지 않는다. 다음 순서로 실패 범위를 좁힌다.

### Phase A — freeze와 정적 authority

```sh
git diff --check
python3 tools/prompt-manager/pm.py lint
just rust-test-authority
just rust-ignored-test-policy
just rust-hexagonal
just rust-wire-inventory
just rust-public-api
just rust-cargo-modules
just rust-bench-artifacts
```

### Phase B — compile 및 owner/adapters

```sh
just rust-profile dev-all-targets
just rust-profile test-fast
just rust-profile test-integration
just rust-profile test-cli-smoke
```

### Phase C — daemon correctness

```sh
just rust-verify-hellgate-fast
just rust-profile test-daemon
just rust-verify-hellgate-broad
```

### Phase D — standard merge gate

```sh
just rust-profile verify-rust
just rust-profile verify-rust-heavy
```

Heavy rail에 필요한 nightly/tool이 없으면 해당 항목은 `BLOCKED`이며 standard rail
PASS로 대체하지 않는다.

### Phase E — product quality

```sh
just rust-verify-quality-relevance
just rust-verify-quality-ambiguity
just rust-verify-quality-snippet
just rust-verify-quality-scale
just rust-verify-quality-tail
just rust-verify-quality-ann
just rust-verify-quality-concurrency 16
just rust-verify-quality-ops
just rust-verify-quality-ui
just rust-verify-quality-all
```

### Phase F — process, fault, Linux perf, external integration

이 단계는 현재 제공 recipe만 실행하는 것으로 끝나지 않는다.

- 실제 release binary SIGINT/SIGTERM drain probe
- forced-kill/restart 및 crash-point matrix
- disk-full/FD exhaustion/permission fault probe
- canonical Linux scale/tail/concurrency run
- real OpenAI provider relevance/failure probe
- exact-head Semantica producer publish/activate/query roundtrip
- deploy manifest 기반 start/health/stop/restart drill

## 19. 감사 결과 기록 템플릿

각 finding은 아래 형식을 사용한다.

```text
check_id:
status: PASS|FAIL|BLOCKED|NOT_RUN|N/A
severity: P0|P1|P2|P3
claim:
frozen_head:
dirty_digest:
proof_type: S|U|A|D|P|F|Q|X
command:
selected_executed_ignored:
exit_code:
artifact_or_log:
source_evidence:
oracle_expected:
observed:
failure_class:
reproduction:
owner:
remaining_seam:
```

Gate별 closeout 표:

| Gate | 목적 | 상태 | 가장 강한 증거 | 미검증 seam | finding IDs |
| --- | --- | --- | --- | --- | --- |
| G0 | snapshot/evidence |  |  |  |  |
| G1 | owner/architecture |  |  |  |  |
| G2 | public contract/wire |  |  |  |  |
| G3 | ingest/durability |  |  |  |  |
| G4 | generation lifecycle |  |  |  |  |
| G5 | query correctness |  |  |  |  |
| G6 | product quality |  |  |  |  |
| G7 | SDK/CLI/IPC |  |  |  |  |
| G8 | corruption/recovery |  |  |  |  |
| G9 | shutdown/cancellation/concurrency |  |  |  |  |
| G10 | operations/security/observability |  |  |  |  |
| G11 | performance/capacity |  |  |  |  |
| G12 | producer/provider integration |  |  |  |  |
| G13 | docs/deployment/runbook |  |  |  |  |

## 20. 현재 감사 시작점에서 우선 확인할 항목

이 목록은 최종 판정이 아니라 다음 감사의 초기 routing이다.

1. `G9-01/G9-02`: README의 SIGINT/SIGTERM drain 주장과 production signal wiring을
   별도 process probe로 확인한다.
2. `G2-05/G3-11/G12-03`: dirty `BatchPublishReceipt` semantic scope 필드 변경을
   producer exact-head와 roundtrip한다.
3. `G0-07/G11-08/G11-09`: stale local benchmark artifact를 authority에서 제외하고
   clean exact-head artifact를 canonical host에서 재생성한다.
4. `G6-03/G12-07/G12-08`: real-provider relevance와 failure policy를 release
   cadence에서 실행한다.
5. `G9-05/G9-09`: cancelled Lance task와 OpenAI worker thread의 잔존 작업 상한을
   실측한다.
6. `G7-03/G7-07/G7-08/G7-09`: query-only SDK socket requirement와 searchctl
   help/read-only/admin 경계를 결정한다.
7. `G10-09/G13-04/G13-05`: exporter 및 service packaging owner가 외부인지 이
   repo인지 확정하고 실제 deployment receipt를 연결한다.
8. `G11-09`: large corpus/RSS/tail/concurrency를 quiet Linux host에서 blocking
   threshold로 검증한다.

## 21. 완료 조건

감사는 다음을 모두 만족해야 종료할 수 있다.

- 모든 mandatory row가 `PASS`, `FAIL`, `BLOCKED`, `N/A` 중 하나로 분류됨
- `NOT_RUN`인 mandatory row가 없음
- 모든 `PASS`가 요구된 증거 계층과 exact-source receipt를 가짐
- 모든 `FAIL`에 재현 절차, 영향, owner가 있음
- 모든 `BLOCKED`에 부족한 input/environment와 해제 조건이 있음
- 과거 GREEN, focused test, advisory artifact를 전체 closure로 승격하지 않음
- 최종 verdict가 `PURPOSE_GREEN`, `PURPOSE_RED`, `PURPOSE_BLOCKED` 중 하나임
- production claim은 process, external provider/producer 및 canonical Linux
  evidence가 모두 있을 때만 사용함
