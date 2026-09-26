# quanta-index 목적 적합성 2차 정적 감사

> Archive classification: historical static audit at the source recorded below. See [SEP-27-001](../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md) and [the documentation archive](../ARCHIVE-INDEX.md). Revalidate every finding against current source.


- 감사일: 2026-09-21
- 방식: 소스, 계약, 문서, CI recipe, 기존 artifact만 정적으로 검토
- 실행하지 않은 것: 테스트, 빌드, lint, benchmark, daemon, provider 호출
- 기준: `docs/analysis/quanta-index-purpose-validation-checklist.md`
- 1차 보고서: `docs/analysis/quanta-index-purpose-static-audit-2026-09-21.md`
- quanta-index HEAD: `3ad279a08879de35fa96a5495a3382af28f095d0`
- branch/upstream: `main` / `4914156f4191daa3e12998bdb38f2b821a057fdd`
- upstream 대비: ahead 145, behind 0
- 종료 시 tracked diff digest: `9183ae4b869e4b0cb12a8ef36fa869c1a7bcf5a34fb85d1be7ee7ad82a0f0b6c`
- 기준 체크리스트 digest: `af7d1d9ff271d819512d65a956aa7ff58a0e2ac4031a794f64ac638f1d4bf215`
- 1차 보고서 digest: `dd573b384e005e44c865fe0cf0792bfa5d7fbdd0b23add13c52817604379fcf0`

## 1. 결론

**2차 정적 종합 판정: PURPOSE_RED**

1차에서 확인한 signal handler 부재, cancellation 이후 residual work, 품질 증거의
source binding 부족, 운영/복구 authority 부족은 유지된다. 2차 감사에서는 더 직접적인
제품 정합성 결함이 확인됐다.

1. RepoMap은 active generation의 snapshot을 새 activation 없이 덮어쓸 수 있고,
   activation request의 digest도 저장된 snapshot에 결속하지 않는다.
2. 이미 finalize된 publish replay가 현재의 mutable preflight를 먼저 통과해야 하므로,
   base GC나 policy/config 변경 뒤 original receipt replay가 실패할 수 있다.
3. public cursor가 full generation pin과 canonical query에 결속되지 않아 다른 repo/query에서
   client-controlled seek boundary로 재사용될 수 있다.
4. hybrid 계열에 nested cap 검증, typed fusion identity, explain reconciliation/provenance
   정합성 결함이 있다.
5. control socket은 read와 mutate operation을 같은 UID/GID admission으로 허용하고
   operation-level authorization을 하지 않는다.
6. daemon은 plane thread를 감독하지 않고 startup rollback 및 bounded shutdown 상한도 없다.
7. quality/performance artifact gate는 artifact 부재를 허용하고 일부 summary는 exact source와
   독립 oracle에 결속되지 않는다.

현재 source는 감사 중 계속 변했다. 따라서 본 보고서는 위 digest의 dirty tracked snapshot과
변화 목록에 없던 clean tracked source를 기준으로 한 정적 판정이다. clean source와 실행 증거를
의미하는 release approval이 아니다.

## 2. 감사 중 source drift

- quanta-index tracked diff digest가 감사 시작 `310c9360...`에서 종료
  `9183ae4b...`로 변했다.
- dirty path도 9개에서 prompt/control 문서, Justfile, contract/SDK, test authority 등을
  포함하는 21개 tracked file과 여러 untracked file로 증가했다.
- sibling Semantica도 감사 중 HEAD와 dirty 상태가 변했다.
- 따라서 G0 exact snapshot과 G12 cross-repo exact pair는 `BLOCKED`다.
- RepoMap store, ingest dispatcher, cursor, query routes, daemon supervisor, IPC server 등
  아래 핵심 finding의 source는 감사 중 tracked change 목록에 없었다.
- 중간에 관찰했던 `check-test-authority.py` 함수 splice 오류는 종료 snapshot에서 수정됐다.
  최종 결함으로 계상하지 않는다.

## 3. 2차 우선순위 결함

### P0-1. RepoMap active generation을 activation 없이 교체할 수 있음

판정: `FAIL_STATIC`

근거:

- `ingest_bundle`은 bundle을 materialize한 뒤 기존 동일 generation의 digest를 비교하지 않고
  `insert_snapshot`을 호출한다.
  - `crates/quanta-index-repomap/src/store.rs:117-152`
- snapshot key와 persisted filename은 `(repo, revision, generation)`만 사용한다.
  - `crates/quanta-index-repomap/src/store.rs:36-53`
  - `crates/quanta-index-repomap/src/persistence.rs:732-750`
- active authority는 numeric generation만 저장한다.
  - `crates/quanta-index-repomap/src/store.rs:23-26`
- query는 active generation 여부를 확인한 뒤 같은 key의 현재 snapshot을 읽는다.
  - `crates/quanta-index-repomap/src/store.rs:259-285`
  - `crates/quanta-index-repomap/src/store.rs:323-348`

도달 경로:

1. generation `N`의 bundle A를 ingest하고 activate한다.
2. 같은 `(repo, revision, N)`에 내용이 다른 bundle B를 ingest한다.
3. map insert와 persistence가 같은 key를 덮어쓴다.
4. 새 activation/CAS 없이 active generation `N`의 query-visible content가 B로 바뀐다.

영향:

- immutable generation 불변식 파손
- publish와 activate 분리 파손
- activation receipt와 실제 content의 결속 상실
- retry가 idempotent replay가 아니라 active content mutation이 될 수 있음

필요 조치:

- 동일 generation 재publish는 stored manifest/authority/snapshot/content digest가 모두 같을 때만
  idempotent success로 처리한다.
- 하나라도 다르면 typed `GENERATION_CONTENT_CONFLICT`로 거부한다.
- active generation overwrite는 별도 조건 없이 금지한다.
- RepoMap ingest도 canonical body digest와 durable replay authority를 가져야 한다.

### P0-2. RepoMap activation digest가 stored snapshot에 결속되지 않음

판정: `FAIL_STATIC`

근거:

- activation은 request의 `manifest_digest`가 비어 있지 않은지만 검사한다.
  - `crates/quanta-index-repomap/src/store.rs:163-171`
- 이후 generation key의 존재만 확인하고 request digest와 snapshot digest를 비교하지 않는다.
  - `crates/quanta-index-repomap/src/store.rs:172-207`
- ack에는 repo/revision/generation만 있고 manifest/snapshot/authority digest가 없다.
  - `crates/quanta-index-contract/src/repomap.rs:2127-2145`

영향:

- caller가 잘못된 digest를 제출해도 같은 generation이 존재하면 activation이 성공한다.
- producer는 ack가 어떤 body와 snapshot을 승인했는지 증명할 수 없다.
- P0-1과 결합하면 generation number는 같지만 content가 다른 snapshot이 계속 성공 상태로 보인다.

필요 조치:

- candidate manifest/authority/snapshot identity를 stored sealed snapshot과 exact compare한다.
- expected-active CAS와 content-bound ack를 추가한다.

### P0-3. Finalized replay가 mutable preflight 뒤에 있음

판정: `FAIL_STATIC`

근거:

- 순서가 `digest verification -> route preflight -> catalog begin/replay`다.
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:86-136`
- search-corpus preflight는 현재 resource envelope와 delta base를 다시 검사한다.
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs:469-475`
- delta base preflight는 현재 ledger의 sealed identity와 physical base 존재를 요구한다.
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs:630-683`
- idempotency 계약은 finalized replay가 storage를 건드리지 않고 recorded result를 반환한다고
  설명한다.
  - `crates/quanta-index-core/src/domains/idempotency.rs:1-37`

도달 경로:

1. delta publish가 성공하고 receipt가 catalog에 finalize된다.
2. retention/GC가 base generation을 제거하거나 resource/config policy가 바뀐다.
3. producer가 동일 body를 retry한다.
4. catalog replay 조회 전에 preflight가 실패해 original receipt를 반환하지 못한다.

영향:

- retry 결과가 mutable current state에 따라 달라진다.
- ack loss/recovery의 핵심 idempotency 보장이 깨진다.

필요 조치:

- verified digest 뒤 read-only finalized lookup을 먼저 수행한다.
- finalized면 mutable preflight 없이 original receipt를 replay한다.
- miss일 때만 preflight와 atomic claim을 수행하고, race를 catalog에서 재확인한다.
- 단순히 `begin`을 preflight 앞으로 옮기면 fresh refusal이 in-progress intent를 남길 수 있으므로
  별도 inspect/claim semantics가 필요하다.

### P1-1. Public cursor가 full request에 결속되지 않음

판정: `FAIL_STATIC`

근거:

- `LexicalCursor`는 generation과 ordering boundary만 가진다.
  - `crates/quanta-index-contract-base/src/query/lexical_cursor.rs:77-92`
- 실제 generation pin은 `(repo_id, revision_id, manifest_generation)`이다.
  - `crates/quanta-index-contract-base/src/query/pin.rs:18-23`
- lexical route는 cursor의 numeric generation만 검증한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs:174-195`
- history/runtime/structural cursor도 full pin과 canonical query binding이 없다.
  - `crates/quanta-index-contract/src/query/history_cursor.rs:58-75`
  - `crates/quanta-index-contract/src/query/runtime_metadata_cursor.rs:35-47`
  - `crates/quanta-index-contract/src/query/structural_cursor.rs:37-46`

영향:

- query A의 cursor를 query B에 넣어 결과를 조용히 skip할 수 있다.
- 같은 numeric generation을 가진 다른 repo/revision에서 lexical cursor가 통과할 수 있다.
- 현재 cursor는 tamper-resistant continuation이 아니라 client-controlled seek boundary다.

필요 조치:

- cursor를 `route + full GenerationPin + canonical query/constraints + order + cap + aux epochs`에
  결속한다.
- 외부 입력이면 signed opaque token 또는 server-side cursor를 사용한다.
- editable seek boundary가 의도라면 tamper-resistant/동일 검색 continuation 주장을 제거한다.

### P1-2. Hybrid nested cap과 hybrid-seed identity/window 불일치

판정: `FAIL_STATIC`

근거:

- contract는 nested `text_query.top_k`도 gate 대상으로 설명한다.
  - `crates/quanta-index-contract/src/query/requests.rs:186-207`
- decoder는 typed refusal을 위해 out-of-range 값을 수용한다.
  - `crates/quanta-index-contract-base/src/query/requests.rs:20-33`
  - `crates/quanta-index-contract-base/src/query/requests.rs:159-184`
- hybrid와 hybrid-seed route는 outer top-k만 검증한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs:166-182`
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:35-64`
- 실제 seed fusion identity는 `(owner_kind, entity_id, corpus_kind)`다.
  - `crates/quanta-index-contract/src/results/query_responses.rs:561-594`
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:310-370`
- 하지만 `has_more` universe는 string `candidate_id/owner_id`만 집계한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:161-177`
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:207-212`

영향:

- raw IPC caller가 invalid nested cap과 valid outer cap을 함께 보내 성공시킬 수 있다.
- owner kind/corpus가 다르지만 ID string이 같은 결과가 한 개로 축약돼, 후속 결과가 있는데도
  `has_more=false`가 될 수 있다.

필요 조치:

- 중복 cap 필드를 제거하거나 nested range/equality를 명시적으로 검증한다.
- fusion, dedup, universe count, cursor window가 동일한 typed identity를 사용해야 한다.

### P1-3. Explain reconciliation과 provenance가 typed contract를 충족하지 않음

판정: `FAIL_STATIC`

근거:

- hybrid explain은 lexical/dense/fusion provenance를 재계산한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs:686-721`
- 불일치 여부는 trace/summary 문자열의 boolean으로만 기록된다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs:811-823`
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs:847-906`
- reconciliation이 false여도 성공 response를 반환하며 `SearchExplanation`에는 typed verdict가 없다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs:190-211`
  - `crates/quanta-index-contract/src/results/explanation.rs:550-571`
- live semantic/hybrid/hybrid-seed builder의 `ranker_weights_hash`는 `[0u8; 32]`다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:405-471`
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:815-948`
- explain route에는 실제 ranking digest 계산 함수가 따로 있다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs:456-489`

영향:

- consumer는 debug string을 파싱하지 않으면 mismatch를 알 수 없다.
- zero sentinel이 실제 provenance hash처럼 보인다.
- live response와 explain의 provenance 의미가 다르다.

필요 조치:

- typed reconciliation status 또는 typed refusal을 계약에 추가한다.
- 실제 digest를 기록하거나 `Option`/typed absence를 사용한다.
- zero/empty sentinel을 valid provenance로 허용하지 않는다.

### P1-4. Read와 mutate control operation의 권한이 분리되지 않음

판정: `FAIL_STATIC`

근거:

- 단일 control protocol에 status/metrics/inventory와 activate/rollback/quarantine discard가 섞여 있다.
  - `crates/quanta-index-contract/src/ipc/split.rs:127-150`
- socket ACL은 peer UID/GID만 판정한다.
  - `crates/quanta-index-ipc/src/socket_access.rs:63-113`
- dispatcher에는 principal/capability context가 전달되지 않는다.
  - `crates/quanta-index-search-plane/src/control_dispatcher.rs:197-265`
- runtime은 단일 control socket/policy를 구성한다.
  - `crates/quanta-index-searchd/src/app/runtime.rs:1452-1465`

영향:

- readiness/metrics/inventory 조회가 필요한 principal은 activate, rollback, discard 권한도 가진다.
- 로컬 UDS peer 인증은 fail-closed지만 least privilege는 충족하지 않는다.

필요 조치:

- read-only/admin socket을 분리하거나 operation별 capability ACL을 적용한다.
- negative authorization matrix를 product contract에 포함한다.

### P1-5. Daemon partial-alive, startup rollback, bounded drain 부재

판정: `FAIL_STATIC`

근거:

- query/control/ingest thread를 spawn한 뒤 external shutdown flag만 poll하며 thread exit를 감시하지 않는다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:20-38`
- IPC accept loop는 accept/spawn 오류로 반환할 수 있다.
  - `crates/quanta-index-ipc/src/server.rs:906-917`
- 2/3번째 plane spawn 실패 시 앞서 시작한 thread를 정리하지 않고 `?`로 반환한다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:28-34`
- listener bind 뒤 permission/credential/nonblocking 설정 실패는 `UdsServer` 생성 전 발생할 수 있다.
  - `crates/quanta-index-ipc/src/server.rs:738-803`
- connection handle은 완료 시 join하지 않고 제거하며 shutdown join 결과도 무시한다.
  - `crates/quanta-index-ipc/src/server.rs:856-860`
  - `crates/quanta-index-ipc/src/server.rs:920-923`
- panic이 `connection_closed()` 전에 발생하면 live counter가 감소하지 않을 수 있다.
  - `crates/quanta-index-ipc/src/server.rs:888-904`
  - `crates/quanta-index-ipc/src/counters.rs:127-151`
- control/ingest budget은 entry에서만 검사하고 durable operation 중에는 deadline을 무시한다.
  - `crates/quanta-index-search-plane/src/control_dispatcher.rs:197-214`
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:139-156`
- plane/connection join에는 timeout이 없다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:40-57`

영향:

- 한 plane만 죽고 process/lease/다른 sockets는 살아 있는 partial outage가 지속될 수 있다.
- startup error 뒤 detached thread 또는 stale socket이 남을 수 있다.
- admission budget 120초를 shutdown upper bound로 해석할 수 없다.
- 반복 handler panic이 live connection cap을 restart까지 잠식할 수 있다.

필요 조치:

- 모든 plane을 감독하고 unexpected exit 시 global shutdown/error와 readiness down을 수행한다.
- startup을 transactional하게 만들고 부분 spawn/bind를 rollback한다.
- cooperative request deadline과 hard process drain deadline을 분리한다.
- connection task join/panic/close reason과 counter reconciliation을 보장한다.

### P1-6. CI artifact authority가 absence/stale/wrong-type를 green으로 만들 수 있음

판정: `FAIL_STATIC`

근거:

- artifact checker는 artifact 부재를 기본 PASS로 취급한다.
  - `tools/ci/lint/check-bench-artifacts.py:25-28`
  - `tools/ci/lint/check-bench-artifacts.py:223-252`
- Just/CI 호출에는 blocking `--require`가 없다.
  - `Justfile:523-527`
  - `.github/workflows/ci.yml:235-240`
- quality aggregator는 schema 2의 `detail.passed` 외에 임의 schema의 top-level `passed`도 허용한다.
  - `tools/benchmark/quality_integration_summary.py:37-62`
- `bool(value)`를 사용해 string `"false"`도 true가 된다.
  - `tools/benchmark/quality_integration_summary.py:55-58`
- exact HEAD/source digest를 검증하지 않는다.
- freshness 대상에 ambiguity/snippet/ops/ui/integration이 빠져 있다.
  - `tools/ci/lint/check-bench-artifacts.py:50-61`
- snippet의 golden window는 fixture 원문이 아니라 이미 출력된 snippet에 oracle을 다시 적용한다.
  - `crates/quanta-index-searchd-harness/src/snippet.rs:560-581`

영향:

- fresh perf/quality artifact가 0개여도 clean CI가 green일 수 있다.
- stale 또는 다른 source의 summary가 current quality proof처럼 집계될 수 있다.
- 잘못 crop됐지만 bounded인 snippet이 self-oracle을 통과할 수 있다.

필요 조치:

- blocking workflow는 mandatory family를 `--require`하고 exact clean HEAD/source digest를 검증한다.
- schema와 boolean type을 strict하게 검사한다.
- mandatory dimension 전체에 corpus/config/model/host provenance를 요구한다.
- golden oracle은 SUT output이 아니라 fixture source와 original match anchor에서 계산한다.

### P1-7. Persisted state recovery/upgrade authority가 실행 가능하지 않음

판정: `FAIL_STATIC`

근거:

- inventory는 activation catalog, rollback history, SQLite catalog를 offline-importer 대상으로 분류한다.
  - `tools/ci/inventory/wire-surface.toml:308-331`
- catalog는 WAL을 사용한다.
  - `crates/quanta-index-catalog/src/connection.rs:89-135`
- executable importer/export/restore, backup consistency boundary, checkpoint/restore drill을 확인하지 못했다.
- wire guard는 opcode/version 상수를 중심으로 감시하며 persisted structural schema drift를 충분히
  식별하지 않는다.

영향:

- DB 파일만 복사하거나 producer artifact만 재생해서 activation choice, rollback history,
  durable sequence, runtime overlay를 동일하게 복원할 수 없다.
- valid old state, unsupported state, corruption을 구분하지 못한다.

필요 조치:

- 모든 offline-importer row에 실행 가능한 tool과 frozen fixture를 연결한다.
- WAL, activation/history, catalog를 같은 consistency boundary로 backup/restore한다.
- persisted shape마다 explicit format version/fingerprint/migration gate를 둔다.

## 4. 1차 보고서 교정

### 4.1 Receipt schema 변경

확인된 사실:

- 새 receipt field는 required decode다.
- current DTO가 raw CBOR로 catalog에 저장되고 current type으로 직접 replay decode된다.
- catalog table/version은 unchanged `v1`이다.

교정된 판정:

- rolling skew나 기존 state root를 지원해야 한다면 upgrade blocker다.
- breaking-first, lock-step, 새 state root cutover가 명시적 정책이고 실제 legacy state가 없다면
  strict decode 자체는 제품 결함이 아니다.
- 현재 실제 결함은 **배포 inventory/cutover receipt 없이 valid-old와 corrupt를 구분하지 못하는 것**이다.
- 따라서 1차의 무조건 P0 표현을 `DECISION_REQUIRED/BLOCKED`로 낮추고, legacy state 또는 skew가
  확인될 때 P0 release blocker로 승격한다.

### 4.2 G4 판정 범위

- search-corpus lexical/semantic generation lifecycle의 CAS/rollback/snapshot 구조는
  `PASS_STATIC + RUNTIME_UNVERIFIED`를 유지한다.
- RepoMap은 P0-1/P0-2 때문에 `FAIL_STATIC`이다.
- 1차의 전체 G4 `PASS_STATIC`은 철회한다.

### 4.3 Quality-all 설명

- `quality-all` recipe 자체는 seven rail을 다시 실행한다. “자체 output이 stale by construction”은
  근거가 부족하다.
- 실제 결함은 mandatory dimension 누락, schema/type/freshness/source binding 부족,
  release workflow의 absence-pass다.
- external comparison은 경쟁 우위 주장을 release requirement로 삼을 때만 mandatory다.

### 4.4 동시 same-body publish race

- catalog protocol은 applied=false row를 `Resume`으로 반환하고 Fresh/Resume 모두 apply/finalize해
  direct reuse나 admission policy 완화 시 중복 apply/finalize race가 가능하다.
- 현재 production ingest는 `SERIAL_DISPATCH`와 state-root lease로 reachability가 차단된다.
- 현행 P0가 아니라 latent design risk다. serial-ingest invariant를 machine guard로 고정하거나
  catalog owner/lease/wait semantics를 추가해야 한다.

### 4.5 보안/config 오탐 제거

- UDS ownership/mode, stale inode, peer credential verification은 fail-closed다.
- production profile에서 무설정 hash embedder로 조용히 fallback한다는 근거는 없다.
- bounded aggregate metric/cardinality guard도 존재한다.
- 잔여 결함은 operation-level authorization, local UID accept flood, connection outcome/panic,
  exporter/production observation 범위다.

## 5. 체크리스트 구조 자체의 보완

기존 checklist에 다음 판정 규칙을 먼저 적용해야 한다.

- 정적 감사에서는 runtime-only item을 `FAIL` 분모에 넣지 않고 `RUNTIME_UNVERIFIED`로 분리한다.
- `G3-06`의 “SQLite transaction이 전체 batch를 적용”은 native lexical/semantic store가 SQLite
  transaction 밖이므로 잘못된 oracle이다. Catalog atomicity와 cross-store crash-convergent saga를 분리한다.
- public path에서 batch digest는 canonical body hash이고 key 일부이므로 same-key/different-body는
  adapter invariant다. Product case는 carried digest mismatch와 같은 natural operation/different digest다.
- pagination은 surface가 bounded single response인지에 따라 applicability를 먼저 판정한다.
- compaction/remap, external comparison, exporter는 제품 profile/claim에 따라 N/A를 허용하되
  근거와 owner를 기록한다.
- unknown field는 semantic payload fail-closed와 envelope extension compatibility policy를 분리한다.

추가할 항목:

| ID | 우선순위 | 추가 검증 항목 |
|---|---:|---|
| G2-11 | P1 | 모든 mutating SDK method가 variant뿐 아니라 request identity, digest, counts, durable sequence, forbidden field를 검증한다. |
| G3-13 | P0 | Finalized replay는 base GC, policy/config 변경, provider 불가 뒤에도 original receipt를 반환한다. |
| G3-14 | P1 | Serial-ingest invariant를 machine-enforce하거나 in-progress row owner/lease/wait를 둔다. |
| G3-15 | P1 | Open/import 후 durable sequence가 positive이고 stored max보다 큰지 검증한다. |
| G4-15 | P0 | 모든 generation owner는 동일 logical generation의 different content overwrite를 거부한다. |
| G4-16 | P0 | Activation은 candidate digest와 stored sealed snapshot을 exact compare하고 ack에 identity를 반환한다. |
| G5-18 | P1 | Cursor를 route/full pin/canonical request/order/cap/aux epoch에 결속하고 cross-query/repo reuse를 거부한다. |
| G5-19 | P1 | 중첩·중복된 모든 cap field를 raw wire에서 range/equality 검증한다. |
| G5-20 | P1 | Fusion, dedup, universe count, window가 동일한 typed identity를 사용한다. |
| G5-21 | P1 | Explain reconciliation mismatch는 typed status 또는 typed refusal이며 debug string parsing이 필요 없다. |
| G6-10 | P1 | Provenance digest의 zero/empty sentinel을 금지하고 typed absence를 사용한다. |
| G6-11 | P1 | Quality artifact는 strict schema/type, exact HEAD, dirty state, corpus/config/model/host digest에 결속된다. |
| G6-12 | P1 | Golden oracle의 입력과 계산 경로는 SUT output과 독립적이다. |
| G7-10 | P1 | Control operation별 principal/capability 권한표와 negative authorization을 검증한다. |
| G8-11 | P1 | Unexpected plane exit가 global shutdown/error와 all-plane readiness down으로 전파된다. |
| G8-12 | P1 | Startup 부분 실패가 thread, listener, socket inode, lease를 rollback한다. |
| G9-11 | P1 | Cooperative deadline과 hard drain deadline/escalation을 별도 검증한다. |
| G9-12 | P1 | Worker panic/close reason을 join·관측하고 live counter를 반드시 reconciliation한다. |
| G11-10 | P1 | Blocking workflow는 mandatory artifact absence를 실패시키고 exact-source artifact를 publish한다. |
| G12-10 | P0 | Daemon binary SHA/source HEAD와 producer가 link한 contract/SDK tree digest를 하나의 receipt에 묶는다. |
| G12-11 | P1 | Path dependency resolved root와 dirty state를 기록하고 source mismatch를 거부한다. |
| G12-12 | P1 | Provider-declared model과 provider-observed immutable model/version 및 usage/cost를 구분한다. |
| G12-13 | P1 | Persisted state마다 executable backup/restore/importer와 frozen migration fixture가 있다. |

## 6. 수정 우선순위

1. RepoMap immutable generation, digest-bound activation, content-bound ack를 먼저 고친다.
2. finalized replay lookup을 mutable preflight 앞의 별도 non-mutating authority로 분리한다.
3. full-context cursor와 hybrid identity/cap/reconciliation contract를 고친다.
4. daemon plane supervisor, startup rollback, hard drain deadline, panic reconciliation을 구현한다.
5. read/admin control 권한을 분리한다.
6. artifact gate를 strict source-bound/absence-fail로 만들고 independent oracle을 세운다.
7. receipt/persisted state cutover 정책과 executable importer/restore를 확정한다.
8. source를 clean exact pair로 동결한 뒤에만 실행 검증 단계로 넘어간다.

## 7. 이번 감사가 증명하지 않은 것

- 테스트 통과 여부
- compile 가능 여부
- daemon 실제 signal/drain 동작
- crash/restart 수렴성
- provider timeout/cancellation의 실제 잔여량
- 품질/성능 threshold 충족 여부
- clean quanta-index/Semantica exact-pair compatibility

이 항목들은 정적 결함 수정 후 별도 실행 증거가 필요하다.
