> HISTORICAL ONLY. 당시 소스의 설계·실행 기록이며 현재 작업 지시가 아니다. 현재 작업·상태·담당은 [단일 SSOT](../../plans/oct-13-native-incremental-wave2/tickets/README.md), 실행 판정은 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md)을 따른다. 이 본문에 상태를 추가하지 않는다.

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

후속 계측 계획은 [TRACE-WATERFALL-PLAN.md](../../plans/oct-10-index-closeout/TRACE-WATERFALL-PLAN.md)에 정리했다.
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

§10.31 계획의 마지막 계약 감사에서는 [TRACE-WATERFALL-PLAN.md](../../plans/oct-10-index-closeout/TRACE-WATERFALL-PLAN.md)
revision 2로 다음을 보완했다. enqueue 전 번호 발급의 순서 역전, dormant context가 남은
footer 발급, 구버전 ingest의 diagnostic field 거절, token 오류가 제품 오류로 전파되는
경계, legacy env에 묶인 BM25/process-I/O 계측, profile hash의 원본 의미를 각각 수정했다.
writer의 출력 순서와 producer 손실 회계를 분리하고, 종료 장벽·동일 bundle wire 적용·
기존 계측의 단일 경로 전환·profile schema 변경을 명시했다. 오버헤드는 `--timeline` 유무가
아닌 실제 recorder Off/Record로 비교한다. 이 변경은 문서 계획이며 운영 구현은 아직 없다.

revision 3에서는 [TRACE-EXECUTION-PACKETS.md](../../plans/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md)에 P00–P09의
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
  소스·명령·관측 결과는 [VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md)의 마지막 절을 따른다.

### 10.34 LQ5 잔여 계약 재대조 (2026-10-12)

원래 §4.2/4.4/7의 LQ5는 아직 완료되지 않았다. §10.32의 최소 준비 worker는
target 안의 CPU/read 준비를 제공하며, 다음 운영 정책을 구현한 것으로 해석하지 않는다.

| 원래 요구 | 현재 코드/증거 | 남은 작업 |
| --- | --- | --- |
| hard limit 전 soft merge trigger | 명시적 설정의 soft/hard run·encoded-byte 정책과 bounded rotating 후보 선택 구현; source no-op의 동일 권한·동일 카운터 cursor 예외 연결; 기본 비활성, 측정값 미동결 | 일반 append 거절 후 같은 sealed base에서 반복되는 scan window의 attempt-local 복구, 비용 계측과 후보 수치 동결 |
| aggregate scratch/I/O/disk reserve | 준비 packet과 read cache의 128 MiB 예산; 설정된 pool quota를 boot inventory·신규 쓰기·부분 실패·동일 store GC에 연결; 논리 occupancy scrape 구현, 최신 통합 runtime 수용 전 | 공통 Runtime authority에 publication 전체와 pool 외 비용을 예약; 실제 durable 삭제 실패 후 ledger 복구와 최신 통합 검증 |
| prepared merge 반영 후 target admission | seal 전 run/encoded-byte target 계산과 typed refusal 구현; 정책 설정 시 활성화 | 실제 merge 결과·독립 seal proof·hard limit 회복 경로의 통합 검증, 물리 reserve 결합 |
| bounded staging/pin/drift | 현 worker는 idle staging을 만들지 않으며 publication 안에서 소비/폐기 | 원안의 idle staging을 유지할 경우 byte/age 및 pin 비용 한도; 범위를 바꾸려면 명시적 계약 변경 |
| 지속 churn·long pin·space pressure | 세 compaction cycle과 기존 XL 기능 증거 | 지속 debt·write amplification·RSS·동시 query·임시 disk peak 및 외부 취소 수용 |

고정 `07f0430…` XL에는 phase RSS와 디스크 경계값이 있지만 run/debt/pinned/obsolete/
staging/output reserve가 없다. 이 한 실행으로 threshold나 hard reserve를 산출하지 않는다.
ENOSPC/EIO/short-write를 호출 경계에서 반환하는 회귀도 실제 파일시스템 공간 고갈,
kernel fsync 오류, power-loss 또는 지속 운영 수용과 구분한다.

별도 native RFC의 R17(lexical maintenance), R11(전역 자원 예약), R32(fair pump)는
새 경로의 구현 소유권이다. 기본 경로 전환과 동등한 운영 검증 전에는 legacy LQ5를
자동 대체하지 않는다. Runtime은 자원 예약/펌프를, lexical owner는 merge 전략과
post-merge 계산을 소유해야 하며, 두 경로에 독립 scheduler를 복제하지 않는다.

현재 `NativeResourcePlan/Ledger`의 CPU/I/O/memory/provider 예약은 실행 비용 추정치다.
`ResourceNeed`에는 disk 항목이 없으며 maintenance class도 없다.
`ReadSessionLease`는 객체 사용 수명을 보장하지만 retained/pinned bytes 예약은 아니다.
`FileScratchSpace`는 clone 사이 scratch 주 파일의 논리적 byte 한도를 공유하며,
SQLite secondary spill/WAL·파일시스템 여유·active/retained/pinned/backup을 예약하지 않는다.
새 native 코드가 위 aggregate reserve 요구를 이미 해결했다고 판정하지 않는다.

최소 연결 계약은 다음과 같다. 구현 전에 기존 Core/Runtime 예약 인터페이스와 결합하고,
lexical leaf에 독립 전역 scheduler를 추가하지 않는다.

- Runtime: 하나의 service-owned admission authority가 foreground/query/maintenance와
  보존 객체·staging·spill·backup을 함께 회계한다. 지원 workload로 고정한 정책 입력이
  없으면 production 정책 수용은 `BLOCKED`다. free-space 관측을 원자적 예약으로 대체하지 않는다.
- Publication: `PublicationContext::begin`의 최초 durable attempt 쓰기 전에 RAII guard를
  얻고, worker join과 packet 소비/폐기까지 유지한다. writer mutex 안에서 예약을 기다리지 않는다.
  외부 cancel/deadline과 bounded refusal을 연결하며 오류·취소·unwind에서 반환한다.
- Storage: 신규 encoded object를 쓰기 전에 잠정 charge하고 digest reuse 확인 후 환불한다.
  adoption batch flush 뒤에도 attempt 전체 누적값을 유지한다. object pool 외 Tantivy,
  coverage/root/manifest/journal 및 검증 비용도 공통 authority에 포함한다.
- Lexical: run과 segment를 구분한 계량, pending/obsolete/output bytes 및 merge 비용을
  제공한다. File/Native 준비 결과를 반영한 target을 seal 전에 검사하고, 기존 한도를 줄이는
  publication을 잘못 거절하지 않는다. `segments.entries` 검사만으로 run/bytes admission을
  구현했다고 부르지 않는다.
- 검증: Runtime ledger/guard, lexical merge/target, storage charge의 write set을 분리한 뒤
  정적으로 연결한다. 무거운 실행은 통합 후 한 배치로 묶고 세 번 이상의 지배적 merge,
  long pin·space pressure·동시 query·외부 cancel·aggregate RSS/임시 disk peak를 확인한다.

현재 map의 인증 요약은 map 구조 encoded bytes를, run registry는 소유 segment bundle의
논리 index bytes를 제공한다. File은 run posting bundle과 바깥 map 구조도 포함한다.
이 값들과 debt 행 수만으로 source bodies·Tantivy·control/journal·보존 객체·임시 파일을
포함한 전체 physical bytes를 산출할 수는 없다. seal 전 전수 객체 순회로 누락 값을 채우면
변경 범위 비용 계약을 깨므로 남은 비용의 증분 회계가 필요하다. 임의 cap 또는 scratch
한도 재명명으로 이 코드 공백을 닫지 않는다.

독립 seal 검증은 이제 File의 clean run 병합 입력/출력과 이전된 segment·run-posting
귀속 바이트, legacy Native의 clean 입력과 분리 가능한 출력 segment 귀속을 반환한다.
관측값은 내구 identity 뒤에 게시한다. Native의 mixed fresh/debt 출력은 귀속 불가를
명시하며 0으로 채우지 않는다. 이는 실제 read/write I/O, write amplification 또는 pinned
객체의 물리 회수량이 아니므로 정책 수치 동결에는 추가 비용 계측이 필요하다.

retention planner의 요청별 집합 측정 session에 lexical/semantic production 구현을
연결했다. generation membership과 inode/digest union은 요청 안에서 변경된 집합만
반영하며, 새 generation의 첫 스캔/객체 인증은 남는다. 단발 측정도 같은 session에
위임한다. 세션은 기존 generation의 mutation stripe를 비차단 획득해 scrub receipt
쓰기를 막으며, 부재 generation은 pin하거나 캐시하지 않는다. scratch SQLite의 주
파일 상한은 별도 설정이며 RSS·journal memory·여유 공간 예약이 아니다. 실행 판정은
[VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md)의 최신 절을 따른다.

전체 track 측정은 유지한다. 두 physical session을 먼저 획득해 lexical/semantic lifecycle과
lexical pool mutation을 측정 종료까지 막으며, 획득 충돌과 cached unsealed writer는
거절한다. idle writer release도 lifecycle read를 보유하고 release/discard는 merge worker를
join한다. 새 소스의 실행 판정은 [VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md)의 최신 절을 따른다.
physical census는 두 트랙이 공유하는 service-owned deadline과 cap이 있는 SQLite inode
집합을 사용한다. 기본 deadline은 60초이며 `QUANTA_INDEX_RETENTION_MEASUREMENT_TIMEOUT_MS`로
지정한다. 이는 운영 guardrail이지 측정된 capacity threshold가 아니다. directory-only 순회는
pending 4096 frame/depth 64에서 거절하며 DB·순회 실패의 부분 합계는 반환하지 않는다.
전체 root walk와 busy/retry 가용성, root-wide write ledger 및 boot reconciliation은 남는다.
현재 physical byte 값은 unique
inode의 파일 길이 합이며 실제 allocated blocks 또는 filesystem 여유 공간 예약을 뜻하지 않는다.

이번 ROOT 후속 trace/build/profile 및 precise publication fault의 실행 판정은
[VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md)의 최신 절을 따른다. A/D/C5 및 기존 XL product/lifecycle
수용은 유지하며, LQ5 전체 정책 코드와 운영 검증은 별도 미완료 범위다.

#### 일반 append의 hard-limit 복구 계약

동일 source-event 재시도는 동일 reserved generation과 sealed base를 유지한다.
일반 append가 첫 bounded scan window에서 병합할 쌍을 찾지 못하면 run 증가와 cursor
변경이 함께 거절된다. source no-op의 cursor 예외는 이 append의 복구 증거가 아니다.
첫 창 뒤에 가능한 쌍이 있어도 같은 sealed cursor를 재사용하면 탐색이 진전되지 않는다.

- 같은 durable attempt 안에 base-root digest, policy digest, source-event/operations binding을
  가진 bounded scan checkpoint를 둔다. 정책 또는 base가 바뀐 checkpoint는 재사용하지 않는다.
- source mutation을 적용하기 전에 필요한 scan 진전을 수행한다. 하나의 예약 번호를
  유지하며 별도 maintenance generation이나 합성 no-op source event를 만들지 않는다.
- checkpoint는 serving authority가 아니다. 최종 seal은 checkpoint의 탐색 진전과 선택한
  완전한 cohort를 독립 검증하고, 기존 post-merge hard admission을 적용한다.
- 입력 보존과 예약 guard는 scan, worker join, 최종 publish 또는 명시적 attempt 폐기까지
  유지한다. cancel/deadline/재시작 후 같은 attempt를 재개하며, 불가능한 merge는 bounded
  typed refusal이다. 무제한 scan이나 hard limit 완화로 복구를 대체하지 않는다.
- 수용 oracle은 첫 window의 병합 불가 run, 뒤쪽의 고정 가능한 쌍, 실제 append, 거절 후
  재시작/재시도, 동일 generation, pinned base query, forged/stale checkpoint 거절을 포함한다.

이 checkpoint 구현과 복구 oracle은 아직 `NOT_RUN`이며, 위 source-ready cursor 수리나
기존 no-op 회귀로 완료 처리하지 않는다. Publication 전체의 자원 guard는 lexical
`begin_attempt`뿐 아니라 상위 materializer의 첫 durable source-event 예약 전에 취득해야 한다.

#### Publication admission 연결과 관측 경계

Core의 `PublicationResourceAdmissionPort`는 같은 authority에서 두 단계 permit을 취득한다.
dispatcher는 terminal replay 확인 뒤, 바깥 journal의 최초 `prepare` 전에 control permit을
얻는다. materializer는 인증한 sealed plan과 입력 footprint를 가지고 최초 source-event 예약
전에 work permit을 얻는다. control과 work 비용은 겹치지 않아야 하며, 둘 다 기존
`PublicationValidationOwner`가 journal 종료와 source-event 정산까지 보유한다.
proof/body custody 뒤 work permit, 그 뒤 control permit 순으로 해제한다.
동일 owner의 정확히 같은 요청은 재예약하지 않고, 본문·작업·footprint 변경 또는 durable
예약 이후 첫 work 획득을 거절한다. 첫 control 획득을 이미 work/source 예약한 owner로
늦추는 것도 거절한다. control 거절은 새 journal을 만들지 않는다. work 거절은 이미 수행한
prepare/claim/applying을 취소했다고 주장하지 않으며 원 journal을 Uncertain으로 남긴다.
control 취득 뒤 요청 취소를 이미 durable한 ingest의 중단으로 재해석하지 않는다.

포트에 기본 성공 구현은 없다. `SearchdRuntimeParts`에 실제 authority를 주입하면 Runtime
조립이 동일 Arc를 dispatcher와 materializer에 설치한다. 기본 boot는 아직 None이며,
whole-call 비용 상한·retained/pinned 인벤토리·공통 ledger 구현은 남아 있다. 포트와 scripted
회귀 또는 조립 경계만으로 aggregate memory/I/O/disk reserve 수용을 완료 표시하지 않는다.

출력 하위 예약은 borrowed `PublicationOutputAdmission`으로 전달한다. control/work
parent와 출력 child가 같은 allowance를 공유하며 child가 parent 수명을 보존한다.
leaf 예약에는 원 요청 budget을 전달하지 않는다. durable journal 승인 뒤 capacity
판단은 즉시 반환하는 service 소유 판단이며 peer disconnect로 정산을 중단하지 않는다.
현재 실제 연결 범위는 semantic atomic sidecar의 생성·promotion·명시적 rollback 삭제다.
staged contract의 두 번째 rename까지 같은 child permit을 유지한다. Tantivy/LanceDB
본체의 pre-growth 제한, control/source/scratch, semantic maintenance 및 이전 attempt
복구의 ledger reconciliation은 남는다. 결과 크기를 완료 뒤 검사하거나 pool quota만
이 port에 연결하는 것으로 publication 전체 reserve를 완료하지 않는다.

semantic의 dataset/contract 승격은 generation-local intent의 old/new inode·contract
identity로 결합했다. rollback은 새 dataset 격리와 이전 pair 복원·sync를 staged 출력 삭제보다
먼저 수행하고, forward recovery는 새 pair sync를 backup 삭제보다 먼저 수행한다. intent 없는
ambiguous backup은 삭제하지 않는다. 독립 부분 삭제·rename/sync 재시작 fixture는 작성했으나
실행 수용 전이며, recovery의 output permit 재채택·정산도 전체 ledger 범위로 남는다.
intent 전 staging tree fence는 상속 파일까지 순회·sync하므로 O(delta) 비용을 증명하지 않는다.
파일 수·시간 envelope와 인증된 immutable base의 내구 파일 재사용 계약이 필요하다.

lexical은 인증한 File/Native before/after run·segment·논리 index bytes와 target debt 행 수를
봉인 identity의 내구 게시 뒤 기록한다. scrape는 마지막 봉인 관측과 누적 관측 횟수만 읽으며
객체를 전수 순회하지 않는다. 재시작 뒤 첫 새 봉인 전에는 availability=0과 관측 부재를 표시한다.
File/Native index byte scope는 서로 다르고, 이 값은 active corpus 합계·unique pool bytes가 아니다.
merge 입출력·debt/pinned/obsolete bytes·RSS/물리 disk peak의 계측과 정책 수치 동결은 여전히 남는다.

### 10.35 Publication capacity foundation과 미연결 경계 (2026-10-12)

새 Core `publication_capacity`는 filesystem I/O가 없는 단일 논리 파일 길이/unique inode
원장이다. boot seed가 모두 성공한 뒤 seal하며, parent 예약은 global에 한 번만 charge한다.
child는 같은 allowance를 분할하고 `reserve_growth`는 같은 파일의 쓰기 전에 상한 증가분을
분할한다. commit 실제 길이의 slack만 반환하며 dropped creation-started ticket은 확정
absence/commit까지 보수적으로 유지한다. 완료된 retained parent는 active allowance 슬롯을
차지하지 않는다. hardlink reuse와 zero-length inode도 각각 한 번/한 file slot으로 처리한다.

정적 감사에서 발견한 boot over-cap 후 불완전 seal과 다른 ledger의 숫자 token 충돌을
수리했다. over-cap은 원 capacity error를 보존하면서 seal을 막는다. retained/pending/cursor
token은 checked process-local authority identity와 결속하고 외부가 직접 구성할 수 없다.
token은 wire/persisted 형식이 아니며 restart에서는 실제 inventory로 새 token을 만든다.
고정 oracle의 실행 판정은 `VALIDATION.md`의 해당 소스별 절을 따른다.

retained inode resize는 변화 중 길이를 확인하기 전에 busy를 거절하도록 수리했다.
동시 lookup/re-reserve/reuse가 정상적인 in-flight 성장 때문에 전체 authority를 poison하지
않는다. 정산 직후 stale ID의 mutation 전 예약도 비poison 거절하며 fresh ID로 재시도한다.
현재 ID의 미회계 길이 불일치 및 잘못된 terminal settlement는 poison을 유지한다.
parent 수명별 마지막 삭제와 monotonic high-water의 독립 숫자 회귀를 추가했다.
path-bound output child는 정확히 한 file slot만 받으며 여러 파일은 각각 예약한다.
lease가 제공하는 `StateRootDirectoryV1`은 root FD와 공유 lock owner를 함께 보유한다.
writer는 해당 guard를 파일과 미완료 I/O의 수명 동안 유지해야 한다.

아직 다음 경계는 구현/수용되지 않았다.

- production boot의 `publication_admission: None`과 cost estimator, output binder, pool
  subset/global의 원자적 회계. CPU/I/O/memory authority는 기존 Runtime 소유 계약과 결합하며
  이 file-capacity leaf로 별도 scheduler를 만들지 않는다.
- publication journal 이전의 source upload는 별도 ingress allowance를 공통 원장에 잡는다.
  upload store의 local disk/slot quota와 work permit은 그 전체 수명을 대신하지 않는다.
  create/grow/TTL unlink를 연결하고 upload→publication에서 같은 retained inode charge를
  이전하며 중복 예약하지 않는다.
  upload의 root custody는 FD-relative I/O와 lease-owned parent guard로 수리했고, 생성 전
  parent/component 인증, 8-entry inventory frontier와 원 budget checkpoint, ACK 전 sync를
  추가했다. 이는 local store의 경로·수명·취소 계약 수리이며 공통 원장 연결은 여전히 남는다.
- commit된 동일 inode를 다시 쓰는 mutable DB/WAL/hint의 pre-growth와 여러 publication의
  비용 소유권. Core retained inode `reserve_resize`/`ResizeTicket` 계약은 구현됐지만 실제 writer의 FD/inode와 정산 authority를 연결하는 binder는 없다.
- replaced inode, hardlinks, pin 및 unlinked-live writer/reader FD의 마지막 수명. path 부재 또는
  unlink 성공만으로 환급하지 않는다. same-process pending 채택과 재시작 정산도 binder 책임이다.
  restart의 실제 파일 집계만으로 미완료 durable journal의 잔여 작업 예약을 복원했다고
  간주하지 않는다. 신규 요청을 받기 전에 persisted attempt/intent와 inventory를 대조해
  pending output와 승인된 잔여 allowance를 재채택하는 계약이 필요하다.
- `publication_inventory.rs`는 module에 등록됐고 열린 root와 held lease/lock inode identity를 검사한다.
  부트 전 root census는 lease guard의 인증된 FD로 순회하고 seal까지 custody를 유지한다.
  인증된 root pathname 별칭도 동일한 pinned directory를 집계하고 종료 시 경로/guard identity를
  재검증한다. entry별 bounds와 nofollow inode/alias 검사를 갖지만 production 호출, 실제 adapter
  open 이전 설치, controlled writer 전체 coverage가 필요하다. 새 unique inode를 map에
  추가하기 전에 max_bytes/max_files를 판정하며 map cardinality는 max_files와 max_entries로
  제한한다. 조기 capacity 거절 fixture의 컴파일·실행 수용은 `VALIDATION.md`의 해당 소스별 결과를 따른다.

SQLite WAL·LanceDB writer 연결 조사와 semantic crash 회귀 통합은
`Review Oct 10 index closeout --보완`이 진행 중인 범위다. 해당 조사와 별개의 새 adapter를
여기서 중복 구현하지 않는다. pinned Lance `file` local create/copy는 inner ObjectStore
wrapper를 우회하므로 wrapper API 존재만으로 hard limit을 수용하지 않는다. URI 또는
writer 경계를 바꾸는 경우 실제 append/index/optimize/reopen/delete와 inheritance/promotion
coverage를 함께 수용해야 한다.

이 foundation은 allocated filesystem blocks/free-space reservation, aggregate RSS, I/O
headroom 또는 enabled LQ5 정책의 운영 수용이 아니다. 다음 코드 순서는 retained mutable
회계 계약→boot/binder/estimator→기존 writer hook과 일괄 통합이며, 정책 수치는 지원
workload의 merge 비용을 측정한 뒤 고정한다. idle staging은 이전의 계약 선택으로 유지한다.

### 10.36 통합 활성화와 daemon 수용 차단 경계 (2026-10-12)

`publication_admission: None`을 `Some`으로 바꾸는 단일 수정은 전체 자원 제한의 구현이
아니다. boot scanner와 ledger를 연결할 때 다음 경계를 하나의 통합 후보에서 수용한다.

| 경계 | 필수 계약 | 독립 수용 oracle |
| --- | --- | --- |
| boot | root lease 획득 후 mutable adapter/catalog open 전에 bounded inventory; persisted attempt/intent의 pending output와 잔여 allowance 재채택; 실패 시 ingress 공개 금지 | cap 초과·누락/불일치 intent·취소에서 writer/admission 미공개, 같은 inode hardlink 단일 charge |
| admission/estimator | control·work·ingress가 같은 authority를 공유; 실제 encoder/vendor writer의 검증된 상한 사용; 기존 CPU/I/O/memory authority와 결합 | control 거절 전 journal 없음, work 거절 전 source-event 예약 없음, 동시 요청 합계 cap 유지 |
| ingress/writer | 업로드 생성/증가/TTL 삭제와 일반 output의 pre-create/pre-growth를 연결; pool subset과 전체 ledger의 중복 global charge 금지 | 부분 write/sync 실패 charge 보존, upload 인계 시 inode charge 불변, mutable WAL 성장 전에 reserve |
| retirement/recovery | replaced inode·hardlink·pin·service-owned FD의 마지막 수명 뒤 durable 삭제 정산; process orphan과 restart 재채택 | unlink 뒤 열린 FD/긴 pin에서 charge 유지, 마지막 수명+sync 뒤 한 번만 환급, stale signal 거절 |
| physical retention | transient lock contention과 실제 cap 초과를 typed 결과로 구분; owner가 bounded service budget으로 plan과 census를 다시 계산 | 외부 held reader 동안 새 authority record 없음; 해제 후 바뀐 실제 byte를 새로 측정하고 exact quota 판정; 지속 경합은 bounded refusal |

실제 sidecar/vendor writer binder와 Native/physical GC는 기존 소유 채팅의 변경을 통합한다.
이 표가 별도 scheduler·journal 또는 중복 binder 구현을 요구하지는 않는다. 부트 census
단독 실행이나 모든 파일 쓰기 뒤 fstat만으로 활성화 조건을 충족했다고 판단하지 않는다.

두 실행(`test-daemon`의 shared body scrub, staged hard-scan owner)이 physical snapshot
lifecycle busy로 실패했다. 이 실패를 fixture retry나 blocking lock 변경으로 지우지 않는다.
잠금 획득 순서와 durable publication 상태를 먼저 확인하고 해당 owner가 경계를 수리한
뒤 두 critical gold를 포함한 한 통합 실행 배치로 수용한다. quiet-host Off/Record와 enabled
정책의 sustained churn/pin/space/RSS/temp-disk 수용은 별도 후속 범위다.

최신 owner 소스는 transient contention에 전용 typed code를 사용하며 record 전체의
원 service budget으로 재시도한다. 각 attempt의 guard는 backoff 전에 반환되고 다음
attempt는 active pin/root snapshot/plan을 다시 읽는다. 실제 quota/storage/cancellation은
재시도하지 않는다. 기존 record reconciliation의 raw CBOR·inode 불변 gold를 추가했다.
이 구현의 정적 확인은 완료했고 위 두 daemon 실패의 새 소스 실행 수용은 별도 판정한다.

### 10.37 최종 잔여 작업과 native 분업 (2026-10-12)

현재 소스 기반 전체 실행 목록은 [Native RFC §23.55](../../rfc/oct-10-native-incremental-vnext.md#2355--current-source-final-action-register-2026-10-12)가
소유한다. 이 절은 legacy LQ5와 성능 작업의 경계만 정리하며 §10.34–§10.36의 정확성
계약을 완화하지 않는다. 문서 대조에서 새 runtime/벤치를 실행하지 않았다.

후속 [Native RFC §23.56](../../rfc/oct-10-native-incremental-vnext.md#2356--service-owned-retention-code-checkpoint-2026-10-12)은
자동 history/work/custody retirement의 정책·Catalog producer·서비스 연결을 구현한다.
이 native F02 코드는 legacy LQ5 admission 수리나 지속 디스크 한도의 수용을 대체하지
않는다. 신규 runtime 체크포인트는 `NOT_RUN`이다.

| 작업 | 구현·수용 구분 | 다음 최소 작업 |
| --- | --- | --- |
| 일반 append hard-limit staged 복구 | 수정된 G36 replacement, 원 body/예약/checkpoint, G+1 authority 거절, G35 replay/활성화·검색 회귀 PASS(103.834초). 고정 후보 전체 daemon은 519 PASS/2 FAIL; 두 GC fixture 수리 뒤 영향 GC/scrub 12 PASS | 이 staged 기능 수용은 완료. 실패했던 전체 배치는 그대로 FAILED이며 후속 GC 수용은 별도 source/scope. 다른 채팅의 최신 변경·전체 운영 수용으로 승계하지 않음 |
| publication/physical capacity | Legacy canonical reader·GC를 기존 Common binder에 연결하고 단일 pending ticket 재시도·동일 capability 검사 구현. POSIX fsync 보존과 동일 epoch 재시도 수리 후 영향 115/115·strict VERIFIED. 최종 workspace 컴파일·선언 daemon 319/319(10 skipped) VERIFIED. qualified 결합 fault 수용은 별도. production admission은 None이며 전체 mutable writer/부팅 연결은 미완료 | 첫 mutable open 전 leased inventory·명시적 control/work 상한, Lexical epoch/attempt/Tantivy, lifecycle root incarnation/Catalog, upload/RepoMap/history writer 연결. 후속 Control 원자 쓰기·journal resize/재시도·lifecycle identity/staging·Lance/Semantic 파일 삭제 owner 구현, typed I/O 원인 보존을 포함한 영향229/229·5-crate strict·workspace 컴파일 VERIFIED. Native H 조립 중 컴파일 실패는 후속 workspace 검사에서 해소. attempt retirement/unowned를 같은 삭제 owner로 추가 연결했고 최신 영향235/235·5-crate strict·workspace 컴파일 VERIFIED. 새 literal 선언 daemon319/319(10 skipped,1332.733s)·검토 후 module/hexagonal/no-allow VERIFIED. vendor/ciborium workspace lint 상속 FAILED는 남음. Control create-pending/FD/임시파일 GC·directory 오류 소유권과 실제 fault·지속 수용은 남음. None→Some만으로 수용하지 않음 |
| File prepared merge와 bulk staging | 후속 source-buffer/map-frame 수리, producer/merge 7개 및 lexical 집중 회귀·strict의 실제 결과가 존재 | 변경된 source가 있거나 새 비용 가설이 있을 때만 해당 회귀 재실행. 과거 106개 union의 producer 0-match를 PASS coverage로 계산하지 않음 |
| 새 NativeText churn | 변경 owner/term COW 갱신과 32회 논리 churn oracle 존재; 물리 비용/지속 수용 미완료 | 같은 delta에서 N/history/fanout을 바꾸고 실제 object writes/bytes/fsync·보존 공간·GC 및 필요한 drain·latency를 측정. File run debt를 NativeText에 이식하지 않음 |
| Large/XL와 formal performance | source별 legacy 기능 PASS는 보존; 이후 memory/interface 변경의 현재 통합·native 성능 수용 없음 | active memory/staged 수용 뒤 하나의 최신 후보로 기능 검증, 이후 admission을 지킨 matched 성능 측정. timeout/cap을 높인 결과를 개선으로 판정하지 않음 |

기존 A/D/C5를 재설계하거나 공유 memory ledger·StoredObject·profile을 별도 작성하지
않는다. `인덱싱 레이어링 어딧`은 해당 native 자원 파일, `Review Oct 10 index closeout`은
staged/daemon 경계, `증분 (2)`는 후속 인덱싱 비용 감사를 진행하고 있다. source별로 정확한
write set을 확인한 뒤 잔여 파일을 배정한다. 배포 staging 안전성은
`Review Oct 10 index closeout --보완`의 별도 active 범위다.

LQ5의 전체 자원·운영 의무는 qualified native parity와 명시적 legacy 퇴역 전까지 남는다.
이는 독립 native foundation·도메인 구현을 막는 신규 순차 gate가 아니다. 공유 facade,
port와 최종 통합만 직렬화하고, heavy build/index/test는 기존 resource gate를 사용한다.
