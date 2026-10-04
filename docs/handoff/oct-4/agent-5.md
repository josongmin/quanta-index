# Agent 5 handoff — lexical/robustness benchmark, Quanta RCA, B09 hardening

작성: 2026-10-04, Asia/Seoul. 상태 관측: 15:20 KST.

## 1. 인계 요약

이 채팅은 Gin lexical 실패 조사에서 시작해 벤치 계약 보강, 파일 검색 비교, 식별자 견고성/외부 NL 벤치 확장, Quanta RCA 및 구조 수정까지 진행했다. 최근 직접 소유 범위는 **S30-B09의 라벨·NL planner·typo 정책/순위·JS 선언 생산자·증거 소비자**다.

- **구현/좁은 검증 완료:** 정확한 원래 선언 이름의 파일 정답, 대표 파일 정답 분리; scored Keyword OR NL 요청; 기본 literal-first와 명시적 OSA1 분리; source-attested 선언 우선순위와 cursor; Rust/Python 공통 JS ABI15 grammar; runtime/provenance/Unicode17 결속.
- **실행 완료:** 고정 소스 `d1a1b709`의 Quanta 33개 진단 캡처, 11,272개 응답. 최신 hardening에서는 별도 Unicode 2질의 native control과 기존 영수증 읽기 전용 재검증을 추가했다.
- **남음:** 소유 변경의 최종 출판/통합, 새 소스의 5제품 전체 행렬, 정확한 선언 이름 span 평가, 독립 relevance 검수/새 holdout, 외부 전체 색인 증명, 조용한 호스트 성능, 넓은 semantic 품질 및 CI/release qualification.
- **현재 판정:** 검토한 경계에서 재현한 코드 결함 3개는 수정하고 owner 검증으로 닫았다. 전체 벤치/제품 적격성은 아직 닫지 않았다. 모든 아래 품질 관측은 `diagnostic_unqualified`다.
- **수정 장소:** 구현은 공유 main에 있다. 외부 private checkout은 실행 소스 고정을 위한 snapshot이다. 작업트리에만 구현하고 main을 방치한 상태로 해석하지 말 것.

이 문서는 세션 인계 자료다. 과거 원본 결과/판정을 덮어쓰거나, 현재 HEAD로 모든 과거 실행이 재검증됐다고 선언하지 않는다.

## 2. 현재 소스·소유권·작업공간

| 항목 | 관측/의미 |
| --- | --- |
| 공유 저장소 | `/Users/songmin/Documents/code-new/quanta-index` |
| 현재 HEAD | `2062fed3ff86287a0914ece99dc2fe0af829c29b`, main |
| dirty 상태 | 다수 staged 변경과 다른 작업자의 변경 존재. staged라는 사실은 소유권/검증/commit 완료의 증명이 아니다. |
| 최근 hardening 시작점 | `e43cda8c87b4a06fecac82a266011e30f84a2986` + 명시한 dirty overlay |
| 최근 native 대규모 캡처 소스 | `d1a1b7097c5c915afd8fb24467c6d737acdb3b14`, clean private `source-v3` |
| structural repair의 base main | `1ca82c0e97cacf083745ab0f14a9292302af7502` |
| 이전 RCA 기준 | `d7063ac755916d48867416d4f970b6aebc360abd` |
| 현재 소스와 hardening 결속 재확인 | `final-source-binding.json`의 검토 파일 **62개 모두 현재 바이트와 일치**. 이번 인계 작성 중 읽기 전용 확인. 전체 저장소/바이너리 결속 판정이 아니다. |
| publication | 최근 hardening 턴에서는 commit/push를 실행하지 않았다. 이 인계 요청도 문서 저장 범위다. 과거 세션의 개별 commit/push를 전부 복원해 검증한 것은 아니다. |

주요 공유 변경: dependency/CI, benchmark schema/diagnostics, runtime/ingest observation, lexical ASCII scan, B07 timing, B08 review/controller. 파일 단위로 전부 이 채팅 소유라고 판단하면 안 된다. 겹치는 hunk는 현재 diff와 아래 결속 파일을 함께 대조한다.

다른 작성자의 `/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-1.md`가 존재한다. 본 문서와 B07/B08 owner 문서는 별도 인계이며 서로 덮어쓰지 않는다.

## 3. 세션 범위와 반드시 지킬 계약

### 3.1 원래 300질의 RCA

원래 기준은 `quanta-index@c64f6af5d5e2817e346336ceee0e57f5fb25d74f`의 읽기 전용 checkout:
`/Users/songmin/.codex/worktrees/gin-300-benchmark/quanta-index`다.

- Gin: `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, code_only 99파일.
- checkout: `/Users/songmin/Documents/code-new/qi-large-scale-rerun-20260927/full-checkouts/gin`.
- manifest: `/Users/songmin/Documents/code-new/qi-100-query-20260928/corpus-releases/gin/manifests/gin/code_only.json`.
- file-universe SHA-256: `d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`.
- 원본 캡처: `/Users/songmin/Documents/code-new/qi-smoke-gin-300-20260928` 및 `/private/tmp/g3`.
- 원래 Quanta miss: S027 `Param`/`context.go`, S040 `mappingByPtr`/`binding/form_mapping.go`, S122 `Type`/`testdata/protoexample/test.pb.go`, S155 `Next`/`context.go`, S273 `writeContentType`/`render/render.go`.
- 원래 실행 verdict의 selected/executed/passed는 600/600/600, failed 0이다. 검색 적중 실패와 실행 실패는 다르다. `capped`를 자동 실패로 세지 않는다.

원래 Quanta/Semble top10은 청크, 외부 세 제품은 파일이었다. 동명 사용처/테스트/생성 코드와 기계 생성 대표 gold 때문에 당시 값으로 제품 순위를 확정하면 안 된다. 이후 파일 모드와 정답 계약을 보강했다. 새 snapshot의 symbol control은 원래 캡처 시점 색인 상태를 소급 증명하지 않는다.

### 3.2 기본 표본은 1,196개, lane은 별도

사용자는 이후 exact 기본 실행을 **Gin 1,196개 전체**로 지정했다. 300개는 역사적 회귀/원본 재생 subset이며 새 대표 점수의 전체 분모로 대체하면 안 된다.

각각 별도 표/분모/요청 정책으로 보고한다:

1. Exact 기본 검색: Gin 1,196.
2. Prefix / infix / identifier components: 부분 이름/분해 계약별.
3. Typo 기본 검색: literal-first 기본 정책의 견고성.
4. Explicit typo recovery: 사용자가 지정한 OSA1 정책.
5. No-answer: 독립 부재 정답 및 반환/거절 행동.
6. NL/semantic: Gin20, CLARC, CodeSearchNet 등 외부 qrel 계약별.
7. 저장소 작업 검색: ARB의 원래 query/snapshot과 adapted query를 분리.

`substring_file`의 경로순·상수 점수 파일 집합, `keyword_file`의 scored 파일 순위, symbol/청크 응답을 같은 ranking 지표로 혼합하지 않는다. 기본 검색의 오타 관측은 Sourcegraph fuzzy finder/UI의 능력을 측정한 것이 아니다. Semble 파일 적중은 살아남은 subtoken BM25로도 가능하므로 이름 교정/선언 회복 성공으로 승격하지 않는다.

### 3.3 채점·분모

- `intended_name_file`: **원래 exact 이름을 실제 선언한 파일들**. 인접 이름을 정답으로 추가하는 지표가 아니다.
- `intended_original_file`: 표본의 대표 원래 파일 회복. 같은 이름의 다른 선언 파일과 구별한다.
- file Hit/MRR/NDCG와 정확한 declaration-name span/identity 회복은 별도다.
- 실행 실패 포함 운영 점수/coverage와 조건부 검색 품질을 분리한다. 비교는 공통 적격 ID를 명시한다.
- 잘린 결과라도 top10 계약을 충족한 `capped`는 적격일 수 있다. partial/error/missing/unjudged는 사유와 수를 출력한다.
- unknown/no-positive qrel을 자동으로 no-answer로 바꾸지 않는다.
- 반환 문맥 span 크기로 선언 적중을 결정하지 않는다. `unit_id`와 source-bound 정확한 indexed declaration/name span이 필요하다.
- 수행하지 않은 요청, 잘못된 unit/case/authority, 적격하지 않은 source를 정상 품질 0점으로 위장하지 않는다.

## 4. 최근 구조 수정: 구현과 직접 근거

| 단위 | 확인된 문제 / 수정 | 소유 구현과 검증 |
| --- | --- | --- |
| R0/B1 라벨 | 과거 near-name 명칭/대표 파일과 실제 exact-name 선언 파일 의미가 혼동될 수 있었다. primary/secondary 계약을 명시하고 source authority/unit 불일치를 거절한다. | `identifier_robustness_multiproduct_report.py`, `identifier_robustness_fresh_join.py`; 고정 경로/grade/status 및 잘못된 authority·중복 top10 거절 tests. |
| N1 NL 요청 | Phrase OR가 match-only 상수 점수 후보를 만들어 scored relevance 의도와 달랐다. 정규화·소문자화·중복 제거 후 scored Keyword OR를 생성한다. | Rust/Python `query_plan`; rare-term vs frequent-term 독립 L3 fixture, operator 안전성/byte/token budget/effective request tests. |
| T1 typo 정책 | literal-first 기본 검색과 명시적 OSA1을 같은 기능으로 설명하면 원인/성공이 뒤섞인다. explicit 경로를 독립 평가한다. | `code_search.rs`; literal 결과 보존, literal-empty fallback, 명시적 교정의 실제 API/fixture. |
| T2 typo 순위 | source에서 확인한 선언 증거를 실제 emitted file ranking에 반영한다. distance > declaration > bounded occurrence로 정렬한다. | `code_search/ranking.rs`, lexical route; 27파일 고정 fixture, unknown coverage 보존, budget/cancel/page/cursor tests. |
| JS 선언 생산자 | 유효한 reserved export alias 때문에 Svelte `builders.js`가 parse_failed였고 선언 coverage가 누락됐다. 공통 ABI15 grammar를 수정했다. | `vendor/tree-sitter-javascript`, Rust `symbols.rs`/build, Python `declaration_parsers.py`; 56선언 회복, explicit Svelte 잔여 3건 rank1. invalid reserved binding은 계속 거절. |
| proof 연결 | stale 중복 Rust proof 명령, 독립 line golden의 잘못된 schema 동반 수정, runtime/registry seam을 수정했다. | `run.py`가 portable proof command를 재사용; source end_line 기대값 8 유지; inventory와 CI/dependency tests. |

### 4.1 NL planner의 현재 계약

- `NL_PLAN_PROFILE = nl-scored-keyword-or-v3`.
- chunk profile: `quanta-natural-language-ucd17-v3`; file profile: `quanta-natural-language-file-ucd17-v2`.
- 기본 32 tokens; 명시적 설정 최대 64. raw token 최대 96 chars, folded term 최대 256B, 전체 입력 최대 16KiB.
- canonical token 정규화와 첫 등장 순서 dedup; `AND`/`OR` 같은 단어는 query 데이터로 안전하게 처리한다.
- lexical 요청 예: `select:file case:no find OR retry OR handling`.
- semantic raw query는 lexical 정규화 문자열로 바꾸지 않는다.
- `TEXT_NORMALIZER_VERSION=2.0`의 per-character lowercase이며 full casefold/contextual locale lower가 아니다.

### 4.2 typo 순위의 보수적 경계

- Complete(count>0), source-attested `RawAsciiLocalName` 선언만 bonus 증거로 사용한다.
- 선택한 파일 후보 집합의 source file/SYMBOL 제약, definition 범위, 원문 spelling, source digest를 확인한다.
- FuzzyTermQuery distance1/transposition 지원과 bounded 전체 후보 수집을 사용한다.
- 점수: `200 - 100 * distance + 8 * declaration_at_same_distance + 2 * min(occurrences - 1, 3)`.
- unknown coverage는 bonus를 만들지 않고 content 후보를 보존한다.
- order signatures:
  - `code_search_file_overlap_score_v2_desc_source_repo_path_line_candidate`
  - `code_search_identifier_typo_osa1_declaration_v2_desc_source_repo_path_line_candidate`
- 이전 order/mode/generation cursor 거절, distinct file 및 byte-budget paging 검증이 있다.
- default 23 miss는 아래 실제 `ordinary` 정책 잔여다. 이를 삭제하려고 기본 정책을 조용히 explicit fuzzy로 교체하지 않는다.

### 4.3 JavaScript grammar/runtime

- upstream tree-sitter-javascript 0.25.0, commit `44c892e0be055ac465d5eeddae6d3e194424e7de`, MIT.
- `_module_export_name`에 `reserved('properties', $.identifier)` 사용. tree-sitter CLI 0.25.10, ABI15 generated C/H를 Rust/Python이 공유한다.
- ABI14 우회는 invalid reserved declaration/import binding을 허용해 **거절했다**. 재도입하지 않는다.
- `QUANTA_GRAMMAR_BUILD_ID`, build input hash, policy grammar tag, Python 외부 compile cache와 library marker로 결속한다. JS에서 unpatched language-pack fallback을 쓰지 않는다.
- `quanta-provenance.json`의 23파일 해시 검증 완료.
- Python pins: tree-sitter 0.25.2, tree-sitter-language-pack 0.10.0, regex 2025.10.23, unicodedata2 17.0.0. 프로젝트/uv/CI/precommit expectation을 함께 반영했다.

## 5. 최신 session hardening: 세 결함 RED → GREEN

근거: [hardening REPORT](/Users/songmin/Documents/code-new/qi-b09-session-hardening-20261004-0snzc83u/REPORT.md).

| 결함 / 귀속 | 재현과 수정 | 닫힌 경계 / 한계 |
| --- | --- | --- |
| fresh OSA join의 고정 옛 runtime pins. 기존 코드 결함이 이번 dependency upgrade로 드러남. | source/active/receipt/capsule이 모두 새 pins로 일치해도 old version literal 때문에 거절. `_source_admission`을 source-lock authority의 4개 package identity 및 cross-receipt equality로 수정. `GOLD_RUNTIME_PACKAGES`를 stdlib `retrieval_contract.py` 단일 소유로 이동. | old 0.23.2/0.9.1 및 new 0.25.2/0.10.0 허용. missing/blank/type/extra/mismatch/tamper는 거절. 실제 runtime을 무조건 최신으로 위장하지 않는다. |
| loaded parser의 old AST에 new grammar digest를 부여. pre-existing cache/provenance 결함. | 격리 프로세스에서 실제 parser load 후 복제한 parser.c의 node name 변경. 새 census identity는 성공하면서 old AST를 쓰던 RED 재현. `_LOADED_SOURCE_DIGESTS`와 dynamic load 전후 generation 검사 추가. | 새 grammar generation은 새 프로세스 필요. 원래 바이트 복구는 가능. warm per-file path에 source scan을 추가하지 않았다. 과거 frozen capture가 오염됐다는 증거는 없다. |
| Python/Rust NL Unicode lowercase 불일치. 이번 normalized keyword emission에서 도입. | CPython UCD15.1 `.lower()`와 Rust UCD17이 U+1C89/U+10D50/U+A7CB에 서로 다른 요청을 생성. Python을 digest-checked 공식 Unicode17 table로 수정. | ASCII fast path, cached map. Rust 전체 1,112,064 valid Unicode scalars를 독립 UCD reference와 대조. 실제 SDK native record를 옛 validator는 거절하고 수정 validator는 수락. |

Unicode table: `/Users/songmin/Documents/code-new/quanta-index/vendor/unicode/17.0.0/lowercase.json`.
1,488 nonidentity mappings, SHA-256 `816553919f8ef756f202f475d00455c4de7039aaf72d934bbbeb81b1c0e870f6`.
공식 `UnicodeData.txt` field13 및 unconditional `SpecialCasing.txt`로 만든 default lowercase다. Greek sigma의 contextual final-sigma, sharp-s의 full casefold와 혼동하지 않는다. 라이선스와 재생성 설명을 함께 저장했다.

최신 hardening에서 **production ranking 로직은 바꾸지 않았다**. 새 품질 향상 수치를 이 세 repair에 귀속하지 않는다. 초기 probe의 경로/cache/Python/import 설정 오류는 도구 오류로 구분했고 제품 행동 RED로 세지 않았다.

## 6. 고정 실행 결과와 시간

아래는 `d1a1b709` frozen 소스, release/all-features runner와 daemon의 관측이다. 현재 dirty main 전체의 새 실행 결과가 아니다.

| cohort / 요청 | 이전 관측 | repaired frozen 관측 | 분모·해석 |
| --- | ---: | ---: | --- |
| Gin exact `keyword_file` | 1192 | **1192/1196** | same-original-name file qrels; 대표 first-span 파일은 별도 1182/1196 |
| OSA default | 4298 | **4340/4363** | 12저장소, 11,695파일; 기본 literal-first |
| OSA explicit | grammar 수정 전 4360 | **4363/4363** | 독립 명시적 OSA1 정책; MRR 1, nDCG 약 0.999926 |
| CLARC original common | 63/425 | **96/425** | 동일 source/query/qrel의 공통 425개 |
| CLARC renamed common | 41/425 | **84/425** | 동일 공통 425개; original과 별도 |
| CodeSearchNet positive-known | 307/408 | **345/408** | 462 submitted 중 54 unknown; 전체 573 중 111 source-blocked |

- CLARC 새 적격은 444/526: original 98/444, renamed 85/444. 새로 들어온 19개는 공통425 before/after와 분리한다.
- 이전 RCA 소스부터 여러 엔진 변경이 있었다. 전체 cohort 차이는 단일 요인의 인과 실험이 아니다. Keyword/Phrase 및 declaration/reference fixture가 각 메커니즘을 독립 검증한다.
- OSA default 잔여 **23건 전부 `execution.mode=ordinary`**. literal 결과가 있어서 automatic correction을 억제했다. explicit은 그 파일을 찾았다. 색인 누락/실행 오류라고 확정하지 않는다.
- Gin 잔여 4질의 `writeContentType`, `ContentType`, `New`, `Write`를 기존 `exact_symbol_name` API로 별도 실행했다. 4개 모두 선언 파일을 반환했다. **symbol unit control**이며 file@10/정확한 name-span metric으로 바꾸거나1196 분모에 더하지 않는다.
- semantic pilot은 이전 snapshot `8ef15426`에서 zero-overlap NL 2질의의 semantic/hybrid 각2응답이다. semantic 1/2, hybrid 0/2의 **10청크** 관측으로 routing/source/model/unit 경계만 확인했다. 넓은 semantic file 품질은 미확인이다.

### 6.1 시간의 경계

| 실행 | 관측 시간 | 의미 |
| --- | ---: | --- |
| frozen release/all-features compile | 217초 | 검색/코퍼스 인덱싱 시간이 아니다. |
| 별도 NL input 준비 | 42.268초 | query/소스 준비; native 호출 합계에 포함하지 않는다. |
| Quanta 33개 fresh capture | **1318.534초** | 11,272응답의 capture process wall; 단일 contended-host 관측 |
| 그 중 Gin1196 SDK 호출 합계 | 6.597초 | p50/p95 4.582/11.379ms |
| Gin lexical build | 4.779초 | 아래 publish/seal/activate 내부에 포함 |
| Gin publish/seal/activate | 5.393초 | lexical build와 합산 금지 |
| Gin capture process wall | 13.775초 | query sum/index 시간과 동일 타이머가 아니다. |
| 별도 exact-symbol 4 controls | 3.875초 | 33개 capture/11,272응답 분모 밖 |
| 최신 Unicode native 2 controls | 1.169622초 | source `d1a1b709`, current Python replay; 새 대규모 benchmark 아님 |
| 과거 5제품 global12 join | 323.800초 | offline join/검증 작업 시간; 제품 검색 시간 아님 |

12저장소별 인덱싱/SDK sum/p50/p95/process wall은 [structural RESULTS](/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR/RESULTS.md)의 33셀 표를 사용한다. contended host의 단발 timing으로 성능 순위를 발표하지 않는다. SDK 왕복, 서버 내부 lexical.search, Semble 프로세스 내부 BM25는 서로 다른 경계다.

### 6.2 과거 외부 5제품 값: 최신 Quanta와 섞지 말 것

기존 원본 21,815 task-product 행을 새 라벨 계약으로 재생한 same-name file Hit@10/4363:

| 제품/기존 요청 | same-name 선언 파일 | 대표 원래 파일 |
| --- | ---: | ---: |
| Quanta | 4298 | 4289 |
| Semble | 3087 | 3037 |
| Sourcegraph | 143 | 140 |
| cs | 141 | 138 |
| OpenGrok | 0 | 0 |

이 표는 **fresh competing-product run이 아니다**. 최신 Quanta 4340/4363 또는 explicit4363과 섞어 최신5제품 순위로 발표하지 않는다. 외부 literal/keyword API 관측이므로 fuzzy UI/지원되지 않는 recovery 능력의 순위도 아니다.

## 7. 병렬 감사의 종료 상태

이번 인계 시 `collaboration.list_agents`에서 아래 3명은 모두 completed다. 진행 중인 background worker라고 표현하지 않는다.

| agent | 완료한 범위 | 남긴 한계 |
| --- | --- | --- |
| `/root/local_bench_gaps` | global12 4363질의/21815응답 join. 삽입1114·삭제1073·치환1087·전치1089 독립 분모 확인. 대표파일 표 Quanta4289/Semble3037/SG140/cs138/OG0 재계산. | diagnostic, 기존 기본 요청, 새 engine/fuzzy/UI/적격 품질 판정 아님. |
| `/root/external_identifier` | 위 원본 join/phase/verdict, 파일/시간 표, manifest input SHA 재대조. | 새 검색/테스트 재실행 없음. native passed8155와 selected/executed8726, 파일 적중을 구별함. |
| `/root/external_robustness` | SG 사후 indexed-scope 증명의 최소 계약 분석. 동일 index tree/container/process의 보존 근거와 12repo/11695파일 universe 구분. | native path inventory/full file extraction 미실행. 당시8/12 완료 관측은 역사적 상태이고 현재12완료 증명이 아니다. |

SG scope 후속은 새 sidecar에서 `after_only`로 수집할 수 있다. 캡처 뒤 만든 receipt를 원래 요청 전후 receipt로 붙이지 않는다. C3/B08의 **13,347파일** 증거를 B09 **11,695파일** universe 증거로 재사용하지 않는다. 현재 `sourcegraph_index_scope.py`는 검증기이고 누락한 원본 native inventory를 자동 생성해 준다는 증거는 없다.

## 8. 코드 소유 지도와 중복 작업 방지

모든 repo-relative 경로의 root는 `/Users/songmin/Documents/code-new/quanta-index`다.

| 영역 | 구현/테스트 소유 파일 | 다음 수정이 필요할 때 경계 |
| --- | --- | --- |
| 라벨/5제품 join | `tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py`, `identifier_robustness_fresh_join.py`; 대응 CI tests | 같은 이름의 전 선언 파일, representative, unit/authority/status/eligible 집합 |
| NL Rust/Python planner | `benchmarks/retrieval/src/query_plan.rs`, `tools/benchmark/retrieval/query_plan.py`, `retrieval_contract.py`; `test_retrieval_benchmark.py`, runner/pair-spec schemas | 실제 요청과 독립 replay commitment를 함께 변경. 최근 UCD17 fix는 host lower 우회 구현 수리다. |
| lexical typo/rank/cursor | `crates/quanta-index-lexical/src/searcher/code_search.rs`, `code_search/ranking.rs`, `tests/l3_exact_source.rs`; search-plane lexical route와 lexical_pages tests | distance/declaration/occurrence, source authority, cap/cancel/paging. 공유 ASCII scan hunk와 구분. |
| grammar/producer/oracle | `vendor/tree-sitter-javascript/**`, `benchmarks/retrieval/{build.rs,src/symbols.rs}`, `declaration_parsers.py`, `source_oracle.py`, `symbol_coverage.py` | generated bytes/runtime/source/load generation 결속. 범용 parser fallback 추가 금지. |
| Unicode reference | `vendor/unicode/17.0.0/**`, Python planner, Rust/Python property tests | 공식 UCD default-lower 계약 유지. runtime lower/table 변경 시 양 producer/consumer 검증. |
| source runtime identity | `tools/benchmark/corpus_binding.py`, `retrieval_contract.py`, fresh join; corpus/source-oracle/join tests | source lock의 4-pin SSOT와 active/receipt/capsule equality |
| proof/의존성/CI | `benchmarks/retrieval/proof-required-tests.json`, `run.py`, Cargo/pyproject/uv, CI/precommit, relevant tests | 공유 integration owner와 협의. test registry collection과 실제 test execution 구별. |

현재 dirty 중 **다른 채팅/소유자 작업을 보존할 경로**:

- `tools/benchmark/retrieval/execution_batch.py`, `holdout_review.py`, `query_timing_overhead.py`, `README.md`.
- `tools/ci/tests/test_holdout_review.py`, B07/B08 tickets, plan README/INDEX.
- `benchmarks/retrieval/src/diagnostics.rs`, `tests/sdk_roundtrip.rs`, `run.py`, proof registry는 공동 소비 경계다.
- ingest observation contract, lexical ingest/open/file_authority, harness scale/open_loop, public-api baselines, J7Q scale/tail tickets.
- `code_search.rs`의 concurrent ASCII scan 최적화는 이번 채팅의 ranking repair와 별도 소유다.

이 목록은 hunk 소유권의 최종 증명이 아니다. 현재 diff와 각 owner 증거를 확인한다. `git add -A`, reset/checkout으로 공유 변경을 통째로 처리하지 않는다.

### 8.1 B07/B08 다른 채팅의 현재 범위

권위 문서는 [B07](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md), [B08](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md), [INDEX](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md)의 최신 섹션이다.

- B07: diagnostic9/protocol7/phase4, completed-response clocks, SDK RPC attribution, ingest clocks, frozen release SDK proof25/25 각2회, 20-capture scanner A/B output/work-counter equivalence가 기록돼 있다. timing은 혼재하며 quiet-host/scale/perf 적격성은 남아 있다. 같은 timer를 재구현하지 말 것.
- B08: supplemental request의 `answerability_min_grade` 누락과 ready sibling을 failed/pending repo가 막던 controller 경계를 다른 owner가 수리했다. 111 owner tests,14실제 요청 payload preflight(모델 호출0), mutation 거절, ready7+failed2 draining preflight(제품 호출0)가 기록돼 있다.
- B08의 typeorm/tailscale unresolved decisions, django/sqlalchemy/zellij 리뷰, supplemental qrels/admission/final scoring는 문서상 남아 있다. 이번 문서 작성 중 실제 controller를 새로 관측하지 않았으므로 terminal/runtime 상태를 최신으로 단정하지 않는다.
- B08에 남아 있는 “B09 join이 old pins를 요구한다” 문장은 **이 채팅의 hardening으로 이미 해결한 과거 항목**이다. 재구현 대신 새 join owner tests/결속과 대조하고 owner 문서 상태만 reconcile한다.
- B08 proof registry에 새 이름을 등록하기 전 해당 concurrent tests31를 실행했고 통과했다. 그 구현을 이 채팅이 작성했다고 주장하지 않는다.

원래 release 속도 조사는 `/Users/songmin/Documents/code-new/qi-gin-lexical-release-20260930-3abc98c9/RELEASE-RCA.md`의 별도 source/timer 실험이다. 현재 B09 release snapshot 또는 정식 배포 qualification과 같은 실험으로 합치지 않는다. 이번 인계 작성에서는 이 오래된 RCA의 수치를 새로 재검산하지 않았다.

Semantica 제품 API 확장/E2E는 Quanta-only lexical benchmark 수리의 필수 작업으로 다시 넣지 않는다. 기존 `select:file`/exact-symbol 연결이 이미 있다는 사실을 확인한 뒤 adapter/record/contract의 재현된 실패 경계만 수정하는 방향으로 정정된 세션이다.

## 9. 검증 영수증: 실행한 것과 미실행한 것

### 9.1 최신 hardening의 owner 검증

Python executable:
`/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR/test-env-v3/bin/python`.
CPython3.13.9와 위 pinned dependencies를 사용했다. repo `.venv`나 과거 실행 env를 덮어쓰지 않는다.

| command / selector | 상태 | 관측과 한계 |
| --- | --- | --- |
| `python -m pytest tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_identifier_robustness_multiproduct_report.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_corpus_binding.py -q` | VERIFIED | 147 passed,118.71s; intentional duplicate ZIP warning1 |
| `python -m pytest tools/ci/tests/test_retrieval_benchmark.py -k 'query_plan or natural_language or diagnostic_v9 or rust_proof' -q` | VERIFIED | 20 passed,6.21s |
| 같은 파일 `-k ucd17_scalar_property` | VERIFIED | 1 passed,1.83s |
| final join/planner selector | VERIFIED | 22 passed,9.02s; 앞 tests와 겹치므로 추가 unique로 더하지 않음 |
| `python -m pytest tools/ci/tests/test_holdout_review.py -k 'admission_queue or supplemental_request' -q` | VERIFIED | 다른 owner의 integration31 passed,1.82s |
| `./scripts/cargow --lane component-source-policy-lane test -p quanta-index-retrieval-bench --lib --bins --locked` | VERIFIED | lib121+bin17=138; build4.79s/test0.52s |
| 같은 front door `test -p quanta-index-lexical --test l3_exact_source --locked` | VERIFIED | 30 passed; build12.52s/test9.04s |
| 같은 front door `test -p quanta-index-search-plane --lib lexical_pages --locked` | VERIFIED | 14 passed; build18.43s/test0.12s |
| 같은 front door `test -p quanta-index-search-plane --lib typed_cursor_tests --locked` | VERIFIED | 3 passed; build0.52s/test0.01s |
| proof inventory collect + `--verify --role python` | VERIFIED | Python authority772 identities와 collection 일치. 772개 전부 실행했다는 뜻은 아님. |
| Ruff8 touched Python files, format3, bench fmt, diff whitespace | VERIFIED | hygiene; 제품/품질 proof 아님 |

Unique 합계는 **Python owned168 + concurrent integration31**, **Rust185**다. 옛 proof750+15subtests 또는 이전 inventory733를 현재 unique 합계에 더하지 않는다.

Rust rails의 당시 환경:

```sh
QUANTA_INDEX_RESOURCE_ADMISSION=0 \
CARGO_BUILD_JOBS=2 \
QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR=1 \
CARGO_TARGET_DIR=/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/component-source-policy-lane \
./scripts/cargow --lane component-source-policy-lane test \
  -p quanta-index-retrieval-bench --lib --bins --locked
```

이 환경은 contended host의 좁은 owner rail을 2jobs로 실행한 기록이다. quiet-host admission/performance를 충족하는 예제가 아니다. 재개할 때 현재 front door/cache authority를 확인한다.

Python 재실행에서는 `PYTHONDONTWRITEBYTECODE=1`, pytest cache/basetemp를 새 외부 root에 둔다. 새 formal proof는 source/binary/input/config를 고정하고 이전 실행 폴더의 출력을 덮어쓰지 않는다.

### 9.2 native controls와 읽기 전용 재검증

- **VERIFIED:** `unicode_control.py`에서 2개 Go-file 고정 fixture, 2개 NL queries를 SDK→daemon→runner로 실행. U1 `Ᲊ 𐵐 Ɤ`→`target.go`, U2 `ΟΣ İ Straße`→`stable.go`. fixture commit `820e71e8e9aff2887b432cf8468fd2c47741c303`.
- **FAILED(의도한 RED):** 동일 native record를 옛 Python planner로 replay하면 U1 request identity 불일치. packet을 편집해 만든 거절 예제가 아니다. 수정 후 VERIFIED.
- **VERIFIED:** 원본/새 OSA **24셀·8,726 task pairs**의 universe/commit/query/name/family/gold/file-qrel/source authority 동일.
- **VERIFIED:** 옛33captures의 **11,272 request identities**가 수정 planner와 일치,0.878s. request-only replay이며 새 검색/전체 source-result replay가 아니다.
- **VERIFIED:** 기존 **367 artifacts**가 recorded SHA와 일치. 옛 receipt와 `/private/tmp/g3`는 변경하지 않았다.
- **이번 문서 작업만 VERIFIED:** git HEAD/dirty/source-bound62files, 증거 문서/owner tickets/완료agent 상태를 읽기 전용으로 확인. 제품 검색/단위테스트는 이 문서 저장 턴에서 재실행하지 않았다.

## 10. 증거의 위치와 사용 범위

### 10.1 최근 RCA/structural execution

RCA: [FINAL-RCA-AND-PLAN.md](/Users/songmin/Documents/code-new/qi-b09-final-audit-20261004-GnGlJM/FINAL-RCA-AND-PLAN.md).

root: `/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR`。

| artifact | 용도 |
| --- | --- |
| `RESULTS.md`, `final-results-v3.json` | cohort별 점수, per-cell timing, residual task traces, 진단 구간 |
| `snapshot-binding-v3.json`, `binary-binding-v3.json` | frozen source/owned file/binary 결속 |
| `fresh-execution-ledger-v3.json`, `native-v3/**` | 33captures의 command/exit/record/score/diagnostics/phases |
| `full-diagnostic-replay-v3.json` | 전체 native diagnostic/ingest/window/phase validator replay |
| `verification-v3.json`, `main-integration-verification.json` | frozen proof와 별도 main integration proof |
| `five-product-label-replay-v2.json` | 21,815개 과거 task-product scores. 새 외부 검색이 아니다. |
| `nl-paired-observations.json` | 공통425와 새 추가19 분리 |
| `svelte-proof-v3.json` | valid JS/56decls와 explicit 잔여3건 |
| `default-literal-policy-residual-v3.json` | default23건의 actual ordinary mode |
| `exact-symbol-controls-v3/execution.json` | 별도4symbol queries와 unit/source 결속 |
| `semantic-pilot-v2/**` | 별도 source의 제한된2-query routing pilot |
| `MANIFEST-v3.sha256` | 367개 보존artifact의 integrity |

실행driver `run_fresh_v3.py`, `finalize_v3.py`, `render_results_v3.py`, `exact_symbol_controls_v3.py`는 외부root에 있다. 이 세션에서 producer/consumer를 확인했다. 옛 `finalize_v3.py`는 query/name/family/source를 assert하지만 각 qrel 자체의 assert는 부족했다. 독립8,726-pair equality 검증으로 해당 proof gap을 닫았다. 원본driver/결과를 사후 수정하지 않았다.

frozen binary SHA-256:

- runner `bin-v3/quanta-index-retrieval-bench`: `ce5c28b6f8e2e2ee7bde1de4c98a252a192f47cb8494e190ec84d6b4865b518a`。
- daemon `bin-v3/quanta-index-searchd`: `5f11101f3bd1e1018a3ad8f8d9360d83345a26c5de255da25f403a457ca33b9b`。

### 10.2 최신 hardening

root: `/Users/songmin/Documents/code-new/qi-b09-session-hardening-20261004-0snzc83u`。

- `REPORT.md`: finding/ownership/intent→owner→consumer→oracle/proof/실행command/잔여.
- `start-state.json`, `proof-state.json`, `final-source-binding.json`: overlay와62reviewedfiles의 범위.
- `unicode-native/{execution,record,pack,manifest,expected,context,diagnostics,phases}.json`: 새2controls의 실제 행.
- `old-validator-rejection.json`: 같은record의 옛planner RED.
- `osa-original-qrel-equality.json`: 8,726pairs의 독립qrel/source 비교.
- `prior-query-identity-replay.json`: 11,272request identities。
- `prior-artifacts-integrity.json`: 기존367artifact integrity.

### 10.3 외부 5제품 historical join

root: `/Users/songmin/Documents/code-new/qi-b09-final-20261004-SQoHAA`。
`five-product-global12-v1.json`, `join-execution.json`이 원래12repo/4,363query join이다.
앞의 same-name 재채점root와 label metric이 다르므로4289/4298을 engine regression이라고 부르지 않는다.

## 11. 미완료 백로그: 구체적인 다음 작업과 완료 조건

미완료 항목에서 새로 확정한 code defect와 실행/증거/적격성 부족을 구별한다. 아래 항목 전체를 engine bug로 취급하지 않는다.

| 우선 | 상태 / owner | 다음 작업 | 완료 조건 |
| --- | --- | --- | --- |
| P0 | NOT_RUN: integration/publication | current HEAD/dirty/hunk owners 재확인. 공유dependency/schema/proof registry를 한 명이 통합하고 owned diff를 공개 가능하게 정리. 영향tests 재실행 후 final source 고정. | owned source/tests review, 변경 범위 일치, 필요한 focused tests/inventory 통과. commit/push는 명시한 범위로 수행. publication과 quality qualification은 별도. |
| P0 | NOT_RUN: B08 review/admission owner | original unresolved evidence를 해결하고 supplemental actual reviewer request와 새decision/qrels/admission까지 진행. 수리한threshold/queue 재구현은 불필요. | 실제model 결과의 독립replay, unresolved pair 목록이 없거나 명시한 제외, query/source/rubric/grade identity 일치. preflight만으로review 완료 선언 금지. |
| P1 | NOT_RUN: capture/report owner | final frozen source에서5제품 행렬을 fresh 실행.1196exact와robustness/NL/no-answer는lane별. required cells를 실제입력에서 열거. | raw status/HTTP/exit, rank unit/order/unique files, blind request/input/source/revision replay. 모든 셀의completed/blocked/not_run 명시. profile별 별도scoreboard. |
| P1 | BLOCKED: external index owner | SG/OG 전체indexed path inventory·native stored bytes/revision/source scope 확보. 새capture에는before/after receipt. | 해당manifest/file hashes와 전체파일 일치. 사후검증은after_only로 기록하고 과거receipt에 혼입하지 않음. B08 13347과B09 11695 분리. |
| P1 | NOT_RUN: independent gold owner | 동명/사용처/test/generated/no-answer/positive grade 독립검사. 사람review 미실시. AI review는AI로 유지. | review provenance와 고정qrel판, ambiguous/excluded 집합, source-attested gold, 독립기대값. 사람이 안 한 검사를human이라고 부르지 않음. |
| P1 | NOT_RUN: span evaluator owner | file hit와 별도로 정확한decl-name/ID/span recovery 평가 연결. 기존unit_id/source indexed span 사용. | 같은줄2선언, 올바른파일의잘못된이름, 사용처만적중, context확대, case fixture의 고정negative/positive를 독립gold로 검증. |
| P1 | NOT_RUN: B08 holdout owner | 미사용repository/query family split, license/commit/source-admission/duplicate exposure 관리로 새holdout 고정. | 이번12repo를 새holdout이라고 바꾸지 않음. tuning/evaluation 노출감사, 미사용분리와coverage/blocked population 명시. |
| P1 | NOT_RUN: B07 performance owner | idle/admitted host, release all-features, 반복warm/cold, SDK끼리/내부끼리 동일경계. index/full/delta/delete/no-op/reopen 개별측정. | queries sum/p50/p95/coverage, build/model prep/index/publish clocks, chunk/index 작업량 명시. 중첩phase 합산 금지. 출력동일성과 속도수용 별도판정. |
| P2 | NOT_RUN: semantic quality owner | lexical잔여에서candidate생성/lane실행·기여/model/source/generation을 추적하고 동일qrels ablation 실행. | zero-overlap pilot을 확장할 독립holdout과file/semantic unit 계약. global RRF/model/chunking 변경은 실패단계를 확정한 후. |
| P2 | NOT_RUN: product policy owner | default23literal-first 잔여를accepted behavior로 둘지 새policy로 바꿀지 판단. prefix/infix/components의 ambiguity/precision도 별도계약. | policy변경 시 독립ambiguity/no-answer/critical-stratum fixtures와 미사용holdout. explicit lane100%를default성공으로 보지 않음. |
| release | NOT_RUN: release/CI owner | final source의SDK/필요한광역rail, hosted CI, 정식build/install/readiness/generation activation/cutover. | 해당실행성공. focused단위/compile/commit/push에서 추정하지 않음. |

### 11.1 인계 후 실행 순서와 병렬 경계

1. integration owner가 현재 HEAD/dirty와 62파일 binding을 읽고 기존 owner diff를 대조한다. 새 출력 root와 final source를 확정한다.
2. **병렬 가능:** benchmark owner의 review/qrels/holdout 준비, external index owner의 native inventory, Quanta owner의 새로 재현한 실패에 대한 unit/RCA. 큰 벤치를 최초 결함 탐색기로 사용하지 않는다.
3. schema/profiles/runtime pins/proof inventory는 단일 integration owner가 합친다. 기존 planner/symbol API/Semble file adapter/review validator를 재사용한다.
4. admission이 완료된 ready repository부터 제품 실행을 직렬로 진행한다. 실패한 repository 때문에 다른 ready 작업을 막지 않는다. derived suite scoring view를 native record라고 부르지 않는다.
5. native 원본/새 qrels/전체 required cells를 replay한다. common eligible 집합과 operational coverage를 붙여 lane별 결과를 출력한다.
6. B07 perf host는 build/index/review 병렬 부하에서 분리한다. 먼저 출력 동일성을 확인하고 이후 반복 측정으로 속도를 판정한다.
7. 정확한 span/미사용 holdout/external scope가 없는 부분은 diagnostic을 유지한다. 필요한 CI/release rail 없이 deployable이라고 선언하지 않는다.

중단/재조사 조건:

- source/binary/query/qrel/unit/profile identity 불일치, 불명확한 parse source coverage, response 누락/중단, duplicate distinct-file response는 typed refusal/제외 사유로 처리한다.
- 검색 결과에 맞춰 원래 gold를 조용히 변경하지 않는다. 개정판/구판/common cohort를 보존한다.
- default에 exact literal 후보가 있다는 것만으로 explicit 교정 평가를 대체하지 않는다. 반대도 동일하다.
- one-off probe/log/실행 binary를 repo 안에 추가하지 않는다. 기존 ticket에는 요약과 외부 root 참조만 남긴다.
- 원래 `/private/tmp/g3`, 옛benchmark captures, clean source snapshots 덮어쓰기 금지.

## 12. 이번 저장 작업의 범위

이번에 실행한 것은 HEAD/status, AGENT_CORE/owner tickets/외부 REPORT 읽기, 완료 agent 상태 확인, 62파일 SHA 일치 확인, 본 문서 작성과 문서 검사다.

- **VERIFIED:** handoff 저장, 내용/참조 경로/Markdown whitespace 검사.
- **NOT_RUN:** 이 문서 저장 턴의 새 product calls/test rerun/benchmark/commit/push/CI/deployment.
- 소스/기존 captures/다른 owner handoff는 이번 작업에서 변경하지 않았다.

다음 담당자는 [B09 ticket](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md)의 최신 structural/hardening 절, 외부 hardening REPORT, 본 문서 11절부터 재개한다. 옛 계획의 미구현 표시보다 현재 구현과 bound proof를 우선한다.
