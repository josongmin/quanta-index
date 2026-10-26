# Lexical 저장·검색·증분 검증 최종 구조안

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> 기술 계약/날짜별 설계 근거 참고 문서다. 현재 작업·담당·순서는 [Oct-17 SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 갱신한다. 아래 과거 상태/packet/미구현 판정은 현재 배정표가 아니다. 실행 결과는 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md), legacy의 Accepted 결정은 기존 ADR을 따른다.


문서 역할: legacy paged lexical의 당시 기술 설계 참고. 옛 실행 순서와 비용/검증 체크포인트는 [보존 이력](LEXICAL-CHECKPOINTS.md)으로 이전했다. 현재 작업·담당·검증 대기는 [Oct-17 SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 갱신한다. 설계 설명은 최신 Native 구현 또는 성능 수용의 증거가 아니다.

[Legacy closeout 범위](../../plans/oct-10-index-closeout/README.md)의 후속 계획이다. 기존 A/D/C5 완료 조건을 대체하지 않는다.
이 문서는 이전의 F15 포맷 유지·쿼리 우선 계획을 대체한다. 유지할 것은 검색 의미와 소유권이며,
당시 F15/Native LQ의 디스크 포맷과 전체 검증 호출 구조가 교체 대상이었다.

범위 구분: 여기의 Native/File authority는 기존 Tantivy generation에 결합된 paged
경로다. 새 owner-key NativeLiteral/NativeTextIndex와 SQLite Catalog/Kernel 구현은
[Native RFC §1.1](../../rfc/oct-10-native-incremental-vnext.md#11-reading-order-and-decision-boundaries),
§14/§23을 따른다. 두 경로의 저장소·format·GC·성능 증거를 섞지 않는다. 아래 구조 판정은
당시 설계 근거이며 날짜별 후속 실행을 부정하지 않는다. LQ5 의무는 동등한 native 수용과
명시적 legacy 퇴역 전까지 남는다; native 구현 진행만으로 자동 취소되지 않는다.

현재 계측 잔여: [W2-09](../../plans/oct-17-index-closeout/tickets/README.md#w2-09). 옛 packet D 배정은 [RFC 이력](../../rfc/oct-10-native-incremental-vnext.md#2377-parallel-delivery-plan-and-exclusive-write-sets-2026-10-13)으로 보존한다.
Native text_query already implements BM25; map regex/DSL/token gold and actual
callers before declaring a feature missing. Instrument existing harness with
a genuine Native mode, including producer O(N) closure proof and Catalog/WAL/
object/scratch/upkeep cost. The existing legacy scale path is not Native cost.

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


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#7-실행-순서소유권완료-조건)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 7.1 범위별 체크리스트


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#71-범위별-체크리스트)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 7.2 반드시 명시할 자원 정책


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#72-반드시-명시할-자원-정책)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 7.3 작업 분리와 통합


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#73-작업-분리와-통합)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 7.4 조건부 실험


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#74-조건부-실험)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

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


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#10-과거-비용-근거와-현재-검증-상태)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.1 이전 F16 검색 핵심 경로 구현과 검증


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#101-이전-f16-검색-핵심-경로-구현과-검증)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.2 F16 독립 oracle 후속 실행과 paged rewrite 경계 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#102-f16-독립-oracle-후속-실행과-paged-rewrite-경계-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.3 Paged 통합 후 추가 결함과 복구 배치 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#103-paged-통합-후-추가-결함과-복구-배치-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.4 배치 집중 실행과 실제 초기 게시 경로 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#104-배치-집중-실행과-실제-초기-게시-경로-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.5 전체 owner 재실행에서 발견한 정렬·복구 결함 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#105-전체-owner-재실행에서-발견한-정렬복구-결함-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.6 내구성·자원 경계 보완과 재실행 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#106-내구성자원-경계-보완과-재실행-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.7 전체 owner 실행과 후속 구조 수리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#107-전체-owner-실행과-후속-구조-수리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.8 Lexical 재실행과 정책 경계 정리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#108-lexical-재실행과-정책-경계-정리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.9 Daemon 전체 실행에서 발견한 소유권 회귀 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#109-daemon-전체-실행에서-발견한-소유권-회귀-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.10 수정 경로 재검증과 최종 포맷 분류 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1010-수정-경로-재검증과-최종-포맷-분류-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.11 Backup/restore 경계까지 실제 실행 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1011-backuprestore-경계까지-실제-실행-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.12 Optimized Large 실행의 실제 query 비용 결함 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1012-optimized-large-실행의-실제-query-비용-결함-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.13 Literal 수리 후 Large 통과와 남은 갱신 비용 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1013-literal-수리-후-large-통과와-남은-갱신-비용-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.14 공통 map·캐시·보존 비용 회귀와 XL 실패 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1014-공통-map캐시보존-비용-회귀와-xl-실패-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.15 Native 초기 구축의 중간 COW 게시 제거 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1015-native-초기-구축의-중간-cow-게시-제거-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.16 통합 회귀의 실제 결과와 Native bootstrap 집중 검증 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1016-통합-회귀의-실제-결과와-native-bootstrap-집중-검증-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.17 최종 경계 회귀 및 strict lint (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1017-최종-경계-회귀-및-strict-lint-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.18 SDK process 실행 실패의 분류와 fixture 수리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1018-sdk-process-실행-실패의-분류와-fixture-수리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.19 SDK 통과, Large 재측정 및 XL 재실패 RCA (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1019-sdk-통과-large-재측정-및-xl-재실패-rca-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.20 XL 반복 검증의 구조 수리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1020-xl-반복-검증의-구조-수리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.21 Large 재실행과 측정 observer 분리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1021-large-재실행과-측정-observer-분리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.22 Boundary-only optimized 재측정 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1022-boundary-only-optimized-재측정-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.23 Quota 실패 후 저장 중복 수리 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1023-quota-실패-후-저장-중복-수리-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.24 새 저장 포맷 실행 검증과 진단 벤치 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1024-새-저장-포맷-실행-검증과-진단-벤치-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.25 XL4 실패의 원인 경계와 중복 실행 방지 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1025-xl4-실패의-원인-경계와-중복-실행-방지-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.26 XL 초기 File query 검증 거절과 segment 단위 교집합 계약 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1026-xl-초기-file-query-검증-거절과-segment-단위-교집합-계약-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.27 File query 수리의 집중 검증과 실행 소스 결합 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1027-file-query-수리의-집중-검증과-실행-소스-결합-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.28 File query 수리 후 fresh release와 Large 진단 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1028-file-query-수리-후-fresh-release와-large-진단-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.29 File query 수리 후 XL 누적 시간 초과와 retention 비용 경계 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1029-file-query-수리-후-xl-누적-시간-초과와-retention-비용-경계-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.30 병렬 독립 RCA와 과도한 수리 범위의 재감사 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1030-병렬-독립-rca와-과도한-수리-범위의-재감사-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### 근거 범위


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#근거-범위)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### RCA-1: 작은 갱신 뒤에도 보존 확정은 전수 작업이다


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#rca-1-작은-갱신-뒤에도-보존-확정은-전수-작업이다)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### RCA-2: 초기 구축은 작은 durability group의 확정 비용도 크다


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#rca-2-초기-구축은-작은-durability-group의-확정-비용도-크다)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### RCA-2b: 한 파일 delta도 touched segment를 즉시 재작성한다


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#rca-2b-한-파일-delta도-touched-segment를-즉시-재작성한다)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### RCA-3: 과거 검색 거절과 현재 검색 범위를 구분한다


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#rca-3-과거-검색-거절과-현재-검색-범위를-구분한다)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### RCA-4: 전체 시간 제한과 관찰 비용을 분리해야 한다


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#rca-4-전체-시간-제한과-관찰-비용을-분리해야-한다)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### 후속 수리 순서와 수용 조건


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#후속-수리-순서와-수용-조건)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.31 한 번의 실행으로 보는 waterfall 계약과 추가 감사 (2026-10-10)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1031-한-번의-실행으로-보는-waterfall-계약과-추가-감사-2026-10-10)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.32 LQ5 최소 준비 worker 계약 (2026-10-11)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1032-lq5-최소-준비-worker-계약-2026-10-11)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.33 최소 준비 코드와 XL 종료 검증 (2026-10-11)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1033-최소-준비-코드와-xl-종료-검증-2026-10-11)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.34 LQ5 잔여 계약 재대조 (2026-10-12)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1034-lq5-잔여-계약-재대조-2026-10-12)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### 일반 append의 hard-limit 복구 계약


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#일반-append의-hard-limit-복구-계약)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

#### Publication admission 연결과 관측 경계


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#publication-admission-연결과-관측-경계)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.35 Publication capacity foundation과 미연결 경계 (2026-10-12)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1035-publication-capacity-foundation과-미연결-경계-2026-10-12)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.36 통합 활성화와 daemon 수용 차단 경계 (2026-10-12)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1036-통합-활성화와-daemon-수용-차단-경계-2026-10-12)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.

### 10.37 최종 잔여 작업과 native 분업 (2026-10-12)


과거 본문은 [보존 이력](LEXICAL-CHECKPOINTS.md#1037-최종-잔여-작업과-native-분업-2026-10-12)으로 이전했다. 현재 작업은 [SSOT](../../plans/oct-17-index-closeout/tickets/README.md)만 따른다.
