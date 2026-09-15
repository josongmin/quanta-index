# 구조 개선 테스트 계획 — owner-local / integration

## 1. 범위와 판정 원칙

- 상위 설계: [구조 개선 최종안](structural-remediation-plan.md). 대상 HEAD는 `4914156f4191daa3e12998bdb38f2b821a057fdd`이며 이 문서는 W0–W7의 검증 상세다.
- 구현 handoff: [전체 구조 개선 실행 프롬프트](implementation-agent-prompt.md). 다른 에이전트에게 전달할 실행 범위·순서·종료 계약이다.
- **아래 OL/IT ID는 추가·강화할 테스트 묶음의 계획 ID다. 실행된 test 수나 기존 Rust 함수 이름이 아니다.** 기존 파일은 재사용할 위치를 뜻하며 새 계약을 이미 검증한다는 뜻이 아니다.
- 제품 finding 32건 모두에 owner-local 증거와 통합 증거를 배정한다. 문서/경계 항목은 정적 guard를 함께 사용하고 의미 없는 runtime test를 새로 만들지 않는다.
- owner-local은 해당 owner의 public port/adapter를 직접 검증한다. 실제 backend의 commit/open/delete 의미는 fake adapter로 증명하지 않는다.
- integration은 SDK/UDS, 실제 storage, daemon composition을 연결한다. process crash/lease/restart는 별도 OS process로 실행한다. in-process `reopen`만으로 process recovery를 PASS하지 않는다.
- 정상·거부·복구·consumer 증거를 구분한다. P0/P1 invariant는 [test authority catalog](../../../tools/ci/test-authority.toml)의 독립 proof-target 규칙을 따른다.

### 현재 실행 rail의 실제 범위

[Justfile](../../../Justfile)을 대조한 결과:

| Rail | 현재 범위 | 이 계획에서의 조치 |
| --- | --- | --- |
| `just rust-profile test-fast` | workspace `--lib --bins`, runtime crate 제외 | owner-local unit 묶음. Cargo integration target까지 실행됐다고 해석하지 않음 |
| `just rust-profile test-integration` | contract/core/lexical/RepoMap의 선택된 7개 target | semantic persisted/model, 신규 storage/search-plane target은 별도 명령 및 recipe 등록 필요 |
| `just rust-profile test-daemon` | 선택된 runtime target들과 DSL truth | 현재 `composite_generation_authority_restart`, `semantic_boot_report`는 이 recipe에 없음. 명시적 실행 후 aggregate에 편입 |
| workspace nextest CI | authority catalog에 PR/merge/nightly binding 존재 | 현재 선택 recipe의 누락과 별개. 해당 CI가 실제로 실행한 target/test receipt로 판정 |

위 누락은 “테스트 파일이 없다”는 뜻이 아니다. 선택 recipe의 PASS만으로 해당 target까지 검증됐다고 주장하지 않기 위한 경계다.

## 2. 공통 fixture와 oracle

### 재사용할 기반

- [E2eRuntime 기반](../../../crates/quanta-index-searchd-harness/src/harness.rs): 작은 deterministic corpus와 query/ingest 기능 연결.
- [공통 corpus](../../../crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs), [frontdoor scenario](../../../crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs): SDK route별 기대값과 fixture 공용화.
- [SearchdBinaryProcess](../../../crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs): 실제 child process. 현재 `stop`은 `kill`/`wait`이므로 graceful drain 검증에는 별도 shutdown 경로가 필요하다. 시작 시 socket 연결만 확인하는 것에 더해 protocol readiness handshake를 확인한다.
- [semantic persisted tests](../../../crates/quanta-index-semantic/tests/persisted_semantic.rs): exhaustive constrained cosine oracle 재사용 가능. 새 ANN 품질 판정은 native ANN이 실제 생성된 fixture를 사용한다.

### 공통 dataset

| Fixture | 구성 / 용도 |
| --- | --- |
| F-small | repo A/B, revision 2개, generation g1/g2/g3. 추가·교체·삭제·rename·clear-surface와 빈 generation 포함 |
| F-boundary | 후보 수와 public k의 독립 조합. k=`0,1,9_999,10_000,10_001`; 후보 수=`0,1,k-1,k,k+1` 중 유효 조합. count/projection/response byte 경계 포함 |
| F-text | identifier, phrase, regex false-positive, combining mark, 한글·다문자 casefold, 원문 offset. Native/Sourcegraph syntax별 expected ID 고정 |
| F-vector | finite deterministic vectors, duplicate owner/membership, filtered-out nearest row, zero/NaN/Inf/dimension mismatch, old/new model profiles. exact와 실제 ANN-required corpus를 분리 |
| F-aux | history generation, runtime overlay e1/e2, structural chunk universe, RepoMap nodes/edges. 일부 domain 미준비와 source mismatch 포함 |
| F-scale | W0에서 고정한 corpus/host/model tiers. 같은 한 파일 delta, active/inactive 증가, 1/8/32 client, query+ingest+GC 혼합 |

- expected IDs/count/root/error는 fixture 작성 시 고정한다. 결과를 구현으로 다시 계산해서 expected로 쓰지 않는다. full-vs-delta 비교 외에도 수동 golden과 same-count content mutation을 둔다.
- concurrency는 barrier/channel/관찰된 failpoint handshake로 순서를 만든다. 짧은 sleep 뒤 “아마 실행 중”이라고 가정하지 않는다. outer watchdog timeout은 hang 탐지이며 성능 SLO 증거가 아니다.
- fake clock은 순수 정책/TTL 테스트에만 사용한다. UDS deadline·process lifecycle은 실제 timer와 bounded watchdog으로 확인한다.
- failpoint는 발생 지점 hit를 확인하고 정상 경로 counterexample도 실행한다. 테스트 feature 또는 전용 fixture 경계에 격리하며 production binary가 임의 fault-control 요청을 받아들이게 하지 않는다. fault binary와 release binary의 feature 차이를 receipt에 기록한다.
- 임시 state root, socket, DB, cache, source spool은 test별 격리한다. 실패 시 child process를 종료·wait하고 pin/FD/lock 해제를 확인한다. 재현용 seed/log/root manifest는 repo 밖 artifact 경로에 보존한다.

## 3. Owner-local 테스트 묶음

모든 row의 상태는 **계획**이다. 기존 owner 모듈의 `#[cfg(test)]`와 integration target을 우선 확장한다. 파일 하나당 test 하나를 만드는 방식으로 쪼개지 않는다.

| ID / 작업 | Owner와 위치 | 정상 oracle | 거부·경합·복구 oracle | 실행 종류 |
| --- | --- | --- | --- | --- |
| OL-01 / W1 | [contract-base](../../../crates/quanta-index-contract-base/src), [contract tests](../../../crates/quanta-index-contract/tests), [SDK tests](../../../crates/quanta-index-sdk/src/tests.rs), core policy | canonical body hash/roundtrip, stream/session/read token/window, top-k 경계가 SDK/core에서 일치 | unknown/duplicate field, 다른 body 같은 key, invalid profile/token/window, budget overflow. hash 필드 자체 제외와 mutation field 포함 확인 | unit + contract integration + fuzz |
| OL-02 / W2 | 신설 storage adapter. 제안 target `catalog_transactions`, `catalog_faults` | 실제 SQLite에서 operation/receipt/epoch/reference commit; pin된 aux epoch 조회; batch별 checkpoint | commit 전후 IO fault, busy deadline, invisible staging, replay floor, retired session, Deleting object attach 거부. DB 재open 후 결과 일치 | 실제 temp DB + fault injection |
| OL-03 / W2 | [lifecycle](../../../crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs), [ingest dispatcher](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs), retention owner | 순수 reference state machine과 publish/activate/CAS/pin 전이 일치 | invalid base/profile에 backend mutation 0; stale attempt publish 거부; base GC 후 replay; pin/retire 순서별 생존. positive/negative target은 authority 등록 시 분리 | controlled ports + real catalog 결합 |
| OL-04 / W3 | [tantivy_smoke](../../../crates/quanta-index-lexical/tests/tantivy_smoke.rs), [planner_authority](../../../crates/quanta-index-lexical/tests/planner_authority.rs) | real Tantivy full/delta/reopen의 ID·순위·logical root 일치, unchanged segment 공유 | 각 필수 sidecar missing/truncated/checksum mismatch; delete/merge/GC 중 old reader 보존; file/dir sync fault. 파일 존재만으로 durability PASS 금지 | real adapter/temp FS |
| OL-05 / W3 | semantic [persisted](../../../crates/quanta-index-semantic/tests/persisted_semantic.rs), [lifecycle model](../../../crates/quanta-index-semantic/tests/semantic_generation_lifecycle_model.rs), [SCv2 scenarios](../../../crates/quanta-index-semantic/tests/scv2_persisted_scenarios.rs) | exact version open, old-base branch, full/delta row·membership root 일치; 실제 index metadata 확인 | missing ANN, stale membership, post-delete refill, model/norm 오류, cleanup 중 pin; restart 후 exact version. generated seed를 reference model과 대조 | real Lance + independent exact oracle |
| OL-06 / W4 | search-plane snapshot registry 신규 모듈; [기존 query owner](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs) | same key 32 waiter → open 1회, cache-fit warm metadata reopen 0 | 최초 waiter 취소 후 나머지 생존, 모든 waiter 취소, load 실패 재시도, active handle eviction, quarantine epoch와 결과 확정 경합 | controlled opener/counters/barriers |
| OL-07 / W4 | query planner/executor 신규 모듈, [core policies](../../../crates/quanta-index-core/tests) | operator별 top-k/lookahead/scope cap; count·distinct의 독립 reference 결과 | corpus-wide all-match, regex/output/visited cap, oversized single item, incomplete ANN window. capacity는 public k와 내부 probe를 구분 | tiny deterministic corpus + counting collector |
| OL-08 / W5 | [IPC server](../../../crates/quanta-index-ipc/src/server.rs), [IPC wire tests](../../../crates/quanta-index-ipc/tests/wire_historical_cbor.rs), searchd executor | bounded socket accept/read, class별 queue/permit 반환, 정상 shutdown | slowloris/partial frame, disconnect, queue full, child permit starvation, same-root second owner, permissions/peer mismatch. fake dispatch 완료·실제 점유 slot을 함께 관찰 | 실제 local socket + controlled dispatch |
| OL-09 / W6 | [embedding cache](../../../crates/quanta-index-embed/src/cache.rs), [query embedder](../../../crates/quanta-index-search-plane/src/query_embedder.rs) | corpus/query/cache profile 일치, batch stream peak 제한, atomic cache read | same-name revision 교체, wrong dim/norm/NaN, corrupt/truncated cache, concurrent eviction/write, provider timeout/retry. 요청/embedding row 수를 따로 계측 | temp cache + deterministic HTTP/provider stub |
| OL-10 / W2/W6 | [aux authority](../../../crates/quanta-index-search-plane/src/readiness.rs)에서 이동한 history/runtime/structural owner | key/version visibility, 단일 overlay epoch, source dependency, history total order | missing required authority만 거부, unrelated history 미준비 시 lexical 허용, e1 pin 중 e2 update/GC, partial large batch invisible | real catalog + small domain fixture |
| OL-11 / W6 | [hybrid policy](../../../crates/quanta-index-core/tests/hybrid_policy.rs), query ranker/explain owner | independent lane union 대 rerank 차이, stable tie/dedup, rank contribution과 canonical seed/metric 일치 | dense lane 실패의 silent downgrade 거부, empty 정상 결과 구분, duplicate owner, scope 밖 hit 제외; embedding 동일 입력 중복 호출 없음 | deterministic lane/provider spies |
| OL-12 / W6 | [normalization tests](../../../crates/quanta-index-lq-norm/tests), [bridge](../../../crates/quanta-index-lq-bridge/tests), [positions](../../../crates/quanta-index-lq-positions/tests), [trigram](../../../crates/quanta-index-lq-trigram/tests), [regex](../../../crates/quanta-index-lq-regex/tests) | F-text golden offsets/IDs; full/from-prior equivalence | unicode fold/token boundary, invalid regex/complexity, exact verify false-positive 제거, delete 후 postings 잔존 없음 | golden + property |
| OL-13 / W6 | [RepoMap owner_surface](../../../crates/quanta-index-repomap/tests/owner_surface.rs), [bootstrap_owner_flow](../../../crates/quanta-index-repomap/tests/bootstrap_owner_flow.rs) | immutable view/node/edge order, keyset page union=전체 reference | page cap, stale/tampered token, crash 전후 epoch, query 중 publish, 없는 node/edge, whole snapshot clone 계측 제거 | actual domain store/temp persistence |
| OL-14 / W0/W5/W7 | [harness scenarios](../../../crates/quanta-index-searchd-harness/src/scenarios.rs), [artifact](../../../crates/quanta-index-searchd-harness/src/artifact.rs), [benchmark tooling](../../../tools/benchmark), [test authority guard](../../../tools/ci/lint/check-test-authority.py) | expected truth 선검증, exact-source artifact, bounded metric/sample counts | missing/skipped row, stale SHA, null threshold, error를 success latency로 혼합, label/sample cap. docs는 path/capability/경계 guard로 검사 | harness/tooling tests + static guards |

### Owner-local 실행 방법

아래는 repo root에서 실행할 **명령 형식**이다. `<owner>`, `<target>`, `<exact_test_name>`은 실제 등록한 값으로 치환한다. 새 storage crate/target은 생성·등록 전 실행할 수 없다.

```sh
# Unit owner loop; a selected function must be discovered before execution.
./scripts/cargow --lane test-fast-lane test --locked --all-features -p <owner> --lib -- --list
./scripts/cargow --lane test-fast-lane test --locked --all-features -p <owner> --lib <exact_test_name> -- --exact

# Native adapter / owner integration target.
./scripts/cargow --lane test-integration-lane test --locked --all-features -p <owner> --test <target> -- --list
./scripts/cargow --lane test-integration-lane test --locked --all-features -p <owner> --test <target>
```

- filter 실행은 selected/executed count를 확인한다. `0 passed`, ignored-only, fixture early-return은 PASS 증거가 아니다.
- focused regression이 통과하면 같은 owner의 영향받는 target 전체를 실행한다. body hash/visibility/reference owner 변경은 positive뿐 아니라 negative/recovery target도 포함한다.
- 기존 semantic integration을 놓치지 않도록 최소한 다음 target을 별도 포함한다. 이 명령들은 **이번 문서 작성에서 실행하지 않았다**.

```sh
./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test persisted_semantic
./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test semantic_generation_lifecycle_model
./scripts/cargow --lane test-integration-lane test --locked --all-features -p quanta-index-semantic --test scv2_persisted_scenarios
```

## 4. Integration 시나리오

**D** = 실제 SDK/UDS와 native backend를 연결한 runtime. **P** = D를 별도 child process로 실행. **Q** = 고정 corpus/host/profile의 성능·품질 runner. P에서만 OS process crash/lease 증거를 발급한다.

기존 target에 case를 확장하고, 의미상 새 lifecycle target이 필요하면 authority catalog와 Justfile에 함께 등록한다. 아래 target 이름은 기존 재사용 위치이며 모든 새 case가 이미 존재하는 것은 아니다.

| ID / 선행 OL | 시나리오: 입력 → 동작 | 합격 oracle | 형태 / target owner |
| --- | --- | --- | --- |
| IT-01 / 01–05 | F-small full/unsealed batches → seal → activate CAS → SDK lexical/semantic query → process restart | ack/root/read token 결속, g1/g2 선택별 expected IDs, publish만으로 active 전환 없음, 빈 정상 generation도 serve | P; `end_to_end`, `sdk_frontdoor`, `composite_generation_authority_restart` |
| IT-02 / 02–05 | invalid cross-track batch 및 정상 batch의 write/fsync/native seal/catalog commit/activation 앞뒤 failpoint → kill/restart/retry | invalid 입력의 index mutation 0; old active 보존; committed receipt만 replay; 한쪽 prepared를 public generation으로 serve하지 않음 | P; `composite_generation_authority_restart` 확장 + 필요 시 신규 fault target |
| IT-03 / 01–03/09 | commit → ack 유실 → base GC → same-body replay → receipt floor prune → retired session; 별도 stale worker 지연 commit | 보존 범위의 동일 receipt, 재apply/중복 committed row 0, 이후 expired/retired/conflict 구분, 늦은 fence 거부. provider 중복 호출은 외부 exactly-once로 판정하지 않음 | P; `composite_generation_authority_restart`, `e2e_restart_replay_determinism` |
| IT-04 / 02–06 | g1 query barrier 중 g2 activate·g1 retire·new attach·compaction을 교차 실행; release 후 GC; 중간 restart | query pin 선점 시 old bytes 생존, retire 선점 시 새 pin 거부, shared object 최종 참조 전 삭제 0, release 후 실제 회수; repo B 영향 없음 | D+P; `e2e_generation_activation_concurrency`, composite target |
| IT-05 / 04–06 | cache-fit 동일 query 32개, 첫 waiter 취소; active root 증가; inactive/active 필수 artifact 손상; cold restart | miss open 1회, warm metadata reopen 0, bytes/FD cap, failed flight 재시도, inactive 격리, active 손상 scope 거부, protocol readiness 상태 정직 | D+P; `semantic_boot_report`, composite target, `e2e_perf_chaos` |
| IT-06 / 01/07/12 | F-boundary를 모든 지원 route의 SDK/UDS로 실행; count/distinct/regex all-match, wire cap 전후 입력·출력 | k=10,000 허용·0/10,001 거부, scope cap이 backend 입력에 반영, count exactness/unknown 구분, bounded encode 후 정상 frame, 거부 뒤 다음 query 정상 | D; `sdk_frontdoor`, `e2e_text_route_hellgate`, `e2e_perf_chaos` |
| IT-07 / 08 | slow peer + blocked long query + 정상 client + ingest/GC; query/control queue 포화, disconnect, 두 번째 daemon start, graceful shutdown | 정상 client 독립 진행, 명시적 overload/deadline, 취소 후 slot 점유 실측, claimed mutation은 status로 결론, live socket unlink 0, peer/mode policy, drain 후 child 종료 | P; composite/chaos target 확장. 현재 kill helper와 별개 graceful fixture 필요 |
| IT-08 / 03–05/12 | 같은 final corpus를 full와 여러 delta sequence로 생성; replace/delete/rename/clear 후 restart, pinned old base에서 분기 | independent full oracle와 logical roots/IDs 일치; membership·offset 유지; unchanged bytes 재복사 0. ANN은 고정된 품질 tolerance로 비교하며 byte/순위 완전 동일을 강제하지 않음 | D+P+Q; `e2e_lexical_full_fidelity`, `e2e_restart_replay_determinism`, semantic targets |
| IT-09 / 02/03/10 | F-aux e1 query 중 e2 대형 batch를 stage/publish; aux 일부 미준비·source mismatch; epoch GC 후 restart | 한 query에서 epoch 혼합 0, invisible rows 미노출, 필요한 domain만 not-ready, unrelated lexical 성공, old epoch pin 유지·release 후 GC | D+P; `e2e_predicate_authority_lifecycle`, `e2e_filter_execution`, composite target |
| IT-10 / 01/05/09 | daemon에 deterministic provider 연결; ingest/query/cache 공통 profile; model revision 교체·cache corruption·provider 지연/오류 | 공간 혼용 0, corrupt entry miss/재계산과 metric, finite/dim/norm 거부, batch별 vector residency cap, 실패가 ready/성공으로 바뀌지 않음 | D+P; `end_to_end`, `semantic_boot_report` 및 embed owner fixture |
| IT-11 / 05/07 | 실제 ANN-required corpus에서 nearest rows delete/replace, selective filter, unindexed delta, ANN artifact 삭제 후 reopen | live-row coverage 100%, duplicate/stale membership 0, refill 중 budget/exhaustiveness 표시 정확, exact oracle 대비 ANN recall threshold 통과 | D+Q; semantic persisted/SCv2, `end_to_end` |
| IT-12 / 07/09/11 | lexical-only hit·semantic-only hit·겹친 hit와 hard filter; hybrid/re-rank/seed/explain을 SDK로 실행 | hybrid가 dense-only hit 포함, rerank는 scope 준수, canonical candidate/metric/window 일치, 원 query rank trace·exact presence, dense 실패 silent downgrade 0 | D; `sdk_frontdoor`, `explain`, `e2e_perf_chaos` |
| IT-13 / 10–13 | F-text와 history/RepoMap을 Native/Sourcegraph로 query; relevance/recency keyset page 중 mutation·token tamper/expiry | text 의미·offset golden, history total order, page union에 중복/누락 0, 원 read token 고정, graph node/edge/byte cap과 authority 오류 | D+P; `e2e_dual_syntax_lowering_parity`, `dsl_scenarios`, `repo_map_end_to_end` |
| IT-14 / 01–05/10 | old-format frozen export → 새 root import/rebuild → binary restart → producer/SDK version cut → read-only rollback 및 post-write 정책 검사 | high-water/domain root·counts·query·replay 일치, version mismatch 명시, unsupported old replay 자동수용 0, rollback 가능 범위 receipt 일치 | P + 외부 producer; 신규 migration target 필요, `rust-verify-hellgate-cross-repo` |
| IT-15 / 04–09/14 | F-scale 1/8/32 client, active/inactive 증가, 반복 delta/compaction/GC/cache churn; controlled cold/warm/soak | p50/p95/p99·QPS/error·RSS/FD·disk/WAL·queue가 사전 threshold 만족; 회수 가능한 객체·sample의 단조 누수 없음; compaction/GC 진행 확인 | Q+P; harness `scale`/`tail`/`ops`, benchmark tooling 확장 |
| IT-16 / 05/09/11/14 | production learned profile로 fixed held-out corpus ingest → dense/hybrid/rerank 및 exact-vector baseline 비교 | profile/source digest 결속, judged nDCG/MRR/recall와 ANN recall threshold; real provider identity/error 경계 확인. hash/stub 결과로 대체 금지 | Q + opt-in real provider; harness `relevance`와 `relevance_openai_ab` |

### Process·외부 integration 실행

현재 recipe 밖에 있는 실제 target을 명시적으로 실행하는 예:

```sh
./scripts/cargow --lane test-daemon-lane test --locked --all-features -p quanta-index-searchd-runtime --test composite_generation_authority_restart
./scripts/cargow --lane test-daemon-lane test --locked --all-features -p quanta-index-searchd-runtime --test semantic_boot_report
just rust-profile test-daemon
```

- 실제 child process test는 `CARGO_BIN_EXE_quanta-index-searchd`의 binary path/hash와 feature set을 기록한다. test 함수의 존재만으로 production release binary까지 동일하다고 주장하지 않는다.
- 외부 producer는 `QUANTA_INDEX_SEARCHD_BIN`에 검증 대상 binary를 지정한 뒤 `just rust-verify-hellgate-cross-repo <producer_root>`를 실행한다. producer HEAD·dirty digest·features와 daemon binary hash를 함께 남긴다. 이 recipe 내부의 외부 Cargo 실행은 해당 repo 환경 규칙도 충족해야 한다.
- real provider는 자격 증명·고정 model/profile·비용/요청 budget이 있는 별도 rail이다. local PR suite는 stub을 사용하고, credential 미제공/실행 제외는 IT-16 `blocked/not-run`으로 남긴다.
- benchmark/quality recipe가 저장하는 기존 short SHA·기본 artifact path를 그대로 최종 authority로 사용하지 않는다. W0에서 상위 계획의 40-char SHA/source/model/host schema와 repo 밖 artifact 출력으로 정리한 뒤 실행한다.

## 5. Finding별 최소 증거 연결

아래는 최저 요구 묶음이다. 동일 fixture를 여러 finding이 공유할 수 있지만, 정상 경로의 한 PASS를 unrelated negative/recovery 증거로 복제하지 않는다.

| Finding | Owner-local | Integration / 추가 guard |
| --- | --- | --- |
| QI-BB-001 | OL-06 | IT-05 |
| QI-BB-002 | OL-08 | IT-07, IT-15 |
| QI-BB-003 | OL-02, OL-03, OL-04, OL-05 | IT-04, IT-15 |
| QI-BB-004 | OL-07 | IT-06, IT-11 |
| QI-BB-005 | OL-07 | IT-06, IT-15 |
| QI-BB-006 | OL-04, OL-05 | IT-08, IT-15 |
| QI-BB-007 | OL-09 | IT-16 |
| QI-BB-008 | OL-10, OL-13 | IT-09, IT-13 |
| QI-BB-009 | OL-09 | IT-10, IT-15 |
| QI-BB-010 | OL-14 | IT-15, artifact truth/schema guard |
| QI-BB-011 | OL-12 | IT-13 |
| QI-BB-012 | OL-14 | IT-01/IT-05/IT-07의 구현 claim 대조 + doc/capability lint |
| QI-BB-013 | OL-14 | IT-01 + hexagonal/module/public API guards |
| QI-BB-014 | OL-08 | IT-07 |
| QI-BB-015 | OL-08, OL-14 | IT-15 |
| QI-BB-016 | OL-08 | IT-15 |
| QI-BB-017 | OL-05, OL-06 | IT-05, IT-15 |
| QI-BB-018 | OL-11 | IT-12, IT-16 |
| QI-BB-019 | OL-11 | IT-12 |
| QI-BB-020 | OL-02, OL-10 | IT-09, IT-15 |
| QI-BB-021 | OL-05, OL-09 | IT-10, IT-15 |
| QI-BB-022 | OL-11 | IT-12 |
| QI-BB-023 | OL-10 | IT-13 |
| QI-BB-024 | OL-07, OL-12 | IT-06, IT-15 |
| QI-BB-025 | OL-01, OL-07 | IT-06 |
| QI-BB-026 | OL-02, OL-06 | IT-05 |
| QI-BB-027 | OL-05 | IT-11 |
| QI-BB-028 | OL-09 | IT-10, IT-16 |
| QI-BB-029 | OL-03 | IT-02 |
| QI-BB-030 | OL-04 | IT-02, IT-05 |
| QI-BB-031 | OL-01, OL-05, OL-09 | IT-10, IT-11 |
| QI-BB-032 | OL-02, OL-03 | IT-03 |

DA-01/02/10은 IT-03/05/07, DA-03/05는 IT-04/05, DA-04는 IT-09, DA-06은 IT-08, DA-07은 IT-06/11, DA-08은 IT-05/07/15, DA-09는 API/경계 inventory, DA-11은 IT-14, DA-12는 OL-14/IT-15/16 및 W0 gate로 검증한다.

## 6. 등록·실행 순서와 승격 조건

### 테스트 등록

1. owner가 기존 integration target을 확장하거나 의미가 다른 새 target을 만든다. 신규 파일 경로·Cargo target·authority ID를 같은 변경에 등록한다. 상위 계획의 OL/IT ID는 Rust 함수명 대신 추적 필드로 사용한다.
2. [test-authority.toml](../../../tools/ci/test-authority.toml)의 `integration_targets`와 executable CI rail을 연결한다. 새 P0/P1 invariant는 `invariant_universe` 및 positive/negative/recovery/consumer mapping을 추가·이전한다.
3. 현재 guard는 P0/P1의 positive/negative에 owner-local을 요구하고 proof-role 독립성을 검사한다. 같은 target 하나를 네 역할에 반복 기입하지 않는다. 필요하면 owner-local positive/negative target을 별도 책임으로 구성하고 consumer는 runtime/SDK target으로 둔다.
4. `Justfile` 선택 recipe에도 새 target을 넣는다. test-authority 등록만으로 `test-integration`/`test-daemon` recipe 선택이 자동 갱신되지는 않는다.
5. `just rust-test-authority` 및 `just rust-ignored-test-policy`를 실행하고 CI receipt에서 실제 target/test 수를 확인한다. registry 정적 PASS는 runtime PASS가 아니다.

### C1–C4에 필요한 테스트

| Checkpoint | 먼저 통과할 owner-local | 필수 integration | 다음 단계로 넘기는 증거 |
| --- | --- | --- | --- |
| W0 backend 결정 | OL-04/05의 native lifetime probe, OL-02 catalog probe, OL-08 runtime probe, OL-14 fixture truth | native G0-L/S/C/R은 해당 probe 결과로 판정; performance baseline은 IT-15의 기존 기능 subset | 선택 API/layout, 실패 재현, source/config/host, threshold. 미래 구현 전체 PASS로 확장 금지 |
| C1 최소 catalog slice | OL-01–05의 해당 slice | IT-01/02/03 중 single-repo full/empty/unsealed/seal/restart/replay | 실제 durable receipt와 child restart; 남은 delta/aux gate 명시 |
| C2 수명·delta | OL-02–07, OL-10/12의 필요한 부분 | IT-04/05/08/09/11 | shared-byte GC, full-vs-delta oracle, aux visibility, native version pin |
| C3 부하·품질 | OL-01–14의 변경 범위 전부 | IT-06/07/10/12/13/15/16 및 C1/C2 회귀 | resource·native correctness·production relevance를 구분한 receipt |
| C4 전환 | migration/importer owner-local, OL-01/02/03/10/14 | IT-14와 동일 최종 source의 전체 필수 IT case | producer/SDK/binary/version/root/rollback 경계. 외부 rail 미실행이면 release blocked |

작업 중에는 변경 owner-local → 관련 IT만 실행한다. 통합 checkpoint에서 `test-fast`, `test-integration`, 명시적 semantic/storage target, `test-daemon`, 누락된 process target을 올린다. public DTO/error 변경은 public-api/fuzz, crate 경계 변경은 hexagonal/modules/workspace check/test를 추가한다. 같은 source·조건의 이미 통과한 무관한 큰 rail을 반복 실행하지 않는다.

### 최종 receipt

- 공통: exact HEAD + dirty/source digest, command/target/filter, feature/toolchain, selected/executed/passed/failed/ignored 수, raw log, fixture seed/digest, test-plan ID.
- storage/process: DB/native versions, state-root manifest, failpoint hit 위치, process/binary hash, crash 전후 receipt/root/pin/reference 상태, 실제 cleanup 결과.
- resource/quality: offered/accepted/completed QPS와 error율, latency distribution, RSS/FD/disk/WAL, cache/open/scan/GC counter, model/corpus/threshold digest, host/noise 조건.
- status는 `planned / implemented-not-run / passed / failed / blocked / not-applicable`을 구분한다. 미지원 OS test는 이유와 실제 지원 OS의 대응 증거를 요구하며 silent skip으로 닫지 않는다.
- `owner-local passed`는 integration/recovery/relevance 완료가 아니다. in-process harness PASS는 child-process crash PASS가 아니다. 현재 프로파일의 ignored real-provider test는 IT-16을 닫지 않는다.
- 최종 완료는 32개 finding의 최소 연결이 모두 채워지고, 변경 P0/P1 proof-role과 상위 계획의 G0/C1–C4 조건이 같은 최종 source에서 충족된 경우다.

## 7. 이번 변경의 검증 범위

이번 작업은 **테스트 계획 문서 추가**다. Rust test 구현·실행, 실제 provider 호출, backend probe, 성능 측정은 하지 않았다. 기존 test 파일·recipe·authority catalog를 정적으로 확인했다. OL 14개/IT 16개/32개 finding 연결과 실행 예시 target 5개의 존재·등록 검사는 PASS다. bugbash 문서들의 local link/anchor·whitespace 검사와 root hygiene도 PASS이며, repo doc lint는 기존 broken paths 2건으로 실패했다. 상세 결과는 상위 계획의 검증 기록에 반영했다.
