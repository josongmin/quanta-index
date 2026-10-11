# Lexical 저장·검색·증분 검증 최종 구조안

재감사: 2026-10-10. 소스 기준: 8f11cad9587aec1668a9fb3f5f1f055337487071.
관련 working-tree 변경도 대조했다. 이 HEAD만으로 미커밋 source나 실행 결과를 재현할 수 있다는 주장은 아니다.
상태: **paged 저장·검색 경로 구현, 최종 실행 수용 중**. 현재 구현·검증 범위는
10.28절까지의 기록을 따른다. 실제 제품 전환과 최종 성능 수용은 **NOT_RUN**이다.
현재 Accepted 계약을 아래 목표 구조로 전부 바꿨다는 뜻이 아니다. 검증 의미 변경은 ADR,
producer/consumer, source-closure와 회귀 oracle에 함께 반영한다.

[현재 ledger](README.md)의 후속 범위다. 기존 A/D/C5 완료 조건이나 현재 writer를 대체하지 않는다.
이 문서는 이전의 F15 포맷 유지·쿼리 우선 계획을 대체한다. 유지할 것은 검색 의미와 소유권이며,
F15/Native LQ의 현 디스크 포맷과 전체 검증 호출 구조는 교체 대상이다.

## 1. 최종 판정

현재 구현을 대형 corpus의 최종 구조로 승인하지 않는다. 기존안은 정규식/조회 병목은
다루지만 fixed bucket, flat metadata, 전체 검증, generation별 객체 열거 비용을 남겼다.

권고안은 **크기가 제한된 불변 segment + paged gram routing/현재 문서 표 + 영속 generation root +
읽는 범위의 검증 + 독립 전체 scrub**이다. 검증 타이밍을 명시적으로 바꾸는 설계다.
이는 공개 시스템/연구를 현재 요구에 적용한 설계 판단이며, 성능 1위가 증명된 구현은 아니다.

- query: 하나의 sound regex planner → 비용 기반 posting iterator → 현재 버전 필터 →
  후보 본문 지연 로딩 → exact matcher → 기존 ranking/count/preview.
- update: 변경 문서만 새 segment에 작성 → 문서 버전/coverage/routing의 변경 경로 갱신 →
  변경분 독립 검증 → durable root 게시. 이전 문서는 현재 버전 표로 즉시 무효화한다.
- storage: 파일/청크는 별도 typed namespace와 정규화 의미를 유지한다. 같은 block codec,
  authenticated directory, range reader, cache/accounting을 공유한다.
- lifetime: 새 root가 필요한 객체를 직접 참조한다. 이전 generation들을 따라 delta를
  재생하는 체인이 아니다. 기존 activation CAS, pin, event/retry 소유자를 사용한다.
- operational cost: compaction, scrub, GC를 bounded 작업으로 운영하고 비용을 별도 계측한다.
  foreground에서 빠진 비용을 없어진 비용으로 보고하지 않는다.
- Tantivy BM25, semantic/vector, symbol/graph의 의미와 소유권은 유지한다. 이 계획으로
  자연어 검색 품질이나 전체 retrieval의 SOTA를 주장하지 않는다.

## 2. 적대적 감사 결과

소스 관측과 설계 결함을 구분한다. 아래는 성능 재현 결과가 아니다.

| ID | 관측 / 이전안의 허점 | 근거와 조치 |
| --- | --- | --- |
| A1 | regex-only는 필수 literal이 있어도 전체 scan gate에 걸릴 수 있음 | [CodeSearch](../../../crates/quanta-index-lexical/src/searcher/code_search.rs)의 parse가 추출 plan을 버리고 executor만 유지. canonical RegexPlan을 연결 |
| A2 | 작은 seed 선택 전에 모든 관련 posting payload를 읽음 | 같은 파일의 candidate_ids_observed. descriptor-first + streaming intersection/union |
| A3 | serving에 모든 raw/NFC/folded 본문이 남음 | `file_authority/verify/output.rs::ServingOutput`, [SourceFile](../../../crates/quanta-index-lexical/src/file_authority.rs). 본문뿐 아니라 metadata도 pageable하게 변경 |
| A4 | Native LQ cache miss는 전체 shard decode 경로 | [match_sets](../../../crates/quanta-index-lexical/src/searcher/match_sets.rs), `text_authority/reader.rs`. 본문/trigram/positions가 결합된 CBOR 교체 |
| A5 | fixed 256 bucket은 corpus가 커지면 변경 bucket도 커질 수 있음 | `file_authority/root.rs`의 PREFIX_BITS=8, `file_authority/producer.rs`. append segment와 크기 제한 compaction으로 변경 |
| A6 | F15 root 자체가 모든 source row를 Vec로 보유 | 같은 root의 AuthorityRoot.sources. 본문 lazy만으로 O(files) heap/encode/open은 사라지지 않음. 현재 문서/coverage map의 영속 page root 필요 |
| A7 | inherited object reuse는 이미 존재하지만 whole-generation 검증은 별도 | `file_authority/publication_proof.rs`. 현재 bytes 전체 재인증을 유지하면서 전체 O(delta)를 약속할 수 없음 |
| A8 | hard link도 객체 수만큼 열거/생성하면 비용 발생 | [generation_dir](../../../crates/quanta-index-lexical/src/generation_dir.rs), [coverage ADR](../../adr/SEP-27-003-code-search-source-and-preview-contract.md). 새 lexical 객체는 root 참조로 공유. Tantivy/다른 owner 잔여 비용도 따로 확인 |
| A9 | file 후보를 chunk 후보로 통합하면 Unicode 경계에서 누락 가능 | whole-file NFC와 per-chunk NFC는 다름. 공용 저장 primitive와 공용 결과 universe를 혼동하지 않음 |
| A10 | hash가 맞는 것만으로 posting 누락이 없다고 증명할 수 없음 | 변경 source→postings 관계의 독립 검증, 기존 검증된 base에서의 전이 불변식, full rebuild oracle 필요 |
| A11 | scan/top-k/timeout/캐시 hit를 섞으면 허위 속도 향상 가능 | [stats 계약](../../../crates/quanta-index-core/src/domains/lexical/outbound.rs), [qualification ADR](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md). exact count, 실패 분모, cache 상태를 분리 |
| A12 | overlay만 추가하면 read amplification과 삭제/GC 부담이 무제한 증가 | segment/run 수, obsolete bytes, compaction debt, old pins를 budget으로 제한. backpressure를 포함해 검증 |

A3–A7의 위 경로는 구현 전 flat backend의 역사적 근거다. paged 전환에서 삭제된 파일을
현재 구현 링크로 취급하지 않는다. 새 source owner는 10.6절과 OCT-10-002를 따른다.

A9 반례: 첫 raw chunk는 "e", 두 번째는 U+0301 뒤에 "target"이 이어지는 문자열이면 그 NFC에는
U+0301이 남지만, 합친 파일의 NFC는 "étarget"이어서 U+0301이 없다.
2026-10-10 Python unicodedata로 해당 정규화 차이를 확인했다. 실제 Quanta 회귀 실행은 아니다.

현재 F15의 32,768파일·총 소스 128MiB 등의 policy는 현재 지원 상한이다.
새 포맷은 corpus admission과 resident budget을 분리하되, 지원 상한은 실제 검증 후 변경한다.

### 2.1 추가 적대·객관 감사: 계획의 누락

아래는 **이전 계획의 미정 조건**이다. 현재 코드에서 재현한 장애로 보고하지 않는다.

| ID | 누락 / 반례 | 보완 위치 · 구현 gate |
| --- | --- | --- |
| B1 | approximate 후보의 여집합은 NOT의 정답을 누락시킴 | 6절: negative prefilter 약화 또는 exact child 뒤 complement. LQ1/LQ3 |
| B2 | segment 크기 제한만으로 희소 query의 전체 segment 사전 순회를 막지 못함 | 4.5절: paged gram→segment routing, 갱신 증폭까지 계측. LQ2/LQ5 |
| B3 | segment-local ID 교집합·문서 분산·중복 locator가 누락/허위 hit/중복 count를 만듦 | 4.1절: logical document 배치와 단일 visible locator. LQ2/LQ3 |
| B4 | 현재 run 한도로 publish를 막으면 한도를 줄일 compaction도 설치 못 할 수 있음 | 4.4절: soft trigger, merge reserve, 병합 후 target admission. LQ4/LQ5 |
| B5 | hash한 buffer와 실제 decode/match한 bytes가 다르면 read 검증이 무효 | 5.1절: 같은 owned bytes를 검증·사용. LQ2/LQ3 |
| B6 | bootstrap 쓰기와 이후 쓰기의 rollback 경계, 새 root의 cursor key, launcher 전환이 불명확 | 9절: 기존 offline owner·새 signing key·중단 전환·재시작 oracle. LQ4/LQ5 |
| B7 | 마지막 명령 목록이 8.1절보다 좁아 scrub 등 실행이 빠질 수 있음 | 실행 명령은 8.1절로 단일화. 무거운 저장 구조 변경의 heavy gate 포함 |

객관 감사에서 배제한 중복 설계:

- Native LQ의 [compile_expr/compile_regex_content_leaf](../../../crates/quanta-index-lexical/src/searcher/compile.rs)는
  현재 exact regex match set을 만든 뒤 NOT을 적용한다. B1은 새 pushdown이 보존할 불변식이며
  현재 구현의 NOT 누락을 재현한 결과가 아니다. CodeSearch가 거절하는 문법까지 새로 지원하지 않는다.
- [CursorBindingV2](../../../crates/quanta-index-contract-base/src/results/cursor_envelope.rs)와
  [CursorKeyStore](../../../crates/quanta-index-search-plane/src/query_dispatcher/cursor_key.rs)는 이미
  full pin/query/order/cap binding, 서명·TTL, state-root별 key를 소유한다. 별도 cursor envelope를 만들지 않는다.
- [offline state migration](../../../crates/quanta-index-searchd/src/app/state_migration.rs)은 이미
  source custody, staging/deep-open, manifest-last, rename/fsync를 소유한다. 새 복구 manager를 만들지 않는다.
- 선택적 검증은 미접근 손상 감지 시점을 늦추는 계약 변경이다. 동일한 검증을 더 싸게 했다는
  결론이나 SOTA 판정은 아니다. routing/compaction 비용의 우위도 실행 전에는 미확인이다.

### 2.2 후속 재감사: 실제 재사용 범위와 소비자 경계

| ID | 판정과 근거 | 보완 · 수용 위치 |
| --- | --- | --- |
| C1 | **계획의 재사용 설명 정정.** FileCoverageSnapshot의 imbl 공유 트리는 메모리 구조다. 디스크 coverage_pages는 u8 slot 256개와 최대 4096행 page이며 lazy disk tree가 아니다 | 4절: 재사용 가능한 계약/codec과 새 paged store 구현을 분리. LQ0/LQ2 |
| C2 | **소스에서 확인한 소비자 비용.** regex_match_set은 후보 본문 BTreeMap·Vec·전체 RoaringBitmap을 만들고 AuthorityDocSetQuery가 소비한다. CodeSearch는 전체 ranked Vec를 정렬한 뒤 자른다 | 6.1절: 전체 producer→consumer와 오류 전달, ID 폭, bounded page collector를 함께 이전. LQ2/LQ3 |
| C3 | **설계 누락.** 물리 body block을 독립 regex/NFC 입력으로 쓰면 경계 match·anchor·정규화가 달라진다. 현 RegexExecutor는 한 문서 입력을 받고 engine call 중간에는 취소하지 못한다 | 6.2절: 첫 버전은 admitted logical document 전체를 bounded buffer로 검증. 취소의 실제 단위 명시. LQ2/LQ3 |
| C4 | **새 공유 저장소의 경합 위험.** root pin만으로 GC 중 orphan 객체의 재사용을 보호할 수 없고, 참조 열거에 실패한 root를 무시하면 live 객체가 삭제될 수 있다 | 5.5절: adopt lease, 완전한 mark 조건, 실패 시 sweep 중지. 기존 lifecycle owner에서 구현. LQ4/LQ5 |

C1의 메모리 snapshot 재사용은 유효하다. 재사용 불가인 디스크 기능까지 이미 존재한다고
표현한 부분을 정정한다. C2는 구현 시작 전 비용 경로의 정적 확인이며 OOM/시간 초과 재현이 아니다.
후속 핵심 경로 변경과 실제 daemon 회귀는 10.1절에 기록한다.
C3/C4의 새 block/shared-store 설계에 대한 회귀는 NOT_RUN이다.

### 2.3 호출 경로·평가 범위 재감사

| ID | 확인한 경계 | 보완 |
| --- | --- | --- |
| D1 | 기존 doc allocator는 prior manifest/stored-doc watermark에서 시작한다. 새 계획의 namespace-wide ID 비재사용을 branch/fork/rollback까지 보장하는 allocator는 아님 | 4.6절: 문서버전 ID의 범위·durable high-water·원 attempt 배정을 명시. 현재 generation-isolated 코드의 충돌 재현 주장이 아님 |
| D2 | mutation은 Tantivy commit → text authority → file authority 순서다. crash 후 index가 sidecar보다 앞서면 현재 planner는 full rebuild를 선택 | 5.3절: sealed target과 unsealed attempt를 나누고 원 event/input/ID 배정으로 복구. 단계 중간 재적용으로 중복 쓰기 금지 |
| D3 | 기존 regex cache key는 generation+normalized regex이며 전체 namespace의 match set을 전제로 함. 새 candidate pushdown 결과에는 이 전제가 성립하지 않을 수 있음 | 6.1절: exact membership의 평가 domain을 바인딩. 부분 domain 결과는 shared leaf cache 금지 |
| D4 | 현재 scope replace/tombstone은 incremental이지만 UpsertChunk/ClearLexicalSurface 등은 rebuild 분기가 있음. 한 종류의 delta만 벤치하면 이 경로가 빠짐 | 4.2절: operation별 구현·허용 비용을 고정. 정상 파일 delta에서 rebuild fallback 금지, bulk/recovery는 별도 측정 |

D1/D2/D4의 현재 분기는 [text_authority_plan](../../../crates/quanta-index-lexical/src/text_authority_plan.rs),
[text_docs](../../../crates/quanta-index-lexical/src/text_docs.rs),
[build_ops_inner/write_text_authority](../../../crates/quanta-index-lexical/src/adapter_open.rs)에서 확인했다.
D3은 기존 cache의 재현된 결함이 아니라 새 pushdown이 만드는 위험이다. 현재의 전체-domain
cache나 context-free Unicode fold를 불필요하게 교체하지 않는다.

### 2.4 정렬·파일별 조회·실패 정책의 교차 감사

| ID | 확인한 누락 / 충돌 | 보완 |
| --- | --- | --- |
| E1 | 기존 preview는 선택적 예산 초과에서 hit를 유지한다. 이전 7.2절의 모든 phase 한도 초과 시 실패 규칙은 이 계약과 충돌 | 6.3절: 필수 검색·무결성·취소와 선택적 preview refusal을 구분. aggregate 메모리 회계는 유지 |
| E2 | canonical document-key, logical ID, Tantivy DocId의 정렬을 구분하지 않았다. ID 변환은 순서를 보존하지 않을 수 있음 | 4.1/6.1절: lexical merge 순서는 namespace 내 logical u64로 고정. native bridge는 segment DocId 순서를 별도로 보장 |
| E3 | source→unit 조회를 요구했지만 실제 페이지 표현은 미정이었다. SourceFileCoverage는 unit-set digest만 가지며 삭제할 ID를 열거할 수 없음 | 4.1절: 같은 current-map owner에 paged source membership과 live locator를 함께 바인딩. coverage digest를 lookup/proof로 오인하지 않음 |

E1은 [preview_types](../../../crates/quanta-index-lexical/src/searcher/preview_types.rs),
[selected preview](../../../crates/quanta-index-lexical/src/searcher/candidates.rs),
[L4 회귀 계약](../../../crates/quanta-index-lexical/tests/l4_match_anchored_preview.rs)에서 확인했다.
E2는 현재 [AuthorityDocSetQuery](../../../crates/quanta-index-lexical/src/authority_doc_set.rs)가
lookup 결과를 segment DocId bitmap으로 다시 정렬하는 경계를 새 reader가 보존해야 한다는 뜻이다.
E3의 [unit-set digest](../../../crates/quanta-index-contract/src/source_coverage.rs)는 전체 chunk/symbol
record의 commitment이며 내부 숫자 ID 목록이 아니다. E2/E3은 새 설계의 누락이며 현재 장애 재현이 아니다.

## 3. 공개 근거와 적용 한계

2026-10-10에 확인한 1차 자료다. 공개 자료가 없는 Cursor 내부 protocol은 추정하지 않는다.

| 자료 | 적용할 근거 | 적용 한계 |
| --- | --- | --- |
| [Cursor, Fast regex search, 2026-03-23](https://cursor.com/blog/fast-regex-search) | sparse build/covering, disk postings, 후보 뒤 exact match, commit 기반 변경 layer | build_all은 trigram보다 많은 gram을 만들 수 있음. sparse의 공간/갱신 우위나 내부 crash protocol은 증명되지 않음. 마스크는 Blackbird 선행 설계로 소개 |
| [GitHub Blackbird, 2023](https://github.blog/engineering/the-technology-behind-githubs-new-code-search/) | blob 기반 분산/중복 제거, commit 일관성, 작은 index flush와 compaction, AND/OR iterator 뒤 exact 검증 | 분산 GitHub workload와 로컬 daemon은 다름. 전체 배포 구조를 복제하지 않음 |
| [N-gram selection evaluation, 2025-09 revision](https://arxiv.org/html/2504.12251v2) | FREE/BEST/LPMS 등의 우열이 workload·query 특성에 따라 달라짐 | 코드 검색의 update/restart/GC 품질을 검증한 논문이 아님. 한 기법을 보편적 최적이라 선언하지 않음 |
| [REI: Regular Expression Indexing for Log Analysis, 2025-10](https://arxiv.org/html/2510.10348v1) | 로그의 선택된 n-gram bit-vector와 granularity 설계가 비용에 큰 영향 | 짧고 정형적인 log line 결과를 긴 코드 파일에 전이하지 않음. Quanta에선 비교 후보 |
| [Russ Cox, regex trigram planner](https://swtch.com/~rsc/regexp/regexp4.html) | HIR에서 AND/OR 필요조건을 만들고 복잡한 조건은 보수적으로 약화 | Quanta의 case/NFC/dialect 계약을 별도로 증명해야 함 |
| [RocksDB compaction](https://github.com/facebook/rocksdb/wiki/Compaction) | compaction 정책은 읽기·쓰기·공간 증폭의 교환 | RocksDB 도입을 결정한 것이 아님. write stall과 background 비용을 수용 기준에 포함 |
| [Linux fs-verity](https://docs.kernel.org/filesystems/fsverity.html) | 파일을 읽기 전용으로 만들고 읽은 블록을 검증하는 커널 기능 | 전체 미접근 바이트가 현재 정상이라는 선제 증명은 아님. macOS 공통 기반으로 전제하지 않음 |

독립 비교가 없으므로 “Cursor보다 빠름”, “세계 최고”, “2026 전체 연구의 유일한 SOTA”는
판정하지 않는다. 이 문서의 결론은 알려진 병목과 실패 모델에 맞춘 구현 권고다.

## 4. 저장 구조 선택

| 후보 | 판단 |
| --- | --- |
| 현 F15와 Native LQ에 query 최적화만 추가 | 단기 효과는 있으나 flat root/전체 검증/갱신 증폭을 남김. 최종 구조로 선택하지 않음 |
| 전역 posting COW B-tree 직접 구현 | 작은 변경을 page로 제한 가능하지만 많은 gram의 random rewrite, 트리/codec/recovery 구현 부담. 기본안으로 선택하지 않음 |
| bounded immutable segments + paged live-document map | 변경분 순차 작성, 기존 reader/codec 활용, lazy lookup 가능. **권고안**. compaction 비용은 명시적으로 관리 |
| Tantivy만으로 모든 원문/regex 의미 통합 | source exactness, file/chunk 정규화, span/positions 계약의 대체 근거 부족 |
| Git base + 무제한 overlay/parent chain | read amplification, lineage/retention/GC 결합으로 배제 |
| suffix array 또는 REI로 전체 교체 | 현재의 mutable code workload에 대한 우위 근거 부족. 전체 교체하지 않음 |

이 선택은 새 범용 KV 엔진을 만드는 지시가 아니다. 다만 디스크 page store는 새 구현 범위다.
[FileCoverageSnapshot](../../../crates/quanta-index-contract/src/coverage_snapshot.rs)의 imbl OrdMap은
메모리 내 경로 복사와 snapshot 의미를 제공한다. [coverage_pages](../../../crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs)는
고정 256-slot persistence이며 새로 필요한 disk lookup/split/merge를 이미 제공하지 않는다.
기존의 coverage 계약, bounded codec, hashing, nofollow reader와 snapshot oracle을 재사용한다.
current-map/coverage/routing/segment-set의 disk metadata에는 lexical 내부의 한 bounded COW page 계층을
공유하고 ordered seek, split/merge, crash/root publication을 LQ2에서 실제 구현·검증한다.
메모리 snapshot을 단순 serialize한 결과를 paged disk lookup으로 부르지 않는다.
기존 borrowed get/iter를 그대로 유지하려고 모든 row를 메모리에 올리지 않는다. storage read port는
fallible lookup/iteration과 owned 또는 leased row를 제공하고 caller도 함께 바꾼다. producer의
변경 bundle과 작은 staged map에서의 메모리 snapshot 재사용은 별도 경계로 유지한다.
추가 DB 의존성은 crash/pin/read 비용을 비교하지 않은 상태에서 도입하지 않는다.

### 4.1 한 generation의 구성

~~~text
Published generation root
  ├─ current source/version + coverage root (paged, copy-on-write)
  │    ├─ source → typed live unit membership (paged)
  │    └─ logical document-version → source / visible locator (paged)
  ├─ gram-to-segment routing root (paged, authenticated)
  ├─ segment-set root (paged descriptors, bounded runs)
  │    └─ immutable segments
  │         ├─ authenticated term directory / block descriptors
  │         ├─ blocked postings (+ optional masks)
  │         ├─ independently readable positions
  │         └─ document/body locators
  ├─ immutable source/normalized body blocks
  ├─ Tantivy generation binding
  └─ normalizer / gram strategy / policy / format identities
~~~

- FileDocument, ChunkDocument, path/content/case surfaces를 typed namespace로 구분한다.
  원문 digest가 같아도 repo/path/coverage/권한/검색 결과 identity를 합치지 않는다.
- content digest는 본문 중복 제거에 사용한다. digest lookup만으로 문서 접근 권한을 주지 않는다.
  최초 공유 범위는 같은 state root의 기존 격리 범위다.
- current source map은 source key별 revision token과 live unit-set/coverage를 바인딩한다.
  posting의 revision이 다르거나 source가 삭제됐으면 query에서 제외한다.
- source별 unit membership은 외부 typed unit identity와 내부 logical ID/derived-input binding을
  열거하는 bounded pages다. 한 source row에 무제한 Vec를 넣지 않는다. 이것과 live locator는
  같은 root에서 함께 게시하는 current-map의 조회 방향이며 독립된 두 authority가 아니다.
  source→unit과 unit→source/visible locator의 양방향 완전성을 initial build와 변경 bundle에서
  검사한다. coverage.unit_set_sha256는 원 chunk/symbol record로 검증하고 내부 ID 집합의 hash로
  대체하지 않는다. FileDocument의 source별 단일 행은 producer의 chunk/symbol unit-set에 넣지
  않는다. symbol coverage를 text 문서 ID 집합으로 축소하거나 없는 text 문서를 만들어 맞추지 않는다.
  삭제·replace 비용은 해당 source의 이전/새 unit 수와 변경 경로에 비례할 수 있다.
  이 관계의 보수·검증 비용도 delta에 포함하며 다른 source의 전체 조회로 대신하지 않는다.
- 내부 source/doc-version identity는 삭제·재생성에서 재사용하지 않는다. producer revision이나
  source hash는 같은 값으로 돌아올 수 있으므로 freshness/visibility token으로 단독 사용하지 않는다.
  generation 할당만 기존 C5 owner가 소유하고 source/doc identity는 lexical owner가 소유한다.
- 내부 logical authority ID는 **문서버전**을 식별하는 namespace 내 checked u64이며 segment-local u32와 구분한다.
  Native의 stored u64 field, allocator, decoder, membership, mutation retirement를 함께 이전한다.
  MAX_DOC_ID/u32 변환만 소비자에 남기지 않는다. 외부 candidate ID와 generation 번호는 바꾸지 않는다.
  같은 source key의 변경 버전은 새 ID, 정확히 상속된 버전과 compaction은 기존 ID다. 배정 범위는 4.6절이다.
- segment-local document ID는 압축용이다. 외부 candidate identity와 ranking tie/cursor를
  물리 ID에 의존시키지 않는다. compaction 뒤에도 결과 의미를 유지한다.
  현재 Native LQ의 index-wide u32 watermark를 segment-local ID 한도와 분리한다.
  durable source/revision counter의 overflow는 명시적으로 거절하며 wrap/reuse하지 않는다.
- 첫 포맷은 namespace 내 한 logical document-version의 모든 gram을 한 logical segment에 둔다.
  posting/body/positions의 물리 block 분할은 허용하지만 document를 독립 segment로 쪼개지 않는다.
  단일 문서도 수용 못 하는 segment 한도로 기존 지원 입력을 조용히 제외하지 않는다.
  명시된 최대 document/membership 한도를 넘으면 부분 색인 대신 typed refusal이다.
- current-map은 live document별 단 하나의 visible segment/local-ID locator를 바인딩한다.
  query는 version과 locator를 함께 검사한다. 서로 다른 segment의 local ID를 직접 교집합하지 않고
  같은 namespace의 logical authority ID로 합친 뒤 중복 제거하여 count/rank/cursor에 전달한다.
  compaction은 token을 유지하되 새 root의 locator를 원자적으로 바꾼다. old pin은 옛 locator를 쓴다.
- segment의 local ID는 namespace 내 **logical authority u64 오름차순**으로 할당하고 posting과
  local→logical 변환은 그 순서를 보존한다. 외부 path/candidate key 순서와 혼동하지 않는다.
  compaction도 이 규칙으로 재배치한다. 따라서 logical ID iterator를 bounded fan-in으로 합칠 수 있다.
  allocator의 canonical bundle 순서는 retry 배정용이며 상속 문서까지 path순이라는 뜻이 아니다.
  전체 후보를 HashSet에 materialize해야만 중복 제거되는 구조를 기본 reader로 만들지 않는다.
- body/postings/positions는 독립된 인증 범위다. descriptor는 key 범위, count, encoded/decoded
  byte bounds, offset, digest를 커밋한다. 한 본문을 읽느라 segment 전체를 해시하지 않는다.
- source/current-map/segment 목록도 bounded page로 읽는다. 전체 metadata를 Vec로 펴거나
  전체 source를 build 입력으로 수집하면 locality 수용 실패다.
- normalizer/gram 전략은 한 index root에서 하나로 고정한다. sparse weight/tie/seed가 바뀌면
  명시적 rebuild다. 내부 IR 두 벌이나 legacy reader를 유지하지 않는다.

### 4.2 증분 쓰기와 compaction

1. 기존 event/body/base/target 계약을 검증한다. changed source와 이전 source metadata만 읽는다.
2. 변경 source에서 raw/normalized body, chunk/file postings, positions를 bounded stream으로 만든다.
   high-entropy/대용량 입력은 sort/spill budget을 사용하며 초과 시 게시 전에 거절한다.
3. 새 segment의 source→membership을 독립 verifier로 검사하고, current-map/coverage,
   routing의 변경 경로와 root summary를 갱신한다. 같은 producer 함수를 oracle로 재사용하지 않는다.
4. 새 객체·metadata를 durable하게 쓴 뒤 기존 publication/activation 절차로 root를 게시한다.
   실패/불확정 sync를 성공 ACK로 바꾸지 않는다. replay는 원 event의 root를 반환한다.
5. compaction은 pinned 입력 root의 live revision을 기준으로 새 객체만 준비한다.
   결과는 다음 정상 publication의 unsealed target에만 반영한다. 입력이 바뀌면 재검증/폐기하며,
   이미 sealed된 root나 source-event binding은 변경하지 않는다.
6. 삭제는 current-map에서 즉시 반영한다. 물리 회수는 compaction/GC이며 삭제된 결과가
   compaction을 기다리며 노출되지 않는다.

segment 크기는 문서 수뿐 아니라 encoded bytes·posting memberships·decode scratch로 제한한다.
작은 변경을 무한한 작은 segment로 쌓지 않는다. run count, merge fan-in, pending bytes,
free-space reserve와 동시 merge를 제한하고 admission/backpressure를 제공한다.
run은 merge schedule의 segment 집합이며 segment 자체와 다른 단위다.
run 수 제한만으로 corpus 크기에 비례하는 총 segment 수나 query fan-out을 제한했다고 주장하지 않는다.

compaction은 query가 볼 수 있는 의미를 바꾸지 않는다. 입력 live set과 출력 live set을
streaming 비교한다. 오래된 pin이 참조하는 segment/body를 조기 삭제하지 않는다.
hot repeated replacement, delete-only, rename, 대규모 branch 전환, long pin을 각각 검증한다.
foreground만 빠르고 compaction debt가 계속 증가하면 이 구조의 성능 수용은 실패다.

mutation 분류는 다음과 같이 고정한다. 기존 fallback을 새 저장소 뒤에 숨기지 않는다.

| 입력 | 새 기본 경로 / 허용 비용 |
| --- | --- |
| 파일 replace/tombstone, 일부 namespace 갱신 | current source→unit/locator 경로에서 이전 문서를 찾아 changed bundle 적용. unrelated source 전체 수집 금지 |
| metadata-only/동일 본문 | 4.3절 identity/coverage 변경을 먼저 판정. 영향 없는 derived surface만 재사용 |
| 진짜 no-op | 모든 필수 namespace/binding이 동일한 경우 child roots 재사용. 새 event/target 처리는 유지하되 본문·posting 재작성 없음 |
| legacy UpsertChunk 등 부분 입력 | canonical effective file bundle로 수렴시킬 수 있는 producer/caller와 함께 이전. 불충분한 입력은 mutation 전 typed refusal; 자동 전체 rebuild로 우회 금지 |
| full bootstrap/명시적 전체 surface clear·replace | bulk operation으로 선언하고 streaming build/coverage 갱신. 전체 비용을 파일 하나 delta 결과에 섞지 않음 |
| 불완전 attempt의 recovery | 5.3절의 명시적 복구 경로. rebuild 여부·입력 범위·비용을 정상 delta와 별도 보고 |

현재 text_candidates_at_file의 corpus-sized TopDocs limit만으로 전수 scan이나 O(N) allocation을
단정하지 않는다. 새 retirement lookup은 source→unit map을 사용하며, 이전 경로를 남기면 실제
visited docs/reader opens/heap을 계측하여 범위를 입증한다. 미지원 legacy 경로는 caller와 계약도 함께 닫는다.


### 4.3 source bundle과 identity 규칙

[SourceFileRevision](../../../crates/quanta-index-contract-base/src/source_file.rs)의 producer revision/hash,
[coverage](../../../crates/quanta-index-contract/src/source_coverage.rs)의 unit-set/policy,
[publication event](../../../crates/quanta-index-core/src/domains/source_publication.rs)의 event/target은
서로 다른 identity다. 이를 하나의 content digest나 증가 번호로 대체하지 않는다.

- canonical mutation owner가 이전 bundle과 replace/tombstone/clear를 결합한 **파일별 완전한
  effective bundle**을 만든다. delta의 일부 unit만으로 전체 파일을 덮어쓰지 않는다.
- 내부 doc-version token은 source incarnation과 해당 namespace의 검증된 derived-input identity를
  바인딩한다. namespace policy/unit set이 바뀌면 영향을 받는 postings를 모두 교체한다.
  입력이 동일함을 확인한 namespace만 token과 derived bytes를 재사용한다.
- raw source 부재, 0-byte 파일, zero-unit Complete, NotRequested/Unsupported/ParseFailed는
  서로 다르다. chunk를 이어 붙여 원문을 만들거나 누락 coverage를 Complete로 바꾸지 않는다.
- metadata-only/동일 bytes 새 revision/symbol-clear/rename/delete-recreate를 별도 회귀로 둔다.
  physical dedup은 외부 source identity·coverage·candidate ID를 합치지 않는다.
- 한 개의 namespace token만 바꿔 다른 surface를 오래된 상태로 남기지 않는다.
  file/chunk/symbol/semantic의 paired binding은 기존 producer/search-plane 계약으로 검사한다.

### 4.4 compaction 게시 범위

첫 구현은 **sealed snapshot 불변성**을 유지한다. 같은 generation의 root를 교체하는
새 physical epoch나 별도 maintenance generation allocator를 도입하지 않는다.

- worker는 exact input descriptors + pinned root로 작업하고 결과를 bounded staging에 둔다.
  게시자는 unsealed target에서 입력 segment가 여전히 존재하고 live-view 관계가 맞는지 검증한다.
  compaction에 포함하지 않은 새 segment/새 문서 버전을 삭제하거나 덮어쓸 수 없다.
- target seal 전에 compaction output/root를 확정한다. 봉인 이후에는 이전 root를 포함하여
  어떤 root/segment도 제자리 수정하지 않는다. cache와 cursor는 원 snapshot에 계속 묶인다.
- 동시 게시로 input set이 달라졌으면 조건을 재검증하거나 결과를 폐기한다. merge input과
  output의 중복 가시성, stale output에 의한 문서 재등장, source event 재발급을 금지한다.
- idle 중 준비한 결과는 다음 publication까지 대기한다. **idle active generation의 즉시
  디스크 축소는 첫 버전의 보장이 아니다.** staged output/pins에는 byte/age 한도를 두고 회수한다.
- 입력 root가 가진 obsolete bytes는 compaction 결과가 포함된 새 root 게시와 old pin 해제 후
  회수 가능하다. 운영 metric은 prepared bytes, active obsolete bytes, reclaimable bytes를 구분한다.
- 정상 ingest가 기다리는 compaction은 요청 전체 시간에 포함한다. I/O/scratch/disk reserve를
  확보하지 못하면 target 게시 전에 backpressure 또는 typed refusal. query 결과를 잘라서 넘기지 않는다.
- merge가 바꾸는 live locator/routing 페이지의 작성·검증도 merge 비용이다. 이 작업을
  다음 작은 delta의 seal에서 무계측 전수 갱신으로 숨기지 않는다. activation gate 밖에서
  target에 대한 검증을 끝내고 foreground 시간·총 metadata write budget에 포함한다.
- hard limit 전에 soft threshold로 merge를 시작하고 output·검증 scratch·I/O reserve를 확보한다.
  admission은 prepared merge를 반영한 **target의 예상 run/bytes**를 검사한다. 현재 run 수만 보고
  이를 줄일 publication까지 거절하지 않는다. merge가 획득해야 할 gate를 잡은 채 기다리지 않는다.
- 오래된 pin 때문에 reserve가 회복되지 않거나 입력 재검증이 반복 실패하면 bounded refusal이다.
  무기한 대기나 eventual success를 보장하지 않는다. 한도 직전·도달·초과, 취소와 crash를 시험한다.

### 4.5 Segment routing과 실제 갱신 비용

첫 포맷은 **paged gram→segment map**을 둔다. segment 수만큼 사전을 여는 query 경로를
희소 검색의 기본 경로로 남기지 않는다. doc postings 자체를 전역 COW tree로 옮기는 선택과는 다르다.

- key는 namespace/surface/case/gram-strategy와 canonical gram을 바인딩한다. value는 해당 gram을
  가진 segment들의 paged set이다. 긴 value를 한 Vec로 다시 쓰지 않고 변경 경로만 갱신한다.
  hash key를 쓰면 충돌 항목을 덮어쓰거나 삭제하지 않는다. 원 key 구별 또는 후보 합집합을 보존한다.
- segment-set과 routing은 같은 root에 커밋한다. 변경 segment의 distinct gram으로 membership을
  추가/제거하고, verifier는 새 segment 사전과 routing의 양방향 관계를 검사한다.
  unchanged routing은 검증된 base의 전이로 유지한다. 미등록 새 segment를 게시할 수 없다.
- obsolete 문서를 가진 segment는 후보에 남을 수 있다. 정확한 가시성은 current-map이 판정한다.
  gram 부재/segment 제외는 인증된 경로와 완전한 namespace coverage가 있을 때만 인정한다.
  정책상 미색인 gram, 추출 불가 조건, 미지원 문법은 사전에서 검증된 부재와 다르다.
- lookup은 routing pages + 선택된 segment의 관련 directory/posting blocks를 읽는다.
  common gram의 큰 segment set은 streaming 처리하고 query budget을 넘으면 거절한다.
  모든 문법에 희소 query와 같은 비용을 약속하지 않는다.
- routing은 공짜가 아니다. delta/merge의 distinct grams, 변경 metadata pages/bytes, split/merge,
  검증·fsync 비용을 foreground/background 모두 계측한다. 무작위 변경·고유 gram이 많은 입력으로
  전수 segment probing baseline과 비교한다. read/write/space 예산을 못 맞추면 LQ5 수용 실패이며,
  routing 유지 비용을 숨기거나 검증되지 않은 자동 fallback으로 통과시키지 않는다.

### 4.6 문서버전 ID 배정과 재시도

numeric ID의 의미 범위는 `(state-root incarnation, repository, typed document namespace)`다.
같은 범위의 branch/worktree/retained generation 사이에는 번호를 재사용하지 않는다. base root의
max live ID만으로 다음 번호를 결정하지 않는다. generation 배정은 계속 C5의 기존 owner가 소유한다.

- lexical allocation owner가 해당 범위의 durable high-water를 보유하고, 검증된 변경 bundle에
  필요한 번호 범위를 짧은 배타 구간에서 일괄 확보한다. branch별 build lock만으로 공유 counter를
  보호하지 않는다. 큰 I/O/build/merge는 lock 밖에서 실행한다. reserve/fsync 비용도 update에 포함한다.
- high-water를 durable하게 올린 후 원 event/target의 lexical attempt metadata에 range와
  ordered bundle digest/base/format을 바인딩한다. 이 배정이 durable하기 전 문서를 쓰지 않는다.
  counter sync 후 binding 전 crash는 번호의 빈 구간을 허용한다. 불확정 번호를 다시 배정하지 않는다.
- counter와 range binding은 unsealed generation discard로 지워지는 영역 밖에서 보존한다.
  원 operation의 component metadata이며 event/target 선택 권한은 기존 journal/C5에만 있다.
- binding이 존재하는 retry는 원 범위와 canonical unit 순서를 재사용한다. 다른 입력으로 같은
  range를 쓰거나 완료 target을 재작성하지 않는다. attempt metadata는 기존 publication custody에
  연결하며 별도의 source-event/generation registry를 만들지 않는다.
- delete-all/compaction/rollback/GC는 high-water를 낮추지 않는다. missing/corrupt counter를
  0이나 현재 live max로 복구하지 않는다. 검증 가능한 durable 상태가 없으면 거절·명시적 rebuild다.
- restore는 기존 owner가 새 incarnation으로 fence하며, 옛 namespace의 객체/ID를 새 배정과
  무바인딩으로 섞지 않는다. 검증된 backup의 high-water를 새 namespace에 명시적으로 바인딩하고
  입력 counter가 없으면 live max로 추정하지 않는다. boundary/overflow, sibling branch 동시 배정,
  원 retry와 crash를 검사한다. 독립 rebuild의 비교 기준은 logical 결과이며 새 번호의 byte 일치가 아니다.

## 5. 검증 계약의 의도적 변경

**브레이킹 체인지의 핵심은 포맷뿐 아니라 손상을 발견하는 시점이다.**

현재는 여러 publication/open 경계에서 전체 바이트를 다시 확인한다.
권고안은 정상 요청에 필요한 범위와 변경분을 검증하고 전체 조사는 scrub이 소유한다.
“읽지 않은 파일도 매 요청 전에 손상 없음”이라는 기존 보장은 새 기본 계약에서 제거한다.
이 변경을 숨기거나 동일한 보장으로 표현하지 않는다.

| 경계 | 새 기본 계약 |
| --- | --- |
| initial build / untrusted import | 완전한 source·membership 검증. 현 포맷 밖 입력은 명시적 rebuild |
| delta / seal | trusted committed base + 변경 source/segment의 독립 membership 검증 + 변경 metadata 경로/참조 closure 검증 |
| ordinary reopen | durable root/binding/format/policy/recovery 상태 확인. 전체 본문 재독해 없음 |
| query | 사용하는 metadata, posting, body 범위의 hash/bounds/namespace 검증. 부재·skip·live-version 판단에 쓰는 경로도 검증 |
| scrub | 모든 reachable 객체, semantic membership, live coverage, orphan/inventory를 전체 검사하고 quarantine |
| recovery / invalid root | typed refusal 또는 독립 full scrub/rebuild. 과거 캐시를 정상 상태로 승격하지 않음 |

### 5.1 신뢰와 정확성의 범위

- daemon이 state root의 유일한 정상 writer이며, filesystem의 선언된 durable-write 모델을 전제한다.
  same-UID/root 공격자가 trust root와 임의 데이터를 함께 위조하는 것을 막는 보안 시스템으로
  주장하지 않는다. 필요한 경우 외부 trusted root/플랫폼 저장 capability가 별도 요구다.
- read-only chmod, hard link, inode/mtime, Merkle digest 자체는 저장장치 불변성의 증명이 아니다.
  macOS에서 Linux fs-verity가 있다고 가정하지 않는다.
- 기본 range reader는 bounded owned buffer에 읽고, **그 bytes를 hash한 뒤 같은 buffer를 decode/
  match**한다. 검증 뒤 파일을 다시 읽거나 mutable mmap을 그대로 소비하지 않는다.
  offset/length/decode cap은 allocation 전에 확인한다. 경로는 기존 root custody 아래 해석하고
  symlink/외부 object 참조를 거절한다. mmap 실험도 같은 검증·사용 불변식을 충족해야 한다.
- Merkle/hash는 trusted root에 대한 바이트 관계를 증명한다. posting이 source의 모든 gram을
  포함한다는 의미적 완전성은 producer 검증과 전이 불변식이 소유한다.
- trusted base는 owner가 완전 검증 또는 검증된 전이로 게시한 root만 인정한다. 임의
  persisted “verified=true”나 caller의 digest로 proof를 만들지 않는다. 정상 restart에서는
  durable publication/recovery chain의 custody를 확인하며 매 generation 역재생은 요구하지 않는다.
- 재해석 가능한 page schema, 누락된 namespace/normalizer, 단조성·범위가 깨진 directory는
  게시 전에 거절한다. hash가 맞아도 malformed 구조를 허용하지 않는다.
- 부재 증명에는 authenticated key bounds와 완전한 관련 segment/run 집합이 필요하다.
  Bloom/mask/directory를 손상시켜 후보를 누락시키는 공격도 거절해야 한다.
- 미접근 객체의 손상은 해당 객체를 읽거나 scrub할 때 발견한다. scrub overdue/중단/마지막 완료
  상태를 노출한다. 같은 root의 검증된 cached bytes는 snapshot 데이터로 쓸 수 있으나
  디스크의 현재 건강 상태를 증명한다고 표시하지 않는다.
- scrub/quarantine와 query cache는 기존 owner의 epoch/eviction 규칙으로 연결한다.
  새 객체 이름으로 바뀐 repaired retry에서 오래된 실패나 성공 캐시를 잘못 재사용하지 않는다.

### 5.2 lifecycle와 다른 owner의 전수 비용

metadata, coverage, seal inventory, activation/open이 여전히 전체 source를 열거하면
lexical segment를 바꿔도 end-to-end locality가 성립하지 않는다.
[seal](../../../crates/quanta-index-lexical/src/sealed_generation/seal.rs),
[verify](../../../crates/quanta-index-lexical/src/sealed_generation/verify.rs),
[inventory](../../../crates/quanta-index-lexical/src/inventory.rs),
[coverage](../../../crates/quanta-index-lexical/src/sealed_generation/coverage.rs),
[open](../../../crates/quanta-index-lexical/src/adapter_open.rs)를 함께 추적한다.

기존 pin/activation owner 아래 lexical 객체 공유와 GC를 연결한다. 별도 전역 lifecycle
manager를 만들지 않는다. GC는 active/retained/pinned/staging roots를 보호하고,
mark 이후 게시·pin 등록 경합을 epoch/gate와 unlink 직전 재검사로 처리한다.
전체 orphan 조사를 매 seal에 수행하지 않고 staging ownership과 독립 GC/scrub으로 옮긴다.

Tantivy의 generation-local mutable metadata는 계속 독립적이어야 한다. 새로운 lexical
object sharing을 Tantivy 파일에 무작정 적용하지 않는다. Tantivy inheritance, semantic,
ranked sidecar 등 다른 owner의 잔여 O(N) 경로는 전체 계측 결과에 남긴다.
이 계획만으로 모든 provider의 end-to-end O(delta)를 확정하지 않는다.


### 5.3 durable publication과 실패 전이

아래는 기존 event/journal 상태에 대한 요구이며, 동작이 겹치는 새 journal을 추가하는 지시가 아니다.
현재 [SourceEventState/Phase](../../../crates/quanta-index-core/src/domains/source_publication.rs)의
Allocated, Bound Pending, Staged, Active와 원 operation journal에 매핑한다.

| 경계 | 성공 조건 | 실패·재시작 시 |
| --- | --- | --- |
| reservation/binding | 원 incarnation/event/payload/base/target에 한 번 바인딩 | 같은 event는 원 target. 다른 payload/base는 conflict, 번호 재사용 없음 |
| staged object 작성 | bounded temp 생성, bytes 검증, 파일·필요 디렉터리 durable sync | 중간 객체는 staging 소유이며 query에서 미노출. 원 attempt 복구 또는 정리 |
| lexical seal | 새 root의 변경 closure와 모든 필수 sidecar binding 완성 | root만 존재한다고 ready로 취급 금지. 불완전 seal은 거절 |
| paired stage | lexical/semantic/필수 auxiliary의 정확한 generation binding | 한 track만 성공해도 전체 Active 아님. 완료 target의 finalize 재개 |
| activation | 기존 expected-head CAS 및 source-stream 규칙 충족 | CAS conflict는 inactive target 보존/기존 정책. 임의 rebase·auto-activation 금지 |
| 응답 유실/timeout | durable owner 상태 조회로 확정 | Unknown을 실패로 단정하지 않고 원 key로 replay; 새 event 생성 금지 |

기존 mutation lock/activation gate/pin guard의 순서표를 LQ0에서 작성한다.
build·regex·full scrub·대용량 I/O를 activation 임계구역에 넣지 않는다.
실제 owner API가 요구하는 예외는 이유와 상한을 명시한다.
모든 crash cut에서 catalog/root/ready binding의 old-or-new 일관성을 독립 OS-child 재시작으로 검사한다.

**부분 mutation 복구를 normal open과 분리한다.** 현재처럼 index commit과 sidecar 작성 사이에
중단될 수 있다는 전제로 첫 새 포맷의 정책을 고정한다.

- target이 이미 완전히 sealed됐다면 원 완료 정보를 검증하고 finalize/stage/activate만 재개한다.
  base가 GC됐다는 이유로 완료 target을 다시 build하지 않는다. 상태가 불명확하면 먼저 durable owner를 조회한다.
- unsealed lexical attempt만 기존 discard/rebuild owner 아래 정리하고, **같은 event/target**을
  원 sealed base와 immutable 변경 bundle·4.6절 ID binding으로 다시 구성한다. 첫 구현은 중간
  Tantivy commit을 추측해 mutation을 이어 붙이는 recovery를 채택하지 않는다.
- discard 전에 해당 attempt의 writer/worker를 fence·정지하고 seal/ready 여부를 재확인한다.
  원 event binding/ID 배정, 보호 base, 이미 완료한 다른 track과 shared objects는 삭제하지 않는다.
  완료한 semantic/auxiliary track의 기존 결과를 재사용할 수 있는지는 해당 owner가 판정한다.
- pending recovery가 필요한 base와 입력 content references는 terminal seal/discard 결정까지
  보호한다. 본문을 journal에 복제할 필요는 없지만 동일 bytes를 다시 얻을 수 있어야 한다.
  live checkout/current head에서 입력을 대체하지 않는다. 원 입력 부재는 typed refusal/BLOCKED다.
- crash cuts는 counter→binding, Tantivy commit→text store, text→file/root, seal→receipt,
  receipt→activation을 포함한다. final receipt 유실과 실제 unsealed 실패를 같은 rebuild로 처리하지 않는다.

### 5.4 shared-object quarantine와 scrub

- 같은 객체를 여러 generation이 참조하므로 한 generation만 quarantine하고 끝내지 않는다.
  기존 integrity owner에 object identity와 영향 범위를 연결한다. 알려진 손상 객체를 참조하는
  모든 read는 거절하고, 영향 root의 readiness/cache를 무효화한다.
- query hot path에서 모든 root를 역스캔하지 않는다. object taint 확인과 root quarantine 정리는
  분리하고, 역참조 자료구조는 기존 lifecycle 소유 아래 bounded하게 유지한다.
- scrub은 pinned root의 reachable closure를 검사한다. 다른 root/staging의 객체는 이 root의
  orphan으로 삭제하지 않는다. 전역 orphan 회수는 전체 root set을 아는 GC만 수행한다.
- [기존 scrub progress](../../../crates/quanta-index-core/src/domains/integrity.rs)는 in-process
  cursor custody를 갖는다. 새 page traversal도 root/순서/진행 위치를 바인딩하고 restart에서는
  원 owner의 재시작 규칙을 유지한다. 완료되지 않은 pass를 last-success로 표시하지 않는다.
- repair가 valid cached bytes를 되살리거나 taint를 지우는 조건은 owner가 검증한다.
  같은 digest 이름으로 새 파일을 놓았다는 사실만으로 quarantine을 해제하지 않는다.
- lexical segment/page의 read-integrity와 Tantivy/BM25 등 별도 owner의 기존 검증 범위를
  API/문서에서 구분한다. 이번 변경으로 모든 backend의 query-time byte 검증을 주장하지 않는다.

### 5.5 Shared GC의 mark와 객체 재사용

현재 [reclaim_sealed_generation_with_settlement](../../../crates/quanta-index-lexical/src/adapter_lifecycle.rs)는
generation 디렉터리의 durable retirement를 소유한다. shared object graph의 안전한 회수가
이미 구현돼 있다는 뜻은 아니다. 그 owner 아래 다음 전이를 추가한다.

- 보호 root에는 active/retained/pinned 외에 durable pending/staged event, maintenance attempt,
  pending recovery의 base/input closure, 보존 중인 quarantine root도 포함한다. 원 event의 replay/retention 규칙을 확인하기 전에
  단순 age 만료로 target을 없애지 않는다. 명시적 discard와 자동 orphan sweep은 별개다.
- mark는 신뢰할 수 있는 전체 보호 root set과 참조 closure를 얻었을 때만 완료한다.
  missing/corrupt directory 때문에 참조를 열거하지 못하면 해당 pool의 sweep은 중지한다.
  이를 empty root로 취급하거나 partial mark를 재시작 후 completed로 승격하지 않는다.
  leaf body 손상과 참조 graph 자체의 불명확함은 구분한다.
- writer/compactor가 기존 digest 객체를 재사용할 때는 읽기·검증 전에 pool lifecycle gate 아래
  **adopt lease를 staging 보호 집합에 등록**한다. 단순 exists/hash hit는 수명 증거가 아니다.
  GC의 삭제 선택·unlink와 이 등록을 직렬화한다. query는 기존 pinned root에서만 참조를 얻는다.
- publication은 staging 보호를 durable target root로 인계한 뒤 lease를 해제한다. crash recovery가
  durable root/event/staging 상태를 판정할 때까지 sweep을 재개하지 않는다. 새 journal은 추가하지 않는다.
  재사용하려는 객체가 이미 회수됐으면 검증된 원본으로 재작성하거나 게시 전 거절한다.
- 검증 순서는 `GC selects orphan → writer adopts → GC unlinks → target publish`의 각 경합과
  inverse order, 불완전 mark, 보호 root 등록 중 crash를 포함한다. 결과는 안전한 재사용/재작성/
  refusal 중 하나여야 하며 missing object를 가진 published root는 허용하지 않는다.
- root/lease 수와 mark scratch도 budget을 가진다. mark가 막혔을 때 blocked-GC bytes와 disk pressure를
  노출하고 admission으로 처리한다. live 보호를 풀어 공간 수용을 통과시키지 않는다.

## 6. Query 실행과 메모리

- 기존 validated HIR와 [regex owner](../../../crates/quanta-index-lq-regex/src/literal_extract.rs)를
  canonical bounded Boolean necessary-condition plan으로 확장한다. prefix/suffix/internal literal,
  alternation/concat/repetition을 처리한다. 예산 초과 시 조건을 약화하며 branch를 버리지 않는다.
- (foo|bar)는 OR, 각 literal의 gram은 AND다. 짧거나 추출 불가인 OR branch가 있으면
  전체 조건이 unfilterable일 수 있다. unsupported/resource failure는 빈 결과가 아니다.
- **approximate candidate의 여집합을 NOT 결과로 쓰지 않는다.** M ⊆ P일 때 U−P ⊆ U−M이므로
  false positive를 포함한 P를 뒤집으면 정답이 빠진다. negative prefilter는 U로 약화하고 원식을
  exact 평가하거나, child를 exact 평가한 뒤 완전한 admitted live universe U에서 complement한다.
  A AND NOT B는 A로 좁혀도 B는 exact 판정한다. U는 scope/current-version/coverage가 확정된 집합이다.
  regex의 부정 문자 class와 Boolean NOT을 혼동하지 않는다. 빈 후보여도 전체 문법·권한·coverage
  admission을 생략하지 않는다. 현재 Native LQ NOT의 exact-leaf 계약과 CodeSearch 문법 범위를 유지한다.
- grammar·case·NFC·zero-width admission은 기존 exact matcher 계약을 유지한다.
  Unicode case folding은 검증된 literal-prefix 규칙을 쓰며 단순 lowercase로 바꾸지 않는다.
- descriptor count/bytes와 scope 선택도로 execution order를 정한다. AND는 짧은 목록부터,
  OR는 필요한 모든 branch를 union한다. streaming seek/intersection으로 큰 Vec materialization을 피한다.
  obsolete postings를 포함한 count는 비용 추정에만 쓴다. exact count/exhaustion이나 BM25의
  live-corpus 통계로 재사용하지 않는다. ranking 통계는 같은 pinned generation의 기존 owner가 소유한다.
- 전체 논리식에 대한 필요조건 일부만 조회해도 최종 exact verification으로 모든 원 조건을
  확인하면 sound하다. 무조건 모든 gram을 읽거나, 반대로 OR branch를 생략하면 안 된다.
- 4.5절 routing과 request-local page 조회를 재사용한다. fan-out·metadata 조회도 query budget에
  포함한다. 미색인 term을 검증된 absent term으로 취급하여 segment를 건너뛰지 않는다.
- current revision/coverage로 stale candidate를 제거한 뒤 pinned body를 읽는다.
  live checkout을 열어 snapshot에 섞지 않는다.
- source/body cache, normalization scratch, posting iterator, preview buffers, result cache,
  동시 요청 및 pinned old root의 메모리를 각각 charge한다. 모든 본문을 상주시킬 필요는 없다.
  query/metadata cache는 root incarnation·generation root·query/options/policy를 바인딩한다.
  body cache는 기존 격리 범위 안에서 digest·normalizer로 공유할 수 있으나 scope/권한을 부여하지 않는다.
  branch/worktree 전환은 새 root를 선택하며 서로 다른 root의 current-map을 섞지 않는다.
- literal/regex/typo/components/ranking/preview와 Native LQ substring/phrase/positions를 함께 이전한다.
  whole-file와 chunk normalization은 별개다. path-only 후보 판정에는 본문 read가 필요 없어야 한다.
  최종 선택 row의 필수 source 검증·preview read는 6.3절을 따르고 별도 계측한다.
- count/order/cursor/exhaustion의 exact 계약은 유지한다. top-k가 찼다는 이유로 임의 중단하지 않는다.
  모든 결과가 필요한 common query는 그만큼의 비용을 낸다. 필수 검색 budget 초과 시 typed refusal이다.

### 6.1 Reader 뒤의 전체 집합·정렬 비용 제거

새 reader를 기존 전체 집합 API에 연결하는 것만으로 LQ3를 완료하지 않는다. 대상은
[match_sets](../../../crates/quanta-index-lexical/src/searcher/match_sets.rs),
[restrictions](../../../crates/quanta-index-lexical/src/searcher/restrictions.rs),
[predicates](../../../crates/quanta-index-lexical/src/searcher/predicates.rs),
[AuthorityDocSetQuery](../../../crates/quanta-index-lexical/src/authority_doc_set.rs),
[text_docs](../../../crates/quanta-index-lexical/src/text_docs.rs), regex cache와 모든 호출자다.

- leaf evaluator는 본문을 한 logical document씩 읽고 즉시 해제한다. 후보 본문 BTreeMap,
  전체 candidate Vec, 전체 verified Vec를 연쇄 생성하지 않는다. logical ID 순서의 정확한
  membership을 한 reader 계약으로 제공한다. 작은 집합은 charged pages, 큰 집합은 query-owned
  bounded spill pages를 사용하며 전체 heap bitmap을 필수 중간 표현으로 요구하지 않는다.
  spill 전환은 고정된 in-memory 한도 전에 buffer·I/O 여유를 예약하고 수행한다. 기존
  [collection budget](../../../crates/quanta-index-core/src/domains/lexical/collection_budget.rs)의
  refusal은 sticky이므로 실패한 reserve를 잡아 해제·spill 뒤 성공으로 되돌리지 않는다.
- membership에는 namespace/root/query identity와 **평가 domain**, completion 상태,
  ordered iteration과 exact contains를 제공한다. 모든 read는 fallible이다. NOT에 incomplete/failed 집합을 넘기지 않으며 취소된 집합을
  cache에 게시하지 않는다. spill의 FD/bytes/age/pin은 요청·cache 수명에 맞춰 charge하고 정리한다.
- Complete는 `해당 predicate를 해당 domain 전체에서 판정했다`는 의미다. A 후보 안에서만 계산한
  B의 membership을 namespace 전체의 B로 취급하지 않는다. domain 밖의 ID는 false가 아니라
  명시적인 domain mismatch다. NOT은 같은 domain의 차집합이며 바깥 universe로 임의 확장하지 않는다.
- 첫 구현의 shared leaf cache는 해당 namespace 전체의 admitted universe를 완전히 평가한
  결과만 받는다. pushdown/권한·scope 제한 아래 만든 부분-domain 결과는 request-local이다.
  부분 cache를 더 큰 query에 재사용하는 포함 관계 추론은 도입하지 않는다. 기존 전체-domain
  cache도 root/normalizer/policy 및 query admission을 다시 확인하며 cache hit로 admission을 건너뛰지 않는다.
- Tantivy와는 stable logical authority ID로 결합한다. compaction의 local ID나 이전 DocAddress를
  넘기지 않는다. 같은 pinned generation의 native alive set과 current-map을 함께 존중한다.
  native lookup/scan 비용도 계측한다. lexical routing 개선을 Tantivy segment 조회 전체의 locality로
  승격하지 않는다. 새 bridge가 같은 workload의 전체 query 예산을 못 맞추면 LQ3/LQ5 실패다.
- native DocSet은 **그 Tantivy segment의 DocId 오름차순**이다. logical ID `[10,20]`이 native
  DocId `[7,3]`에 대응할 수 있으므로 lexical iterator를 변환만 해서 advance/seek에 넘기지 않는다.
  lookup 결과는 charged bounded 정렬/merge 또는 spill로 재정렬하고, column scan은 native 순서로
  exact membership을 조회한다. 경로 선택을 admission 전에 고정하고 추가 비용을 예산에 포함한다.
- Tantivy DocSet의 advance/seek가 직접 Result를 돌려주지 않는 경계를 처리한다. page I/O·hash·
  resource 오류를 non-match/TERMINATED만으로 숨기지 않는다. 기존
  [budgeted_search](../../../crates/quanta-index-lexical/src/budgeted_search.rs)의 stop/검사 경로에
  typed fault를 연결하고 최종 결과·exact count·cache 게시 전에 확인한다. explain 경로도 동일하다.
- CodeSearch의 ranked Vec 전체 정렬은 기존 [ranked_page](../../../crates/quanta-index-lexical/src/ranked_page.rs)의
  order/cursor·bounded collector 로직을 공용화해 대체한다. 전체 match는 계속 평가하되 필요한
  page.fetch개 key/witness만 유지하고 선택된 row만 DTO/preview로 만든다. global matches와 cursor
  이후 eligible count를 따로 센다. exact_total의 현재 의미를 바꾸지 않는다.
- 빈 tail page를 전체 no-match로 오인해 typo fallback을 실행하지 않는다. 동일 score/tie,
  cursor가 마지막 뒤인 경우, 중간 read 실패, 공통 질의, spill 한도/취소를 독립 oracle로 검사한다.
- grouped file/owner projection은 별도다. 기존 GroupedPageCollector의 메모리는 group 수에
  비례하므로 page.fetch만으로 상한을 주장하지 않는다. 기존 representative/reducer 의미를 유지한
  bounded spill/reduction 뒤 top-k를 적용하고 group count·순위·취소를 별도 검증한다.

### 6.2 물리 block과 logical 문서의 경계

- body block 분할은 I/O 단위다. regex의 \A/\z, line/word boundary, 장거리 반복과 NFC/fold의
  의미 단위는 기존 file 또는 chunk 문서다. block마다 matcher를 새로 실행하거나 임의의 고정
  overlap을 붙이는 것으로 완전성을 가정하지 않는다. 저장 block과 producer chunk도 다르다.
- 첫 구현은 선택된 **admitted logical document 전체**를 bounded buffer로 조립하고 기존
  normalizer/exact matcher 의미를 유지한다. 한 문서의 raw/normalized/mapped text 공존 비용과
  필수 normalization scratch를 함께 예약한다. 선택적 preview는 6.3절의 별도 admission을 거친다.
  corpus 전체 본문 상주와는 다르다. 현재 file-search의 8MiB
  입력 상한 등 지원 조건은 oracle로 보존하며 무단 축소/partial match로 통과시키지 않는다.
- query read는 이미 저장된 normalized surface를 재사용할 수 있다. normalization은 source 변경
  시 logical 문서 범위에서 완료하며 결합 문자 경계를 임의 block 단위로 끊지 않는다.
  더 큰 문서의 streaming matcher/normalizer는 별도 의미 증명 없이는 지원 확대 근거가 아니다.
- [RegexExecutor](../../../crates/quanta-index-lq-regex/src/executor.rs)의 단일 engine call은
  non-preemptible이다. I/O block·decode·문서 사이와 engine call 전후에 취소를 확인한다.
  timeout을 hard wall-clock deadline으로 광고하지 않고 최대 단일 입력/compiled pattern bounds와
  관측한 cancellation latency를 보고한다. hard deadline이 제품 요구가 되면 별도 실행 격리를 설계한다.

### 6.3 필수 결과와 선택적 preview의 실패 정책

bounded reader로 바꾸면서 기존 선택적 출력 계약까지 강화하지 않는다.

| 경계 | 실패 정책 |
| --- | --- |
| eligibility, exact membership/count, ranking/order/cursor에 필요한 read·work | I/O·hash·decode·예산 오류는 typed failure. 부분 결과를 exact 성공이나 cache entry로 게시 금지 |
| 선택된 row의 필수 source identity·extent·authority/digest 검증 | 기존 mandatory integrity 검사를 유지. source 불일치·손상을 preview 예산 부족으로 덮지 않음 |
| preview만의 work/mapping/retained-output 한도, focus 크기 | 기존 PreviewUnavailableReason과 빈 snippet/highlights로 반환. hit·score·순위·exact count·cursor 유지 |
| 요청 취소·deadline, mandatory ledger 실패 | preview-only 한도와 겹쳐도 mandatory failure가 우선. optional refusal로 변환 금지 |

preview ledger는 mandatory collection ledger와 분리하고, bytes는 실제 수명 동안 aggregate cap에도
반영한다. 이미 실패한 mandatory ledger를 새 ledger로 갈아 끼워 우회하지 않는다. preview 한도를
낮춰도 검색 의미는 같아야 하며 반환된 출력의 reservation은 전송/요청 소유 수명까지 유지한다.
selected body를 읽는 경우 같은 verified bytes를 사용한다. path 판정의 무본문 read 목표를
전체 selected-row integrity 면제로 확대하지 않는다. 기존 Native 선택 chunk는 optional refusal
전에 raw digest를 확인하므로 해당 비용도 최종 page 비용에 포함한다.

## 7. 실행 순서·소유권·완료 조건

설계 선택은 고정한다. 첫 구현의 gram은 현재 trigram, matcher 의미는 현 계약,
저장 구조는 bounded segment와 paged metadata, compaction은 4.4절의 다음 publication 반영이다.
Sparse/mask/REI와 새 검색 API는 첫 제품 수용의 필수 조건이 아니다.

LQ1과 제한된 F16 위 LQ2/LQ3/LQ4의 일부를 구현·검증했다. LQ0–LQ5 전체 수용은 아직 아니다.
문서 완성, 컴파일, 단위 회귀, daemon, 성능 수용을 서로 다른 체크로 관리한다.

| 단계 | 선행 | 변경 owner / 대표 파일 | 산출물과 gate |
| --- | --- | --- | --- |
| LQ0 | 없음 | lexical + ADR/benchmark owner | 계약 전이표, operation별 mutation/read/recovery call graph, limit 설정, baseline·독립 oracle 고정. 코드·문서의 현 검증 계약 차이 목록 |
| LQ1 | LQ0의 의미/인터페이스 | lq-regex, lq-trigram, lexical/regex.rs | canonical HIR 필요조건과 descriptor/iterator API. short OR/NOT/Unicode/unfilterable/admission golden 통과 |
| LQ2 | LQ0 format/limits | lexical/file_authority, text_authority, text_authority_plan, sealed_generation/coverage*, 해당 read port | canonical block codec, 새 disk page store와 doc ID allocation, source bundle writer, independent verifier. split·merge·overflow·delete/fork parity |
| LQ3 | LQ1+LQ2 | searcher, authority_doc_set, text_docs, regex_match_cache, ranked_page, budgeted_search, adapter_open | 모든 file/chunk 소비자와 bounded membership/reader/cache/collector. typed read fault, ranking/count/cursor/preview 및 resource gate |
| LQ4 | LQ2+LQ3 | sealed_generation, generation_dir, inventory, adapter_lifecycle 및 publication caller | seal/open/recovery/quarantine/shared GC/adopt 통합. 원 event replay, paired activation, crash/concurrency, backup/restore oracle |
| LQ5 | LQ2+LQ4 | lexical mutation/maintenance owner + benchmark owner | bounded compaction, backpressure, sustained workload, current-head whole-call 결과. 전환 리허설 통과 |
| LQ6 | LQ5 baseline | gram/codec owner | sparse/masks/압축/mmap/REI 중 한 변형씩 비교. holdout·write/space 비용까지 수용한 것만 채택 |

### 7.1 범위별 체크리스트

**LQ0 — 구현 시작 전에 닫을 경계**

- 현재 Accepted ADR의 source/preview/cost/lifecycle 조항을 새 계약에 대응시킨다.
  [source closure](../../../tools/ci/source_closure.py)와 테스트 inventory 입력도 같이 갱신할 대상으로 지정한다.
- BeforeIntent→UnderOperationLock→build→seal→stage→activate→independent open의 실제 호출을
  연결해 중복/필수 검증과 owner를 표시한다. proof reuse는 exact identity에만 허용한다.
- 내부 type 경계: source bundle, authenticated block descriptor, pinned authority reader,
  domain-bound completed membership, verified changed-segment, candidate root. 이름은 구현에서 정하되 caller가 boolean/digest만으로
  proof를 만들거나 verified segment를 바꾼 뒤 원 proof를 재사용할 수 없어야 한다.
- format version, namespace/normalizer, page bounds, stable ID/overflow, deterministic encoding과
  digest domain separation을 명세한다. offset/length/count의 checked arithmetic을 고정한다.
- run/segment/document의 단위, visible locator, routing membership 전이와 비용 모델을 고정한다.
  corpus·최대 단일 문서·distinct gram 한도, soft/hard merge threshold와 reserve를 함께 설정한다.
- baseline이 없는 수치의 성능 향상을 약속하지 않는다. baseline capture가 자원 때문에 지연돼도
  oracle/계측/codec 구현은 진행할 수 있으나 성능 채택 gate는 통과시키지 않는다.
- 재사용/신규 구현 표를 닫는다. 특히 imbl 메모리 snapshot, fixed coverage disk pages,
  lazy disk view, Native exact membership bridge를 같은 기존 기능으로 합쳐 추정하지 않는다.
- 4.2절의 지원 operation과 모든 실제 producer/caller를 대응시킨다. scoped delta, metadata-only,
  no-op, bulk, recovery의 비용을 구분하며 legacy caller가 남은 채 typed refusal만 추가하지 않는다.

**LQ2 — 자료구조 구현 완료**

- source/current-map은 기존 snapshot 의미를 유지하는 lazy/paged 조회로 연결한다.
  전체 map materialization을 요구하는 port/consumer도 함께 이전한다.
- source별 unit membership이 여러 page를 차지하는 fixture에서 replace/delete/partial namespace
  갱신을 검사한다. hash가 맞는 누락·중복·타 source 연결도 독립 bundle oracle와 불일치하면 거절한다.
- 한 coverage hash prefix로 편향된 입력에서도 fixed 256-slot 한도에 종속되지 않는 page split을
  검증한다. disk read port의 오류를 None/empty로 바꾸는 caller가 없어야 한다.
- directory의 absence/skip도 검증한다. 비어 있는 root, page split/merge, 경계 key,
  단조 ordering, subtree counts/bytes, 참조 cycle/depth와 source unit-set을 검증한다.
- routing 누락/잔여 membership, hash 충돌, 다중 segment의 같은 local ID, 중복 doc-version,
  최대 크기 문서와 block 경계를 넘는 gram을 독립 source oracle로 검사한다.
  hash 직후 외부 bytes가 바뀌어도 검증한 buffer만 소비하거나 typed refusal이어야 한다.
- body locator는 0-byte·큰 파일·범위 경계·Unicode normalization scratch·잘린 입력을 처리한다.
  logical full-body hash와 range hash의 역할을 구분한다. partial read로 full verification을 주장하지 않는다.
- initial build/changed bundle/compaction verifier는 canonical normalizer를 공유할 수 있지만
  같은 posting producer를 불러 expected 결과를 만들지 않는다.
- 4.6절 ID 배정의 sibling branch/rollback/delete-all/restore/crash를 검증한다. 높은 watermark의
  작은 corpus fixture로 동작을 검사하고 u32 경계 테스트를 위해 수십억 문서를 만들지 않는다.
- 새 포맷의 writer/reader/verifier/scrub/inventory/GC를 한 포맷 계약으로 개발한다.
  옛 포맷을 새 verifier에서 조용히 해석하지 않는다.

**LQ3–LQ4 — 소비자와 게시 연결 완료**

- literal/regex, path/content, typo/components, phrase/positions, file explain,
  selected preview, ranking/live BM25, exact count/cursor, strict symbol coverage를 목록으로 추적한다.
- 6.1절의 bitmap/Vec/collector caller를 모두 이전한다. logical ID가 u32::MAX를 넘는 fixture,
  불완전 membership의 NOT 금지, spool read failure와 최종 typed fault 검사를 통과한다.
- path순·배정순·native DocId순이 서로 다른 fixture에서 union/intersection/advance/seek와
  compaction 전후 결과를 검사한다. native lookup/column scan 두 경로가 같은 oracle를 만족해야 한다.
- 6.3절의 preview-only exhaustion에서는 hit/count/order를 유지하고 integrity/cancel에서는 실패한다.
  낮은 메모리에서의 정상 spill 전환과 sticky refusal 뒤 성공 금지를 각각 검사한다.
- subset-domain B 평가 뒤 global B/NOT B를 실행하는 순서·역순·cache hit/miss를 검사한다.
  partial membership의 completion flag만 보고 global cache나 complement가 허용되면 실패다.
- regex/NFC는 block 분할 전후 결과가 같아야 한다. 전체 문서에만 성립하는 match/anchor와
  긴 결합 문자열, 입력 최대 크기·compiled pattern 한도에서 cancellation latency를 확인한다.
- shared physical storage가 repo/branch/worktree/track 간 scope를 넓히지 않는지 검사한다.
  query pin을 얻기 전 root가 reclaim되는 경합, query 취소 시 buffer/pin 반환을 검사한다.
- generation을 open하는 모든 문을 나열한다: direct open, proven open, activation,
  restart inventory, rollback/retained selection, SDK/daemon 경로. 한 문만 full scan을 남기지 않는다.
- full scrub은 foreground gate와 별도 API 의미를 가진다. 요청 read에서 발견한 corruption은
  기존 durable quarantine owner로 전달하고 timeout/cancel을 corruption으로 오분류하지 않는다.
- custom object pool 공유는 lexical owner 범위다. semantic/RepoMap을 새 manager로 이동하지 않는다.
  다른 track과의 결합은 기존 paired publication 계약을 사용한다.
- GC mark 실패와 orphan adopt/unlink 경합을 5.5절 기준으로 검사한다. lifetime API와 pin/GC
  호출자가 함께 이전되기 전 generation 디렉터리 삭제를 shared-object reclamation으로 바꾸지 않는다.
- index-only commit, sidecar-only 준비, sealed/receipt 유실의 각 상태에서 5.3절 recovery를 실행한다.
  recovery base의 GC 경합과 입력 부재를 포함하고, 완료한 다른 track을 중복 build하지 않는지 검증한다.

**LQ5 — 통합·운영 수용 완료**

- source event repeated replace/delete/rename, no-op, empty corpus, branch switch,
  중복 content, 긴 pin, slow storage, ENOSPC/short-write/fsync failure를 실행한다.
- 지속 부하에서 최소 세 번의 merge 주기와 idle/staging cleanup을 관측한다. 계획된 최대 support에서
  비용을 지배하는 merge 크기/level도 포함한다. L0의 작은 merge 세 번만으로 steady-state를 주장하지 않는다.
- compaction 대기·foreground merge와 background CPU/I/O를 측정에 포함한다.
  동시 query 아래 run/pending/obsolete/pinned bytes와 debt 증가율이 선언한 예산 안에서 유지돼야 한다.
  입력률이 처리능력을 넘으면 admission이 작동해야 하며, 거절률을 감춘 처리량을 보고하지 않는다.
- 9절의 새 state-root bootstrap·중단·복원 리허설을 독립 프로세스에서 완료한다.
- LQ0–LQ5 전부 수용한 묶음만 기본 경로로 전환한다. half-writer/half-reader 배포 금지.

### 7.2 반드시 명시할 자원 정책

LQ0에서 baseline과 지원 fixture를 근거로 아래 값을 설정하고, candidate 측정 전에 고정한다.
결정 owner는 해당 구현 owner이며 미정/누락 값으로 production default를 만들지 않는다.

| 예산 | 적용 위치 / 소진 동작 |
| --- | --- |
| root/page/segment encoded bytes, decoded rows/depth/memberships | allocation·decode·write 전 검사. 단일 document admission과 일치. typed refusal |
| normalized body, regex plan, postings, membership/spill, mandatory results | work는 phase 간 누적, bytes는 실제 보유 수명·동시 peak로 회계. 단일 logical body·group reducer·spill 전환 여유 포함. 취소/한도에서 exact 성공 금지 |
| optional preview scratch/output | 별도 ledger와 aggregate cap 적용. preview-only refusal은 hit 유지, integrity/cancel/mandatory failure는 전파. 6.3절 |
| shared cache + 동시 query/build/maintenance heap | 개별 요청 한도 외 aggregate admission. lifetime 끝까지 charge |
| routing pages/segment fan-out + 변경 metadata bytes | query/write 각각 누적 계측. 전수 probing/serialization로 숨기지 않음 |
| runs/merge fan-in/pending bytes, staging age/bytes | soft trigger·병합 후 target admission·reserved merge capacity. bounded wait 또는 publish 전 거절 |
| disk reserve | active+retained+pinned+staging+merge output+spill+전환 backup 포함. 여유 없는 merge 시작 금지 |
| FD/mmap/old pins/history retention | owner별 상한. 오래된 reader를 강제 파괴해 GC 성공으로 만들지 않음 |
| scrub step/최대 지연 | 기존 scheduler 안에서 quota·진행·overdue 노출. skip을 completed로 바꾸지 않음 |

corpus admission, runtime cache budget, format bounds는 같은 숫자가 아니다.
runtime budget 변경으로 이미 봉인된 format/normalizer 의미를 바꾸지 않는다.

### 7.3 작업 분리와 통합

- planner lane(LQ1), storage lane(LQ2), query consumer lane(LQ3)을 인터페이스 고정 후
  병렬로 배정했다. 빌드·실행은 루트 owner가 직렬로 통합한다.
- code_search.rs/port.rs는 query owner, match_sets.rs는 루트 owner가 수정한다.
  adapter_open.rs의 연결은 별도 인계 없이 중복 수정하지 않는다.
  format/manifest/coverage와 lifecycle 공용 파일도 writer를 한 명으로 둔다.
- 기존 A/D/C5 owner의 변경을 먼저 대조하고 필요한 타입/동작만 인계받는다.
  reservation, semantic compatibility, RepoMap pin을 중복 구현하지 않는다.
- compile/XL benchmark는 host admission 아래 직렬 실행한다. 서로 다른 HEAD의 테스트/벤치를 합치지 않는다.
- 실패한 단계는 원인·영향 범위·필요한 재실행을 기록한다. 관련 없는 green을 재실행하며 완료를 미루지 않는다.

### 7.4 조건부 실험

- Sparse: weight/tie/normalizer 고정, training/dev와 holdout/unseen 분리, no-false-negative.
- Masks: collision은 false positive만. immutable build/compaction에서 occurrence-safe 재구성.
- 압축: sparse list delta/block와 dense bitmap을 byte·decode·seek 비용으로 비교.
- mmap: verified directory의 대안. pin/unlink·page fault·RSS와 외부 mutation 조건 유지.
- REI/signature: 실제 code workload에서 granularity/coverage를 비교. 미색인 gram은 pass-through.

LQ6를 채택할 때만 strategy identity/format을 변경하고 재색인한다. 선택하지 않은 실험의
코드와 설정을 영구 fallback 엔진으로 남기지 않는다.

## 8. 적대적 acceptance와 성능 기준

| 공격 / 실패 조건 | 독립 oracle / 수용 기준 |
| --- | --- |
| regex optional/short OR branch, leading wildcard, Unicode fold | prefilter 없는 admitted matcher와 완전 동등 |
| gram false positive 아래 NOT/AND NOT/nested OR NOT | 완전한 live universe의 exact oracle와 동등. invalid branch가 빈 후보에 숨지 않음 |
| A 안에서 B 평가 후 global B/NOT B, cache 재사용 | membership domain 바인딩·부분 cache 거절. 원 unrestricted oracle와 동등 |
| source 변경/삭제 뒤 이전 segment에 hit가 남음 | current-map revision 판정으로 즉시 제외. old pinned query에는 원 결과 유지 |
| compaction이 revision/순위/중복을 바꿈 | 입력 live set과 출력 live set, ranking/count/cursor 비교 |
| segment-local ID 충돌, 같은 version의 다중 locator, block 경계 gram | canonical document/locator oracle. 누락·허위 hit·중복 count 없음 |
| path/key순과 logical ID순 불일치, native ID 변환 뒤 비단조 순서 | lexical merge와 native advance/seek 각 정렬 계약 검사. lookup/scan 및 compaction 결과 동등 |
| multi-page source membership의 누락·중복·다른 source 연결 | independent effective bundle과 양방향 locator 검증. scoped delta에서 unrelated source scan 없음 |
| routing에서 새 segment가 빠짐/없는 gram의 경로 손상 | changed-segment 양방향 membership 검사·authenticated absence. corpus scan으로 실패를 숨기지 않음 |
| 파일과 chunk 경계 NFC가 다름 | 별도 namespace golden. 파일 prefilter를 chunk oracle로 사용 금지 |
| storage block을 가로지르는 regex/anchor/NFC와 최대 입력 | unblocked logical document oracle와 동등. 고정 overlap/partial input으로 대체 금지 |
| logical ID > u32::MAX, bitmap cache 경계, native read 실패 | namespace/pin별 stable ID 정확성, typed fault, incomplete NOT/cache 게시 금지 |
| 같은 base에서 sibling ID 배정, delete-all 뒤 재배정, rollback/restore | 범위 내 ID 비재사용, 원 binding retry 동일성, counter corruption에서 추정 금지 |
| common match·동점·group projection·cursor 뒤 빈 page | 기존 전체 정렬 oracle와 count/order/window 일치. bounded heap/spill, typo fallback 오발동 없음 |
| preview 한도 초과와 corruption/cancel 동시 발생, spill 전환 중 한도 | preview-only는 hit 유지. 필수 실패 우선, sticky 실패 리셋 금지, bytes/FD 반환 |
| 해시가 맞지만 posting이 빠진 새 segment | source 기반 독립 membership 검증과 full rebuild 비교로 게시 전 거절 |
| 존재하지 않는 key처럼 보이게 directory/filter 손상 | 부재/skip 경로의 인증·ordering·bounds 검증 후 typed corruption |
| unused object same-length corruption | unrelated query 선제 감지는 새 계약 밖. 해당 read와 full scrub은 반드시 거절 |
| range hash 후 decode/match 전에 backing bytes 변경 | 검증된 owned snapshot 사용 또는 typed refusal. 미검증 새 bytes 소비 금지 |
| root/segment write·fsync·rename/CAS 각 crash cut | old 또는 new complete root, missing mixed snapshot 금지, 원 event replay |
| index commit 후 sidecar 전 crash, 완료 receipt 유실 | unsealed만 원 input/base/ID로 rebuild. sealed는 finalize 재개. 중복 mutation/다른 track 재작성 없음 |
| publish/compaction/GC/query pin 경쟁 | barrier로 순서 고정. live/old pin 참조 객체 unlink 금지 |
| GC 대상 orphan adopt, unreadable 보호 root, mark 중 crash | 보호 인계가 먼저이거나 writer 거절. incomplete mark에서는 sweep 없음 |
| long pin + update flood + disk-full | bounded admission/backpressure, 실패 ACK와 Unknown 구분, recover/reclaim |
| hard run limit에서 준비된 merge 설치, publication 취소 | 병합 후 target admission, gate 교착 없음, 취소 후 reserve/pin 반환 |
| 새 root에 옛 cursor/key·pending journal 유입, 전환 중 crash | 기존 key/activation owner에서 거절. old/new launcher 중 한 개만 재개, confirmed writes 유실 없음 |
| oversize/entropy/CBOR·offset·codec 공격 | allocation 전 한도, canonical decode, cancellation, typed refusal |
| cache hit가 corruption/failure를 숨김 | cache identity/epoch/eviction, retry/repaired-root를 별도 검증 |

기존 [regex](../../../crates/quanta-index-lexical/tests/regex_literal_alternation.rs),
[Unicode](../../../crates/quanta-index-lexical/tests/unicode_normalization_goldens.rs),
[F15 parity](../../../crates/quanta-index-lexical/tests/f15_file_authority.rs),
[text shards](../../../crates/quanta-index-lexical/tests/text_authority_shards.rs),
[source](../../../crates/quanta-index-lexical/tests/l3_exact_source.rs),
[preview](../../../crates/quanta-index-lexical/tests/l4_match_anchored_preview.rs),
[resident](../../../crates/quanta-index-lexical/tests/resident_bytes.rs) oracle을 새 owner로 이전/확장한다.
포맷을 바꾼다고 기존 누락·변조·수명 회귀를 삭제하여 통과시키지 않는다.
감지 시점이 바뀐 테스트는 새 계약의 관련 read/scrub 거절과 함께 명시적으로 교체한다.

성능 수용:

- 기존 [workflow](../../../tools/benchmark/code_search_workflow.py),
  [matrix](../../../tools/benchmark/code_search_matrix.py),
  [causal capture](../../../tools/benchmark/retrieval/causal_cost_capture.py)를 사용/확장한다.
- small/현재 XL에서 시작해 검증된 support를 넓힌다. 같은 변경 크기로 corpus를 증가시켜
  changed bytes, routing/root/metadata pages, opened segments, postings/body read bytes, syscalls,
  peak heap/RSS를 비교한다. 변경 bytes가 같아도 distinct grams와 merge work는 별도 변수다.
- full build, no-op, 1-file replace, delete, burst, sustained churn, reopen, scrub/GC/compaction을 측정한다.
  inactive retention/old pins와 정상 background maintenance를 포함한다.
- 지원 operation별 실행 분기를 기록한다. 정상 파일 delta의 authority rebuild 횟수는 0이어야 한다.
  metadata-only/no-op, bulk clear, crash recovery가 실행되지 않았으면 각각 NOT_RUN으로 남긴다.
  allocation reserve/fsync, retirement lookup, input retention과 recovery 비용도 stage 누락 없이 보고한다.
- deterministic work gate: 정상 lexical delta에서 unrelated source body 전체 순회가 없어야 한다.
  prefilter가 제외한 문서의 body decode=0, positions를 요구하지 않는 경로의 positions decode=0.
  false-positive 후보의 exact verification read는 허용한다. flat full-source serialization이 남으면 실패다.
  희소/absent gram query가 unrelated segment 사전을 전수 open하면 routing locality 실패다.
  compaction은 입력 범위에 비례한 작업을 하므로 이를 숨겨 전체 update의 최악 O(delta)를 주장하지 않는다.
  body cache 뒤 Native membership/CodeSearch 정렬/group reduction에서 전체 집합을 heap으로
  되살리면 bounded serving 실패다. spill bytes와 native lookup/scan work도 결과에 포함한다.
- 실제 시간은 client 시작→decoded output와 server stage를 나눈다. p50/p95와 불확실성,
  startup/freshness, read/write/space amplification, metadata 비용과 실패율을 함께 보고한다.
- process cold, warm/new query, result-cache hit를 분리한다. OS cache를 비우지 않았다면 cold-disk라고
  표시하지 않는다. 캐시/실패/권한/ignore/normalizer 차이가 있는 결과를 같은 셀로 비교하지 않는다.
- query strata: rare/common/absent, regex-only, path, multilingual/combining, typo, unseen regex.
  전략 선택에 사용한 training/dev 질의와 holdout을 분리한다.
- baseline 이후 candidate 결과를 보기 전에 latency/RSS/disk/compaction-debt 예산을 고정한다.
  상한 확대·fixture 축소·실패 제거로 개선을 만들지 않는다.
- 새 검증 계약과 옛 계약을 비교할 때 integrity timing을 명시한다. 같은 byte/refusal 계약의
  query-only ablation과 새 전체 구조의 system comparison을 분리한다.
- publication 시간만 줄고 activation/open·다른 provider·background 비용으로 이동하면
  전체 개선으로 수용하지 않는다. lexical의 locality를 전체 ingest O(delta)로 승격하지 않는다.


### 8.1 실제 실행 범위와 테스트 매핑

[test-authority.toml](../../../tools/ci/test-authority.toml)의 현재 integration-storage scope는
lexical-text-authority-shards 하나다. 이 profile 통과만으로 F15/preview/resource/GC를 검증했다고
표시하지 않는다. 기본 daemon targets에 없는 integrity-scrub은 별도 선택 또는 daemon-all로 실행한다.

| gate | 현재 재사용할 inventory ID / owning suite | 추가할 독립 oracle |
| --- | --- | --- |
| query 의미 | lexical-l1-query-domain-window, lexical-l3-exact-source, lexical-l4-match-anchored-preview, contract-lexical-cursor | regex exhaustive parity, native/file 경계, stale version·exact count·tie/cursor |
| writer/format | lexical-l2-file-mutation, lexical-f15-file-authority, lexical-text-authority-shards, lexical-sealed-manifest | paged root split/merge, malformed hashed block, partial namespace update, independent full rebuild |
| resources | lexical-resident-bytes, lexical-regex-cache-bounds, lexical-sealed-commitment-cost | aggregate concurrency, cancellation release, FD/disk/spill cap, source/read syscall locality |
| recovery/GC | lexical-boot-inventory, searchd-runtime-e2e-physical-gc, searchd-runtime-e2e-boot-quarantine | shared-object fan-out quarantine, staging ownership, snapshot pin/GC race, power-loss cuts |
| scrub | searchd-runtime-e2e-integrity-scrub (runtime_extended_suite) | selected/unselected corruption, resume/root mismatch, interrupted pass, repaired object |
| product | restart/replay, exact-count/window, keyset-cursor 및 paired publication owner | 새 포맷 SDK 요청, strict coverage, old handle/cursor 거절, 실제 producer bootstrap/restore |

LQ0에서 각 ID의 실제 target/필터/ignored 상태를 다시 확인하고 새 테스트를 canonical inventory에
등록한다. 파일이 존재하거나 suite가 컴파일됐다는 사실은 실행 증거가 아니다.
source→test 선택기가 새 codec/lifecycle 파일을 놓치지 않는 회귀도 포함한다.

구현 단계에서는 ./scripts/cargow로 owner unit/property와 필요한 integration target을 선택한다.
통합 단계에서는 just rust-profile test-integration 및 just rust-profile test-daemon-all로 확대한다.
최종 기본 경로 전환 전 just rust-profile verify-rust와 public API/module gate를 실행한다.
새 binary codec/정규식 planner에는 bounded decoder/필터 soundness fuzz를 추가하고,
posting 한 항목 제거, OR branch 누락, approximate NOT, routing 누락, stale token 허용,
pin 확인 생략이 oracle에 잡히는지 확인한다. 저장·검증 경계를 바꾸는 이번 구현은
[Justfile](../../../Justfile)의 load-bearing 지침에 따라 just rust-profile verify-rust-heavy도
최종 merge 전에 실행한다. nightly/tool 부재는 BLOCKED이며 focused green으로 대체하지 않는다.
새 unsafe/mmap을 채택하면 기존 heavy rail이 그 owner 경로를 실제 다루는지도 확인한다.
이 절의 전체 수용 명령·테스트는 실행 계획이다. 부분 실행 결과는 10.1절에 별도로 기록한다.

### 8.2 완료 판정

LQ0–LQ5의 설계·코드·focused regression·daemon/producer·성능·전환 리허설을 각각 기록한다.
실행된 실패는 FAILED, 필수 입력 부재는 BLOCKED, 미실행은 NOT_RUN으로 남긴다.
모든 필수 셀이 VERIFIED이고 알려진 정확성/복구 실패가 없어야 제품 전환 완료다.
LQ6 미채택 전략은 NOT_APPLICABLE로 구분한다. 선택한 전략의 미실행을 NOT_APPLICABLE로 숨기지 않는다.
구현 완료만으로 성능 완료 또는 SOTA를 선언하지 않는다.

## 9. 포맷 전환·백업·복구

첫 전환은 **새 state root에 full bootstrap 후 중단 전환**이다.
기존 디렉터리 안에서 부분 migration하거나, old/new reader를 함께 운영하는 방식은 사용하지 않는다.
전환 명령 실행은 이 계획 작성에 포함하지 않는다.

1. 유지보수 구간에 producer publication을 멈추고 미확정 작업을 원 root에서 settle한다.
   Unknown의 성공/실패를 추측하지 않는다. durable 결과가 확정되지 않으면 전환을 막는다.
   기존 binary/config/format과 state root를 보존하고 producer의 immutable source snapshot/event
   위치를 기록한다. 기존 daemon의 mutation/GC도 중단한다. catalog는
   [기존 SQLite backup owner](../../../crates/quanta-index-catalog/src/backup.rs)를
   사용한다. catalog 파일과 WAL을 임의 복사하여 backup으로 인정하지 않는다.
2. 새 incarnation의 state root에 현재 producer snapshot을 full bootstrap한다.
   lexical뿐 아니라 해당 제품이 요구하는 semantic/auxiliary binding도 완성한다.
   옛 root의 completion/event record를 복사하여 새 target이 완료됐다고 만들지 않는다.
3. bootstrap 동안 새 producer publication은 허용하지 않는다. 새 source 변경의 미처리 위치를
   producer가 보존하고, bootstrap 입력 위치와 새 root의 active binding 일치를 확인한다.
   새 root의 bootstrap은 새 stream/event로 바인딩한다. 옛 root용 완료·할당·pending journal을
   새 root에 replay하지 않는다. resume 시 producer의 기존 reconciliation으로 이후 변경을 반영한다.
   연속성이 불명확하면 새 완전 snapshot으로 재구축하며 checkpoint를 추측하지 않는다.
4. 새 root의 independent open, scrub, paired activation, SDK 실제 query, sentinel golden,
   freshness와 resource smoke가 통과한 뒤 daemon을 중단하고 새 root/binary로 재시작한다.
   기존 CursorKeyStore가 새 root에서 새 key를 만들게 하며 old keys/를 복사하지 않는다.
   SDK handle은 다시 얻고 old cursor는 기존 서명/context 검사에서 typed refusal이어야 한다.
5. bootstrap 이후 source event의 새 publication을 허용하기 전 실패하면 기존 binary/config/root로
   복귀할 수 있다. **bootstrap 자체의 쓰기**와 이후 새 source publication을 구분한다.
   이후 publication이 확정됐거나 결과가 Unknown이면 단순 경로 rollback을 허용하지 않는다.
   producer의 현재 완전 snapshot으로 이전 지원 포맷을 재구축하거나 forward repair한다.
6. 기존 history/pins는 새 포맷으로 자동 이관하지 않는다. 옛 history는 보존된 old root와
   대응 binary로만 복원한다. 첫 전환의 history/cursor 단절을 제품 계약에 명시한다.
   old root/backup의 자동 삭제는 하지 않는다.

새 포맷 bootstrap의 staging/finalization은 기존 offline migration owner를 확장한다.
완성된 destination 게시에는 기존 manifest-last → rename → parent fsync 계약을 사용한다.
현재 restore-state가 새 포맷 변환까지 구현했다는 뜻은 아니다. 아래 전환 경계도 LQ4/LQ5 필수다.

- destination은 비어 있는 새 경로다. 기존 root를 rename으로 덮어쓰지 않는다.
- 서비스 launcher의 binary/config/root 선택은 하나의 durable deployment 설정으로 전환한다.
  producer 정지 상태에서 atomic replace/fsync하고, 재시작은 그 설정만 따른다.
  새 디렉터리가 있다는 이유로 boot가 임의 선택하지 않는다. 이 지원도 구현 대상이다.
- StateRootLease는 개별 root의 배타성이다. 서로 다른 old/new root의 daemon 동시 실행을 막았다고
  가정하지 않는다. supervisor의 old 자동 재시작을 중지하고 단일 producer endpoint를 전환한다.
- root 게시 전후, launcher 설정 교체 전후, 첫 새 publication ACK 유실을 crash-cut으로 검증한다.
  restart 후 선택한 root/key/source checkpoint가 일치할 때만 producer를 재개한다.

backup/restore 검증은 catalog만이 아니라 해당 root가 참조하는 객체 closure, format,
normalizer, policy, readiness/retention identity를 포함한다. 공유 object pool 밖 경로 참조,
symlink, 누락 blob, 잘못된 incarnation을 거절한다.
root/counter/owner binding은 기존 restore 절차를 따르며 새 incarnation으로 오래된 예약을 차단한다.

전환 전후 저장 공간을 포함한 resource gate와 복원 리허설을 완료하기 전 제품 전환은 **NOT_RUN**이다.

## 10. 과거 비용 근거와 현재 검증 상태

[matching XL 기록](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#post-repair-matching-xl-diagnostic)은
2026-10-08 source 703d0e6899599ab4ab69c051089a9e4eda497ab0의 과거 진단이다.
32,768파일, 113,023,546 source bytes, seed 5864059738136528177.

| 과거 관측 | 시간 |
| --- | --- |
| 3,437바이트 변경 delta ingest/seal | 40.970795167초 |
| 그 안의 generation-proof 6개 envelope 합 | 32.609455209초 |
| delta activation | 13.395811292초 |
| no-op seal | 45.405355541초 |

앞선 감사에서 execution/profile/comparison의 source·exit·timeout·binary digest 전후 일치와
profile hash를 확인했다. 이번 재감사에서 벤치를 다시 실행한 것은 아니다.
중첩된 file-authority child clock을 위 proof envelope에 다시 더하지 않는다.
이 값은 현재 HEAD 성능, 94초 원 실행의 완전 RCA, paired 개선 효과의 증거가 아니다.

**VERIFIED — 정적 범위**: git rev-parse/status, rg, sed로 관련 source·dirty 범위와 Accepted 계약을
재대조했다. 공식 자료/원 논문을 읽고 대안·failure model을 비교했다.
Python unicodedata의 NFC 반례와 문서 링크/whitespace 검사는 이 범위에만 해당한다.
최종 실행 계획 보완에서는 Python 표준 라이브러리로 링크/문서 구조와 inventory ID 15개의
실제 source 경로, integrity-scrub의 runtime_extended_suite 매핑을 확인했다.
integration-storage와 기본 daemon의 선택 범위를 대조했고 기존 README의 다른 변경을 보존했다.
추가 감사는 compile_expr의 exact NOT, CursorBindingV2/KeyStore, offline state migration을
소스로 대조했다. Python 집합 반례는 approximate NOT의 누락과 segment-local ID 충돌을 확인한
수학적 예시이며 제품 회귀가 아니다. B1–B7의 보완은 설계·수용 조건이며 구현 증거가 아니다.
후속 감사 C1–C4는 coverage_snapshot/coverage_pages, match_sets/authority_doc_set,
CodeSearch/ranked_page, budgeted_search, RegexExecutor와 generation reclaim을 추가 대조했다.
디스크 page tree가 이미 존재한다는 설명을 정정하고 producer→consumer·오류·수명 경계를 보완했다.
호출 경로 재감사 D1–D4는 text_authority_plan의 watermark/rebuild 분기, adapter_open의 commit 순서,
원 event custody와 regex cache key를 대조했다. ID 배정·부분 mutation 복구·membership domain·operation
분류의 보완도 설계 요구이며 신규 구현/회귀를 실행한 증거가 아니다.
이전 추가 감사에서는 이 계획 파일만 수정했다. 이후 검색 구현은 10.1절로 구분한다.
README와 A/D/C5 구현은 다른 작업의 변경을 유지한다.

**NOT_RUN — 전체 수용**: paged/segment 저장 구조 전환 후 baseline/native XL,
sustained compaction, 외부 producer 통합, 전체 workspace suite/CI/release,
독립 경쟁 엔진 성능 비교. 기존 bounded F16 구조의 XL 진단은 10.1절에 기록한다.

향후 실행 명령·선택 범위의 기준은 **8.1절 하나**다. 좁은 integration-storage/basic daemon을
최종 gate로 대체하지 않는다. 새 codec/normalizer의 property/fuzz와 신규 inventory를 포함한다.
source/API/format/검증 의미가 달라지는 전체 변경을 수용하기 전 부분 green을 완료로 표시하지 않는다.

기존 A/D/C5 dirty 파일, adapter_lifecycle.rs, SDK/core public surface는 writer 인계 없이 수정하지 않는다.
검색 계측의 materialized_files 문구와 해당 회귀만 lexical outbound owner 범위에서 수정했다.
대형 build/benchmark는 host admission 아래 직렬로 실행한다. 일회성 결과는 checkout 밖에 둔다.

### 10.1 이전 F16 검색 핵심 경로 구현과 검증

2026-10-10, 작업 시작 HEAD `8f11cad9587aec1668a9fb3f5f1f055337487071`와 그 위의
작업 트리에서 진행한다. 아래 항목은 전체 LQ0–LQ5 수용과 구분한다.

| 범위 | 구현 상태 | 실제 경계 |
| --- | --- | --- |
| HIR 필요조건 | 구현 | raw NFC byte gram의 AND/OR/AllDocs. short OR·optional·Unicode branch를 보존하고 표현식 크기 제한 |
| Native regex | 구현 | 동일 raw stream으로 후보/exact 검증, 샤드별 즉시 검증으로 전체 후보 본문 복제 제거. 캐시 miss의 전체 shard 읽기·해시·decode와 전체 exact bitmap/cache는 유지 |
| posting reader | 구현 | 인증된 descriptor부터 읽고 AND의 작은 목록부터 교집합. empty/AllDocs 분리. 인증한 소유 buffer를 decode |
| CodeSearch 결과 수집 | 구현 | literal 결과 스트리밍, page limit 크기 ranking heap, 선택 행만 DTO/preview 생성. exact total·cursor 유지 |
| 본문 수명 | 구현 | 상주 metadata+locator, 선택 source 범위 SHA 검증 후 동일 본문 정규화. path/language는 본문 읽기 없이 판정 |
| seal 소유 증명·본문 없는 open | 구현, 검증 중 | sealed/F16 root/Native manifest를 함께 변경. 변경 bucket/shard 독립 검증, exact base proof만 상속. 재시작도 저장된 serving metadata 사용 |
| paged root·writer·routing | 미구현 | flat source root·256 bucket·전체 metadata/preflight 경로 유지 |
| shared GC·compaction | 미구현 | 기존 generation reclaim 유지. 지속 부하·공유 객체 GC/compaction 수용 주장 없음 |

검증 의미 변경은 [OCT-10-001](../../adr/OCT-10-001-seal-owned-lexical-serving-metadata.md)에 고정했다.
동일 길이 미변경 leaf의 변조는 선택 읽기/scrub에서 탐지한다. control metadata·missing·truncated·
symlink·orphan의 즉시 거절은 유지한다. 기존 테스트의 탐지 단계를 이에 맞춰 수정하고 대체 oracle을 추가했다.

검증 기록:

- **VERIFIED — 형식 전환 전 검색 핵심**: 3개 owner crate production/test Clippy 및
  unit + regex/window/source/preview/F15/cache/shard/resident integration **647 passed, 1 skipped**.
  이 결과는 이후 F16/seal/open 변경을 검증하지 않는다.
- 최초 556개 실행은 555 pass/1 fail/1 skipped였다. 새 strict-extractor 테스트의 잘못된
  긴 prefix 거절 가정을 수정했고 위 647개 실행에서 재검증했다.
- **VERIFIED — 당시 경계 guard**: `lint-hexagonal-boundaries.py`, 기본
  `check-cargo-modules-snapshot.py` (guarded contract/core tree).
- **VERIFIED — F16 production/test Clippy**: 아래 3개 owner crate의 Clippy가 통과했다.
  최초 compile/lint의 const/type inference, unused import, 정수 타입과 lint 실패를 수정한 뒤 실행했다.
- **FAILED — F16 최초 focused 회귀**: 491개 중 482 pass/9 fail/10 skipped.
  신규 형식에 맞지 않는 fixture의 임시 경로·page 길이·resident charge·mutant descriptor를 수정했다.
  후속 file/text authority 85개 실행은 84 pass/1 fail였고, 남은 retired-ID mutant descriptor도 수정했다.
- **FAILED — F16 전체 owner 회귀**: 아래 owner 명령으로 **882 run/872 pass/10 fail/9 skipped**,
  296.981초. 실패는 publication fault의 구형 root decoder 9개와 구형 manifest golden 1개였다.
  F16 layout으로 독립 oracle을 갱신했다.
- **VERIFIED — 수정한 10개 회귀**: publication fault 9개와 manifest golden 1개를 다시 실행해
  **10 passed**, 77.630초. 원 882개 실행의 실패 기록은 유지하며 전체 실행을 재실행했다고 표시하지 않는다.
- **VERIFIED — workspace 타입 검사**: `check --workspace --all-targets --all-features --locked`.
  최초 실행은 A/D/C5의 신규 예약 요청을 CLI doctor mock이 열거하지 않아 E0004로 실패했다.
  명시적 unexpected-request 거절 분기를 추가한 뒤 clean worktree `e4b95452`에서 재실행해 통과했다.
- **VERIFIED — 최종 owner Clippy**: 갱신된 publication oracle의 type complexity와 checked arithmetic을
  정리하고 `64ad9191`에서 같은 3-crate production/test 명령을 재실행해 통과했다.
- **VERIFIED — 경계 guard**: clean worktree에서 hexagonal 및 기본 cargo-modules guard 통과.
  cargo-modules의 실제 guarded 범위는 contract/core다.
- **NOT_RUN — 최종 변경 전체 수용**: 외부 producer,
  지속 부하, CI/release, 독립 경쟁 엔진 비교. 아래 XL 진단은 전체 성능 수용과 구분한다.

직렬 검증 명령(동일 build lane 캐시 재사용):

```sh
./scripts/cargow --lane test-daemon-lane clippy \
  -p quanta-index-lq-regex -p quanta-index-lq-trigram -p quanta-index-lexical \
  --lib --tests --all-features --locked -- -D warnings

./scripts/cargow --lane test-daemon-lane nextest run \
  -p quanta-index-lq-regex -p quanta-index-lq-trigram -p quanta-index-lexical \
  --lib --tests --all-features --locked --test-threads 2 --no-fail-fast \
  --status-level fail --final-status-level fail

./scripts/cargow --lane test-daemon-lane nextest run \
  -p quanta-index-lexical --lib --test unicode_normalization_goldens \
  --all-features --locked \
  -E 'test(publication_faults::) | test(a_generation_sealed_under_the_previous_format_is_refused_typed)' \
  --test-threads 2 --no-fail-fast --status-level fail --final-status-level fail
```

공유 작업 트리의 기존 A/D/C5 변경과 이번 검색 변경을 함께 복사한 전용 worktree를 만들었다.
첫 optimized XL을 고정한 source는 `e7589578828c72a7543e2cbf0890b62930de5c6f`, 경로는
`/Users/songmin/.codex/worktrees/lexical-f16-validation/quanta-index`다.
공유 `main`의 index/HEAD를 변경하거나 부분 변경을 cherry-pick하지 않았다.
공유 작업 트리와 고정 worktree의 변경된 non-doc 122개 경로가 같은 바이트임을 대조했다.
workspace/Clippy는 직렬로 main의 lane cache를 재사용했고, canonical daemon profile은
worktree별 target 격리를 다시 적용한다. XL도 daemon과 같은 worktree의 `test-daemon-lane`을 사용한다.

XL rail에는 Native top-10 질의만 있었으므로 F16 CodeSearch 검증을 추가했다.
full/delta/noop/delete/reopen마다 고유 파일 토큰 2개의 literal/regex를 검사한다(총 20개).
repo0 파일 삭제 후 부재와 같은 상대경로인 repo1 파일 잔존을 source identity로 판정한다.
기존 fixture, schema, Native latency와 operation clocks는 유지한다. 추가 검사는 clock 밖이며
전체 시나리오 시간에는 포함된다. CodeSearch latency·전체 corpus recall·검색 품질 점수는 측정하지 않는다.

**FAILED — 첫 daemon-all**: `QUANTA_INDEX_TEST_THREADS=2 just rust-profile test-daemon-all`,
`64ad9191`에서 catalog 65개 항목·11개 binary·420개 테스트를 선택했다.
실제 결과는 184 run/183 pass/1 fail/10 skipped/236 NOT_RUN, 367.082초다.
`sdk_default_code_search_matches_terms_across_chunks_as_one_file`의 두 번째 cursor page가
`contradictory code-search work counts`로 거절됐다. 전체 매칭 수 2와 페이지 DTO 수 1을
같다고 요구한 라우트 invariant가 원인이다. producer의 페이지 수집 계약에 맞춰
`materialized_files == fetched_files`로 수정하고 과소·과다 DTO 거절 회귀를 보강했다.
**VERIFIED — consumer/SDK 수정 회귀**: `6be3bdbe`에서
search-plane lib의 `code_search_execution_counts_reject_contradictory_page_provenance` 1개와
runtime_fast_suite의 위 SDK 테스트 1개가 각각 통과했다(0.009초/0.724초).
catalog 동일 범위 재검증은 `--no-fail-fast`, test threads 2로 실행했다.
`run-local-test-scope.py`의 `load_catalog`/`resolve_targets`/`build_command`를 사용했으며
scope의 65개 ID·11개 binary 선택은 원 명령과 동일하다.
이 rail은 기본 daemon과 integrity-scrub·process restart 범위를 포함한다.
중단된 첫 실행의 나머지 범위를 통과로 표시하지 않는다.

F16의 32,768파일·128MiB 원본 총량 상한과 16MiB root 한도, Native 전체 membership,
component/typo 중간 집합은 남는다. 본문 재검증 제거 구현을 전체 ingest O(delta)나 과거 94초의
실측 해결로 승격하지 않는다. XL 결과는 새 소스와 바이너리로 실제 실행한 뒤 별도 기록한다.


추가 검증 경로: 기존 lq-norm fuzz crate에 `regex_necessary_gram` target을 추가했다.
정규식 exact match가 참이면 raw-byte posting 후보에서 동일 문서가 누락되지 않는지를
독립 `windows(3)` posting oracle로 검사한다. arbitrary input 및 short/empty/optional/OR/
ASCII case/Unicode 분기를 bounded 입력으로 생성한다. fuzz lockfile의 regex·automata·syntax·
aho-corasick·memchr 버전은 workspace 실행 의존성과 일치시켰다.
Justfile smoke, heavy CI와 test-authority rail에 함께 등록했고 test-authority guard가 통과했다.
아래 target 실행은 F16 decoder fuzz와 전체 heavy gate의 대체 증거가 아니다.


**FAILED — daemon-all 재실행**: `6be3bdbe`, 420 run/418 passed/2 failed/10 skipped,
516.818초. 이번 실행은 선택 범위 전체를 완료했다. 두 실패는 `runtime_risk_suite`의
`full_corpus_runtime_fixture_executes_real_rows_only` 및
`regex_timeout_is_typed_and_does_not_poison_next_query`다.
새 timeout checkpoint가 `timeout` 문구를 사용해 기존 공개 `timed out` oracle에 불일치했다.
생산자의 두 메시지만 계약에 맞춰 수정했다. timeout 판정·순서·fixture는 유지했다.
**VERIFIED — timeout 수정 focused 회귀**: `8859fb26`에서 위 두 테스트가
2 passed/140 skipped, 32.891초로 통과했다. full-corpus 테스트 내부의 241 runtime rows도 통과했다.
원 전체 실행은 FAILED로 유지한다. 메시지 두 곳 외 생산 query 동작 변경은 없으며 전체 daemon을
수정 후 다시 실행한 결과로 표시하지 않는다.

fuzz CI의 `--locked` metadata는 새 checkout에서 lockfile이 없어서 exit 101로 실패함을 재현했다.
두 fuzz crate의 `.gitignore`에서 Cargo.lock을 제외하고 workspace lock의 공통 버전을 유지한
standalone lockfile을 추가했다. 새 resolver의 자동 최신 버전으로 production과 다른 regex/
serde 등을 검증하지 않도록 했다. 두 lock의 공통 package version 차이는 0개다.
`metadata --locked --offline`과 test-authority guard는 통과했다. 원격 heavy CI는 NOT_RUN이다.


**FAILED — 비최적화 XL 진단**: `8859fb26`의 fresh dev `scale_matrix`,
SHA-256 `830990ec2ae3b1bb73cedc17e36c917a189c7453cc0286b1dc90132fa2d6c298`,
32,768파일·seed 5864059738136528177·기존 한도 그대로 실행했다.
`full_ingest ok=1` 이후 `full_seal ok=0`이 기록됐다. 설정된 client timeout은 600초이나,
daemon cleanup이 끝나기 전 최종 오류 본문이 출력되지 않아 실패 원인은 이 기록만으로 확정하지 않는다.
실패한 작업을 SIGTERM으로 종료했고 capture는 730.133초, exit -15, status FAILED다.
execution의 global timed_out는 false다. 성공 profile/summary는 없으며 delta도 실행하지 못했다.
source/바이너리는 전후 일치했다. 기록은
`/private/tmp/quanta-index-xl-f16-8859fb26-20261010/{execution.json,stdout,stderr}`에 보존했다.

실패 로그 유실 경계는 `e7589578`에서 고쳤다. causal 실행의 phase 오류를 daemon cleanup 전에
원 error chain과 함께 stderr로 출력한다. 성공 clock, 실패 반환, cleanup 결과는 그대로다.
기존 `[profile.bench]`(opt-level 3, LTO off, codegen-units 16)로 optimized XL 바이너리를
빌드했다. 과거 release/LTO-off/codegen-units 1 실행과 paired 성능 비교로 취급하지 않는다.


**VERIFIED — fuzz/CI 등록 정책 회귀**: `e7589578`에서
`python3 -m pytest tools/ci/tests/test_check_test_authority.py tools/ci/tests/test_circleci_authority.py -q`
191 passed, 6.85초. `check-test-authority.py`도 통과했다. 이 검사는 Rust fuzz 실행이 아니다.

**VERIFIED — optimized build**: clean `e7589578`에서
`./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-harness --bin scale_matrix --all-features --profile bench --locked`,
6분 37초. binary SHA-256은
`cb7edf73479c8a9f8601e436d440197147c074432f582126f7b8b653c8f01ed4`다.

**VERIFIED — XL native 시나리오 / FAILED — 원 capture 후처리**: 위 binary로
32,768파일·64 source repos·113,023,546 원본 bytes·seed 5864059738136528177을 실행했다.
native exit 0, 236.547초이며 full/delta/noop/delete/reopen 및 추가 CodeSearch 20개 검사가
통과했다. capture는 지원 범위 밖 Python 3.9의 `zip(strict=...)`에서 후처리가 실패했다.
원 `execution.json`의 FAILED를 보존했다. Python 3.11의 `causal_cost_profile`로 동일
summary/manifest/trace/binary를 재집계해 exit 0을 확인했다. 원 execution의 전후 source·binary,
stdout/trace/summary digest와 현재 clean source를 대조했다. 재집계는 native 재실행이 아니다.

| XL 항목 | 실측 |
| --- | ---: |
| 초기 ingest + seal | 166.961초 |
| 단일 파일 ingest + seal | 14.160초 |
| delta activation + reclaim | 0.543초 |
| no-op seal / activation | 12.260초 / 0.573초 |
| delete seal / activation | 12.948초 / 0.550초 |
| 같은 프로세스 내 daemon 재시작 | 1.378초 |
| Native warm p50 / p95, 32 samples | 16.793ms / 17.963ms |
| registry 상주 회계값 | 227,785,539 bytes |
| 샘플링된 최대 process RSS | 4,030,054,400 bytes |

파일 변경은 3,437 bytes이고 lexical admission의 newline 포함 source 읽기는 1파일/3,438 bytes다.
delta BM25 census는 1문서/0.245ms, live build는 1.777ms(reused 3/new 1/dead 1)다.
no-op은 census 0문서, live build 2.015ms(reused 5)다. 전체 문서 BM25 재구축은 관측되지 않았다.
delta의 `bytes_written=1,083,920,315`는 hardlink 중복을 포함할 수 있는 logical directory-size
증가량이며 실제 쓰기 I/O가 아니다. macOS physical write I/O는 측정하지 못했다.

**남은 실측 비용**: delta/noop에서 file-authority plan prepare 1회와 revalidate 3회가
각각 총 3.965초/4.204초였다. 32,768 staging 경로의 반복 negative open이 소스에서 확인됐다.
generation proof 6회 합계는 2.824초/2.886초, seal file admission은 1.842초/1.681초,
finalize는 1.047초/1.799초다. 일부 span은 중첩되므로 이 숫자를 모두 더하거나 독립 wall share로
해석하지 않는다. 아래 후속 수정은 이 중 반복 negative open을 대상으로 한다.

원시 결과와 재집계는 checkout 밖
`/private/tmp/quanta-index-xl-f16-e7589578-bench-20261010/`에 있다.
이 실행은 causal instrumentation이 켜진 단일 공유-host 진단이다. 원 capture 후처리는 FAILED이며
복구 replay의 status도 `diagnostic_unqualified`다. quiet-host 성능 수용, CodeSearch latency/품질,
과거 94초 대비 paired 개선, 외부 엔진 순위로 승격하지 않는다.

**VERIFIED — regex 필요조건 fuzz smoke**: clean `e7589578`, fixed nightly `nightly-2026-08-01`,
local cargo-fuzz 0.13.1(원격 CI pin 0.13.2와 구분), ASan + debug assertions + overflow checks.
resource-admission lock 아래 `regex_necessary_gram`을 `-max_total_time=60 -max_len=386
-seed=586405973`으로 실행했다. 61초/167,027 runs, exit 0, 반례·crash 없음.
corpus와 crash 출력은 `/private/tmp/quanta-index-regex-fuzz-e7589578-20261010/` 아래에 둔다.
60초 smoke는 완전한 soundness 증명, F16 decoder fuzz 또는 원격 heavy gate가 아니다.

**VERIFIED — staging 부재 경로의 focused 회귀**: 후속 source
`1800f7708d7613246692d615dfb5de80b6be24db`는 `validate_plan_storage`만 변경했다.
세 디렉터리를 descriptor 기반 nofollow로 열어 staging 부재를 한 번 확인하면 동일한
inherited-root 길이 규칙을 적용한다. 존재하면 원 파일별 검사 경로를 그대로 사용한다.
매 호출 root/manifest/operation/policy binding, 128MiB 합계 제한, writer의 실제 seal 검증을 유지한다.
symlink·non-directory·검사 오류는 부재로 취급하지 않는다. 다음 호출 전 staging 생성과
용량 초과/redirect를 거절하는 기존 회귀를 확장했다. 외부 프로세스가 동일 호출 도중
staging을 생성하는 경합의 탐지 시점까지 이전 per-file probe와 동일하다고 주장하지 않는다.
후속 재검증과 실제 seal이 게시 전 데이터를 검증하며, 내부 writer는 기존 mutation lock을 사용한다.

```sh
./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical \
  --lib --all-features --locked \
  -E 'test(file_authority::tests::admitted_plan_) | test(file_authority::tests::packed_)' \
  --test-threads 2 --no-fail-fast --status-level fail --final-status-level fail
./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-lexical \
  --lib --tests --all-features --locked -- -D warnings
```

각각 8 passed(0.431초), Clippy exit 0(21.81초)다. optimized build도 같은 명령으로
17.66초에 통과했다. 새 binary SHA-256은
`e8e3e573932827e81b9669aca29a6ec64020d8fea9f6b4291feefa81805f068d`다.
**VERIFIED — 후속 XL 진단**: Python 3.11 capture 전체가 exit 0,
`VERIFIED_DIAGNOSTIC`, 225.809초로 끝났다. 전후 clean source·binary digest가 일치한다.
corpus/seed/config/profile은 첫 optimized 진단과 같다. 변경은 staging 부재 경로 한 개이며
두 번의 공유-host 실행을 반복·무작위화한 성능 수용으로 취급하지 않는다.

| 같은 XL 조건 | `e7589578` | `1800f770` |
| --- | ---: | ---: |
| 초기 ingest + seal | 166.961초 | 167.574초 |
| 단일 파일 ingest + seal | 14.160초 | 10.329초 |
| no-op seal | 12.260초 | 8.465초 |
| delete seal | 12.948초 | 9.158초 |
| delta plan prepare | 1,062.487ms | 176.677ms |
| delta plan revalidate 3회 | 912.586 / 982.247 / 1,007.629ms | 4.200 / 4.437 / 4.394ms |
| no-op plan prepare | 1,121.415ms | 177.717ms |
| no-op plan revalidate 3회 | 977.532 / 1,020.424 / 1,084.962ms | 4.717 / 4.477 / 4.418ms |

후속 full/delta/noop/delete/reopen과 CodeSearch 20개 검사가 모두 통과했다.
Native warm p50은 17.024ms, registry 상주 회계값은 227,791,251 bytes,
샘플링된 최대 process RSS는 3,627,974,656 bytes다. 이 값은 CodeSearch latency가 아니다.
source admission은 delta 1파일/no-op 0파일이며 BM25의 증분 재사용도 유지했다.
반복 generation proof·seal admission·finalization의 corpus 비례 비용은 그대로 남는다.
10.329초와 8.465초를 전체 ingest O(delta), 과거 94초 RCA의 완전 해소 또는 SOTA 순위로 표시하지 않는다.
원시 결과는 checkout 밖 `/private/tmp/quanta-index-xl-f16-1800f770-bench-20261010/`에 있다.

**VERIFIED — staging 수정 source 전체 owner 회귀**: clean `1800f770`, 위 3개 owner의
`nextest run --lib --tests --all-features --locked --test-threads 2 --no-fail-fast`가
**882 run / 882 passed / 9 skipped**, 380.798초로 통과했다. 기존 전체 실행에서 실패한
publication fault 9개와 previous-format refusal 1개도 이번 전체 선택 범위에서 통과했다.
9 skipped는 실행 성공으로 계산하지 않는다. 이전 FAILED 기록과 구분한 새 전체 실행이다.
**VERIFIED — 같은 source daemon-all 전체 회귀**: canonical catalog의 65개 ID를
`run-local-test-scope.py`의 `resolve_targets(["daemon-all"])`와 `build_command`로 선택했다.
`./scripts/cargow --lane test-daemon-lane nextest run`에 해당 11개 binary/filter와
`--test-threads 2 --no-fail-fast --status-level fail --final-status-level fail`을 사용해
**420 run / 420 passed / 10 skipped**, 514.579초로 통과했다.
이 결과는 이후 Native 독립 검증 수정과 아직 구현하지 않은 paged/segment 구조의 증거가 아니다.

**확인된 검증 계약의 누락**: 기존 `ShardBody::verify_semantics`는 변경 shard를
`ShardBuilders`로 다시 만들어 비교했다. 저장된 posting 변조는 거절하지만 같은 producer를
oracle로 재사용하므로 7.1절의 독립성 조건을 충족하지 않았다. 잘못된 query 결과를 재현한
결함과 구분한다. `4bf24113`의 수정은 문서 원문 byte windows와 shared tokenizer만으로
네 posting 관계를 직접 계산한다. gram key/문서 ID 목록과 term inventory/canonical position
bytes를 전체 비교하며 posting builder/producer helper/위조 count 기반 iterator를 호출하지 않는다.
기존 stamp/range/NFC/fold/cell cap을 유지한다. surface별 oracle map을 순차 생성·해제하고
이전 whole-shard 재구축과 두 serialize copy를 제거했다. expected Vec는 fallible growth를
사용하지만 stable BTreeMap의 allocation refusal나 shard byte/scratch hard cap까지 증명한 것은
아니다. 기존 입력 지원 상한을 새 임의 제한으로 축소하지 않았다.

위조 position count, nonminimal/trailing/truncated bytes, missing/extra/wrong gram·term·doc·position,
folded·stamp·range·normalizer 변조 회귀를 추가했다. source review/rustfmt/diff check는 통과했고,
clean `4bf24113`에서 focused owner regression은 **52 passed / 384 skipped**, 98.326초로 통과했다. 후속 `d19a9dda`는 Clippy 2건을 수정했고 lexical lib/tests all-features Clippy가 4.39초로 통과했다. 전체 882/420 회귀를 이 source에서 재실행한 결과는 아니다. `PositionsIndex`의 중복 wire term을 마지막 값으로
해석하는 기존 decoder 동작은 논리 posting 비교를 우회하지 않으나 strict canonical-wire
거절의 증거도 아니다. 후속 LQ2 codec에서 해당 표현을 닫는다.


### 10.2 F16 독립 oracle 후속 실행과 paged rewrite 경계 (2026-10-10)

**VERIFIED_DIAGNOSTIC — clean `d19a9dda5eb26b244699e39be4193df646cce877` XL**:
32,768파일/113,023,546 bytes, native exit 0, elapsed 197.397초. 전체 색인 seal
142.146초, 1파일 delta seal 9.042초, no-op seal 7.761초, delete seal 8.770초,
same-process reopen 1.482초다. Native warm p50/p95는 17.224/18.176ms(32 samples),
registry 회계값은 227,685,597 bytes, sampled peak RSS는 3,389,292,544 bytes다.
full/delta/no-op/delete/reopen과 CodeSearch 20개 identity 검사가 통과했다.
원시 결과는 `/private/tmp/quanta-index-xl-f16-d19a9dda-bench-20261010/artifact/summary.json`,
실행 판정은 같은 디렉터리 상위 `execution.json`이다.

공유 호스트 단일 diagnostic이며 경쟁 엔진 순위/quiet-host qualification이 아니다.
config digest `a3378a9776f451a6e9a09699ca4e6282cd19538fd5996776f71e3111134c72b0`는
직전 1800 실행과 다르다. 이번에는 `--client-timeout-ms 600000`을 명시했고 직전은
같은 유효 기본값을 생략했다. 동일 config digest의 paired ablation으로 표시하지 않는다.
논리 generation directory bytes는 hardlink를 중복 집계할 수 있어 실제 물리 쓰기량이 아니다.
과거 93.969초의 모든 원인을 이 실행으로 분해하거나 완전히 해결했다고 주장하지 않는다.

**구현 진행 중 / 전체 수용 미완료 — 공유 paged 저장소 전환**:
위 F16 실행과 현재 dirty source를 분리한다. compact v17 File/Native roots,
immutable shared object pool, COW maps, bounded segment/posting pages, catalog-owned u64 ranges,
selected-use authentication, request-wide physical I/O accounting, row/page-resumable scrub,
epoch-fenced disk-backed GC와 실제 referencing-generation quarantine fence가 연결됐다.
기존 flat backend는 제거됐다. 이 변경을 위 F16 수치나 882/420 결과로 승인하지 않는다.

- **VERIFIED — 첫 focused 런타임**: `./scripts/cargow --lane test-daemon-lane nextest run`
  (lexical/search-plane/searchd, `--lib --all-features --locked`, lexical_store/lexical_gc,
  File/Native paged oracle, adapter_ingest, scheduler/fanout/fence 범위): 49 run/49 pass,
  965 skipped, 21.189초. 이후 요청 scope·바이트 회계·gram 비용 수정은 이 결과의 승인 범위 밖이다.
- **FAILED / 중단 — 첫 전체 owner 시도**: 같은 3개 crate `--lib --tests` 범위는
  1382 selected 중 341 run, 331 pass, 8 executed assertion failures, 2 SIGINT,
  10 skipped, 1041 NOT_RUN. 615.092초에 더 이상 최신 source를 검증하지 않는 실행을 중단했다.
  실패는 absent File family, 실제 fd-relative root 경로의 fault hook 누락,
  u64 match-set 논리 바이트 정책과 구형 테스트 cap, current-format manifest fixture였다.
  수정 후 재검증이 필요하다.
- **확인된 비용 결함 / 수정**: 작은 파일마다 2MiB gram universe를 count/enumerate하던
  producer와 독립 oracle 경로가 1만 파일 fixture에서 수분을 썼다. producer는 발견한
  gram만 정렬하고 작은 oracle은 raw windows를 사용한다. fixture 수를 줄이지 않았다.
- **VERIFIED — 결합 타입 검사**: lexical/search-plane/searchd-runtime의
  `check --lib --tests --all-features --locked`가 59.96초에 통과했다.
  이후 lint/fixture 수정을 포함한 최종 재검증은 남아 있다.
- **VERIFIED — 경계**: `just rust-hexagonal`. cargo-modules의 의도한 core 신규 4타입을
  현재 source tree와 대조해 baseline을 갱신했고 기본 `just rust-cargo-modules`도 통과했다.
- **FAILED — 첫 strict Clippy / 수정 중**: 새 paged backend의 fallible indexing,
  checked conversion, error handling과 함수 계약 지적을 소유 모듈별로 수정 중이다.
  단순 compilation을 strict-lint 통과로 대체하지 않는다.
- **추가 발견 / 구현 연결**: shared pool이 pair/root retention byte cap에서 누락됐다.
  pair는 selected closure를 disk-backed dedup으로 포함하고 root는 실제 전체 track을
  한 번 계산한다. 아직 GC되지 않은 객체도 root cap에 포함한다. 이 회계 경로는
  corpus-dependent traversal이며 전체 증분 비용에서 숨기지 않는다.
- **추가 발견 / 구현 연결**: offline deep-open이 lexical inventory만 확인하던 경로에
  shared closure scrub을 연결했다. 실제 SearchCorpus backup/restore 후 File/Native query,
  missing/corrupt pooled body의 staging 게시 거절 회귀는 source-ready, 실행은 NOT_RUN이다.

**NOT_RUN — 전체 수용**: 최종 source의 strict Clippy, full owner, daemon/restore/held-reader GC,
새 optimized XL과 검색 품질 비교. whole ingest O(delta), compaction 지속 부하 상한,
과거 93.969초의 전체 RCA 완료 또는 SOTA 우위를 아직 주장하지 않는다.


### 10.3 Paged 통합 후 추가 결함과 복구 배치 (2026-10-10)

- **FAILED / 중단 — 두 번째 focused runtime**: lexical/search-plane/searchd의
  `nextest run --lib --tests --all-features --locked`에서 102개를 선택했다.
  여러 File 테스트가 없는 generation 디렉터리를 pin하려다 ENOENT로 실패했다.
  실행 도중 source가 수정됐고 10,001파일 테스트가 장시간 계속되어 중단했다.
  최종 nextest summary가 없으므로 미출력 테스트를 성공으로 계산하지 않는다.
- **수정 / 재실행 필요**: 실제 미생성 generation은 `Ok(None)`으로 처리한다.
  dangling symlink, ENOTDIR와 기타 I/O 오류는 absence로 취급하지 않는다.
  해당 preflight/redirect 회귀를 확장했다.
- **관측된 비용**: 10,001파일 fixture의 실행 중 스택 표본은 `put_blob`마다
  adoption epoch, pending head, journal append, clean head의 내구 기록을 반복했다.
  파일 내용 전체의 메모리 적재가 원인이라는 증거는 없다. 단일 표본은 전체 시간
  비율이나 과거 93.969초의 RCA를 확정하지 않는다.
- **구현 / 재실행 필요**: 공통 저장소의 scoped adoption batch를 File·Native
  producer에 연결했다. 최대 64개 참조·2MiB(새 encoded data objects와 batch
  manifest 포함; 작은 control records 별도)마다 journal을 묶어 확정한다.
  부분 append는 manifest와 기존 head chain을 인증해 복구한다. Err/panic은
  scoped view를 닫고 gate를 해제하며, 루트 게시보다 성공 flush가 먼저다.
- **교차 감사에서 확인된 결함 / 수정**: `map_apply`와 `map_bootstrap`의 마지막
  root adoption이 배치 gate를 보유한 채 두 번째 flock을 시도했다. 배치 인식
  adoption으로 통일했다. timeout을 둔 batch-map 회귀와 참조·바이트 경계,
  partial/full append 복구, manifest/chain 손상, escaped-view 거절 회귀를 추가했다.
- **NOT_RUN**: 위 최종 수정의 runtime/성능 수용. strict Clippy 지적을 수정한
  결합 재검사를 진행 중이며, 성공 결과 없이 성능 개선이나 전체 완료를 주장하지 않는다.


### 10.4 배치 집중 실행과 실제 초기 게시 경로 (2026-10-10)

`./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical
--lib --all-features --locked -E 'test(/lexical_store|lexical_gc|file_authority::plan::tests|adapter_ingest::tests|text_authority::paged_verify/)'
--test-threads 2 --no-fail-fast --status-level fail --final-status-level fail`:
**FAILED**, 41 run / 39 pass / 2 fail / 312 skipped, 14.520초.
배치 참조·바이트 분할, partial/full append 복구, manifest/chain 손상 거절,
Err/panic escaped view와 batch-map 교착 회귀는 실행 통과했다.
실패는 동일 32KiB 블록의 content-addressed dedup을 무시한 128KiB 물리 용량 기대값과
Native no-op의 새 batch manifest 인증 읽기 1회를 빠뜨린 테스트 상한이었다.
고정된 네 개의 서로 다른 블록으로 용량 fixture를 수정하고 no-op의 정확한
root 수 + manifest 1회 상한 및 compaction 대상 0개 oracle을 유지했다. 후속 재실행은 별도다.

실제 offline 회귀의 첫 실행은 `quanta-index-searchd-runtime
--test state_migration_owner_v1 -E 'test(real_search_corpus_)'`에서
**FAILED**, 2 run / 0 pass / 2 fail / 55 skipped, 0.381초였다.
빈 daemon state root의 `indexes/lexical`을 만들기 전에 `ObjectStore::open`을 호출한
production 초기화 순서 결함으로 첫 게시가 실패했다. 기존 TempDir root fixture가 이
부재 경로를 덮지 못했다. 원래 입력·예약과 preflight 거절의 무쓰기 계약을 보존하면서
track root를 생성하는 수정을 진행 중이다. backup/restore 성공을 아직 주장하지 않는다.

공용 adapter fixture의 생산 API와 동명인 build shim을 명시적인
`build_fixture_batch` / `build_fixture_batch_with_owner`로 바꿨다.
pass-through preflight shim과 blanket dead-code exemption을 제거했으며,
사전 검증 및 예약 없는 게시의 거절 회귀는 생산 포트를 직접 호출한다.

### 10.5 전체 owner 재실행에서 발견한 정렬·복구 결함 (2026-10-10)

`./scripts/cargow --lane test-daemon-lane nextest run`의 core/lexical/search-plane/
searchd/scan-experiment `--lib --tests --all-features --locked --test-threads 2`
실행은 **FAILED / 중단**이다. 1592 selected 중 550 run, 545 pass,
3 executed failures와 2 SIGINT, 10 skipped, 1042 NOT_RUN, 688.545초다.
이미 수정 중인 source를 검증하는 반복 대형 fixture 구축을 중단했으며,
미실행 범위를 성공으로 계산하지 않는다.

- **실제 결함 / 수정 후 재검증 필요**: File route merge의 heap 순서가
  `(gram, segment_digest, posting_count)`인 반면 durable key 순서는
  `(surface, gram, posting_count, segment_digest)`였다. 여러 segment에 같은
  gram이 있는 3,000-source fixture가 `bootstrap keys are not strictly sorted`로
  실패했다. producer의 stream 순서를 durable key 계약과 일치시켰다.
- **실제 결함 / 수정 후 재검증 필요**: fd-relative Native root의 rename 이전
  장애가 중첩 `text-authority` 디렉터리에 임시 파일을 남겼다. 자기 임시 파일의
  실패 정리와 재시작 복구를 구현 중이다. 정규 daemon의 state-root OS lease와
  직접 구성한 다중 adapter의 동시 쓰기 조건을 구분해 검토한다.
- **fixture 수정 / 재검증 필요**: 미정리 64KiB orphan의 두 32KiB 블록이
  같아 content-addressed dedup으로 실제 증분 물리 용량이 작아졌다.
  서로 다른 두 블록으로 고정된 입력과 용량 기대값의 관계를 복구했다.
- **관측된 비용 / 부분 수정**: 10,001-file fixture는 query 이전의 source 구축에
  머물렀다. 실행 스택 표본에서 객체별 `sync_all`과 gram bitmap 초기화를 확인했다.
  작은 입력(16KiB 이하)은 bounded windows 정렬·dedup으로 바꿨다. adoption batch는
  객체별 file/directory fsync를 제거하지 않는다. 이 비용의 최종 수용은 optimized
  실행에서 별도로 판단한다.

track root 생성은 입력·예약 검증 뒤의 공통 publication owner로 옮겼다.
실제 backup/restore와 최종 장애 주입, 전체 owner/daemon, optimized XL의 재실행은
아직 **NOT_RUN**이며, 이전 F16 수치와 이 구현을 합쳐 완료로 표시하지 않는다.

### 10.6 내구성·자원 경계 보완과 재실행 (2026-10-10)

최신 계약은 [OCT-10-002](../../adr/OCT-10-002-shared-paged-lexical-authorities.md)다.
앞 절의 실패 기록을 삭제하지 않으며, 아래 결과가 실제로 덮는 범위만 갱신한다.

- **VERIFIED — 집중 회귀**: `./scripts/cargow --lane test-daemon-lane nextest run
  -p quanta-index-lexical --lib --all-features --locked`, selector
  `lexical_store|lexical_gc|file_authority::plan::tests|adapter_ingest::tests|text_authority::paged_verify|distinct_source_grams|route_stream_orders|metadata_walk_has|independent_root_directory_writers`,
  threads 2: 52 run / 52 pass / 311 skipped, 19.091초.
  route 정렬 역전, 원래 입력·예약 뒤 track root 생성, 실제 용량 dedup, metadata walk 예산,
  재사용 객체의 동기화 복구와 manifest 재사용, full barrier 실패 시 미게시,
  독립 root writer의 live temporary 보호를 포함한다.
- **VERIFIED — 실제 장애 주입**: 같은 lexical lib의
  `pool_and_root_io_refusals_never_admit_unsealed_target|pool_and_root_sigkill_cuts_resume_without_admitting_staging|retry_preserves_the_attempts_original_native_document_ids|independent_root_directory_writers_preserve_a_live_temporary`,
  threads 2: 4 run / 4 pass / 359 skipped, 143.164초.
  SIGKILL과 I/O 거절·재시작 순서 검증이며 물리 전원 장애의 증명은 아니다.
- **FAILED / 일부 통과 — 실제 대형·복원 회귀**: lexical lib,
  `cancellation_inside_search`, `l2_file_mutation`, runtime `state_migration_owner_v1`의
  `real_search_corpus_|a_cancelled_budget_is_refused_before|language_constraint_applies_before|combined_replacement_retires_old_symbols`,
  threads 2: 6 run / 4 pass / 2 fail / 462 skipped, 132.142초.
  원래 3,000·10,001파일 fixture, 두 취소 경로와 교체·시간 보고 회귀는 통과했다.
  복원 두 테스트는 File 결과를 chunk ID로 기대한 oracle 오류로 실패했다.
  Native chunk ID와 고정 File ID를 구분해 수정했으며 복원 성공은 재실행 전까지 주장하지 않는다.
- **생산 결함 수정**: 실제 daemon 관측 계약이 Native child `build=Some`,
  `publish=None`을 거절했다. 분리 측정되지 않은 세 child clock은 `None`으로,
  실제 측정한 outer Native/File 시간은 유지했다. File build 전체를 source-write 시간으로
  보고하던 부정확한 child 값도 제거했다. 0을 꾸미거나 관측 계약을 약화하지 않았다.
- **추가 자원 경계 수정 / 최종 owner 재실행 필요**: posting의 한 gram 전체를 읽은 뒤
  한도를 감지하던 경로를 route/segment/page/current locator 단계별 누적 회계로 바꿨다.
  metadata walk의 별도 물리 상한은 유지한다. GC inventory와 sweep은 pinned object shard와
  삭제 대상 파일의 device를 확인한다. 요청 비용 거절과 삭제 안전성의 보완이다.
- **VERIFIED — 계측 도구 회귀**: `.venv/bin/python -m pytest
  tools/ci/tests/test_causal_cost_profile.py tools/ci/tests/test_causal_cost_capture.py -q`:
  94 pass. 공통 pool의 POSIX data sync, full control sync, adoption barrier와 재사용 복구를
  기존 opt-in 진단으로 구분했다. 계측 시간은 해당 호출의 envelope이며 물리 쓰기 완료량이 아니다.
- **추가 oracle / 실행 대기**: 실제 shared FileBody를 상속한 두 세대를 손상시켜 모두
  quarantine/fence하고, 비참조 이전 세대의 pinned 검색은 보존하는 runtime 회귀를 추가했다.
  기존 semantic-only scrub 테스트를 공유 lexical 객체의 증거로 대체하지 않는다.

최종 source의 전체 owner, canonical daemon, workspace/strict lint 및 optimized XL은
각각 별도 실행 결과로 기록한다. 과거 F16 숫자와 현재 paged source를 합쳐 수용하지 않는다.


### 10.7 전체 owner 실행과 후속 구조 수리 (2026-10-10)

core/lq-regex/lq-trigram/lexical/search-plane/searchd/scan-experiment의
`./scripts/cargow --lane test-daemon-lane nextest run --lib --tests
--all-features --locked --test-threads 2 --no-fail-fast`는 **FAILED**다.
1801 run / 1777 pass / 24 fail / 10 skipped, 1172.152초다.
수정 이후 source의 결과와 혼합하지 않는다.

- **실제 쿼리 비용 결함**: 6,194-document Native query가 같은 segment 설명과
  map page를 반복 인증·decode하며 128MiB physical-read 상한을 초과했다.
  상한을 늘리지 않고 요청 내부에만 존재하는 용량 제한 descriptor/page 재사용으로
  수리한다. 새 요청에서는 인증 상태를 다시 만든다. 재실행 전 수용 상태는 **NOT_RUN**이다.
- **실제 오류 처리 결함**: 공유 객체 손상에서 최초 세대 receipt 또는 첫 fanout
  receipt 쓰기 실패가 이후 참조 세대 차단을 건너뛸 수 있었다. 알려진 참조 집합의
  차단을 먼저 완료하고 receipt 오류가 다른 참조 차단을 생략하지 않도록 수리한다.
  inventory 자체가 완전히 열리지 않는 경우의 전역 차단은 증명 범위에 포함하지 않는다.
- **계측 계약 수정**: F17은 메모리 입력을 검증하므로 죽은 F15 staged-read/replay
  지표를 제거했다. 성공한 File seal의 Upsert 수·원본 길이는
  `file_admission_sources_verified` / `file_admission_source_bytes_verified`로 보고한다.
  논리적 검증량이며 pool physical I/O와 같다고 해석하지 않는다.
- **오류 분류 수정**: 8MiB 초과는 `INGEST_RESOURCE_BUDGET_EXCEEDED`, 원본 SHA
  불일치는 별도 invalid contract로 구분한다. 예약 없는 raw mutation은 계속 거절한다.
- **테스트 이관**: 사라진 generation-local leaf 경로, v16 현재-format 기대,
  필수 root 디렉터리 부재 기대와 shared scrub의 단일-call 완료 가정을 수정한다.
  상속 FileBody/NativeBody의 고정 witness·inode·길이·SHA 및 독립 rebuild oracle을
  유지한다. 공개 source batch는 항상 sealed이므로 raw unsealed writer fixture를
  위한 admission bypass는 추가하지 않는다. writer resource policy는 private unit
  scope에서, 게시 후 readable/seal release는 실제 batch integration에서 검증한다.

최종 집중 회귀, daemon, strict workspace lint와 새 optimized XL 결과는 아직
**NOT_RUN**이다. 구조 검사 3종과 causal Python 94개 통과만으로 완료를 선언하지 않는다.

### 10.8 Lexical 재실행과 정책 경계 정리 (2026-10-10)

- **FAILED — lexical 전체 재실행**: 같은 lane의 `nextest run -p
  quanta-index-lexical --lib --tests --all-features --locked --test-threads 2
  --no-fail-fast`: 665 run / 661 pass / 4 fail / 9 skipped, 1125.693초.
  6,194-document Native query, 독립 full rebuild 및 공유 객체 fanout 회귀는 통과했다.
  이는 뒤의 codec/모듈 이동 이후 전체 source를 다시 실행한 결과는 아니다.
- **VERIFIED — 실패 수정 집중 회귀**: `nextest run -p quanta-index-lexical
  --lib --test l2_file_mutation --test sealed_commitment_cost`에서 bounded cursor,
  optional File root, 8MiB 경계, preflight/lock 이후 변조 및 delta seal 비용을 선택했다.
  6 run / 6 pass / 402 skipped, 212.727초. 심볼릭 링크를 일반 I/O 오류로 반환하던
  File root reader는 무결성 오류로 분류했고, 실제 부재와 권한/장치 오류는 구분했다.
  나머지는 새 저장 구조에 맞지 않던 fixture event 재사용·scrub step 및 비용 기대값이다.
- **비용 oracle**: delta seal 해시 2,736,183B는 새 파일 45,108B + 상속 coverage
  페이지 2,691,075B와 정확히 일치했다. 임의 `base_bytes/2` 상한을 제거하고 독립
  디스크 inventory의 바이트/파일수, 비coverage inode·길이 상속, pooled NativeBody
  inode·길이·SHA와 두 세대 검색 oracle을 유지했다. 전체 seal O(delta) 주장은 아니다.
- **정책 수리 / runtime 재검증 대기**: AttemptRecord의 동일한 2필드 CBOR와
  fixture의 named-field JSON을 명시적 serde codec으로 이전했다. 고정 literal과
  잘림/추가 필드/누락/중복 거절 oracle을 추가했다. Root 이름/상한과 receipt codec을
  하위 소유 모듈로 옮겨 새 순환 의존성을 제거했다. allowlist 예외는 늘리지 않았다.
- **VERIFIED — 구조/공개 계약**: module-cycle, derive, digest-fallibility,
  test-authority, ignored-test ownership 및 wire inventory 검사 통과.
  `just rust-cargo-modules rust-public-api` 통과: Contract/Core 모듈과 Contract/SDK
  공개 surface가 등록 baseline과 일치한다. Registry-only proof lint는 실행 증명이 아니다.

위 모듈 이동 이후 관련 runtime, 최종 workspace Clippy, daemon/SDK와 새 optimized XL은
각각 후속 실행 결과로 갱신한다. 이전 실패와 최종 source의 통과 결과를 섞지 않는다.

### 10.9 Daemon 전체 실행에서 발견한 소유권 회귀 (2026-10-10)

- **VERIFIED — codec/모듈 이동 집중 회귀**: lexical lib, `sealed_manifest`,
  `l2_file_mutation`에서 `lexical_store|lexical_gc|shared_fault|quarantine|bounded_cursor|fixture_reservation_json`
  선택: 60 run / 60 pass / 393 skipped, 30.044초. 이후 fixture의 불필요한
  `expect`만 제거한 최종 테스트 source는 별도 재실행한다.
- **FAILED — daemon-all 등록 범위 실행**: `nextest`가 선택한 runtime/harness
  11개 바이너리, threads 2: 423 run / 414 pass / 9 fail / 10 skipped,
  1068.359초. 등록 범위에는 중복으로 포함된 시나리오가 있어 고유 시나리오 수로
  표현하지 않는다. 이 결과는 아래 수리 전 source의 결과다.
- **실제 부팅 소유권 회귀 / 수정 후 실행 대기**: exact shared-object closure
  계측이 history restore에서 모든 retained 세대의 내용 검증을 먼저 수행했다.
  inactive 손상 두 시나리오는 정상 active daemon까지 중단했고, active 손상은
  원래 `ACTIVATION_TARGET_UNOPENABLE` 원인 연결을 우회했다. 두 명시적 content
  integrity 오류에만 실제 전체 물리 용량을 pair 상계로 증명하는 무삭제 복원
  경로를 추가했다. pair/total/count 한도를 유지하며, 물리 상계가 없거나 한도를
  넘거나 일반 I/O가 실패하면 거절한다. 새 admission의 exact 측정은 유지한다.
  손상된 대형 multi-pair root의 보수적 거절 한계는 ADR에 명시했다.
- **실제 대형 게시 timeout / 수정 후 실행 대기**: 4,096파일 재시작 시나리오의
  두 등록 실행 중 하나가 게시 IPC의 기존 300초 제한을 초과했다. 스택 표본은
  독립 File seal 검증의 반복 map 조회·물리 인증을 가리켰다. producer와 인증
  상태를 공유하지 않는 새 검증 pass에 기존 4MiB map-page 재사용을 적용했다.
  시간 제한이나 메모리 상한은 늘리지 않았고, 독립 원본 oracle을 유지한다.
- **테스트 입력/기대값 수리 / 실행 대기**: 공유 FileBody 손상의 live scrub
  fanout과 restart 후 selected-read 인증을 분리했다. 기존 inactive/active compact
  root 부팅 계약은 그대로 검증한다. GC retained-byte 기대값은 고정 durable wire를
  독립 파싱한 reachable pool closure를 더하며 reclaimed-dir oracle은 유지한다.
  backup/restore fixture는 실제 `select:file.owners` 조회가 요구하는 ownership
  batch를 명시적으로 게시하고 고정 owner 기대값을 추가했다. raw substring의
  typed plan-limit 상세에는 실제 distinct gram 수와 cap을 복구했다.

위 수리의 집중 runtime, workspace lint, SDK 프로세스 경로 및 새 optimized
Large/XL은 후속 실행 전까지 **NOT_RUN**이다. 부팅 quota의 우연한 전체 해시를
영속 quarantine 계약으로 삼지 않으며, 기록되지 않은 receipt의 재시작 보장은
현재 범위의 증명으로 표시하지 않는다.

### 10.10 수정 경로 재검증과 최종 포맷 분류 (2026-10-10)

- **VERIFIED — consumer/독립 검증**: `./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-semantic -p quanta-index-lexical --lib
  --test sealed_manifest --all-features --locked`에서 Semantic manifest,
  door receipt 실패 후 fanout, 고정 JSON fixture, File 독립 seal oracle,
  request map cache를 선택했다. 30 run / 30 pass / 408 skipped, 8.005초.
  캐시는 각 seal pass에서 새로 만들며 producer의 인증 상태를 상속하지 않는다.
- **FAILED — daemon 수리 집중 실행**: runtime/search-plane lib,
  `runtime_risk_suite`, `state_migration_owner_v1`, `process_readiness_owner_v1`:
  14 run / 12 pass / 2 fail / 780 skipped, 301.680초. 부팅 3종, live shared
  corruption fanout, 손상 leaf의 restart/selected-read 격리, GC 물리 회계,
  raw substring 제한, history restore 상계 회귀와 4,096파일 실제 프로세스
  재시작·전체 ranked-page 조회는 통과했다. 300초 IPC 제한을 변경하지 않았다.
  남은 두 실패는 backup fixture의 Sourcegraph 결과 ID 기대값이었다.
  CodeSearch는 고정 file ID, Sourcegraph ownership projection은 원래 chunk ID를
  유지하는 기존 계약에 맞게 테스트를 수리했다. 수리 후 두 사례는 실행 대기다.
- **확정 오류 분류 누락 / 수정 후 실행 대기**: ObjectStore가 미지원 pooled
  object format을 `GenerationSidecarCorrupt`로 반환해 restore의 content fallback에
  들어갈 수 있었다. 기존 `GenerationManifestFormatUnsupported`로 분리하고
  observed/supported 숫자를 남겼다. 고정 21-byte header와 restore 오류 전파·무삭제
  회귀를 추가했다. malformed header/unknown kind의 corrupt 분류는 유지한다.
- **VERIFIED — 정적 범위**: `just rust-module-cycles rust-test-authority
  rust-wire-inventory rust-error-shape rust-no-allow rust-derive-allowlist`,
  `just fmt-check`, `git diff --check`. 마지막 format 분류 수정은 개별 rustfmt와
  diff 검사만 실행한 상태이며 최종 Cargo 실행에서 다시 확인한다.

workspace strict Clippy, 최종 owner/backup 회귀, SDK 실제 프로세스 publication,
새 optimized Large/XL은 후속 실행 전까지 **NOT_RUN**이다. 이 절의 pass 수를
이전 실행과 합쳐 고유 테스트 수 또는 전체 green 결과로 표현하지 않는다.

### 10.11 Backup/restore 경계까지 실제 실행 (2026-10-10)

- **FAILED — 마지막 수리 집중 실행**: `./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-searchd-runtime -p quanta-index-search-plane
  -p quanta-index-lexical --lib --test state_migration_owner_v1 --all-features
  --locked`, restore 상계/Storage/미지원 format, 고정 header, map cache와 backup을
  선택했다. 9 run / 8 pass / 1 fail / 989 skipped, 6.743초. backup restore의
  File/Native/owner 조회는 통과했지만, 후속 old-incarnation delta fixture가
  잘못된 fresh stream을 선언해 실제 allocation-scope 검사보다 lineage 검사에서
  먼저 거절됐다. 기대값을 약화하지 않고, 복원된 G1의 실제 coverage publication을
  parent로 입력하도록 fixture를 고쳤다.
- **VERIFIED — 최종 backup/format 집중 실행**: 같은 package/target에서
  `real_search_corpus_backup_|restore_does_not_convert_unsupported|fixed_object_header`
  선택. 4 run / 4 pass / 994 skipped, 6.279초. 실제 pool 포함 backup/restore,
  누락/손상 Native body backup 거절, incarnation 회전 후 full replacement 성공,
  올바른 parent를 가진 old-incarnation delta의 scope 거절과 G3 무생성을 확인했다.
- **strict lint 중 발견한 테스트 코드 수리**: unnested pattern, 불필요한 clone,
  hex 임시 문자열, lock guard 수명, 독립 GC parser의 타입 분기를 정리했다.
  suppressions를 추가하지 않았고 production 경로 또는 테스트 기대값을 완화하지
  않았다. 최종 workspace Clippy는 별도 실행 결과를 따른다.

이 시점에 관측된 daemon/backup 오류는 모두 소스 수리와 해당 집중 회귀를 마쳤다.
최종 owner 전체 재실행, SDK 실제 프로세스 publication, optimized Large/XL과
정식 release/heavy qualification은 아직 **NOT_RUN**이다.

### 10.12 Optimized Large 실행의 실제 query 비용 결함 (2026-10-10)

- **VERIFIED — 최종 정적 검사**: `./scripts/cargow --lane test-daemon-lane
  clippy --workspace --all-targets --all-features --locked -- -D warnings`,
  `just fmt-check rust-workspace-lints rust-hexagonal rust-no-allow
  rust-derive-allowlist rust-cargo-toml-hygiene rust-module-discipline
  rust-module-cycles rust-error-shape rust-digest-fallibility rust-test-authority
  rust-ignored-test-policy rust-wire-inventory proof-authority-lint` 및
  `just rust-machete` 통과. 이후 literal 경로 수정은 별도 재검증한다.
- **VERIFIED — optimized 실행 파일 빌드**: dirty 작업 source를 독립된 clean
  validation checkout으로 복사한 뒤 `./scripts/cargow --lane test-daemon-lane
  build -p quanta-index-searchd-harness --bin scale_matrix --release --all-features
  --locked` 통과. 빌드 통과를 벤치 통과로 표현하지 않는다.
- **FAILED — Large 4,096파일**: 기본 `scale-supported-v1` 정책으로
  `python -m tools.benchmark.retrieval.causal_cost_capture --tier large` 실행.
  controller 99.106초 후 exit 1. full ingest/seal/activation 이후 고정
  `code_search_full_probe`에서 `LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED` / `global query
  authenticated I/O exceeds policy`를 반환했다. 성공 summary는 생성되지 않았다.
  source/binary 변경은 없었고, timeout/history/I/O 상한을 덮어쓰지 않았다.
- **확정 구조적 비용 결함 / 수정 중**: literal 경로는 모든 gram posting을
  완전히 읽은 뒤 교집합을 계산한다. Large anchor의 18개 gram 중 공통 접두어
  gram이 많은 파일의 current locator를 반복 인증한다. 오류는 posting 작업의
  16,384 physical-read 또는 512MiB decode 한도에서 발생하며, 기존 로그로 어느
  차원 또는 네 anchor 질의 중 어느 질의인지는 확정할 수 없다. query meter의
  중복 계산은 소스 추적에서 확인되지 않았다. 기존 descriptor count 기반
  `necessary_candidates`의 희소 seed 및 후보 내부 교집합을 literal에도 연결한다.
  마지막 실제 원본 검증과 기존 한도는 유지한다. 후속 실패에는 질의 문맥을 남긴다.

수정 후 집중 회귀, optimized Large 재실행, XL 및 SDK 실제 프로세스 publication은
아직 **NOT_RUN**이다. 실패한 실행의 일부 구간을 성공 benchmark로 승격하지 않는다.

### 10.13 Literal 수리 후 Large 통과와 남은 갱신 비용 (2026-10-10)

- **VERIFIED — literal 집중 회귀**: `./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-lexical --lib --test l3_exact_source --all-features
  --locked`의 CodeSearch/short-scan/fold/AND/sparse selector. 47 run / 47 pass /
  359 skipped, 78.796초. 새 2,048파일 literal·regex·Both·언어 제한 oracle은
  기존 eager 조회의 membership 하한이 16,384를 넘는 입력에서 기존 read/byte cap을
  유지한다. `posting_probes`는 최종 파일 수가 아닌 실제 posting membership
  charge 차이를 보고한다.
- **FAILED → VERIFIED — strict lint**: anchor 오류에 질의 문맥을 붙이는 과정에서
  `anyhow::Context` import 누락으로 컴파일 실패했다. import 수정 뒤
  `clippy --workspace --all-targets --all-features --locked -- -D warnings`
  재실행 통과. 알고리즘/지원 한도를 변경한 실패가 아니다.
- **VERIFIED_DIAGNOSTIC — optimized Large 4,096파일**: clean validation revision
  `88296e1af895cc39272a128e164611c27865a5d3`, seed `5864059738136528177`, 기본
  `scale-supported-v1`로 capture 완료. source/binary 전후 일치, controller exit 0,
  161.517초. 20개 고정 CodeSearch file-anchor 검사 및 Native 결과 parity 통과.
  이는 아래 후속 비용 수리 이전 source이며 최종 성능 수용 또는 경쟁 비교가 아니다.

| Large 측정 | 관측값 | 범위 |
| --- | ---: | --- |
| 초기 구축 | 87.468초 | ingest + seal, activation 제외 |
| 증분 갱신 | 19.360초 | 1파일/2,160B 변경 입력부터 seal까지 |
| 증분 활성화 | 65.718ms | 활성화 + reclaim |
| 무변경 seal | 4.347초 | 신규 event/generation, replacement/tombstone 없음 |
| 삭제 seal | 27.272초 | repo0의 source 1개 삭제 |
| 재오픈 | 5.648초 | 같은 OS process의 daemon thread 재시작, page cache 유지 |
| Native warm P50 / P95 | 10.653 / 11.857ms | 32 samples |
| 상주 registry 회계 / sampled peak RSS | 16.704 / 517.953MiB | 서로 다른 지표 |

후속 source 수리 범위:

- 게시의 candidate 단독 exact-closure 검사는 바로 뒤 required-set 보존 계획의
  동일 검사와 중복이다. durable write 이전 plan의 누락/쿼터/무결성 거절을 유지하며
  중복 호출만 제거한다. 영속 비검증 용량 캐시는 만들지 않는다.
- Native producer, 전이 증명, 변경 segment 증명은 각 pass의 새 4MiB MapPage 캐시를
  사용한다. 생산자의 인증 상태를 독립 증명으로 넘기지 않는다.
- 공통 map diff는 child fence 변화에서 전체 subtree merge로 후퇴한다. ordered
  frontier로 동일한 인증 subtree를 건너뛰되 모든 변경 key/value와 정렬 순서,
  split/merge/height 변화, 취소·손상 거절을 독립 oracle로 유지한다.
- 기존 `seal_file_admission_ns=6.704초`는 Native 전이/새 segment/Tantivy coverage와
  File 증명을 합친 값이다. File 단독 비용으로 해석하지 않는다. 다음 실행은 네
  검증 하위 단계와 Native/File 게시를 개별 진단 span으로 나눈다.

Native는 파일 하나를 바꿔도 최대 2MiB cohort를 bounded compaction으로 다시 만든다.
상속 coverage 해시, 보존 closure/track 검사와 이 cohort 재작성 때문에 전체 ingest
O(delta)를 주장하지 않는다. 후속 수리의 통합 회귀·Large/XL 재측정은 **NOT_RUN**이다.

### 10.14 공통 map·캐시·보존 비용 회귀와 XL 실패 (2026-10-10)

- **VERIFIED — 구조 수리 집중 회귀**: `./scripts/cargow --lane
  test-daemon-lane nextest run -p quanta-index-lexical -p
  quanta-index-search-plane --lib --all-features --locked -E
  'test(/(lexical_store|paged_transition|paged_verify|search_corpus_history)/)'
  --test-threads 2 --no-fail-fast`. 76 run / 76 pass / 862 skipped,
  실행 23.944초. 고정 BTreeMap oracle, split/height 변화, shared branch 읽기 절감,
  손상·취소, producer cache와 독립 proof cache, candidate 1회 계산 및 durable write
  이전 쿼터 거절을 포함한다. cache 계측은 실제 물리 읽기 0회이면 실패한다.
- **FAILED — 수리 이전 optimized XL 32,768파일**: 10.13과 같은 clean source
  `88296e1af895cc39272a128e164611c27865a5d3` 및 binary로 기본
  `scale-supported-v1` 실행. `full_seal` 요청이 600초에 IPC timeout을 반환했고,
  ingest 정리는 `HardDeadlineEscalated { unfinished: ["ingest-accept"] }`로
  실패했다. controller 788.845초, producer exit 1, capture의 외부 timeout은 false.
  source/binary 전후 일치. 성공 summary 또는 XL 갱신·검색 결과는 없다.
- **XL 게시 증폭 관측**: 완료된 Native/File root의 두 atomic publication 경계와
  소스 호출 순서로 로그를 나누면, batched physical object writes는 Native
  287,038개, File 112,991개였다. adoption full barriers는 각각 4,523회와 2,164회.
  각 sync 호출의 elapsed 합은 각각 약 273.7초와 122.0초이다. sync 타이머는 서로
  중첩되지 않지만 이 합은 stage 전체 wall time이나 독립 device I/O 분해가 아니다.
  BM25 census 6.435초/live 2.506초를 전체 봉인 비용으로 대체하지 않는다.
- **남은 계측 한계**: 이전 source는 Native/File wall span과 object-kind 집계가
  없어 어느 map/body/posting이 객체 수를 지배하는지 확정할 수 없다. 새 source의
  `native_authority_publish`, `file_authority_publish` 및 네 seal 하위 span으로
  수리 후 비용을 확인한다. 읽기 캐시와 `map_changed_keys` 수리는 producer의
  `map_apply` 쓰기 수를 줄이는 변경이 아니다.

관련 owner 전체 재실행을 진행 중이다. SDK 실제 프로세스 검증과 이 수리 source의
optimized Large/XL은 아직 **NOT_RUN**이다. timeout·quota·adoption 상한 증가 또는
fixture 축소로 실패를 성공으로 바꾸지 않는다.

### 10.15 Native 초기 구축의 중간 COW 게시 제거 (2026-10-10)

10.14의 물리 게시 증폭에 대해 Native의 비어 있는 root 구축을 직접 고친다.
기존 방식은 2MiB cohort마다 누적 current/membership/segment/gram/term map에
`map_apply`를 수행한다. 최종 root에 필요하지 않은 중간 COW 페이지도 저장·채택된다.

- 내부 `NativeBootstrapMaps`는 각 cohort의 bounded edit 집합을 임시 SQLite에
  보관한다. `(map namespace,key)`의 primary key로 중복을 거절하고 삭제 입력도
  거절한다. heap cache는 2MiB, mmap은 비활성화하며, 정렬과 journal은 디스크를 쓴다.
- 모든 payload/record는 scratch 입력 이전에 기존 durable attempt에 채택한다.
  scratch는 authority 또는 재시작 상태가 아니며 기존 attempt 복구를 대체하지 않는다.
- current/membership/segment/gram/term 및 compaction debt가 모두 비고 아직
  compaction에서 새 입력을 소비하지 않은 경우에만 이 경로를 사용한다. 각 map의
  정렬된 fallible cursor를 기존 `map_bootstrap`으로 한 번씩 소비한 뒤 root를 설치한다.
  nonempty delta/compaction, source map, ID 예약·증가 및 봉인 증명은 유지한다.
- source 정적 교차 검토에서 scratch drop 순서와 `journal_mode=OFF`의 rollback
  보장 부재를 보완했다. connection을 먼저 닫고 scratch를 제거하며, 디스크 rollback
  journal과 `synchronous=OFF`를 사용한다. 임시 DB에 durability 의미를 부여하지 않는다.

이 변경의 독립 oracle 회귀 및 성능 재측정은 아직 **NOT_RUN**이다. 10.14의 owner
실행은 이 변경 이전에 컴파일된 바이너리이며, 그 결과로 새 초기 구축을 검증했다고
주장하지 않는다. 물리 게시 감소량과 XL 성공 여부는 새 바이너리의 실제 결과를 따른다.

### 10.16 통합 회귀의 실제 결과와 Native bootstrap 집중 검증 (2026-10-10)

- **FAILED — bootstrap 수리 전 owner 배치**: `./scripts/cargow --lane
  test-daemon-lane nextest run`의 `quanta-index-{core,lq-regex,lq-trigram,lexical,
  semantic,search-plane,searchd,sdk,ipc,scan-experiment}`, `--lib --tests
  --all-features --locked --test-threads 2 --no-fail-fast`. 72 binary,
  2,346 run / 2,345 pass / 1 fail / 17 skipped, 실제 테스트 1,301.302초.
  lexical/Native/File/공유 손상 격리·보존·활성화·SDK/IPC 회귀가 포함됐다.
  이 실행은 중간에 source에 추가한 Native bootstrap 이전에 컴파일됐다.
- **확정 fixture 불일치**: semantic의 earlier-format refusal 1건은
  `downgrade_to_v7` fixture가 format 13에서 추가된 policy/view-policy 필드를
  남겨 `unexpected manifest shape: 23 fields`로 실패했다. 현재 병렬 source에
  두 필드 제거가 이미 반영되어 있으므로 중복 수정하지 않았다. format 7의 고정
  21필드 검사와 모든 door의 `GENERATION_MANIFEST_FORMAT_UNSUPPORTED` 기대값은
  유지한다. 현재 source로 해당 실패를 별도 재검증한다.
- **VERIFIED — Native bootstrap 이후 집중 회귀**: `./scripts/cargow --lane
  test-daemon-lane nextest run -p quanta-index-lexical --lib --test
  l2_file_mutation --test generation_delta_base_carryforward --all-features
  --locked -E 'test(/(paged_bootstrap|text_authority|owned_delta_noop_and_delete|
  nonempty_noop_delta|delta_text_authority_update_admits_only_the_changed_scope)/)'
  --test-threads 2 --no-fail-fast`. 18 run / 18 pass / 415 skipped,
  85.540초. 새 회귀 4개, independent Native verifier, 독립 full rebuild
  parity, 실제 seal을 거치는 증분·무변경·삭제를 포함한다.
- **독립 branch oracle**: 초기 2-cohort 구축 뒤 stable ID 3을 남기고 ID 1·2를
  retire하며 ID 4를 추가해 nonempty COW를 확인한다. stable membership/query/body를
  보존하고, clear-all로 ID 5를 추가한 뒤 이전 ID 3·4의 부재를 확인한다.

큰 Native 페이지 경계 회귀와 earlier-format 실패 재검증은 진행 중이다. 최종 strict
lint, SDK 실제 daemon process, 이 bootstrap source의 optimized Large/XL은 **NOT_RUN**이다.
2,345개 pass와 후속 18개 pass를 하나의 최종 전체 suite 성공으로 합치지 않는다.

### 10.17 최종 경계 회귀 및 strict lint (2026-10-10)

- **VERIFIED — 경계 및 earlier-format 재검증**: `./scripts/cargow --lane
  test-daemon-lane nextest run -p quanta-index-lexical -p quanta-index-semantic
  --test text_authority_shards --test vector_index_contract --all-features
  --locked -E 'binary(text_authority_shards) | test(a_generation_sealed_under_an_earlier_format_is_refused_typed_at_every_door)'
  --test-threads 2 --no-fail-fast`. 9 run / 9 pass / 12 skipped,
  253.142초. Native page 경계에서 독립 rebuild parity, 한 scope 변경의 불변
  segment 보존, 선택 본문 및 current-page 손상 거절, unsupported layout,
  unsealed skew 및 semantic 이전 포맷의 모든 door typed refusal을 포함한다.
- **VERIFIED — 현재 통합 source strict lint**: `./scripts/cargow --lane
  test-daemon-lane clippy -p quanta-index-contract -p quanta-index-core
  -p quanta-index-lexical -p quanta-index-ipc -p quanta-index-semantic
  -p quanta-index-repomap -p quanta-index-search-plane -p quanta-index-sdk
  -p quanta-index-searchd-runtime --all-targets --all-features --locked -- -D warnings`.
  실행 40.22초, exit 0. 같은 checkout에서 병렬 owner가 실행한 결과를 확인해
  중복 실행하지 않았다. 처음 실행에서 추가한 테스트/helper의 lint 6건이
  실패했고, 현재 source의 해당 수리 후 재실행은 통과했다. vendor Tantivy의
  기존 capped warning 8건은 남아 있다. 전체 workspace/release 검증은 아니다.
- **VERIFIED — SDK process 입력 build**: `./scripts/cargow --lane
  test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd
  --all-features --locked`. debug build exit 0, 18.66초. crash hooks를 포함한
  이 바이너리를 checkout 밖에 복사하고 원본과 SHA-256 일치를 확인했다.

SDK의 실제 daemon process 6개 수용 테스트와 이 source의 optimized Large/XL은
아직 **NOT_RUN**이다. build와 lint를 process 동작 또는 성능 성공으로 승격하지 않는다.

### 10.18 SDK process 실행 실패의 분류와 fixture 수리 (2026-10-10)

- **FAILED — 실제 SDK process 첫 실행**: `QUANTA_INDEX_L2_TEST_BINARY=<fresh
  debug daemon> ./scripts/cargow --lane test-daemon-lane nextest run
  -p quanta-index-sdk --test l2_daemon_publication --all-features --locked
  --run-ignored all --test-threads 1 --no-fail-fast`. 6 run / 4 pass / 2 fail,
  60.431초. 활성화 실패 후 재시작, prepared text/markdown 변경·삭제·무변경,
  reservation 재시작/명시 활성화, cross-stream 순서는 통과했다.
- 첫 실패는 paged producer에서 제거한 eager collect/shard subclock의 존재를
  기대한 계측 oracle이다. 세 subclock은 `None`, 실제 Native/File enclosing
  stage는 양수이고 lexical build 안에 포함되어야 한다는 현재 계약으로 수리했다.
  full/delta/noop/delete의 검색 결과·receipt·lineage 검사는 유지한다.
- crash 회귀는 앞의 6개 cut을 실제 exit 86 및 복구로 통과한 뒤, 두 track 회수
  사이 cut에서 directory 개수 oracle이 실패했다. 실제 lexical root의
  `lexical-object-pool`을 generation pair로 잘못 셌다. root별 canonical pair
  이름만 허용하고 lexical pool을 별도로 확인하며 foreign entry는 거절한다.
  실제 g1/g2/g3 존속/회수 검사는 완화하지 않았다. 남은 2개 cut과 전체 process
  재실행이 통과하기 전 crash 8개 완료로 세지 않는다.
- **VERIFIED — fixture 수정 후 정적 검사**: `just fmt-check rust-module-cycles
  rust-test-authority rust-wire-inventory rust-no-allow rust-error-shape
  rust-derive-allowlist`, exit 0. `git diff --check`와 untracked source의 trailing
  whitespace 검사도 통과했다. process 재실행 및 fixture의 새 strict lint는 후속이다.

생산 소스는 수정하지 않았다. SDK 재실행에는 동일 debug binary SHA-256
`ae0a0233c1f67c16b4caa631614cdd43e18d7f079c2a661a16097fb9e0253134`를 사용한다.
성능용 clean export `631061ee908c4218c021cf5c275580e4e9f84a10`은 이 test-only 수정
이전 snapshot이다. 생산 소스가 같음을 대조했고 optimized build는 3분 46초에
통과했다. benchmark 결과와 SDK fixture 재실행의 source 범위를 구분한다.

### 10.19 SDK 통과, Large 재측정 및 XL 재실패 RCA (2026-10-10)

- **VERIFIED — SDK 실제 process 재실행**: 10.18과 같은 daemon binary와 명령,
  fixture 수리 source에서 6 run / 6 pass / 0 skipped, 53.352초. 예약/활성화,
  full→delta→noop→delete, 구세대 pin, 재시작, cross-stream, prepared text/markdown,
  8개 named crash cut을 포함한다. `clippy -p quanta-index-sdk --test
  l2_daemon_publication --all-features --locked -- -D warnings`도 exit 0,
  22.24초에 통과했다. SIGKILL/명시 종료를 power-loss proof로 승격하지 않는다.
- **VERIFIED_DIAGNOSTIC — 새 optimized Large**: clean export
  `631061ee908c4218c021cf5c275580e4e9f84a10`, binary SHA-256
  `4b4ce3c2b5f68ecff43b1dda870dfe3db62c76ff26b809b5ebb4e77723a8bcac`.
  같은 seed `5864059738136528177`, 4,096파일, 소스 8,607,106바이트,
  `scale-supported-v1` 기본 상한에서 controller 127.019초, exit 0.
  source/binary 전후 일치, clean 유지. 고정 CodeSearch anchor 및 Native parity 통과.
  구축 79.575초, 1파일/2,160바이트 갱신 11.857초, noop 2.871초,
  삭제 12.863초, same-process daemon-thread reopen 4.870초.
  Native warm p50/p95 11.254/11.522ms(32 samples), cache 회계 17,515,203바이트,
  process RSS 531,578,880바이트. cache 회계는 RSS가 아니다.
- **Large 갱신 분해**: Native 게시 3.884초, File 게시 1.221초,
  봉인 하위 검증 합 약 1.978초, lexical build 7.545초,
  finalize 3.927초, semantic 0.132초. 일부 timer는 enclosing/child 관계이므로
  모두 합산하지 않는다. 이전 실행보다 짧지만 다중 수리와 host/cache 차이가 있어
  개별 patch의 독립 절감량 또는 통계적 성능 수용으로 주장하지 않는다.
- **FAILED — 같은 binary의 XL**: 32,768파일, source framing 113,056,314바이트,
  기본 client 요청 600,000ms에서 `full_seal` IPC timeout. controller 약
  757초에 실패 정리를 마쳤다. 성공 summary, XL delta/query 결과는 없다.
  Native 게시 178.161초, File 게시 168.619초, Native 전이 검증 9.663초,
  Native 신규 segment 검증 65.620초, File 전이/segment 검증 239.730초.
  File 검증 완료는 client timeout 이후 cleanup 중 관측한 값이다.
- **초기 쓰기 증폭 수리의 범위**: 새 XL의 physical new-object sync는
  Native 117,991회 + File 112,991회 = 230,982회. 이전 실패 실행의
  400,029회보다 작다. Native batch barrier 1,855회는 64-ref 상한의 최소치에
  가깝다. 단일 수리의 wall 절감량으로 귀속하지 않는다. 별도 단계의 sync elapsed
  합을 physical device I/O 또는 전체 stage wall time으로 표시하지 않는다.
- **소스 확정 반복 검증**: File route-diff visitor가 route마다 term-map 조회와
  같은 PostingPage 전체 read/decode를 반복한다. segment source oracle와 순차
  page 검증은 이미 존재한다. 또한 MapPage cache는 raw 인증 bytes만 보관해
  매 point lookup에서 전체 page CBOR를 재디코드한다. 이 두 경로를 각각 소유
  모듈에서 수리하고 독립 source/posting/route 양방향 검증을 유지한다.
- **구현 공백 분리**: Native는 단일 retire에도 surviving cohort를 즉시 compact한다.
  freshness 자체는 current/live-local 필터로 유지된다. 현재 eager 정책은 부채를
  억제하지만 계획의 soft trigger·merge reserve·target admission과 동일하지 않다.
  따라서 sustained churn을 포함한 전체 LQ4/LQ5 수용은 아직 **NOT_RUN**이다.
  임의 threshold를 넣어 이 비용을 background로 옮겼다고 완료 처리하지 않는다.

위 성능 수리 이후 focused/runtime/optimized Large·XL은 다시 검증한다. 자연어 검색
품질, 경쟁 엔진 비교, quiet-host qualification, 실제 제품 전환 및 원격 heavy CI는
이 진단 결과에 포함되지 않는다.

### 10.20 XL 반복 검증의 구조 수리 (2026-10-10)

- **MapPage point lookup**: 요청/proof별 기존 4MiB LRU에서 raw 인증 bytes와
  decoded `Arc<Page>`를 함께 회계한다. point lookup은 hit 시 전체 page CBOR를
  재디코드하지 않는다. full `PageRef`의 fences/count/height/object 일치를 검사하고,
  miss는 기존 SHA·dependency·canonical page 검증을 거친다. 새 proof는 별도의
  meter/cache로 재인증한다. cache에서 evict된 caller-owned Arc는 bounded traversal
  scratch이며, retained-cache 상한을 전체 process RSS 상한으로 표시하지 않는다.
- **File source/posting/route proof**: source에서 독립적으로 유도한 gram/membership
  oracle는 유지한다. `TermPageStream`이 정렬된 term map과 posting page의 전체
  inventory를 대조하며 page를 연속 1회 읽는다. 새 segment의 expected route는
  이 source oracle 루프에서, 제거 segment의 expected route는 인증된 sealed-base
  term/page 순회에서 계산한다. 전역 정렬 route diff의 before/after를 exact segment
  `BlobRef`와 segment/surface별 fixed-40-byte key SHA-256·건수로 대조한다.
  누락·추가·잘못된 count/value·변경 대상 외 segment를 거절하며 witness 상태는
  변경 segment 수에 비례한다. producer가 제공한 checksum을 oracle로 쓰지 않는다.
- **Native changed-segment proof**: current record와 그 `BlobRef`를 한 번에
  조회·인증해 membership 비교에 사용한다. 같은 key의 두 번째 map 조회를 제거하며
  기존 caller와 current ID/kind/SHA 검사는 유지한다.
- **새 회귀**: 독립 300-row lookup/Arc 재사용/foreign descriptor 거절, cache eviction
  및 fresh-scope tamper, File full→delta→delete와 route 누락·추가·count·value 위조,
  세 gram을 가진 한 posting page의 인증 physical read 2회(map page + posting page)를
  검사한다. 별도 lane의 소스 교차 감사에서 확정 correctness 결함은 발견하지 못했다.

wire/disk 포맷, durability, quota, timeout은 변경하지 않았다.

- **VERIFIED — 집중 실행**: `./scripts/cargow --lane test-daemon-lane nextest run
  -p quanta-index-lexical --lib --test l2_file_mutation --test
  generation_delta_base_carryforward --all-features --locked -E
  'test(/(lexical_store|file_authority::paged::verify|paged_transition|paged_verify|
  paged_bootstrap|owned_delta_noop_and_delete|nonempty_noop_delta|
  delta_text_authority_update_admits_only_the_changed_scope)/)'
  --test-threads 2 --no-fail-fast`. 53 run / 53 pass / 382 skipped,
  84.999초. 새 oracle와 기존 journal/GC/map/bootstrap/증분 회귀를 포함한다.
- **VERIFIED — 정적 경계**: 10.18의 `just fmt-check rust-module-cycles
  rust-test-authority rust-wire-inventory rust-no-allow rust-error-shape
  rust-derive-allowlist` 재실행 exit 0. strict lexical Clippy 첫 실행은 새 코드의
  상수 산술 1건과 test-only redundant clone 3건으로 실패했다. 상수 크기는
  `size_of::<[usize; 2]>()`로 동일하게 표현하고 마지막 사용의 clone을 제거했다.
  수정 후 `clippy -p quanta-index-lexical --all-targets --all-features --locked
  -- -D warnings`는 exit 0, 36.36초에 통과했다.

finalize의 두 source-finalization transaction 합은 Large 10.19에서 0.15–0.27초다.
remaining 수초를 그 중복 호출 탓으로 귀속하지 않는다. pair별 quota는 retained
객체의 전체 SHA 인증으로 정확한 reachable closure를 계산하고, root-wide quota는
garbage/unsealed까지 포함한 nofollow stat/inode census로 계산한다. 현 2세대 보존
벤치에서 동일 retained set의 확정 중복 측정은 찾지 못했다. closure/census/GC 각각의
wall 분해는 별도 span이 없어 미확정이다. 이 O(corpus) 소유자 비용을 제거했다고
주장하지 않는다.

이 시점의 optimized Large·XL 및 현재 생산 소스의 SDK process는 **NOT_RUN**이었다.
후속 실행은 아래에 구분한다.

### 10.21 Large 재실행과 측정 observer 분리 (2026-10-10)

- **VERIFIED — Large correctness 진단**: 10.20의 세 수리를 포함한 release binary에서
  4,096파일의 full→delta→noop→delete→같은 OS process 내 daemon 재개방을 완료했다.
  export revision `04ae82003de3202f899bf9454139bb4e61a187a5`, binary SHA-256
  `be357910a3c9284be9730f43de83b8e328006404dcb7495e27a7615fdc112a6d`.
  controller 553.793초, 초기 구축 245.821초, 1파일 증분 81.916초, noop 27.507초,
  삭제 82.753초, 재개방 36.518초였다. source framing 8,607,106바이트,
  registry 17,513,731바이트, whole-process peak RSS 509,820,928바이트였다.
  이전 Large 10.19보다 크게 느려져 **성능 수용은 실패**했다. 실제 OS process 재시작은
  이 벤치의 범위가 아니며 SDK process 검증과 구분한다.
- **소스·계측 확정**: Native 초기 게시 sync label별 호출 수는 두 실행 모두
  39,708회였고, 그 timer 합 증가는 6.270초였다. Native 전체 게시 증가는
  56.305초여서 sync timer만으로 설명되지 않는다. decoded page cache는 기존 map을
  읽는 경로에 적용되며, 빈 current/due map의 초기 bootstrap 주경로에는 적용되지 않는다.
  이 근거로 새 cache 또는 host 부하를 전체 지연의 확정 원인으로 귀속하지 않는다.
- **벤치의 intrusive observer**: 기존 harness는 작업 실행 중 500ms마다 state root
  전체를 재귀 순회한다. 새 Large의 periodic disk probe 누적 wall envelope는 full
  91.637초, delta 49.011초, noop 15.565초, delete 50.858초였다. 이 값은 작업과
  동시에 실행한 observer 비용이므로 작업 wall에서 빼거나 인과적 절감량으로 쓰지 않는다.
  leaf causal profiling을 꺼도 이 disk worker는 계속 실행되는 별도 경로였다.
- **FAILED — leaf profile 비활성 Large 진단**: 동일 frozen binary에서
  `QUANTA_INDEX_CAUSAL_PROFILE_V1=0`으로 실행했으나 재개방 phase의 RSS sampling
  gap 831.585ms가 기존 500ms 상한을 넘어 typed refusal로 종료했다. 417.331초의
  실패 실행이며 성공 summary 또는 대체 성능 결과로 사용하지 않는다.
- **구현 — canonical 관측 계약**: latency 기본 모드는 `boundary-only`로 시작/종료
  census만 수행하고 작업 중 disk worker를 만들지 않는다. `periodic`은 디스크 high-water
  진단을 명시적으로 선택한 모드다. 두 모드는 artifact와 config digest에 결속하고,
  CLI·capture·replay가 동일 enum/string 계약을 사용한다. boundary-only의 periodic peak는
  unavailable로 기록한다. RSS sampling/4GiB ceiling, 입력·timeout·durability·quota·검색
  oracle는 유지한다. 모드가 다른 이전 결과를 same-config ablation으로 제시하지 않는다.
- **VERIFIED — 관측 계약 회귀**: `./scripts/cargow --lane test-daemon-lane nextest
  run -p quanta-index-searchd-harness --lib --bin scale_matrix --all-features --locked
  -E 'binary(scale_matrix) | test(scale::tests::)' --test-threads 1 --no-fail-fast`:
  50 run / 50 pass / 120 skipped, 17.508초. 첫 compile은 기존 거대 `json!` 표현식의
  recursion limit을 넘어 실패했다. 전역 limit 변경 대신 두 nested JSON을 사전 계산해
  동일 artifact shape를 유지했고 재실행에서 통과했다. boundary worker 미생성·경계 bytes,
  periodic 샘플, 모드별 config digest, mode/peak 모순 및 sample-derived gap 위조 거절,
  CLI default/명시값/잘못된 값 거절과 기존 source lifecycle을 포함한다.
- **VERIFIED — capture/replay**: `uv run --frozen --extra dev python -m pytest -q
  tools/ci/tests/test_causal_cost_capture.py tools/ci/tests/test_causal_cost_profile.py
  --tb=short`: 105 pass. 명령의 requested/effective 모드를 결속하고, 위조된 phase mode,
  interval, sampled maximum/gap 및 boundary timer의 JSON boolean alias를 거절한다.
  새 관측 필드 없는 artifact는 명시 거절하며 legacy IR를 추가하지 않았다. Ruff 통과.

- **VERIFIED — 현재 생산 코드의 실제 SDK process**: `./scripts/cargow --lane
  test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd
  --all-features --locked`로 새 daemon을 만들고 binary를 외부 임시 경로에 고정했다.
  SHA-256 `afcd48f0845767019646e2b8c325de932b86569421229892ce826c3b73064e64`.
  `QUANTA_INDEX_L2_TEST_BINARY=<frozen-daemon> ./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked
  --run-ignored all --test-threads 1 --no-fail-fast`: 6 run / 6 pass / 0 skipped,
  52.563초. 실제 OS process 재시작, full→delta→noop→delete, prepared text/markdown
  move/delete/noop, 원 publication 보존, 예약 generation/activation 및 named crash-cut
  복구를 포함한다. 10.20의 세 수리 이후 실행이며 harness 관측 모드의 증거는 아니다.

- **VERIFIED — 반복 compaction과 backup/restore**: `./scripts/cargow --lane
  test-daemon-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-runtime
  --test generation_delta_base_carryforward --test state_migration_owner_v1
  --all-features --locked -E 'test(native_compaction_cycles_preserve_current_and_held_source_views)
  | test(/real_search_corpus_backup/)' --test-threads 1 --no-fail-fast`:
  3 run / 3 pass / 71 skipped, 13.270초. 새 test helper의 `&&str` 인수 compile 오류를
  `&str`로 고친 뒤 실행했다. 작은 2-source corpus의 replace→delete→recreate→paired
  replace→replace→noop에서 실제 세 publication의 compaction counter 증가를 확인했다.
  모든 고정 marker의 candidate IDs/count·source SHA·generation을 독립 source oracle와
  비교하고, held g1 reader가 원 결과를 유지하며 noop의 source/compaction 재유도가 없는지
  검사했다. 실제 source corpus의 File/Native 검색을 backup→restore 뒤 재검증하고,
  missing/corrupt pooled Native body의 backup 거절도 확인했다. 대형 sustained-churn의
  부채·공간 압력·latency 수용을 대신하는 결과는 아니다.

- **VERIFIED — 소비자와 lint**: `nextest run -p quanta-index-searchd-harness --bin
  open_loop_matrix --all-features --locked -E
  'test(/history_budget_uses_scale_policy_bounds_and_reaches_real_seal|open_loop_artifact_binds_requested_and_effective_history/)'
  --test-threads 1 --no-fail-fast`: 2 run / 2 pass / 18 skipped, 1.957초. 기본 mode를
  전달하는 기존 open-loop 소비자의 history/runtime config 결속을 확인했다.
  `clippy -p quanta-index-lexical -p quanta-index-searchd-harness --all-targets
  --all-features --locked -- -D warnings`는 새 API의 `must_use`와 test-only clone 지적을
  고친 뒤 exit 0, 58.78초에 통과했다. 두 명령 모두 기존 `./scripts/cargow --lane
  test-daemon-lane`으로 실행했다. 10.20의 정적 경계 7종도 다시 통과했다.

observer 분리 이후 새 optimized Large·XL은 **NOT_RUN**이다.
XL 10.19의 600초 timeout을 성공으로 대체하지 않는다.

### 10.22 Boundary-only optimized 재측정 (2026-10-10)

- **VERIFIED — optimized build**: `./scripts/cargow --lane test-daemon-lane build
  -p quanta-index-searchd-harness --bin scale_matrix --release --all-features --locked`
  는 외부 고정 소스와 기존 target에서 exit 0, 2분 58초에 완료했다. export revision
  `da5b036f7869fedfde39c3c6dfd57ddc010bf8c9`, binary SHA-256
  `36f9a99205a196950f9f90c32d8d162e2c6ab6af60eaf1acd0d26a6f863cc4c4`.
  이후 원 checkout의 crate·retrieval 도구·Cargo 파일 1,027개를 대조해 차이 0건을
  확인했다. 원 HEAD는 `8f11cad9587aec1668a9fb3f5f1f055337487071`이며, dirty main
  자체를 clean release 또는 전체 qualification으로 표시하지 않는다.
- **VERIFIED — Large correctness 진단**: `python -m
  tools.benchmark.retrieval.causal_cost_capture --cwd <frozen-source>
  --binary <frozen-release>/scale_matrix --source-revision <above-export>
  --binary-sha256 <above-sha256> --tier large --seed 5864059738136528177
  --disk-observation boundary-only --max-seconds 900 --out-root <external-output>`.
  기존 global resource admission 아래 실행했다. 첫 admission 대기는 1,800초에
  미실행 종료했고, 두 번째 대기 2,200.098초 후 실제 controller 101.549초에
  exit 0이었다. admission 대기를 engine latency나 engine timeout으로 계산하지 않는다.
  source revision·binary SHA는 실행 전후 동일했고 export는 clean이었다.

| Large 4,096파일 | 실제 결과 |
| --- | --- |
| 초기 구축 | 55.098초 |
| 1파일 2,160바이트 증분 | 11.440초 |
| no-op seal | 3.545초 |
| 삭제 seal | 11.995초 |
| 동일 OS process 내 daemon 재개방 | 4.867초 |
| 재개방 첫 query | 1.836ms |
| warm Native query p50 / p95, 32 samples | 11.750 / 12.666ms |
| lexical registry 회계값 / whole-process sampled peak RSS | 17,512,894 / 534,675,456바이트 |

- **계측 계약 확인**: 모든 phase의 disk worker sample 수와 periodic probe wall은
  0이다. 시작/종료 census만 관측했고 periodic disk peak는 unavailable로 남겼다.
  RSS 4GiB ceiling·500ms 최대 sampling gap, IPC 600초, source bytes·membership
  admission·durability·quota·검색 oracle는 바꾸지 않았다. 새로운 artifact는
  `VERIFIED_DIAGNOSTIC`이며 leaf profiling과 host 변동이 있는 진단이다.
  이전 periodic 결과와 같은 config의 ablation 또는 수리별 인과적 배속을 주장하지 않는다.
- **증분 비용**: 11.440초는 lexical 6.621초 + finalize 4.433초 + semantic 0.131초
  + unattributed 0.254초다. BM25 live build는 5.739ms, census는 변경 문서 1개,
  source admission 원문 검증도 변경 파일 1개·framed 2,161바이트다. noop/delete의
  source admission 검증은 0바이트다. 유지 root의 SHA closure·quota/census 비용을
  제거하거나 whole-call O(delta)를 달성했다고 주장하지 않는다. nested timer는 중복 합산하지 않는다.
- **FAILED — XL 보존 용량**: 동일 source·binary·seed·boundary-only·cap의
  `--tier xlarge`은 controller 578.251초에 producer exit 1이었다. 첫 요청의 `xl`
  이름은 argparse에서 거절되어 엔진이 실행되지 않았고 canonical `xlarge`으로 수정했다.
  실제 실행의 Native 게시 185.523초, File 게시 153.780초, Native 전이 10.729초,
  changed-segment proof 56.630초, File proof 38.822초였다. 초기 게시 under-lock은
  532.809초에 `SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED`로 거절했다.
  active/candidate 한 세대의 lexical+semantic 합계 1,142,692,886바이트가 기존 pair
  1GiB 상한보다 68,951,062바이트 컸다. 입력은 32,768파일·raw 113,023,546바이트,
  framed 113,056,314바이트다. capture timeout은 false였으나 초기 activation·증분·query는
  실행하지 못했다. 이전 600초 timeout과 이번 용량 실패를 모두 보존하며 XL 성공으로
  표시하지 않는다. quota는 encoded bytes 기준이고 4KiB filesystem block 수와 다르다.
- **최종 read-only 감사 범위**: Native/File retirement와 다음 unsealed publication의
  compaction output은 seal 전 독립 proof에 연결돼 있다. 추가 core correctness 결함은
  확정하지 못했다. 현재 inline compaction은 §4.4의 별도 staged worker·soft trigger·merge
  reserve 정책과 다르다. 정책 구현·대규모 sustained churn/long pin/space-pressure 수용과
  §9의 launcher 전환·독립 프로세스 복원 리허설은 전체 LQ4/LQ5 출시 gate에 남는다.
  작은 세 compaction 주기·in-process restore 및 별도의 SDK process 재시작 통과를
  이 운영 수용의 대체 증거로 합치지 않는다.

### 10.23 Quota 실패 후 저장 중복 수리 (2026-10-10)

- 회계상 object 이중 계산은 확인하지 못했다. pair measurer는 lexical+semantic을
  합산하며 lexical의 generation 파일은 inode, shared closure는 SHA로 중복 제거한다.
  따라서 실패 수치를 Native/File 단독 또는 allocated blocks로 귀속하지 않는다.
- `QLMAP001` leaf는 키마다 동일한 81바이트 `BlobRef`를 반복 저장했다. File의
  per-segment gram map과 Native posting page의 여러 route는 실제로 같은 blob을 공유한다.
  새 `QLMAP002`는 내부 `Page::Leaf`와 object dependency graph를 유지하고, 같은 leaf의
  정렬된 full-BlobRef dictionary + 2바이트 row index가 inline보다 엄격히 작은 경우만 선택한다.
  dictionary 비용은 `2 + 81*U + 2*N`, inline은 `81*N`이다. 동률은 inline이며 두 모드의
  공통 tag는 비교에서 제외한다. 96행/참조 1개의 새 inline 대비 절감은 7,501바이트,
  구 wire 대비는 tag 차이 때문에 7,500바이트다. 전체 XL 감소량은 재측정 전 미확정이다.
- decoder는 fanout 이내 table count, strict ordering/uniqueness, index bound, 모든 entry
  사용과 최소 mode를 검사한다. 기존 fences/height/count/exact dependency 검증도 유지한다.
  잘린 bytes·trailing bytes·이전 magic은 typed refusal이며 legacy reader를 추가하지 않았다.
- 동시에 진행된 Native body 수리는 NFC text만 저장하고 같은 normalizer로 folding을
  재생성한다. 기존 두 표면 합산 32MiB admission을 유지하며 query·compaction·seal·scrub이
  같은 decoder를 사용한다. 이 수리를 중복 구현하지 않고 coupled source에 포함한다.
- 봉인 format 19, File root 18, Native root 6으로 이전한다. format authority는 기존
  `lexical_root_names`에 하나만 두고 attempt binding도 같은 봉인 version을 사용한다.
  policy/hash 도메인의 v17은 의미가 바뀌지 않아 유지한다. format 17/18 open과 base
  inheritance refusal을 보완하며 새 input/root로 명시적 rebuild한다.
- **VERIFIED — 정적 경계**: wire inventory가 이동한 constant와 새 format을 아직 반영하지
  않은 첫 검사에서 실패했다. inventory를 같은 변경으로 갱신한 뒤 `just fmt-check
  rust-module-cycles rust-test-authority rust-wire-inventory rust-no-allow rust-error-shape
  rust-derive-allowlist`가 exit 0이었다.
- **FAILED — 첫 집중 runtime**: lexical store/manifest, 새 게시·base inheritance,
  Unicode body, delta/noop/delete, bootstrap와 세 compaction 주기를 합쳐 120개 실행,
  118개 통과/2개 실패였다. 두 실패는 legacy format 10/12의 typed rebuild refusal을
  검사하는 테스트에서 현재 version을 아직 `serves 18`로 기대한 fixture 오류다.
  production은 올바른 format 19 refusal을 반환했다. 독립 legacy 입력 10/12는 유지하고
  기대 메시지만 `serves 19`로 고친 두 테스트를 별도로 재실행한다. 앞선 118개 통과를
  재실행 전체 120개 통과로 바꾸지 않는다. changed-scope source admission은 122.758초,
  작은 세 compaction 주기는 10.778초에 통과했으며 이를 반복하지 않는다.
- **VERIFIED — 새 SDK daemon build**: 같은 warm target의 `--all-features --locked`
  dev build는 exit 0, 23.03초였다. frozen binary SHA-256은
  `e8b051343b82c4398874fa64f5dd6a472c4dbfe82445f489d319d6c8909d84c8`이다.
  `QUANTA_INDEX_L2_TEST_BINARY=<frozen-daemon> ./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked
  --run-ignored all --test-threads 1 --no-fail-fast`: 6 run/6 pass/0 skip, 71.741초,
  exit 0. 실제 OS process 재시작과 named crash-cut의 delta/activation 복구를 포함한다.
- **VERIFIED — strict lexical lint**: `./scripts/cargow --lane test-daemon-lane clippy
  -p quanta-index-lexical --all-targets --all-features --locked -- -D warnings`:
  exit 0, 51.37초. dependency Tantivy의 기존 8개 warning은 repository lint 성공과 구분한다.
- **FAILED — 추가 독립 query oracle의 첫 실행**: `./scripts/cargow --lane
  test-daemon-lane nextest run -p quanta-index-lexical --test unicode_normalization_goldens
  --test text_authority_shards --test l2_file_mutation --all-features --locked
  --test-threads 2 --no-fail-fast`: 55 run/54 pass/1 fail/0 skip, 355.727초.
  실패는 current manifest fixture의 기대 version 17과 실제 19의 불일치다.
  해당 독립 기대 literal을 19로 수정한 테스트 하나만 재실행한다. 생산 코드는 변하지 않았다.
  Unicode 고정 golden, composed/decomposed token/phrase 동등성, File mutation/corruption,
  shard 경계의 independent rebuild oracle(266.599초), 한 scope delta의 inherited segment
  보존·fresh object byte 상한(185.703초)이 통과했다. 두 큰 fixture는 반복하지 않는다.
- **VERIFIED — 새 optimized build / 벤치는 대기**: current dirty source를 외부 clean export
  `2e210fd849e1c22c3856be7e7c0d04f03223610d`로 고정했다. source 1,887개,
  working-source SHA-256 `74d46e5dd0d04b8d12a3e09bbd642cc1216d77e63082af1a5846bb3a8d368ce3`.
  기존 release target을 재사용한 새 build는 exit 0, 6분 29초였다. binary SHA-256은
  `0fb003adc198e91e5fe109f6b3c2a0aa193cb2b7317e5f5931de32bd3ba0fd2d`다.
  별도 경로에 고정한 뒤 Large capture를 admission 대기에 넣었다. pair 1GiB,
  total 2GiB, process RSS 4GiB, IPC 600초, capture 900초와 seed는 유지한다.
  map dictionary와 단일 Native body가 함께 적용되므로 둘 사이의 단독 효과를
  분리한 비교 또는 독립 검색 품질 SOTA 주장으로 사용하지 않는다.

### 10.24 새 저장 포맷 실행 검증과 진단 벤치 (2026-10-10)

- **VERIFIED — 실패 fixture의 제한 재실행**: format 10/12의 rebuild refusal 기대
  문구 수정 후 lexical lib의 해당 두 테스트는 2 run/2 pass/386 skip, 0.022초,
  exit 0이었다. Unicode current-format literal 수정 후 해당 integration 테스트 하나는
  1 run/1 pass/8 skip, 1.450초, exit 0이었다. 최초 120개/55개 실행의 실패 기록은 유지한다.
- **VERIFIED — 새 포맷 backup/restore**: `./scripts/cargow --lane test-daemon-lane
  nextest run -p quanta-index-searchd-runtime --test state_migration_owner_v1 --all-features
  --locked -E 'test(real_search_corpus_backup_restore_preserves_file_and_native_queries)
  | test(real_search_corpus_backup_refuses_missing_or_corrupt_pooled_native_body)'
  --test-threads 1 --no-fail-fast`: 2 run/2 pass/55 skip, 7.667초, exit 0.
  이 테스트는 in-process restore이며 §9의 독립 OS process 전환 리허설과 구분한다.
- **FAILED — Large capture 후처리 / VERIFIED — producer와 독립 replay**:
  Large6는 121.328초에 producer exit 0, timeout false였고 summary의 모든 oracle가
  통과했다. controller를 Python 3.9.6으로 실행해 `zip(..., strict=...)` 후처리에서
  실패했다. pyproject의 지원 범위는 Python >=3.10이다. 원 `execution.json`의 FAILED와
  raw 자료를 보존하고, Python 3.12.12에서 공식 `causal_cost_profile` CLI로 같은
  binary·summary·manifest·trace를 별도로 replay했다. exit 0이며 profile은
  `diagnostic_unqualified`이다. producer를 반복 실행하거나 원 실패를 성공으로 덮어쓰지 않았다.
  원 실행 전후 source revision/binary SHA 일치와 clean 상태, stdout/stderr SHA를
  replay 후 다시 대조했다. binary source custody는 10.23의 fresh build이며 이 replay가
  새로운 producer 실행 또는 formal qualification을 발급한 것은 아니다.

| Large6 / 4,096파일 | 관측 |
| --- | ---: |
| 초기 구축 | 70.251초 |
| 한 파일/2,160B 변경 ingest→seal | 18.530초 |
| 새 event의 noop seal | 2.755초 |
| 삭제 seal | 8.718초 |
| 같은 OS process의 daemon thread 재개방 | 4.608초 |
| 재개방 후 첫 query | 1.527ms |
| Native warm query 32개 p50 / p95 | 23.177 / 26.717ms |
| registry cache 회계값 / process sampled peak RSS | 17,514,194 / 502,939,648B |
| full / delta / noop / delete의 unique retained bytes | 82,097,198 / 98,284,582 / 82,240,790 / 99,133,068B |

- 저장량은 Large5의 full 103,369,676B에서 82,097,198B로 줄었다. 반면 delta는
  11.440초에서 18.530초, warm p95는 12.666ms에서 26.717ms였다. 현재 자료로 속도
  개선을 선언하지 않는다. 여러 source 변경이 함께 적용된 단일 진단이며 quiet-host
  반복/교차 비교가 아니므로 map/body 각각의 인과적 회귀·개선으로 단정하지 않는다.
  새 delta의 lexical build 9.025초, finalize 8.987초, BM25 live build 3.830ms,
  census 1문서/0.229ms를 구분한다. BM25 수치가 전체 증분 비용을 대표하지 않는다.
- **진행 중 — XL4**: 동일 source/binary/seed, `xlarge` 32,768파일, 기본 1GiB pair/
  2GiB total/600초 IPC/900초 capture/RSS 4GiB/boundary-only로 실행한다.
  이번 controller는 지원되는 Python 3.12.12를 사용한다. XL3 quota 실패는 보존하며
  XL4의 전 phase와 query oracle 완료 전 성공으로 표시하지 않는다.
- 외부 export와 현재 production 소스는 대조했으며 이후 차이는 위 Unicode 테스트의
  독립 기대 literal 17→19 하나다. 테스트 수정은 생산 바이너리에 영향을 주지 않는다.
  전체 source tree가 같거나 whole-workspace qualification이 끝났다고 합치지 않는다.

### 10.25 XL4 실패의 원인 경계와 중복 실행 방지 (2026-10-10)

- **FAILED — XL4**: 같은 fixed source/binary에서 controller 774.946초, producer
  exit 1, capture timeout false였다. 초기 봉인의 IPC read 600,000ms timeout과
  RSS sampling gap 36,382.819ms >500ms가 함께 발생했다. cleanup은 unfinished
  `ingest-accept`를 남겨 `HardDeadlineEscalated`, daemon exit 70이었다. 자원 lease는
  cleanup 이후 반환됐으며 성공 summary/manifest/profile 또는 증분 결과는 없다.
  original quota failure XL3와 새 timeout/observation failure XL4를 별도로 보존한다.
- 부분 기록은 Native publish 238.774초, File publish 186.685초,
  Native transition 60.702초, Native changed-segment 검증 107.444초,
  File transition/segment 검증 48.262초다. finalize 19.299초는 실패했고,
  publish-under-lock 717.996초도 실패였다. timeout 이후 cleanup 중 나온 clock을
  정상 client latency나 성공한 봉인으로 바꾸지 않는다. 이 실행에서 새로운 quota
  수용량, activation, delta/noop/delete 성공을 확인하지 못했다.
- **VERIFIED — 소스 RCA의 한계**: old `da5b036…`와 new `2e210fd…` 사이 Native
  transition/changed-proof/writer 파일은 같았다. 이전 body decoder도 indexed text를
  fold해 저장된 folded text와 비교했고, 새 decoder도 fold를 한 번 계산해 반환한다.
  fold 반복 증가 가설은 지지되지 않는다. map2는 fanout<=96의 참조 정렬/중복 확인을
  추가했지만 단일 실행과 큰 RSS 관측 공백만으로 전체 지연의 원인이라고 단정하지 않는다.
  proof 범위나 인증을 줄이는 임시 우회는 적용하지 않았다.
- finalization의 현 계약은 보존 generation directory의 unique-inode census,
  모든 reachable object의 SHA 인증·digest dedup, 별도 physical track census다.
  `search_corpus_history.rs`, `adapter_lifecycle.rs:927`, `lexical_gc.rs:242`의 이
  corpus 전체 비용은 남아 있다. Large6 finalize 8.987초 안의 source transaction
  합계는 0.270초다. 나머지는 retention/GC의 세부 계측이 없으므로 quota 단독 시간으로
  귀속하지 않는다. map2가 추가 whole-closure 재순회를 넣었다는 근거는 없다.
- **NOT_RUN — 중복 XL5 대기 취소**: 다른 병렬 작업이 같은 fixed source/binary/
  seed/caps로 `/private/tmp/qi-oct10-final-native6-xl-awake`에서 이미 실행 중인 것을
  확인했다. 추가 요청 `/private/tmp/quanta-paged-xl-20261010-final-5`는 resource slot
  진입 전에 SIGTERM으로 취소했다(84.199초 대기, exit 143, not-admitted).
  기존 lease나 다른 작업을 중단하지 않았다. 새 실행은 `caffeinate -is`로 절전을
  방지하며 algorithm/timeout/RSS/quota 계약은 유지한다. 이 환경 변경 때문에 XL4와
  동일 환경의 인과적 ablation으로 주장하지 않는다. 완료 뒤 실제 source/bin/input,
  실행 상태와 모든 oracle를 대조하기 전 성공으로 표시하지 않는다.

### 10.26 XL 초기 File query 검증 거절과 segment 단위 교집합 계약 (2026-10-10)

- **FAILED — awake XL**: `/private/tmp/qi-oct10-final-native6-xl-awake`는
  §10.23의 export `2e210fd…`, binary `0fb003ad…`, 같은 seed와 기본 caps에서
  controller 589.573초, producer exit 1이었다. full seal과 activation은 통과했지만
  `code_search_full_probe`의 `content:regex:/scalefileneedle001file00000/`가
  `LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED: global query authenticated I/O exceeds policy`로
  거절됐다. 성공 summary/profile과 XL delta/noop/delete/reopen 결과는 없다.
  full quota 통과를 관측했지만 exact retained byte gauge는 수집되지 않았다.
- **VERIFIED — 소스 경로**: `gram_count_estimates`는 이미 authenticated route key의
  posting count로 순서를 고른다. 이후 gram별 `posting_ids`/`posting_ids_within`이
  같은 후보의 current record와 segment/term/posting을 다시 읽는다. default `case:no`의
  regex HIR은 case-sensitive literal과 다른 necessary expression을 만들 수 있다.
  19개 gram이나 6,400개 seed를 실제 실패의 고정 하한으로 사용하지 않는다.
  오류는 16,384회 read 또는 512MiB byte 조건의 공통 메시지이므로 어느 조건과
  어느 gram에서 최초 초과했는지는 아직 확정하지 못했다.
- **구현 계약**: 아직 후보가 제한되지 않은 `Gram`/`And`/`AllDocs`의 2개 이상 gram
  conjunction은 authenticated descriptor count로 anchor를 고르고, anchor route의
  segment마다 local `u64` mask를 교집합한다. 최종 surviving ID만 current
  `(logical ID, segment SHA, local ID)`와 대조한다. 단일 gram, `Or`, 상위 evaluator가
  이미 제한한 allowed ID 집합은 기존 filtered 경로를 유지한다. 좁혀진 후보 때문에
  unrelated segment의 posting을 다시 읽는 회귀를 피한다. actual source exact matcher는
  그대로 호출한다.
- **인증·자원 계약**: selected term/page/route의 kind, digest, length, count와 locator
  관계를 계속 확인한다. anchor route 모순은 corruption이며 non-anchor term 부재는
  해당 segment의 empty intersection이다. decoded PostingPage 재사용은 한 segment의
  request-local full BlobRef identity에만 한정한다. 실제 I/O와 selected posting
  membership을 모두 기존 QueryWork에 부과한다. producer/wire/한도는 바꾸지 않는다.
- **NOT_RUN — 새 수리 검증**: 작은 fixed-golden numeric-gram/long-conjunction fixture,
  stale replace/delete, path/content, nested boolean, corruption/budget 검증을 먼저 한다.
  독립 oracle와 정적 검토가 수렴한 뒤 warm release를 한 번 갱신하고 XL 전체 lifecycle을
  다시 실행한다. finite class 경계 gram 확장은 이 결함 수용에 필수인 것으로 확인되지
  않았으므로 현재 수리에 추가하지 않는다.

### 10.27 File query 수리의 집중 검증과 실행 소스 결합 (2026-10-10)

- **VERIFIED — 집중 runtime46**: `./scripts/cargow --lane test-daemon-lane nextest run
  -p quanta-index-lexical --lib --all-features --locked -E
  'test(file_authority::paged::verify::tests) | test(file_authority::paged::reader::tests)
  | test(file_authority::query::tests) | test(searcher::code_search::tests)'
  --test-threads 2 --no-fail-fast`: 37 run/37 pass/355 skip, 66.518초, exit 0.
  fixed 128-document regex 후보·exact near-miss, 19-gram conjunction과 physical read
  상한 224/512, replace/delete, path/content, nested Boolean, 선택된 corruption,
  language/Unicode/ranking/cursor와 shared request budget을 확인했다.
- **VERIFIED — 캐시 runtime48**: 같은 wrapper/features/lock의 lexical lib에서
  `-E 'test(lexical_store::store::journal_tests::request_metadata_cache)'
  --test-threads 1 --no-fail-fast`: 2 run/2 pass/390 skip, 0.603초, exit 0.
  종류·길이·digest로 구분한 metadata cache, forged object/blob descriptor 거절,
  새 request의 재인증, 이미 만료된 budget 거절, shared 4MiB bound와 퇴출 재읽기를
  검증했다. lock 대기 중 취소와 multi-call scrub 중간 외부 변조는 이 범위에 포함하지 않았다.
- **VERIFIED — 정적 rail**: `just fmt-check rust-module-cycles rust-test-authority
  rust-wire-inventory rust-no-allow rust-error-shape rust-derive-allowlist` exit 0;
  `./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-lexical
  --all-targets --all-features --locked -- -D warnings` exit 0, 31.76초.
  focused 결과가 whole-workspace 또는 product qualification을 뜻하지 않는다.
- 다른 병렬 작업의 metadata cache는 MapPage만에서 Record/Segment/FilePostings까지
  확장됐다. body/block은 캐시하지 않으며 retained 4MiB 상한은 공유한다. 본 수리의
  segment-local decoded PostingPage와 함께 통합 검증했다. 2,432-read 하한은
  이전 `2e210fd…`의 MapPage-only reader에 해당하며, 새 캐시와 segment-major의 효과를
  각각 분리해 측정한 ablation은 아니다. 한도 거절에는 reads/bytes의 실제값과 한도를
  함께 표기하도록 진단을 보완했다.
- **NOT_RUN — 새 Large/XL 결과**: 중복 build/bench를 추가하지 않는다. 다른 병렬 작업이
  clean export `a15d994989f547a80d7b102a76429481503cc2d0`에서 warm release를 갱신 중이다.
  현재 Rust/Cargo 1,229파일과 해당 export의 byte equality를 확인했다. 새 binary SHA,
  실행 전후 clean source와 caps, 성공/거절 및 모든 lifecycle oracle를 대조한 후 결과를
  수용한다. 이전 실패 artifact는 보존한다. benchmark 준비나 compile 시작은 완료 증거가 아니다.

### 10.28 File query 수리 후 fresh release와 Large 진단 (2026-10-10)

- **VERIFIED — fresh release**: export `a15d994989f547a80d7b102a76429481503cc2d0`에서
  `QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR=1 CARGO_TARGET_DIR=<warm target>
  ./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-harness
  --bin scale_matrix --release --all-features --locked` exit 0, 3분 31초.
  binary `/private/tmp/qi-oct10-final-cache-scale-matrix`, SHA-256
  `ba1b69cdf692987546097bc4e748670abcece9e08d49dba074ce4f8fd5cf5929`.
  byte-equal Rust/Cargo 1,229파일을 대조했으며 별도 중복 build는 하지 않았다.
- **NOT_RUN — 첫 capture 입력 거절**: binary를 `/tmp/...`로 전달한 호출은 macOS의
  `/tmp` symlink 거절로 producer 실행 전 exit 2였다. 실제 `/private/tmp/...`로 정정해
  같은 binary를 사용했으며, 빌드나 engine 실패로 바꾸지 않는다.
- **VERIFIED — Large 진단 capture**: `.venv/bin/python -m
  tools.benchmark.retrieval.causal_cost_capture --cwd <export> --binary <frozen binary>
  --source-revision a15d994989f547a80d7b102a76429481503cc2d0 --binary-sha256 <above>
  --tier large --seed 5864059738136528177 --disk-observation boundary-only
  --max-seconds 900 --out-root /private/tmp/qi-oct10-final-cache-large`를 공용 resource
  admission 아래 실행했다. 104.836초, producer exit 0, timeout false,
  `VERIFIED_DIAGNOSTIC`, 모든 scenario oracle pass였다. 실행 전후 source/bin 동일,
  clean export와 stdout/stderr/profile/binary digest를 별도로 다시 대조했다.
  profile은 `diagnostic_unqualified`이며 quiet-host qualification이나 SOTA 비교가 아니다.

| File query 수리 후 Large / 4,096파일 | 관측 |
| --- | ---: |
| 초기 구축 | 57.842초 |
| 한 파일/2,160B 변경 ingest→seal | 12.615초 |
| 새 event noop seal | 3.142초 |
| 삭제 seal | 13.999초 |
| 같은 OS process daemon thread 재개방 | 5.435초 |
| 재개방 첫 Native query | 1.813ms |
| warm Native query 32개 p50 / p95 | 11.283 / 12.240ms |
| registry 회계값 / process sampled peak RSS | 17,514,507 / 580,927,488B |
| full / delta / noop / delete unique retained bytes | 82,128,734 / 98,316,118 / 82,272,336 / 99,152,834B |

- File regex/literal, source isolation, changed/deleted-source 결과도 검증했으나 표의
  32-query latency는 Native route 수치다. File regex별 latency나 자연어 품질 점수는
  이 artifact에 별도 집계되지 않았다. source 변경과 request cache가 함께 적용된
  단일 진단이므로 각 최적화의 독립 배속이나 전 실행 대비 인과적 개선을 주장하지 않는다.
- **XL 후속 결과**: 같은 fixed source/bin/seed/caps의
  `/private/tmp/qi-oct10-final-cache-xlarge` 결과는 §10.29에 기록한다. 이전의
  full-seal·query 거절 기록은 유지하며 전체 lifecycle oracle가 끝나기 전 XL 성공을
  발급하지 않는다.

### 10.29 File query 수리 후 XL 누적 시간 초과와 retention 비용 경계 (2026-10-10)

- **FAILED — XL 전체 capture**: §10.28의 clean export `a15d994…`, binary
  `ba1b69c…`, seed와 기본 caps로 실행했으나 900.079초 외부 capture 한도에서
  중단됐다. `execution.json`의 `timed_out=true`, `exit_code=null`이며 실행 전후
  source revision/binary SHA는 동일하고 `dirty_after=false`다. stdout은 비어 있고
  성공 summary, tier manifest, causal profile은 생성되지 않았다. 한도나 timeout을
  올려 성공으로 바꾸지 않는다.
- **VERIFIED — 실행된 검색 범위**: stderr의 full/delta/noop/delete seal·activation
  단계는 모두 `phase_end ok=1`이다. `measure_tier_with_runtime_config`와
  `measure_scoped_delete_reopen`의 제어 흐름상 각 단계의 CodeSearch anchor 4질의,
  합계 16질의는 통과했다. 이전 specific regex의 `LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED`
  거절은 이번 실행에서 재발하지 않았다. 이는 실행된 고정 fixture 범위의 증거이며
  전체 corpus/query workload 또는 모든 regex의 성공으로 일반화하지 않는다.
- 마지막 마커는 `same_process_reopen` 시작이다. **재오픈 완료 증거는 없으며**,
  이후 first query와 삭제 후 재오픈 CodeSearch 검증은 **NOT_RUN**이다. 900초는
  전체 capture의 누적 ceiling이다. 재오픈이 900초 걸렸다는 뜻이 아니며 stop/start
  각각의 소요는 현 마커만으로 분리할 수 없다.

| XL / 32,768파일의 완료된 ingest trace 범위 | 관측 |
| --- | ---: |
| full `ingest_publish_under_lock` | 559.921초 |
| 한 파일 delta `ingest_publish_under_lock` | 61.815초 |
| noop `ingest_publish_under_lock` | 48.014초 |
| delete `ingest_publish_under_lock` | 67.577초 |
| 위 네 개의 비중첩 publish 합계 | 737.327초 / 외부 시간의 81.9% |
| delta / noop / delete finalization | 47.251 / 46.002 / 49.091초 |

- 표의 publish는 trace가 실제 완료한 내부 범위다. `DeltaMeasurementV1.update_ms`의
  최종 artifact가 없으므로 61.815초를 전체 ingest→seal 수치로 대신 발급하지 않는다.
  authority build, proof, sync, finalization은 publish의 하위 시계이므로 publish
  시간에 다시 더하지 않는다. 대량 causal trace 출력 자체의 교란도 분리 측정하지 않았다.
- **VERIFIED — retention의 소스 비용 구조**: delta finalization 47.251초 안의
  두 `ingest_source_finalization_transaction`은 1.250초와 0.017초다. 나머지
  약 45.984초는 ledger fence와 `record_sealed_search_corpus`를 포함한다.
  세부 계측이 없어 lexical closure 인증, semantic directory 조사, 전체 physical
  track census 각각의 시간을 발급하지 않는다. receipt 뒤 GC는 두 번째 transaction
  내부이므로 이 45.984초 전체를 GC 시간으로 해석하지 않는다.
- `measure_sealed_generations`는 선택된 generation 디렉터리의 inode를 조사하고,
  File/Native root를 인증한 뒤 `measure_unique_closure_bytes`로 reachable object를
  다시 읽고 digest 단위 중복을 제거한다. 별도의 state-root projection은
  `measure_track_physical_bytes`로 shared pool, quarantine, unsealed/reclaim 파일을
  포함한 전체 물리 트랙을 조사한다. 현재 adoption journal은 root를 기록하지만
  재사용 가능한 durable closure membership/refcount 증명은 제공하지 않는다.
  따라서 작은 변경이나 noop에도 `O(reachable objects) + O(track files)` 비용이 남는다.
- **구조 수리의 다음 계약 / 구현 NOT_RUN**: hot seal의 정확한 보존 집합 회계를
  증분화하려면 producer가 인증한 object identity·length·dependency와 root membership,
  generation 간 unique-object 회계를 하나의 canonical owner에서 관리해야 한다.
  seal/retention/pin/GC의 원자적 전이 및 restart recovery와 결속해야 하며, 물리 quota는
  미봉인·고아·reclaim 파일과 hardlink inode도 회계해야 한다. 이전 byte 합계만 캐시하거나
  인증·쿼터를 생략하는 수리는 이 계약을 충족하지 않는다. 최소 변경 범위와 독립 oracle를
  정한 후 producer/retention/GC/recovery owner로 나누어 구현한다.
- 최소 graph 계약은 세대마다 전체 closure 목록을 복제하는 방식이 아니다.
  `ObjectStore`가 full object identity와 canonical dependency/length를 내구적으로
  소유하고, 선택된 root 집합의 공유 DAG 도달 refcount가 0↔1로 전이하는 부분만
  방문한다. 기존 `SealedGenerationReclaimPort`의 pair/track 측정 표면은 유지할 수 있다.
  pair별 unique-object 합계와 전체 track의 physical inode 합계는 별도 권한이다.
  root에서 도달하지 않는 미봉인/고아 객체도 physical quota에서는 빠질 수 없다.
- 검증 oracle는 공유 diamond DAG와 세대 간 COW 공유, cross-pair 공유, hardlink와
  distinct inode의 차이, pinned 세대의 지연 회수, seal/history/reclaim crash 복구,
  pair/track quota 거절 전 무변경, cold 전체 조사와 증분 합계의 일치다. hot work
  카운터가 변경 그래프에만 비례하는지도 확인한다. metadata row만으로 현재의
  untouched object full-SHA 재인증을 대신할 수 없으며, 이를 selected read/bounded
  scrub/cold reconcile로 옮기면 손상 탐지 시점이 바뀐다는 계약을 명시해야 한다.
- 재오픈의 stop은 daemon join과 maintenance 종료를 포함하고, start는 durable history
  복원 및 그 안의 pair/physical 회계, active pair 검증, maintenance 초기 inventory·disk
  조사를 포함한다. 현재 artifact에서는 어느 호출의 시간이 큰지 확인되지 않았다.
  재실행 이후에도 분리가 필요하면 기존 causal marker에 `reopen_stop`/`reopen_start`를
  먼저 분리하고, start 내부 history restore/active proof/maintenance 초기화를 계측한다.
  이 계측 및 증분 회계 구현은 아직 **NOT_RUN**이다.
- finite ASCII class 경계의 necessary gram 추출은 후보 선택도를 높일 수 있는 별도
  최적화다. 현재 regex 계획은 보수적이므로 이 지점에서 검색 누락이 확인된 것은 아니다.
  선택도 부족을 이번 재오픈 누적 timeout의 원인으로 귀속하지 않는다.
- 검증 중복을 피한다. 다른 병렬 작업의 같은 frozen source 전체 suite와 같은
  source/bin/input/caps XL 재실행을 대조하며, 이번 FAILED artifact는 보존한다.
  focused query/cache tests와 Large diagnostic 통과는 XL lifecycle qualification을
  대신하지 않는다.

### 10.30 병렬 독립 RCA와 과도한 수리 범위의 재감사 (2026-10-10)

#### 근거 범위

- retention/finalization, reopen/measurement, query/build의 세 읽기 전용 감사를
  병렬 수행하고 원시 trace와 호출 경로를 교차 확인했다. 운영 코드 변경, 추가
  build/runtime/benchmark는 **NOT_RUN**이다. 다른 작업의 진행 중 suite는 완료
  증거로 포함하지 않는다.
- 기준은 §10.29의 `a15d994…` / `ba1b69c…` XL 실패 artifact다. 실행 전후 identity,
  clean 상태와 stdout/stderr SHA를 다시 확인했다. 현재 `HEAD=8f11cad…`에는 공유
  dirty 작업이 있으므로 HEAD 이름만으로 실행 소스를 표현하지 않는다. 이번 비교에서
  clean export와 현재 checkout의 `crates/**/*.rs`, Cargo manifests/lock 892파일은
  byte-equal이었다.

#### RCA-1: 작은 갱신 뒤에도 보존 확정은 전수 작업이다

| 완료된 내부 구간 | full | delta | noop | delete |
| --- | ---: | ---: | ---: | ---: |
| publish under lock | 559.921s | 61.815s | 48.014s | 67.577s |
| finalization, 위 publish에 포함 | 45.751s | 47.251s | 46.002s | 49.091s |
| source transaction 2회를 제외한 finalization 잔여 | 44.494s | 45.984s | 44.388s | 47.524s |

- delta의 finalization은 내부 publish의 76.4%, noop에서는 95.8%다. 변경량이
  줄어도 같은 규모의 비용이 남는다는 실측과 전수 순회 소스가 일치한다. 이 잔여에는
  retention lock/snapshot/measurement/durable write와 작은 ledger fence가 함께
  들어 있다. 45.984초를 SHA, SQLite, 물리 조사 중 어느 하나에 전부 귀속하지 않는다.
- XL은 64 source repo를 한 serving pair에 담고 `max_generations=2`로 실행한다.
  정책상 각 seal의 pair-set 측정은 정확히 1회다. 물리 projection 역시 lexical와
  semantic track을 각각 1회 조사한다. **한 seal에서 같은 closure를 여러 번 재계산한
  것이 아니다. 매 seal마다 전수 계산을 다시 수행하는 구조다.**
- `readiness/search_corpus_history.rs::record_sealed_search_corpus`는 pair guard와
  전역 `search_corpus_root_lock`을 이 전체 작업 동안 잡는다. 다른 pair의 보존 기록도
  직렬화되는 구조적 비용이 있다. 다중 pair의 실제 대기 시간은 측정하지 않았다.
- lexical pair 측정은 `adapter_lifecycle.rs::measure_sealed_generations`에서
  private inode 조사, root 인증, `lexical_gc.rs::measure_unique_closure_bytes`의
  전체 reachable object open/read/SHA/dependency decode 및 임시 SQLite dedup으로
  이어진다. semantic pair는 inode 조사다. `PairIndexBytesMeasurer`의 physical
  projection은 두 track 전체 inode 조사를 별도로 수행한다. background
  `SharedGcProgress::step`는 이 동기 retention 경로의 원인이 아니다.

#### RCA-2: 초기 구축은 작은 durability group의 확정 비용도 크다

- Native publish 203.918초와 File publish 176.789초는 full publish 559.921초에
  포함된다. 두 producer는 이미 `with_adoption_batch`를 사용한다. batch 누락이라는
  진단은 배제한다.
- `lexical_store/store.rs`는 group을 64 refs 또는 약 2MiB에서 확정한다. XL full
  seal에서 group 4,019개가 발생했다. group마다 epoch 갱신, pending journal head,
  clean journal head가 control file/parent 동기화를 만든다. 관측된 각각 12,065회는
  `3 × 4,019 + 8`과 일치한다. recovery protocol의 단계를 삭제해 비용을 숨기지 않는다.

| full seal의 직접 동기화 호출 | 횟수 | 누적 호출 시간 |
| --- | ---: | ---: |
| pool adoption barrier | 4,019 | 45.723s |
| adoption journal sync | 4,019 | 19.027s |
| control file sync | 12,065 | 62.340s |
| control parent sync | 12,065 | 58.623s |
| AttemptBatch object file sync | 4,019 | 20.136s |
| AttemptBatch object shard sync | 4,019 | 19.748s |
| 위 여섯 종류의 직접 syscall wrapper 합계 | | 225.596s |

- 위 여섯 timer는 각각 직접 sync 호출을 감싸며 서로를 호출하는 parent/child
  timer가 아니다. 합계는 호출 대기 시간이며 full seal wall time이나 제거 가능한
  절약 시간과 동일하지 않다. 상위 publish/build 시간에 다시 더하지 않는다.
- batched data object file sync 230,982회도 관측됐다. 새 content-addressed 객체를
  각각 독립 파일로 쓰는 구조에서 source file당 약 7.05개 data object와 별도의
  AttemptBatch 파일이 생성됐다. 이 파일 수는 후속 open/SHA/stat 순회의 비용도
  키울 수 있으나, 그 효과를 별도로 측정하지 않았다.
- 수리 후보는 기존 owner의 bounded group commit 계약이다. 단순히 64를 올리면
  manifest 크기, object dependency 상한, write epoch/GC fence, 실패 복구 단위가
  함께 영향을 받으므로 상수 하나로 수정하지 않는다. packed immutable storage는
  가능한 후속 대안이며 현재 증거만으로 전체 저장 포맷 재작성을 필수로 정하지 않는다.
- 독립 교차 검토에서 64 refs의 manifest 예약은 2,727B, 128 refs는 5,351B가
  필요함을 확인했다. recovery validator도 같은 참조 상한을 사용한다. group별
  참조 수/새 byte 수와 flush 원인(refs/bytes/명시적 flush)을 먼저 기록해야 한다.
  batch 전체가 항상 64개로 가득 찼다거나 상한 확대가 그대로 배속으로 이어진다는
  주장은 현재 trace로 입증되지 않는다.

#### RCA-2b: 한 파일 delta도 touched segment를 즉시 재작성한다

- 새 XL의 한 파일 delta에서 batched data object 신규 쓰기 5,644회와 adoption
  group 102개가 관측됐다. Native publish 구간은 3,283개/62 group, File publish
  구간은 2,361개/40 group이다. 이 수치는 쓰기 횟수이며 전부 최종 root에서 도달
  가능하다는 증거는 아니다. 중간 root에서만 필요한 객체도 포함될 수 있다.
- Native는 기존 current ID를 retire하면서 아직 live 문서가 있는 segment를
  `compaction_due`에 기록한 다음 **같은 publish**에서 `compact_due_segments`를
  호출한다. 이 함수는 surviving body를 읽어 segment를 다시 만들고 current,
  membership, posting, route map을 수정한다. 단위 상한은 2,048문서/2MiB이며
  이번 실행이 그 상한을 채웠다는 뜻은 아니다. 근거는
  `text_authority/paged_writer.rs`의 retire/compaction 호출과
  `text_authority/paged_wire.rs::SEGMENT_DOCS`다.
- File도 old segment를 touched로 표시한 뒤 같은 build에서 `compact_segment`를
  호출한다. 최대 64문서의 살아 있는 local ID를 확인하고 posting mask를 역변환해
  새 segment/postings/current locator를 작성한다. source body를 전부 다시 파싱하는
  Native 방식과는 다르지만 segment 전체의 쓰기 증폭이 있다.
- 두 producer의 map 수정은 이미 sorted batch `ObjectStore::map_apply`를 사용한다.
  키마다 전체 root를 재작성하는 구현이라는 가설은 배제한다. 확인된 원인은
  **eager segment compaction과 그에 따른 여러 map의 수정**이다. 5,644개 중 각
  ObjectKind 및 compaction/일반 mutation의 정확한 몫은 추가 계측이 필요하다.
- 최소 수리 후보는 기존 current-ID/tombstone 필터를 유지하며 변경분을 추가하고,
  compaction debt를 별도의 bounded/amortized 정책으로 처리하는 것이다. old posting
  후보 제거의 정확성, live-mask/route-count 계약, held view, crash/GC custody,
  debt 진행·상한 및 반복 변경 시 segment/read 증폭을 같이 증명해야 한다. 압축을
  무기한 미뤄 쓰기 시간을 검색 비용과 무제한 저장 증가로 옮겨서는 안 된다.
- delta Native/File publish는 각각 5.400/3.430초다. 이 쓰기 증폭을 finalization
  47.251초의 직접 원인으로 합쳐 말하지 않는다. 두 구간은 별도 수리 대상이다.

#### RCA-3: 과거 검색 거절과 현재 검색 범위를 구분한다

- 과거 awake XL의 589.573초는 전체 producer 시간이다. full publish만 526.635초였으므로
  이를 실패한 regex 한 번의 검색 시간으로 해석하면 안 된다. 해당 거절은 query의
  read/byte budget 초과이며 어느 한도가 먼저 찼는지는 당시 메시지에 없다.
- 현재 segment-major conjunction은 아직 allowed 후보가 제한되지 않은 pure AND의
  서로 다른 gram 2개 이상에만 적용된다. single gram, OR, 이미 제한된 후보는 기존
  generic/filtered 경로다. exact source matcher는 최종 결과를 계속 검증한다.
- 새 XL에서 full/delta/noop/delete 뒤 source-repo Native 검증 256건과 CodeSearch
  File anchor 16건은 제어 흐름상 완료됐다. File 16건은 literal/regex의 고정 앵커다.
  모든 regex 형상, 자연어 품질 또는 File query별 latency를 검증한 것은 아니다.
  finite-class 경계 gram의 부족은 보수적인 선택도 문제이며 현재 timeout의 입증된
  원인이나 검색 누락 결함으로 분류하지 않는다.

#### RCA-4: 전체 시간 제한과 관찰 비용을 분리해야 한다

- 완료된 네 publish 합계 737.327초가 outer 900.079초의 81.9%다. 다른 모든 작업과
  미완료 reopen에 남은 시간은 합계 최대 162.752초다. 재오픈 단독 지연은 알 수 없다.
- `same_process_reopen` 시작 marker는 phase의 시작 boundary probe가 반환한 뒤,
  stop/start 작업 전에 출력된다. 종료 marker가 없으므로 stop/join 또는 start 내부에서
  제한을 맞았다. 이후 경계 teardown 때문이라고 설명할 근거는 없다. 시작 중 active
  pair 검증이 일부 진행됐는지는 **미확정**이다.
  성공적인 reopen 다음에 놓인 검색 검증들은 **NOT_RUN**이다.
- boundary-only도 디스크 probe를 생략하지 않는다. 완료된 9개 phase의 앞뒤와
  reopen 진입에서 boundary probe 19회, 별도 `directory_bytes` 전수 walk 5회가
  호출됐다. boundary probe는 오류를 unavailable로 보관할 수 있으므로 19회 모두
  정상적인 전체 조사였다고 단정하지 않는다. 해당 상태와 observer 시간은 실패한
  실행의 최종 artifact에 저장되지 않았다.
- stderr는 674,091줄, 48,750,141B다. causal capture가 동기 trace를 켜며 outer
  ceiling에는 이 출력, observer setup/teardown, correctness query도 포함된다.
  seal timer는 phase boundary observer 시간을 제외하지만, 내부 trace 출력의 교란은
  포함될 수 있다. 제품 retention 비용은 실제 측정됐으며 benchmark 교란만으로
  47초를 설명할 근거는 없다.

#### 후속 수리 순서와 수용 조건

1. **결정적인 저빈도 계측부터**: retention root-lock wait/snapshot, lexical pair
   closure, semantic pair, 두 physical track, durable history write를 분리한다.
   객체 수·hash bytes·inode 수를 함께 기록한다. reopen stop/join과 start의 runtime
   build/active proof/초기 maintenance 조사도 분리한다. 완료 phase의 work/observer
   시간·상태를 소량의 구조화 로그로 남겨 timeout에도 완료 범위를 복원한다.
2. **초기 게시 비용**: existing batch/adoption owner에서 commit 단위와 metadata
   확정 단계를 함께 검토한다. 새 group 경계의 실패 지점마다 orphan/adoption 복구와
   GC epoch 안전성이 유지되는 독립 oracle가 선행 조건이다. 한 파일 delta의 eager
   compaction은 별도 쓰기 증폭 항목으로 다루고, ObjectKind별 신규/재사용 수와
   compaction rederived docs/bytes를 계측한 뒤 bounded debt 정책을 적용한다.
3. **확정 경로 증분화**: 계측으로 지배적인 closure 또는 physical 조사부터 수정한다.
   §10.29의 refcount graph/physical ledger는 각각의 전수 작업을 제거할 때 필요한
   계약 후보다. 둘을 한꺼번에 구현해야 한다는 결론은 아직 입증되지 않았다. 같은
   바이트 합계뿐 아니라 공유 객체·hardlink·pin·실제 unlink·crash와 손상 탐지 시점의
   계약을 보존하거나 변경 내용을 명시한다. lock 밖 계산에는 버전 검증/예약이 필요하다.
4. **같은 caps의 전체 lifecycle**: fixed source/bin/input으로 delta/noop의 전체 크기
   비례 비용이 제거됐는지 확인하고, reopen 이후 삭제/보존 query까지 완료해야 XL
   성공을 발급한다. 이전 실패, 변경 소스의 새 결과, 비교용 qualification은 구분한다.

이번 RCA의 소스·기존 trace 대조는 **VERIFIED**다. 위 신규 계측/구조 변경과 그 후의
runtime 검증은 **NOT_RUN**이며, 기존 XL 전체 capture는 계속 **FAILED**다.

### 10.31 한 번의 실행으로 보는 waterfall 계약과 추가 감사 (2026-10-10)

후속 계측 계획은 [TRACE-WATERFALL-PLAN.md](TRACE-WATERFALL-PLAN.md)에 정리했다.
운영 코드의 trace 구현 완료를 뜻하지 않는다.

- 세 독립 감사와 계획 교차 검토로 duration-only 로그, success-only 관측값,
  최종 artifact의 실패 시 유실, observer 시간 누락, lazy boot/reopen 내부 공백을 확인했다.
- wire request ID는 같은 connection에서도 재사용할 수 있고 readiness retry는 같은
  envelope를 재전송한다. per-attempt span과 명시적인 diagnostic IPC link가 필요하다.
  request ID·시간 근접성으로 임의 join하지 않는다.
- 기존 진단 port/context를 확장하고 capture 전체가 clock·span allocator·writer를
  소유한다. semantic/lexical build, retention lock/closure/physical 조사, query error-path
  작업량, stop/join/boot를 같은 시간축으로 연결한다. 제품 observer는 별도 lane이다.
- 개별 sync 출력은 고정 key aggregate로 바꾸고 bounded stream을 실행 중 기록한다.
  실패/timeout에서도 가능한 raw prefix로 viewer를 생성한다. sink 오류·미완료·손실은
  별도 상태로 드러내며 성공 duration이나 benchmark PASS를 합성하지 않는다.
- W0 계약 고정 후 transport/runtime, index owner, harness/exporter 세 write set을 병렬화한다.
  구조와 실패 oracle를 검토한 뒤 좁은 runtime, on/off overhead 비교, 필요한 XL 순서로
  검증한다. 이번 계획 작업 때문에 기존 빌드·벤치를 재실행하지 않았다.

현재 소스/기존 artifact 감사와 계획 교차 검토는 **VERIFIED**다. 신규 tracing 구현,
viewer/runtime/overhead 검증은 **NOT_RUN**이다.

§10.31 계획의 마지막 계약 감사에서는 [TRACE-WATERFALL-PLAN.md](TRACE-WATERFALL-PLAN.md)
revision 2로 다음을 보완했다. enqueue 전 번호 발급의 순서 역전, dormant context가 남은
footer 발급, 구버전 ingest의 diagnostic field 거절, token 오류가 제품 오류로 전파되는
경계, legacy env에 묶인 BM25/process-I/O 계측, profile hash의 원본 의미를 각각 수정했다.
writer의 출력 순서와 producer 손실 회계를 분리하고, 종료 장벽·동일 bundle wire 적용·
기존 계측의 단일 경로 전환·profile schema 변경을 명시했다. 오버헤드는 `--timeline` 유무가
아닌 실제 recorder Off/Record로 비교한다. 이 변경은 문서 계획이며 운영 구현은 아직 없다.

revision 3에서는 [TRACE-EXECUTION-PACKETS.md](TRACE-EXECUTION-PACKETS.md)에 P00–P09의
정확한 기존/신규/보호 파일, 허용 hunk, 공통 계약 동결과 파일 소유권 인계, constructor/mock
이행, API/module baseline 갱신, 직렬 검증 순서를 명시했다. A/B/C의 세 chain만 병렬로
수정하며 ROOT 통합 전 lane handoff를 요구한다. 실행 패킷 작성은 구현 완료 증거가 아니다.

### 10.32 LQ5 최소 준비 worker 계약 (2026-10-11)

- 기존 catalog가 예약한 unsealed target에서만 준비한다. source-event reservation,
  adoption lease, base pin, directory lifecycle guard의 소유권은 기존 publication에 있다.
  idle maintenance generation과 별도 physical epoch는 만들지 않는다.
- worker는 읽기와 CPU 준비만 수행한다. File의 기존 debt 한 개와 Native의 기존 debt
  최대 두 개를 처리하며, physical output과 ID는 foreground 채택 단계에서만 만든다.
  추가 staging disk output이 없어 새 orphan/recovery protocol도 필요하지 않다.
- File 준비는 Native logical mutation이 없는 publication으로 제한한다. Native 준비는
  이번 target에서 retire할 ID가 포함된 cohort를 body 읽기 전에 제외한다. 다른 논리
  변경은 foreground가 처리한다. Native worker는 body decode/NFC 검증까지만 준비하고,
  posting build와 physical ID 부여는 foreground에 남긴다.
- process 전체에 worker 또는 준비 packet 하나만 허용한다. 기존 File scratch ceiling
  안에 두 packet과 bounded object-read cache를 포함한다. busy 또는 준비 예산 부족은
  기존 정확한 publication 경로를 사용한다. integrity/I/O 오류는 숨기지 않는다.
  128MiB ceiling에서 read cache 4MiB를 먼저 제외하고 두 packet을 각각 62MiB로 제한한다.
  Native retirement-ID 복사도 Native packet 예산에 포함한다. 이 값은 준비 scratch의
  admission이며 foreground writer 또는 process RSS 전체의 상한이 아니다.
- 준비한 exact descriptor/current/source/membership reference가 실제 변경 이후에도
  유효할 때만 채택한다. drift는 준비를 버리고 다시 도출한다. 논리 ID, unrelated edit,
  기존 touched/debt 선택 규칙, 독립 seal proof는 바꾸지 않는다.
- worker는 publication의 coverage/base clone과 병행하고 writer mutex를 잡기 전에 join한다.
  오류/중단에서는 내부 budget을 cancel하고 join 후 lease를 반납한다. checkpoint를
  전달하되 진행 중 syscall의 강제 중단을 보장하지 않는다. packet은 해당 publication에서
  소비하거나 폐기하며 idle cache에 남기지 않는다. request cancellation API 신설은 범위 밖이다.
- 실제 no-op source event도 이미 예약한 target에서 Native old debt를 최대 두 개 처리한다.
  worker가 busy이거나 packet cap을 넘으면 foreground가 같은 선택을 다시 도출한다.
  새 logical ID 또는 maintenance generation을 발급하지 않고, 준비 유무에 따라 physical
  root가 달라지지 않는다. 기존 Chunk clear의 coverage admission 거절은 유지한다.
- File producer, Native producer, root의 worker/publication integration을 독점 write set으로
  나눈다. 각 lane은 root에만 보고한다. 정적 검토 후 focused tests와 기존 실제 lifecycle을
  직렬 실행한다. 이 계약만으로 sustained throughput, process RSS aggregate, disk reserve,
  quiet timing 또는 LQ5 전체 수용을 발급하지 않는다.


### 10.33 최소 준비 코드와 XL 종료 검증 (2026-10-11)

- §10.32의 File/Native 준비, publication 전 join/cancel, exact-reference 채택과
  no-op debt 처리를 구현했다. `dba0bdb…`에서 strict lexical/harness Clippy,
  기능 회귀 36개 및 통합 113개가 모두 통과했다. 세 번의 compaction cycle,
  stale/cap/cancel, 이전 base 보존, Unicode 및 기존 publication fault oracle를 포함한다.
- driver-stopped owned TempDir에만 최대 4개의 scoped shard worker를 적용하고
  모든 worker join 및 fallible close를 기존 RuntimeStop/deadline 안에서 완료한다.
  링크·비정상 layout의 기존 close fallback과 caller-owned root 경계를 유지한다.
- 새 고정 `07f0430…` release에서 XL은 기존 seed/caps/boundary-only/900초 계약으로
  한 번 실행해 777.167초, producer exit 0이었다. 10 phases, 재오픈 후 query,
  24.121초의 owned-root 정리, artifact write와 Complete footer까지 완료했다.
  Health/loss/open spans는 0이며 이전 47f904a timeout 기록은 보존한다.
- 이는 product/lifecycle 수용이다. host는 foreign Rust 4개/load 13.29834로 contended였고,
  legacy aggregate parity 및 source/build binding이 부족해 capture는
  PARTIAL_DIAGNOSTIC/exit 2다. 정식 성능 수용이나 정리의 인과적 배속은 증명하지 않았다.
- sustained churn/long pin/space pressure, measured merge threshold/reserve 및 post-merge
  admission은 별도 범위다. 최소 준비 worker를 전체 LQ5 구현·운영 수용으로 승격하지 않는다.
  소스·명령·관측 결과는 [VALIDATION.md](VALIDATION.md)의 마지막 절을 따른다.
