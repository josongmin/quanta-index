# quanta-index 목적 적합성 3차 정적 감사

- 감사일: 2026-09-21
- 방식: 소스, 계약, 문서, CI recipe, 기존 artifact만 정적으로 검토
- 실행하지 않은 것: 테스트, 빌드, lint, benchmark, daemon, provider 호출
- 기준: `docs/analysis/quanta-index-purpose-validation-checklist.md`
- 이전 보고서:
  - `docs/analysis/quanta-index-purpose-static-audit-2026-09-21.md`
  - `docs/analysis/quanta-index-purpose-static-audit-2026-09-21-pass2.md`
- quanta-index HEAD: `3ad279a08879de35fa96a5495a3382af28f095d0`
- branch/upstream: `main` / `4914156f4191daa3e12998bdb38f2b821a057fdd`
- upstream 대비: ahead 145, behind 0
- 3차 시작 tracked diff digest: `9183ae4b869e4b0cb12a8ef36fa869c1a7bcf5a34fb85d1be7ee7ad82a0f0b6c`
- 보고서 초안 직전 tracked diff digest: `675e9b2a10aacaef08ce3ee08446f57fff8c149743ae821539b63f455f16a61e`
- 보고서 artifact 생성 직후 tracked diff digest: `ed82014573bead8ecacf9bbf34b34039cf63a1409c26205556a70895114a8bfc`
- 기준 체크리스트 digest: `af7d1d9ff271d819512d65a956aa7ff58a0e2ac4031a794f64ac638f1d4bf215`
- 1차 보고서 digest: `dd573b384e005e44c865fe0cf0792bfa5d7fbdd0b23add13c52817604379fcf0`
- 2차 보고서 digest: `41d6e12d43e1693a0f90c91265ff8d0bfc5c35a5be986eda1e84aaa92996713c`

## 1. 결론

**3차 정적 종합 판정: `PURPOSE_RED`**

테스트를 실행하지 않았으므로 `U/A/D/P/F/Q/X` 증거가 필요한 항목은 `PASS`로 판정하지
않는다. 다만 `S`가 필수 증거인 구조·계약 체크에서 아래 P0/P1 위반이 확인됐으므로
`PURPOSE_INCOMPLETE`가 아니라 `PURPOSE_RED`다. 실행 검증 미수행은 이 정적 실패를 상쇄하지
않는다.

3차에서 새로 확인한 최상위 문제는 다음과 같다.

1. RepoMap의 persisted filename encoding이 identity tuple에 대해 injective하지 않아 서로 다른
   repo/revision이 같은 파일을 덮어쓸 수 있다.
2. activation pointer가 version, digest, candidate content에 결속되지 않으며, 손상으로 무효화된
   stale pointer가 repair publish 뒤 다음 restart에서 명시적 activate 없이 부활한다.
3. RepoMap read view는 실제 RepoMap snapshot handle을 pin하지 않아 activation/retirement와 query
   사이 TOCTOU가 있다.
4. filtered dense admission이 `Capped`여도 hybrid window가 `Exact`/`has_more=false`를 반환할 수
   있다.
5. auxiliary invalid publish가 durable in-progress intent를 남기며, public SDK가 여러 query 및
   mutation response를 원 요청/identity에 결속하지 않는다.
6. daemon은 signal wiring, plane supervision, startup rollback, hard drain deadline, panic-safe
   accounting이 없다.

2차의 P0인 active-generation overwrite, digest-unbound activation, finalized replay-before-preflight
문제도 독립적으로 재확인됐다.

## 2. 판정 방법 정정

기준 체크리스트는 상태를 `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN`, `N/A`로만 제한하고
`코드상 PASS`를 금지한다. 따라서 이전 보고서의 `PASS_STATIC`/`FAIL_STATIC` 표기는 정식 상태로
재사용하지 않는다.

- source/API/dependency shape처럼 필수 증거가 `S`인 행은 정적 증거로 `PASS` 또는 `FAIL` 가능
- 필수 증거가 `U/A/D/P/F/Q/X`인 행은 이번 패스에서 `NOT_RUN` 또는 외부 조건에 따른
  `BLOCKED`
- 정적으로 도달 가능한 반례가 `S` 행의 불변식을 직접 위반하면 `FAIL`
- runtime-only 주장은 실행 없이 `PASS`로 승격하지 않음

또한 2차 보고서 440-466행의 20개 항목은 **체크리스트 반영분이 아니라 제안 목록**이다.
체크리스트 digest가 2차 이후 그대로이며 실제 파일은 예를 들어 `G5-17`, `G6-09`, `G7-09`에서
끝난다. 이전에 이를 “체크리스트에 추가 완료”로 표현했다면 현재 파일 기준으로 잘못이다.

## 3. source snapshot과 drift

- HEAD는 감사 동안 유지됐지만 tracked diff digest가 `9183ae4b...`에서 `675e9b2a...`, 이후
  `ed820145...`로 계속 변했다.
- dirty path에는 contract/SDK/search-plane 일부와 README, CI/test authority 문서가 포함됐다.
- 아래 신규 P0의 owner인 RepoMap persistence/store는 관찰한 dirty path 밖이었다.
- query/runtime finding 대부분의 owner source도 dirty path 밖이지만 README와 CI 문서 불일치 항목은
  종료 snapshot에만 결속한다.
- 따라서 전체 worktree exact freeze를 요구하는 G0/G12 closure는 `BLOCKED` 또는 `NOT_RUN`이며,
  이 보고서는 clean release receipt가 아니다.

## 4. 신규 P0

### P0-1. RepoMap persisted filename key가 비단사

판정: `FAIL` / 증거: `S`

근거:

- snapshot/activation 파일명은 `encode(repo)--encode(revision)`을 사용한다.
  - `crates/quanta-index-repomap/src/persistence.rs:740-758`
- `encode_component`는 `-`를 escape하지 않는다.
  - `crates/quanta-index-repomap/src/persistence.rs:761-772`
- `RepoId`와 `RevisionId`는 임의 문자열을 보유하며 separator 금지 검증이 없다.
  - `crates/quanta-index-contract-base/src/macros.rs:16-35`
  - `crates/quanta-index-contract-base/src/ids.rs:9-10`
- atomic rename은 동일 final path를 교체한다.
  - `crates/quanta-index-repomap/src/persistence.rs:310-354`

정적 반례:

- `(repo="a--b", revision="c")`
- `(repo="a", revision="b--c")`

두 tuple은 동일한 `a--b--c.json` 및 동일 generation snapshot filename으로 수렴한다. 서로 다른
repository authority가 디스크에서 overwrite되고, restart 후 한쪽 snapshot/activation이 유실되거나
다른 identity에 귀속될 수 있다. 한 tuple의 remove/quarantine도 다른 tuple의 파일에 영향을 줄 수
있다.

필요 조치:

- 전체 typed tuple을 canonical CBOR/length-prefix/hash 기반의 injective key로 저장한다.
- separator, `%`, Unicode normalization, 빈/공백, 장경로 조합의 property test를 둔다.
- 기존 파일명을 새 schema로 import할 때 collision을 탐지하고 fail closed한다.

### P0-2. Activation pointer에 version/digest/content binding이 없음

판정: `FAIL` / 증거: `S`

근거:

- activation record는 repo/revision/generation만 가진다.
  - `crates/quanta-index-repomap/src/persistence.rs:77-96`
- activation load는 JSON shape와 filename identity만 검사한다.
  - `crates/quanta-index-repomap/src/persistence.rs:468-482`
- activation은 plain JSON으로 durable write된다.
  - `crates/quanta-index-repomap/src/persistence.rs:669-687`
- snapshot file과 달리 format version과 SHA-256 envelope가 없다.
  - `crates/quanta-index-repomap/src/persistence.rs:177-183`
  - `crates/quanta-index-repomap/src/persistence.rs:420-446`
- open은 pointer가 지목하는 generation snapshot이 존재하면 그대로 active map에 넣는다.
  - `crates/quanta-index-repomap/src/store.rs:87-105`

유효 JSON 형태의 bitrot/tamper가 generation을 다른 resident snapshot으로 바꾸면 restart에서 silent
activation된다. record 자체 무결성과 exact candidate manifest/content binding이 모두 없다.

필요 조치:

- versioned, self-digested activation envelope
- full repo/revision/generation + manifest/authority/snapshot/content digest 결속
- expected-active CAS와 content-bound activation receipt
- pointer와 candidate의 atomic commit 또는 restart reconciliation

### P0-3. 무효화된 stale activation이 repair publish 후 restart에서 부활

판정: `FAIL` / 증거: `S`

근거:

- corrupt snapshot은 quarantine하지만 syntactically valid activation은 계속 load한다.
  - `crates/quanta-index-repomap/src/persistence.rs:373-408`
- snapshot 없는 activation은 report와 memory에서만 제외하며 durable file을 제거/tombstone하지 않는다.
  - `crates/quanta-index-repomap/src/store.rs:67-105`
- 이후 같은 tuple/generation publish는 snapshot만 다시 저장한다.
  - `crates/quanta-index-repomap/src/store.rs:117-154`
- 다음 open은 남아 있던 activation과 새 snapshot을 매치해 active map에 삽입한다.
  - `crates/quanta-index-repomap/src/store.rs:87-105`
- core contract 설명은 명시적으로 다시 activate될 때까지 NOT_FOUND를 약속한다.
  - `crates/quanta-index-core/src/domains/repomap/outbound.rs:50-52`
- 기존 durability test는 첫 reopen의 NOT_FOUND까지만 확인한다.
  - `crates/quanta-index-repomap/tests/store_durability.rs:161-204`

도달 경로:

1. active snapshot 손상
2. restart에서 snapshot quarantine, activation은 파일로 잔존
3. 같은 generation snapshot publish만 수행
4. 그 시점에는 inactive
5. 다시 restart
6. explicit activate 없이 자동 active

P0-1/P0-2 및 기존 same-generation overwrite와 결합하면 원래 승인하지 않은 다른 content도 active
truth가 될 수 있다.

필요 조치:

- candidate 상실 시 active pointer를 durable tombstone/quarantine
- repair publish만으로 pointer 복원 금지
- 새 explicit activation은 exact content proof/CAS를 요구
- 두 번의 restart를 포함한 corruption-repair fixture 추가

## 5. 신규 P1 — 저장·계약

### P1-1. RepoMap graph materialization이 malformed graph를 조용히 변환

판정: `FAIL` / 증거: `S`

근거:

- bundle/store gate는 graph node uniqueness와 edge referential integrity를 강제하지 않는다.
  - `crates/quanta-index-repomap/src/store.rs:117-134`
- duplicate chunk ID는 `BTreeMap::collect`의 last-write-wins로 축약된다.
  - `crates/quanta-index-repomap/src/materializer.rs:181-189`
- 존재하지 않는 owns-chunk target은 error가 아니라 `continue`다.
  - `crates/quanta-index-repomap/src/materializer.rs:473-482`
- snapshot index의 duplicate `(subject_identity, doc_type)`도 이전 위치를 덮어쓴다.
  - `crates/quanta-index-repomap/src/model.rs:442-465`
- graph stats key는 File/Module/Symbol/Chunk discriminant를 제거한 raw string이다.
  - `crates/quanta-index-repomap/src/materializer.rs:424-453`
  - `crates/quanta-index-repomap/src/materializer.rs:503-509`

결과:

- `File("x")`와 `Symbol("x")`의 call/import signal이 합쳐질 수 있다.
- duplicate와 dangling edge가 rejection 없이 ranking feature로 재해석된다.
- producer graph truth를 search plane이 silent repair한다는 점에서 G1/G3 owner boundary를 위반한다.

필요 조치: typed node identity uniqueness, edge endpoint 존재/variant compatibility, duplicate policy를
materialization 전에 검증하고 typed refusal한다.

### P1-2. RepoMap materialization에 입력 대비 증폭 상한이 없음

판정: `FAIL` / 증거: `S`

- 같은 owner path의 symbol list를 만들면서 symbol을 clone한다.
  - `crates/quanta-index-repomap/src/materializer.rs:168-177`
- 각 file entry마다 해당 list를 다시 clone하고 전체 symbol name/preview를 search text에 join한다.
  - `crates/quanta-index-repomap/src/materializer.rs:64-88`
  - `crates/quanta-index-repomap/src/materializer.rs:234-249`
- wire cap은 16 MiB이지만 node/edge/cardinality 및 materialized output complexity cap은 없다.
  - `crates/quanta-index-ipc/src/codec.rs:17-29`
  - `crates/quanta-index-ipc/src/codec.rs:215-255`

동일 owner path의 file/symbol을 반복한 bounded frame이 materialization memory/CPU/disk를 초선형으로
증폭시킬 수 있다. serial ingest plane에서 availability/DoS 위험이다.

### P1-3. Auxiliary semantic refusal가 durable in-progress intent를 남김

판정: `FAIL` / 증거: `S`

- catalog begin/replay가 apply 전에 실행된다.
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:119-135`
- auxiliary route preflight는 무조건 성공한다.
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:168-245`
  - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:264-276`
- 실제 semantic refusal는 apply 단계에 있다.
  - history parent/ref: `crates/quanta-index-search-plane/src/auxiliary_authority.rs:197-272`
  - runtime chunk universe/order: `crates/quanta-index-search-plane/src/auxiliary_authority.rs:317-332`
  - structural tree: `crates/quanta-index-search-plane/src/auxiliary_authority.rs:424-451`

영구-invalid 요청이 durable in-progress row를 남기고 retry마다 Resume/apply/refuse를 반복한다.
generation이 오래 유지되면 bounded reclaim이 보장되지 않아 state-root growth 경로가 된다.

필요 조치: complete semantic preflight를 catalog claim 전에 수행하거나, terminal-refused 상태와 bounded
retention/repair를 durable protocol로 정의한다.

### P1-4. Public SDK response가 원 요청과 authority identity에 결속되지 않음

판정: `FAIL` / 증거: `S`

- generic dispatch는 request ID와 response error variant만 확인하고 request context를 버린다.
  - `crates/quanta-index-sdk/src/client.rs:134-160`
- query SDK는 대체로 matching variant만 확인한다.
  - lexical: `crates/quanta-index-sdk/src/lexical.rs:833-858`
  - semantic: `crates/quanta-index-sdk/src/semantic.rs:212-237`
  - hybrid: `crates/quanta-index-sdk/src/search.rs:531-556`
  - runtime: `crates/quanta-index-sdk/src/runtime.rs:271-292`
  - structural: `crates/quanta-index-sdk/src/structural.rs:375-396`
- mutating auxiliary/RepoMap SDK도 route별 receipt binding이 빠져 있다.
  - `crates/quanta-index-sdk/src/history.rs:581-610`
  - `crates/quanta-index-sdk/src/history.rs:719-785`
  - `crates/quanta-index-sdk/src/runtime.rs:120-146`
  - `crates/quanta-index-sdk/src/structural.rs:237-261`
  - `crates/quanta-index-sdk/src/repomap.rs:50-119`
- cluster membership route는 exact request binding을 하므로 공통 구현 불가능성은 아니다.
  - `crates/quanta-index-sdk/src/search.rs:72-102`

정상 daemon이 현재 오배달한다는 증거는 아니다. 확인된 결함은 correct request ID와 variant를 가진
wrong pin/candidate/projection/receipt를 client trust boundary가 수용할 수 있다는 것이다.

### P1-5. Durable sequence authority의 open-time reconciliation 부재

판정: `FAIL` / 증거: `S`

- sequence row에 `next >= 1`, digest, stored sequence uniqueness 제약이 없다.
  - `crates/quanta-index-catalog/src/idempotency.rs:40-75`
- allocator는 현재 next를 그대로 발행한다.
  - `crates/quanta-index-catalog/src/idempotency.rs:304-320`
- open은 `next > MAX(durable_sequence)`를 검증/복구하지 않는다.
  - `crates/quanta-index-catalog/src/open.rs:12-29`

restore/manipulation/corruption으로 next가 회귀하면 duplicate/non-monotonic public receipt sequence가
발행될 수 있다.

## 6. 신규 P1 — query/product semantics

### P1-6. RepoMap read view가 실제 snapshot을 pin하지 않음

판정: `FAIL` / 증거: `S`

- read-view 문서는 declared domain의 immutable handle을 한 번 acquire한다고 주장한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs:1-30`
- `QueryReadViewV1`에는 RepoMap handle이 없다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs:150-163`
- acquire는 lexical/semantic handle만 별도로 취득한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs:342-386`
- RepoMap route는 ceremonial domain view를 만든 뒤 독립 port call을 한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/repo_map.rs:15-39`
- store는 active generation lock을 놓은 후 별도 snapshot lock을 획득한다.
  - `crates/quanta-index-repomap/src/store.rs:259-285`
  - `crates/quanta-index-repomap/src/store.rs:323-348`
- activation은 active generation을 바꾼 뒤 이전 snapshot을 retire한다.
  - `crates/quanta-index-repomap/src/store.rs:163-255`

query가 generation N의 active check를 통과한 뒤 concurrent activation N+1이 N을 retire하면, 원래
pin을 획득했다고 기록한 query가 `NotFound`로 바뀔 수 있다. memory safety 문제는 아니며,
`Arc`를 실제로 먼저 얻으면 해결 가능한 authority TOCTOU다.

### P1-7. Filtered dense `Capped`가 exact/exhausted로 오표시될 수 있음

판정: `FAIL` / 증거: `S`

- `Capped`는 target을 채우기 전에 examine ceiling에 도달했음을 뜻한다.
  - `crates/quanta-index-core/src/domains/hybrid/dense_admission.rs:285-298`
- dense admission은 partial rows와 `Capped`를 반환한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/dense_admission.rs:108-125`
- hybrid/hybrid-seed는 outcome을 window completeness 계산에 반영하지 않는다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs:112-157`
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:122-158`
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:207-212`
- window는 observed universe가 작으면 `Exact(returned)`, `has_more=false`를 만든다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/window.rs:110-130`

exact filter가 상위 dense 후보를 많이 탈락시키고 refill ceiling에 닿으면, 아직 검사하지 않은 하위
후보가 있어도 exhausted pagination을 반환할 수 있다. 기존 `G5-16`의 직접 위반이다.

### P1-8. hybrid-seed contribution rank와 실제 RRF rank 불일치

판정: `FAIL` / 증거: `S`

- typed identity로 dedup하면서 contribution rank는 raw `enumerate()+1`을 기록한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:212-282`
- RRF는 dedup 이후 compact unique 순번으로 score를 계산한다.
  - `crates/quanta-index-core/src/domains/hybrid/service.rs:238-255`
  - `crates/quanta-index-core/src/domains/hybrid/service.rs:293-308`
- final seed rank만 교정하고 contribution rank는 유지한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:346-368`

예를 들어 raw `[A1, A2, B]`가 typed identity로 `[A, B]`가 되면 provenance는 B의 lane rank를 3으로
기록하지만 RRF는 rank 2로 계산한다. explain/audit에서 score를 재현할 수 없다.

### P1-9. Unicode RepoMap query가 빈/global query로 변환됨

판정: `FAIL` / 증거: `S`

- policy는 trim non-empty만 검사한다.
  - `crates/quanta-index-core/src/domains/repomap/policy.rs:15-37`
- tokenizer는 ASCII alphanumeric만 term으로 인정한다.
  - `crates/quanta-index-repomap/src/query.rs:294-310`
- index도 ASCII lowercase만 사용한다.
  - `crates/quanta-index-repomap/src/model.rs:450-458`
- term이 비면 query engine은 match 실패로 거부하지 않고 전체 candidate universe를 rank한다.
  - `crates/quanta-index-repomap/src/query.rs:59-111`

한국어/일본어 등 non-ASCII-only query가 입력상 valid인데 tokenless global ranking으로 바뀐다.
공통 Unicode tokenizer를 사용하거나 normalized tokenless query를 typed refusal해야 한다.

### P1-10. Semantic model gate와 입력 gate가 provider I/O 뒤에 있음

판정: `FAIL` / 증거: `S`

- semantic route는 embedding provider 호출 후 model identity mismatch를 검사한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs:37-55`
- gate는 vector 없이 provider model metadata만으로 수행 가능하다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:474-505`
- hybrid는 lexical lane까지 실행한 후 같은 gate를 통과한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs:69-110`
- hash embedder는 alphanumeric token 없는 query를 local refusal하지만 production adapter는 raw
  query를 provider로 보낸다.
  - `crates/quanta-index-search-plane/src/query_embedder.rs:111-151`
  - `crates/quanta-index-searchd/src/app/runtime.rs:473-493`
  - `crates/quanta-index-embed/src/openai.rs:577-600`

retained generation/model mismatch는 매번 billable external call 뒤 deterministic refusal한다. 빈 문자열
또는 punctuation-only query의 결과도 profile마다 local refusal/외부 요청으로 달라진다.

### P1-11. 실행 엔진, 기여 lane, corpus availability를 혼동

판정: `FAIL` / 증거: `S`

- zero-hit semantic/hybrid는 실제 backend를 실행해도 hit가 없으면 `engines_touched`에 넣지 않는다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:420-431`
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:844-854`
  - `crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs:900-916`
- fanout metric은 이 배열 길이를 사용한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs:313-382`
- hybrid-seed는 `examined == 0`을 corpus unavailable로 표시한다.
  - `crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs:147-154`

zero-hit 정상 실행이 fanout 0으로 계측되고, available-empty/filtered-empty가 unavailable로 오분류될
수 있다. `engines_executed`, `lanes_contributed`, sealed corpus availability를 분리해야 한다.

## 7. 신규/재확인 P1 — runtime, operations, evidence

### P1-12. Release daemon에 SIGINT/SIGTERM wiring이 없음

판정: `FAIL` / 증거: `S`

- runtime은 스스로 변경하지 않는 `AtomicBool(false)`를 만든다.
  - `crates/quanta-index-searchd-runtime/src/lib.rs:269-284`
- release binary main에 signal wiring이 없다.
  - `crates/quanta-index-searchd-runtime/src/bin/quanta-index-searchd.rs:1-8`
- searchd는 외부 flag만 polling한다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:20-38`
- 종료 helper도 signal이 아니라 kill/wait다.
  - `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:45-59`
  - `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:101-114`

종료 snapshot의 README drain 주장은 구현과 불일치한다. 이 항목은 이전 패스 결론을 재확인한다.

### P1-13. Plane supervisor와 startup rollback이 없음

판정: `FAIL` / 증거: `S`

- query/control/ingest를 순차 spawn하고 외부 shutdown만 기다린다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:28-38`
- plane result는 shutdown 뒤에만 join/관측한다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:40-57`
- accept/spawn error는 plane thread를 종료할 수 있다.
  - `crates/quanta-index-ipc/src/server.rs:886-917`
- 뒤 plane spawn이 실패해도 이미 시작한 plane을 shutdown/join하지 않고 `?`로 반환한다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:31-34`
  - `crates/quanta-index-searchd/src/app/runtime.rs:1011-1043`

single-plane partial outage가 process 전체 실패로 전파되지 않고, startup partial failure에는 lease를
잃은 orphan plane 가능성이 있다.

### P1-14. Shutdown에 hard deadline이 없음

판정: `FAIL` / 증거: `S`

- control mutation은 budget을 entry에서만 확인한다.
  - `crates/quanta-index-search-plane/src/control_dispatcher.rs:197-214`
- connection 및 plane handle을 timeout 없이 join한다.
  - `crates/quanta-index-ipc/src/server.rs:920-923`
  - `crates/quanta-index-searchd/src/app/searchd.rs:43-57`
- maintenance disk walk/scrub은 cancellation/deadline이 없고 Drop도 무기한 join한다.
  - `crates/quanta-index-searchd/src/app/maintenance.rs:85-130`
  - `crates/quanta-index-searchd/src/app/maintenance.rs:146-189`
- maintenance cadence는 0만 거부하고 상한이 없다.
  - `crates/quanta-index-searchd/src/app/config.rs:241-266`
  - `crates/quanta-index-searchd/src/app/config.rs:1197-1209`

signal handler를 추가하는 것만으로 bounded drain 주장은 성립하지 않는다.

### P1-15. Panic-safe connection/peer-watch 회수가 아님

판정: `FAIL` / 증거: `S`

- 종료 connection handle을 join하지 않고 제거해 panic reason을 폐기한다.
  - `crates/quanta-index-ipc/src/server.rs:856-860`
- live counter 감소와 peer-watch disarm은 정상 tail에만 있다.
  - `crates/quanta-index-ipc/src/server.rs:880-904`
  - `crates/quanta-index-ipc/src/server.rs:1135-1150`
- `PeerWatch`는 explicit disarm만 stop/join하며 Drop이 없다.
  - `crates/quanta-index-ipc/src/server.rs:1219-1283`
- 누적 live counter는 connection cap admission을 잠식한다.
  - `crates/quanta-index-ipc/src/counters.rs:127-151`

외부 입력으로 panic을 직접 유발하는 경로는 이번 감사에서 확정하지 않았다. 확정된 것은 내부 panic
하나가 accounting/thread/FD reconciliation을 건너뛸 수 있는 구조다.

### P1-16. OpenAI cancellation 이후 process-global residual cap이 없음

판정: `FAIL` / 증거: `S`

- cancel된 attempt는 caller 반환 뒤 자체 thread에서 timeout까지 계속됨을 명시한다.
  - `crates/quanta-index-embed/src/openai.rs:329-336`
- 요청마다 OS thread를 만들고 JoinHandle을 보존하지 않는다.
  - `crates/quanta-index-embed/src/openai.rs:401-447`
- worker 제한은 request 내부에만 적용된다.
  - `crates/quanta-index-embed/src/openai.rs:450-520`
- HTTP timeout도 개별 attempt 단위다.
  - `crates/quanta-index-embed/src/openai.rs:622-682`

반복 cancellation 시 detached provider thread/request/FD/cost가 process-global로 중첩될 수 있다.

### P1-17. Shared control socket에 operation-level authorization이 없음

판정: `FAIL` / 증거: `S`

- 하나의 control enum에 metrics/status와 activate/rollback/discard가 섞여 있다.
  - `crates/quanta-index-contract/src/ipc/split.rs:127-150`
- dispatcher는 principal/capability 없이 모든 operation을 실행한다.
  - `crates/quanta-index-search-plane/src/control_dispatcher.rs:197-265`
- composition root는 control socket 전체에 하나의 access policy를 적용한다.
  - `crates/quanta-index-searchd/src/app/runtime.rs:1452-1465`

UDS 인증 부재라는 주장은 오탐이다. private default와 peer credential screening은 존재한다. 문제는
shared UID/GID에 read-only scrape 권한과 mutation 권한을 분리할 수 없다는 점이다.

### P1-18. Partial-alive readiness와 request-correlated diagnostics가 없음

판정: `FAIL` / 증거: `S`

- request ID는 dispatcher에 전달되지 않고 response 조립에만 재사용된다.
  - `crates/quanta-index-ipc/src/server.rs:1108-1156`
- IPC metric은 집계 counter뿐이며 request ID/close reason/panic이 없다.
  - `crates/quanta-index-ipc/src/counters.rs:15-70`
  - `crates/quanta-index-ipc/src/counters.rs:205-265`
- control surface에 all-plane/process readiness opcode가 없다.
  - `crates/quanta-index-contract/src/ipc/split.rs:127-150`
- process helper도 socket connect만 readiness로 본다.
  - `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:63-99`

한 plane이 죽어도 surviving control plane이 전체 process를 healthy처럼 보이게 할 수 있다.

### P1-19. Mandatory quality/perf artifact 부재가 blocking failure가 아님

판정: `FAIL` / 증거: `S`

- artifact checker 기본 정책은 artifact 부재를 허용한다.
  - `tools/ci/lint/check-bench-artifacts.py:25-28`
- fresh family 목록은 전체 product quality family를 포함하지 않는다.
  - `tools/ci/lint/check-bench-artifacts.py:50-61`
- CI/scheduled workflow는 `--require` 없이 checker를 호출하거나 report-only다.
  - `.github/workflows/ci.yml:235-239`
  - `.github/workflows/correctness.yml:410-444`
- integration summary는 느슨한 boolean 변환과 old schema를 허용하고 exact source digest가 없다.
  - `tools/benchmark/quality_integration_summary.py:47-62`
  - `tools/benchmark/quality_integration_summary.py:102-112`

교정: `just rust-verify-quality-all`은 7개 quality rail을 실제 재실행한다. 문제는 stale artifact만
읽는다는 것이 아니라, aggregate recipe가 blocking workflow에 연결되지 않고 공용 checker의
mandatory-family/source binding이 불완전하다는 것이다.

### P1-20. Provider egress/privacy가 deployment authority에 없음

판정: `FAIL` / 증거: `S`

- OpenAI request는 source/query text를 `input`으로 직렬화한다.
  - `crates/quanta-index-embed/src/openai.rs:286-300`
- runtime query adapter는 raw query text를 provider로 전달한다.
  - `crates/quanta-index-searchd/src/app/runtime.rs:473-493`
- 현재 체크리스트는 credential/log leakage는 다루지만 source/query egress 분류, tenant consent,
  redaction, region/retention, provider allowlist를 요구하지 않는다.

외부 provider를 쓰는 것 자체가 결함은 아니다. 결함은 production profile에서 전송되는 source/query
data의 정책과 proof가 release authority에 없다는 점이다.

## 8. 기존 P0/P1 재확인

다음 2차 finding은 별도 축이 독립 재확인했다.

1. RepoMap same-generation different-content overwrite
   - `crates/quanta-index-repomap/src/store.rs:117-154`
   - `crates/quanta-index-repomap/src/store.rs:259-348`
2. RepoMap activation request digest가 stored snapshot과 결속되지 않음
   - `crates/quanta-index-repomap/src/store.rs:163-207`
   - materialized snapshot에 manifest digest가 보존되지 않음:
     `crates/quanta-index-repomap/src/model.rs:404-411`,
     `crates/quanta-index-repomap/src/materializer.rs:42-52`
3. Finalized replay lookup보다 mutable preflight가 먼저 실행
   - `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs:111-136`
   - `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs:469-475`
   - `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs:630-683`
4. Cursor가 full pin/canonical query에 결속되지 않음
   - `crates/quanta-index-contract-base/src/query/lexical_cursor.rs:77-92`
   - `crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs:174-195`
5. Hybrid nested `top_k`, typed fusion identity/window, explain reconciliation/zero digest 문제
   - 2차 보고서의 P1-2/P1-3과 동일

과장하지 않은 범위:

- cursor 결함에서 silent skip은 확정되지만 cross-tenant data leak은 입증하지 않았다.
- RepoMap expected-active CAS 부재는 store 사실이나 현재 직렬 control dispatch를 넘어선 concurrent
  production P0로 확대하지 않았다.
- old RepoMap reader는 `Arc` snapshot을 보유하므로 UAF/혼합 응답 finding은 성립하지 않는다.
- search-corpus activation의 lock/durability fence에 RepoMap 결함을 일반화하지 않는다.

## 9. 체크리스트 자체의 보완 필요

### 9.1 기존 문구 수정

1. `G1-03`은 과도하게 넓다.
   - 현재 canonical hexagonal lint와 Cargo는 search-plane의 IPC 의존을 canonical CBOR/digest/response
     budget 용도로 허용한다.
   - “IPC concrete dependency 전부 금지” 대신 vendor adapter/process lifecycle 소유 금지와
     canonical wire-codec exception을 명시해야 한다.
2. `G3-06`은 SQLite transaction과 native lexical/semantic cross-store crash convergence를 분리한다.
3. `G4-10`은 snapshot artifact뿐 아니라 valid-JSON active-pointer mutation을 포함한다.
4. `G4-11`은 quarantine destination의 unique incident identity와 overwrite 방지를 요구한다.
5. `focus_subjects`가 strict scope인지 ranking hint인지 product owner가 결정해야 한다.
   현재 unresolved focus는 degraded reason만 남기고 global universe로 fallback한다.
   - `crates/quanta-index-repomap/src/query.rs:69-111`

### 9.2 2차 제안과 합쳐 실제 반영할 delta

아래 ID는 현재 체크리스트에 아직 적용되지 않았다. 2차 제안과 충돌하지 않도록 병합한 번호다.

| ID | 우선순위/증거 | 추가 또는 강화할 검증 |
|---|---|---|
| G2-11 | P1 U | 모든 mutating SDK/RepoMap method가 variant뿐 아니라 request identity, digest, count, sequence, forbidden field를 검증한다. |
| G2-12 | P0 U/F | persisted filename/key encoding이 full identity tuple에 대해 injective이며 separator, `%`, Unicode 조합이 alias되지 않는다. |
| G2-13 | P1 U/D | envelope pin과 모든 candidate/projection identity/order가 결속된다. |
| G2-14 | P1 U | hybrid-seed manifest digest, contribution/rank/order/identity invariant를 decode/SDK에서 검증한다. |
| G3-13 | P0 A/F | finalized replay는 모든 mutable preflight보다 먼저 조회되고 record retention 동안 base lifecycle과 무관하게 original receipt를 반환한다. |
| G3-14 | P1 S/F | serial-ingest invariant를 machine-enforce하거나 in-progress owner/lease/wait protocol을 둔다. |
| G3-15 | P1 A/F | sequence는 positive/versioned/digested이며 open에서 `next > MAX(stored)`를 reconcile하고 duplicate를 거부한다. |
| G3-16 | P1 A/F | semantic refusal가 indefinite in-progress intent를 남기지 않고 terminal-refused 또는 bounded repair로 수렴한다. |
| G3-17 | P1 U/F | graph node typed identity uniqueness와 edge referential/variant integrity를 materialization 전에 검증한다. |
| G3-18 | P1 U/Q | ingest cardinality, materialized bytes, CPU/memory complexity에 route별 hard envelope가 있다. |
| G4-15 | P0 A/F | 동일 logical generation의 different content overwrite와 persisted identity collision을 거부한다. |
| G4-16 | P0 A/F | activation이 candidate digest를 exact compare하고 ACK가 manifest/authority/content identity를 반환한다. |
| G4-17 | P0 F | active pointer는 versioned/self-digested이며 exact candidate content에 결속된다. |
| G4-18 | P0 F | invalidated activation은 durable tombstone/quarantine되고 repair publish만으로 restart 재활성화되지 않는다. |
| G4-19 | P1 F | 모든 declared read domain은 실제 immutable handle/fence를 view에 보유해 activation/GC TOCTOU를 막는다. |
| G5-18 | P1 D/F | cursor를 route/full pin/canonical request/order/cap/aux epoch에 결속하고 cross-query/repo reuse를 거부한다. |
| G5-19 | P1 U/D | 중첩·중복 cap field를 raw wire에서 range/equality 검증한다. |
| G5-20 | P1 U/D | fusion/dedup/universe/window가 동일 typed identity를 사용한다. |
| G5-21 | P1 D | explain reconciliation mismatch는 typed status/refusal이며 string parsing이 필요 없다. |
| G5-22 | P1 D/F | dense `Capped/Partial`이면 `Exact` 및 근거 없는 `has_more=false`를 금지한다. |
| G5-23 | P1 U/D | lane dedup, contribution rank, fused score/order를 한 provenance producer가 계산한다. |
| G5-24 | P1 U/D | Unicode/non-ASCII query를 지원하거나 tokenless normalization을 typed refusal한다. |
| G6-10 | P1 U/D | provenance digest의 zero/empty sentinel을 금지하고 typed absence를 사용한다. |
| G6-11 | P1 Q | quality artifact를 strict schema/full HEAD/dirty/corpus/config/model/host digest에 결속한다. |
| G6-12 | P1 Q | golden oracle input/calculation 경로는 SUT output과 독립적이다. |
| G6-13 | P1 S/D | executed engine과 contributing lane을 분리하고 zero-hit에서도 실제 실행/비용을 기록한다. |
| G6-14 | P1 D | corpus 상태를 unavailable/available-empty/filtered-empty로 분리하고 sealed inventory로 판정한다. |
| G7-10 | P1 U/D | control principal/capability별 read/mutate 권한표와 negative authorization을 검증한다. |
| G7-11 | P1 U/D | 모든 SDK query response를 원 selection/pin/cursor/candidate context에 결속한다. |
| G7-12 | P1 D/X | semantic input을 provider I/O 전에 공통 검증하고 profile-independent typed error를 보장한다. |
| G8-11 | P1 P/F | unexpected plane/maintenance exit가 global shutdown/non-zero/all-plane readiness down으로 전파된다. |
| G8-12 | P1 P/F | N번째 plane spawn 실패 시 이전 plane shutdown/join 후에만 lease를 해제한다. |
| G9-11 | P1 P/F | cooperative request deadline과 hard process drain deadline/escalation을 분리한다. |
| G9-12 | P1 P/F | panic에서도 permit/live counter/peer-watch/FD를 RAII로 회수하고 panic reason을 관측한다. |
| G9-13 | P1 P/X | detached provider work에 process-global thread/request/FD/cost cap과 shutdown drain을 둔다. |
| G10-10 | P1 P | request ID를 dispatcher, typed outcome, generation, latency, close reason, panic과 상관시킨다. |
| G10-11 | P1 P | readiness가 query/control/ingest 생존, maintenance heartbeat, required backend proof를 함께 판정한다. |
| G10-12 | P1 X | provider egress allowlist, source/query classification, consent/redaction, region/retention을 release profile에 결속한다. |
| G11-10 | P1 Q | blocking workflow는 mandatory artifact family 부재를 실패시키고 exact-source artifact를 publish한다. |
| G12-09 | P1 X | 기존 credential non-leak에 source/query payload egress 정책을 별도 항목으로 분리한다. |
| G12-10 | P0 X | daemon binary/source와 producer linked contract/SDK tree digest를 하나의 receipt에 묶는다. |
| G12-11 | P1 X | path dependency resolved root/dirty state를 기록하고 mismatch를 거부한다. |
| G12-12 | P1 X | declared model과 provider-observed immutable model/version 및 usage/cost를 구분한다. |
| G12-13 | P1 A/F | persisted state마다 executable backup/restore/importer와 frozen migration fixture가 있다. |
| G12-14 | P0 X | RepoMap aggregate closeout은 exact prepared payload/manifest/content-bound terminal receipt를 요구한다. |
| G12-15 | P1 D/X | known model mismatch를 provider/lexical fanout 전에 거부한다. |
| G13-06 | P1 S/P | receipt writer가 terminal success, full HEAD, dirty digest, toolchain/features/command/binary SHA를 자체 검증한다. |
| G13-07 | P1 P/F | executable deploy, backup, restore, migration, rollback runbook과 receipt가 있다. |

### 9.3 별도 decision-required 항목

- RepoMap `focus_subjects`는 DTO에서 hint로 보이지만 query 구현은 unresolved focus를 global fallback한다.
  이것이 의도된 recall fallback인지 strict scope인지 owner 결정 전에는 defect로 단정하지 않는다.
- exporter/service packaging은 README상 non-goal이다. production claim 주체가 외부 platform이면 해당
  external receipt를 G13에 연결하고, 아니면 이 repo의 미완료 항목으로 남긴다.
- RepoMap strong terminal receipt는 현재 Semantica 쪽에서 unsupported로 명시되어 aggregate closeout이
  거부된다. direct handoff 성공과 strong terminal closure를 분리해야 한다.

## 10. 우선 수정 순서

1. RepoMap persisted identity encoding, immutable generation, content-bound activation pointer/ACK,
   stale pointer tombstone을 하나의 authority 설계로 수정한다.
2. RepoMap graph validator와 실제 read-view snapshot pin을 추가한다.
3. finalized replay lookup과 fresh semantic preflight/catalog claim을 분리하고 durable sequence를
   open-time reconcile한다.
4. filtered dense completeness, typed fusion/window identity, rank provenance, Unicode query gate를
   수정한다.
5. 공통 SDK request/response binding validator를 query/mutation 전 route에 적용한다.
6. daemon supervisor, startup rollback, hard drain deadline, panic-safe RAII, provider residual cap을
   구현한다.
7. operation authorization, all-plane readiness, request-correlated diagnostics를 추가한다.
8. artifact/deploy/backup/provider-egress authority를 blocking release evidence에 연결한다.
9. source를 clean exact pair로 동결한 뒤에만 `U/A/D/P/F/Q/X` 실행 검증을 수행한다.

## 11. 이번 감사가 증명하지 않은 것

- compile 가능 여부
- unit/integration/E2E 테스트 통과 여부
- 실제 crash/restart/corruption 수렴성
- daemon process signal/drain 시간 상한
- OpenAI cancellation residual work의 실측 상한과 실제 비용
- relevance/ANN recall/latency/RSS/QPS threshold
- clean quanta-index/Semantica exact-pair wire 호환성
- deploy/backup/restore 절차의 실행 가능성

이 항목은 모두 `NOT_RUN` 또는 external input이 필요한 경우 `BLOCKED`다. 정적 source finding을
수정한 뒤 exact-source receipt로 별도 검증해야 한다.
