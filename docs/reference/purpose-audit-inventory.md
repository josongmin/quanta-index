# quanta-index 목적 적합성 감사 항목

Status: `REFERENCE_INVENTORY`. 미완료 작업 원장이 아닌 재사용 감사 기준이다.
선택한 감사의 실행 상태는 해당 잔여 원장에서 관리한다.

현재 producer → SDK → publish/seal → activation → query → recovery 경로를
독립 oracle로 감사한다. 이 문서는 항목 목록이며 실행 결과나 구현 완료 선언이 아니다.

Authority: current source와 해당 범위의 실행 결과 → Justfile/test·wire inventory →
[accepted ADRs](../adr/README.md). 역사적 보고서와 등록 상태는 현재 통과 증거가 아니다.
Routine 검사는 [AGENTS.md](../../AGENTS.md)를 따른다. 정식 purpose/release/benchmark
주장은 선택한 계약에 필요한 source/input/config/binary/host와 raw 결과를 결속한다.

## 판정과 증거 범위

- `VERIFIED`: 요구한 범위가 실제 실행되어 독립 oracle을 만족함.
- `FAILED`: 실행한 검사가 요구 동작과 다른 결과를 냄.
- `BLOCKED`: 필수 host/provider/producer/credential/fixture가 없거나 유효하지 않음.
- `NOT_RUN`: 필요한 검사를 실행하지 않음. `NOT_APPLICABLE`: 범위 밖인 근거가 있음.
- P0: search/durable truth 손상. P1: 핵심 기능/process 안전성. P2: 운영·유지보수.
  P3: 낮은 영향의 편의. Shipped P0/P1은 기본 mandatory이며 비활성 product surface는
  owner 결정과 consumer 영향 부재 근거가 있어야 제외한다.

| 증거 | 허용 범위 |
| --- | --- |
| S / U | Source·등록·API shape / owner-local unit·property invariant |
| A / D | 실제 SQLite/Tantivy/LanceDB / full runtime·SDK·UDS composition |
| P / F | 별도 OS process·signal·lease / fault·crash·race·cancellation |
| Q / X | Admitted relevance·latency·RSS·QPS / 실제 external producer·provider |

하위 계층, compile, zero/partial selection, absence-pass, stale receipt, hash embedder,
forced kill 또는 다른 platform 결과로 상위 주장을 통과시키지 않는다. Timeout·미준비·
손상·unsupported·budget refusal과 정상 empty를 구분한다. 미측정은 null이며 zero가 아니다.

Purpose verdict: mandatory P0/P1이 모두 `VERIFIED`이면 `PURPOSE_GREEN`, 실패가 있으면
`PURPOSE_RED`, 필수 input 부재면 `PURPOSE_BLOCKED`, 미실행이면 `PURPOSE_INCOMPLETE`.
Production-ready는 필수 process/ops/scale/external 조건까지 통과해야 하며 P2/P3 잔여도
공개한다. 일부 통과를 전체 qualification으로 승격하지 않는다.

## 감사 항목

기존 G0–G13 ID, severity와 최소 증거 계층을 유지한다. 명령·threshold·format의 현재 값은
코드/registry와 ADR에서 조회하고, 각 row에 expected/observed와 covered/excluded scope를 기록한다.

### G0 — snapshot과 증거 무결성

- `G0-01 P0 S` `git rev-parse HEAD`, branch, upstream, merge-base를 기록했다.
- `G0-02 P0 S` `git status --short`와 `git diff --stat`을 기록했다.
- `G0-03 P0 S` dirty 상태면 대상 path와 diff digest를 기록하고 clean HEAD receipt와 분리했다.
- `G0-04 P1 S` build/test가 `just` 또는 `./scripts/cargow`를 통해 실행됐다.
- `G0-05 P1 S` 각 command에 시작·종료 시각, exit code, selected/executed/ ignored 수와 log/artifact 경로가 있다.
- `G0-06 P1 S` parallel 실행이 동일 Cargo lane, state root, socket root 또는 perf host를 공유하지 않았다.
- `G0-07 P1 S` native BenchArtifactV1은 schema 2이며 full HEAD와 corpus/config/model/host/RSS provenance를 가진다. 다른 artifact는 자신의 현재 schema를 따른다.
- `G0-08 P1 S` prior receipt를 사용했다면 exact source가 동일하거나 delta proof로 범위를 제한했다.

### G1 — owner model과 아키텍처

- `G1-01 P0 S` producer가 source/graph/search fact를 만들고 search plane은 이를 임의 재해석하거나 raw source를 별도 authority로 사용하지 않는다.
- `G1-02 P0 S` semantic public ingest는 typed semantic sources를 받으며, producer-authored completed vector를 public truth로 수용하지 않는다.
- `G1-03 P0 S` search-plane/core가 Tantivy, LanceDB, SQLite, UDS 같은 concrete dependency를 직접 소유하지 않고 port 방향을 유지한다.
- `G1-04 P1 S` runtime composition root만 concrete adapters와 process resources를 조립한다.
- `G1-05 P1 S` query/control/ingest plane의 opcode와 owner가 중복되지 않는다.
- `G1-06 P1 S` history, runtime, structural, RepoMap의 readiness가 unrelated lexical/semantic readiness를 오염시키지 않는다.
- `G1-07 P1 S` Sourcegraph syntax는 명시된 subset translator이며 전체 product parity 또는 reverse translation으로 오인되지 않는다.
- `G1-08 P2 S` crate 이름과 public facade가 backend/transport 이름이 아니라 purpose boundary를 유지한다.

### G2 — schema, compatibility, fail-closed decode

- `G2-01 P0 S/U` request/response DTO의 required field가 manual serialize와 deserialize 양쪽에서 동일하다.
- `G2-02 P0 U` missing, duplicate, unknown, wrong-type, oversized field가 typed error로 거부되고 default 성공으로 바뀌지 않는다.
- `G2-03 P0 U/F` query/control/ingest CBOR decoder가 arbitrary bytes에서 panic, unbounded allocation, hang을 만들지 않는다.
- `G2-04 P0 S` 모든 opcode와 on-disk format version이 wire inventory에 있다.
- `G2-05 P0 X` producer HEAD가 발행한 payload를 current daemon이 decode하고, current SDK receipt validation을 통과한다.
- `G2-06 P1 X` rolling version skew를 지원한다고 주장한다면 old producer/new daemon, new producer/old daemon 조합의 expected accept/refuse matrix가 있다.
- `G2-07 P1 U` request ID, repo/revision/generation identity, digest 및 scope가 response/receipt에서 원 요청과 결속된다.
- `G2-08 P1 U` public API baseline 변경은 intentional breaking change 또는 명시적 migration decision과 연결된다.
- `G2-09 P1 D` 16 MiB 직전 frame은 정상 처리되고 초과/partial/truncated frame은 bounded typed refusal 뒤 다음 connection을 정상 처리한다.
- `G2-10 P1 S/U` error code와 repair payload는 stable하고 machine-readable하며 generic string 또는 silent rewrite로 축소되지 않는다.

### G3 — publish validation, idempotency, atomicity

- `G3-01 P0 A/D` malformed identity, invalid base, wrong model/profile, wrong scope, digest mismatch를 adapter mutation 전에 거부한다.
- `G3-02 P0 A/D` full, delta, replace, tombstone, clear-family가 independent golden corpus의 최종 row/root와 일치한다.
- `G3-03 P0 A/P` commit 전 crash는 committed receipt나 visible generation을 만들지 않는다.
- `G3-04 P0 A/P` commit 후 ack 유실 뒤 same-body retry는 동일 receipt를 반환하고 중복 row/vector를 만들지 않는다.
- `G3-05 P0 A/P` 같은 idempotency key의 다른 body는 typed conflict다.
- `G3-06 P0 A` SQLite transaction은 batch를 전부 적용하거나 전혀 적용하지 않으며 durable sequence가 단조 증가한다.
- `G3-07 P0 A` receipt의 inserted/deleted/replaced/scope counts가 실제 committed mutation과 일치한다.
- `G3-08 P0 A` lexical·semantic 한쪽만 seal된 상태가 public composite generation으로 노출되지 않는다.
- `G3-09 P1 A` replay retention floor 이후 replay는 expired/retired/conflict를 구분하고 임의 재적용하지 않는다.
- `G3-10 P1 F` disk full, DB busy/locked, provider error, native seal failure가 성공 receipt나 ready 상태로 변환되지 않는다.
- `G3-11 P1 A` semantic replace/tombstone scope가 receipt, generation plan, persisted rows 및 SDK validation에서 동일하다.
- `G3-12 P1 A` large semantic batch의 vector residency가 configured cap 안에 있고 전체 batch를 중복 상주시키지 않는다.

### G4 — seal, activation, pin, rollback, retention, GC

- `G4-01 P0 A` seal proof가 lexical/semantic identity, source digest, model profile, scope manifest와 결속된다.
- `G4-02 P0 D` publish/seal만으로 active generation이 바뀌지 않는다.
- `G4-03 P0 D` activate는 expected-active CAS를 사용하며 stale writer를 거부한다.
- `G4-04 P0 D` lexical과 semantic 양쪽 proof가 유효할 때만 composite active pointer가 이동한다.
- `G4-05 P0 D/P` activation 성공 응답 이후 current/status/query가 모두 같은 generation을 관찰한다.
- `G4-06 P0 F` activation과 query가 경합해도 한 응답에서 generation이 혼합되지 않는다.
- `G4-07 P0 F` pinned old generation은 query 종료 전 retire/GC/compaction에서 물리 삭제되지 않는다.
- `G4-08 P0 P` restart가 durable active generation을 복원하고 첫 query가 검증된 handle을 사용한다.
- `G4-09 P0 D/P` rollback도 새 activation과 동일한 proof/CAS/readiness 기준을 적용한다.
- `G4-10 P0 F` active artifact 손상은 bind 또는 query 전에 fail-closed되고, 다른 generation으로 silent fallback하지 않는다.
- `G4-11 P1 F` inactive artifact 손상은 active serving을 중단하지 않고 quarantine provenance를 남긴다.
- `G4-12 P1 F` GC는 durable authority 갱신과 snapshot/pin fence 이후 마지막 reference가 사라진 bytes만 삭제한다.
- `G4-13 P1 D` retention cap 거부는 typed reason과 operator remediation을 제공하며 unrelated repo/pair를 임의 eviction하지 않는다.
- `G4-14 P1 F` compaction/remap 중 old reader는 old physical identity를 끝까지 사용하고 new reader만 CAS된 mapping을 본다.

### G5 — lexical, semantic, hybrid 및 보조 route

- `G5-01 P0 D` text exact/keyword/phrase/regex가 independent golden ID와 snippet/offset을 반환한다.
- `G5-02 P0 D` symbol search가 symbol identity, path, range, language filter를 보존한다.
- `G5-03 P0 D` native와 지원되는 Sourcegraph syntax twin이 같은 normalized meaning과 result set을 만든다.
- `G5-04 P0 D` unsupported/ambiguous syntax는 repair payload가 있는 typed refusal이며 query를 임의 완화하지 않는다.
- `G5-05 P0 A/D` semantic exact mode 결과가 independent exhaustive cosine oracle과 일치한다.
- `G5-06 P0 A/Q` ANN mode가 recall floor, exact returned score, full-page 및 live-row membership 기준을 만족한다.
- `G5-07 P0 D` semantic lexical-scope rerank가 scope 밖 candidate를 반환하지 않는다.
- `G5-08 P0 D` hybrid는 lexical/dense independent union 후 RRF를 적용해 semantic-only hit을 보존한다.
- `G5-09 P0 D` dense lane 실패를 lexical-only 성공으로 downgrade하지 않는다.
- `G5-10 P1 D` tie, dedup, owner collapse, top-k, cursor 순서가 restart와 ingest sequence에 대해 deterministic하다.
- `G5-11 P1 D` explain이 실제 query의 lexical score, dense score, lane rank, fusion contribution과 candidate identity를 재현한다.
- `G5-12 P1 D` history relevance/recency와 epoch-bound cursor가 mutation 중에도 중복·누락 없는 page union을 만든다.
- `G5-13 P1 D` runtime metadata query가 declared overlay epoch와 source dependency를 지킨다.
- `G5-14 P1 D` structural query가 producer-authored fact만 사용하고 raw source parser fallback을 하지 않는다.
- `G5-15 P1 D` RepoMap neighborhood와 cluster membership이 stable key, deterministic order, tamper-resistant pagination을 지킨다.
- `G5-16 P1 D` count/distinct/top-k 결과가 public cap, internal probe, exhaustiveness를 혼동하지 않는다.
- `G5-17 P1 D` 정상 empty result와 `NOT_READY`, unsupported, budget exceeded를 구분한다.

### G6 — relevance, snippets, explainability, repairability

- `G6-01 P1 Q` judged corpus가 query intent, relevant IDs, hard negatives, route family와 corpus digest를 고정한다.
- `G6-02 P1 Q` lexical/semantic/hybrid 각각 MRR@10, NDCG@10, Recall@20 및 top-1/top-k containment threshold를 만족한다.
- `G6-03 P1 Q/X` real-provider semantic 평가가 provider/model revision, request shaping, cache state와 비용을 기록한다.
- `G6-04 P1 Q` hash provider 결과는 deterministic development rail로만 표시되고 learned semantic quality claim에 사용되지 않는다.
- `G6-05 P1 Q` snippet은 hit-centered, bounded, deterministic하며 highlight offset이 실제 matched bytes/codepoints와 일치한다.
- `G6-06 P1 Q` ambiguity error는 supported alternatives와 docs anchor를 제공하되 자동 rewrite하지 않는다.
- `G6-07 P1 Q` UI contract가 opaque/debug string parsing 없이 candidate, provenance, error, highlight를 렌더링할 수 있다.
- `G6-08 P2 Q/X` Sourcegraph 경쟁 claim은 동일 overlap subset과 corpus에서 실행한 artifact가 있을 때만 작성한다.
- `G6-09 P1 S/Q` aggregate quality summary가 개별 dimension의 FAIL/BLOCKED를 덮어쓰지 않는다.

### G7 — public front door

- `G7-01 P0 D` SDK connect → publish → seal → activate → query → status의 실제 UDS roundtrip이 동작한다.
- `G7-02 P0 D` SDK가 request ID, digest, receipt scope/count, activation ack를 독립적으로 검증한다.
- `G7-03 P1 D` query-only consumer가 불필요한 ingest/control socket 부재로 실패하지 않거나, 세 socket requirement가 명시적 제품 계약이다.
- `G7-04 P1 D` connection refusal, EOF, malformed response, timeout, overload가 서로 구분되는 typed SDK error다.
- `G7-05 P1 D` searchctl의 모든 documented query가 SDK와 동일한 semantics와 generation provenance를 보인다.
- `G7-06 P1 P` daemon이 준비되기 전 socket 존재만으로 ready를 선언하지 않고 protocol handshake/readiness가 성공해야 한다.
- `G7-07 P2 D` `--help`와 `--version`은 성공 exit이고 usage text가 실제 subcommand/options와 일치한다.
- `G7-08 P2 S/D` CLI의 read-only와 mutating command가 정확히 분류되고 operator 권한 경계가 문서화된다.
- `G7-09 P2 S` publish/activate/rollback을 SDK-only로 유지할지 admin CLI에 노출할지 owner decision이 있다.

### G8 — corruption, crash, restart

- `G8-01 P0 P/F` lexical seal의 manifest, sidecar, segment missing/truncation/ checksum mismatch가 typed corruption으로 검출된다.
- `G8-02 P0 P/F` semantic manifest, Lance table/index, membership, model profile drift가 typed corruption으로 검출된다.
- `G8-03 P0 P/F` catalog row digest 또는 DB corruption은 authority 복구를 추정하지 않고 startup/query를 fail-closed한다.
- `G8-04 P0 P/F` write/fsync/rename/catalog-commit 각 crash point 뒤 restart가 old active 또는 fully committed new state만 노출한다.
- `G8-05 P1 P/F` half-sealed generation은 자동 public activation되지 않고 replace/rebuild 또는 명시적 repair를 요구한다.
- `G8-06 P1 P/F` quarantine은 exact artifact, reason, generation, observed-at provenance를 제공한다.
- `G8-07 P1 P/F` discard가 active/pinned/shared artifact를 삭제하지 않는다.
- `G8-08 P1 P` same state root 두 번째 daemon은 live socket을 unlink하거나 authority를 탈취하지 못한다.

### G9 — shutdown, timeout, cancellation, concurrency

- `G9-01 P1 P` 실제 release daemon에 SIGINT를 보내면 accept 중단 → in-flight drain → socket/lease 정리 → bounded exit 순서가 관찰된다.
- `G9-02 P1 P` SIGTERM도 동일 semantics를 가지며 default process kill을 graceful drain으로 오인하지 않는다.
- `G9-03 P1 P/F` forced kill 뒤 restart가 lock/socket/catalog를 복구한다.
- `G9-04 P1 F` peer disconnect와 deadline이 queue/dispatch permits를 반환한다.
- `G9-05 P1 F` cancellation 후 Tantivy/Lance/OpenAI background work의 잔존량과 최대 생존 시간이 bounded하다.
- `G9-06 P1 F/Q` 1/8/32 client와 slow client 혼합에서 정상 client가 head-of-line blocking으로 starvation되지 않는다.
- `G9-07 P1 F` query/control/ingest queue 포화가 명시적 overload/deadline으로 응답하고 daemon 전체 deadlock을 만들지 않는다.
- `G9-08 P1 F` long query, activation, ingest, GC, quarantine 교차 실행이 deadlock, use-after-delete, mixed generation을 만들지 않는다.
- `G9-09 P1 F` OpenAI worker thread와 Lance blocking task에 process-global admission 또는 실측 가능한 상한이 있다.
- `G9-10 P2 S/D` explicit Cancel RPC가 없다면 caller-visible cancellation의 보장과 non-guarantee가 SDK/ops 문서에 명시된다.

### G10 — operator가 상태를 판별하고 안전하게 운용할 수 있는가

- `G10-01 P1 D` readiness가 active generation, 각 required backend proof, not-ready reason을 machine-readable하게 제공한다.
- `G10-02 P1 D` doctor가 socket, state root, catalog, active artifacts, provider/profile, quarantine 상태를 실제 probe한다.
- `G10-03 P1 D` metrics가 request count/error/timeout/overload/latency, ingest/provider/cache/GC/quarantine를 route·generation을 폭발시키지 않는 bounded cardinality로 제공한다.
- `G10-04 P1 P` log에 request ID, typed code, generation 및 operation context가 있으며 secret/source payload를 노출하지 않는다.
- `G10-05 P1 P` socket directory와 state root의 owner/mode가 startup에서 검증되고 insecure path는 거부된다.
- `G10-06 P1 P` 지원 OS에서 peer credential/group 정책이 실제 local socket client로 검증된다.
- `G10-07 P1 S/P` TLS/authz 부재가 UDS filesystem boundary에 의존한다는 threat model 및 deployment precondition으로 명시된다.
- `G10-08 P1 P` disk full, inode exhaustion, read-only FS, file descriptor exhaustion에서 기존 active query가 가능한 범위와 실패 mode가 확인된다.
- `G10-09 P2 S/P` production metrics/log exporter의 owner가 이 repo 또는 외부 platform 중 하나로 지정되고 실제 scrape/shipping receipt가 있다.
- `G10-10 P2 D` quarantine list/detail/discard와 retention refusal에 operator가 실행 가능한 remediation을 제공한다.

### G11 — declared tier에서의 생산 제약

- `G11-01 P1 Q` small/medium/large/xlarge tier가 docs 수, bytes, vector dim, active/inactive generations, query mix와 concurrency로 고정돼 있다.
- `G11-02 P1 Q` full ingest, one-file delta, seal, activate, cold open, warm query, GC의 wall time을 분리 측정한다.
- `G11-03 P1 Q` peak RSS, disk amplification, index bytes, open FD/thread/task 수를 기록한다.
- `G11-04 P1 Q` route별 p50/p95/p99, QPS, error/timeout count를 golden behavior 검증 후 기록한다.
- `G11-05 P1 Q` ANN build/query는 recall과 latency를 같은 artifact에서 보되, recall 미달을 빠른 latency로 상쇄하지 않는다.
- `G11-06 P1 Q` 1/8/32 concurrency에서 page-max slow client를 섞고 HOL ratio, saturation point, overload behavior를 기록한다.
- `G11-07 P1 Q` retention metadata walk, open-cache, regex cache, embedding cache, metrics label/sample count가 corpus/generation 증가에도 bounded하다.
- `G11-08 P1 Q` baseline/current가 동일 corpus/config/model/host class이며 stale HEAD나 contended host 결과를 blocking proof로 사용하지 않는다.
- `G11-09 P1 Q` production claim은 canonical quiet Linux host에서 반복 측정한 blocking threshold를 만족한다.
- `G11-10 P2 Q` local macOS/advisory latency는 correctness 검증과 개발 추세로만 사용한다.

### G12 — producer/consumer/provider 통합

- `G12-01 P0 X` producer HEAD, dirty digest, enabled features를 기록한다.
- `G12-02 P0 X` 검증 대상 release daemon binary의 source HEAD와 SHA-256을 기록한다.
- `G12-03 P0 X` 실제 producer가 full/delta/semantic replace/tombstone payload를 발행하고 current daemon의 receipt를 검증한다.
- `G12-04 P0 X` producer finalize와 daemon activation 사이 failure/retry가 partial public state를 만들지 않는다.
- `G12-05 P1 X` producer query caller가 text/symbol/semantic/hybrid를 실제 roundtrip하고 expected generation과 IDs를 확인한다.
- `G12-06 P1 X` producer와 daemon의 deploy/cutover 순서, minimum compatible version, rollback 순서가 명시된다.
- `G12-07 P1 X` real embedding provider가 pinned model revision, dimension, normalization, retry/timeout/batch/cost policy를 지킨다.
- `G12-08 P1 X` provider 429/5xx/timeout/partial response가 bounded retry 후 typed failure이며 ready generation을 만들지 않는다.
- `G12-09 P1 X` credential은 artifact/log/cache key에 포함되지 않는다.

### G13 — 실행 주장과 운영 인계

- `G13-01 P1 S/P` README의 핵심 runtime 주장이 현재 source/process probe와 일치한다. README에 실행별 HEAD를 삽입하지 않는다.
- `G13-02 P1 S` archived/partially-superseded/planned 문서를 current SSOT로 인용하지 않는다.
- `G13-03 P1 S` public query, control, ingest route와 CLI/SDK examples가 실제 contract와 동기화돼 있다.
- `G13-04 P1 S/P` service owner, supported OS/arch, state/socket paths, permissions, resource limits, startup/shutdown/restart 절차가 있다.
- `G13-05 P1 P` deploy manifest 또는 외부 platform 정의가 health check, graceful timeout, restart policy, persistent state, secret injection을 실제 binary 동작과 맞춘다.
- `G13-06 P1 P` backup/restore 또는 rebuild-from-producer 정책이 RPO/RTO와 함께 process-level drill로 검증됐다.
- `G13-07 P1 S/P` wire/catalog/index format upgrade와 rollback runbook이 있다.
- `G13-08 P2 S` non-goal과 deferred feature가 현재 구현 기능처럼 서술되지 않는다.
- `G13-09 P2 S` known limits에는 no HTTP, UDS auth model, observability owner, provider requirements, scale threshold, cancellation semantics가 포함된다.

## 실행 선택과 종료 조건

| 범위 | 현재 실행 owner / 제한 |
| --- | --- |
| 정적·owner·adapter·CLI·daemon | [루트 build/verification usage](../../README.md#build-and-verification), Justfile과 test-authority에서 해당 selector를 선택 |
| Query truth·generation·fault | [검증/품질 결정](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md), 해당 실제 backend/process oracle |
| 품질·Linux performance | [벤치 usage](../../tools/benchmark/README.md), [품질 잔여](../plans/oct-4-parallel-closure/tickets/INDEX.md#legacy-scope-routes); 필요 native artifact는 `check-bench-artifacts.py --require`, local fixture/advisory를 qualified 결과로 대체 금지 |
| Producer·운영·release | [SEP-21 residual plan](../plans/oct-4-parallel-closure/tickets/INDEX.md#release-and-proof), [state usage](../operator/state-cutover-runbook.md) |
| Test coverage·mutation·model | [검증 잔여 board](../plans/oct-4-parallel-closure/tickets/INDEX.md#test-and-platform) |

정적 blocker → owner/adapter → 실제 runtime/process → external/quality 순서로 비용을
올린다. 전체 recipe 목록을 중복 실행하지 않는다. 같은 source의 quality aggregate가
요구 producer를 이미 실행하면 개별 producer 반복은 남은 위험이 있을 때만 한다.
공유 Cargo lane/state/socket/host의 동시 writer는 분리하고 timing은 조용한 host에서 직렬 실행한다.
Missing nightly/tool/provider는 해당 범위만 `BLOCKED`이며 작은 rail로 대체하지 않는다.

SIGINT/SIGTERM graceful drain은 별도 production-binary signal 관찰이 필요하다.
Kill/wait만으로 통과하지 않는다. Cancellation 뒤 native/provider 잔존 작업 상한,
1/8/32 HOL·overload, disk/FD/permission 장애는 실제 fault/process 결과로 확인한다.
Advisory OpenAI A/B나 scale/tail 실행은 승인된 real-provider·대형 tier·Linux threshold가 아니다.
Cross-repo recipe가 전체 publish/activate/query matrix를 실행하지 않으면 빠진 scenario를 남긴다.

Finding 최소 필드: check ID, 상태, severity, owner, claim, source/dirty, proof type,
command, selected/executed/ignored, exit, independent expected, observed, covered/excluded,
failure/reproduction 또는 blocked input·해제 조건. Artifact/log와 필요한 input/dependency/
config/binary/host identity는 선택한 formal 계약에 따라 기록한다. 일회성 증거를 repo에 만들지 않는다.

감사를 종료할 때 모든 mandatory row를 분류하고 unresolved/NOT_RUN을 숨기지 않는다.
각 `VERIFIED`는 요구한 증거 계층을 충족해야 한다. 이 목록 압축은 어느 row도 닫지 않는다.
