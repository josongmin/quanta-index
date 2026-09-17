# quanta-index 최종 bugbash findings — 2026-09-16 (4차 확정본)

후속 실행 계획: [구조 개선 최종안](structural-remediation-plan.md). 32개 finding을 매핑하고 적대 검토에서 확인한 설계 누락·모순 12건을 반영했다. 공통 owner, 전환·삭제 순서, 반례 fixture와 검증 gate를 포함한다. 계획 보완은 제품 finding 해결 완료를 뜻하지 않는다.

## 1. 감사 경계

- 대상 HEAD: `4914156f4191daa3e12998bdb38f2b821a057fdd` (`4914156 fix(search): harden corpus authority roots`)
- 기준 브랜치: `main`, 감사 시작 시 `origin/main`과 동일하고 worktree는 clean이었다.
- 범위: lexical, semantic/hybrid, generation lifecycle, incremental ingest, UDS IPC, RepoMap, embedding, 품질/성능 증거, 운영성, 문서/유지보수성.
- 이번 변경은 감사 문서만 추가한다. 제품 코드는 수정하지 않았다.
- 1차 감사 시작 시 worktree는 clean이었다. 2·3·4차 감사 시작 시에는 앞선 결과인 `docs/bugbash/`만 untracked였고 제품 source/config에는 변경이 없었다.
- 2차 패스는 semantic open/validation, hybrid 후보 집합, auxiliary authority persistence/locking, ingest resource envelope, explain/history 결과 의미를 독립적으로 다시 추적했다.
- 3차 패스는 query 경계값, daemon boot/recovery, ANN index lifecycle, embedding cache/model identity, ingest 선검증과 UDS bind ownership을 추가로 추적했다.
- 4차 패스는 lexical seal/sidecar durability와 activation equivalence, semantic normalization invariant, batch replay/idempotency identity, cache corruption 경계를 추가로 추적했다.
- 증거 등급:
  - **확정**: 현재 HEAD의 도달 가능한 코드 경로 또는 현재 HEAD에서 실행한 검사로 재현됨.
  - **추론**: 코드 구조상 예상되는 운영 영향. 실제 운영 corpus/host 부하 측정은 별도 필요.
  - **미검증**: 외부 시스템, 실제 OpenAI 호출, Linux 전용 성능 runner처럼 이번 로컬 감사에서 실행하지 않은 범위.

## 2. 결론

### 직접 답변

- lexical backend는 자체 inverted-index 엔진이 아니라 **Tantivy 0.22**다. 의존성은 [Cargo.toml](../../../Cargo.toml):137, adapter는 [quanta-index-lexical](../../../crates/quanta-index-lexical)에 있다.
- Tantivy 선택은 유지하는 편이 맞다. BM25, segment, mmap, query execution을 다시 구현할 이유가 없다.
- 다만 현재 lexical은 Tantivy 위에 phrase position, trigram, regex verification, metadata projection용 sidecar를 직접 구축한다. 따라서 “Tantivy만 얇게 감싼 구조”는 아니다.
- 증분 update API는 있다. 하지만 물리 비용은 진정한 `O(changed data)` 증분이 아니다. lexical delta는 base generation 전체를 복사하고 text 변경 시 전체 text authority sidecar를 재생성한다. semantic도 dataset 전체를 staging으로 복사한 뒤 seal 시 전체 commitment/index 작업을 한다.

### 판정

- **기능 정확성 기반은 강하다.** exact generation identity, fail-closed activation, typed refusal, deterministic ordering/restart, DSL capability guard, broad E2E가 구현돼 있다.
- **대규모·장기 실행 production ready로 판정할 수 없다.** 현재 HEAD에 P1 13건, P2 17건, P3 2건이 남아 있다.
- 가장 먼저 고칠 항목은 lexical seal과 query-open 검증의 불일치, `scope_top_k`/`top_k` 계약 위반, ingest 선검증 실패로 생기는 half-sealed generation, lexical query별 전체 reopen, 직렬 UDS, 실제 index directory를 지우지 않는 retention, semantic open/boot의 전체 integrity rescan, embedding model/cache identity, batch idempotency, auxiliary authority의 전역 lock/whole-snapshot rewrite다.
- 기본 semantic은 64차원 token hash다. 배선과 model identity 검증은 되지만 의미 기반 검색 품질을 대표하지 않는다.
- 일반 hybrid는 독립 lexical+dense recall이 아니라 lexical 후보 안에서만 dense rerank한다. semantic-only relevant hit는 현재 API에서 들어올 수 없다.
- **P0는 발견하지 못했다.** 이는 전체 결함 부재 증명이 아니라 현재 실행 범위에서 즉시 데이터 손상/전면 불능을 재현하지 못했다는 뜻이다.

## 3. 심각도 기준

| 등급 | 기준 |
| --- | --- |
| P0 | 즉시 데이터 손상, 활성 세대 오염, 기본 경로 전면 불능 |
| P1 | production 차단 수준의 계약 위반, 가용성 또는 무제한 자원 문제, 핵심 기능 품질 미충족 |
| P2 | scale-up 또는 장기 운영 전에 해결해야 하는 성능, 내구성, 보안 경계, 검증 공백 |
| P3 | 유지보수/용량 조정 부채. 단독으로 현재 기능을 깨지는 않음 |

## 4. Findings index

| ID | 등급 | 영역 | 요약 | 상태 |
| --- | --- | --- | --- | --- |
| QI-BB-001 | P1 | lexical query | 매 query마다 sealed generation과 모든 sidecar를 다시 연다 | 확정 |
| QI-BB-002 | P1 | IPC | socket별 accept loop가 connection/request를 직렬 처리하며 실행 취소가 없다 | 확정 |
| QI-BB-003 | P1 | retention | authority record만 GC하고 Tantivy/LanceDB 실데이터는 지우지 않는다 | 확정 |
| QI-BB-004 | P1 | semantic scope | SDK/contract의 `scope_top_k`를 dispatcher가 적용하지 않는다 | 확정 |
| QI-BB-005 | P1 | lexical resources | `count:all`과 projection이 corpus 전체를 collect할 수 있다 | 확정 |
| QI-BB-006 | P2 | incremental ingest | logical delta가 generation/dataset 전체 복사와 전체 sidecar rebuild를 유발한다 | 확정 |
| QI-BB-007 | P1* | semantic quality | 기본 semantic provider가 learned embedding이 아닌 64차원 token hash다 | 확정 |
| QI-BB-008 | P2 | RepoMap | query가 snapshot 전체를 이중 clone/sort/반환하고 persistence가 비원자적이다 | 확정 |
| QI-BB-009 | P2 | embedding cache | disk cache와 provider request sample에 retention/cap이 없다 | 확정 |
| QI-BB-010 | P2 | performance proof | 현행 scan experiment가 실행 불가하고 checked-in 수치가 현재 HEAD를 증명하지 않는다 | 확정 |
| QI-BB-011 | P2 | text semantics | phrase/raw/regex 보조 경로가 whitespace/ASCII folding에 의존한다 | 확정 |
| QI-BB-012 | P2 | documentation | README의 reader cache/async listener 설명이 실제 구현과 다르다 | 부분 보완 (2026-09-16 README/SSOT sync) |
| QI-BB-013 | P3 | maintainability | 핵심 dispatcher/adapter/authority 파일이 수천 줄 단위로 결합돼 있다 | 확정 |
| QI-BB-014 | P2 | local IPC | UDS mode/peer identity를 강제하지 않고 기존 live socket도 stale로 간주해 unlink할 수 있다 | 확정, 영향은 배포 환경 의존 |
| QI-BB-015 | P2 | observability | 진단 저장소는 process-local이며 일부 error/sample 벡터는 무제한이다 | 확정 |
| QI-BB-016 | P3 | capacity | lexical writer cache 메모리 한도가 고정값이고 전체 RSS budget과 연동되지 않는다 | 확정 |
| QI-BB-017 | P1 | semantic lifecycle | generation open/검증이 semantic row와 membership 전체를 반복 scan한다 | 확정 |
| QI-BB-018 | P2 | hybrid recall | 일반 hybrid의 dense lane은 lexical 후보에 갇혀 semantic-only hit를 찾지 못한다 | 확정 제한, 제품 계약 결정 필요 |
| QI-BB-019 | P2 | hybrid seed | legacy/v2 동시 제공 때문에 dense work와 payload가 중복되고 metric이 active 결과와 다르다 | 확정 |
| QI-BB-020 | P1 | auxiliary authority | 전역 ledger lock 아래 전 세대 snapshot을 clone/전체 rewrite하며 GC도 하지 않는다 | 확정 |
| QI-BB-021 | P2 | ingest resources | semantic derivation이 배치 전체 vector를 메모리에 모으고 resource 상한이 없다 | 확정 |
| QI-BB-022 | P2 | explain | explain이 원 질의/점수 기여도가 없는 snippet 기반 presence probe다 | 확정 |
| QI-BB-023 | P2 | history quality | history `top_k`가 relevance/시간이 아니라 SHA key 순서로 잘린다 | 확정 |
| QI-BB-024 | P2 | regex resources | regex cache가 entry 수만 제한하고 corpus-wide ID 집합을 복제 보관한다 | 확정 |
| QI-BB-025 | P1 | query contract | `top_k` 허용 범위가 route마다 다르고 0/10,000 경계가 잘못 동작한다 | 확정 |
| QI-BB-026 | P1 | restart availability | 비활성 과거 generation 하나의 손상도 daemon boot 전체를 막는다 | 확정 |
| QI-BB-027 | P2 | ANN lifecycle | ANN 종류/파라미터/존재가 manifest와 open-time 검증에 포함되지 않는다 | 확정, index 손실 시 Lance 동작은 미검증 |
| QI-BB-028 | P1 | embedding identity | cache key와 OpenAI identity가 model revision을 구분하지 못해 embedding space가 섞일 수 있다 | 확정 경로, provider 변경 영향은 추론 |
| QI-BB-029 | P1 | ingest recovery | cross-track contract를 mutation 전에 검증하지 않아 lexical-only sealed target을 만들 수 있다 | 확정 |
| QI-BB-030 | P1 | lexical durability | seal/activation 검증이 query 필수 sidecar의 내구성과 openability를 보장하지 않는다 | 확정, focused fault test 재현 |
| QI-BB-031 | P2 | semantic contract | `L2Unit`을 기록하지만 실제 vector unit norm을 검증하거나 정규화하지 않는다 | 확정, cosine 단독 순위 영향은 제한적 |
| QI-BB-032 | P2 | ingest replay | `batch_digest`가 idempotency key로 검증·기록·ack되지 않아 replay를 구분하지 못한다 | 확정 구현 공백, 보장 의도는 역사 계획 문서 기준 |

`QI-BB-007`은 제품이 semantic search 품질을 기본 기능으로 보장할 때 P1이다. hash profile을 명시적인 개발/테스트 전용 모드로 규정하면 P2 구성/출시 차단으로 낮출 수 있다.

## 5. 상세 findings

### QI-BB-001 — lexical generation이 query마다 전부 reopen된다

**증거**

- text query가 매번 `lex_opener.open(...)`을 호출한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):428-439.
- symbol, hybrid, semantic lexical scope, hybrid seed, explain도 같은 opener를 호출한다: 동일 파일 489-491, 548-550, 643-645, 732-734, 983-985.
- `LexicalAdapter::open`은 호출마다 sealed identity 확인, `Index::open_in_dir`, tokenizer 등록, repo metadata load, `IndexReader` 생성/reload, text authority sidecar와 모든 metadata snapshot load를 수행한다: [lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):3682-3760.
- semantic adapter에는 exact generation key 기반 bounded open cache(8)가 있다: [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):76-125, 180-240. lexical에는 동등한 read cache가 없다.

**도달 영향**

- daemon의 정상 text/symbol/hybrid/explain 요청이 모두 disk metadata read와 CBOR sidecar deserialize 비용을 반복한다.
- text authority는 original/folded text, trigram, position map을 포함하므로 corpus가 커질수록 query latency와 allocation이 corpus 크기에 종속될 수 있다.
- 직렬 UDS(QI-BB-002)와 결합하면 한 번의 cold/reopen 지연이 전체 query socket의 head-of-line blocking으로 전파된다.

**보완**

1. `(repo, revision, generation, sealed digest)`를 key로 `Arc<LexicalLoadedGeneration>` read cache를 둔다.
2. 동일 key 동시 miss는 single-flight로 합친다.
3. entry count와 실제 추정 byte를 함께 제한한다. sidecar 원문/folded 복제로 인해 count-only cache는 안전하지 않다.
4. immutable sealed generation은 activation 전환만으로 invalidate하지 말고 물리 GC 직전에 pin/refcount를 확인해 제거한다.
5. cold/warm open time, hit/miss, sidecar bytes, eviction을 metric으로 남긴다.

**완료 기준**

- 같은 generation 두 번째 query에서 index open과 sidecar file read가 0회임을 fault/counting adapter로 검증한다.
- 같은 key 32개 동시 query가 한 번만 load한다.
- cache eviction 중 in-flight query와 generation GC가 안전하다.
- cold/warm p50/p95/p99와 RSS를 real-size corpus에서 측정한다.

### QI-BB-002 — UDS 처리 구조가 전체 socket을 직렬화하며 dispatch deadline/cancel이 없다

**증거**

- accept loop는 connection을 inline으로 처리한다: [server.rs](../../../crates/quanta-index-ipc/src/server.rs):217-247.
- 한 connection은 close될 때까지 여러 request를 순차 처리한다: 동일 파일 376-414.
- query/control/ingest server는 각각 thread 하나로 spawn된다: [searchd.rs](../../../crates/quanta-index-searchd/src/app/searchd.rs):20-45.
- 30초 timeout은 stream read/write에만 걸린다. `IpcDispatcher::dispatch`에는 deadline/cancellation context가 없다: [server.rs](../../../crates/quanta-index-ipc/src/server.rs):279-337, 376-414.

**도달 영향**

- slow peer가 request body를 늦게 보내거나 connection을 유지하면 같은 socket의 다음 client가 대기한다.
- long lexical/semantic dispatch가 진행되는 동안 새 query를 accept하지 못한다.
- client가 timeout/close돼도 이미 시작한 server-side query는 계속 실행한다. shutdown join도 활성 connection/dispatch 종료에 종속된다.

**보완**

1. query socket은 bounded worker pool 또는 async task로 connection을 분리한다.
2. global/per-repo in-flight cap과 bounded queue를 두고 초과 시 typed overload response를 반환한다.
3. request envelope 또는 server policy에서 absolute deadline을 만들고 dispatcher/search adapter까지 전달한다.
4. lexical collector, semantic call, projection loop에서 cancellation/deadline을 주기적으로 검사한다.
5. control/ingest는 순서 보장이 필요한 mutation 단위만 serialize하고 socket read 자체는 분리한다.

**완료 기준**

- slowloris connection, 30초짜리 query, 정상 query를 동시에 넣었을 때 정상 query가 독립 SLO 안에 끝난다.
- client disconnect 뒤 dispatch가 deadline 내 중단된다.
- queue full, timeout, cancellation이 서로 다른 typed code와 metric으로 관찰된다.

### QI-BB-003 — retention이 실제 index bytes를 관리하지 않는다

**증거**

- retention의 `encoded_len`은 search-corpus authority CBOR record의 길이다: [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):2515-2546.
- byte cap은 이 record 길이 합으로 계산된다: [search_corpus_retention.rs](../../../crates/quanta-index-search-plane/src/search_corpus_retention.rs):105-175.
- GC는 reaped authority record file만 `remove_file`한다: [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):2874-2927.
- core의 destructive port는 incomplete generation 전용이며 sealed exact identity 삭제를 거부해야 한다: [generation.rs](../../../crates/quanta-index-core/src/domains/generation.rs):70-84.
- lexical/semantic의 `remove_dir_all` 구현은 incomplete generation discard/recovery에만 있다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):3826-3875, [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):344-430.

**도달 영향**

- history record 수는 제한돼 보여도 sealed Tantivy/LanceDB generation directory는 계속 남는다.
- QI-BB-006의 full-copy generation과 결합하면 state root disk 사용량은 대략 `보존되지 않은 과거 세대 수 × corpus 크기`로 증가한다.
- `max_bytes` 이름과 실제 의미가 달라 operator가 disk safety를 잘못 판단할 수 있다.
- repo 밖의 별도 운영 청소가 있는지는 이번 감사에서 확인하지 않았다. repo 내부 lifecycle만으로는 물리 GC가 없다.

**보완**

1. sealed generation용 별도 GC port를 lexical/semantic/RepoMap에 정의한다.
2. actual recursive bytes를 측정해 admission/retention에 사용한다. authority bytes는 별도 지표로 분리한다.
3. durable GC intent → active/candidate/predecessor pin 확인 → cache fence → physical delete → parent fsync → authority receipt 완료 순서로 crash-recoverable protocol을 만든다.
4. 부분 삭제와 restart를 idempotent하게 복구한다.
5. repo/revision pair 전체 retirement와 전역 byte cap을 명시한다.

**완료 기준**

- 5개 sealed generation을 만든 뒤 cap=2를 적용하면 authority뿐 아니라 각 backend directory도 정확히 2개만 남는다.
- active/candidate/predecessor는 어떤 crash point에서도 삭제되지 않는다.
- reported retained bytes와 `du` 기준 실제 bytes가 정의된 오차 안에서 일치한다.

### QI-BB-004 — semantic lexical scope의 `scope_top_k`가 무시된다

**증거**

- contract는 scope `TextQueryRequest.top_k`를 lexical candidate cap으로 정의한다: [requests.rs](../../../crates/quanta-index-contract/src/query/requests.rs):16-35.
- SDK도 `scope_top_k`와 outer `top_k`가 별도 의미라고 명시한다: [semantic.rs](../../../crates/quanta-index-sdk/src/semantic.rs):182-196.
- dispatcher는 scope의 `top_k`를 읽지 않고 `search_all_constrained`를 호출한 뒤 전체 candidate ID를 `BTreeSet`으로 만든다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):619-679.
- lexical `search_all_constrained`는 `num_docs` 기반 full-recall limit를 사용한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):8109-8170.
- semantic filter는 ID 전체를 하나의 `embedding_id IN (...)` 문자열로 만든다: [sql.rs](../../../crates/quanta-index-semantic/src/sql.rs):23-36.
- SDK frontdoor test는 `scope_top_k(2)`를 전달하지만 결과가 non-empty인지만 검사한다: [sdk_frontdoor.rs](../../../crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs):3961-3971.

**도달 영향**

- 호출자가 요청한 lexical scope cap보다 뒤의 문서가 semantic 결과에 들어갈 수 있어 공개 API 계약을 위반한다.
- broad scope에서 corpus 전체 candidate ID, `BTreeSet`, SQL string을 생성하므로 메모리와 query planning 비용이 무제한 증가한다.

**보완**

1. scope request의 `top_k`를 validation하고 lexical bounded search에 전달한다.
2. lexical rank 기준 정확히 cap개 ID만 semantic allowlist로 넘긴다.
3. backend predicate 제한을 넘는 ID 수는 chunked query, temporary relation 또는 backend-native join으로 처리한다.
4. explanation에 requested/effective scope cap과 actual candidate count를 기록한다.

**완료 기준**

- lexical match 100개, `scope_top_k=2`, outer `top_k=10`에서 semantic 후보가 lexical 상위 2개 밖으로 나가지 않는다.
- `scope_top_k=0`, max 경계, tie, constraint contradiction을 typed/결정적으로 처리한다.

### QI-BB-005 — full recall과 projection이 무제한 corpus collect를 허용한다

**증거**

- 일반 query는 continuation 확인용 `top_k + 1`과 semantic max top-k를 적용하지만 `count:all`은 probe를 우회한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):120-142.
- lexical policy에는 query 의미 검증은 있으나 절대 top-k/candidate byte cap이 없다: [lexical service.rs](../../../crates/quanta-index-core/src/domains/lexical/service.rs):13-55.
- `full_recall_limit`은 requested/limit/`num_docs` 중 최댓값이다. repo/path/file projection과 bounded count도 full recall collect를 선택한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):4127-4162.
- Tantivy `TopDocs`에 이 collect limit가 전달된다: 동일 파일 7815-7875.
- wire body는 16 MiB 제한이 있지만 response encode가 넘치면 typed domain response 없이 connection이 닫힌다: [codec.rs](../../../crates/quanta-index-ipc/src/codec.rs):17-18, 145-160, [server.rs](../../../crates/quanta-index-ipc/src/server.rs):404-411.

**도달 영향**

- 작은 request가 모든 matching document를 heap에 materialize하고 sort/convert할 수 있다.
- projection의 전역 collapse 정확성을 위해 전체 입력이 필요할 수 있지만, 현재 방식은 aggregation까지 전체 DTO를 수집한다.
- 큰 response는 계산을 완료한 뒤 encode 단계에서 폐기될 수 있다.

**보완**

1. `max_examined_candidates`, `max_result_rows`, `max_response_bytes`, `max_wall_time`를 policy로 둔다.
2. `count:all`은 row 반환과 분리해 count collector/aggregation으로 실행한다.
3. repo/path/file projection은 streaming/grouped top-k collector로 바꾼다.
4. full result가 필요한 surface에는 pagination/cursor를 제공한다.
5. frame 초과 가능성을 dispatch 중 예측하고 typed `RESULT_TOO_LARGE`를 반환한다.

**완료 기준**

- 100만 document synthetic corpus에서 `top_k=10`, projection, `count:all` 각각의 peak RSS와 wall time이 정의된 budget 안에 든다.
- 결과가 cap/pagination으로 잘린 경우 exact/continuation semantics가 응답에 명시된다.

### QI-BB-006 — logical delta는 있지만 물리 작업은 full-copy/full-rebuild다

**증거**

- lexical delta generation 준비는 base generation directory를 recursive `fs::copy`한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):2873-2918, 3043-3092.
- text 변경 batch commit 뒤 text authority sidecar 전체를 persist한다: 동일 파일 3663-3675.
- sidecar rebuild는 Tantivy의 모든 document를 `AllQuery`로 읽고 sort하며 original/folded text/index를 다시 만든다: 동일 파일 2597-2641, 2657-2870.
- semantic도 recursive copy helper를 사용하고 기존 dataset 또는 base generation 전체를 staging으로 복사한다: [build.rs](../../../crates/quanta-index-semantic/src/build.rs):277-292, 1424-1462.
- seal은 전체 row count, coverage, semantic row commitment, membership commitment를 다시 수집하고 ANN index를 생성한다: 동일 파일 1505-1544.

**판정**

- API/정확성 관점에서는 delta와 immutable generation을 지원한다.
- 성능/IO 관점에서는 변경량에 비례하는 증분 update가 아니다.

**보완**

1. immutable generation invariant를 유지하면서 unchanged Tantivy segment/sidecar shard를 hard-link, reflink 또는 content-addressed object로 재사용한다.
2. text authority를 file/shard 단위로 분할해 changed scope만 갱신하고 manifest에서 조합한다.
3. LanceDB versioning/append-delete/index maintenance를 이용해 batch마다 전체 dataset copy를 피한다.
4. seal commitment는 Merkle/shard commitment를 조합해 changed shard만 재해시한다.
5. filesystem capability가 없을 때 full-copy fallback을 명시하고 metric으로 드러낸다.

**완료 기준**

- 100 GB corpus에서 1 MB file 수정 시 bytes read/written, elapsed time, temporary disk가 변경량에 근접하게 증가한다.
- full build와 delta build 결과의 digest/query output이 동일하다.

### QI-BB-007 — 기본 semantic 검색은 의미 임베딩이 아니다

**증거**

- 기본 embedder는 64차원 `search-owned-hash-text-v1`이다: [query_embedder.rs](../../../crates/quanta-index-search-plane/src/query_embedder.rs):4-14.
- 구현은 token을 FNV hash slot 두 곳에 누적한다: 동일 파일 81-115. learned semantic representation이 아니다.
- `QUANTA_INDEX_EMBEDDER` 미설정 시 hash profile을 선택한다: [config.rs](../../../crates/quanta-index-searchd/src/app/config.rs):344-373.
- corpus/query가 같은 provider를 쓰고 model identity를 맞추는 배선은 올바르다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):311-361.
- 의미 paraphrase real-provider E2E는 OpenAI key가 필요한 ignored test다: [end_to_end.rs](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs):1927-1942.
- checked-in relevance summary는 lexical 2건과 exact-token semantic 1건뿐이며 semantic MRR/NDCG floor가 0이고 external lexical floor도 미구성이다: [relevance summary](../../../artifacts/search-quality/relevance/latest/summary.json).
- OpenAI A/B는 2 cases뿐인 advisory capture다. paraphrase 1건에서 OpenAI top-1=1, hash top-1=0이었다: [OpenAI A/B summary](../../../artifacts/search-quality/relevance/openai-ab/latest/summary.json).

**도달 영향**

- default “semantic” route가 lexical token overlap이 약한 자연어/코드 paraphrase를 안정적으로 찾는다는 근거가 없다.
- aggregate E2E green은 배선, persistence, ranking path를 증명하지만 semantic relevance를 증명하지 않는다.

**보완**

1. 출시 profile은 learned embedding provider를 필수로 하고 hash는 `dev/test`로 명명한다.
2. offline 운영이 필요하면 버전 고정 가능한 local code/text embedding model을 제공한다.
3. model ID/version/dimension 외에 normalization/tokenization/provider revision을 manifest에 고정한다.
4. 언어/파일 종류/paraphrase/hard negative/rename/error message 등 실제 query 분포로 judged corpus를 확장한다.
5. provider 장애, 비용, rate limit 시 동작을 fail-closed 또는 명시적 lexical fallback 중 하나로 계약화한다.

**완료 기준**

- representative judged set에서 route별 MRR/NDCG/Recall과 hard-negative threshold를 통과한다.
- exact HEAD, model revision, corpus digest, config, cold/warm 조건이 artifact에 들어간다.
- real provider gate는 nightly/release rail에서 실제 실행되고 ignored 상태만으로 출시되지 않는다.

### QI-BB-008 — RepoMap query와 persistence가 snapshot 크기에 선형 이상으로 커진다

**증거**

- store가 snapshot을 read lock 아래 clone한다: [store.rs](../../../crates/quanta-index-repomap/src/store.rs):179-203.
- query engine이 entries를 다시 clone하고 전체 scan/score/sort한 뒤 included=false인 row까지 전부 반환한다: [query.rs](../../../crates/quanta-index-repomap/src/query.rs):11-111.
- `top_k=2`인데 5개 entry 전체를 응답하는 동작이 test에 고정돼 있다: [bootstrap_owner_flow.rs](../../../crates/quanta-index-repomap/tests/bootstrap_owner_flow.rs):279-318.
- 시작 시 모든 JSON을 load하고 하나라도 decode 실패하면 전체 open이 실패한다. write는 temp/fsync/rename 없이 `fs::write`한다: [persistence.rs](../../../crates/quanta-index-repomap/src/persistence.rs):140-224.

**도달 영향**

- query당 snapshot 전체 이중 clone + `O(N log N)` sort + 전체 response encode가 발생한다.
- dropped row까지 반환하므로 16 MiB frame cap에 도달하기 쉽다.
- write 중 crash로 JSON이 truncate되면 다음 boot에서 unrelated snapshot까지 포함한 RepoMap 초기화가 실패할 수 있다.
- snapshot/activation file retention도 repo 내부에서 확인되지 않았다.

**보완**

1. immutable snapshot은 `Arc`로 공유하고 query별 전체 clone을 제거한다.
2. subject/owner path/query token용 precomputed index와 bounded heap을 사용한다.
3. 기본 응답은 included row만 반환하고 dropped summary/explanation은 aggregate 또는 page로 제공한다.
4. persistence는 same-directory temp write → file fsync → rename → parent fsync로 바꾼다.
5. per-file checksum/manifest와 retention을 추가하고 corrupt artifact의 범위를 해당 generation으로 격리한다.

**완료 기준**

- snapshot size가 10배 증가해도 `top_k` 고정 query의 allocation/response bytes가 거의 고정된다.
- 모든 atomic-write crash point에서 이전 또는 새 snapshot 중 하나로 복구한다.

### QI-BB-009 — embedding cache와 telemetry sample이 무제한 증가한다

**증거**

- persistent cache는 SHA-256 prefix로 256개 shard directory를 만들고 text당 `.vec` file 하나를 저장한다: [cache.rs](../../../crates/quanta-index-embed/src/cache.rs):169-240.
- entry/byte/age cap, eviction, compaction은 없다. in-memory fallback도 unbounded `BTreeMap`이다: 동일 파일 146-167.
- OpenAI profile의 disk cache는 기본 활성화되고 state root의 `embed-cache`에 연결된다: [config.rs](../../../crates/quanta-index-searchd/src/app/config.rs):39-48, [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):347-354.
- OpenAI telemetry는 모든 outbound request sample을 process-lifetime `Vec`에 push하며 자동 cap이 없다: [telemetry.rs](../../../crates/quanta-index-embed/src/telemetry.rs):28-37, 69-77.

**도달 영향**

- 다양한 corpus/model/text가 누적되면 disk bytes와 inode가 계속 증가한다.
- long-lived OpenAI process는 request 수에 비례해 telemetry memory가 증가한다.
- cache write가 direct best-effort write라 partial vector는 miss로 복구되지만 stale/partial file 자체는 정리되지 않는다.

**보완**

1. model namespace별 max bytes/entries/age와 global cap을 둔다.
2. access metadata를 별도 manifest/embedded DB로 관리하고 background incremental eviction을 수행한다.
3. temp+rename과 length/checksum으로 partial file을 구분한다.
4. request sample은 bounded ring buffer 또는 histogram으로 바꾸고 counters만 누적한다.
5. hit rate, bytes, entries, eviction, corrupt miss를 export한다.

**완료 기준**

- cap을 넘는 workload/모델 변경/강제 종료/restart에서 disk와 RSS가 제한 안에 유지된다.
- eviction이 active request의 valid cache file을 깨지 않는다.

### QI-BB-010 — 현재 performance proof가 release 판단에 부족하다

**증거**

- `scan_vs_index`는 `adapter.build` 직후 sealed identity 없이 `open`한다: [main.rs](../../../crates/quanta-index-scan-experiment/src/main.rs):242-265.
- 현재 HEAD에서 작은 probe도 `GENERATION_IDENTITY_INCOMPLETE`로 실패했다.
- 이 실험은 searcher를 sample loop 전에 한 번만 열어 실제 daemon query의 QI-BB-001 reopen 비용을 측정하지 않는다.
- checked-in [scan-vs-index.md](../../../artifacts/experiments/scan-vs-index.md)는 git SHA가 없고 현재 harness로 재생할 수 없다. 과거 50.4 MB/100,000 chunks build는 79.7초, query p95는 0.372 ms였지만 현재 성능 증거로 쓸 수 없다.
- warm DSL artifact는 HEAD보다 102 commits, cold는 105 commits 뒤다: [warm](../../../artifacts/dsl-bench/warm-matrix.json), [cold](../../../artifacts/dsl-bench/cold-matrix.json).
- scale/tail artifact는 HEAD보다 90 commits 뒤다: [scale](../../../artifacts/search-quality/scale/latest/summary.json), [tail](../../../artifacts/search-quality/tail/latest/summary.json).
- local scale rail은 16 files인 small tier만 측정하고 medium/large/xlarge는 Linux perf runner 소유의 advisory 선언이다: [scale.rs](../../../crates/quanta-index-searchd-harness/src/scale.rs):95-145.
- `open_ms`라는 필드는 index open이 아니라 activation 시간을 잰다: 동일 파일 346-370.

**도달 영향**

- 현재 HEAD의 ingest throughput, cold/warm latency, concurrent throughput, RSS, disk amplification, GC를 수치로 판정할 수 없다.
- in-process searcher-only 수치를 daemon end-to-end SLO로 오해할 위험이 있다.

**보완**

1. scan experiment가 generation을 정상 seal하고 exact digest/SHA/config를 artifact에 기록하게 고친다.
2. adapter-only, daemon warm, daemon cold, IPC, open, planning, execution을 별도 timing으로 분해한다.
3. real-size tier에서 full build와 1-file delta의 bytes read/write, temporary disk, CPU, RSS를 측정한다.
4. 1/8/32 concurrent clients, slow client, mixed lexical/semantic/hybrid, count/projection worst case를 측정한다.
5. controlled idle host에서 exact-head A/B를 수행하고 stale artifact는 release gate에서 거부한다.

**완료 기준**

- current HEAD와 corpus/config digest가 일치하는 Linux artifact가 존재한다.
- p50/p95/p99, QPS, error/timeout, peak RSS, disk amplification, build/update/GC 시간이 모두 포함된다.
- benchmark command가 clean checkout에서 재현 가능하다.

### QI-BB-011 — text sidecar 정규화가 Tantivy/query 언어와 완전히 통일되지 않았다

**증거**

- phrase position 경로는 명시적 TODO와 함께 `split_whitespace` + `to_ascii_lowercase`를 사용한다: [phrase.rs](../../../crates/quanta-index-lexical/src/phrase.rs):209-224.
- Tantivy case-sensitive tokenizer는 `SimpleTokenizer`다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):2281-2288.
- text authority folded copy와 raw/substring helpers도 ASCII lowercase를 사용한다: 동일 파일 2637, 4277-4309.
- token matching 일부는 ASCII alphanumeric/underscore 경계에 의존한다: 동일 파일 4299-4309.

**도달 영향**

- build/query sidecar는 내부적으로 대체로 self-consistent하지만 punctuation, combining mark, Unicode case folding, CJK 등의 의미가 Tantivy token 경로와 달라질 수 있다.
- 동일 DSL이라도 leaf 종류에 따라 token boundary/case-insensitive 결과가 달라질 수 있다.

**보완**

1. shared normalizer에 Unicode normalization, case folding, token boundary 정책을 하나로 정의한다.
2. Tantivy analyzer와 sidecar phrase/trigram/raw verification이 같은 token stream 또는 명시적으로 다른 계약을 사용하게 한다.
3. exact/raw/regex처럼 byte semantics가 필요한 surface는 Unicode text semantics와 분리해 문서화한다.

**완료 기준**

- punctuation, snake/camel case, accented Latin, composed/decomposed Unicode, CJK, emoji 경계 corpus에서 leaf별 기대 결과를 golden test로 고정한다.

### QI-BB-012 — README가 현재 구현과 다르다

**2026-09-16 doc sync:** root `README.md`, `docs/ssot/*`, `may-28-lancedb-adoption`,
`may-25-search-owned-semantic-derivation`, and broken plan links were refreshed
to match current code. Remaining gap: no automated doc-truth checker yet; stale
plan prose may still exist outside the touched files.

**원래 증거 (4914156 기준)**

- [README.md](../../../README.md):25는 lexical adapter에 reader caching이 있다고 쓰지만 QI-BB-001 경로에는 없다.
- 동일 문서 31은 `tokio current-thread UDS listener`라고 쓰지만 실제 listener는 blocking std UDS + thread다.
- current status/verification snapshot이 2026-07-15 기준이다: 동일 문서 14, 49.
- production observability가 없다는 문구는 현재도 대체로 맞지만 process-local bounded samples와 embedding counters가 생겨 상태를 더 정확히 써야 한다: 동일 문서 162.
- `just lint-doc-paths`는 현재 HEAD의 기존 문서 링크 2개 때문에 실패한다: `docs/plans/jun-2-dsl-final-cut/README.md:149`, `docs/plans/jun-2-dsl-final-cut/tickets/HISTORICAL-MAP.md:9`. 이번 findings의 local link target은 별도 검사에서 모두 존재했다.

**보완 및 완료 기준**

- 구현 사실을 기준으로 README를 갱신하고, 지원/비지원/실험/advisory/release-gated 상태를 구분한다.
- architecture claim에 source-backed check 또는 doc test를 추가한다.
- 성능 표에는 SHA, corpus/config digest, 측정 계층, host, cold/warm, concurrency를 필수로 둔다.

### QI-BB-013 — 핵심 모듈의 책임 집중도가 높다

**증거**

- `quanta-index-lexical/src/lib.rs`: 8,311 lines.
- `query_dispatcher.rs`: 9,799 lines.
- `readiness.rs`: 7,127 lines.
- `semantic/build.rs`: 3,255 lines.
- `sdk_frontdoor.rs`: 4,415 lines, `end_to_end.rs`: 3,649 lines.
- workspace는 `too_many_lines = "allow"`다: [Cargo.toml](../../../Cargo.toml):100.

**영향**

- schema/storage/planning/projection/lifecycle가 같은 compilation/change surface에 있어 review와 병렬 수정의 충돌 범위가 크다.
- QI-BB-004와 QI-BB-012처럼 contract와 실행/문서가 어긋날 때 영향 경로를 찾기 어렵다.

**보완**

- public API를 유지하면서 lexical open/build/sidecar/query/projection, dispatcher route family, authority persistence/retention을 책임별 module로 분리한다.
- 파일 길이 자체보다 dependency direction과 state owner를 먼저 고정한다.
- 분리 전후 compile/link/test 시간을 측정해 crate 과분할은 피한다.

### QI-BB-014 — UDS 접근 제어와 socket ownership이 배포 환경에 암묵적으로 의존한다

**증거**

- bind는 parent `create_dir_all`과 `UnixListener::bind`만 수행하고 mode/owner를 검증하거나 설정하지 않는다: [server.rs](../../../crates/quanta-index-ipc/src/server.rs):170-200.
- 같은 bind는 기존 path가 socket이면 live listener인지 확인하지 않고 즉시 unlink한다: 동일 파일 170-192.
- state-root lock file은 Unix에서 `0600`/`NOFOLLOW`로 연다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):150-167. 그러나 state-root/socket directory mode는 OS umask에 맡긴다.
- state-root lease는 같은 state root의 두 번째 daemon은 막지만 socket override는 별도 path ownership lease가 없다: 동일 파일 102-130, 224-240. 서로 다른 state root가 같은 override path를 쓰면 뒤 daemon이 앞 daemon의 live socket path를 제거할 수 있다.
- dispatcher는 peer credential을 받지 않는다. query/control/ingest authorization도 filesystem write 권한 외에는 없다.

**영향**

- 기본 single-user private state root에서는 문제가 없을 수 있다.
- permissive umask, 공유 group directory, 잘못 provision된 state root에서는 다른 local process가 query뿐 아니라 activation/ingest socket에 접근할 수 있다.
- socket path가 충돌하면 먼저 실행 중인 listener는 열린 FD를 유지해도 새 client가 접근할 pathname을 잃는다. 뒤 daemon은 같은 path를 차지할 수 있어 가용성과 endpoint identity가 바뀐다.
- 현재 증거만으로 원격 취약점이라고 볼 수는 없다. local deployment hardening gap이다.

**보완 및 완료 기준**

- private mode라면 state/socket directory `0700`, socket `0600`, expected owner, non-symlink parent를 시작 시 강제/검증한다.
- shared mode가 필요하면 group/ACL과 query/control/ingest 권한을 분리하고 Linux/macOS peer credential 검증을 추가한다.
- 기존 socket에 connect/protocol ownership probe를 수행해 live endpoint면 `SOCKET_IN_USE`로 거부하고, stale임이 증명된 경우에만 inode identity를 잡고 제거한다. socket path별 lease도 허용 가능하다.
- permissive umask, pre-created wrong-owner/mode directory, live socket 충돌, stale socket 복구를 각각 fail-closed test로 검증한다.

### QI-BB-015 — 관측성은 test/harness 수준이며 일부 진단 collection은 무제한이다

**증거**

- query samples는 process-local `BoundedQueryObsStore`의 최대 4,096개 ring으로 보관된다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):213-255.
- 같은 store의 `errors: Vec<ObsError>`는 cap 없이 push한다: 동일 파일 216-235.
- embedding telemetry는 cross-process completeness를 보장하지 않는 local diagnostic이라고 명시하고 request sample은 unbounded `Vec`다: [telemetry.rs](../../../crates/quanta-index-embed/src/telemetry.rs):11-37, 69-77.
- daemon은 store를 dispatcher에 연결하지만 production exporter/endpoint는 없다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):850-865.

**도달 영향**

- queue saturation, lexical cold open, GC bytes, delta write amplification, cache hit, timeout/cancel 원인을 운영 환경에서 상관 분석하기 어렵다.
- 반복 cardinality error 또는 OpenAI request가 장기 실행 RSS를 지속 증가시킬 수 있다.

**보완**

1. errors/request samples를 bounded ring/histogram으로 바꾼다.
2. Prometheus/OpenTelemetry 또는 구조화된 scrape endpoint 중 하나를 제공한다.
3. 필수 metric: route latency, queue/in-flight, open hit/miss/time/bytes, examined candidates, response bytes, ingest stage, bytes copied, generation disk bytes, GC, provider/cache/retry, timeout/cancel/overload.
4. repo/query text처럼 cardinality·민감도가 큰 label은 넣지 않는다.

### QI-BB-016 — lexical writer memory budget이 process capacity와 연동되지 않는다

**증거**

- lexical writer cache는 최대 16 generation, writer당 15 MB budget의 고정값을 사용한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):104-148.
- writer budget만 단순 합산해도 약 240 MB이며 mmap, loaded sidecar, semantic cache, query allocations는 별도다.

**보완 및 완료 기준**

- writer/cache/concurrency budget을 하나의 process memory envelope에서 계산하고 config로 조정한다.
- active writer 수, allocated writer budget, RSS pressure를 관찰하고 idle writer를 안전하게 commit/evict한다.
- lexical read cache(QI-BB-001)를 추가하기 전에 byte-weighted global capacity test를 만든다.

### QI-BB-017 — semantic generation open이 전체 integrity scan을 반복한다

**증거**

- `open_generation`은 marker/manifest/schema/row count를 확인한 뒤 main table의 `semantic_row_commitment_v1`을 매번 다시 계산한다: [search.rs](../../../crates/quanta-index-semantic/src/search.rs):225-340.
- 같은 open에서 cluster-membership table도 열고 commitment를 다시 검증한다: 동일 파일 342-383.
- membership 검증은 committed row 수만큼 `Vec`을 예약하고 모든 문자열 field를 복제한 뒤 root를 계산한다: 동일 파일 425-517.
- `validate_generation_identity`는 stale cache를 신뢰하지 않기 위해 query cache를 우회하고 `open_generation`을 호출한다: [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):244-340.
- sealed ingest 직후 physical validation, activation/rollback, restart rehydrate가 이 validator를 호출한다: [ingest_dispatcher.rs](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs):269-280, [search_corpus_lifecycle.rs](../../../crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs):176-190, 239-291.
- daemon boot의 semantic scanner는 state root의 모든 sealed generation을 순회해 `open_generation`을 실행한다: [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):537-659. scanner 결과를 ledger에 넣은 뒤 active generation은 lifecycle rehydrate에서 다시 validate한다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):783-806, [semantic_boot.rs](../../../crates/quanta-index-searchd/src/app/semantic_boot.rs):99-145.
- validation 결과는 query open cache에 넣지 않는다. 따라서 activation 뒤 첫 query 또는 8-entry cache miss가 동일 generation을 다시 full-scan한다: [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):76-125, 180-240.

**도달 영향**

- seal 이후의 activation latency, daemon boot latency, 첫 semantic/hybrid query latency가 semantic row 수와 membership row 수에 선형으로 증가한다.
- 정상 seal → activation → first query 흐름만으로 같은 generation을 세 번 연속 full scan할 수 있고, restart/rollback/cache eviction 때 다시 반복된다.
- boot 비용은 active generation 하나가 아니라 `O(모든 남은 sealed generation의 semantic rows)`다. 물리 retention이 generation directory를 제거하지 않는 QI-BB-003과 결합해 restart 시간이 장기적으로 계속 증가할 수 있다.
- membership은 streaming hash가 아니라 전체 row DTO를 메모리에 모으므로 큰 cluster corpus에서 startup RSS도 증가한다.
- 직렬 UDS(QI-BB-002)와 결합하면 cache miss 하나가 같은 socket의 모든 query를 막는다.

**보완**

1. seal-time full integrity proof와 open-time identity/schema proof를 분리한다.
2. row commitment를 shard/Merkle root로 저장하고 open에서는 immutable file/version manifest와 root만 검증한다.
3. full scrub은 seal 단계에서 한 번 수행하거나 background/offline 검증으로 이동하고 주기/결과를 metric으로 남긴다.
4. activation에서 full 검증이 필수라면 검증된 exact-generation handle/proof를 query cache에 안전하게 승격한다.
5. membership commitment는 최소한 streaming accumulator로 계산해 전체 row `Vec<String>` materialization을 제거한다.
6. boot inventory는 manifest/marker의 cheap metadata만 읽고, active/candidate/predecessor처럼 필요한 generation만 동기 검증한다. 나머지는 bounded background scrub과 QI-BB-026 quarantine 경로로 보낸다.

**완료 기준**

- seal, activation, restart, first query 각각의 rows read를 계측해 seal 이후 동일 generation full scan이 0회 또는 정책상 정확히 1회임을 증명한다.
- 비활성 generation 수와 무관하게 active generation boot budget이 유지된다.
- 1천만 semantic row에서 activation/boot/first-query budget과 peak RSS를 충족한다.
- marker/manifest/table/version corruption 각 rail이 cheap open 또는 background scrub 중 정의된 시점에 fail-closed한다.

### QI-BB-018 — 일반 hybrid는 dense recall이 아니라 lexical-scoped rerank다

**증거**

- hybrid는 lexical search를 먼저 실행하고 그 결과 ID 집합만 `search_scoped_constrained`에 넘긴다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):531-610.
- explanation builder도 semantic lane이 lexical universe에 제한되어 semantic-only hit를 절대 만들 수 없다고 명시한다: [semantic_query.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs):800-856.
- runtime test는 lexical outsider가 semantic query와 더 잘 맞아도 hybrid 결과에서 제외되는 동작을 고정한다: [end_to_end.rs](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs):2631-2705.
- 반면 HybridSeed v2는 별도 dense lane을 실행해 dense-only entity를 허용한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):778-831.

**도달 영향**

- lexical overlap이 없는 paraphrase, API 개념 질의, 동의어 질의의 relevant document는 dense score가 높아도 일반 hybrid에 들어올 수 없다.
- 결과는 RRF 형식이지만 candidate universe는 lexical recall과 동일하다. 기능적으로는 `BM25 recall + dense rerank`다.
- 이 의미를 일반적인 `hybrid search`로 노출하면 사용자와 relevance benchmark가 recall 확대를 기대할 수 있다.

**보완**

1. 현재 동작을 유지한다면 surface를 `lexical_scoped_rerank`처럼 명시하고 일반 hybrid와 구분한다.
2. true hybrid는 독립적으로 bounded BM25 lane과 dense lane을 실행한 뒤 union에 RRF를 적용한다.
3. lexical constraints와 authorization은 두 lane에 동일하게 push down하고 post-filter로만 처리하지 않는다.
4. zero-token-overlap paraphrase, lexical-only, dense-only, tie를 포함한 judged corpus로 recall/NDCG/latency를 함께 측정한다.

**완료 기준**

- true-hybrid surface에서는 semantic-only relevant hit가 top-k에 들어오는 E2E가 통과한다.
- scoped rerank를 별도 유지한다면 API/SDK/CLI 이름, explanation, 문서가 lexical universe 제한을 동일하게 표현한다.

### QI-BB-019 — HybridSeed legacy/v2 병행 경로가 vector work와 결과 계약을 중복한다

**증거**

- HybridSeed는 lexical-scoped semantic search로 legacy `seed_candidates`를 먼저 만든다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):721-777.
- 바로 이어서 global 또는 corpus별 semantic search를 다시 실행해 v2 lane과 `seed_candidates_v2`를 만든다: 동일 파일 778-831.
- response는 legacy와 v2 candidate list를 모두 직렬화한다: [query_responses.rs](../../../crates/quanta-index-contract/src/results/query_responses.rs):428-445, 1588-1606.
- wire decode와 `window.returned` 검증은 v2가 있으면 v2 길이를 active result로 사용한다: 동일 파일 1680-1691.
- dispatcher의 `lq_merge_result_count`는 active v2가 아니라 legacy `seed_candidates.len()`을 기록한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):1255-1273.
- request contract는 empty `dense_corpora`를 SCV2 migration window의 legacy global dense lane이라고 명시한다: [requests.rs](../../../crates/quanta-index-contract/src/query/requests.rs):386-395.

**도달 영향**

- 한 요청이 동일 query vector로 scoped dense search와 독립 dense search를 모두 수행한다.
- payload와 SDK 해석 surface가 이중화되고 legacy/v2 후보 수가 다를 수 있다.
- merge-count metric이 실제 active result/window와 달라 dashboard/SLO 판단을 왜곡한다.
- migration 종료 조건이나 제거 시점이 코드에서 강제되지 않아 compat 경로가 상시 비용으로 남을 수 있다.

**보완**

1. v2를 canonical response로 확정하고 intentional contract baseline change로 legacy field/path를 제거한다.
2. 임시 호환이 필요하면 독립 query를 두 번 실행하지 말고 v2 결과에서 정의된 lossless/best-effort legacy projection을 만든다.
3. metric은 `window.returned()` 또는 active candidate list 길이만 사용한다.
4. compat 사용량, sunset version/date, 제거 gate를 machine-readable하게 둔다.

**완료 기준**

- mock semantic adapter가 요청 corpus lane당 정확히 한 번만 호출됨을 검증한다.
- response의 canonical candidate list가 하나이고 metric/window/serialized length가 동일하다.
- migration rail이 명시된 종료 version에서 legacy field 부재를 검증한다.

### QI-BB-020 — auxiliary authority가 전역 lock과 whole-snapshot persistence에 묶여 있다

**증거**

- history/runtime/structural authority는 모든 repo/revision/generation을 한 `Arc<RwLock<Ledger>>` 안의 `BTreeMap`으로 보관한다: [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):255-268, 1503-1847.
- history와 runtime query는 global read guard를 잡은 채 filter evaluation과 `top_k` scan을 수행한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):863-946, 3241-3306.
- history/runtime/structural ingest는 global write guard 아래 state를 변경하고 durable persistence까지 수행한다: [ingest_dispatcher.rs](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs):713-868.
- activation/rollback은 global ledger read guard를 physical pair validation과 durable CAS가 끝날 때까지 의도적으로 유지한다. QI-BB-017의 semantic full scan도 이 guard 안에서 실행된다: [search_corpus_lifecycle.rs](../../../crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs):239-291.
- `persist_from_ledger`는 mutation 종류와 무관하게 history, runtime, structural 전체 map을 각각 clone하고 세 CBOR file을 모두 다시 쓴다: [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):2930-2955.
- search-corpus retention receipt는 sealed lexical/semantic identity만 prune한다. history/runtime/structural generation map은 제거하지 않는다: 동일 파일 423-469.
- state는 persistence 전에 in-memory ledger에 적용된다. 세 file write도 개별 atomic replace일 뿐 하나의 transaction이 아니어서, 앞 file이 durable한 뒤 뒤 file이 실패하면 caller는 error를 받지만 memory와 일부 disk state는 이미 변경돼 있다.

**도달 영향**

- 작은 dirty/history mutation 하나가 전체 auxiliary corpus clone + encode + fsync를 유발한다.
- 큰 history query 또는 pure-negative/runtime scan이 다른 repo의 ingest/activation write lock을 막는다. 반대로 persistence 중 모든 auxiliary query가 막힌다.
- 오래된 generation이 memory와 CBOR snapshot에 계속 남아 RSS, restart decode time, write amplification이 지속 증가한다.
- partial durable success 뒤 실패 응답은 retry/receipt 의미를 모호하게 만든다.

**보완**

1. authority를 domain과 `(repo, revision, generation)`별 immutable `Arc` snapshot으로 shard하고 lock 아래에서는 handle만 clone한다.
2. query scan/text matching과 disk I/O는 global lock 밖에서 실행한다.
3. 전체 snapshot file 대신 per-key WAL/manifest, embedded DB, LSM 등 changed-key 비례 persistence를 사용한다.
4. search-corpus retention과 함께 auxiliary generation을 transactionally prune하고 active reader pin을 존중한다.
5. mutation receipt에 durable sequence/digest를 포함하고 partial write/restart/retry를 idempotent하게 복구한다.

**완료 기준**

- 한 repo의 100만-row worst-case history query 중 다른 repo ingest latency가 독립 budget을 지킨다.
- 1-row mutation의 cloned/encoded/written bytes가 전체 state가 아니라 변경량에 비례한다.
- generation cap 적용 뒤 in-memory map, CBOR/DB bytes, restart time이 함께 제한된다.
- 각 fsync/rename failure point에서 성공 receipt와 durable state가 일치한다.

### QI-BB-021 — semantic ingest가 배치 전체 embedding을 동시에 보유한다

**증거**

- legacy chunk와 typed semantic-source derivation 모두 배치의 모든 text를 모아 한 번에 `embed_batch`하고 모든 vector를 반환받은 뒤 scope별 DTO로 재분배한다: [semantic_derive.rs](../../../crates/quanta-index-search-plane/src/semantic_derive.rs):122-221, 301-355.
- OpenAI provider의 request count/token batching은 network request만 나눈다. 최종 결과는 다시 전체 `Vec<Vec<f32>>`로 flatten한다: [openai.rs](../../../crates/quanta-index-embed/src/openai.rs):390-437.
- semantic Lance batch 생성은 각 metadata string을 별도 vector에 복제하고 모든 embedding을 하나의 `flat_vectors`에 다시 복사한다: [build.rs](../../../crates/quanta-index-semantic/src/build.rs):859-948.
- `SearchCorpusIngestBatch::validate_surface_mutations_v1`은 mutation 충돌/정렬만 검사하며 record/text/vector byte budget을 검사하지 않는다: [ingest.rs](../../../crates/quanta-index-contract/src/ipc/ingest.rs):724-865.
- IPC 16 MiB frame cap은 입력 wire bytes만 제한한다. `QUANTA_INDEX_EMBED_DIM` parser에는 상한이 없고 zero는 later derivation에서만 거부되며, concurrency에도 최대 상한이 없다: [codec.rs](../../../crates/quanta-index-ipc/src/codec.rs):17-18, [config.rs](../../../crates/quanta-index-searchd/src/app/config.rs):436-460.

**도달 영향**

- 많은 짧은 record 또는 큰 dimension은 작은 serialized input을 수백 MB의 `f32` vector와 metadata 복제로 확장할 수 있다.
- OpenAI concurrency를 높이면 in-flight HTTP 결과와 aggregate vector가 겹쳐 peak RSS가 더 커진다.
- ingest socket은 직렬이지만 process OOM은 query/control availability까지 같이 잃게 한다.

**보완**

1. max records, input text bytes, output vector bytes, dimension, embedding concurrency를 하나의 ingest resource policy로 검증한다.
2. bounded chunk 단위로 embed → validate → Lance append하고 전체 `all_vectors`를 보유하지 않는다.
3. metadata column builder가 가능하면 borrowed/dictionary encoding을 사용하고 불필요한 string 복제를 줄인다.
4. stage별 queued texts/vector bytes/in-flight requests/peak batch bytes를 metric으로 남기고 backpressure한다.

**완료 기준**

- 최대 허용 batch와 dimension에서 peak RSS가 선언한 process envelope 안에 든다.
- 상한을 넘는 dimension/concurrency/record bytes는 startup 또는 ingest 전에 typed error로 거부된다.
- chunked와 기존 full derivation의 vector ordering/digest/query 결과가 동일하다.

### QI-BB-022 — explain은 ranking explanation이 아니라 불완전한 presence probe다

**증거**

- explain request는 generation과 이미 반환된 candidate만 받고 원 질의, lowered plan, ranker provenance를 받지 않는다: [requests.rs](../../../crates/quanta-index-contract/src/query/requests.rs):840-852.
- 실행은 candidate snippet 또는 candidate ID로 새 probe query를 만들고 기본 top-50 결과에 candidate가 있는지만 검사한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):962-1012.
- response의 `contributions`는 비어 있고 `ranker_weights_hash`는 zero이며 strategy는 `presence_probe`다: 동일 파일 1013-1033.
- direct candidate-ID lookup이 아니라 top-50 재검색이므로 실제 index에 있는 candidate도 probe 결과 상위 50개 밖이면 `NOT present`로 보고할 수 있다.
- 현재 search-product-quality RFC도 explanation fidelity를 미완성 영역으로 두고 synthetic post-hoc text가 아니라 실제 planner/runtime provenance를 요구한다: [rfc.md](../../../docs/plans/jun-7-search-product-quality/rfc.md):64-86, 221-249.

**도달 영향**

- 사용자는 원 질의에서 왜 이 결과가 이 순위인지, 어떤 leaf/filter/boost/engine이 기여했는지 알 수 없다.
- stale membership 확인과 ranking explanation이 한 `explain` 이름에 섞여 API 의미가 모호하다.
- UI rail은 contribution이 0이어도 `presence_probe` tag와 non-empty summary만으로 통과한다.

**보완**

1. 검색 응답에 immutable query/plan/execution provenance ID를 넣고 explain은 해당 실행 기록을 조회한다.
2. BM25/boost/filter/projection/hybrid RRF의 실제 contribution과 raw/final rank를 typed section으로 제공한다.
3. candidate 생존 확인은 exact candidate-ID lookup 전용 API/section으로 분리하고 top-k 재검색을 사용하지 않는다.
4. provenance retention/size/privacy budget을 명시한다.

**완료 기준**

- score/boost/engine input을 바꾸면 예상 contribution과 최종 rank가 함께 변하는 golden test가 통과한다.
- 설명 contribution 합성 규칙이 실제 emitted score와 정의된 오차 안에서 일치한다.
- indexed candidate presence는 주변 corpus 크기와 무관하게 정확히 판정된다.

### QI-BB-023 — history top-k가 relevance나 시간 순서가 아니라 SHA 순서다

**증거**

- commit authority는 `BTreeMap<CommitSha, CommitRecord>`, diff authority는 `BTreeMap<HistoryDiffKey, ...>`다: [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):1503-1548, 1573-1588.
- `CommitSha`는 raw 20-byte lexicographic `Ord`를 derive한다: [history.rs](../../../crates/quanta-index-contract/src/lex/history.rs):51-58.
- executor는 map 순서로 match를 scan하다 `top_k`개가 차면 즉시 멈추며 score/recency sort를 하지 않는다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):3241-3278.
- current `CommitCandidate`에는 score가 없고 시간은 output field일 뿐 selection/order에 쓰이지 않는다: [commit_candidate.rs](../../../crates/quanta-index-contract/src/results/commit_candidate.rs):10-30.

**도달 영향**

- `type:commit fix top_k=N`은 가장 관련 있거나 최신 N개가 아니라 SHA byte가 작은 matching N개를 반환한다.
- 새 commit의 SHA가 뒤쪽이면 매우 relevant해도 작은 top-k에서 영구히 보이지 않을 수 있다.
- diff는 `(commit_sha, file_path)` key 순서로 잘려 동일한 문제가 있다. pagination/cursor도 없어 뒤 결과에 접근할 방법이 없다.

**보완**

1. commit message/author/committer와 diff text를 실제 indexed retrieval 대상으로 만들고 score를 contract에 포함한다.
2. 최소 기준을 `(score DESC, committer_time DESC, sha ASC, path ASC)` 같은 total order로 고정한다.
3. time-only/filter-only query의 기본 order와 tie-break를 명시한다.
4. stable cursor pagination과 examined/matched count를 제공한다.

**완료 기준**

- SHA 순서와 relevance/시간 순서를 의도적으로 반대로 만든 fixture에서 top-k가 계약 순서를 따른다.
- ingest 순서/restart와 무관하게 같은 결과/next cursor가 나온다.
- large non-match와 selective match의 latency/RSS budget을 함께 충족한다.

### QI-BB-024 — regex cache가 corpus 크기 기준으로 bounded되지 않는다

**증거**

- regex cache는 최대 128 entry만 제한하고 각 value는 matching candidate ID 전체 `BTreeSet<String>`이다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):368-420.
- timeout 없는 일반 regex는 trigram prefilter/verification 결과 전체를 cache한다. usable literal이 없으면 모든 text-authority doc을 후보로 만든다: 동일 파일 5314-5419.
- cache hit의 `get`은 전체 `BTreeSet<String>`을 clone하고 insert도 계산 결과를 clone해 보관한다: 동일 파일 395-414, 5414-5419.

**도달 영향**

- broad regex 128개가 각각 N개 document를 match하면 cache가 최대 `128 × N` candidate-ID 문자열을 소유할 수 있다.
- hit도 O(matches) clone allocation을 수행하므로 cache가 latency를 줄이면서 RSS/allocation을 크게 늘릴 수 있다.
- entry count는 bounded지만 byte/candidate count가 없어 QI-BB-016의 process memory envelope를 우회한다.

**보완**

1. cache를 byte-weighted LRU로 바꾸고 process-global read/write cache budget에 포함한다.
2. candidate string 대신 generation-local compact doc ID bitmap/roaring set을 `Arc`로 공유한다.
3. 최대 cached match count를 넘는 broad regex는 cache하지 않고 reason metric을 남긴다.
4. cache hit/miss/entry bytes/match cardinality/clone bytes를 관찰한다.

**완료 기준**

- 100만 document를 match하는 128개 regex를 반복해도 cache/RSS가 선언된 byte cap을 넘지 않는다.
- cache hit가 match set 전체 deep clone을 하지 않는다.
- eviction과 generation GC가 in-flight regex query를 깨지 않는다.

### QI-BB-025 — `top_k`의 public 허용 범위와 실제 route 동작이 일치하지 않는다

**증거**

- semantic 정책은 `1..=10_000`을 유효 범위로 선언하고 경계값 10,000을 받아들이는 unit test까지 둔다: [service.rs](../../../crates/quanta-index-core/src/domains/semantic/service.rs):9-34, 122-130.
- hybrid 정책도 같은 10,000 경계값을 허용한다: [hybrid/service.rs](../../../crates/quanta-index-core/src/domains/hybrid/service.rs):14-26, 198-202.
- dispatcher의 continuation probe는 `top_k + 1 <= 10_000`을 요구해 10,000을 거부한다. 같은 파일의 test도 9,999 성공과 10,000 실패를 고정한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):120-130, 4762-4786.
- semantic은 정책 검증을 통과한 뒤 이 probe에서 실패하고, hybrid/hybrid-seed도 같은 probe를 사용한다: 동일 파일 162-164, 619-679, 702-742.
- 공용 text request와 SDK builder는 `top_k`가 존재하는지만 확인하고 0/상한을 검증하지 않는다: [requests.rs](../../../crates/quanta-index-contract-base/src/query/requests.rs):18-29, [text_query_builder.rs](../../../crates/quanta-index-sdk/src/text_query_builder.rs):32-50.
- `top_k=0`은 semantic/hybrid에서는 거부되지만 lexical/symbol은 한 건을 probe한 뒤 0건으로 자르고, history/runtime은 첫 match를 push한 후 `len >= 0`에서 break해 **1건을 반환**한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):405-498, 863-955, 3241-3306, 4013-4015.

**도달 영향**

- 정책과 test가 보장한다고 말하는 정상 최대값이 semantic/hybrid/일반 lexical probe 경로에서 실행되지 않는다.
- 동일 wire field의 0이 route에 따라 typed error, empty page, 1-row response로 갈린다. SDK type-state는 이 값을 막지 못한다.
- boundary behavior가 여러 helper에 분산돼 새 route가 어느 의미를 따라야 하는지 결정적 authority가 없다.

**보완**

1. 모든 query route가 공유하는 `TopKPolicyV1`을 contract/core 한 곳에 두고 wire decode, SDK build, dispatcher, adapter가 같은 validator를 사용하게 한다.
2. 10,000을 실제 지원한다면 continuation row를 위한 internal fetch ceiling을 10,001 이상으로 분리한다. 지원 ceiling이 9,999라면 public 정책과 문서를 그 값으로 내리고 intentional contract change로 처리한다.
3. 0은 모든 route에서 동일 typed error로 거부한다.
4. result window가 없는 history/runtime/structural response에도 bounded window/cursor 계약을 추가한다.

**완료 기준**

- lexical, symbol, semantic, hybrid, hybrid-seed, history, runtime, structural에 대해 `{0, 1, 9_999, 10_000, 10_001, u32::MAX}` truth table E2E가 동일 정책을 통과한다.
- SDK와 raw IPC가 같은 error code를 반환한다.
- 허용된 최대값에서 `returned <= top_k`와 `has_more/candidate_count` invariant가 유지된다.

### QI-BB-026 — 비활성 과거 generation의 손상이 daemon 전체 boot를 막는다

**증거**

- lexical scanner는 state root의 모든 generation directory를 순회한다. non-canonical directory, scope mismatch, sealed identity/open 검증 실패 하나가 전체 scan error가 된다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):3898-3996.
- scanner가 이미 `validate_generation_identity`를 호출한 뒤 boot seed가 반환된 모든 lexical candidate를 다시 validate한다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):929-982.
- semantic scanner도 모든 sealed directory의 manifest를 읽고 `open_generation`으로 row/membership commitment까지 검증하며 하나의 오류를 전체 scan 실패로 반환한다: [semantic/lib.rs](../../../crates/quanta-index-semantic/src/lib.rs):537-659.
- 이 scan들은 query/control/ingest socket bind 전에 실행된다. 이후 active generation은 lifecycle rehydrate에서 다시 검증된다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):778-806, 890-913.
- scanner는 activation catalog를 기준으로 active/candidate/predecessor와 그 밖의 오래된 generation을 구분하지 않는다.

**도달 영향**

- 현재 active generation이 완전해도 사용하지 않는 과거 generation 하나의 손상이나 legacy directory 하나 때문에 서비스가 시작되지 않는다.
- QI-BB-003의 물리 GC 부재와 결합해 boot failure surface와 scan 비용이 시간에 따라 증가한다.
- lexical은 정상 generation도 두 번 open/validate하고 semantic active는 scan 후 rehydrate에서 다시 full scan한다.
- 현재는 손상된 inactive generation을 격리하고 정상 active corpus로 제한 기동한 뒤 repair할 운영 경로가 없다.

**보완**

1. activation/history authority를 먼저 읽어 boot에 필수인 active/candidate/predecessor set을 결정한다.
2. 필수 active generation 손상은 fail-closed로 유지한다. 비활성 손상은 경로와 reason을 quarantine inventory에 기록하고 해당 generation만 serving/rollback 대상에서 제외한다.
3. directory inventory, manifest validation, deep data scrub를 분리하고 같은 generation을 한 boot에서 중복 검증하지 않는다.
4. quarantine 조회/삭제/재검증을 typed control/CLI surface와 metric으로 제공한다.

**완료 기준**

- 손상된 비활성 old generation이 있어도 정상 active generation으로 boot/query가 성공하고 quarantine receipt가 남는다.
- 손상된 active generation은 socket bind 전에 명확한 typed cause로 실패한다.
- inactive generation 수와 무관하게 동기 boot 검증량이 필수 set 크기에 비례한다.
- lexical/semantic generation별 deep validation 횟수가 boot당 최대 1회다.

### QI-BB-027 — ANN index가 sealed generation의 운영 계약에 포함되지 않는다

**증거**

- 256 rows 이상에서 seal 시 `IvfHnswSqIndexBuilder::default()`와 cosine만 지정해 ANN index를 만든다: [build.rs](../../../crates/quanta-index-semantic/src/build.rs):62-69, 1527-1545.
- semantic manifest에는 row/model/commitment 정보는 있지만 ANN 존재 여부, index 종류, builder 파라미터, engine version, build stats가 없다: [manifest.rs](../../../crates/quanta-index-semantic/src/manifest.rs):62-108.
- production `open_generation`은 marker, manifest, schema, row/membership commitment를 검증하지만 `list_indices`나 index metadata를 확인하지 않는다: [search.rs](../../../crates/quanta-index-semantic/src/search.rs):225-383.
- query는 cosine과 `limit`만 설정하고 nprobes, search effort, refine/recall policy를 명시하지 않는다: 동일 파일 933-970.
- seal test는 ANN이 만들어졌는지 `list_indices`로 직접 확인한다. 이는 build-time 존재 증거지만 restart/open-time 계약은 아니다: [persisted_semantic.rs](../../../crates/quanta-index-semantic/tests/persisted_semantic.rs):1445-1485.

**도달 영향**

- 동일 manifest identity라도 dependency default가 바뀌면 index topology와 recall/latency가 달라질 수 있다.
- index artifact가 누락·손상됐을 때 open 단계에서 명시적으로 잡지 못한다. Lance가 flat scan으로 fallback하는지 query error를 내는지는 이번 감사에서 fault test하지 않아 **미검증**이다.
- recall/latency tradeoff를 release artifact와 generation identity로 재현하기 어렵다.

**보완**

1. manifest에 ANN required/present, index type, 모든 build/search 파라미터, library format/version, build row count와 index digest/stat을 기록한다.
2. open에서 expected index metadata를 확인하고 `ANN_INDEX_MISSING`, `ANN_INDEX_INCOMPATIBLE`처럼 typed failure를 낸다. 의도적 exact fallback을 허용한다면 mode와 metric을 응답/관측성에 노출한다.
3. query search effort/refine를 bounded policy로 고정하고 corpus tier별 recall@k 대 latency gate를 둔다.
4. dependency upgrade는 같은 corpus/query의 index metadata와 relevance/perf A/B를 통과해야 한다.

**완료 기준**

- 255/256-row 경계에서 manifest와 실제 index 존재가 일치한다.
- sealed ANN file 제거/손상 뒤 restart와 query가 문서화된 typed behavior를 보인다.
- exact baseline 대비 recall@k, p95/p99, build time, index bytes를 current HEAD artifact로 남긴다.

### QI-BB-028 — embedding cache/model identity가 provider revision을 구분하지 못한다

**증거**

- cache 설명은 model과 dimension으로 stale reuse를 막는다고 하지만 실제 key는 `(model_id, dimension, text)`만 hash한다. provider의 `model_version()`은 key에 들어가지 않는다: [cache.rs](../../../crates/quanta-index-embed/src/cache.rs):14-28, 40-55, 127-143.
- `CachingEmbeddingProvider`는 inner `model_version()`을 외부에 그대로 노출하므로 version 개념 자체는 contract에 존재한다: 동일 파일 114-123.
- OpenAI provider는 configured model name을 `model_id`로 쓰면서 `model_version()`은 항상 `None`을 반환한다: [openai.rs](../../../crates/quanta-index-embed/src/openai.rs):411-443.
- searchd의 OpenAI profile은 같은 persistent cache wrapper를 corpus와 query embedding 양쪽에 사용한다: [runtime.rs](../../../crates/quanta-index-searchd/src/app/runtime.rs):311-361.
- query/index gate는 model ID와 optional version만 비교한다. 양쪽이 같은 model name과 `None`이면 통과한다: [query_dispatcher.rs](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs):515-527.
- file cache entry는 raw little-endian `f32` 배열뿐이다. read는 byte 수가 4의 배수인지만 검사하고 dimension, finite 값, checksum을 확인하지 않으며 write도 direct best-effort overwrite다: [cache.rs](../../../crates/quanta-index-embed/src/cache.rs):215-265.
- sealed semantic manifest 내부에는 실제 vector를 포함한 row root가 있지만 public/activation `GenerationSnapshot`은 source `manifest_digest`만 운반한다: [semantic_row_integrity_v1.rs](../../../crates/quanta-index-semantic/src/semantic_row_integrity_v1.rs):170-255, [control.rs](../../../crates/quanta-index-contract/src/ipc/control.rs):800-817.

**도달 영향**

- 같은 model ID/dimension의 provider revision이 다른 vector를 내면 과거 cache hit와 새 miss가 한 generation 안에 섞일 수 있다. 새 query text가 새 revision으로 계산돼도 identity gate는 이를 구분하지 못한다.
- model version을 제공하는 다른 provider도 cache key가 version을 무시하므로 같은 문제가 발생한다.
- 같은 dimension 길이의 finite bit corruption은 정상 cache hit로 수용된다. corpus build에서는 잘못된 vector가 새 semantic row로 봉인될 수 있고 query path에서는 재계산 없이 잘못된 query vector를 사용할 수 있다.
- 서로 다른 state root에서 같은 source manifest를 independently build하면 실제 semantic row root가 달라도 외부 generation identity는 같을 수 있다. 이 항목은 분산 배포/아티팩트 승격 시 재현성 위험이며 현재 단일-host 동작 자체의 손상 증거는 아니다.

**보완**

1. cache namespace/key에 model ID, **required revision/epoch**, dimension, normalization, distance metric, embedding/view policy digest를 포함한다.
2. production provider가 immutable revision을 제공하지 않으면 operator-supplied `QUANTA_INDEX_EMBED_MODEL_REVISION` 같은 epoch를 필수로 하고, 변경 시 새 cache namespace/generation을 요구한다.
3. cache entry에 format/version, dimension, vector checksum을 넣고 temp-write + file fsync + rename + parent fsync로 교체한다. hit에서 length/finite/checksum을 검증하고 malformed/stale entry를 제거한다.
4. activation/receipt 또는 promotable artifact manifest에 semantic row root와 ANN contract를 포함해 실제 content identity를 attestation한다.

**완료 기준**

- 같은 model ID/dimension/text에서 version만 다른 fake provider가 old cache entry를 재사용하지 않는다.
- cache entry의 truncation, same-length bit flip, NaN 주입, concurrent writer를 fault test해 모두 miss/recompute 또는 typed failure로 수렴한다.
- revision 없는 production profile은 startup에서 fail-closed하거나 명시적으로 cache disabled/development mode여야 한다.
- 한 sealed generation의 모든 row가 같은 model revision/policy namespace를 가짐을 seal-time 검증한다.
- 두 state root의 동일 외부 generation identity가 다른 semantic row root를 가질 수 없거나, 불일치가 promotion/activation에서 거부된다.

### QI-BB-029 — search-corpus ingest가 cross-track 계약을 mutation 전에 전부 검증하지 않는다

**증거**

- direct materializer 진입점은 surface mutation 충돌만 검증한 뒤 target state를 검사하고 semantic derivation → lexical build → semantic publish 순서로 실행한다: [ingest_dispatcher.rs](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs):189-285.
- semantic generation contract는 `ReplaceGeneration`에 `base_generation`이 있으면 거부한다: [generation_contract.rs](../../../crates/quanta-index-semantic/src/generation_contract.rs):141-152. 이 검증은 semantic build 안의 `ensure_generation_contract`에서 뒤늦게 실행된다: [build.rs](../../../crates/quanta-index-semantic/src/build.rs):1360-1380, 1616-1622.
- lexical은 `Delta + base_generation 없음`만 거부하고 `ReplaceGeneration + base_generation 있음`은 허용한다. mode와 무관하게 base가 있으면 directory 전체를 복사하고, seal batch면 lexical sealed identity까지 기록한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):3043-3095, 3320-3428.
- lexical base clone은 directory 존재만 확인한다. semantic base clone은 sealed marker를 요구한다: [lexical/lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):2873-2910, [build.rs](../../../crates/quanta-index-semantic/src/build.rs):1424-1458.
- 빈 `manifest_digest`도 ingest DTO/authority record에서 선검증되지 않지만 composite activation identity는 이를 거부한다: [ingest.rs](../../../crates/quanta-index-contract/src/ipc/ingest.rs):502-521, 724-735, [readiness.rs](../../../crates/quanta-index-search-plane/src/readiness.rs):1933-1973, 3192-3220.

**도달 영향**

- `ReplaceGeneration + base_generation` sealed batch는 lexical target을 base clone으로 만들고 seal한 뒤 semantic에서 거부될 수 있다.
- delta base가 lexical에는 존재하지만 미봉인이고 semantic에는 유효하지 않은 경우에도 lexical target이 먼저 seal된 뒤 semantic이 실패할 수 있다.
- target generation은 lexical-only sealed 상태가 되며 incomplete-generation discard는 sealed exact generation 삭제용이 아니다. 같은 generation의 정상 재시도와 자동 convergence가 막혀 operator 수동 복구가 필요할 수 있다.
- 빈 digest generation은 build/authority 기록까지 진행돼도 activation contract를 만족할 수 없다.

**보완**

1. `SearchCorpusIngestBatch::validate_v1()`에서 mode/base shape, non-empty/canonical digest, batch identity, record/resource limits를 mutation 전에 공통 검증한다.
2. Delta base는 lexical/semantic 양쪽의 exact sealed identity와 같은 repo/revision을 preflight한다.
3. lexical과 semantic의 build plan을 모두 만든 뒤 pair staging에 쓰고, 둘 다 검증된 경우에만 sealed marker/authority를 promote한다.
4. 중간 실패로 남은 half-sealed pair를 exact receipt에 따라 quarantine/repair할 recovery protocol을 제공한다.

**완료 기준**

- invalid mode/base, 빈 digest, absent/unsealed/mismatched base가 lexical·semantic·authority bytes를 0개 변경한다.
- lexical seal 직후부터 semantic seal/authority commit까지 각 failpoint에서 재시도가 같은 generation으로 수렴한다.
- half-sealed fixture가 manual directory 삭제 없이 typed repair 또는 안전한 rebuild로 복구된다.
- cross-track preflight와 fault matrix가 raw IPC와 SDK 양쪽에서 실행된다.

### QI-BB-030 — lexical seal과 activation 검증이 query-openable sidecar 상태를 보장하지 않는다

**증거**

- text authority의 5개 sidecar는 `File::create`/`std::fs::File::create` 대상에 직접 serialize된다. temporary file, file `sync_all`, atomic rename, checksum manifest가 없다: [lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):2657-2764.
- lexical build는 Tantivy commit 뒤 text authority sidecar를 재생성하고, sealed batch이면 그 다음 generation identity marker를 atomic durable write한다: 동일 파일 3396-3428, 3644-3678. marker의 file/parent fsync는 앞서 쓴 sidecar file data를 fsync하지 않는다: 동일 파일 2335-2414.
- activation/restart가 호출하는 `validate_generation_identity`는 sealed identity 일치와 `Index::open_in_dir`만 확인한다. query의 `open`이 추가로 읽는 text authority와 repo metadata sidecar는 검증하지 않는다: 동일 파일 3682-3804.
- 실제 activation/rollback/restart는 이 좁은 validator를 physical pair의 openability 판정으로 사용한다: [search_corpus_lifecycle.rs](../../../crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs):174-191, 239-307, 320-338.
- 4차 focused fault test에서 정상 sealed generation의 `text-authority-trigram.cbor`만 삭제했다. 같은 candidate에 `validate_generation_identity`는 성공했고 `LexicalIndexOpenPort::open`은 `text authority sidecar incomplete`로 실패했다. 테스트 코드는 실행 직후 원복했다.

**도달 영향**

- publish 후 검증과 activation CAS가 성공했는데 첫 text/symbol/hybrid/explain query가 실패하는 상태를 만들 수 있다.
- sidecar write 중 process/power loss가 나면 sealed marker가 durable하더라도 sidecar payload가 zero-length/truncated/old-new 혼합 상태일 수 있다. validator는 이를 보지 않는다.
- sidecar bytes와 sealed identity 사이의 content commitment가 없어 silent same-shape corruption도 activation identity만으로 검출할 수 없다.
- text authority가 전혀 없는 legacy generation은 `None`으로 열 수 있으므로, “sidecar 없음”과 “해당 generation이 원래 sidecar를 요구하지 않음”도 sealed contract에 명시돼 있지 않다.

**보완**

1. lexical generation manifest에 필수/선택 sidecar 목록, format version, byte length, checksum과 Tantivy commit identity를 기록한다.
2. 모든 sidecar를 unique staging file에 쓰고 file fsync → rename → directory fsync를 완료한 뒤 manifest와 sealed marker를 마지막에 promote한다.
3. activation/restart validator가 실제 serving open과 동일한 파일 집합과 schema/checksum을 검증하게 한다. 가능하면 검증된 loaded handle을 query cache에 그대로 넘겨 QI-BB-001의 이중 open도 제거한다.
4. incomplete/half-promoted sidecar를 sealed identity와 구분해 quarantine 또는 안전한 rebuild가 가능한 recovery protocol을 둔다.

**완료 기준**

- 각 sidecar의 missing, truncation, bit flip, stale copy와 매 fsync/rename crash point에서 publish/activation/restart가 query-openable 상태만 ack한다.
- activation 성공 직후 별도 reopen 없이 모든 lexical query family가 동일 검증 handle로 실행된다.
- sealed manifest checksum과 실제 모든 query-required bytes가 일치하며 generation identity가 그 manifest를 결속한다.
- sidecar가 없는 generation은 manifest의 explicit capability set과 query typed refusal로 구분된다.

### QI-BB-031 — semantic `L2Unit` normalization은 metadata일 뿐 실제 vector invariant가 아니다

**증거**

- `TextEmbeddingProvider` trait은 반환 vector가 unit-normalized라고 명시한다: [outbound.rs](../../../crates/quanta-index-core/src/domains/semantic/outbound.rs):17-38.
- search-owned derivation은 provider 종류와 실제 출력에 관계없이 model contract에 `EmbeddingNormalization::L2Unit`을 기록한다: [semantic_derive.rs](../../../crates/quanta-index-search-plane/src/semantic_derive.rs):82-103.
- OpenAI provider는 response count/index/dimension만 확인하고 raw float vector를 반환한다. finite 값과 L2 norm은 검사하거나 정규화하지 않는다: [openai.rs](../../../crates/quanta-index-embed/src/openai.rs):215-255.
- semantic ingest는 공통 query-vector validator를 재사용한다. 이 validator는 finite와 nonzero norm만 확인하고 `norm ≈ 1`은 요구하지 않는다: [build.rs](../../../crates/quanta-index-semantic/src/build.rs):456-467, [service.rs](../../../crates/quanta-index-core/src/domains/semantic/service.rs):37-58.
- unit vector가 아닌 `[1.0, 0.0, 2.0]`을 valid로 고정한 test가 있다: [semantic_policy.rs](../../../crates/quanta-index-core/tests/semantic_policy.rs):40-50.
- durable generation contract/manifest는 normalization 문자열이 batch와 같은지만 비교한다. 실제 row norm의 commitment나 tolerance 검사는 없다: [generation_contract.rs](../../../crates/quanta-index-semantic/src/generation_contract.rs):155-200, 238-268.

**도달 영향**

- custom provider, 변경된 remote response, same-length cache corruption이 non-unit vector를 반환해도 generation은 `l2_unit`으로 봉인된다.
- cosine distance 자체는 vector 크기에 불변이므로 non-unit이라는 사실만으로 현재 cosine 순위 오류가 증명되지는 않는다. 다만 contract/manifest가 실제 bytes를 설명하지 못하고, ANN/quantization 또는 후속 소비자가 unit norm을 전제로 할 경우 재현성과 품질이 달라질 수 있다.
- 이번 감사에서는 real OpenAI response의 norm 분포와 non-unit vector가 Lance ANN recall에 미치는 영향은 측정하지 않았다.

**보완**

1. provider 경계에서 dimension, finite, nonzero와 L2 norm tolerance를 중앙 검증한다.
2. contract가 `L2Unit`이면 중앙에서 deterministic normalization하거나 tolerance 밖 출력을 typed failure로 거부한다. 원본 vector를 유지하려면 normalization을 `None`으로 기록한다.
3. cache hit와 fresh provider output에 같은 validator를 적용하고 normalization policy/version을 cache key와 manifest identity에 넣는다.
4. seal-time sampled 검사보다 모든 ingest vector의 streaming validation을 사용하고 norm deviation metric을 남긴다.

**완료 기준**

- norm 0, NaN/Inf, 0.5, 2.0 vector를 반환하는 fake provider에서 documented normalize/fail behavior가 corpus/query 양쪽에 동일하다.
- `L2Unit` generation의 모든 persisted row가 정해진 tolerance 안에 있고 manifest policy와 일치한다.
- cache hit/fresh miss 혼합 batch도 동일 결과를 만들며 provider별 recall/latency artifact가 normalization policy를 기록한다.

### QI-BB-032 — `batch_digest`가 idempotency identity로 동작하지 않는다

**증거**

- 역사 계획 문맥의 SDK cutover 문서는 quanta-index가 “idempotency-keyed, replay-safe batch publish”를 보장한다고 명시한다. 현재 authoritative product contract인지 여부는 별도 확인이 필요하지만 public SDK가 `batch_digest`를 필수로 받는 배경 증거다: [may-25-sdk-cutover-wave-plan.md](../../plans/may-25-sdk-cutover-wave-plan.md):286-299.
- search-corpus wire DTO는 `batch_digest`를 필수 문자열로 운반하지만, 공개 validator는 surface mutation의 중복/충돌만 검사한다. digest 형식, payload와의 일치, 과거 digest 충돌은 검증하지 않는다: [ingest.rs](../../../crates/quanta-index-contract/src/ipc/ingest.rs):502-529, 724-735.
- direct materializer는 per-repo/revision stripe lock 아래 body를 derive/build한 뒤 generation authority를 갱신한다. `batch_digest` 조회·저장·duplicate short-circuit가 없다: [ingest_dispatcher.rs](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs):189-285.
- search-corpus receipt는 `batch_digest`가 아니라 `manifest_digest`만 반환하고 SDK도 generation/manifest/count/seal만 대조한다: 동일 파일 592-607, [lexical.rs](../../../crates/quanta-index-sdk/src/lexical.rs):567-605.
- 반대로 repo metadata/dirty 같은 auxiliary receipt는 `BatchPublishReceipt.manifest_digest` 필드에 `batch_digest`를 넣는다. 같은 receipt field가 route별로 다른 identity를 뜻한다: [lib.rs](../../../crates/quanta-index-lexical/src/lib.rs):3432-3515, [ingest_dispatcher.rs](../../../crates/quanta-index-search-plane/src/ingest_dispatcher.rs):776-805.

**도달 영향**

- ack 유실 뒤 같은 unsealed batch를 재전송하면 duplicate인지 판별하지 못하고 lexical/semantic derive, commit, sidecar rebuild와 provider 호출을 다시 수행할 수 있다.
- 같은 `(repo, revision, generation, batch_digest)`에 다른 body가 와도 digest conflict로 거부되지 않는다. op별 overwrite semantics가 최종 row 중복을 막을 수는 있지만 idempotency key의 immutable-body 계약을 보장하지 않는다.
- receipt만으로 “새로 적용”, “이미 durable해서 replay ack”, “동일 key의 다른 payload 거부”를 구분할 수 없다. route별 `manifest_digest` 의미까지 달라 producer recovery 판단이 불명확하다.

**보완**

1. canonical encoded batch body를 server가 hash하고 caller digest와 대조한다. digest 형식/domain/version을 고정한다.
2. `(operation kind, repo, revision, generation, batch_digest)`별 durable idempotency record에 canonical body hash와 최종 receipt를 기록한다.
3. 같은 key/same body replay는 storage mutation과 embedding 호출 없이 이전 durable receipt를 반환하고, 같은 key/different body는 typed conflict로 거부한다.
4. receipt에 `manifest_digest`, `batch_digest`, `applied`, durable sequence/epoch를 별도 필드로 둔다. route별 field overloading을 제거한다.
5. idempotency record의 retention을 generation GC/pin과 결속한다.

**완료 기준**

- response write 전후 disconnect, process crash, client timeout 뒤 동일 batch replay가 한 번만 materialize/embed/persist한다.
- 같은 digest의 1-byte payload 변경은 mutation 전에 typed conflict가 된다.
- aux/search-corpus 모든 route에서 receipt field 의미가 동일하고 producer가 replay 여부를 판정할 수 있다.
- 1/8/32 concurrent duplicate publish가 하나의 apply와 동일 durable receipt로 수렴한다.

## 6. 유지할 설계

- Tantivy/LanceDB를 adapter 경계 안에 두고 core contract를 vendor-neutral하게 유지한 구조.
- semantic의 sealed immutable generation, exact identity/digest 검증, incomplete generation만 파괴 가능한 recovery 경계. lexical은 QI-BB-030의 sidecar durability/openability gap을 닫아야 같은 평가가 가능하다.
- semantic generation open cache의 bounded FIFO와 read-lock hit 경로.
- semantic corpus/query provider identity를 동일하게 묶고 mismatch를 fail-closed하는 계약. QI-BB-028처럼 revision이 실제로 식별된다는 전제는 보완해야 한다.
- regex/trigram candidate 뒤 exact verification, deterministic ordering, restart/replay 검증.
- unsupported DSL을 조용히 무시하지 않고 typed refusal하는 capability governance.
- 16 MiB IPC frame cap, AST depth/fanout/structural limit, semantic top-k max 같은 기존 방어선.
- workspace `unsafe_code = "forbid"`, `unwrap/panic/todo` 제한 등 강한 정적 정책.

자체 lexical engine으로 교체하면 위 안전장치와 Tantivy의 segment/query 동작을 동시에 재구현해야 한다. 현재 문제는 engine 교체보다 adapter/lifecycle/serving layer에서 해결하는 것이 비용과 위험이 낮다.

## 7. 권장 보완 순서

아래 Wave A–E는 감사 당시의 결함 우선순위다. 실제 구조 변경의 작업 묶음·의존 순서·삭제 조건은 [구조 개선 계획](structural-remediation-plan.md#9-실행-묶음과-의존-순서)을 따른다.

### Wave A — 계약/자원 안전

1. QI-BB-030 lexical sidecar atomic durability, manifest commitment, serving-equivalent activation validation.
2. QI-BB-029 ingest 공통 preflight와 cross-track staged seal/recovery.
3. QI-BB-032 durable batch idempotency identity와 unambiguous receipt.
4. QI-BB-028 versioned embedding/cache/content identity와 cache checksum.
5. QI-BB-031 actual vector normalization invariant.
6. QI-BB-004 `scope_top_k` 적용과 regression test.
7. QI-BB-025 route 공통 `top_k` 정책과 boundary E2E.
8. QI-BB-005 full-recall/projection 절대 budget, pagination/aggregation.
9. QI-BB-021 ingest record/vector byte envelope와 bounded streaming.
10. QI-BB-024 regex cache byte cap/compact ID set과 oversized response typed error.

### Wave B — query serving

1. QI-BB-001 lexical exact-generation read cache + single-flight + byte budget.
2. QI-BB-002 bounded concurrency/queue/deadline/cancellation.
3. QI-BB-026 active-first boot inventory와 inactive quarantine.
4. QI-BB-017 semantic seal/open proof 분리와 repeated full scan 제거.
5. QI-BB-020 auxiliary per-key snapshot/lock 분리와 lock 밖 persistence.
6. cache와 concurrency를 합친 RSS/latency load test.

### Wave C — storage lifecycle와 incremental

1. QI-BB-003 actual-byte sealed generation GC와 crash recovery.
2. QI-BB-009 embedding cache/sample retention.
3. QI-BB-006 unchanged segment/shard 재사용과 changed-data 비례 update.
4. QI-BB-020 auxiliary generation GC와 changed-key persistence.
5. RepoMap atomic persistence/retention.

### Wave D — 제품 품질과 운영 증거

1. QI-BB-007 production semantic profile와 representative relevance gate.
2. QI-BB-018 true hybrid와 lexical-scoped rerank surface 분리.
3. QI-BB-019 HybridSeed v2 단일 canonical contract로 종료.
4. QI-BB-022 provenance-backed explain과 exact presence lookup 분리.
5. QI-BB-023 history relevance/recency ranking과 cursor.
6. QI-BB-027 versioned ANN contract와 recall/latency gate.
7. QI-BB-010 exact-head controlled benchmark.
8. QI-BB-015 production metric export와 SLO.
9. QI-BB-011 shared text normalizer.
10. README와 architecture claim 갱신.

### Wave E — 구조 정리

1. QI-BB-008 RepoMap indexed/bounded query response.
2. QI-BB-013 책임별 module 분리.
3. QI-BB-014 deployment access policy와 QI-BB-016 memory envelope.

Wave A와 B 완료 전의 성능 수치는 현재 durability/API 계약/serving/restart path를 대표하지 않는다. Wave C의 물리/auxiliary GC 없이 장기 soak를 통과해도 disk/RSS 안정성을 증명하지 못한다. Wave D의 ANN/hybrid/explain/history 보완 전에는 “재현 가능한 semantic recall”, “hybrid recall 확대”, “explainable ranking”, “relevant history top-k”를 제품 기능으로 주장할 수 없다.

## 8. 이번 감사의 실행 증거

| 명령 | 결과 | 포함 범위 | 제외/한계 |
| --- | --- | --- | --- |
| `just rust-profile test-fast` | PASS | fast Rust profile | daemon full E2E, real provider, scale perf 제외 |
| `just rust-profile test-daemon` | PASS: 200 passed, 0 failed, 1 ignored | aggregate daemon/runtime E2E | ignored real OpenAI test 제외. host package-lock contention이 있어 135.57초 full-corpus 시간은 성능 수치로 사용하지 않음 |
| `./scripts/cargow --lane test-fast-lane test --locked -p quanta-index-core validate_top_k_kills_boundary_gt_to_ge_mutation` | PASS: 1 passed | semantic policy가 `top_k=10_000`을 허용하는 현재 계약 | dispatcher 실행 성공을 뜻하지 않음 |
| `./scripts/cargow --lane test-fast-lane test --locked -p quanta-index-search-plane query_window_uses_one_continuation_row_and_never_requires_full_count_v1` | PASS: 1 passed | dispatcher probe가 9,999만 허용하고 10,000을 거부하는 현재 동작 | 두 PASS의 상충 자체가 QI-BB-025 증거 |
| `./scripts/cargow --lane test-fast-lane test --locked -p quanta-index-semantic --test persisted_semantic delta_with_unsealed_base_fails_closed` | PASS: 1 passed | semantic base seal gate | lexical-first cross-track half-seal E2E는 미구현/미실행 |
| 임시 focused test: `quanta-index-lexical --test tantivy_smoke bugbash_validator_accepts_generation_that_query_open_rejects_tmp` | PASS: 1 passed | sealed generation의 trigram sidecar 삭제 후 identity validator 성공, query open 실패 재현 | 감사용 test를 실행 후 원복했으며 checked-in regression test는 아직 없음 |
| `./scripts/cargow --lane test-fast-lane test --locked -p quanta-index-core semantic_query_vector_finite_non_zero_is_valid -- --exact` | PASS: 1 passed | non-unit `[1, 0, 2]` vector를 현재 policy가 허용함 | provider/Lance 품질 영향 측정은 아님 |
| `python3 tools/benchmark/sourcegraph_parity.py --check` | PASS: 38 filters, 29 required surfaces, 0 waived gaps | DSL parity inventory/typed refusal | ranking quality와 대규모 runtime 성능 제외 |
| `python3 tools/ci/lint/check-dsl-capability-truth.py` | PASS | capability docs/registry truth | backend performance 제외 |
| `./scripts/cargow --lane test-fast-lane run --locked -p quanta-index-scan-experiment --bin scan_vs_index -- --out-dir <tmp> --chunks 100 --chunk-bytes 128 --needle-count 5 --samples 2` | FAIL: `GENERATION_IDENTITY_INCOMPLETE` | benchmark harness 재생성 가능성 | 제품 query test 실패가 아니라 experiment가 current seal contract를 따르지 않는 tooling failure |
| `just lint-doc-paths` | FAIL: 기존 broken path 2건 | repository Markdown path | 이번 findings 밖의 기존 문서 2개가 원인. findings 자체의 local link target 누락은 0건 |
| `just lint-root-hygiene` | PASS | repository root hygiene | 문서 내용 정확성 제외 |
| findings local-link/source-range/ID/count checker | PASS: 137 links, missing/range issue 0, 32 IDs unique, P1 13/P2 17/P3 2 | 이번 findings의 구조와 local source target | source line 의미는 manual audit로 별도 확인 |
| `git diff --no-index --check /dev/null docs/bugbash/sep-16/findings.md` | whitespace diagnostic 0; exit 1은 untracked file content diff 때문 | untracked 문서의 whitespace/error marker | 제품 코드/Markdown 의미 검증 제외 |

실행하지 않은 범위:

- real OpenAI API semantic E2E.
- canonical Linux medium/large/xlarge performance runner.
- controlled idle host의 multi-client load/soak, disk-full, power-loss/fault injection.
- production state root의 실제 disk/RSS/traffic 측정.
- real power-loss 환경의 lexical sidecar durability와 filesystem별 ordering 검증.
- 외부 Semantica/producer repository와의 exact-head integration.

## 9. 최종 exit criteria

다음 조건을 모두 충족하기 전에는 “성능·기능적으로 완료”로 닫지 않는다.

### 기능

- lexical publish/activation/restart가 serving에 필요한 모든 sidecar의 checksum/durability를 검증하고, activation 성공이 실제 query open 성공을 보장함.
- ingest가 invalid mode/base/digest와 양쪽에서 exact sealed가 아닌 delta base를 mutation 전에 거부하고, 모든 cross-track failpoint retry가 같은 generation으로 수렴함.
- canonical `batch_digest`가 payload를 결속하고, same-body replay는 재작업 없이 같은 durable receipt를 반환하며 same-key/different-body는 거부됨.
- `scope_top_k`와 모든 public `top_k`/count/projection 계약이 route 공통 boundary regression test로 고정됨.
- embedding cache/model/generation identity가 immutable provider revision과 실제 semantic row root를 구분함.
- `L2Unit` semantic contract가 corpus/query/cache 모든 vector의 실제 norm과 일치함.
- learned semantic profile이 representative judged relevance gate를 통과함.
- true hybrid가 독립 dense recall을 제공하거나 lexical-scoped rerank로 API가 명확히 분리됨.
- HybridSeed가 단일 canonical candidate list를 반환하고 metric/window와 일치함.
- explain이 원 query의 실제 score/ranker provenance를 제공하고 presence lookup은 exact ID로 수행됨.
- history top-k가 명시된 relevance/recency total order와 stable cursor를 가짐.
- Unicode/token boundary semantics가 leaf 간 일관되거나 차이가 계약에 명시됨.
- RepoMap과 oversized response가 bounded/paginated typed behavior를 가짐.

### 성능

- exact-head real-size corpus에서 cold/warm p50/p95/p99, QPS, peak RSS, response bytes가 측정됨.
- 1-file delta의 IO/time/temp disk가 전체 corpus가 아니라 변경량에 근접함.
- 1/8/32 client와 slow-client 혼합에서 head-of-line blocking이 없음.
- lexical cache hit가 실제 file reopen/sidecar decode를 제거함.
- semantic activation/restart/first query가 sealed corpus 전체를 반복 scan하지 않음.
- lexical activation 검증에서 만든 loaded handle을 첫 query가 재사용하고 sidecar를 다시 decode하지 않음.
- boot 동기 비용이 inactive generation 수/row 수와 독립이고 generation별 deep validation을 중복하지 않음.
- semantic ingest와 regex cache가 record/vector/match byte cap 안에서 동작함.
- ANN index 종류/파라미터가 고정되고 exact baseline 대비 recall@k/latency/index bytes가 gate됨.

### 운영

- actual index bytes 기준 retention과 sealed-generation physical GC가 restart/crash-safe함.
- embedding cache, RepoMap, diagnostics가 모두 bounded retention을 가짐.
- embedding cache가 versioned/checksummed atomic entry를 사용하고 corruption을 wrong hit로 수용하지 않음.
- timeout이 client I/O뿐 아니라 server dispatch 취소까지 전파됨.
- metric/exporter로 queue, cache, open, scan, ingest, GC, provider failure를 진단할 수 있음.
- UDS mode/owner/auth policy가 배포 모델에 맞게 fail-closed 검증되고 live socket path 충돌이 기존 endpoint를 unlink하지 않음.
- active generation 손상은 fail-closed, inactive 손상은 격리·관찰·복구 가능한 boot policy가 있음.
- ANN artifact 누락/손상의 restart/query behavior가 manifest 계약과 typed error로 고정됨.
- auxiliary history/runtime/structural state가 per-key changed-data 비례로 persist되고 retention 시 memory/disk에서 함께 제거됨.
- auxiliary query와 persistence가 전역 ledger lock을 장시간 점유하지 않으며 partial durable failure가 receipt와 일치함.
- batch idempotency record가 generation retention과 함께 정리되고 crash/retry 뒤 receipt와 durable apply 횟수가 일치함.

### 증거

- benchmark/relevance artifact가 exact 40-char HEAD, corpus/config/model digest, host, cold/warm, concurrency를 포함함.
- stale/unknown-SHA artifact는 release gate에서 자동 거부됨.
- `test-fast`, `test-daemon`, DSL capability/parity, doc lint와 필요한 Linux perf/relevance gate가 같은 HEAD에서 통과함.
