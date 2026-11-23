# NC-TEXT · Native Text 구축 최종 작업 패킷

2026-11-21 KST 갱신. 사용자가 승인한 최종 설계의 구현·인수 계획과 검증 결과다.

이 패킷은 [전체 퇴역 계획](../TANTIVY-RETIREMENT.md#구조-감사와-최종-실행-계획--2026-11-15)의 Native Text 구현 순서를 구체화한다. 기존 History·Caller·Source·설치 복구·Hybrid·T/A/B/E06의 담당과 종료 조건은 변경하지 않는다. [작성·실행 경계](../README.md#레인별-착수와-인계)와 [원 수용 기준](../DOD.md)을 유지한다. 공용 TANTIVY-RETIREMENT/DOD/원장은 F가 소유하고 이 채팅은 이 전용 패킷을 갱신한다.

Main에는 multipage writer, descriptor receipt, byte-full dictionary, borrowed cold stream, prefix 조회 재사용·정확한 구간 병합과 Literal cold owner-prefix 봉인이 연결됐다. 후속 LCP·wide128 credit·Storage fixture 수리를 포함한 R44 동결본의 빌드·Clippy·전체958개 본문과 별도 leaky 관측은 아래 표에서 구분한다. 원본 corpus의 prefix-cache6,129,674B·Literal 봉인18,060,924B 감소는 각각 당시 코호트의 component 결과이며, LCP local 회귀의 비용 감소와 구분한다. 원512MiB WorkBytes·R/W/M 각100만·원 입력·단일 FULL을 유지한다. 원본 SDK·현재 Large·quiet Off 수용은 별도 범위이며 [최종 Main 재검증과 mutation RCA](#final-main-revalidation-and-mutation-rca)가 증거와 잔여를 소유한다.

## 다음 Native Text 포맷과 전환 계획

설계 감사 기준은 Main `df090e56a`의 작업트리다. 기존 경로는 [owner별 Contribution·inventory](../../../../crates/quanta-index-lexical/src/native/text.rs#L9091), [ordered phase0 packet과 최종 병합](../../../../crates/quanta-index-lexical/src/native/text_spool/packed_runs.rs#L782), [페이지 기반 term dictionary](../../../../crates/quanta-index-lexical/src/native/text/term_dictionary.rs)와 COW 갱신을 사용한다. 기존 경로의 R51 etcd에서 ordered inventory read 19개 창은 158,295,650B를 사용했고, 한도 거절은 최종 병합 전에 발생했다. 이 값에는 필수 검증과 논리 edit 청구도 들어 있다. 세그먼트 후보의 원본 SDK 검증은 아래 R55/R56 동결본에 결속한다. NTX0–NTX4는 기존 포맷의 구현·비용 기준선과 검증 기록이다.

**목표 구조는 문서별로 분할한 불변 inverted-index 세그먼트다.** Lucene은 불변 세그먼트, 세대별 live docs, 원자적 문서 교체를 사용한다. Tantivy는 메모리 한도로 버퍼를 flush하고 로컬 docID와 버전별 alive bitset을 사용한다. Lucene의 posting codec은 block-packed 정수·skip·positions를 분리하고, Zoekt는 shard 포맷 버전과 새 shard 재생성을 사용한다. Elastic의 merge 정책은 읽기 fan-out, 삭제 회수, 병합 부하의 운영 경계를 보여 준다. 이 패턴을 Quanta의 기존 Source·Catalog 게시 계약에 적용한다. [Lucene segments](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/package-summary.html#segments-and-docids) · [Lucene postings](https://lucene.apache.org/core/10_5_0/core/org/apache/lucene/codecs/lucene104/Lucene104PostingsFormat.html) · [Tantivy architecture](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md) · [Zoekt format](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md) · [Elastic merge settings](https://www.elastic.co/docs/reference/elasticsearch/index-settings/merge)

### 코드 작업 배정과 착수 순서

첫 목표는 빈 Text root의 원본 Source FULL을 새 세그먼트로 게시하고, 재오픈 뒤 정확한 term·phrase 조회를 끝내는 것이다. TEXT가 공용 접합을 한 곳에서 맡고, writer·reader·계측을 분리한다. 기존 F/D가 제품·설치 수용을 소유한다.

| 우선순위·패킷 | 단일 작성 범위 | 완료 조건과 인계 |
| --- | --- | --- |
| P0 · 포맷 계약 | TEXT 통합 작성자. 새 `native/text_segment`의 wire·manifest 계약, 모듈 등록, 고정 wire·논리 fixture만 소유한다. | magic·layout 정책, 정렬 키, 페이지/객체 최대 크기, 문서 표, 하위 객체 참조, Source 결속을 고정한다. 불변 세그먼트는 recipe·문서 신원에, FULL root는 SourceCut·정렬된 owner 범위에 결속한다. FULL의 문서 표 전체가 활성 집합이다. P1에서 활성 문서 위치와 live-doc 세대를 도입한다. 객체 신원 및 child 참조는 기존 Storage 규칙을 사용한다. wire 재현과 이상 버전·길이·순서 거절을 고정 fixture로 제공한다. |
| P0 · FULL writer | 새 `native/text_segment/build.rs`와 전용 fixture. [Source cursor](../../../../crates/quanta-index-lexical/src/native/text.rs#L13063), [기존 분석](../../../../crates/quanta-index-lexical/src/native/text.rs#L10378), [child-first install 포트](../../../../crates/quanta-index-core/src/index/ports.rs#L194)를 사용한다. | source owner를 한 번 분석해 문서 표와 최종 posting을 만든다. 용어는 세그먼트 사전에서 한 번 쓰고 로컬 docID·위치 delta를 block으로 쓴다. 빈 문서·큰 단일 문서·여러 페이지에서 메모리 승인과 순서·개수를 확인한다. spill이 필요한 입력은 실제 임시 I/O를 청구하고, 실패·취소 뒤 새 root가 게시되지 않아야 한다. |
| P0 · reader 및 정확성 | 새 `native/text_segment/read.rs`와 독립 query fixture. [기존 reader 진입점](../../../../crates/quanta-index-lexical/src/native/text_reader.rs#L624), [검색 실행기](../../../../crates/quanta-index-lexical/src/native/text_query.rs#L1051), [직접 term 조회](../../../../crates/quanta-index-lexical/src/native/text.rs#L13299)를 소비한다. | 고정 wire를 독립 해석하고 live owner·exact DF/TF·phrase 위치·EOF를 재현한다. 여러 세그먼트에서도 안정 OwnerKey 순서와 동점 순서, scope와 pinned Source 검증이 일치해야 한다. 잘못된 recipe/root/child·truncation·상한·취소에서 명시 오류를 내고, 같은 의미의 ranker·필터는 기존 실행기로 공유한다. |
| P0 · 비용 기준선 | 기존 TEXT 진단 담당. 현재 trace/원본 harness를 사용해 계측하고, 필요한 코드 변경은 기존 diagnostic 경계만 소유한다. | 같은 원본에서 단계별 exclusive R/W/B/M과 종료 누적치, 실제 고유 객체·edge·CAS hit를 남긴다. R51 이후 최신 제품 증거와 진단 결과는 source·binary·입력에 묶는다. 계측과 fixture/codec 작업은 병렬이며, 제품 비용 비교는 수용 전에 끝낸다. |
| P0 · FULL 접합 | TEXT 통합 작성자만 [`NativeTextIndex` build/root 결선](../../../../crates/quanta-index-lexical/src/native/text.rs#L12973)과 reader/overlay/policy dispatch 변경을 합친다. | writer·reader를 기존 SourceLocal `IndexBuildPort` 및 pinned query 경로에 접합한다. 격리한 새 포맷 후보에서 빈 prior root의 Bootstrap FULL·검색·재오픈·source digest, 원본 etcd의 시작부터 ACK까지 Work와 실패 축을 확인한다. 출하용 기본 선택은 P1 및 P2 수용을 기다린다. |
| P1 · Patch·overlay·compaction | Patch는 새 root/live-doc 갱신과 fixture, compaction은 세그먼트 재작성과 fixture를 소유한다. 공용 root·reader 접합은 TEXT 통합 작성자가 합친다. | replace/delete/no-op/rename, 새·구 view pin, 요청 overlay, regex 및 repo/path scope를 독립 전체 재구축과 대조한다. compact 전후 논리 신원·랭킹과 회수 경계를 검증한다. owner가 이동하거나 문서가 사라져도 동일 세대 내 중복·누락은 0이다. |
| P2 · 제품·포맷 전환 | 기존 F 통합 큐가 동결 빌드와 제품 수용을 소유하고, 기존 D가 선택·복구·롤백을 소유한다. | 원본 Caddy/etcd FULL과 최종 R/W/B/M, 양쪽 quiet Off 두 회, Large 두 재시작, 이전 pin·ACK 유실·restore-forward를 검증한다. 수용된 포맷만 선택하고 옛 root와 바이너리는 기존 복구 경계에서 보존한다. |

포맷 계약과 고정 fixture가 준비되면 writer와 reader가 병렬로 작성한다. 각 작업자는 자기 모듈과 전용 fixture를 편집한다. 이미 dirty 상태인 `native/text.rs`, `native/text_query.rs`, `native/text_overlay.rs`, `native/text_reader.rs`의 접합은 통합 작성자가 hunk별로 인수한다. 현행 `text_spool`·occurrence·dense/dictionary 구현은 R51 비교와 기존 담당의 범위다. 새 포맷은 [`legacy-tantivy` 전용 run/statistics 모듈](../../../../crates/quanta-index-lexical/src/lib.rs#L87)에 의존하지 않는다. Core·Storage·Catalog 공용 변경은 구현에서 실제 포트 결손이 확인되면 해당 기존 오너와 인계한다.

각 패킷은 호출 진입점, 소유 파일, 소스 revision, 실제 실행 selector·exit·본문 수와 다음 패킷이 소비할 타입 또는 artifact를 인계한다. 새 모듈이 등록되면 집중 회귀는 기존 Cargo front door에서 실행한다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --release --locked --offline \
  --no-default-features -p quanta-index-lexical --lib --no-tests fail \
  -E 'test(native::text::text_segment::) | test(segmented_full_)'
```

P0 후보는 [고정 wire](../../../../crates/quanta-index-lexical/src/native/text_segment/wire.rs)의 manifest, 로컬 문서 표와 페이지를 사용한다. 원본 SDK runner에서 `--text-layout segmented-full-v1`로 새 recipe와 format을 선택한다. 이 후보는 빈 prior의 FULL과 핀된 term·phrase를 지원한다. 기존 WorkBudget으로 새 토큰과 실제 I/O를 청구하고, 같은 NFC·경계·위치가 증명된 토큰 프로그램은 기존 캐시에서 빌린다. 검색은 세그먼트의 term 범위로 페이지를 찾고, BM25 점수를 매기기 전에 정확한 DF를 집계한다. P1/P2 판정은 별도다.

2026-11-20 Main `df090e56a72f` 작업트리에서 P0 writer·reader·원본 SDK 선택 경로를 연결했다. case 행 65,536개 또는 key backing 16MiB의 메모리 창을 넘으면 이미 생성한 행을 승인된 scratch로 이관한다. 문서 하나의 로컬 docID를 유지하며 메모리와 scratch가 동일한 posting 페이지 writer를 사용한다. 게시 전에는 모든 세그먼트·posting 페이지의 순서, 범위, 문서별 case 토큰 수를 검사한다. Large의 기존 4,096파일·두 OS 재시작 경로에는 명시적인 segmented layout 선택을 추가했다.

원본 Caddy 590개·etcd 1,421개의 파일 SHA, manifest, 고유 검색 증인을 재검증했다. 초기 집중 Nextest와 R52 동결 빌드는 각각 공용 잠금 대기 900초·1,800초 후 exit 124였다. 후속 seal 회귀는 컴파일의 `unused_results` 진단을 수정했다. 통합 Nextest에서 `spill.rs`를 포함한 Lexical 라이브러리 rlib가 생성됐다. 초기 집중 실행은 DataFusion 의존성 컴파일 정체 뒤 종료해 본문 0건이었다. 원본 SDK의 R/W/B/M 회계는 동일 요청의 `lexical_build → native_text_root`와 `native_publish`를 결속하고 닫힌 `operation/2` 누적치와 원 한도 대비 여유를 읽는다.

R52 첫 소스 복사는 Catalog `history_pages.rs` 변경을 검출해 컴파일 전에 거절했다. 양쪽 카탈로그와 복사된 이전 파일을 보존한 뒤 3,042개 Main 입력을 다시 결속했다. release Cargo는 공용 Harness의 `let_underscore_drop` 2건과 macOS `F_SETFD`의 `unused_results` 1건으로 **FAILED**, exit 101이었다. 원본 SDK 본문은 0건이었다. F의 두 수정 파일을 해시로 대조했고 R53은 실패 기록을 보존하며 같은 release Cargo 캐시를 사용한다.

R53 release는 **VERIFIED**, 8분 16초·exit 0, 데몬 `5b1adbe4…`·SDK 도구 `3969a1a3…`다. 원본 두 Record는 **FAILED**, 각 exit 1이다. 초기 빈 Text root의 `BuildPrepare`에서 `InvalidInput`으로 거절됐고 Lexical root 구간의 Work는 두 코퍼스 모두 0이었다. 설치된 MemoryScope에서 자식 벡터의 0바이트 lease를 요청한 것이 원인이다. 빈 벡터는 checkpoint 뒤 lease 없이 만들도록 수정했다. 별도로 R53 SDK 시점의 Main 전체 입력 21개가 동결본과 달랐다. R54는 빈 root 수정과 설치된 MemoryScope·취소 회귀를 포함해 최신 소스를 다시 동결한다.

R54 첫 요청은 공용 잠금 대기 3,600초 뒤 exit 124로 **BLOCKED**였다. 소스 캡처, Cargo, SDK 본문은 모두 시작 전이었다. 잠금 보유 실행이 `lancedb`까지 진행한 것을 확인한 뒤 한 건만 다시 접수했다.

재접수한 R54의 3,047개 빌드 입력은 복사 전후 일치했다. release는 새 ANN 디렉터리의 `unused_results` 두 건으로 **FAILED**, exit 101이었다. 두 검증 호출의 성공값을 바인딩하고 오류 전파를 유지했다. R55에는 빈 root 수정과 ANN 컴파일 수리가 함께 포함됐다.

R55 release **VERIFIED**는 5분 43초·exit 0, 데몬 `ee9abf74…`·SDK 도구 `4dfff86e…`다. 원본 Caddy 590파일·etcd 1,421파일은 같은 동결본에서 FULL·검색·OS 프로세스 재오픈과 Record trace 검증을 각각 통과했다. 초기 빈 root 게시와 FULL을 구분하도록 `native_source_apply`가 성공한 동일 요청의 최종 Work를 추출했다.

| 원본 SDK Record | FULL 게시 | 최종 R / W / WorkBytes / M | WorkBytes / mutation 여유 |
| --- | ---: | ---: | ---: |
| Caddy R55 | 2,686.428ms | 47,106 / 16,315 / 164,713,427 / 394,216 | 372,157,485B / 605,784 |
| etcd R55 | 8,036.642ms | 118,000 / 41,523 / 471,630,063 / 946,860 | 65,240,849B / 53,140 |

R55의 전체 원본 SDK Record는 동결본 범위에서 **VERIFIED**다. live Main은 시작 시 Harness 4개 경로, 최종에는 ANN 테스트 1개까지 차이가 났다. 변경된 컴파일 입력 3개는 `native_corpus`의 의존 경로이며 데몬 의존 경로에는 변경이 없었다. live Main 동일성은 **FAILED**이며 R56에서 다시 결속한다. Large와 quiet Off는 별도 실행 범위다.

R56 release **VERIFIED**는 5분 48초·exit 0, 데몬 `d214a1d5…`·SDK 도구 `6fe5bbbb…`다. 첫 복사 중 `history_checkpoint.rs` 변경을 검출해 이전 카탈로그와 파일을 보존했고, 재동결은 3,049개 빌드 입력이 일치했다. normalizer의 38개 입력·툴체인·기존 rlib 해시가 같아 검증된 R55 산출물을 재사용했다. 원본 두 FULL·검색·OS 재오픈과 Record 검증은 모두 **VERIFIED**이고, 게시 소요는 Caddy 2,599.479ms, etcd 7,863.588ms였다. 종료 R/W/B/M 네 축은 R55 표와 정확히 일치했다. 이후 live Main의 History·Harness 4개 경로가 달라 전체 동일성은 **FAILED**다. Large는 원본 SDK를 통과한 동일 R56 동결본과 데몬에서 검증한다.

같은 원본 Caddy의 `LexicalBuild/2`는 R51 385,475,916B·mutation 625,976에서 R56 109,738,195B·380,357로 줄었다. `NativeSourceApply/3`은 두 코호트 모두 5,637,475B·6,308이었다. R56 etcd `LexicalBuild/2`는 304,661,018B·911,907이고, 그 안의 posting seal 83개는 합계 95,100,661B·4,816이다. 부모와 자식의 포함 Work는 중복 합산하지 않는다.

R56에서 직접 자식의 Work 네 축이 모두 존재하는 구간만 빼서 `LexicalBuild/2`의 직접 Work를 계산했다. Caddy는 R1,283/W0/B78,555,683/M378,459, etcd는 R3,103/W0/B209,293,932/M906,733이었다. `ManagedObjectInstall` 자식에는 Work가 없어서 posting seal의 직접 Work는 계산할 수 없다. 현재 `managed_object_inputs`와 `map_root_reuses`는 각각 호출 입력 수와 Map root 재사용을 보여 준다. 이 trace에는 고유 객체 수·외부 참조 edge 수·CAS hit 수를 판정할 증거가 없다.

R56 별도 spill SDK 회귀 **VERIFIED**다. 단일 163,869B 문서의 32,769개 토큰·case 행 65,538개가 메모리 창을 넘었다. 원 한도에서 FULL·검색·OS 재오픈과 두 Record trace를 통과했다. LexicalBuild는 W65,927·M33,138, posting seal은 R66,699였고 전체 종료는 R67,954/W66,256/B24,804,350/M33,426이다. 산출물은 `/private/tmp/qidx-r56-spill-q62zqpwv/result/`에 있다. 이 입력은 원본 코퍼스와 독립인 writer 경계 회귀다.

R56 Large 첫 입장은 1,800초 뒤 exit 124·Cargo0·본문0이었다. 두 번째 시도는 입장한 뒤 DataFusion release 중간 산출물 기록에서 로컬 디스크 `ENOSPC`로 **FAILED**, Cargo exit 101·본문0이었다. 기존 출력과 SDK를 통과한 R56 release 쌍을 보존했다. 다음 시도는 같은 R56 소스·데몬, Large selector·seed·두 재시작 계약을 유지하고, 테스트 드라이버의 Cargo target과 임시 데이터만 외장 SSD의 별도 경로로 잡았다. quiet Off 사전검사는 외부 Rust 7개와 1분 부하 44.29/상한 8로 **BLOCKED**였다.

외장 SSD에서는 로컬에서 실패한 DataFusion 산출물이 생성됐다. 이어 `native_product_process`가 Harness의 `source_fixture.rs`를 경로로 직접 포함하면서 `crate::scale_failure`와 `legacy-bench`를 Runtime 테스트의 모듈·feature로 해석해 컴파일이 거절됐다. 기존 Harness 개발 의존성의 공개 `source_fixture`로 연결을 바꿨다. 후속 드라이버는 원 R56 입력에서 이 테스트 진입점 한 파일만 바꾼 별도 소스에 결속한다. R56 release 데몬·SDK 및 기존 Large seed·입력 한도를 유지한다.

**R56 Large VERIFIED.** 외장 SSD에서 수정된 release 드라이버를 새로 컴파일했다(2분 13초, exit 0). 원 R56 데몬 `d214a1d5…`와 일치하는 SHA를 실행 전후 검증했다. `large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks`가 기존 4,096파일의 Source 본문·신원, repo별·전역 랭킹을 두 OS 재시작에서 확인했다. Nextest `47627a37-6f08-4d6e-9dba-e8c65e297f25` 본문 134.355초·1/1·exit 0이다. 근거는 `/Volumes/Extreme SSD 1/build-cache/r56-driver.5k_evgeu/large-summary.json`이다. 같은 시점의 quiet Off 사전검사는 외부 Rust 8개, 1분 부하 106.77/상한 8로 **BLOCKED**이며 원본 Caddy·etcd 각각 0/2회다.

Large 이후 live Main을 다시 캡처했다. HEAD `df090e56a72f`는 R56과 같고 빌드 입력 74개가 달랐다. 차이에는 `Cargo.lock`, 두 Cargo manifest, History·Harness·Semantic과 공용 진단 소스가 포함된다. NC-TEXT segmented writer·reader·wire와 프로덕션 SDK 접합의 직접 파일은 R56 해시와 일치했다. R56 제품·Large 결과의 수용 범위는 위 동결본과 명시된 드라이버 수정이다.

같은 드라이버 소스와 외장 target에 집중 Lexical Nextest를 접수했다. 공용 build-test 잠금 대기 1,800초 뒤 exit 124로 **BLOCKED**였고 Cargo와 테스트 본문은 0건이다. 원본 SDK 및 별도 spill SDK, Large 본문 결과는 위와 같다. 종료 시점에 quiet Off 입장을 다시 확인했으나 외부 Rust 19개와 1분 부하 54.14/상한 8로 **BLOCKED**였다.

### P1 구현 상태

- `TextOwnerCursor`가 Tree와 Segment의 admitted owner를 공통의 문서·토큰 수와 기존 4,096문서 한도로 검증한다. Segment는 정렬된 문서 표를 읽고 핀된 Source의 owner·record·입장 정책을 검증한다. unanchored·regex·contains가 같은 유한 집합을 사용한다.
- Overlay는 현재 포맷의 기존 기여 문서 수와 길이를 반영한다. Segment prior 조회는 인증된 디렉터리와 마지막 문서 표를 공유한다. DF는 선택된 term과 mask 대상 owner가 속한 세그먼트의 인증된 posting에서 계산하며 한 세그먼트의 멤버십 비트맵을 재사용한다. Tree는 기존 inventory와 inverse posting 일치 검증을 유지한다.
- V1 회귀는 단일 세그먼트에서 unanchored·regex, stale Source 거절, overlay 교체와 exact DF를 확인하도록 작성했다. 제품 재실행과 여러 세그먼트·continuation의 overlay 회귀는 별도 검증 범위다.
- live Main `df090e56a72f`의 P0/P1·Tree 집중 Nextest는 입장 후 Lexical release rlib를 생성했고, Harness의 `openai_ab_runner_config_json` 미사용 진단으로 Cargo exit 101·본문 0이었다. 호출자는 `legacy-bench` 경로와 Harness 단위 테스트에 있다. 입력 전후의 HEAD·Cargo·대상 TEXT 파일 해시는 일치했다. 독립 집중 실행은 같은 selector를 사용하며, 동결 복사본에만 Harness helper와 해당 import의 `cfg(any(test, feature = "legacy-bench"))` 경계를 적용한다. 복사 중 History 쿼리 소스 변경을 한 번 감지했고 다음 복사본은 입력 해시를 모두 대조했다. 최근 quiet Off 입장은 외부 Rust 7개·1분 부하 49.68/상한 8로 **BLOCKED**였다. live Main 실행 출력은 `/Volumes/Extreme SSD 1/build-cache/nc-text-p1.Gw02kQ/`에 있다.
- 공용 SDK 20건은 `load_document_root`의 참조 생존 기간과 빈 슬롯 패턴 이동 충돌 E0505에서 컴파일 exit 101·본문 0이었다. `read.rs`에서 빈 슬롯을 먼저 채운 뒤 공유 참조와 SourceCut을 확인하도록 고쳤다. 다른 SourceCut의 재사용 거절과 재조회 Work 불변을 한 고정 회귀에 넣었다. 공용 SDK 재실행은 기존 오너가 맡는다. E0505 수정 전 집중 복사본은 Cargo 입장 전에 자체 종료했고, 수정 후 같은 selector로 한 건만 다시 접수했다.
- E0505 수정 후 독립 동결본 `nc-text-p1-focus.9rP9NG`에서 새 Lexical release rlib를 생성했다. 그 실행은 Harness `process.rs`도 사용하는 `model_revision_of` import까지 cfg로 제한해 E0432·exit 101·본문 0이었다. Main 오너는 helper만 cfg로 제한했고 import를 유지했다. live Main 재실행 `nc-text-p1-main.z6xUk7`은 이후 History `query.rs`의 `ControlFlow` 반환 타입 두 건과 visitor 수명 두 건으로 exit 101·본문 0이었다. 독립 재실행 `nc-text-p1-replay.8E0cga`는 Lexical을 컴파일한 동결본에 Main 오너의 Harness 파일 한 개만 해시 검증 후 반영하고 같은 selector로 실행한다.
- 이전 대기 프로세스가 입장·종료 기록 없이 사라진 뒤 같은 동결본에 재접수했다. `nc-text-p1-admitted.SnbWtZ`에서 production Lexical·Harness 컴파일을 지나 테스트 바이너리를 만들다가 `repo_has_file`의 공용 macro 선언 순서와 기존 pinned-root fixture의 raw 필드 읽기에서 컴파일 exit 101·본문 0이었다. 공용 test module을 매크로 뒤에 등록하고 fixture가 `SelectedTextRoot::statistics()`를 사용하도록 수정했다. unanchored 회귀의 부재 키는 case tag를 포함한 page fence 밖으로 옮겼다. 기존 대기는 Cargo 입장 전 종료하고 `nc-text-p1-fixtures.IW2ndP` 하나에 production 해시를 유지한 테스트 수정과 두 fixture selector를 결속했다. quiet Off 재검사는 외부 Rust 0개·부하 266.47/상한 8에서도 거절됐다.
- 최종 집중 후보 `nc-text-p1-fixtures.IW2ndP`는 공용 잠금 대기 1,800초 뒤 exit 124·Cargo 0·본문 0으로 **BLOCKED**다. 동결 파일 해시는 대기 전후 일치했다. 종료 시 preflight는 외부 Rust 19개와 1분 부하 146.01/상한 8로 **BLOCKED**였고 원본 quiet Off는 Caddy·etcd 각각 0/2회다. R56 원본 Record·Large의 동결 결과를 유지하며, 수정 후 최신 Main에서 원본 SDK·Large·quiet Off를 통과한 release 쌍은 없다.

**Delta 물리 계약 방향.** 다음 포맷 root는 겹치지 않는 OwnerKey 상한, 활성 문서 수·길이, 파티션당 참조 한 개를 정렬해 저장한다. 마지막 상한은 최대 OwnerKey다. FULL은 파티션의 불변 세그먼트를 전체 활성으로 직접 참조한다. 변경으로 여러 세그먼트나 삭제 비트가 생긴 파티션만 별도 불변 manifest에 base·delta 참조와 세대별 활성 비트맵을 쓴다. Patch가 쓰는 root directory에는 경계·요약·참조만 들어가며 나머지 manifest와 posting은 재사용한다. FULL마다 owner 위치 map을 따로 쓰는 비용은 피한다. Storage 16MiB 객체와 4,096개 child 상한, SourceCut edge를 포함한 root 용량을 검증한다. Patch는 `SourceInputCursor`로 prior/next를 검증한 뒤 해당 범위의 제한된 문서 표에서 이전 위치를 찾는다. 교체·삭제에서는 이전 비트를 지우고 새로 입장한 문서만 분석해 delta를 쓴다. 전체 root의 정확한 활성 문서 수와 길이 합계를 새 SourceCut에 결속한다. reader는 범위 안에서 활성 posting만 병합하고 DF·BM25 계산 전에 제외를 적용한다. 기존 포맷과 혼용되지 않도록 root magic과 recipe 정책을 새로 부여한다. [Lucene IndexWriter](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/IndexWriter.html) · [Tantivy segments and deletes](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)

Compaction은 한 범위의 제한된 입력에 대해 살아 있는 문서·posting을 병합한다. 선택 범위·세그먼트당 활성 문서 상한·기존 WorkBudget이 작업 범위를 제한한다. SourceAdvance의 병합은 기존 `build_members`와 Catalog CAS에서 새 Text root와 함께 게시한다. 독립 `CompactSourceLocal`도 같은 journal·Core 작업·Catalog CAS를 사용한다. 정확한 기대 view와 member artifact, owner 범위를 선택하고 동일 SourceCut·profile·논리 commitment를 검증한 뒤 대상 member의 물리 artifact를 교체한다. 이전 pin 수명과 회수는 현재 root 도달성 규칙을 사용한다. [Lucene TieredMergePolicy](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/TieredMergePolicy.html)

**Layered V2 작업트리.** [Wire](../../../../crates/quanta-index-lexical/src/native/text_segment/wire_v2.rs)는 새 root·partition 정책, OwnerKey 경계, live mask·합계, SourceCut과 path posting 참조 순서를 고정한다. [Writer](../../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs)는 FULL에서 불변 세그먼트를 직접 등록하고, Patch에서 변경 owner가 속한 파티션만 다시 쓴다. 삭제는 live mask로 처리하고 변화한 admitted 문서만 분석한다. 기존 path posting의 merge를 연결하고, 새 posting 세그먼트는 게시 전에 독립 디코더로 검사한다.

[Reader](../../../../crates/quanta-index-lexical/src/native/text_segment/read/read_v2.rs)는 root를 한 번 인증하고 고정 크기 파티션 행을 직접 읽는다. term DF의 층별 활성 비트셋을 재사용하고, hit가 없는 파티션은 두 번째 읽기에서 제외한다. hit 탐색은 층마다 한 페이지를 유지하며 로컬 docID 순으로 진행한다. 빈 root와 문서 전용 읽기는 빈 버퍼의 lease를 요청하지 않는다.

[패치 내 compaction](../../../../crates/quanta-index-lexical/src/native/text_segment/compact_v2.rs)은 16층부터 합산 활성 문서가 128개 이하인 인접 층의 posting을 새 세그먼트에 재생한다. 정확히 16층에서 병합할 쌍이 없고 root 자리가 있으면 live owner의 중앙 경계 양쪽으로 mask·문서 수·토큰 수를 나눈다. 이 분할은 인증된 문서 표와 동일한 물리 세그먼트 참조를 사용한다. reader는 물리 descriptor와 각 파티션의 활성 문서 경계를 검증한다. 16층을 넘는 대량 Patch는 기존 등록 한도를 적용한다. 이후 독립 compaction에서 선택한 파티션의 한 쌍을 병합하거나, 병합할 쌍이 없는 16층 이상 파티션을 분할하거나, 삭제 비트가 남은 한 층을 재포장할 수 있다.

런타임과 원본 SDK runner에서 `segmented-layered-v2`를 명시적으로 선택한다. [프로세스 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs)에 빈 뷰, FULL, 교체·이름 변경·삭제·no-op, 이전 Exact view, term·phrase·순수 부정·`select:path`, OS 재오픈을 추가했다. 1,041개 owner의 16층 입력에서 물리 root 행 1→2, 양쪽에 등록된 동일한 세그먼트 ref와 공통 term의 전체 match 수를 확인하도록 했다. 동일 Source의 독립 repack은 artifact 교체, SourceCut·profile 유지, 점수와 순위 일치, 이전 view·재오픈, stale artifact 거절을 확인하도록 했다. 기존 Large에서도 V2 선택을 추가했다.

신규 모듈 형식·diff 검사와 Cargo 대상 확인은 **VERIFIED**다. 공식 public API 도구로 Contract·Core·Kernel·Lexical을 렌더링했고 Contract `UpdateKind`와 Core `IndexBuildPort`의 해당 기준선이 렌더링 결과와 일치한다. 현재 source의 Semantica Source/AST 수용 경로는 새 compaction 의도를 명시적으로 거절한다. 이 API 검증은 SDK·프로세스 본문을 포함하지 않는다. 이전 통합 Nextest `nc-text-v2-proof.6r9yWt`는 공용 잠금 대기 1,800초 후 exit 124·Cargo 0·본문 0으로 **BLOCKED**였다. 최종 release·V2 원본 Caddy·etcd SDK·Large는 **NOT_RUN**이다. 재부팅 전 quiet Off 사전검사는 외부 Rust 14개·1분 부하 39.10/상한 8로 **BLOCKED**였으며 양쪽 본문은 각각 0/2회다. 원본 512MiB, R/W/M 각 100만, 요금 및 SDK60을 사용한다.

호스트 재부팅으로 `nc-text-v2-repack.b2lFo2` 대기가 종료됐다. 재접수한 `nc-text-v2-postboot.MHHgvS`는 187.906초 뒤 입장했고 Lexical의 미사용 변수·반환값과 `unreachable_pub` 진단 6건을 확인했다. 해당 변경을 수정한 `fix1`은 같은 Nextest selector를 사용한다. 첫 미러의 History fixture 1건 해시 차이를 보존했고, release는 최신 입력을 다시 동결하도록 준비했다. V2 원본 Record·quiet Off 검증 도구는 기존 원 manifest·고유 검색 증인·512MiB/100만 혼합 한도와 같은 pair 해시를 사용한다. 원본 입력의 SHA·개수만 재검증됐으며 제품 본문은 아직 실행되지 않았다.

`fix1`은 Lexical release 라이브러리를 생성하고 Semantic `row_attestation.rs`에서 exit 101·본문 0으로 종료됐다. 제거된 `interruptible`을 참조하는 한 건과 그로 인한 타입 추론 진단이 원인이다. attestation scan을 기존 ANN `settle_query_batches`로 연결해 원 요청과 버퍼 예약을 vendor stream의 종료까지 유지한다. `fix2`는 해당 Semantic release 라이브러리를 생성했다. 이후 live Main의 Embed가 컴파일된 Core에 없는 `with_http_attempt`를 호출해 exit 101·본문 0이었다. 동결본에서는 Core·Embed 양쪽 API 일치를 확인했다.

후속 감사에서 V2 FULL이 Source 참조 3개와 path root를 등록할 때 4,092개까지만 담을 수 있지만 4,093번째 segment를 seal할 수 있음을 확인했다. FULL은 이미 seal한 segment, 남은 window, 예상 path edge를 합산해 다음 segment를 만들기 전에 제한을 적용한다. path posting이 없는 입력은 기존 4,093개 경계를 유지한다. wire fixture는 path root 유무에 따른 4,092/4,093개 수용을 확인한다. 형식·diff 검사를 통과했고 실행 회귀는 대기 중이다.

이 수리를 포함한 `release-cohort.ma7fmqs6`의 Main 입력 3,084개는 복사 전후 해시가 일치한다. 이전 `release-cohort.vp9jc1a5`는 공용 잠금 대기 중 입장 전에 종료했다(exit 143, 본문 0). `ma7fmqs6`도 공용 슬롯에서 1,800초 뒤 exit 124·Cargo 0·본문 0으로 **BLOCKED**였다. 후속 `release-cohort.xgp178sy`는 1,547.891초 뒤 입장했으나 외부 캡처기가 `shared/quanta-installed-artifact-trust`를 복사하지 않아 Cargo metadata exit 101, Nextest exit 102·본문 0으로 **FAILED**였다. 외부 캡처기를 `shared/`까지 포함하도록 수정했다. `release-cohort.uh6ymfgt`의 복사 해시 일치 3,132개와 동일 명령의 Cargo metadata 597개 패키지 해석을 **VERIFIED**로 확인하고 통합 Nextest 한 건을 접수했다. 통합 Nextest가 성공하면 동일 소스로 release pair를 만든 뒤 원본 Caddy·etcd Record, Large, quiet Off를 검증한다. 03:09 KST quiet Off 사전검사는 외부 Rust 작업 8개와 1분 부하 81.61/상한 8로 **BLOCKED**였다. Caddy·etcd Off 본문은 각각 0/2회다. 반복기의 입장 전 timeout은 본문 미실행을 확인한 경우에만 재시도를 허용하고, Off의 종료 부하를 본문과 같은 lease에서 검사한다.

`uh6ymfgt`는 378.982초 뒤 실제 컴파일에 입장했다. SDK DSL의 미사용 `GenerationPin`과 History builder의 `load_history_batch_object` import 누락으로 Cargo exit 101·본문 0이었다. 두 import만 수정했다. 외부 실행기 수정 중 생긴 shell terminal 기록 누락은 원 Cargo 진단과 lock release 로그로 확인했다. `release-cohort.igwdj_li` 3,134개 입력은 456.032초 뒤 입장해 SDK·History 진단을 넘어 Lexical에 도달했다. `inventory_reference` import, overlay의 private import 2개, 미사용 `Serialize`로 exit 101·본문 0이었다. 이 4건은 live Main에서 이미 수리돼 `release-cohort.1ituzeg8`의 3,136개 입력과 Cargo metadata를 검증해 통합 실행 한 건을 접수했다.

`1ituzeg8`은 819.094초 뒤 입장해 이전 Lexical 진단을 넘어 Semantic·Kernel·Runtime을 컴파일했다. 런타임 테스트의 dev-dependency `searchctl`에서 DSL 읽기 길이와 digest 반환값을 버려 exit 101·본문 0이었다. 두 반환값 처리는 현재 Main에서 이미 수정됐다. 통합 Nextest와 제품 release의 판정을 분리하고, revision을 다시 캡처해 `quanta-index-searchd`·`native_corpus` release 빌드를 한 건 접수했다. 원본 SDK, Large 및 통합 회귀의 성공 기준을 각각 적용한다.

`release-cohort.m3mwubjc`는 Main 입력 3,139개를 동결해 제품 release를 **VERIFIED**로 끝냈다. Cargo exit 0·6분 1초, daemon `32a38d3e…`, SDK 도구 `b8393d65…`, 빌드 전후 동결 소스 일치다. 같은 pair의 원본 Caddy 590개 V2 SDK 본문이 exit 0, FULL 게시 4.505초, 첫 검색과 OS 재오픈 검색을 통과했다. 후속 검증기가 동결본의 Python 실행 보조 파일을 import하며 bytecode 파일 1개를 생성해 검증 단계가 중단됐다. 3,139개 입력의 변경·누락은 0개였다. 해당 생성 파일을 외부 기록으로 보존하고 엄격 사전검사를 다시 통과했다. Caddy의 두 trace와 최종 Work는 보존된 본문에서 검증하고 원본 etcd는 첫 본문을 이어서 실행한다.

같은 release의 원본 etcd 1,421개 SDK는 **FAILED**였다. `BuildPrepare`에서 R233,685/W94,435/B512,092,888/M999,991이고 Catalog 확인 16 mutation을 추가하다 거절됐다. 본문 posting 83회 뒤 추가한 경로 맵 `map_apply` 하위 span의 mutation은 편집 13,484회와 설치 33,200회였다. BuildPrepare는 ACK에 도달하지 않았다. Caddy SDK 본문은 성공했지만 보존 Record trace는 3,039개 이벤트를 누락하고 span 2개가 열려 있어, trace 검증과 종결 Work 인수는 **FAILED**다. 출력량 한도에 도달하지 않았으며 4,096-event 비차단 큐의 손실 구간이 확인됐다. Large 테스트 대상은 fixture의 `json!` 배열 반복식 오류로 exit 101·본문 0이었다. fixture에 기존 Text descriptor를 직렬화하도록 수정했다.

**V2 경로 참조를 SourceCut에 결속.** 경로 맵의 `(term, owner)`마다 SourceRecord를 Catalog child로 다시 등록하던 값을 자식 없는 32바이트 inline digest로 변경했다. SourceCut owner map이 해당 객체를 보존한다. 검색 후보는 ticket에 고정한 owner map의 정확한 digest, 레코드 등록 정보, 경로 및 admission을 검증한 뒤 평가한다. key·DF·phrase·랭킹과 COW 갱신 계약은 동일하고, V2 recipe의 path policy가 인코딩 변경을 식별한다. 기존 Tree는 object 값과 기존 policy를 사용한다. 새 release에서 원본 Work를 다시 측정한다.

inline 경로 release는 동결 입력 3,139개·Cargo exit 0·6분 42초로 **VERIFIED**였다. 원본 Caddy 590개 **VERIFIED**는 FULL 3.394초, 첫 검색·재오픈, 두 trace 및 R84,053/W36,276/B179,385,753/M413,746이다. etcd 1,421개는 경로 맵 완료 뒤 Text root 확인에서 **FAILED**였다. R222,948/W96,577/B508,393,258/M999,969에서 182 mutation을 추가하려다 거절됐다. 완료한 경로 맵의 편집과 설치는 각각 13,484회, 32,998회였다. 따라서 경로 간선만 제거해도 별도 값 행이 지배하는 맵 비용은 남는다.

다음 V2 경로 policy는 Sensitive term의 정규화 바이트가 Folded와 같으면 같은 posting을 공유한다. 두 바이트열이 실제로 다른 term의 Folded key만 별도로 기록한다. Folded 조회는 정렬된 두 term 범위를 owner 순으로 병합하고 한 owner가 두 범위에 나오면 동일한 SourceRecord인지 검증한 뒤 한 번만 센다. DF와 후보가 같은 cursor를 쓰며, 정확한 SourceCut 검증과 phrase 평가는 유지한다. ASCII 가정 없이 기존 정규화 결과를 대조한다. 같은 경로에 `oldroute`와 `OLDROUTE`를 포함한 회귀가 case별 결과, 패치 및 재오픈을 확인한다.

공유 case posting을 적용한 `release-cohort.7nq8pv_u`는 Main `df090e56a72f` 입력 3,140개에서 Cargo exit 0·6분 26초로 **VERIFIED**였다. daemon `43072380…`와 SDK 도구 `81599ada…`는 같은 동결 소스에 결속했다. live Main의 차이 1개는 `read_v2.rs`의 `#[cfg(test)]` 통계 assertion이다. release 와이어와 생산 reader는 일치한다.

| 원본 Record | FULL 게시 | R / W / B / M | 남은 B / M |
| --- | ---: | --- | --- |
| Caddy 590 **VERIFIED** | 3.455초 | 77,712 / 31,179 / 174,628,131 / 409,507 | 362,242,781 / 590,493 |
| etcd 1,421 **VERIFIED** | 7.763초 | 211,405 / 85,489 / 497,183,698 / 990,909 | 39,687,214 / 9,091 |

양쪽 runner·ingest·record validator가 각각 exit 0이며, 같은 요청의 FULL ACK·첫 검색·OS 재오픈 검색과 원 manifest/source SHA를 검증했다. etcd 첫/reopen trace는 각각 89,221/12,147 events, 누락 0이다. 두 후보에서 모두 완료한 etcd 경로 맵의 편집과 설치를 대조했다. 공유 case 적용 후 편집 13,484→6,895회와 설치 mutation 32,998→30,292회로 줄었고, 해당 두 span의 합계는 mutation 9,295회와 WorkBytes 10,452,057B 감소했다. 512MiB·R/W/M 각 100만·SDK60·기존 요금을 적용했다. 같은 daemon의 원본 Large 4,096개·두 OS 재시작·순위 검증은 **VERIFIED**, Nextest `8fe1c13c-d3da-4000-8cc2-7bffde29391f`, 1/1 PASS·69.845초·73 skipped·exit 0이다. quiet Off는 05:28 KST 외부 Rust 7개·1분 부하 30.51/상한 8에서 입장이 **BLOCKED**였다. Large 종료 후에도 Rust 20개·부하 26.92로 거절됐다. 양쪽 본문은 각각 0/2회다.

집중 Nextest의 `release-cohort.sjb9vswk`는 생산 동결본과 최신 `read_v2.rs` 테스트 assertion 한 경로만 다르다. 통합 selector의 `--lib` 실행은 당시 Runtime 배포 선택 library test의 `Box<dyn Error>` 형식 불일치로 exit 101·본문 0이었다. Text UDS를 `native_product_process`에서 직접 실행해 dense split 79.581초 및 독립 compaction 11.605초를 통과했다. 세 번째는 대문자 `Renamed.rs`의 파일 목록 기대 순서 오류로 실패했다. 순서를 수정한 동결본에서 실패한 한 건만 다시 실행해 대소문자·FULL/patch·pin·재오픈을 6.695초, 1/1 PASS·exit 0으로 확인했다(`02ea5dab-03bf-4e36-a315-8593f7e0052b`).

Kernel·Lexical library는 31건 본문 중 30건을 통과했다. V1 정규식 oracle의 공통 fixture WorkBytes 한도가 1,000,000B였고 엔진 준비와 생성의 고정 비용만 15,782,912B였다. 저예산의 `ResourceLimited` 확인을 유지하고, 정규식 성공·Source 정합성 검증에만 32MiB fixture를 사용하도록 수정했다. 해당 테스트만 다시 실행해 1/1 PASS·exit 0을 확인했다(`24869882-025c-4a0d-9776-81d32c8f042b`). 집중 회귀는 library 31건과 Text UDS 3건 모두 **VERIFIED**다. 06:26 KST quiet Off 재접수는 외부 Rust 2개·1분 부하 24.84/상한 8로 exit 75 **BLOCKED**였고, 후속 사전검사에서도 Rust 5개·부하 27.39였다. 양쪽 Off 본문은 각각 0/2회다.

### Current NC-TEXT acceptance

**경로 전용 Source와 확정 비용 계측 후속.** [기존 G1 실패](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)로 본문 제외 Source가 경로 postings, 문서 수, 조회·프리뷰 인증에서 누락되는 것을 확인했다. Tree와 Layered는 경로 통계를 모든 Source owner의 수와 경로 토큰 수로 결속한다. Layered `QTXROOT3`는 본문 파티션이 0이어도 경로 root를 보존한다. 기존 Tree·Layered 후보의 recipe와 root가 바뀌므로 새 선택은 원 Source에서 FULL을 게시해야 한다. 경로 전용 조회에서 본문 제외를 유지한다.

관리 객체의 신규 Catalog 등록과 간선은 성공한 커밋에서 기록한다. Storage는 기존 파일 바이트를 검증한 뒤 Catalog가 확인한 ObjectRef를 기록한다. 원 FULL을 소유한 daemon 요청의 confirm·advance만 모아 각각 고유 identity를 센다. 사전 검사 통과, 물리 설치 입력, reserve 및 불완전한 Record는 확정 수치를 제공하지 않는다. 합성 타임라인, native-ingest, 진단·정규화 Python 검사 747건을 통과했다.

06:41 KST 기준 이전 공유 case 후보의 quiet Off는 **BLOCKED**였다. 호스트 관찰 종료 시 별도 Rust 2개, 1분 부하 33.64/상한 8이었고 Caddy와 etcd 각 0/2회였다. 경로·계측 첫 동결에서 미사용 접근자가 Cargo exit 101로 거절됐다. 해당 접근자를 제거한 Main 입력 3,140개 동결본 `release-cohort.gfb65g89`는 8분 17초·exit 0으로 **VERIFIED**됐다. release 직후 Main 빌드 입력 차이는 0개다. 같은 쌍의 [원본 Record 저널](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/release/record-status.json>)은 FULL·검색·OS 재오픈·trace 검증을 두 건 모두 확인했다.

| 원본 | FULL 게시 | 최종 WorkBytes / mutation | 잔여 WorkBytes / mutation | 신규 객체 / 등록 간선 / CAS 재사용 |
| --- | ---: | ---: | ---: | ---: |
| Caddy 590 | 3.182초 | 174,628,235 / 409,507 | 362,242,677 / 590,493 | 780 / 1,442 / 0 |
| etcd 1,421 | 7.944초 | 497,183,870 / 990,909 | 39,687,042 / 9,091 | 2,343 / 3,955 / 0 |

Record 뒤 quiet Off 사전검사는 별도 Rust 8개와 1분 부하 28.77/상한 8로 **BLOCKED**였다. G1·Layered·Large는 `QTXROOT3`의 고정 경계를 읽도록 독립 검사 fixture 한 파일만 수정한 동결본에서 **VERIFIED**였다. Nextest `2c7e28d4-151f-4aaf-abd2-16bffee28d77`는 6/6·exit 0으로 Tree G1, Layered path-only·FULL/Patch·dense split·compaction, Large를 실행했다. 같은 release daemon의 원본 Large 4,096개·두 OS 재시작과 순위 검증은 118.283초였다. [본문 결과](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/path-regressions.a0dxuj4o/runtime-nextest.log>)

이후 live Main의 생산 소스 차이는 Root가 공용 `TextStatistics` 선언을 별도 leaf로 옮긴 4경로다. 필드·derive·공개 경로가 같고 Root의 module-cycle 검사도 통과했다. 위 제품 증거는 선언 이동 전 동결본에 결속된다. [공용 접합 기록](../../nov-20-native-cutover-closeout/tickets/01-q-regressions.md)

wire·CAS 집중 회귀 11건은 **VERIFIED**다. 첫 실행의 fixture 반환값 lint를 교정한 뒤 10건이 통과했고, 고정 direct root golden에는 경로 문서 수 1의 기대 바이트를 반영해 해당 한 건만 다시 통과했다. CAS 회귀는 scalar·batch 재사용, 확인 실패 미계측 및 atomic advance를 실행했다. [10건 본문](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/path-regressions.a0dxuj4o/census-wire-retry.log>) · [golden 1건](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/path-regressions.a0dxuj4o/golden-retry.log>)

07:36 KST 새 후보 quiet Off 첫 입장은 별도 Rust 6개와 1분 부하 25.42/상한 8에서 exit 75 **BLOCKED**였다. 이후 세 관찰 구간에서도 입장 조건을 충족하지 못했다. 마지막 구간에서 별도 Rust가 0개로 줄었지만 08:01:53 KST의 1분 부하는 49.37/상한 8이었다. Caddy·etcd 각각 본문 0/2회다. [동결본 quiet Off 저널](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/release/off-status.json>)

원 manifest의 경로는 전부 ASCII다. 모든 파일이 Text에 입장한다고 놓고 정규화된 경로의 indexable run과 고유 `(owner, case, term)` 키를 세면 Caddy의 직접 path 분석·cold map edit 상한은 2,660+5,274회, etcd는 6,858+13,484회다. 이는 path별 두 청구 지점의 정적 상한이며 SQLite·Storage·Catalog 설치 Work와 V2 최종 여유는 원본 Record에서 측정한다.

### 쓰기·읽기 계약

- **FULL.** 핀된 SourceCut의 owner를 확정된 순서로 읽고 기존 NFC·case·position 의미로 분석한다. 승인받은 메모리 범위에서 term별로 모아 로컬 docID·빈도·위치 delta가 들어간 최종 posting block과 term dictionary를 직접 봉인한다. 메모리 경계에서 각각 검색 가능한 세그먼트를 flush하며, FULL ACK 전에 세그먼트 전체를 하나로 병합하는 전역 단계는 폐기한다. 세그먼트 레시피, 객체 최대 크기, 실제 하위 객체 참조, 독립 디코더의 범위·순서·개수·digest 검증까지 결속한다. 문서 세그먼트가 정방향 owner 인벤토리와 별도의 posting scratch가 표현하던 indexed membership을 소유한다.
- **문서 신원과 증거.** 세그먼트 문서 표가 로컬 docID를 안정적인 OwnerKey, SourceRecord와 indexed length에 결속한다. 게시 전 `(case, term, local docID, position)` 정렬 스트림의 봉인을 독립 디코더 결과와 대조하고, 입력 의미는 핀된 원본에 대한 독립 oracle로 검증한다. 조회는 핀된 root의 인증된 block과 문서 표, 같은 view의 Source membership을 사용한다. 기존 [owner 검증](../../../../crates/quanta-index-lexical/src/native/text.rs#L9091)과 [posting witness 검증](../../../../crates/quanta-index-lexical/src/native/text.rs#L9151)이 맡는 잘못된 owner·stale source·손상 위치 거절을 새 계약에서 재현한다. 기존 owner 유니버스, 파일 단위 term 조회와 overlay 소비자를 함께 이관한다.
- **수정과 삭제.** 현재 활성 OwnerKey에서 이전 문서 위치를 찾는다. 교체는 새 문서를 delta 세그먼트에 추가하고 기존 docID를 새 세대의 live-doc 상태에서 제외한다. 삭제는 해당 문서의 제외를 게시한다. 활성 문서 위치 매핑에 대해 live-doc 상태를 검증한다. owner마다 이전 term 전체를 열거해 term별 삭제를 기록하던 작업은 이 경로에서 사라진다. Source·Text·형제 view 구성원이 기존 Catalog CAS에서 함께 전진하고, 재시도와 ACK 유실은 같은 operation의 결과를 돌려준다.
- **검색과 compaction.** 새 live-doc 상태를 적용하면서 기존 scope·overlay 및 scoring 범위별 exact DF, 문서 수·길이, phrase 위치, BM25와 안정적인 동점 순서를 유지한다. 첫 독립 oracle은 live postings를 정확히 센다. 최적화는 generation에 결속된 exact DF와 입증된 건너뛰기 상한을 사용한다. 세그먼트 fan-out, 삭제 비율과 병합 대기를 별도로 관측한다. 기존 Catalog·Storage 권한에서 승인받은 compaction이 동일한 논리 문서 집합을 새 세그먼트로 만들고 root를 CAS로 교체한다. reader pin과 복구에 필요한 세대의 객체는 도달성과 lease에 따라 회수한다. [논리 신원과 compaction](../../../adr/NOV-07-001-native-index-execution-and-custody.md#consolidated-implementation-boundaries)

### 작업 순서와 종료 조건

제품·설치 수용은 동결한 단일 후보의 결과를 사용한다. 초기 구현에 필요한 메모리 승인은 각 flush의 살아 있는 버퍼를, 512MiB WorkBytes는 FULL 요청의 누적 작업을 제어한다.

| 순서 | 구현·판정 |
| --- | --- |
| 1. 기준선과 원가 | 같은 etcd 원본으로 현재 경로의 완결 비용을 별도 진단에서 확보한다. source/admission·토큰 생성·owner inventory·phase0/병합·객체 설치·게시의 겹치지 않는 Work R/W/B/M, 실제 객체 종류·고유 수·edge·CAS hit, 물리 I/O와 메모리 피크를 기록한다. 원본 SDK의 수용 증거는 고정 제품 프로필을 사용한다. |
| 2. 새 포맷의 FULL 한 경로 | TEXT가 writer·독립 reader·정확한 조회·canonical submit 결선을 함께 구현한다. 기존 normalizer, Source owner/cut, WorkBudget, prepared install, Catalog 게시를 사용한다. 빈 문서·긴 term·상한 경계·다중 세그먼트·두 case·phrase·파일 조회·손상·취소를 검증한다. 같은 코퍼스 A/B에서 실제로 제거한 owner inventory, phase0 및 최종 재작성, 고유 객체 예약·확인만 절감으로 계산한다. 기존의 논리 청구 단위, 남은 sort/spill/검증·설치 청구를 적용한다. 신규 token run의 mutation도 전부 청구한다. |
| 3. Delta와 운영 | add/replace/delete/no-op, 오래된 view와 overlay, rename/동일 본문, 복구·취소·ENOSPC·ACK-loss를 독립 전체 재구축 결과와 대조한다. compact 전후 논리 신원과 순위가 일치하고, 새 FULL 직후 및 삭제가 누적된 상태에서 세그먼트 fan-out·고빈도 term과 함께 원 mixed/검색/자원 기준을 충족해야 한다. FULL 필수 flush·검증·설치 비용은 게시한 요청에 모두 포함한다. |
| 4. 제품 수용 | 최신 Main 동결 후보의 원본 Caddy/etcd 단일 FULL·검색·재오픈·source digest, Large 두 재시작, Caddy/etcd quiet Off 각 2회를 실행한다. 512MiB WorkBytes·R/W/M 각 100만·SDK 60초·물리 파일 30,000과 기존 요금을 적용한다. etcd의 최종 사용량과 네 축의 여유를 남긴다. [원 수용 기준](../DOD.md#기존-고정-성능-및-자원-기준) |
| 5. 영속 포맷 전환 | 새 magic·layout/recipe 정책·새 물리 root로 원 SourceCut에서 재색인한다. 옛 root와 해당 바이너리는 한 쌍으로 보존한다. 제품 수용과 설치 복구가 검증된 후보에 한해 Source high-water, 새 view, operation, signed selection을 결속한다. 하나의 활성 writer와 Catalog 선택 권한으로 기존 원자적 선택 절차를 실행한다. 구 pin은 기존 바이너리에서 소진하거나 정합성이 증명된 방식으로 종료한다. 전환 이후 첫 영속 커밋부터 ACK 유실을 포함해 기존 restore-forward 경계를 적용한다. TEXT는 포맷·SDK60, F는 통합 수용, D는 Native→Native 설치 복구·선택·롤백을 담당한다. [배포 선택과 복구 ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#candidate-preparation-and-selection-durability) |

**채택 판정.** 새 세그먼트가 512MiB와 100만 mutation을 만족하는지는 원본 검증으로 판정한다. 객체 수 감소는 실제 Catalog 작업으로 검증한다. 기존 prefix 공유의 이론적 추가 상한은 14,278회다.

## 1. 고정한 결정과 현재 상태

구현 계약은 [Text ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody)과
[연속 스캔·SQL replay 결정](../../../adr/NOV-07-001-native-index-execution-and-custody.md#sequential-cold-alias-scan-and-sql-replay)에 통합했다.
아래는 현재 작업의 검증 범위다. 과거 고정 바이너리의 영수증은
[원 SDK closeout](#fresh-source-bound-original-sdk-closeout)에 보존한다.

| 범위 | 현재 결과와 잔여 |
| --- | --- |
| 고정 후보 R44 빌드 | VERIFIED — Main HEAD `ca1dc5c6`의 dirty 동결 입력3,035개, compiler0/2m44s·changed0/extra0·artifact600. daemon`cf003784`/helper`5eb96ce5`. Normalizer는 R40에서 실제 컴파일한 rlib`750f3b08`과 입력38개를 검증한 `VERIFIED_R40_REUSE`다. 현재 Main 전체의 clean/exact-commit 증거로 확대하지 않는다. |
| R44 Lexical library Clippy | VERIFIED — exit0·진단0, 원 설정 SHA`351788ae` 결속. Workspace·test target lint를 포함하지 않는다. |
| R44 Normalizer+Lexical+Storage library | 본문 VERIFIED:958/958·2 skips. 최초 Nextest는 leaky1을 보고해 엄격 runner FAILED(run`489033c2`). 출력 관측만 보강한 동일 코호트 재실행도958/958·실제 LEAK 상태 없음(run`19a4c5ec`). 원 leaky1은 원인 미확정·미재현으로 보존하며 수정됐다고 주장하지 않는다. |
| Wide128 credit 회귀 | VERIFIED — 같은 단일 회귀에서 구 생산 코드 RED(exit100, Memory 거절) → 생산2파일 GREEN(exit0, 1/1PASS), run`5a1d37b4`→`3d5b20c1`. 실제128행 flush·40B 긴 키·전체 term/owner/ref·두64행 페이지·EOF·반납을 검증했다. R44 전체 library에도 포함한다. MAX1024B 상한의 수식 검토와40B 동적 사례를 구분한다. |
| Storage 손상 객체 회귀 | VERIFIED — 독립 negative request의 fixture 예산만 분리한 뒤 집중1/1PASS(run`7c22f8bd`), R44 전체958/958에도 포함한다. 집중 wrapper가 filtered959건을 잘못 분류한 실패는 보존했다. 생산 budget·요금·SDK 단일 FULL은 그대로다. |
| 원 Caddy Record | R44 VERIFIED — 원590파일·단일 FULL·검색·reopen, 게시42,176.392ms(단계 시작→검색45,910.111ms), runner/ingest/verify 모두0. 원 manifest·source SHA와 실제 daemon 일치, compiled source1,268/전체3,035개·Main HEAD가 실행 전후 동일했다. Record를 quiet Off로 확대하지 않는다. |
| 원 etcd Record | R44 FAILED — B536,866,381+18,046/536,870,912, R346,471/W589,843/M786,036, `packed_runs.rs:655`의 logical posting admission. RAM·timeout·R/W/M cap 거절이 아니며 ACK0·query/reopen 미도달이다. ingest0·COMPLETE_DIAGNOSTIC, Main·source·pair 전후 동일. 원 trace의 InventoryRead32→PostingEdit33 실패는 최종 postings/root 진입 전이다. |
| 원 Large | R44 VERIFIED — 원4,096파일·두 OS재시작, Nextest`7f0f9b0f`, 본문1/1·exit0·133.605s·60 filtered. 실제 test depfile 입력18개와 매 시작의 R44 daemon을 결속했다. Source3,035/compiled1,268/pair/config 전후 동일. Leaky 수치는 NOT_REPORTED다. 별도 Large fixture 예산과 contended 기능 증거를 SDK512MiB/quiet 성능 수용으로 확대하지 않는다. |
| V7 Text component 진단 | FAILED — 원 etcd1,421파일·실제 Runtime factory, submit B730,022,033/R466,431/W806,228/M999,996, 다음 M8 거절. Nextest`030107b0`, 본문1/FAIL1·exit100. 진단 Work10GB/1800s이며 원 SDK512MiB/60s 증거가 아니다. Trace Complete188,731/188,731·누락0. 아래 RCA와 attribution 한계를 따른다. |
| 후속 trace 부모 연결 | VERIFIED — lane38 안의3개 Small/top writer 호출이 active scoped context를 전달하며 명시적 None을 유지한다. 기존67-term+warm CAS 회귀에서 구 생산 코드 RED 1FAIL/exit100 → 수리본 GREEN 1PASS/exit0(run`b66de171`). 두 단계 모두 Lexical 재컴파일·전체3,035입력 전후 일치, 동일 test SHA`e080cb81`다. 검증 후 R44 두 파일을 복원했다. R44 제품/전체 검증에 소급 포함하지 않는다. |
| 후속 cold prepare/merge | VERIFIED — R44 기반 변경7파일의 집중 `test(token_prefix) \| test(cold_back_reference_multi_run_wire_work_and_eof)` 20/20PASS·665 filtered·exit0, run`e98538d8`. 첫 입장은 공용 빌드 잠금에서300초 후 exit124·본문0이었다. 같은 후보의 재입장과 release 본문 결과를 확인했고 R44 동결 경로3,035개 전부 원본 해시를 복원했다. 원본 SDK와 Large는 최신 Main 동결 후보에서 별도로 검증한다. |
| R45 release | VERIFIED — 동결 Main `3a5dddb` 입력3,037개, compiler0/13m57s·artifact600, daemon`9b6dcb77`/helper`bc314747`. 실행 당시 별도 Main 변경11개 중 Catalog retention이 실제 컴파일 의존 경로에 포함돼 live Main 일치 판정은 FAILED다. 아래 SDK 결과는 R45 동결본에 귀속한다. |
| R45 원본 SDK Record | Caddy590·etcd1,421 모두 FAILED — 60초 `REQUEST_DEADLINE_EXCEEDED`, 각 runner1·diagnostic ingest0·FULL ACK0. daemon이 처음 거절한 작업은 `BuildPrepare` 중 cancellation이었다. R44의 byte 거절과 V7의 변이 거절은 각 실행의 원인으로 유지한다. |
| R45 Large / quiet Off | Large BLOCKED — R45 동결 입력과 daemon/helper 사전 검사를 통과했고, 공유 빌드 잠금에서 1,800초 대기한 뒤 `cargow`가 exit124로 종료됐다. 컴파일과 본문은 0건이다. quiet Off preflight는 외부 Rust29개·1분 load85.98/상한8에서 BLOCKED다. Caddy·etcd 각각의 Off2회는 NOT_RUN이다. |
| R46 Main 동결 빌드 | VERIFIED — 캡처 HEAD `587b4237`, 입력3,038개·컴파일 소스1,268개, 컴파일 13m34s·exit0, daemon `cc981650`/helper `bc314747`. 원본 SDK 실행 후 live Main에는 입력 변경32개와 컴파일 소스 변경11개가 확인됐다. |
| R46 원본 SDK Record | Caddy VERIFIED — 590파일, FULL ACK·검색·재시작, 게시52,405.329ms, runner/ingest/verify 모두0. etcd FAILED — 1,421파일·FULL ACK0, B536,855,862+16,670/536,870,912, M799,225, R328,049/W599,632, `prepared_leaf_run.rs:209`. ingest0·COMPLETE_DIAGNOSTIC. |
| R46 Large | VERIFIED — F의 원본 재시도 `d9b4545f-2520-48ba-952e-6ba8d54fb64b`, 4,096파일·두 OS재시작·1/1PASS·64 skip·exit0. 본문164.306s, 요약164.308s. 드라이버 `c684d9d2`, daemon `cc981650`, 동결 입력3,038개와 컴파일 소스1,268개의 실행 전후 결박이 일치했다. 첫 시도의 잠금 대기 만료와 본문0은 이력이다. |
| R46 집중 회귀 | BLOCKED — 동일한 회귀5건의 입장 시도2회가 각각 1,800초 후 `cargow` exit124, Nextest 본문 0건. 변경 경로 `rustfmt --check` 및 `git diff --check` VERIFIED. |
| R46 quiet Off | Caddy·etcd 각2회 입장 BLOCKED — 네 번 모두 exit75, Rust4개·1분 load55.94/상한8. 각 실행 본문은 NOT_RUN이다. |
| R47 Main 동결 빌드 | VERIFIED — 캡처 HEAD `07911b81a9b7`, 입력3,043개·컴파일 소스1,268개, 9m50s·exit0, daemon `2489c744`/helper `2e0e695b`. 원본 실행의 live Main 차이는 컴파일 의존 경로 밖 테스트 파일4개였다. |
| R47 원본 SDK Record | Caddy VERIFIED — 590파일·FULL·검색·재시작, 게시27,637.918ms, runner/ingest/verify 모두0. etcd FAILED — 1,421파일, B536,870,904+16/536,870,912, M791,993, R327,967/W599,452, `cold_tokens/program.rs:379`; 최종 posting 병합과 FULL ACK는 미도달. |
| R47 Large | VERIFIED — 원본4,096파일·두 OS재시작, `d99ab74f-1a63-4b74-b47e-7f36f8477456`, 1/1PASS·68 skip·exit0, 본문101.879s. 드라이버 `d01e1d77`, daemon `2489c744`; 동결 소스3,043개·직접 test source18개와 실행 전후 결박 일치. |
| R47 quiet Off | Caddy·etcd 각2회 입장 BLOCKED — 네 번 exit75, 외부 Rust7개·1분 load129.46/상한8, 본문0. |
| R48 Main 동결 빌드 | VERIFIED — `07911b81a9b7`, 입력3,045개·컴파일 소스1,268개, 2m12s·exit0, daemon `f034724c`/helper `2e0e695b`. Record 전후 live Main 입력 차이는 0개였다. |
| R48 원본 SDK Record | Caddy VERIFIED — 590파일·FULL·검색·재시작, 게시20,565.443ms, runner/ingest/verify 모두0. etcd FAILED — 1,421파일, B536,859,015+12,324/536,870,912, R327,718/W599,968/M795,161, `scratch-sqlite/src/edit_write.rs:918`; 최종 posting 병합·FULL ACK 미도달. |
| R48 Large | VERIFIED — 원본4,096파일·두 OS재시작, `201488cd-8429-44d6-ad22-fa00fb4b8545`, 1/1PASS·68 skip·exit0, 본문105.383s. 드라이버 `3fecc84a`, daemon `f034724c`; 동결 소스3,045개·직접 test source18개와 실행 전후 결박 일치. |
| R48 quiet Off | Caddy·etcd 각2회 입장 BLOCKED — 네 번 exit75, 1분 load75.46/상한8, 본문0. Large 종료 후 재확인도 63.66/상한8에서 거절됐다. |
| R51 Main 동결 빌드 | VERIFIED — `07911b81a9b7`, 입력 3,047개·컴파일 소스 1,270개, 3m33s·exit 0, daemon `8eab5d68`/helper `1e844b0c`. 원본 Record 전후 Main 입력 해시가 일치했다. |
| R51 원본 SDK Record | Caddy VERIFIED — 원본 590파일·FULL·검색·재오픈, 게시 27.127s. 기존 결과의 trace ingest·검증은 프로젝트 Python으로 각각 exit 0. etcd FAILED — 원본 1,421파일, B536,869,021 + 3,895 > 536,870,912, R327,739/W600,416/M796,121. `Storage write_preparation.rs:95`에서 최종 병합 전 거절됐고 FULL ACK는 0이다. etcd trace는 COMPLETE_DIAGNOSTIC이다. |
| R51 Large | VERIFIED — 원본 4,096파일·두 OS 재시작, `455a68d1-25a4-429a-b653-be33ca01e06a`, 1/1 PASS·68 skip·exit 0·100.963s. 드라이버 `e2e981aa`, daemon `8eab5d68`. |
| R51 Text 집중 회귀 | VERIFIED — 동결본 Normalizer 22/22·exit 0, `91d128d4-c4ae-459a-b517-47d998295777`. 남은 테스트용 packet 참조를 제거한 Main Lexical 81/81·exit 0, `ab35bc18-77c2-462b-aaeb-2b10894bfef2`. |
| R51 종료 Work 계측 | VERIFIED — 미사용 테스트 import를 정리한 Main Runtime 1/1·exit 0, `caa6b89c-4dcf-4355-b840-c3381bd1cbbb`. 원본 Caddy 성공 trace의 FULL 요청은 종료 B430,435,359/M639,835를 기록했다. |
| R51 quiet Off | BLOCKED — Caddy·etcd 각 2회 입장 시도 모두 exit 75, 외부 Rust 15개·1분 부하 42.77/상한 8, 본문 0회. |

**R51 인벤토리 준비.** 캐시 이진 탐색은 양쪽 이웃이 확인한 공통 접두부를 다시 읽지 않는다. 냉간 producer는 기존 프로그램을 먼저 조회하고 그 뒤 실제 suffix에 필요한 arena만 승인받는다. SQL packet은 SQL 경로에 진입할 때 준비한다. R48→R51의 etcd 첫 완료 lane34 1,127개에서 WorkBytes는 104,630,062→104,039,001B이고 mutation은 719,024로 같다. 원본 Caddy lane34 589개에서도 315,025B가 감소했다. 전체 arena가 거절되는 fixture에서는 40개 검증된 run을 재사용하고 새 suffix에만 mutation 1을 청구했다. R51의 etcd 전체 FULL 비용과 최종 mutation 여유는 미측정이다.

**잔여와 담당.** R51의 원본 SDK와 Large 결과는 `/Users/songmin/Library/Caches/quanta-index/native-sdk-caddy/closeout-20261117-o2gex5bs/`의 `release-main-51`과 `large-main-51-prepared`에 있다.

- **Text 구현 잔여.** R51 etcd의 ordered inventory read 19개 창은 158,295,650B를 썼고, 거절은 occurrence payload의 최초 identity 해시 전에 발생했다. 전체 512MiB 수용에는 후속 posting 병합·적용까지 포함하는 구조적 절감이 필요하다. 동일 배치·exact NFC·동일 leaf 재사용은 기존 구현을 사용한다. 별도 V7 진단에서 확인된 mutation 거절은 원본 FULL 검증에서 함께 확인한다.
- **Storage와 Core 경계.** 현재 raw payload와 예상 `ObjectRef`를 받는 prepared leaf 채택은 독립적으로 wire digest를 확인한다. Object 전용 typed handoff는 미구현이며 Map의 기존 prepared wire 이관과 별개다. 먼저 채택된 leaf의 재해시·복사에 귀속되는 실제 순비용을 분리한다. 이관이 필요한 경우 F와 공유 포트 작성권 및 기존 독립 검증·Original custody를 결속한다. 16,670B 설치 청구와 1,620B 초과분은 실패한 다음 작업의 수치이며 전체 남은 절감 목표는 원본 FULL로 판정한다.
- **검증 잔여.** 원본 etcd FULL과 Caddy·etcd 각 quiet Off 2회가 열려 있다. Large는 R51 결과를 소비하며, 관련 입력이 달라질 때 새 최종 후보에서 다시 검증한다. Tantivy 퇴역과 BOOT·호출자 전환은 각각 기존 담당의 결과를 소비한다.

**추가 병합 전 수리.** 기존 direct 검증 순회와 flat 프로그램의 그룹 수 순회에서 term table 크기와 가장 긴 position vector를 집계한다. 허용 term 수 밖이거나 단일 블록의 필수 최소 wire가 8,192B를 넘으면 alias 후보 backing과 유료 탐색을 건너뛴다. 기각된 입력은 기존 그룹 writer를 사용한다. R46 원본 etcd의 완료된 owner shape 1,128건에서 최소 term 수에 못 미친 Direct owner는 6개였다. 이들의 전체 key·position 탐색량 상한은 312B다. 별도의 원본 ASCII 1,392파일 계산에서는 8개가 최소 term 수에 못 미쳤다. 이 수치는 해당 탐색의 후보 범위다.

토큰 prefix 조회는 기존 byte 비교에서 서로 다른 ASCII 구분자를 식별한다. 이미 상속할 indexed prefix가 있고 그 직전 run이 정확히 끝나는 경우에만 마지막 indexed run도 재사용한다. 한 run 때문에 direct slab을 flat 병합으로 바꾸는 분기는 기존 경로를 유지한다. Unicode 구분자와 token 내부 경계는 기존 판단을 유지한다. 원본 etcd의 ordered-neighbor 모델에서 이 추가 mutation 재사용 후보는 최대 4 run이다.

확인 인벤토리의 기존 64칸은 같은 배치에서 공유를 완료한 정확한 본문을 우선 보존한다. 같은 항목이 있으면 반복으로 표시하고, 꽉 찬 캐시는 아직 반복이 확인되지 않은 첫 항목을 교체한다. 배치 준비와 owner별 posting 검증을 완료한 뒤에만 승격한다. 원본 manifest 순서·256-owner 창 모델에서 추가 적중 후보는 LICENSE 다섯 개와 YAML 한 개다. 소스 56,957B, 기존 토큰 재사용을 빼기 전 8,061 indexed run이며 실제 SDK 절감량은 미측정이다. 기존 회귀는 캐시가 가득 찬 상태에서 동일 본문의 다음 창 조회, 두 owner의 contribution·posting, 슬롯 한도와 반납을 확인하도록 보완했다.

세 수리는 기존 wire, Work 요금, 512MiB 및 읽기·쓰기·변경 한도를 유지한다. 첫 집중 실행은 공유 잠금에서 600초 대기 후 exit124·본문0이었다. Main `07911b81a9b7`의 debug/no-default Lexical 집중 실행은 run `b4493921-4acb-45d9-b072-a42f82afaea9`에서 **30/30 PASS**, 663 skip, 본문0.812초·exit0이었다. 선택한 변경 경로 9개의 실행 전후 SHA가 일치했다. R47은 같은 수정이 포함된 Main 입력을 새로 결박했다. 원본 FULL의 총 비용은 이 release에서 확인한다.

R47 etcd의 최초 거절은 flat prefix/suffix 병합에서 binary 탐색의 추가 두 row를 읽기 전이다. 해당 청구는 16B이며, 남은 WorkBytes는 8B였다. 진단 ingest는 `COMPLETE_DIAGNOSTIC`, accepted/written151,747·유실0, matched2/open0/omitted0이다. 완료된 lane34는 1,120건·B103,381,650/M709,496, occurrence lane46은 1,120건·B83,186,018이다. R46의 1,129/1,128건과 서로 다른 prefix까지 실행됐으므로 차이를 전체 절감량으로 해석하지 않는다. 재해시 API 변경만으로 이 단계와 남은 posting의 총량을 해결할 근거는 없다. R47 결과는 같은 Cargo 후보의 단일 FULL이며, quiet Off 본문은 아직 0회다.

R47 trace의 lane32 중 LexicalBuild 직계 ordered 창은 12개·B115,743,873이고, InventoryDiff 아래 owner별 경로는 337개·B38,762,751이다. R46의 관측 prefix는 ordered 16개였다. 현재 source에서 확인 인벤토리를 재사용한 Ready directory는 ordered 입장 증거가 없으므로 해당 owner는 기존 검증 경로를 사용한다. 이때 나머지 owner 모두가 fallback되던 경로를 수정했다. 연속 비지원 owner를 한 범위로 처리한 뒤 원 parent에서 packet을 봉인하고, 이후 지원 범위는 다시 ordered 창으로 처리한다. 실제 메모리 credit 거절 시에는 남은 범위를 기존 경로로 처리한다. 후속 회귀는 ordered → directory 두 owner → ordered의 packet과 모든 term/owner/EOF를 확인한다. 첫 빌드는 지역 변수 shadowing으로 exit101·본문0이었고, 수정 후 run `d3d7798a-cc57-4010-b62d-5e4e6b3066a0`에서 **13/13 PASS**, 681 skip, 0.282초·exit0이었다. 이 수정은 R47 동결 이후이며 R48에 포함한다.

R48 원본 etcd는 ordered 창 19개를 실행했고 owner별 alias lane32는 0개였다. 이 ordered 창의 B158,295,650/W461,661과 R47의 ordered+owner별 B154,506,624/W446,998은 다른 관측 prefix다. R48 lane34 완료1,127개, occurrence lane46 완료1,126개였다. 첫 거절은 occurrence의 inventory scratch edit가 SQL 문자열·bind를 만들기 전의 정규 batch 입장이며, 이번 427B 초과분은 거절된 다음 연산만 설명한다. 실제 비용은 최종 병합과 commit 전까지 추가로 남아 있다. 최신 trace는 139,708 event를 기록했다.

**현행 포맷의 정적 분류와 구현 경계.** Multipage writer/reader, descriptor receipt,
LCP owner merge, negotiated64/128 credit, one-shot term dictionary와 최종 map apply는
기존 구현을 재사용한다. 새 writer·대형 fan-in·별도 coordinator를 만들지 않는다.
확인된 추가 수리는 위 trace 부모 연결이며 공개 API·wire·가격 변경이 없다.
원 etcd 총량은 아직 해결되지 않았다. 다음 생산자 변경은 아래 두 비용 축을 모두
종료 조건으로 가져야 하며, 최종 Dense 설치만 고쳐 원 byte 실패를 닫을 수 없다.

**V7 mutation RCA.** 서로 시간상 겹치지 않는 완료 inventory lane34 span1,420개의
관찰된 Work 합은 M905,334/B131,861,121이다. `token_prefix_prepare::try_prepare`가
기존 정확한 prefix 프로그램을 재사용한 뒤 남은 suffix를 `ColdTokens::fill_from_cursor`로
생성하며, 실제 새 indexable run마다 기존 M1을 청구한다. Catalog reserve/confirm의
관찰 합 M59,334와 구분한다. 마지막 오류는8입력 confirm의 사전 청구에서 발생했다.
M16 승인 뒤 다음 M8을 거절했으며 확인 SQL에는 진입하지 않았다. 이를 객체2개의
확인 성공이나 새 파일8개 쓰기로 해석하지 않는다(`object_io=NOT_AVAILABLE`).
이 수치는 관찰된 prefix이며 전체 완료 비용·토큰 기반 mutation 하한이 아니다.
Shape의 complete owner 수와 별도 원 corpus census도 그대로 빼서 cache hit 수로 만들지 않는다.
원 manifest의 payload SHA를 같은 입력의 canonical owner census와 대조하면 동일 본문은
13그룹과 첫 파일 이후 반복24개다. 기존 reuse를 고려하기 전의 token mutation 절감 상한은
13,282회다. 전체 본문 cache 확장만으로 mutation 한도 수용을 예측할 수 없다.

**V7 posting 재작성 RCA.** trace SHA와 연속 이벤트188,731개 및 완료 footer를
대조했다. 첫 `NativeTextPostings/35` span(20801)은 정렬 run 봉인과 첫 term page
준비를 포함하며 B172,753,435/R383/W2,428/M0을 소비했다. 같은 lane의 뒤따르는
603개 비중첩 span은 B3,676,898이었다. 첫 span 안의 scratch SQLite 쓰기 요청은
9,629회/39,440,384B였다. 이 요청량과 WorkBytes는 별도 지표다. 첫 span 전체를 제거 가능한 복사 비용으로
해석할 수 없다. `merge_group`은 phase0의 검증된 packet을 읽고 정렬 순서를 유지하면서
출력 packet과 term cardinality metadata를 새로 기록한다. 이때 기존 일반 writer는
같은 term의 여러 owner 행에 term key를 각각 기록했다. cold merge가 기존 packet 내부
literal 참조 형식 `QITXPR03`을 쓰도록 연결했다. 읽기는 packet-local literal 범위,
정렬·digest·fence를 동일하게 검증한다. 실제 SDK 절감량은 원본 입력의 재실행으로 확인한다.

Prefix를 상속하지 않은 토큰 프로그램은 이미 정렬된 단일 slab을 그대로 보유한다.
기존 검증 순회에서256행 단위 position minimum을 만들고 locator 배열 생성과 후속
materialization을 제거했다. 재사용할 prefix가 있는 후속 owner는 기존 index로 필요한
chunk만 읽어 정확한 case·term·position을 보존한다. 이 두 수리는 사설 prepare와 scratch
경로에 한정한다. 새 토큰 생성의 변이 청구와 Catalog 등록 비용은 남는다.
4,096행 fixture의 직접 프로그램 메타데이터는 B32,840에서 B72로 감소했다. 동일 prefix
선택 B5,760, 자식 프로그램 B3,212, 전체 case·position 결과와 메모리 반납을 검증했다.
당시 실행은 `e98538d8-95e0-43b7-93a7-06ec5ee355e3`이다.

R45 Record의 Caddy token prepare 완료589개에서 lane34 합은 R44의 B50,256,331에서
B49,673,739로 감소했고 M377,888은 같았다. etcd는 완료861개까지만 비교할 수 있다.
R44 첫861개 대비 B82,372,301→80,632,824, M559,914→559,903이며 실행 코호트와
경로가 완전히 같지 않다. 두 trace는 `COMPLETE_DIAGNOSTIC`이다. etcd는 최종 posting
병합에 진입하기 전에 취소돼 새 패킷 형식의 SDK 절감량은 미측정이다.

**병합 전 후속 수리.** 같은 냉간 배치에서 레시피와 본문 객체 참조·길이가 일치하는
새 owner는 원 인벤토리 ordinal을 공유하도록 했다. 기존 확인 인벤토리 캐시에서
빗나간 경우에도 두 번째 본문 읽기, token row 생성, 별도 inventory root 준비가
필요 없다. 실제 owner별 contribution과 posting은 유지한다. inline alias의 staged
anchor는 공용으로 읽고, 각 owner는 동일 attempt의 seal과 EOF를 확인한다. 후보 조회는
기존 배치의 metadata를 사용하고 실제 비교를 WorkBytes에 반영한다. 집중 회귀는 같은
입력으로 재사용의 유무를 비교해 mutation과 WorkBytes, 두 owner의 posting 순서·참조·EOF를
확인한다. 원본 manifest 순서와 256-owner 창의 정적 모델은 새 공유 후보 14개를 찾았다.
기존 prefix 재사용과 실제 경계, SDK의 순 Work 감소는 실행으로 확인한다.

별도 토큰 캐시의 exact NFC hit에서는 이미 검증을 마친 프로그램의 case row 수를
사용한다. 프로그램 생성에서 `row / 2 == indexed runs`를 대조하고, hit는 정규화된 본문의
완전한 일치를 확인한 후 token run을 다시 순회하지 않는다. 회귀는 direct와 inherited
프로그램, 257바이트 run 제외, 위치 6 보존, mutation 0과 실제 조회 WorkBytes를 검사한다.

V7의 23개 lane32에서 W611,873과 owner별 term 합611,507의 차이는 현재 비용 모델에서
phase-zero 패킷 약366개에 해당한다. 이 중 최대64개를 RAM에 보관하는 안을 검토했다.
보관된 패킷이 병합 전 사라지면 원본 복사본이 없고, 보관 grant가 후속 필수 토큰 배치를
거절시킬 수 있어 해당 안을 철회했다.

원본 etcd의 delimiter만 바뀐 동일 토큰 프로그램 후보는 별도 NFC 분석에서 0개였다.
같은 절대 위치의 비prefix 후보 분석에는 추가 본문 순회 비용이 들어간다. 더 큰 범위의
내부 구간 재사용에는 NFC/token 경계, 제외 run을 포함하는 ordinal과 위치 delta를 증명하는
프로그램 변경이 필요하다. V7에서 관측된 M1/신규 run과 Catalog·Storage의 각 고정 비용은
유지된다. R46 원본 etcd에서는 마지막 inventory prepare의 occurrence leaf 채택에서
WorkBytes 거절이 발생했다. 완료된 lane34 1,129개는 B104,817,942/M722,988이고 이 중
마지막 owner의 lane35가 실패했다. posting read는 완료16개이고 최종 posting 병합은
미도달이다. R44와 R46의 수락 총량은 서로 다른 거절 지점에서 측정됐다.

Lexical의 `identify`는 canonical leaf wire identity를 계산하고, 현재 `PreparedObjectInstallRun`
경계는 raw payload와 예상 `ObjectRef`를 전달한다. Storage는 wire를 구성해 identity를
다시 검증한다. 동일 payload의 canonical wire와 등록 정보를 소유한 admission 후 handle을
전달하면 반복 인코딩·해싱을 줄일 수 있다. 기존 raw-input 경계의 독립 검증을 보존해야 하며,
순회 제거 후에도 실제 512MiB FULL 결과로 총량을 확인해야 한다. R46 집중 회귀는
Large 드라이버가 빌드된 뒤 재제출했고, 두 시도 모두 공유 잠금에서 exit124·본문 0이었다.
17:34 KST live Main HEAD는 `07911b81a9b7`이며 R46의 런타임 결과는 `587b4237` 동결본에
결박된다. 이 시각의 quiet Off 재확인은 Rust6개·1분 load25.32/상한8에서 거절됐다.

**계측 수리의 의미.** lane38 내부 flush가 바깥 context를 lane39에 전달해, 중첩된
두 inclusive Work span이 형제로 기록됐다. V7의 spans27,628개와 healthy footer는
그대로 유효하지만 parent-ID만으로 자식을 합하거나 부모에서 차감한 direct 비용은
무효다. 실제 분석기는 `child Work exceeds parent`로 거절했고 이 실패를 보존했다.
수리는 active scoped parent를 전달하며 명시적 `None`도 유지한다. 과거 trace를
고쳐 쓰거나 이 수리만으로 모든 diagnostic lane의 비용 포함 관계를 보장하지 않는다.
집중 검증은 R44 동결 의존성에서 동일 회귀를 실행했다. 구 생산 코드는 실제
`nested posting flush lost its active Work parent`로 실패했고, 생산 `text.rs`만
SHA`282bf1b9` 수리본으로 바꾼 GREEN은 전체67-term·warm CAS·이전 root 보존과
중첩 flush 검사를 통과했다. 명령은 다음과 같다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --release --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E 'test(=native::text::tests::occurrence_blocks::prepare_read_cost::posting_prepare_reuses_packet_across_case_seeks_within_each_stream)' --test-threads 1 --no-fail-fast --config-file <R44-nextest.toml>
```

Full SDK·Large·전체 library의 새 Main 재검증을 이 집중1건으로 대체하지 않는다.

**다음 변경의 채택 기준.** Byte 축은 실제 phase0 packet encode→SQL write→merge→
SQL replay의 제거 여부로, mutation 축은 새 suffix token 생성과 실제 Catalog 객체/edge
작업의 제거 여부로 각각 검증한다. 같은64-term packet의 기존 Storage wire dedup,
이미 있는 prefix program·confirmed root reuse를 새 절감으로 중복 계산하지 않는다.
새 bounded producer/consumer가 필요하면 첫 효과 전 Original 승인, 전체 source EOF·
count/seal·독립 replay, 후속 실패의 poison, 한 atomic FULL을 함께 보존한다.
기존 논리 posting debit을 유지하고 요금 계수·budget reset·Corpus 필터로 통과시키지 않는다.
동일 frozen dependency·원 입력에서 전체 root/term/owner/position과 net Work를 비교한 뒤
원 SDK512MiB/60s·R/W/M 각100만의 실제 FULL을 채택 조건으로 삼는다.

업계 참고 기준은 bounded ingest/flush와 merge의 명시적 구분([Lucene IndexWriter](https://lucene.apache.org/core/10_4_0/core/org/apache/lucene/index/IndexWriter.html)),
terms와 postings codec 책임 분리·block encode([Lucene104PostingsFormat](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/codecs/lucene104/Lucene104PostingsFormat.html)),
비교 증거를 재사용하는 LCP merge([원 논문](https://arxiv.org/abs/1403.2056))다.
이는 구조 선택의 참고이며 현 저장 정책·고정 예산에 맞는 성능 증거는 아니다.
압축 codec·owner-only membership는 reader/writer·recipe policy·GC까지 함께 바꾸는
별도 저장 정책 변경이다. 현재 측정으로 그 breaking change가 필수라고 결론내리지 않는다.

**후속 LCP 수리의 현재 경계.** R35 이후 64-owner merge는 정확한 LCP loser
메타데이터를 재사용하도록 구현했다. R36 release 빌드는 source3,026/changed0/
extra0·compiler0/11m48s로 검증됐다. 집중 run6715e945는10건 중8PASS/2FAIL:
두 실패는 이전 비용 골든707,886→695,905B와56,748→55,009B 차이였다.
첫 fixture는 비용 assertion 뒤 tuple/EOF 검사에 도달하지 못해 전체 통과로
판정하지 않았다. 비용 단가는 그대로이며 추가 metadata와 successor 비교도
계상한다. 이전 값보다 작은지와 새 exact 값을 모두 유지하고 비용 단언을
전체 출력·반납 검사 뒤로 옮긴 R37은 release 빌드 VERIFIED다: Main2f09e557의
동결 입력3,027개·changed0/extra0·artifact600·compiler0/16m23s,
daemon922cdc04/helper130fc432. R34 normalizer 입력38개와 동일 rlib의 출처를
재확인했으며 Cargo fresh=true를 새 컴파일로 해석하지 않는다.
집중10/10은 VERIFIED(run32dee1ff,본문0.156s)이며 전체 tuple·EOF·반납 검사를
포함한다. 전체 library는 R37에서FAILED:892/953 실행·891PASS·Storage1FAIL,
2 skips·fail-fast로61건 NOT_RUN이다. 손상 객체 읽기 회귀의 fixture 준비 중
ResourceLimited였으며, 현재 Main에는 반례마다 root·예산을 분리한 수정이 있다.
새 metadata 계산의 Clippy 산술 규칙 위반도 확인해 checked 계산으로 수리했다.
별도 R38 Clippy의20진단 중18개는 Snapshot에 clippy.toml이 빠진 정책 불일치였다.
실제 LCP 비교 분기·closure 진단2개도 수리했다. 다음 Clippy는 Main 설정
SHA351788ae를 checkout 밖에 고정하고 CLIPPY_CONF_DIR로 그대로 적용한다.
R38은 Mainc81fadda 입력3,027개를 다시 동결했지만 Catalog의 컴파일 오류4건으로
FAILED(compiler101,source changed0/extra0)다. read_only의 참조 타입 오류3건은
현재 Main에 이미 수리돼 있으며, boot_manifest의 usize SQL 바인딩은 기존 담당에게
인계했고 담당의 checked 바인딩 수리를 현재 Main에서 확인했다. R39는
이 수리와 LCP 최종 정적 수리를 포함한 Main885d3e16 입력3,027개에서 빌드0과
정책이 결속된 Lexical Clippy0(진단0)을 확인했다. 전체 library도 fail-fast 없이
953/953·미실행0으로 통과했다(run10fac6c9). R37/R38 SDK는 실행하지
않았으며 R39 원본 SDK의 Caddy60초 취소·etcd byte 거절은 위 표에 반영했다. 원 etcd512MiB 통과나 논문의
복잡도 상한을 현재 구현 성능으로 확대하지 않는다.

**전체 입력을 사용한 구조 분석.** 같은 원본 etcd1,421파일·8,666,217B의 경로와
각 SHA를 실행 전후 대조했다. 고정 Rust1.92와 검증된 normalizer로 run1,009,671개
중 긴 run7개를 제외한1,009,664개를 확인했다. owner별 두 case의 고유 posting 행은
611,539개, 전역 고유 term은133,221개다. 이는 SDK 실행이나 Work 요금 하한이 아니다.
동일 suffix의 두 case가 같은 owner·position을 갖는10,852쌍도 실제 Dense draft
공유 수가 아니다. Draft는 owner와 packed occurrence object ref를 비교하므로,
의미상 같은 위치라도 block ref가 다르면 공유할 수 없다.

**미채택 후보.** 같은64-term 페이지의 미설치 Dense draft를 전체 행·packing plan으로
비교해 중복 인코딩·해시·prepare를 제거하는 코드는 checkout 밖에서 검토 중이다.
Storage는 이미 동일 wire의 실제 파일 설치를 제거한다. 이 후보를 중복 fsync 제거로
보고하지 않는다. 중복이 없는데 선택 메모리 승인을 필수 설치까지 보유하는 결함을
찾아 checkout 밖 후보에서 보완했고 SHA 키의 임시 복사도 빌린 비교로 바꿨다.
Main 반영·컴파일·원 입력 A/B는 NOT_RUN이다. 최종 posting apply
전에 발생한 현재 etcd의 byte 거절을 이 후보만으로 해결했다고 판단하지 않는다.
`FileObjectStore::install_batch`는 동일 packet의 ref+전체 wire를 대조한 뒤 unique만
Catalog 예약·확인으로 보낸다. 따라서 이 경우 후보의 Catalog mutation 절감은0이다.
leaf 중복이256개 packet 경계를 넘을 때의 추가 절감 여지는 별도 실측 대상이며,
현재 원 입력의 빈도는 NOT_REPORTED다. Directory는 페이지당 한 batch라 기존
중복 제거를 이미 사용한다. 이 후보를 mutation-cap 해결책으로 승격하지 않는다.

**단일 Dense leaf의 직접 root는 별도 미구현 후보다.** 현재 완성된 page plan이
하나여도 writer는 DD01 directory를 설치하고, read·cold memo의 expected root도
directory를 요구한다. 인증된 최종 `plans.len()==1`에서만 DP01 leaf를 직접 root로
사용하면 실제 directory 객체 하나를 생략할 여지는 있다. Warm은 이전 fence를
보존하므로 행 수 cutoff로 단일 leaf를 가정하지 않는다. 채택 시 root tag·recipe
policy, checked DF/lookup/scan, cold expected root·receipt, warm one↔many CAS와
GC 도달성을 함께 바꿔야 한다. witness는 유지할 수 있지만 content-addressed
재사용을 고려한 실제 mutation 절감과 query 비용은 미측정이다. 현재 공통 Catalog
거절의 해결책으로 채택하지 않는다.

Dense DP01 leaf는 이미 childless다. 행의 witness32B를 지우면 row당 Catalog child가
사라진다는 가정은 틀리다. 현재 reader는 pinned owner Contribution·inventory의
occurrence ref와 posting witness를 대조한다. 이 증거를 제거하는 owner-only 형식은
별도 저장 정책·모든 writer/reader의 의미 변경이며, 현재 수리에 섞지 않는다.

64-owner 상수를 크게 만드는 것도 단순 연결 수정이 아니다. 현재 InventoryBatch는
최대256 root를 완료한 뒤 backing을 반환하고 내부64-owner run을 봉인한다.
전체 owner를 함께 유지하려면 metadata·cursor·held block의 Original 승인과 배치
수명을 바꿔야 한다. 트리만 확장해도 ordered run은 SQL packet을 계속 기록하므로
phase0 입출력이 사라지지 않는다. 새 전면 병합이나 persisted layout을 측정 없이
채택하지 않으며, 현재 수리에 공개 API·wire·가격 변경은 필요하지 않다.

**phase0 SQL 제거의 조건부 설계 경계.** 현재 SQL은 저장뿐 아니라 전역 정렬,
64-term cardinality 사전검사·독립 재읽기와 sensitive/folded 페이지 짝짓기를 맡는다.
`apply_cold_bounded_postings`도 concrete `PostingCursor`와 SQL epoch에 결속된
`DensePostingPrepassProof`를 받는다. 따라서 전역 winner cursor를 기존 소비자에
연결하는 것만으로 제거할 수 없다. 실제 채택에는 다음 작업이 함께 필요하다.

- 첫 posting 효과 전, 봉인된 changed-count 상한으로 Original을 승인하고 모든 배치의
  owner·Contribution·alias·DirectoryReceipt·원 attempt/slot 결속을 보존한다.
  현재 배치 완료 시 이 metadata는 반환되며 owner top SQL만으로 receipt를 복원할 수 없다.
- source EOF와 prefix cache 반환 후 실제 cursor·tournament·bounded replay/page의
  동시 credit을 승인한다. 이 시점의 miss만 기존 64-owner 경로로 돌릴 수 있으며,
  posting 설치나 top edit 이후 오류는 poison/terminal이다.
- producer와 consumer의 같은 검증 계약에서 count·독립 replay·두 case의 scheduling·
  전체 owner EOF와 descriptor seal을 함께 제공한다. 일반 update/prior/mixed 경로를
  cold shortcut으로 통과시키거나 SQL proof를 만든 것처럼 취급하지 않는다.
- 기존 논리 posting write/bytes debit은 보존한다. 제거 후보는 실제 phase0 인코딩,
  SQL packet 쓰기·재읽기·copy/parse이며, 여러 run의 final packet과 term SQL이
  남는 설계라면 그 비용도 계속 계상한다.

이는 새 bounded producer/consumer 구현이 필요한 조건부 후보이며 Main에는 반영하지
않았다. 1,421개 owner의 동시 credit 적합성·실제 절감량·SDK512MiB 통과는 미확정이다.
아래의 이미 연결된 bulk dictionary와 최종 map apply 절감을 중복 주장하지 않는다.

**이미 연결된 bulk 경로.** cold 빈 root는 모든 term edit를 처리한 뒤
`term_dictionary_write::prepare`를 한 번 호출한다. 하나의 `Window`가 EOF까지
페이지를 생성하며, 두 root의 마지막 `apply_stream`도 빈 root에서 기존
`cold_stream::EmptyStream`의 sorted radix 경로를 사용한다. 64-term 버퍼마다
기존 term 페이지를 읽어 COW하는 구조가 아니다. 해당 경로를 새 bulk IR로
중복 구현하지 않는다. 남은 SQL 왕복은 posting·top edit와 fence anchor 저장·재읽기다.

phase0 packet도 이미 `&[u8]`로 rusqlite에 전달된다. vendored rusqlite의
`SQLITE_TRANSIENT` 바인딩은 SQLite 소유 복사를 요구하고, 현재 안전 API에
borrow/move만으로 이를 없애는 경로는 없다. `2 × payload + 6208`은 명시된
Work 청구식이며 실제 memcpy 두 회의 증거가 아니다. 이를 절반으로 낮추거나
새 unsafe `SQLITE_STATIC` 수명 경계를 만들어 512MiB 문제를 닫지 않는다.

2026-11-19 wide writer 수리 전 확인에서 Main의 Native Lexical·Normalizer 관련116개
입력은 R39와 모두 같은 SHA였다. 이후 아래 credit 수리의 생산2개·테스트1개가
변경됐으므로 현재 Main 전체가 R39와 같다는 의미가 아니다. 인과 비교는 R39의 같은 의존성에서 LCP 이전 두 생산 파일만
바꾸는 별도 진단으로 분리한다. 원 R/W/M 각100만과 Core Original64MiB를 유지한다.
진단 Work10GB/1800s는 SDK512MiB/60s 수용이 아니다.

진단 v1은 fixture의 Arc trait 변환 두 곳에서 compiler101·본문0으로 실패했다.
v2는 본문1개에서 Reads100만/B893,082,468로 실패했지만 SDK 경로의 비용으로
사용할 수 없다. 관측용 proxy가 retained read·scoped read·cold bound·leaf run을
전달하지 않고 `subtree_packing`을 Nested에서 None으로 바꿨다. 또한 plain
managed-store 생성자에는 제품 read/write IO가 없었으며 커스텀 allocator도 실제
ledger의 반환·retained floor와 달랐다. 한도를 높이거나 이 값을 제품 하한으로
사용하지 않고 실패 기록을 보존했다. 현재 Main의 실제 Native Text wrapper들은
해당 capability와 packing을 유지함을 별도로 정적 확인했다.

v3는 기존 `NativeIndexService::bind_with_capabilities_and_maintenance` 팩토리를
직접 사용해 실제 ledger·managed IO·scratch·Catalog cache를 재사용하며 proxy와
커스텀 allocator를 제거했다. 원 프로필의 Core64MiB/global384MiB·원 입력을
고정한 Text-only SourceAdvance다. Literal/FileMap·전송·worker 경로를 제외하므로
전체 SDK 절대 비용으로 해석하지 않는다. v3는 fixture의 moved-value 오류로
compiler101·본문0에서 실패했다. Copy인 instance ID를 이동 전에 보관하도록 수리한
v4를 같은 동결본에서 실행했다. 실패 시 WorkBudget의 최초 거절도 출력하도록 했으나,
컴파일32m49s 뒤 본문1건은 임시 소켓 디렉터리0755를 제품의0700 정책이
거절해 실패했다. Work/output marker가 없어 corpus 비용은 미측정이다.
동일 소스와 컴파일된 바이너리에서 자식 umask077을 명시한 재검증을 준비한다.
정상 완료한 본문1건·출력·원 입력과 R/W/M을 확인하기 전에는 후속 반사실 비교를
실행하거나 인과 비교를 VERIFIED로 승격하지 않는다. 이 진단은 후속 wide writer
credit 수리를 포함하지 않는다.

v5는 같은 probe·생산 소스와 기존 컴파일 결과를 유지하고 자식 umask077만 명시했다.
Nextest `e6f84cd2-bba5-406c-a50a-224bc31134e8`에서 본문1건을 실행해
31.788s·exit100으로 FAILED했다. submit 사용량은
R466,431/W806,228/B730,022,033/M999,996이며, 추가 mutation8회를1,000,000회
한도가 거절했다. caller는 `catalog/native/authority/managed_install.rs:108`,
failed stage는 `BuildPrepare`, RAM 거절은 없다. 이8회는0-child 확인 또는1-child
예약 모두 가능하다. 객체 kind·정확한 Text 하위 단계·Dense input 수는 미측정이며,
토큰 수를 mutation 하한으로 바꾸거나 객체 수를8로 나눠 추정하지 않는다.
source stage의 B58,622,025/R164,345/W56,078/M55,694는 별도 예산이므로 submit에
더하거나 빼지 않는다. v5의10GB Work 진단을 원 SDK512MiB 수용으로 해석하지 않는다.
FULL output이 없어 LCP 이전·Dense 후보의 후속 전체 A/B는 실행하지 않았다.
진단 Snapshot을 R39로 복원한 뒤 아래 wide credit 수리의 단일 RED/GREEN을 검증했다.

**표현 변경의 경계.** Dense leaf DP01은 행마다 owner32B+witness32B를 저장하고
DD01 fence도 고정66B다. prefix·ordinal 압축은 현재 구현돼 있지 않다. 이를 채택하면
저장 magic·recipe policy와 lookup/scan/warm writer/expected-root 재구성을 함께
바꾸고 이전 policy 거절 또는 rebuild를 검증해야 한다. 현재 private LCP 수리에는
이 breaking change가 필요하지 않으며, 실제 비용 측정 없이 새 codec을 채택하지 않는다.
Term dictionary의 TopCursor는 이미 64행 keyset SQL 버퍼를 사용한다. 용어별4KiB
point query를 제거한다는 가정으로 prefetch를 중복 구현하지 않는다.

**R39 이후 wide writer credit 수리.** 실제 cold 호출부는 SQLite 지원 시
`PostingEdits`를128행으로 구성하지만, ordered owner window의 local credit은
64행만 가정했다. 행 backing의 마지막 성장 peak는96이 아니라192행이며,
긴 키와 lease 상한도 실제 writer 폭에 맞춰야 한다. `local_credit_limits`에
negotiated64/128 폭을 전달하고 효과 전 승인에서 그 값을 사용하도록 Main을
수리했다. 기존64행 계산·요금·한도·wire는 유지한다. 128개의40B 키를 실제 batch
flush에 넣고 두 term page의 전체 tuple·EOF·반납을 검사하는 회귀를 추가했다.
엄격한 child-credit 계약을 검증하며, pooled slack이 있는 실제 Native ledger에서
동일 실패가 관측됐다고 주장하지 않는다. R39의953 PASS는 이 후속 수리를 포함하지
않는다. 후속 집중 검증은 VERIFIED다. 같은 테스트 SHA1ccbd305와 동결 의존성에서
구 생산 코드가 실제 `batch.flush`의 `ResourceLimited; first_denial=Some(Memory)`로
실패했다(run5a1d37b4-3c3a-42a1-a8ba-6cea202a900c, 본문1/FAIL1, exit100).
생산2파일만 수리본으로 바꾼 GREEN은 본문1/PASS1·exit0이다
(run3d5b20c1-3933-4225-b274-742268a5e127). 정확한 선택기는
`test(=native::text::tests::cold_posting_run_tests::wide_posting_writer_funds_long_keys_through_full_packet_and_eof)`이며,
`./scripts/cargow --lane test-workspace-lane nextest run --release --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E <selector> --test-threads 1 --no-fail-fast`로
R39의 같은 Nextest 설정을 적용했다. 소스3,027개와 설정·target6320을 전후 확인했고,
검증 후 R39 소스를 복원했다. 이 집중 증거를 전체 Main 빌드·library나 SDK 수용으로
확대하지 않는다. 후속 R40은 Main8cfa3b39 입력3,034개에 이 수리를 포함해 다시 동결했다.

R40 release 빌드는 통과했으나 정책이 결속된 Lexical library Clippy에서
`text_query.rs:1151`의 인자 수9/8, `text_query/preview_projection.rs:204`의
redundant guard와326행 enum wildcard, `text_reader.rs:707`의 `as` 변환을
확인했다. 이는 설정 누락이나 wide credit 회귀가 아니다. F가 진행 중인 Preview
CorruptObject 수리와 같은 파일이라 기존 담당에게4건을 인계하고 Full3libs를
실행하지 않았다. 다음 동결본에서 수정된 생산·소비 경로를 함께 검증한다.

F의 Preview·VFS 수리를 포함한 R43은 빌드와 Clippy를 통과했다. R41의 compiler0
뒤 normalizer 출처 검증 실패는 R34만 허용한 외부 검증 도구의 오래된 기준이었다.
실제로 컴파일한 R40의 영수증·동일 rlib·입력38개·toolchain·source marker를 검증하는
재사용 경로로 보완했다. R42는 공유 Catalog 소스가 복사 중 바뀌어 Cargo 진입 전에
중단했으며, 이 두 시도를 완료로 승격하지 않았다.

R43의 전체958건 중 유일한 실패는
`object::tests::protected_snapshot_page_stream_authenticates_exact_wire_at_finish`다.
정상·변조2종·절단의 독립 사례가 1,000,000B fixture 예산을 공유했다. 한 번의
open에서 wire131,120B와 decoded wire131,104B를 그대로 청구해 네 번째에서
누적1,048,896B로 거절됐다. Root는 기존 담당과 중복 수정을 조정한 뒤 변조 각
사례와 절단 사례에 같은 한도의 독립 검증 예산을 주도록 테스트2개 hunk만 수리했다.
정상 사례의 reads1/writes0와 변조·절단 CorruptObject 단언은 유지한다. 생산 코드,
청구식, 원 SDK의 단일 FULL·누적512MiB 예산은 변경하지 않는다. 후속 집중1/1과
R44 전체958/958에서 이 fixture 수리를 검증했다. 집중 controller의 filtered 수 분류
오류와 원 FAILED 영수증은 보존했고 본문 결과와 구분한다.

**정적 수리본의 검증 경계.** 위 library937·cache19·원본 A/B는 각각 실행 당시
소스의 결과다. 후속 Clippy 재확인에서536건이 나와 이 채팅 소유 Native 생산·소비
경로를 분담 수리했다. Checked 산술·슬라이스, 지연 오류 생성, private 타입 별칭과
모든 내부 호출부를 맞췄고 기존 wire·요금·Original grant 순서를 유지했다.
`VectorIndex::insert`는 잘못된 슬롯·ordinal·512개 초과를 첫 변경 전에 거절하며,
실패 시 슬롯 불변 회귀를 추가했다. 큰 enum/고정 배열을 새 heap으로 옮기지 않았다.
이 수리본의 Lexical library Clippy는 전체536건을 닫고 exit0(44.31s)으로 통과했다. 회귀·최종 실행 파일 빌드는 별도 재검증 대상이며 과거 PASS를 복사하지 않는다.

| 정적 경계 | 코드 분류와 확인 범위 | 남은 판정 |
| --- | --- | --- |
| Multipage writer/reader | 구현 완료. byte/child/row 한계, COW fence·CAS, 설치 ordinal/ref와 poison까지 대조 | R44 owning library VERIFIED; 원 etcd 수용은 FAILED |
| Cold stream/receipt | 구현 완료. root·attempt·slot·descriptor E, 독립 Unsealed 읽기, EOF·partial failure·Original 수명 대조 | R44 owning library VERIFIED; SDK 전체 총량과 구분 |
| Prefix cache/program | 구현 완료. 동일 cache/cursor 소유권, delimiter·Unicode·position, 승인 실패 전 mutation0, 원본 A/B 비용·wire 대조 | R44 owning library VERIFIED; 추가 총량 수리는 열림 |
| Literal 봉인 | 구현 완료. source prefix·전체 키·EOF와 독립 digest, 잘못된 header·중간 owner 반례 대조 | R44 owning library VERIFIED; 당시 component 비용 증거만 재사용 |
| 새 API/표현 | 현재 수리에는 추가 구현 불필요. private census 참조·검사된 인덱스 반환은 모든 caller를 함께 갱신 | 공개 API·wire·가격 변경 없음 |
| 빌드·SDK·Large·Off | 고정 R44의 실제 source·artifact·원 입력 결속 | 빌드/Caddy/Large VERIFIED, etcd FAILED, Off BLOCKED/NOT_RUN |

**실제 잔여.** 원 etcd FULL의 byte/mutation 총량 해결,
최종 변경 후보의 영향 검증과 quiet Off 입장·실행이다. 후속 trace 수리의 집중 회귀는
위 표대로 VERIFIED이며 R44 제품 검증과 구분한다. R44 Caddy·Large는 위 표대로
검증됐으며 후속 생산 코드 변경의 증거로 자동 승계하지 않는다. 원512MiB·R/W/M 각100만·파일3만·SDK60·단일 atomic
FULL ACK를 유지한다. 내부 scan/SQL 최적화 완료만으로 이 원 제품 수용을 닫지 않는다.
고정 입력의 동일 실패를 반복하거나 토큰 요금을 삭제·여러 SourceAdvance로 나눠 통과시키지 않는다.

**이전 코호트 경계.** 원 SDK 전후 2,945개 source/build 입력은 같았다. 후속 문서 감사에서
그중 18개가 Main과 달라진 것을 확인했다. ColdTokens·occurrence_pack·packed_runs·
Directory·Storage map의 5개 생산 소스는 원 SDK 해시와 같다. 기존 실행 결과는
그 고정 코호트의 증거로 재사용하고, 최종 후보에서 바뀐 영향 범위는 다시 검증한다.

**완료·조건부 패킷.** NTX1a, NTX2A와 Shared Small/Directory DATA/Confirmed cold Dense,
Small cross-page, T-PREPASS-COUNT-DATA, T-CENSUS-SINGLE-FILL은 기존 구현·범위별
회귀를 재사용한다. NTX0은 비교 기준선이다. NTX1b·NTX3는 현재 실행 범위 밖이며,
NTX2B의 key interning은 측정·입장이 필요한 조건부 후보로서 필수 종료 작업이 아니다.
NTX4의 원 제품 수용은 위 표대로 열려 있다.

이전 437/437 `4f9e0f40-8f95-4ad6-a31f-21cd66e43852`와 별도 EOF 86/86
`72852bce-92a0-4a68-87f3-c9c04d60320c`는 당시 scope에 귀속한다.
R11/R12와 앞선 Catalog·Literal·posting 후보의 source·실패 수치·본문0·trace 한계는
[정확한 원문](../../../ARCHIVE-INDEX.md#2026-11-17-rfc-source-and-current-sdk-reconciliation)에 보존한다.

## 2. NTX0 — 현재 패치가 포함된 기준선

- F의 기존 release/SDK 실행 큐에서 관련 source와 원 입력을 결속해 후보를 만든다. 새로운 빌드 큐·worktree·서명 설치 후보를 이 패킷에서 만들지 않는다.
- ordered packet은 inventory diff의 `PostingEdits::flush → PostingRuns::append → flush_pending`에서도 실행된다. etcd의 최종 posting map 단계 미진입을 이유로 이 패치의 영향을 배제하지 않는다.
- 같은 Caddy 590파일·etcd 1,421파일에서 게시·검색·재오픈·source digest와 최초 거절을 확인한다. 실패하면 실패 prefix·완료 작업량을 그대로 기록한다.
- Record는 원인 진단, quiet Off는 성능 수용에 사용한다. Off 미입장은 BLOCKED/NOT_RUN으로 구분하고 contended Record의 시간으로 60초 통과를 만들지 않는다.
- **인계:** candidate source/binary·입력·명령·관측 결과·정확한 실패 단계. 집중 회귀나 Large 성공을 SDK60 성공으로 복사하지 않는다.

## 3. NTX1 — 기존 batch 통합에서 스트리밍 설치기로

이 절의 source-ready SHA·재실행 대기 문구는 NTX1a/계측의 단계별 이력이다. 현재 실행권·후보·Small 진행 상태는 §1과 §7의 현재 표를 따른다. 이력의 이전 PASS를 새 후보의 수용으로 재사용하지 않는다.

**변경 함수:** [검증된 cold draft 경계](../../../../crates/quanta-index-lexical/src/native/text/entry_patch.rs#L962), [dense 설치기](../../../../crates/quanta-index-lexical/src/native/text/dense_posting/write.rs#L469), [wrapper·Top caller](../../../../crates/quanta-index-lexical/src/native/text.rs#L12053). 기존 Text writer가 함수 hunk를 맡고 같은 parent hunk는 한 writer가 합친다.

### NTX1a — 현재 작업 창 전체의 leaf+directory 통합

2026-11-15 F의 직접 인수 지시에서 이 채팅의 최초 production 수정은 `dense_posting::write::install`의 현재 작업 창으로 한정됐다. `entry_patch.rs`/`text.rs`의 추가 parent 수정은 필요성이 확인된 정확한 hunk만 원 writer에게 인수한다.

1. source EOF/digest 검증을 끝낸 현재 창의 새 leaf ref를 `identify`로 먼저 구하고 그 ref로 directory를 구성한다. 재사용 leaf를 포함한 child 인증은 기존 계약을 사용한다.
2. 창 전체의 새 leaf+directory 합집합이 256객체·aggregate wire 64MiB 및 payload/children/backend 준비/출력 slots의 동시 Original 승인을 만족하면 기존 `StagedObjectBatch`에 leaf들→directory들 순서로 넣어 한 번 설치한다. term마다 별도 batch를 만드는 방식은 기본안으로 사용하지 않는다.
3. 창이 적합하지 않으면 첫 효과 전 판단으로 기존 공동 leaf batch→directory batch 경로를 유지한다. 미승인 자체를 terminal memory denial로 기록하지 않는다. 이미 준비한 ref/payload를 재사용하고, 실제 재계산이 있으면 그대로 차지한다. 설치 시작 후 실패는 fallback이나 size 변경 없이 반환한다.
4. 실제 Store+Catalog+복수 Draft caller에서 두 batch COMMIT 4→2, 동일 root/rank/child edge·GC·reopen, 늦은 parent 실패와 자원 거절을 먼저 RED/GREEN으로 검증한다. 표준 SDK·원 예산은 그대로다.

이는 최종 스트리밍 구조의 첫 적용 단위다. 전체 스트리밍 queue·wrapper 통합·prepared API 확장을 이 첫 hunk에 함께 넣지 않는다.

### NTX1b — 최종 생산·소비 계약

1. 첫 object 설치 효과 전에 해당 페이지 원본의 CAS·순서·행 수·EOF와 prepass epoch를 모두 검증한다. 검증 중에는 payload/draft 준비만 한다.
2. 검증된 Draft를 root별로 소비한다. exact payload+children으로 예상 ref를 식별하고 leaf → directory → wrapper의 준비된 순서로 queue에 넣는다. 원시 Draft backing은 마지막 사용 후 해제한다.
3. parent의 child는 같은 packet의 앞쪽 객체, 이전 packet에서 확정된 객체 또는 인증된 기존 객체여야 한다. 예상 ref는 DATA이며 설치·게시 권한이 아니다.
4. queue의 실제 객체 수 ≤256, aggregate wire ≤64MiB를 지킨다. 현재 Draft rows + queue payload/children + backend wire/registration/shard 준비 + 출력 slots의 합산 peak를 Original에서 승인한다. `put_batch_memory_bound`만으로 caller staging까지 승인됐다고 간주하지 않는다.
5. 다음 객체의 encode/append 전에 한도를 판단한다. queue가 있으면 검증된 child-first prefix를 설치하고 계속한다. 단일 객체와 필수 backend 준비도 승인할 수 없으면 ResourceLimited다. 효과 없는 사전 판단에서만 축소할 수 있다. 설치 시작 뒤 거절·I/O 오류를 받아 크기를 줄여 재시도하지 않는다.
6. 각 `put_batch` 반환의 길이·ordinal·객체 ref를 예상값과 대조한다. 모든 필요한 설치 확인이 끝난 뒤 기존 Top 편집과 최종 root 공개를 완료한다. 일부 객체가 provisional로 설치돼도 실패한 attempt의 published root는 바뀌지 않는다.

### NTX1a 독립 RED와 DoD

- NTX1a는 실제 Store+Catalog+복수 Draft caller fixture에서 한 packet에 들어가는 leaf+directory 입력을 고정한다. 기존 두 batch의 COMMIT 4회를 2회로 줄이고 같은 객체/root identity·조회·reopen·child edge/GC를 확인한다. 이 수치를 모든 페이지의 보장치로 확대하지 않는다.
- 256객체·encoded wire 경계, backend credit 미승인, 재사용 leaf와 복수 root에서 효과 전 admission/fallback을 확인한다. term마다 무조건 flush해 기존 공동 배치보다 transaction을 늘리는 구현은 채택하지 않는다.
- same-packet earlier child와 인증된 기존 child, foreign/missing child, late parent 거절·취소·short-write·sync/confirm 실패, 설치 receipt ordinal/ref 변조에서 published Top root 미변경·기존 view 조회·fresh retry/reopen 정산을 확인한다. 기존 cold 페이지의 stale epoch/late CAS/count/EOF 오류 회귀도 재사용한다.
- 객체 수·payload·등록 의미와 논리 Work 요금을 보존한다. 실제 제거한 호출·복사·검증과 Work 차이를 대응시킨다. 최종 Caddy SDK60 및 NTX4 통과 전에는 제품 성능 개선 완료로 닫지 않는다.

실제 Store+Catalog fixture는 Kernel의 [native_text_services](../../../../crates/quanta-index-kernel/tests/native_lifecycle.rs#L7403)와 COMMIT/custody 관측 회귀를 재사용한다. 제품 query/view/reopen은 Runtime의 [기존 Native Text SDK/UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds.rs#L2609)를 소비한다. mock authority의 호출 수를 실제 SQLite COMMIT 수로 보고하지 않는다.

### NTX1b 추가 DoD

- 256개를 넘는 입력, 실제 wire/Original 한계에서 prefix flush하고 same-packet earlier child와 previous-confirmed-packet child를 모두 검증한다. wrapper는 같은 queue에서 해당 child ref가 준비된 뒤에만 추가한다.
- 두 번째 packet 또는 후속 directory/wrapper의 reserve·write·sync/barrier·confirm 실패에서 published Top root 미변경과 attempt retry/reopen 정산을 확인한다. 페이지 전체의 late source 오류는 첫 설치 전에 거절한다. scratch Top 편집과 공개된 root를 구분한다.
- NTX1a의 4→2와 NTX1b의 다중 packet 완료를 별개로 기록한다. 첫 fixture 통과만으로 전체 streaming scope를 닫지 않는다.

**범위 제한:** 기존 [공개 child-first batch](../../../../crates/quanta-index-core/src/index/ports.rs#L193)와 [같은 배치 child 인증](../../../../crates/quanta-index-catalog/src/native/authority/managed_install.rs#L773)을 사용한다. Core/Storage/Catalog 계약 변경이 정말 필요하면 정확한 결손을 별도 인수한다. 전체 DAG 사전 보유, 페이지 간 무제한 보류, 숨은 재계산이나 두 번째 설치 IR은 기본안이 아니다.

## 4. NTX2A/NTX2B — 누적 Work와 용어 중심 생산자

**변경 함수:** [prepare_inventory](../../../../crates/quanta-index-lexical/src/native/text.rs#L10190), [ColdTokens](../../../../crates/quanta-index-lexical/src/native/text/cold_tokens.rs#L337), [기존 grouped output 소비자](../../../../crates/quanta-index-lexical/src/native/text/occurrence_pack.rs#L211). 기존 Text 생산자 writer가 맡고 NTX1과 `text.rs`의 서로 다른 함수 hunk를 구분한다.

### NTX2A — 제거할 실제 작업 확정

- 기존 gated 계측 안에서 census/normalize, fill/sort, block shape/pack, authenticated alias walk, posting push/flush의 배타적 Work와 token/term/position/출력 수를 기록한다. 새 검증 순회나 요금 감면을 계측에 넣지 않는다.
- Off/Record의 출력·원 Work·거절 동일성과 malformed/partial capture 거절을 먼저 확인한다. 상위 Text·prepare와 하위 lane을 중복 합산하지 않는다.
- 원본 전체 분포와 실패 prefix를 구분한다. 632개 owner 완료를 1,421파일로 선형 외삽하지 않는다. 증명 가능한 필수 하한과 추정치를 구분한다.
- `P`는 sensitive/folded를 포함한 case occurrence 수, `U`는 고유 canonical term 수다. `Kall`은 occurrence key bytes 합, `Kuniq`는 고유 key bytes 합이다. 현재 arena의 핵심 backing은 `Kall + 12P`다. caller/출력/SQL memory는 별도 peak에 포함한다.
- 같은 cohort의 owner별 `P/U/Kall/Kuniq`, one-block borrowed alias / multi-block direct / SQL fallback 분기, Original live peak와 spill 횟수를 함께 계측한다. aggregate prepare Work만으로 이 분포를 복원했다고 하지 않는다.
- F가 인계한 기존 제한 분석은 packet term dictionary의 한 순회 raw key 절감 최대 약 2.05MB와 실패 prefix 원 edit charge 41.74MB 이상이다. 이는 다른 후보/cohort의 packet 표현 분석이며 owner-local token pool의 절감량과 혼동하지 않는다. 이번 배타적 계측에서 관련 source·단위를 대조하고 필수 logical edit 요금은 유지한다.

### NTX2A typed counter 인수 계약

Root가 Core `DiagnosticCounterV1`의 아래 key 추가를 소유한다. Text는 집계·분기 판정·emit을 소유한다. 단위는 모두 `u64`이며 성공한 cold owner의 `NativeTextInventoryPrepare` 완료 span에 한 번 기록한다. warm owner는 제외한다.

| exact key | 의미 |
| --- | --- |
| `TextCaseOccurrences` | P: indexed lexical run의 Sensitive/Folded 각각을 세는 occurrence 수 |
| `TextUniqueCanonicalTerms` | U: case prefix를 포함한 canonical stored key의 owner 내 고유 개수 |
| `TextOccurrenceKeyBytes` | Kall: P개 encoded case-prefixed UTF-8 key 길이의 합 |
| `TextUniqueKeyBytes` | Kuniq: 각 고유 stored key 길이를 한 번 합한 값 |
| `TextColdAliasOwners` | 실제 완료 출력이 one-block borrowed alias인 owner 수 |
| `TextColdDirectOwners` | 실제 완료 출력이 arena ordered/multi-block direct인 owner 수 |
| `TextColdSqlOwners` | 실제 완료에 token SQL spool/group replay를 사용한 owner 수; arena 이후 SQL 재생도 이 분기 |
| `TextColdEmptyOwners` | indexed occurrence가 없어 출력이 빈 cold owner 수 |
| `TextShapeCompleteOwners` | 네 shape 값과 실제 분기가 모두 정확하게 관측·완료된 owner 수 |

Unicode 정규화/case folding으로 합쳐지는 중복은 P/Kall에 모두 포함하고 U/Kuniq에서 합친다. Sensitive/Folded는 prefix가 달라 별도 key다. 분기 네 개는 owner별 상호 배타적이다. 네 shape와 분기를 모두 확보하고 `prepare_inventory`가 성공했을 때만 completeness=1로 emit한다. 실패·초기 census 중단·미관측 값은 0으로 만들지 않고 tuple/completeness를 생략한다. aggregate는 completeness=1인 owner만 합산하며 분기 합과 complete 수를 대조한다. 이전 attempt·이전/현재 owner를 섞지 않는다.

Work 계측은 lane 34/35를 inclusive로 유지하고 42 canonical input/normalize, 43 census, 44 fill, 45 sort/validate, 46 block prepare/shape/pack/SQL replay, 47 token SQL spool/warm으로 분리한다. parent-ID는 Work 포함 관계의 증명이 아니다. 같은 WorkBudget·동기 호출·시간상 포함·비중첩과 모든 중간 counter 경계를 소스로 확인한 경우에만 child 차감을 허용한다. 이 검증이 없는 lane34·lane32의 direct residual은 NOT_REPORTED로 둔다. per-term span을 추가하지 않는다. Text-local grant 통계는 전체 Original live peak가 아니며 새 peak getter를 필수 API로 요구하지 않는다.

### NTX2B — 같은 타입 내부 생산자 대체

1. canonical term을 한 번 저장하는 key arena·term index와 term별 bounded position stream을 사용한다. 위치는 입력 순서로 직접 추가하고 고유 term만 canonical byte순으로 정렬한다. 전체 `{term_id,position}` 행을 다시 정렬하는 두 번째 IR을 만들지 않는다.
2. key arena·term slots/index·position nodes·정렬/emit buffer·SQL packet/string은 첫 run 전에 승인한다. 필수 occurrence 출력은 grouped shape를 확인한 뒤 첫 object/edit 효과 전에 기존 `cold_output_credit_bound` 경계에서 승인한다. 두 단계에서 동시에 살아 있는 grant를 합산한다. term마다 무제한 Vec를 만들지 않는다. hash 충돌은 전체 키 비교로 확인하고 hash iteration order를 출력에 사용하지 않는다.
3. position pool은 연결 정보와 metadata를 포함한다. `Kuniq + 4P`를 실제 메모리 상한으로 주장하지 않는다. `U≈P`인 all-unique 입력에서 slot/hash/node 비용이 기존 12P Row보다 커질 수 있다.
4. 실제 capacity가 차면 SQL 입장을 먼저 확인하고 `spool.reset_tokens`로 이전 owner 행을 비운다. 이미 집계한 모든 `(term,pos)`를 정확히 한 번 배출한 뒤 source remainder를 SQL에만 보내는 mode latch를 유지한다. 기존 `tokens(term,pos)` PK와 `ORDER BY term,pos`로 그룹을 완성한다. 첫 reset/insert 효과 뒤 원본 재생·생산자 재시작·경로 역전환은 금지한다. 중간 실패는 terminal/poison으로 처리한다.
5. canonical normalizer·case 생성·position·기존 wire·alias·seal·CAS·EOF/attempt 완료 계약을 유지한다. 기존 단일-block borrowed alias·occurrence encoder·SQL grouping·posting run merge를 재작성하지 않는다.
6. census는 fixed-grant 입장과 모든 크기/위치 검증이 대체될 때만 실제 순회와 함께 제거한다. 유지된 NFC/case encode·hash/비교·node write·spill·merge는 실재 작업대로 차지한다. 메모리 해제나 단순 interning으로 누적 Work를 환급하지 않는다.

### 독립 RED·채택 조건

- 기존 frozen gold와 canonical `(case,key,position)` stream을 비교한다. NFC·Unicode case 확장/충돌·긴 unindexable run의 위치 소비·u32 및 term별 position 경계·중복 위치 거절을 포함한다.
- 반복 용어, all-unique 식별자, chunk 경계에 걸친 같은 term, 다중 chunk, 최종 EOF, hash seed/삽입 배치 경계 변경에서 같은 canonical stream을 만든다. 동일 block/alias 조건에서는 기존 wire와 identity를 비교한다.
- 최초 메모리 미승인, spill/SQL 실패, 취소·후속 owner 변조에서 부분 root 공개 0과 fresh retry 동등성을 확인한다. encoded DATA의 실제 소유권과 persisted alias의 독립 readback을 유지한다.
- NTX2A와 같은 입력에서 key copy·정렬·hash/node·SQL 비용을 포함한 **net Work·peak memory·시간**을 비교한다. RAM 절감만으로 512MiB 통과를 주장하지 않는다. 비용 역전이나 원 게이트 미달이면 후보 실패로 기록하고 채택하지 않는다.
- 다중-block borrowed view는 별도 후속 후보다. NTX2B와 동시에 변경해 개선량을 섞지 않는다. 필수 하한이 전체 Work를 넘는 것으로 입증되면 해당 생산 경계의 별도 재설계를 요청하고 임의 cutoff·요금 삭제를 반복하지 않는다.

## 5. NTX3 — 여러 packet의 prepared run은 후속 작업

NTX1의 실제 SDK 결과에서 packet 간 confirm/reserve가 남은 병목으로 확인될 때만 연다. [내부 다중 그룹 run](../../../../crates/quanta-index-storage/src/object.rs#L609)과 원자적 confirm+next-reserve를 재사용한다. 현재 [공개 prepared 계약](../../../../crates/quanta-index-core/src/index/object_install.rs)은 childless 전용이므로 child-bearing 지원은 별도 typed 계약·writer 인수가 필요한 실제 작업이다.

기본 NTX1에 새 공개 port를 강제하지 않는다. prepared-run의 COMMIT 4→3과 NTX1의 단일 packet fixture 4→2는 다른 실험이며, 어느 것도 전체 SDK 개선량이 아니다. 여러 group의 동시 Original 준비가 승인되지 않으면 적용하지 않는다. 파일 I/O를 SQL transaction 안으로 옮기지 않는다.

## 6. 실행 순서·단일 작성자·인계

이 절은 NTX 구현과 영수증의 인계 기록이다. 새 포맷의 작업 배정은 상단 [코드 작업 배정과 착수 순서](#코드-작업-배정과-착수-순서)를 따른다.

| 순서 | 독립 진행과 합류 |
| --- | --- |
| 0 | F가 NTX0 source/binary·입력 기준선을 고정한다. Text는 NTX1a의 oracle/소스 검토와 NTX2A 계측 설계를 독립 진행할 수 있다. |
| 1 | NTX1a 현재 창 batch 통합과 NTX2A 생산자 계측은 서로 다른 함수 hunk에서 병렬 진행한다. 같은 `text.rs` parent/hunk는 기존 원 writer 한 명이 합친다. |
| 2 | NTX1a의 실제 behavioral RED→구현→owning 회귀는 재사용한다. NTX2B를 채택하면 NTX2A의 입장/비용 근거→gold/adverse RED→생산자 대체→owning 회귀로 진행한다. NTX1b 확대는 최초 결과와 후속 hunk 인수 뒤다. 미실행 selector를 RED/GREEN으로 부르지 않는다. |
| 3 | 각 source-ready마다 F가 변경된 범위만 기존 warm lane/실행권에서 검증하고 같은 후보 SDK에 인수한다. 두 변경의 효과를 구분할 기준선을 보존한다. |
| 4 | 채택한 구현과 기능 오너의 결과를 같은 최종 후보에 합쳐 NTX4를 실행한다. NTX3는 선행 근거가 생긴 경우만 별도 인수한다. |

새 채팅·writer·검증 큐를 만들지 않는다. 기존 작성권을 파일 전체로 확대하지 않으며, 착수 시 HEAD·dirty·활성 hunk와 source-ready 결속을 다시 대조한다. 인계에는 함수/타입·생산자/호출자/소비자·관련 source SHA·정확한 selector·명령/결과·scope/남은 입력을 기록한다. F는 공용 조립·원장·최종 후보와 실행 큐를 담당한다.

**당시 인수:** 이 Text 채팅이 NTX1a의 `dense_posting::write::install`과 전용 회귀를 단독 작성하고, etcd 비용 계측 뒤 필요한 `ColdTokens` hunk를 인수하는 순서였다. D의 명시 반환 후 Root가 기존 `test-workspace-lane`/`cargow`/mutex 실행권을 Text에 대여했다. 당시 환경은 jobs2·resource wait10과 Root/D Rust0·Text executor1에서 owning→release→원SDK를 직렬 실행했다. 테스트·source-ready·실제 selector와 결과는 아래 표에 보존한다.

### proof_requests / proof_results

| 요청 | 실제 source/selector | 상태 | 결과 인수 |
| --- | --- | --- | --- |
| NTX1a-R1 | 기존 `dense_install_batches::native_text_dense_window_leaf_directory_share_two_catalog_commits_and_reopen` — Kernel `native_lifecycle` | RED VERIFIED — run `497063b3`, 4 != 2; 아래 fallback 회귀로 분리 | F canonical Rust 큐 |
| NTX1a-F1 | `dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens` — Kernel `native_lifecycle` | VERIFIED — run `9cd669fc`, actual PASS0.670s; source `a0443fd5` | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX1a-G1 | `native::dense_install_tests::dense_text_real_original_credit_coalesces_and_reopens` — Runtime lib/private fixture | VERIFIED — run `8ee85ee8`, actual PASS 2.420s; 2 vs 4 COMMIT·artifact/TF/positions/reopen | Root가 인수한 D 단일 실행권 |
| NTX1a-F2 | `native::dense_install_tests::dense_text_post_confirm_error_preserves_current_and_resumes_original_operation` | VERIFIED — 같은 run actual PASS 1.380s; Unknown·old current·resume 원 작업·9owner/reopen | Root가 인수한 D 단일 실행권 |
| NTX1a-A1 | `native::dense_install_tests::dense_text_real_original_credit_miss_falls_back_without_denial` | VERIFIED — 같은 run actual PASS 2.222s; 실제 8MiB credit miss·fallback·first denial 없음 | Root가 인수한 D 단일 실행권 |
| NTX1a-B1 | Kernel `physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically` + Storage `object::tests::batch_barrier_failure_leaves_reserved_objects_unconfirmed` / `object::managed_trace_tests::managed_reserve_confirm_trace_preserves_off_cost_and_closes_error` | VERIFIED — Storage run `a72d53a5` actual2/2PASS·208skip·exit0·0.437s; Kernel run `9cd669fc-d775-41bc-8ff1-4a2951e390e5` actual2/2PASS·83skip·exit0·0.853s includes F1 | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX1a-R2 | `native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold` — Runtime `native_index_uds` 기존 rank/view/reopen oracle | VERIFIED — run `6741d8d5-3b8d-4c9f-afe6-7b903144a703`, actual1/1PASS·74skip·exit0·3.067s; compile1m19s | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX2A-R1 | `native::text::tests::inventory_trace_tests::inventory_trace_stages_preserve_work_and_close_under_lexical_parent` | FAILED — `01cb221f` 1 body/FAIL 1; cold 미선택 oracle 오류로 제품 RED 판정 철회 | F canonical Rust 큐 |
| NTX2A-WG | `native::text::tests::inventory_trace_tests::` — existing stage + 실제 cold success + first denial 동등성 3건 | VERIFIED — nextest `54c7b404-8a46-4cae-9818-99148f0e6873`, 실제 Work 3 PASS | F canonical Rust 큐 |
| NTX2A-SR | `native::text::tests::inventory_shape_tests::` — SQL case-key gold + empty 2건 | RED VERIFIED — 같은 run에서 shape 2 FAIL; empty None != Some(0), SQL None != Some(6) | F canonical Rust 큐 |
| NTX2A-G7 | `inventory_trace_tests::` 3 + `inventory_shape_tests::` 4 | VERIFIED — nextest `0a7b720b-190a-441c-a3b1-bbefd2e7032c`, 7/7 PASS·529 skip·exit0·0.126s; shape test `75ac4cae` | F canonical Rust 큐 |
| NTX2A-PY | diagnostic/timeline 소비자 2파일·회귀 2파일 | VERIFIED — locked uv pytest 621 PASS, Ruff/diff-check PASS | Text 독립 Python 실행 |
| NTX-owning | `native::text::` / `native::text_spool::` / `native::literal` — Lexical lib no-default-features | VERIFIED — run `69eabb3a-471a-41b1-9930-6ce3e75d0aa0`, actual367/367PASS·169skip·exit0·14.532s; compile38.09s | Root가 명시 loan한 기존 단일 Q 큐 |

요청 표의 자리표시자는 실행 가능한 selector가 아니다. 본문이 없는 요청을 제출·통과 처리하지 않는다. 실행 결과에는 명령·source·본문 수·실제 실패/통과와 source-ready 후 변경 여부를 기록한다.

**NTX1a-R1 최초 RED source (이력):** [새 회귀](../../../../crates/quanta-index-kernel/tests/native_lifecycle/dense_install_batches.rs)의 SHA-256은 `1bc2aaf7c5227dcf482572444b26c6a863d9ee4b6e49b2fd30b880deb64c4748`다. Kernel `native_lifecycle.rs`는 전용 module mount, `UpdateCostSpyState`의 관측 sample, 실제 `install_batch` 전후 opt-in SQL profile hook만 추가했다. 세 term×9 owners의 dense 복수 draft에서 실제 SQLite COMMIT 합 2회·한 packet을 요구하고 양 case의 owner/TF/positions·동일 root의 reopen을 독립 고정값으로 검증한다. 첫 Root 실행은 컴파일 중 exit 137로 끝났으며 본문 0건으로 행동 RED가 아니다. Root 재실행 nextest `497063b3-0ce8-4439-bc2b-12e534158803`은 본문 1/FAIL 1/exit 100, 84 skip이었다. 관측값은 leaf 6개 batch COMMIT 2 + directory 6개 batch COMMIT 2 = 4, 기대값 2로 실패했다. 앞선 owner/TF/position과 reopen 단언은 통과했다. 이 행동 RED 뒤 production 구현을 시작했다.

**fixture 보완:** 위 Kernel fixture의 `open_managed_with_scratch`와 미연결 `UpdateCostSpy` bound는 `None`이며 WorkBudget도 scoped Original credit이 없다. strict admission 경로의 GREEN 증명으로 쓸 수 없다. 해당 fixture는 미승인 fallback의 4 COMMIT 회귀로 명시적으로 분리하고, 별도 Runtime private fixture에서 production `open_managed_with_io` + 실행 reservation의 Original credit을 재사용해 2 COMMIT을 검증한다. 가짜 backend bound나 승인 우회로 기대값을 맞추지 않는다. 첫 통합 구현은 prior 없는 cold whole window만 선택하며 warm/local은 기존 공동 leaf batch 경로를 유지한다.

**현재 production/source-ready:** `dense_posting/write.rs` SHA `13045e24358df58c6a20c1d963b2f26737e921db214ce827b4e5ea50740dca47`, `wire.rs` SHA `9c27d9b3ce28e5523917a5fff55cda6ac230f971834f0ec885fede222fd30278`. 공통 batch backing은 Original parent에서 먼저 승인해 양 경로에서 유지하고, cold whole window의 payload/children/임시 fence·ref/identity·backend 준비를 원 scope credit으로 합산 승인한다. 모든 설치 receipt 검증 뒤 draft ref/output을 확정한다. 정적 적대 검토·rustfmt·diff-check와 아래 Runtime 3건이 VERIFIED다. 원본 SDK 수용은 NOT_RUN이다.

**NTX1a-G1:** [실제 Runtime private fixture](../../../../crates/quanta-index-searchd-runtime/src/native/dense_install_tests.rs)는 `ManagedObjectIo`/`PublicationFileCapacity`/`NativeResourceLedger`의 실제 Core reservation을 쓴다. 요청당 64MiB IO·memory scope에서 실제 Catalog SQL profiler로 fast 1batch/2COMMIT 대 bound-declined fallback 2batch/4COMMIT, fixed owner/TF/positions·reopen·동일 artifact를 검증했다. run `8ee85ee8` actual PASS이며 SDK60 제품 수용과 별개다.

**Runtime 후속 입장·실패 회귀:** 현재 private fixture SHA는 `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`다. 실제 combined install의 confirm/2 COMMIT 뒤 one-shot Storage 오류를 주입해 Kernel의 `Unknown`, dense 1call·공개 current 불변·빈 조회·reopen을 확인하고 canonical `resume(op2)`로 원 immutable request의 full publish·9owner membership·다시 reopen을 검증했다. 별도 8MiB Core Original 상한에서는 그대로 forward한 backend bound가 16MiB 이상이므로 optional coalesced credit이 거절된다. 원래 mandatory 경로가 2batch/4COMMIT으로 성공하고 first denial 없이 64MiB fast와 artifact·조회·reopen이 같다. 세 본문은 run `8ee85ee8` VERIFIED다. 이전 fresh-op3 fixture 실패 원인은 아래 이력에 보존한다.

**NTX2A Work source-ready:** `text.rs` shared SHA `2d3b5a90f142d2962db7de4c381fa3a98fef59d36e1bc4b4167e2217a027c24a`, `tests/inventory_trace_tests.rs` SHA `e6b39c6314b5298537885807ff1687ce89442b80878e36bf56d39754d146bcbd`. 기존 `inventory_trace_stages_preserve_work_and_close_under_lexical_parent`와 신규 `inventory_trace_work_refusal_preserves_first_denied_charge`를 Root GREEN 요청했다. 후자는 Off/Record의 첫 Bytes 거절·accepted Work·used/attempted/limit/caller를 비교한다. formatter·diff-check 이후 Root 본문은 2건 모두 실패했다. nextest `04bbd234-4bb6-41b9-ae7b-6a9b3a16a0e0`에서 stage test는 census 0 != 1, refusal test는 lane43 시작 없음으로 실패했다. helper `process_change`는 `pack_owners=false`여서 census 경로를 선택하지 않는다. 이 oracle 오류 때문에 최초 `01cb221f` 실패를 생산 코드 결함의 RED로 확정한 판정을 철회한다. 기존 noncold oracle를 복원하고 실제 `prepare_inventory(pack_owners=true, empty prior, MemoryScope)` helper를 추가했다. 현재 test SHA `10f4445ba64ffddf5327123044a2b7fb91208bdf128230b91eadcc5782ae2c05`의 새 `inventory_cold_prepare_work_lanes_preserve_off_record`와 원 stage·거절 회귀까지 3건을 Root에 재요청했다. census 순회 추가 없이 test 경계만 수정했고 재실행 본문 NOT_RUN이다.

**NTX2A shape test-only (RED 이력):** [고정 SQL/empty gold](../../../../crates/quanta-index-lexical/src/native/text/tests/inventory_shape_tests.rs)는 실제 `prepare_inventory` cold 분기에서 `Alpha alpha β`의 P6/U5/Kall30/Kuniq24·SQL1/complete1과 빈 owner의 정확한 0·empty1/complete1을 요구한다. Off/Record 결과·Work를 비교하며 RED 실행 전에는 Text emit이 없었다. `text.rs`에는 module mount만 추가해 당시 shared SHA는 `21ecc09f270db2ed6601fca9fa8f6931d576819aedb1ce479c5525c9253c2c4e`였다. 이전 Work source-ready SHA와 구분한다. Root에 실제 RED를 요청했고, 그 결과 전 emit production은 수정하지 않는다.

**재실행 전 compiler 수리:** Work3+shape2의 Root 재검증은 test의 `BTreeMap::insert` 반환값 미사용 5개로 compiler exit101·본문0이었다. 행동 RED/GREEN으로 세지 않는다. 삽입마다 중복 없음 단언으로 반환을 소비하고 formatter/diff-check를 통과했다. 현재 Work test SHA는 `54ead1d1d996ae6f7941f24aa5432a765f6dbf297450da3dc6f7b32dffdd4ab9`이며 같은 5scope를 다시 요청했다. Runtime positive는 Root의 Kernel typed History dispatch 접합이 끝나기 전 NOT_RUN이다.

**실제 재실행 결과:** nextest `54c7b404-8a46-4cae-9818-99148f0e6873`은 5 body / 3 PASS / 2 FAIL / 529 skip / exit100, compile19.61s·본문0.086s였다. 수정한 실제 cold Work 3건은 모두 통과했다. shape 2건은 emit 미구현의 counter 부재로 실패했으며 이것을 유효한 구현 전 RED로 인수했다. 이 결과 뒤에만 emit production을 수정한다. borrowed one-block Alias, admitted ordered/multi-block Direct, SQL은 성공한 실제 return site에서 구분한다. 출력의 `alias: Some`만으로 생산 경로를 추론하지 않는다.

**emit production/source-ready:** 기존 borrowed alias seal loop의 O(1) `vector_len`과 기존 grouped consume에서 P/U/Kall/Kuniq를 집계한다. 추가 토큰/행 순회나 Work 요금 변경은 없다. 성공한 실제 return site의 Alias/Direct/SQL/Empty를 붙이고 성공한 prepare 뒤 lane48에서 네 값·선택 분기 1·complete 1만 발행한다. tentative alias 실패 시 관측값을 버리고 실제 fallback 값으로 완성한다. overflow/trace 오류는 제품 결과를 바꾸지 않는다. `prepare_owner_blocks` 한 entrypoint에 observer 인자를 추가하고 생산 caller·기존 네 test caller를 함께 변경했다. 현재 SHA는 `text.rs` `ee2c6944ff0c50b657011373e688512089880cbc83cdc03bc126a689b15161e6`, `occurrence_pack.rs` `fb2eba49883cc34f31331634f87ccab557d16ee7c8b34b8db39d808562769634`다. formatter/diff-check 통과, 최초 5scope 뒤 분기 회귀 2개를 더한 현재 7scope는 아래 Root 실제 실행에서 모두 통과했다.

**추가 분기 회귀 source-ready:** 실제 SQL/empty gold 외 `cold_borrowed_alias_shape_matches_fixed_group_stream`와 `cold_sql_inline_alias_reports_sql_branch_and_fixed_group_stream`를 추가했다. persisted 0/1 prefix·UTF-8의 literal 11-key/position gold와 실제 `CheckedBlock` 출력을 대조하고 P12/U11/Kall70/Kuniq64·Off/Record Work·SQL table 유무를 검증한다. SQL은 기존 canonical arena·alias metadata·first key·encoded output minus one 공식으로 선택하며 threshold 탐색을 하지 않는다. Direct가 Directory alias를 반환하는 경우는 실제 backend bound·Original credit을 제공하는 Runtime fixture/SDK Record로 확인한다. 기존 `install_envelope.rs`의 514-key·credit1 fixture는 backend bound가 없어 SQL replay를 확인하는 회귀다. 이것을 Direct 양성으로 승격하거나 fake zero-bound provider로 제품 입장 증거를 만들지 않는다. Alias·SQL·Empty 분기는 Root 7scope에서 통과했다. Direct 분기별 계측 실행은 아직 NOT_RUN이다.

**계측 소비자 인수·VERIFIED:** Root가 인계한 Text 전용 consumer hunk에서 raw lane48/성공 stage/v5/부분 four-tuple/분기 합/completeness/u64 overflow·완료된 empty의 0·비어 있지 않은 owner의 0U 모순을 거절한다. aggregate/cached group도 검증한다. 완료된 v5 lane48에는 tuple이 반드시 있어야 하고 complete 수는 성공한 owner-metric span 수와 같아야 한다. P의 짝수·비어 있지 않은 N개 owner의 P/U 최소 2N·Kall 최소 2P·Kuniq 최소 2U를 canonical 두 case 생성 계약에 대조한다. N은 Record span 수이며 Source 파일 수의 인증으로 승격하지 않는다. `causal_cost_timeline.py`는 Root의 명시 9-key set에 대해 같은 span의 raw duplicate를 거절하며 기존 generic Work 누적과 occurrence-diff 동작을 유지한다. 실제 TDD 실패를 관측한 뒤 `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_causal_cost_diagnostic.py tools/ci/tests/test_causal_cost_timeline.py`를 실행해 **621 passed / 5.80s / exit 0**를 확인했다. 4파일 Ruff check·diff-check는 통과했다. 전체 format-check에는 작업 밖 기존 dirty hunk 차이가 남아 해당 hunk를 변경하지 않았다. 이 결과는 parser 회귀이며 실제 SDK 분포·게시·성능 증명이 아니다. 전체 Original peak API도 새 필수 입력으로 요구하지 않는다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel --test native_lifecycle --no-tests fail \
  -E 'test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens)' \
  --test-threads 1 --failure-output immediate --success-output final
```

공통 영향 회귀의 기존 명령은 다음과 같다. 실제 실행은 F의 현재 canonical queue에서 변경 scope를 확인한 뒤 인수한다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-lexical --lib --no-default-features --no-tests fail \
  --no-fail-fast --status-level fail --final-status-level fail \
  -E 'test(native::text::) | test(native::text_spool::) | test(native::literal)' \
  --test-threads 1
```

새 fixture의 정확한 selector는 해당 테스트를 실제 추가한 writer가 source-ready에 적는다. 존재하지 않는 테스트 이름이나 컴파일 실패를 행동 실패로 발급하지 않는다.

## 7. NTX4 — 제품 수용과 종료 조건

- 같은 source/binary/입력의 Caddy·etcd 각각 quiet Off 2회+Record 1회에서 full ACK·검색·재오픈·source digest 일치를 확인한다. 파일 30,000·Work 536,870,912B·읽기/쓰기/변경 각 1,000,000·SDK I/O 및 submit→ACK 60초를 유지한다.
- Record는 두 epoch의 실제 publish/query/reopen과 완전성 검증을 통과해야 한다. drop/unwritten/open request·span, partial/malformed 결과를 성공으로 취급하지 않는다. quiet 사전 검사 실패는 성능 성공이나 제품 실패로 바꾸지 않는다.
- same-view 조회·이전 root·delta/delete/no-op·실패 후 retry, 표준 Large의 두 OS 재시작과 해당 영향 회귀를 검증한다. 설치기 변경은 객체/root identity, 생산자 변경은 canonical groups·wire·조회 의미를 각각 독립 대조한다.
- Work·RSS·scratch·물리 파일/I/O·COMMIT·xSync를 분리한다. 중첩된 stage/counter는 합산하지 않는다. 동등 기능의 baseline/candidate 비교와 최종 출하 기준은 기존 DOD를 그대로 적용한다.
- 모든 필수 범위에 명령·관측 결과·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE과 scope를 남긴다. 문서·컴파일·집중 회귀만으로 SDK60·전체 Native·SOTA 성능·설치 출하를 완료 처리하지 않는다.

### 현재 원 SDK 진단과 다음 수리 경계

후보 `/private/tmp/qidx-sdk60-ntx.zOObrM`은 daemon `761bc65cd3e9ad926592adb2208ca5143ab19f59a1569792546199a4c55ed972`, 기존 `native_corpus` 재빌드 `71d747aa25d650929753c5fe1ee6568cca8eff2908e8378c70968ce0b30b5783`이다. 빌드·Record 전후 2,785개 입력과 HEAD가 동일했다. 첫 Off는 host load60.330078>8에서 본문0으로 BLOCKED였다. 같은 원 caps의 Record는 Caddy46.27s·etcd19.15s에 각각 ResourceLimited, ACK0·query/reopen 미도달로 FAILED다. 두 raw trace는 schema5·complete/normal, drop/unwritten/open-span/producer-lease0·health[]·open request0이다. 이 실패 후보의 Off/Large를 반복하지 않는다.

| 원본 | 최초 거절 | 실제 완료 metric prefix |
| --- | --- | --- |
| Caddy590 | reads999984+23>1000000, managed_install.rs108; bytes479122433·W547659·M687065·memory denial 없음 | 589 valid tuples: P799870/U207909/Kall5081067/Kuniq1588399; Alias384/Direct204/SQL0/Empty1 |
| etcd1421 | bytes536870674+1461>536870912, object.rs1914; R480232·W452360·M626787·memory denial 없음 | 704 valid tuples: P1004394/U335766/Kall8120086/Kuniq4213880; Alias516/Direct188/SQL0/Empty0 |

Metric 수는 관측된 성공 cold owner이며 전체 파일 수의 수용 증명이 아니다. etcd lane32의 배타 Work는 실제 parent에 포함된 lane33만 뺀 **37604852B**다. global33을 빼 얻은30975194B는 폐기한다. Text297685874B 외에는 Literal186751136·RepoMap32020144·SourceApply15182451 등이 포함된다. 부모·자식은 중복 합산하지 않는다. SQL spill0과 key backing 차이3906206B만으로 NTX2B나 전체512MiB 통과를 채택·예측하지 않는다.

**Caddy 구현 — bounded shared Small2..8 posting block (Root exact loan 후 구현, 제품 수용 NOT_RUN):** wrapper 객체는 현재도 페이지 단위 `put_batch`와 paired/confirmed dedup을 사용한다. 추가 배치 연결이 아니라 term별 객체 수를 줄이는 새 persisted codec/reader 경계다. 기존 public object/map API와 `Posting`/`SmallMap` 메모리 모델은 재사용한다. `entry_map::block`의 bounded child-slot 검증은 참고하되 `(term,owner)` 결합 키는1024B 한도를 넘을 수 있으므로 wire를 그대로 쓰지 않는다.

- Writer는 기존 Text 원 writer 한 명. private `small_posting_block` codec와 `text.rs::apply_postings`의 wrapper 준비·Top 편집, `read_posting` 및 네 production caller를 exact hunk로 인수한다. Catalog/Storage/Core/공개 port·quota는 수정하지 않는다.
- MapValue의 기존32B DATA/5child 한도를 유지하고, 새 inline tag+slot+DF+child1이 shared object를 참조한다. block에는 각 slot의 **전체 canonical term**과 DF2..8·정렬된 owner32→occurrence child-slot을 저장한다. 모든 child ref·slot/order/EOF·DF와 요청 term 일치를 검증한다. 같은 DF인 foreign term/slot 바꿔치기도 거절해야 한다.
- 기존 bounded block의16KiB/512행 정책을 재사용할지 codec 설계에서 고정한다. 실제 encoded payload·child dictionary·staging/decoder backing을 Original에서 효과 전에 승인한다. flush와 format 선택은 고정 recipe/실제 wire shape로 결정하고64MiB/8MiB 가용 메모리에 따라 다른 artifact를 생성하지 않는다. 부족하면 typed refusal이며 효과 뒤 representation 변경·재시도는 없다.
- 새 wire는 artifact digest가 달라지는 실제 format 확장이다. old-reader 수용이나 mixed-version boot를 주장하지 않는다. 기존 persisted Object posting의 read/identity/edge 검증은 보존하고 새 write는 Small2..8의 한 canonical codec을 사용한다. 병행 내부 IR·v2 wrapper·새 object authority는 만들지 않는다.
- 실제 Original Runtime fixture에서 여러 term의 DF2/3/8·독립 owner/TF/positions·BM25 gold와 wrapper object 감소를 먼저 RED로 실행한다. codec 독립 bytes gold, max term1024B, boundary/foreign slot/DF/child·truncation, delta/delete 후 다른 term 생존·old view/reopen/GC, 첫 memory/Work 거절과 post-confirm 복구를 검증한다. 이후 owning·새 binary·원SDK를 다시 결속한다. 객체 감소를 현재 quota PASS로 예측하지 않는다.

**etcd 경계는 추가 확인:** 현재 owned-wire payload view는 없으므로 단순 연결 작업이 아니다. 또한 `FileObjectStore::read_retained`도 IO·payload copy 없이 read/wire/decode Work를 차지한다. 따라서 payload copy만 없애1914 fee를 삭제하는 것은 원 논리 Work 계약의 수리가 아니다. 기존 attempt-local reader에서 중복 **논리 get/decode 호출 전체**를 제거할 수 있는지, 또는 canonical 표현 크기·source 준비 흐름의 구조 변경이 필요한지 Root가 exact producer/consumer 범위로 인수한 뒤 결정한다. checksum·등록·독립 seal/CAS/count/EOF 검증과 PostingRuns의 원 logical edit fee는 유지한다.

### Shared Small 이전 후보 실행과 etcd 후속 결정

- Shared Small의 실제 구현 전 Runtime RED는 4 FAIL(`289bece4`): 원 DF/TF/positions/reopen 확인 뒤 객체 3→1 및 새 policy/refusal 조건에서 실패했다. 구현 뒤 cold Runtime4는 `1abc926d` 4 PASS다.
- 독립 codec4는 `f9a97cad` 4 PASS. wire gold·두 case 슬롯·foreign term/slot/DF/ref·전체 truncation·dictionary/order/EOF·1024B term·16KiB 고정 경계를 확인했다.
- 이전 Shared Small 후보 Lexical/TextSpool/Literal은 `57e1a1cc-8509-4eaf-8f15-fa6a57904c28`, **372/372 PASS /169 skip /exit0**, 본문22.304s다. 새 Original 거절/Work 거절은 backing 해제·첫 denial·원 accepted Work를 확인한다. 이전 JSON wrapper 전제 두 assertion은 full term별 distinct inline slot·같은 block child·독립 membership gold로 교체했다.
- 이전 Shared Small 후보 Runtime owning은 `d90dad09-249e-492b-b14c-7acae01adc89`, **11/11 PASS /639 skip /exit0**, compile5m57s·본문19.117s다. Small cold/policy4·gamma-only delete→alpha/beta 기존 block 유지·old view/reopen·post-confirm Unknown→same-op resume와 기존 dense3 및 profiler helper2를 포함한다. 최초 컴파일은 unused ObjectRef 결과 2곳으로 exit101·본문0이었으며 초기 artifact와 명시 대조하도록 고쳤다.
- Kernel DF9→8→9·실제 GC/reopen 보강 owning은 기존 Q mutex 입장10초 만료/exit124·본문0으로 **BLOCKED**다. 전용 실행 레인·우회 lock·새 runner를 만들지 않았다. SDK BM25 새 policy 영향 검증은 NOT_RUN이다.
현재 실제 실행 명령(공통 prefix: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 ./scripts/cargow --lane test-workspace-lane`, 공통 nextest flags: `--locked --offline --no-tests fail --no-fail-fast --status-level fail --final-status-level fail --test-threads 1`):

| 범위 | nextest 선택 | 결과 |
| --- | --- | --- |
| Lexical | `-p quanta-index-lexical --lib --no-default-features -E 'test(native::text::) \| test(native::text_spool::) \| test(native::literal)'` | VERIFIED372/372 |
| Runtime | `-p quanta-index-searchd-runtime --lib --no-default-features -E 'test(native::dense_install_tests::)'` | VERIFIED11/11 |
| Kernel | `-p quanta-index-kernel --test native_lifecycle -E 'test(text_entry_maps::) \| test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) \| test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically)'` | BLOCKED입장 만료·본문0 |

- Shared block은 각 term 조회에서 bounded 전체 block을 검증하므로 read amplification이 생길 수 있다. 원 SDK와 delta/query 측정 전 term별 Work 감소나 종합 성능 우위를 주장하지 않는다.

etcd의 post-map child readback은 독립 검증이므로 RAM에 보관한 `CheckedBlock`을 fresh authentication으로 바꿔치기하는 안은 채택하지 않는다. 현재 256-owner map apply→diff 경계까지 frame을 보유하는 것은 Original peak도 늘린다. 읽기 전용 검토에서 남은 후보는 다음처럼 구분했다.

| 수리 | 변경 전 코드 | 구현과 검증 경계 |
| --- | --- | --- |
| 같은 Producer call의 leaf 중복 get | `directory::Producer::next_edit`가 get한 뒤 leaf count helper가 즉시 같은 ref를 다시 get | 이미 읽은 StoredObject를 같은 call의 count/parse에 전달하는 국소 수리. post-map fresh get 유지. 실제 leaf 건수·순절감 미측정. |
| Directory descriptor 생산→소비 | occurrence pack은 fence/kind/terms/ref를 descriptor seal에 반영하지만 scratch에는 first key/ref만 남김 | 기존 scratch 행의 canonical descriptor DATA 생산→소비를 구현했다. CAS/order/count/EOF·독립 source key/descriptor seal·post-map persisted get을 보존하고 첫 Producer get을 제거한다. 집중6 통과, 전체 owning 및 원 SDK는 후속이다. |

후자는 단순 연결이나 기존 기능 완료가 아니다. malformed child 발견이 map apply 중에서 post-map diff로 늦어지는 경우 provisional map 객체가 더 생길 수 있으며, 어떤 owner 완료나 published head 변경도 허용하지 않는다. 메타데이터 encode/SQL/read의 실제 Work까지 순비용으로 비교한다. 원본 etcd 전체 ACK/검색/재오픈 전에는 bytes 문제 해결로 닫지 않는다.

현재 가장 좁은 후속 구현안은 새 SQL schema나 frame cache 없이 **DirectoryAlias planned 행 한 개를 canonical descriptor DATA로 변경**하는 것이다.

1. `occurrence_pack::stage_sealed_alias/stage_leaf_alias`가 이미 구한 fence/kind/terms를 사용한다. DirectoryAlias만 `k=fence, p=None, n=Inline(kind:u8,terms:u16,child1)`를 기록한다. 일반 inventory/one-block Inline alias의 Object 경로는 각각의 의미에 맞게 유지한다.
2. `InventoryEdits`·`TextSpool.inventory_batch`는 그 exact Directory descriptor 값만 허용하고 receipt의 occurrence ref는 descriptor child에서 추출한다. generic Inline 허용·receipt 검증 생략은 금지다. SQL row count/poison/read lease/EOF 계약을 유지한다.
3. `directory::Producer`의 anchored branch는 descriptor DATA로 map edit를 만들고 첫 child get/parse를 제거한다. CAS/order/count/EOF와 ordinal 검증을 유지한다. alias root 전용 `entry_patch::MapInput`·`Edits` stamp가 같은 descriptor transcript를 검증하며 다른 root에 descriptor Inline을 허용하지 않는다.
4. `visit_cold_directory`·post-map Cursor의 persisted get/child parse·독립 key/descriptor seal·count/EOF는 유지한다. 새 내부 표현을 수용한 생산·소비 경계를 함께 변경하며 병렬 IR/업캐스터를 만들지 않는다. persisted directory value 자체는 기존 descriptor format과 같다.
5. 독립 RED는 고정 packed2/leaf fixture에서 builder child get0·post-map get1, canonical root/term/position 동일성, foreign kind/count/fence/ref/CAS/EOF 및 post-map child 손상에서 owner completion0·head 불변을 요구한다. generic/nonalias 경로와 Off/Record·첫 거절도 회귀시킨다. packed anchor당 제거된 실제 get은 read1 및 `(16 + 2×payloadLen)B`; 새 SQL/stamp 비용을 차감한 순절감은 실측한다.

이 5경계는 현재 사용자의 `$ss` 명시 승인으로 구현했다. 첫 제안의 `CheckedBlock` 보유·post-map fresh get 대체는 사용하지 않았다. 새 공개 port·SQL schema·persisted directory format 변경은 없다.

### Directory DATA 현재 실행

- source descriptor는 packed block의 마지막 funded key를 move하고 정확한 `[kind,terms:BE16]+child1`을 전달한다. physical `identified`와 logical descriptor child를 비교한 뒤 Planned 행을 쓰며, 설치 receipt 전건 확인 뒤 기존 confirmation만 사용한다. 이미 확인된 Ready 경로도 같은 값을 전달한다.
- `Edits::push_descriptor`는 complete encoded MapValue의 tag/len/data/child와 wire 길이를 original root transcript에 포함한다. 같은 child의 kind/count 변경도 stamp가 달라진다. 일반 root는 Object-only이며 descriptor Inline은 aliased Directory root에서만 허용한다.
- post-map Cursor/visitor는 변경하지 않았다. 실제 child get/parse·독립 seal·token total 검증 뒤에만 owner completion이 가능하다. generic leaf producer는 같은 call에서 이미 읽은 StoredObject를 `stored_leaf_count`로 검증해 즉시 둘째 get을 제거했다.
- 최초 builder get0 behavioral RED는 Q 입장10초 만료·exit124·본문0으로 **BLOCKED**여서 행동 RED라고 부르지 않는다. 구조상 기존 builder가 직접 get한다는 source와 보존한 회귀로 구현했으며, 첫 컴파일의 Result `?`/caller 타입 및 test key move 오류는 각각 수리했다. compiler101·본문0을 행동 실패로 세지 않는다.
- `032a817e-9b74-4329-8b50-1a16ce1b1f82`: **6 PASS /538 skip /exit0**, compile26.79s·본문0.104s. Spool descriptor Ready/Planned+foreign receipt/shape2, builder get0·post-map packed get1, independent fixed-wire/hash 및 same-child kind/count stamp1, 기존 count-only authentication/Original refusal2다.
- 명령: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=60 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-lexical --lib --no-default-features --no-tests fail --no-fail-fast --status-level fail --final-status-level fail -E 'test(native::text_spool::tests::directory_descriptors::) | test(=native::text::tests::occurrence_blocks::prepare_read_cost::cold_multiblock_alias_prepare_visits_each_descriptor_and_child_once) | test(native::text_occurrence::tests::count_only) | test(native::text::tests::descriptor_transcript::)' --test-threads 1`.
- Kernel `a4293c50`는 6 actual/5 PASS/1 FAIL/exit100이었다. 실패는 fixture의 물리 child 기대값 오류다. 모든 owner의 `[0,1]` occurrence payload가 같아 child dictionary는1이고 owner membership은8이다. 고정 header/모든 row의 child slot을1child·slot0으로 정정했으며 owner/DF/TF/positions·pin·실제 파일 GC·reopen 조건은 유지했다.
- 같은 Kernel command 재실행 `01b33230-6c96-4e84-a8fe-70d315c0d7de`: **6/6 PASS /79 skip /exit0**, compile9.10s·본문12.145s. `text_entry_maps::` 4개(DF9→8→9·pin 해제 후 block 실제 파일 제거·latest 재오픈 포함), no-backend-credit dense fallback, same-packet earlier-child atomic confirm을 검증했다. 공통 prefix는 위와 같고 `-p quanta-index-kernel --test native_lifecycle -E 'test(text_entry_maps::) | test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) | test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically)'`다.
- Runtime `352e6009-c0d8-4c5c-b660-1bb98edada53`: **12/12 PASS /639 skip /exit0**, compile2m28s·본문20.742s. 실제 ManagedObjectIo/Catalog/Original 64MiB·8MiB에서 257 distinct terms×두 case의 Directory inventory와 같은 body 두 source의 DF2·독립 첫/중간/끝 positions, artifact 동일성·reopen을 확인했다. 기존 Small/dense 전체9 + policy/profiler 범위도 통과했다. 명령은 같은 prefix·nextest flags에 `-p quanta-index-searchd-runtime --lib --no-default-features -E 'test(native::dense_install_tests::)'`다.
- 전체 영향 `b0a8245d-b8a3-449a-88dc-12328f22b450`: **407 actual /406 PASS /1 FAIL /139 skip /exit100**, compile21.20s·본문58.289s. 새 Ready 재사용 fixture가 첫 owner 뒤 `apply_postings`로 scratch를 seal한 다음 두 번째 inventory diff를 넣어 `posting scratch was sealed`로 실패했다. 실제 생산 순서인 모든 inventory windows→한 번의 final apply_postings로 fixture를 정정하며, 동일 attempt/cache·owner2 DF/positions 조건을 유지한다. 새 leaf Directory·기존 모든 나머지406은 통과했다. 정정 후 `a6b876ee`도406/407로 실패했으며 원인은 first/second owner lookup 시점이었다. `ContributionWrites::flush`는 top surface1에 DATA를 staging할 뿐 `root.owners`를 갱신하지 않으므로, 두 inventory/contribution staging 뒤 한 번 final apply_postings를 하고 나서 owner map을 조회하도록 정정했다. 기대 DF2·두 owner·전514term positions 및 같은 attempt/cache/no-extra-install은 유지했다. exact 실패1의 `2072d289-7578-4dff-9a4b-7a18595e96c9`는1/1 PASS(compile21.59s·본문0.083s·exit0)였다. 그 뒤 같은407 owning을 다시 실행한다.
- SDK BM25 exact `07526974-5acc-46d4-915a-64a7272c8fbe`: **1/1 PASS /74 skip /exit0**, compile3m47s·본문2.052s. 새 Descriptor/Small policy에서 실제 UDS pinned view·fixed BM25 gold와 reopen을 검증했다. 명령은 같은 prefix·flags에 `-p quanta-index-searchd-runtime --test native_index_uds --no-default-features -E 'test(=native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold)'`다.
- Ready fixture의 실제 staging/publication 순서 정정 후 전체 `00be1a18-ad8c-492d-a58f-4d157e8b3b7e`: **407/407 PASS /139 skip /exit0**, compile10.89s·본문15.853s. Text/TextSpool/TextOccurrence/Literal 전체 영향 범위에서 Ready reuse·leaf Directory·post-map fault·기존 회귀가 모두 통과했다. 명령은 같은 nextest flags에 `-p quanta-index-lexical --lib --no-default-features -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal)'`다. 공용 큐가60초 이내 비지 않아 이 재실행은 **운영 입장 대기만600초**로 조정했고 실제 입장은 즉시였다. SDK60/원 caps 변경은 없다.
- 새 candidate SDK는 별도 후속이다. Runtime Directory fixture는 raw bytes가 다른 공백/개행 body와 batch1로 whole-body inventory reuse를 배제하고 confirmed occurrence Ready 경로의 물리 block 설치2회를 추가 검증한다. 이 보강 뒤 Runtime12를 다시 실행한다. 집중6 PASS로 원512MiB 또는 SDK60 통과를 예측하지 않는다.

## 8. 설계 근거

- [Lucene TermsHashPerField](https://github.com/apache/lucene/blob/main/lucene/core/src/java/org/apache/lucene/index/TermsHashPerField.java): term interning, byte/int pool의 term별 stream, 고유 term 정렬.
- [SPIMI 원저자 교재](https://nlp.stanford.edu/IR-book/html/htmledition/single-pass-in-memory-indexing-1.html): 제한된 dictionary·posting 직접 추가와 block 출력·병합. Native 전체가 SPIMI이거나 외부 정렬이 없다는 뜻은 아니다.
- [Tantivy postings writer](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/postings_writer.rs): term index 정렬 후 직렬화. 기존 Native sorted run/merge를 재사용하는 근거다.
- [Lucene RAM flush 설정](https://lucene.apache.org/core/10_5_1/core/org/apache/lucene/index/IndexWriterConfig.html), [Tantivy worker](https://github.com/quickwit-oss/tantivy/blob/main/src/indexer/index_writer.rs): 메모리 기반 flush. Native의 엄격한 Original 사전 승인이나 누적 Work 한도와 동일한 계약으로 복사하지 않는다.
- [Lucene IndexWriter 10.5.2](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/IndexWriter.html): flush와 durable commit 구분. Native의 reserve/confirm·root CAS 프로토콜을 대체하는 API가 아니다.

이 레퍼런스들은 설계 패턴의 근거다. 이 패킷의 신규 구현·원본 SDK 성능 또는 다른 엔진 대비 우위가 검증됐다는 증거는 아니다.

### 2026-11-15 SDK 선행 접합 현재 입력

Kernel History/Cow/request decode 13개 PASS와 이후 SearchPlane의 History replay import/unused imports/CBOR error variant compiler101·본문0은 선행 접합의 이력이다. 아래 Text7·Runtime3은 이후 별도 실제 실행에서 통과했으며, 그 결과를 전체 SearchPlane owning이나 원본 SDK 성공으로 확대하지 않는다.

후속 공용 wire·SourceJoin owning은 D의 기존 Q 큐에서 nextest `fe47aa32-26f1-4b19-a048-84bba92222e0`, **Wire12 + SourceJoin44 = 56 PASS / 373 skip / exit0**으로 종료했다. 실제 command execution `exec-02d5588d-a73d-4879-ae68-f4b222c16d32`의 compile6.27s·Starting56·completed/exit0와 Root의 전체 본문 결과를 대조했다. 기존 `Digest64` tuple wire에 맞춘 독립 gold 수리 뒤 결과이며, 이전 `866f4440`의 gold 불일치는 종결된 이력이다. 명령은 같은 lane의 `./scripts/cargow nextest run --locked --offline --build-jobs 2 -p quanta-index-contract --lib --features quanta-native-identity-v1 -E 'test(native_cutover_evidence_v1::tests::) | test(native_cutover_source_join_v1::tests::)' --no-tests fail --no-fail-fast --test-threads 1`이다. 이 선행 owning PASS도 후속 Text 영향 회귀·release 후보·Caddy/etcd SDK60의 실행을 대신하지 않는다. 해당 다음 actual handle은 아직 미수신이며 worker Rust/SDK runner는 0이다.

Text7의 첫 실행은 새 alias calibration test의 `drop((alias, cold))` E0505로 compiler101·본문0이었다. alias를 먼저 drop한 뒤 cold arena를 drop하도록 최소 수정했다. clone·추가 allocation·production·gold 변경은 없으며 formatter/diff-check 통과 뒤 같은 7scope를 Root에 재요청했다. 이 compile 실패를 제품 RED로 세지 않는다.

SDK 원입력 read-only 재확인: 보존된 `run-original-sdk.py`가 지정한 같은 snapshot/manifest에서 Caddy 590파일·3,272,107B, etcd 1,421파일·8,666,217B의 전체 path 집합·각 파일 SHA·manifest SHA·repository commit·검색 토큰이 일치했다. Caddy manifest는 `3aae07d2535fc1f165ea9e18a597021bd1fc5d732bdbdd128055e88c78f8e937`, etcd는 `15e75db82a188dda532ac7b13ffa215e6ea7c657352c202382ced438f2478fc4`다. 입력을 수정하거나 새 manifest/runner를 만들지 않았다. 이는 SDK 입력 준비 검증이며 old `16d8d0ee` 후보를 새 NTX 후보의 수용으로 재사용할 권한이 아니다. 새 release·원 캡·같은 원입력으로 실제 SDK 게시·검색·재오픈을 실행해야 한다.

### NTX2A-G7 실제 GREEN

Root의 `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-lexical --lib --no-default-features --no-tests fail -E 'test(native::text::tests::inventory_trace_tests::) | test(native::text::tests::inventory_shape_tests::)' --test-threads 1 --no-fail-fast --failure-output immediate --success-output final`은 nextest `0a7b720b-190a-441c-a3b1-bbefd2e7032c`, **7 actual bodies / 7 PASS / 529 skip / exit0**, compile26.14s·본문0.126s로 종료·실행권 반환됐다. 현재 shape SHA `75ac4cae12911c2f30e5fa66e3778118abcc70761e7fd20216bc5b8009a217e9`, production `text.rs` `ee2c6944` / `occurrence_pack.rs` `fb2eba49`를 인수한다.

이 범위는 Work3·SQL/Empty/borrowed Alias/SQL-produced InlineAlias의 shape4다. 특히 SQL 출력에 alias가 있어도 SQL provenance를 유지한다. 고정 P/U/K·literal key/position·Off/Record 원 Work·첫 Bytes 거절·메모리 해제를 확인했다. **Direct DirectoryAlias는 이 fixture의 선택 경로가 아니며 NOT_RUN**이다. 아래 Runtime3은 이후 VERIFIED로 전환됐고 원 SDK60 수용은 NOT_RUN이다. 원본 etcd의 실제 배타적 Work·shape 분포를 얻기 전 토큰 생산자 전면 재설계나 예산 완화를 채택하지 않는다.

Runtime3의 첫 compile은 새 Catalog visitor 공용 parent export 누락으로 compiler101·본문0이었고 Root가 해당 parent를 수정했다. 다음 compile의 Text leaf 네 E0308은 concrete `Arc::clone`이 expected trait-object Arc로 역추론된 문제였다. 기존 concrete Arc를 유지하고 명시 typed local coercion으로 최소 수정했다. 새 provider/Box/wrapper·backing allocation·production·gold 변경은 없다. 현재 Runtime SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과 뒤 같은 Runtime3를 Root에 재요청했다. Root 소유 History cfgtest match 두 곳의 접합 수리는 별개다. 두 compile 실패 모두 실제 제품 RED/GREEN으로 세지 않는다.

Runtime3의 세 번째 compile도 unused-result 한 건으로 compiler101·본문0이었다. fresh retry 뒤 확인한 member artifact를 보존하고 fixture를 다시 열어 current published view·membership artifact의 동일성으로 반환값을 소비했다. clone·새 API·production·gold·한도 변경은 없다. 모든 membership 호출 반환 사용을 재대조했고 formatter/diff-check 통과 후 현재 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`로 같은 3scope를 요청했다. 기존 nextest PID32495·cargo32958·rustc37509는 terminal 이후 사라진 것을 확인했으며 observation timeout만으로 재시작하지 않았다.

### 실행권 인수

Root가 usageLimited였을 때 기존 검증 실행권을 D에 넘겼고 Runtime3 Q 실행 요청도 그 인계에 포함됐다. Source의 `compile-activity-admission.lock`과 QIndex `scripts/cargow`의 `resource-admission/build-test.lock`은 별개라는 RCA는 Root가 인수했다. Runtime3은 아래 실제 run에서 종료했다. 현재 동결 fixture는 `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`이며 production SHA는 아래 GREEN의 4파일과 같다. 후속 owning·release·SDK 실행권은 D에 있고 명시 반환 전 Text worker Rust/SDK runner는 0이다. Root/D의 대화·대기 의도는 live job이나 본문 성공의 증거가 아니다. actual PID/session/terminal을 받은 뒤만 실행 상태와 결과를 판정한다. Source supplier 종료·lock 우회·새 실행 레인을 만들지 않는다.

### Runtime 첫 실제 행동 실패와 단계 RCA

D owning nextest `bfc8e6ad-3d9f-4430-8e61-9613d3139674`은 compile0.77s 성공 뒤 실제 `dense_text_post_confirm_error_preserves_current_and_fresh_retry` 1건이 1.025s·`Error: Conflict`로 실패했고 failfast 때문에 나머지 4건은 NOT_RUN이었다. namespace가 선택한 5개는 business 3 + 공유 SQL profiler helper 2이며 source 변경이나 추가 business testcase가 아니다.

Catalog/RFC의 현재 계약상 Unknown op2는 Pending SourceEvent claim을 유지하고 동일 event의 새 op3는 Conflict를 반환한다. 같은 op2를 다시 submit하는 것도 재실행이 아니며, canonical recovery는 `kernel.resume(op2)`가 새 fence에서 원 immutable request를 재실행하는 경로다. 원 stderr만으로 실패 위치를 단정하지 않기 위해 fault fixture의 initial open/configure/request/submit/reopen/retry에 phase label만 추가했다. 현 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과다. 기대값·예산·공급자·production은 유지했다. D의 같은 Q 실행권에서 failfast 없이 현재 5개 또는 exact 실패1의 재실행을 요청했다. retry 단계임이 확인되면 fixture의 잘못된 op3 API 사용을 canonical resume으로 변경하되 full publish/9owner membership/reopen 동일성은 그대로 증명해야 한다. Conflict 자체를 성공으로 바꾸지 않는다.

복구 전제 소스 대조: native `claim_recovery`는 Pending·현재 epoch 일치를 검사하고 즉시 새 fence를 발급하며 별도 lease 만료 대기가 없다. KernelServices의 read lease는 operation recovery gate가 아니다. 이 fixture의 이전 동기 실행 반환·소유자 drop·같은 exclusive root lease 재획득 뒤 `resume(op2)`가 합법이고, 이전 fence의 늦은 쓰기는 FenceLost다. Catalog 재오픈을 OS 프로세스 재시작 증거로 승격하지 않는다. 실제 Conflict의 phase를 확인하기 전에는 생산 코드 결함이나 fixture 오류를 최종 확정하지 않는다.

### Runtime phase 확정과 canonical recovery 수리

D diagnostic exact1 nextest `f5353ad3-13cc-4128-8eec-357fe6e3be4a`은 compile2m02s PASS 후 actual body1 FAIL·1.780s·640skip·exit100으로 종료했고 stderr는 `fault retry submit: incremental precondition conflict`였다. 따라서 Unknown 원 작업의 Pending event claim을 둔 채 새 op3를 제출한 fixture의 API 사용 오류로 원인을 확정했다.

원 fixture 하나에서 새 request/event 재작성 대신 `kernel.resume(op2)`가 새 fence로 original immutable request를 재실행하도록 수정했다. 이름은 `dense_text_post_confirm_error_preserves_current_and_resumes_original_operation`로 정정했고 복제 testcase를 만들지 않았다. Unknown·dense1call/COMMIT2·old current/empty/reopen·정상 full publish·9owner membership·재개 게시 뒤 다시 reopen artifact 동일성은 모두 유지했다. Conflict를 전체 성공 기대값으로 바꾸지 않았고 엔진·버짓·한도·공급자는 변경하지 않았다. 현 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과다. D가 인수한 같은 Root Q 큐에 `test(/^native::dense_install_tests::dense_text_/) --no-fail-fast`의 primary3 재실행을 요청했다. 이 regex는 shared SQL profiler helper2를 제외한다. 당시 수리 후 실행 대기였고, 아래 run에서 세 본문 모두 통과했다.

### NTX1a Runtime3 실제 GREEN

D의 기존 Root Q 큐에서 아래 명령은 nextest `8ee85ee8-371d-405e-98ff-aef809179e83`, compile2m03s·**actual3 / PASS3 / 638skip / body6.024s / exit0**으로 종료했다. `resource-admission/build-test.lock` 반환을 확인했다. frozen fixture `67b72b82`, production write `13045e24` / wire `9c27d9b3` / text `ee2c6944` / occurrence pack `fb2eba49`가 그대로다.

```sh
QUANTA_INDEX_BUILD_LANE=test-workspace-lane QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 \
./scripts/cargow nextest run --locked --offline --build-jobs 2 \
  -p quanta-index-searchd-runtime --lib --no-default-features \
  -E 'test(/^native::dense_install_tests::dense_text_/)' \
  --no-tests fail --no-fail-fast --test-threads 1
```

- G1 PASS2.420s: 실제 승인 Original의 whole-window leaf+directory 설치 1batch/2COMMIT 대 미지원 fallback 2batch/4COMMIT, fixed DF/TF/positions·artifact·재오픈 동일성.
- F2 PASS1.380s: confirm 뒤 오류에서 Unknown과 기존 공개 root 유지·재오픈, 원 작업 `resume(op2)`의 full publish·9owner membership·다시 재오픈.
- A1 PASS2.222s: 실제 8MiB credit에서 optional admission miss 후 기존 경로 성공, first denial 없음·fast와 같은 artifact/조회/재오픈.

이 결과는 production coalescer와 복구·입장의 집중 검증이다. 실제 Caddy·etcd SDK60 수용, 모든 source의 Direct 분기, 새 dense artifact의 실제 GC fault 조합을 뜻하지 않는다. Root에 영향 owning·기존 child-first/GC/barrier·UDS rank/reopen 후 같은 source/Cargo 후보 release와 원본 SDK를 이어 달라고 요청했다. Text worker Rust/SDK runner는 계속 0이다.


### 현재 Descriptor 후보 source capture

`/private/tmp/qidx-sdk60-directory.383jci/source-before.json`은 HEAD `ade5fadc1a084fbb81eba831861e996081d80867`의 dirty workspace build inputs 2,789개를 결속한다. 기존 실패 후보를 덮어쓰지 않았다. source freeze는 오너에게 알렸으며, before/after 비교 일치를 실제 확인한 경우에만 후보 수용에 쓴다.

보강 Runtime `cc25ca9c-ed2f-427c-8fdf-6b0096c81c7f`: **12/12 PASS /639 skip /exit0**, queue157.84s·compile28.56s·본문15.873s. Directory fixture는 서로 다른 raw 공백/개행 body, inventory batch1로 whole-body inventory reuse를 배제했다. 두 window에서 동일 occurrence block2를 한 번만 설치하고 64MiB/8MiB 모두 같은 artifact·DF2·독립 positions·reopen을 검증했다. 명령은 같은 Runtime12 selector와 `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600`이며 SDK/product caps 변경은 없다.


새 후보 Native-only `cargow tree -p quanta-index-searchd-runtime --no-default-features --locked --offline --edges normal,build --prefix none`와 Tantivy package 차단 검사는 exit0이다. 같은 Q lane/jobs2/운영 대기600에서 daemon release를 빌드 중이다. queue157.80s 뒤 입장했고, 입장 시 캡처2,789개 파일의 현재 drift0을 확인했다. `cargo-metadata.json`의 canonical target_directory에서 결과를 복사하고 source-after-daemon 비교를 통과한 경우만 기존 helper 재빌드/SDK로 진행한다. 현재 binary/pair/SDK 성공으로 판정하지 않는다.


daemon release는 compile2m50s·exit0으로 종료했고 `/private/tmp/qidx-sdk60-directory.383jci/quanta-index-searchd`에 chmod0500으로 복사했다. SHA256 `98d6ef5292b1f5745b0ef11e0fabd50462130ee58e4dd9d446760aee257661fb`·14,614,976B다. source-before/after-daemon은 HEAD와2,789개 입력이 모두 같았다. 현재 기존 `native_corpus`를 별도 rebuild 중이며 pair closure 전 SDK는 NOT_RUN이다. helper의 Legacy/Native 혼합 의존성을 daemon의 Native-only/Tantivy-free 그래프와 혼동하지 않는다.


helper release는 compile5m10s·exit0으로 종료했다. 기존 `native_corpus`의 새 SHA256은 `807af561449566de636b134a277f20a5e2a41ff8b8992855edaa0801121ea616`·2,890,976B다. `source-after-helper.json`은 HEAD와2,789개 입력이 before와 모두 동일해 pair closure를 통과했다. `pair-binding.json` 및 Off1/Off2/Record1의 동일 daemon 복사본을 준비했고 원 runner의 quiet Off1 입장을 시작했다. 이 시점 SDK 본문 수용은 아직 미판정이다.


Off1은 원 quiet preflight에서 foreign rustc1·load26.576172>8로 exit1·SDK 본문0 **BLOCKED**였다. `ALLOW_CONTENDED`를 설정하지 않았고 같은 상태의 Off2를 반복하지 않았다. pre-SDK source closure도2,789개 모두 일치했다. 원 budget의 Record를 동일 Q mutex에서 별도로 실행한다. Record 시간은 quiet 성능 수용으로 사용하지 않는다.


### Descriptor 후보 원 SDK 실제 실패와 다음 경계

새 /private/tmp/qidx-sdk60-directory.383jci의 Record는 **Caddy54.63s·etcd33.60s, 둘 다 ResourceLimited·ACK0·query/reopen 미도달 FAILED**다. source/daemon/helper/원 corpus/원 caps는 위 pair로 결속됐다. Off1은 foreign rustc1·load26.576172>8에서 본문0 BLOCKED였고 Off2·Large는 실행하지 않았다. 실패 후보를 그대로 반복하지 않는다.

| 원본 | 새 첫 거절 | 상태 |
| --- | --- | --- |
| Caddy590 | reads999729+275>1000000, managed_install.rs108; bytes499758302/W523919/M658407; memory denial 없음 | COMPLETE_DIAGNOSTIC·health[]·open request0·ACK0 |
| etcd1421 | bytes536870900+28>536870912, cold_tokens.rs398; R491784/W460564/M689731; memory denial 없음 | COMPLETE_DIAGNOSTIC·health[]·open request0·ACK0 |

post-Record의 전체 workspace 비교는 두 무관한 integration test(native_product_process/ann_regions.rs, native_index_uds/read_sessions/query/hybrid.rs) 변경 때문에 false다. selected release --bin 빌드에는 두 test target이 포함되지 않고 runtime/src·harness/src에서도 include하지 않는다. 나머지2787개 입력 및 HEAD·동결 binary digests는 동일하며 record-source-binding.json에 구분했다. 전체 workspace 불변이라고 주장하지 않는다.

Caddy의 공통 첫329 posting pages를 대조하면 lane37의 R207662/W73258/B86813378와 dense reserve293calls/4306objects가 동일하다. Small lane39는 R525787→292336(-233451), physical wrapper inputs5471→325였다. 새 실행은 이 절감으로452번째 page까지 진행했지만 다시 읽기 한도에서 거절됐다. 이를 전체 읽기756k가 비슷하다는 이유로 퇴행이라 하지 않으며, ACK0을590source 커밋으로 승격하지 않는다. 현재 Small Draft는64-term SQL page마다 flush돼 cross-page child dictionary 재사용이 없다. 다만 trace에는 인접 block child 중복·남은 총 page 수가 없어 NTX1b가 원 한도를 해결한다고 확정하지 않는다. 다음 채택에는 실제 block headroom/child overlap와 전체 term count가 필요하다.

etcd의 완료된 prepare metric은704→785 owner(Alias569/Direct216/SQL0), P1118628/U367668/Kall8859762/Kuniq4458175다. map lane40/전체 map_apply3 비용은 이전보다12162098B 줄었지만, 추가81owner 준비가11927193B를 소비했다. diff는606회/124196392B로 동일한 prefix여서 후속 backlog가 남았다. NTX2B 후보 경계는 canonical census→ColdTokens fill/sort/group이지만 Kall−Kuniq4401587B나 마지막28B 거절을 전체 Work 절감·완주 예측으로 바꾸지 않는다. 원 logical edit·seal/EOF fees는 유지한다.

### 2026-11-16 confirmed cold Dense page 재사용

기존 `PostingCasePages`는 같은 term의 두 case를 같은 페이지에 넣고 backend same-batch sharing을 사용한다. 이번 보완은 다른 term page의 동일 sorted owner/witness DATA에 한정된다. 실제65term×9owner·130case·3page fixture의 수정 전 nextest `a7960636-aeb8-454c-8510-2692d21cf0bf`는 compile 성공 뒤 actual1 FAIL/651skip/exit100이었다. 관측값은 install3·COMMIT6이며 목표는1/2다. compiler 실패는 이 행동 RED로 세지 않는다.

`Draft::cold_key`는 finished·prior 없음·nonlocal만 허용하며 version domain/PAGE_ROWS/count/모든 owner32+witness32를 SHA-256으로 결속한다. 실제 새 해시 순회의 Work도 청구한다. 메모는 기존 batch 상한256슬롯·전체 digest 비교·충돌 시 결정적 교체다. parent Original의 optional grant로 entries와 pending key backing을 생성 전 승인하며 미승인은 원 경로다. 원 stream CAS/order/count/EOF/prepass epoch를 모두 확인한 뒤만 hit을 적용한다. 새 root는 전체 install ordinal/ref 영수증을 통과한 뒤 기록하고 실패한 prepare는 메모를 버린다. 최종 Catalog parent admission과 DF/term Top 편집은 유지한다.

Full-term-bound Small block은 서로 다른 term page 사이에서 같은 root가 될 수 없다. 기존33B Object-only `confirmed_postings` 조회는 Dense Inline도 찾지 못했다. 그 미사용 API·SQL·보조 인덱스·전용 테스트를 퇴역했다. 실제 Top MapEdit 저장은 유지하고 삭제한 인덱스의 예약·쓰기만 제거했다.

새 Lexical owning nextest `5819c585-a27e-4574-9509-cb85fbe3a45e`: **408/408 PASS /139skip /exit0**, compile37.37s·본문27.193s. 공통 prefix는 `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane`, 명령은 `nextest run -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`이다. 기존407에서 obsolete SQL 전용1을 제거하고 메모 custody/fence·고정 hash gold2를 추가했다. production SHA·실제 runtime/SDK 증거는 후속 후보에서 다시 결속한다.

새 Runtime owning nextest `ccab0af4-f692-47bc-aaab-2cdc7923b24a`: **13/13 PASS /639skip /exit0**, queue347.794s·compile10m30s·본문30.413s. 같은 prefix에서 `nextest run -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`을 실행했다. 새 cross-page fixture는 64MiB에서 install3→1/COMMIT6→2, 8MiB에서는 install6→2/COMMIT12→4와 DF9·9owners·양 case TF1/positions0/32/64·artifact·fresh reopen 동일성을 검증했다. 기존 actual Original·post-confirm Unknown/resume·Directory Ready·Small 회귀도 같은13 scope에 포함된다. 시간은 contended 단위 검증이며 SDK60 수용이 아니다.

키 풀 분류: canonical cursor·연속 key/12B row arena·정렬/grouping·packed block의 같은 position vector sharing은 이미 구현됐다. 반복 occurrence의 key byte interning은 새 구현이다. 관측 prefix의 Kall−Kuniq=4,401,587B 및 가정한 row12→8 절감4,474,512B는 metadata 전 backing 추정이며 누적 Work 환급이나 전체 etcd 완주 증거가 아니다. RAW span 부모 chain 기준 read32 배타적 비용은111,996,156−74,391,304=37,604,852B다. 전역 posting33을 빼서30,975,194B라 한 추정은 사용하지 않는다.

### 후속 수용 경계와 전체 Work 소유자

Kernel/UDS 후속 요청은 동일 Q mutex의 입장600초 만료로 exit124·본문0 **BLOCKED**였다. 이를 test FAILED로 세지 않는다. 새 후보 `/private/tmp/qidx-sdk60-cold-roots.hysXvG`의 초기 캡처2,796개와 비교해 다른 오너의 harness ann/harness/lib, Runtime Cargo.toml/canonical_publication_admission, integration native_boot 변경6개를 확인했다. Native test graph 정리 중인 입력이므로 이 초기 캡처로 release/SDK의 동일 source를 주장하지 않는다. 새 binary·SDK는 **NOT_RUN**이며, 합류 뒤 입력을 다시 결속해야 한다. 실제 캡·골든·의존성 feature를 우회하지 않는다.

같은 Kernel6/UDS1 재시도는120.855s 뒤 입장했으나 새 D 합류 dependency `native_cutover_evidence_v1.rs:968-969`의 `blake3::Hasher::update` 반환값 미사용2건에서 compiler101·본문0이었다. build check는 FAILED, 테스트7은 NOT_RUN이다. 해당 오너가 반환값을 한 chained call의 `_hasher` binding으로 소비한 현재 소스를 확인했고 Text가 같은 hunk를 중복 수정하지 않았다. 동일7 scope를 재요청했다. 이 lint 접합 수리는 Text 행동 RED/GREEN이 아니다.

합류 뒤의 dependency tree 재확인에서 Runtime `--no-default-features --edges normal,build,dev`와 helper `--no-default-features --edges normal,build` 모두 Tantivy package0·exit0이었다. 기존383jci 후보 helper graph에 관한 과거 상태를 현재 후보에 복사하지 않는다. 이 결과는 그래프 검증이며 합류 compiler·테스트 본문·새 release binary·SDK 수용을 대신하지 않는다. 현재 후보의 두 tree는 외부 `native-runtime-with-dev-after-merge.txt`/`native-helper-after-merge.txt`에 있다.

etcd Record의 RAW 부모 span별 disjoint accepted Work에서 BuildPrepare는518,944,526B, 그 자식은 Text lexical_build299,881,954B·Literal186,751,136B·RepoMap32,020,144B와 작은 provider/seal이다. 관측 top-level span 합534,132,785B와 실제 accepted536,870,900B 사이2,738,115B는 미관측/바깥 비용이므로 합을 전역 총액이라고 부르지 않는다. 한도536,870,912B까지 남은12B와 거절된 추가28B를 미관측 accepted 비용에 섞지 않는다. 앞의 prepare/diff/map 부분 합만으로 전체 실패를 Text 키 풀 결손으로 단정하지 않는다.

Literal lane47의52,349,516B는 설치한 새 block384개를 각각 한 번 fresh get/decode한 뒤 독립 source seal과 대조한 비용이다. `literal_seal_refuses_changed_persisted_child_without_publication`이 이 검증을 요구한다. body collect66,553,010B도 Literal 내부 owner별 body 한 번이며 기존 bounded delta spill·압축 block encode·독립 seal이 이미 존재한다. 단순 cache나 fee 삭제로 제거하지 않는다. `read_retained`도 같은 logical read/wire/decode Work를 청구하므로 누적512MiB 해법으로 간주하지 않는다.

RepoMap `file_map::check_changes`는 인증된 next source record를 얻지만 전체 path 고유성·EOF를 마친 `stage_contributions`가 같은 source record를 다시 읽는다. 이 경계는 제한된 scratch DATA 전달을 새로 보완할 후보이며 단순 port 연결은 아니다. 전 owner record를 RAM에 보유하거나 EOF 전 쓰기를 시작하는 대안은 채택하지 않는다. scratch 쓰기/읽기까지 포함한 순절감·소유권·회귀를 확인해야 하며 현재는 정적/trace 분석뿐이고 구현/실행 **NOT_RUN**이다.

### 현재 Text 인계 상태

**state: partial.** Text 코드 보완·집중 회귀는 완료했지만 합류 후 제품 수용은 완료하지 않았다. 마지막 동일 Kernel6/UDS1 요청도 입장600초 만료·exit124·본문0 BLOCKED로 종료했다. Text worker의 live Rust/SDK는0이며 초기 SDK 후보 캡처는 수용에 사용할 수 없다. 현재 source freeze는 해제하며 Root의 기존 단일 실행 큐에서 합류 입력을 다시 캡처한다. 실패 후보의 SDK 반복, 별도 실행 레인, 예산/골든 변경은 하지 않는다.

후속 exact 명령은 공통 Q prefix와 다음 target이다. 두 binary의 selector를 구분해 Runtime의 다른 `text_entry_maps` 테스트를 우연히 포함하지 않는다.

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 \
./scripts/cargow --lane test-workspace-lane nextest run \
  -p quanta-index-kernel -p quanta-index-searchd-runtime \
  --test native_lifecycle --test native_index_uds --no-default-features --locked --offline \
  -E '(binary(native_lifecycle) & (test(text_entry_maps::) | test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) | test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically))) | (binary(native_index_uds) & test(=native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold))' \
  --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

Root의 Native test graph 합류 뒤에는 위7 scope와 Runtime13을 재결속하고 Native-only daemon·현재 helper를 같은 소스에서 release 빌드한다. 원본 Caddy590/etcd1421·SDK60·512MiB/읽기100만회·Original caps로 quiet Off와 Record를 분리한다. Source/binary·원 입력·출력·최초 거절이 결속된 새 SDK가 통과해야 NTX4/후속 Large로 진행한다. 키 풀·RepoMap 전달 경로·전체 streaming 확장은 실제 새 후보의 순 Work/입장 결과를 근거로 별도 채택하며 현재 구현 완료로 세지 않는다.

### 2026-11-16 기존 목표 재개 · 현재 코드와 남은 인수

사용자가 이 채팅의 기존 잔여 작업을 active goal로 재개했다. 철회된 I06 3주 감사 배정은 수행하지 않는다. HEAD는 `ade5fadc1a084fbb81eba831861e996081d80867`, index는 비어 있고 기존 dirty/untracked 작업은 보존한다. Dense memo·caller의 현재 SHA는 write `f1417afdcf2cf05baf98bc4aa8e20564a90893038003ea55514ca43f173e7735`, entry_patch `4ae4f704b47541377f489098d27baeb597169ef8e3bca7de23b6e8483fe2d49f`, text `ea34880194100457732e6257440e6eadaa65e42681eb69ec8f85a1788cf65458`로 이전 보완과 같다. 정적 재점검에서 source EOF/cardinality→same-attempt memo→설치 영수증→최종 child 인증 경계의 새 확정 결함은 발견하지 못했다.

공용 F 인수 기록의 Native nextest `44e83b28-438c-4b73-97f5-5a7a2c7c6bbe`는 실제107건 중106 PASS/Contract fixture1 FAIL이며 Runtime/Harness37 PASS에 dense-install13이 포함된다. Contract 수정 후 별도 `b5e041f5-97eb-417a-8c40-eb17a6efaf26`은70/70 PASS다. 이는 인수 기록의 재사용이며 전체107 PASS나 새 release 동일 후보 수용으로 합치지 않는다. Text는 408/13을 무조건 재제출하지 않는다.

이전에 막힌 Kernel6/UDS1은 공용 compiler 접합 수리와 Native 실제 본문 이후의 변경된 입력에서 위 exact selector로 재검증한다. 현재 목표 재개에 따른 직접 검증 범위는 이7건뿐이며 기존 `test-workspace-lane`·jobs2·mutex·운영 대기600초를 유지한다. 실행 중인 동일 요청은 확인되지 않았으며 관측한 이전 nextest PID93953/24708 및 자식은 이미 종료했다. 공용 parent/Cargo/lock·타인 claim·실행기는 수정하지 않는다. 이 결과와 별도로 새 source/binary 결속·원 SDK60은 여전히 NOT_RUN이다.

**보완 전 검증 공백 T-DENSE-ALL-SLOTS:** `dense_install_tests.rs::repeated_dense_request`는65term×2case×9owner를 게시하지만 `check_repeated_dense_membership`는 term000/032/064만 읽는다. case-paired64-row page의 각 case 첫 term에 해당하므로 나머지62term의 실제 조회·위치·재오픈을 직접 확인하지 않는다. 이는 기존 oracle의 범위 제한이며 생산 코드 결함이나 실제 false-green을 재현한 것으로 세지 않는다. preimage SHA는 `97f61142737669ff1fffa0d786b5da0ade35ac6d7e3e01a04ff685b606428e95`다. 같은 helper를65개 전체 term의 고정 owner/TF/position oracle로 보완한다. 실제 Store/Catalog·64MiB/8MiB·COMMIT 수·artifact·4회 open/reopen 검증과 원 예산은 유지한다. 이 cfg(test) private leaf는 현재 Kernel/UDS integration target의 non-test Runtime library에 포함되지 않는다. 새 owning 요청은 기존7의 종료 뒤 동일 큐의 exact cross-page Runtime1이며, 이전 Runtime13 결과를 이 새 oracle의 통과로 복사하지 않는다.

**Kernel6/UDS1 VERIFIED:** 위 exact binary-scoped 명령은 nextest `601f5482-1dc9-422b-95d1-74353adad91f`, discovered7/executed7/PASS7/FAIL0/filter153/exit0으로 종료했다. 기존 mutex 입장368.174s·compile1.29s·본문14.574s, 실행권 반환을 확인했다. 현재 dirty HEAD의 실제 GC·DF9→8→9·old pin/reopen·earlier provisional child·미지원 batch fallback·고정 UDS BM25 범위다. 과거 compiler101/body0 및 입장124를 삭제하거나 성공으로 바꾸지 않는다. 제품 원 SDK60은 이7개로 승격하지 않는다. 새 Runtime1 소스 SHA는 `80fedbea51c28ffd46f7288561c51427d3a604bb437cca2d3fe5bc11998cb54d`이며 다음 동일 큐의 exact selector로 확인한다.

**T-DENSE-ALL-SLOTS VERIFIED:** nextest `524bb92d-2009-4da9-9770-eec63e69291e`는 discovered1/executed1/PASS1/FAIL0/filter611/exit0, compile36.75s·본문5.441s였다. 명령은 공통 Q prefix에서 `nextest run -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(=native::dense_install_tests::dense_text_confirmed_root_reuse_crosses_posting_pages_with_real_original_credit)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`이다.65term×2case×9owner를64MiB/8MiB 및 각각 fresh reopen에서 모두 고정 DF9/TF1/위치0..64로 확인했다. 기존 install1/COMMIT2 대 fallback install2/COMMIT4·동일 artifact·first denial 없음은 유지했다. production 변경 없이 기존 oracle 공백을 보완한 결과다.

현재 재개 목표의 다음 직접 실행은 이 검증 이후 같은 warm lane/단일 mutex/jobs2의 새 source snapshot→Native-only daemon+기존 helper release→원 SDK60이다. 최초 hysXvG snapshot을 재사용하지 않고 새 외부 후보에 before/after 입력 집합과 binary digests를 결속한다. 이 목적에 맞지 않는 clean-Git benchmark closure를 dirty Main에서 우회하지 않으며, 기존 SDK의 dirty snapshot binding과 benchmark/release qualification 범위를 구분한다. 원 SDK의 파일/Work/IO/시간·Original caps·qrels/gold·trace mode는 유지한다. 이전 binary·실패 prefix·지표로 새 후보를 통과 처리하지 않는다.

**새 SDK 후보 준비:** `/private/tmp/qidx-sdk60-all-slots.zU7Bre`에 현재2798 build inputs를 캡처했다. Native daemon+helper normal/build dependency tree의 Tantivy0 검사 exit0, Caddy590파일/3,272,107B와 etcd1421파일/8,666,217B의 manifest SHA·모든 파일 해시·commit·query path/token 일치를 재확인했다. 기존 manifest/원 SDK script를 변경하지 않았다. daemon release는3m16s·exit0이며 before/after2798개+HEAD가 모두 같았다. 복사한 chmod0500 daemon은14,631,472B·SHA256 `8bf83973455773ffaacdbdcc25c2410ad6d545a63a38f134c55d807ddeaf545f`다. 현재 같은 큐에서 helper의 명시적 no-default release를 빌드 중이며 pair closure/SDK 결과는 아직 NOT_RUN이다.

**새 pair 결속:** helper no-default release는12m01s·exit0으로 종료했다. chmod0500 helper는2,873,968B·SHA256 `28c9db78ddd449bdb53f9950f054587a9f359302b295b6290a968c0e23340068`다. before/after-daemon/after-helper의2798개 입력 집합·해시와 HEAD가 모두 일치하며 복사 daemon 해시도 같다. `pair-binding.json`과 원 입력 `input-binding.json`은 새 외부 후보에 있다. 같은 daemon을 Off1/Off2/Record1에 복사·해시 대조했다. 현재 단일 mutex에서 원 `run-original-sdk.py`의 quiet Off1 입장을 시작한다. 환경의 contended override와 Work limit override는 설정되지 않았다. benchmark clean-Git closure/출하·설치 qualification과 이 dirty source SDK 후보 결속을 혼동하지 않는다.

**새 Off1 BLOCKED:** mutex 입장62.829s 뒤 원 preflight가 `host_load one_minute=26.200684 limit=8.0`로 exit1·SDK 본문0을 반환했다. contended override를 설정하지 않았고 Off2는 같은 상태에서 재제출하지 않는다. 이 결과는 새 binary의 ResourceLimited나 성능 실패가 아니다. 같은 pair/원 caps의 Record를 기존 mutex에서 진단 전용으로 실행하고 최초 거절·완료 작업량을 회수한다. Record 시간은 quiet 수용으로 사용하지 않는다.

**새 Record FAILED:** Caddy는43.93s·reads999982+27>1000000·B499650244/W524037/M658415, etcd는20.96s·bytes536870900+28>536870912·R491784/W460564/M689731에서 ResourceLimited·ACK0이었다. query/reopen에 도달하지 않았다. 두 진단 모두 COMPLETE_DIAGNOSTIC·health[]·open request0이며 parent exit1/실행권 반환을 확인했다. Source before/after-record2798개+HEAD 및 두 binary digests가 모두 같았다. 이 실패 후보를 그대로 반복하거나 Large로 우회하지 않는다.

Caddy는 이전452→새454 posting page까지 진행했다. lane37 누적 reads308687→307081과 bytes124723818→132989366, lane39 bytes58084579→51497767이다. 진행 prefix가 달라 이 누적치만으로 순절감률을 만들지 않는다. 같은 first329 page 및 실제 object calls를 대조해야 한다. etcd의 prepare/lexical Work는 이전과 정확히 같아 이 Dense·Small lookup 보완을512MiB 해결로 세지 않는다.

**다음 확정 caller 결손 T-SMALL-PAGE-FLUSH:** `apply_postings`의 Small Draft는 실제16KiB/512row 한계와 무관하게 매64-term SQL page 끝에서 flush한다. case-paired page는 각 case의 순서만 전역 단조이므로 하나의 Draft를 그대로 이어 붙일 수 없다. 기존 canonical Small codec·DF2..8·full-term/child/slot 인증을 재사용하고 case별 제한된 Draft+pending locator로 실제 byte/row 경계에서만 확정하는 caller 보완을 검증한다. 전체 NTX1b queue·prepared API·새 Core/Storage/Catalog 계약으로 확대하지 않는다. source EOF는 page별 기존 검증을 유지하고 전체 Top 적용/공개는 마지막까지 유보한다. 메모리/오류별 다른 packing fallback을 추가하지 않는다.

보완 전 producer/consumer는 `text.rs::apply_postings`의 stage38→page-end `stage_small_posting_block`→stage39 put_batch/locators/top_batch다. wire owner는 `small_posting_block::Draft::push/finish`이고 limit은16384B/512rows다. Current preimage text SHA `ea34880194100457732e6257440e6eadaa65e42681eb69ec8f85a1788cf65458`, codec current policy는recipe-fixed-v1이다. 새 grouping의 결정성은 명시적 layout-policy bump와 fixed hash gold로 결속한다. 동일 digest/다른 wire 허용·후반 install 실패의 성공 처리·post-map persisted 검증 생략은 금지한다.

독립 실제 RED 요청은 Runtime `native::dense_install_tests::small_text_packs_across_posting_pages_with_real_original_credit`다.129lower-case terms×2case×2owners는5 SQL pages지만 각 case의129term/258row block은 실제 byte/row bound 안에 들어가므로 고유 QITXSP01 입력2개를 요구한다. 기존5개가 실패해야 한다. 실제 stage_sources/Kernel submit·모든 term의 DF2/TF1/위치·64MiB/8MiB·fresh reopen/artifact가 동일해야 하며 원 budget은 유지한다. fixture SHA `874a8e8489e7267529a697fb08c4a197e60385263539aebc4957d5dc9cf7bf84`다. Storage는 같은 batch의 동일 object를 Catalog 전에 이미 dedup하므로 caller Dense dedup만으로 Caddy read quota를 해결한다고 주장하지 않으며 이번 보완과 혼동하지 않는다.

### T-SMALL-PAGE-FLUSH implementation and current proof

- Behavioral RED: nextest `9101c8ae-885c-44a1-bcac-ba09fb0d7726`, executed1/FAIL1/filtered612/exit100. Real Store/Catalog returned5 QITXSP01 inputs where the fixed129-term/2-owner/2-case oracle requires2. Membership checks completed before the block-count failure.
- The canonical caller now retains two bounded case-stream drafts across SQL pages. It flushes at the existing16384-byte/512-row envelope, validates the exact install receipt before creating locators, and applies the final Top graph only after all drafts and scratch Top batches complete. EOF tails merge only if the same wire envelope fits; the existing few-term one-block oracle is retained. Child dictionary indices and pending locator slots are remapped in canonical order.
- Original grants cover draft/pending/key/Top backing until its actual last use. The fixed tail-remapping table is admitted before allocation; copy and slot-rewrite Work remains charged. Failed memory/Work admission is terminal and does not create a smaller fallback layout.
- The recipe layout stamp now includes `packing-per-case-stream;merge-fitting-tails;recipe-fixed-v2`. Independent SHA-256 vector is `0b22e8dac28bbf924e6475408c8da973caf32ddcb32bd0df85e6a811b9c0afc5`; the previous page-layout policy remains rejected by producer and reader.
- The first Runtime14 attempt failed compilation (unused EncodedBlock memory field, compiler101/body0); the field now explicitly owns the grant as `_memory`. The subsequent a9c34819 run terminal output was not recoverable after session closure, so no pass/fail result is claimed from it. It is superseded by the current changed-source request.
- Narrow proof VERIFIED: nextest `d768216e-0113-4084-a922-0ceb7f3f4622`, executed7/PASS7/filtered542/exit0, compile30.94s/body0.063s. Command: common Q prefix + `nextest run -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'test(native::text::small_posting_block::) | test(=native::text::tests::small_tail_join_admission_failure_preserves_both_drafts_and_original_leases) | test(=native::text::tests::cold_case_pair_shares_block_with_distinct_bound_slots)'` + the existing single-thread/no-tests-fail suffix. It covers independent wire/decode, distinct child remapping, exact16384 bytes versus16385-byte decline without mutation, and Original/Work first-denial custody.
- Runtime private tests now require fixed129->2 and300->3 blocks at64/8MiB, every term/case/owner/DF/TF/position, artifact equality and reopen. The second-block post-confirm fault requires Unknown(original operation), unchanged published view, all term memberships absent before/after reopen, then resume of that same operation and full membership/reopen equivalence. These added bodies remain NOT_RUN until a valid owning request terminates.

**Integration compile FAILED / bodies NOT_RUN:** the combined Lexical+Runtime request exited101 before nextest discovery due five current RepoMap `file_map.rs` errors: unused `SourceChange` import(:7), missing `PathKey::as_slice` at589/627/631, and borrowed-after-move `plan.input` at437. Admission was immediate, held114.777s; no behavioral failure or passing test is inferred. Observed RepoMap SHA `42649899767df60fb8aebd4b50b1525b15178f6131d291e132e68bac418c8bf1`. This foreign writer hunk is preserved. Runtime/Kernel/new release/SDK require its source-ready correction. The independent Lexical410 scope is requested on the same existing lane; it cannot replace the blocked integration scope.

**Lexical owning VERIFIED:** nextest `5a7d37c7-4f77-4fc1-bc87-08aa0cbe2f41` executed410/PASS410/filtered139/exit0 (body14.047s). Final review then funded the new Top key-lease array/metadata with its own lifetime grant; repeat `5d5e7a75-181d-4aeb-8045-8fbaa68eab48` executed410/PASS410/filtered139/exit0 (compile5.87s/body13.634s). Command is the previous Lexical408 owning selector unchanged. Final Text SHA `cb82c74e3965a6bdc225725737089f6435e153a7ad8f0c30d835ffe970bb7756`; codec SHA `a2c508399459a25faa49e6d8d1fb3591b63084eff3bbd9c39b7253aefa3103d7`; Runtime private leaf SHA `9272b65e9a267d0152bfcf8a925efacd06c59df6effae712afff0e26541da00f`. Formatting and tracked diff whitespace checks exit0. Runtime/SDK status is not inferred from these410.

RepoMap's independent owner subsequently corrected the five observed compile sites (new SHA `f0e4ab3574f849b686ef1866c212afc51d44ca09c3b1b98304ae42c8d399c0e6`). Text did not edit that file. The same previously failed combined command is requested again against this changed dependency input.

The changed-dependency retry also exited101/body0: RepoMap FundedPayload.memory unused at file_map.rs:128 and scratch.rs:154 db.execute usize result unused. This is a new compile boundary after the previous five sites were corrected, not a Text test failure. The shared source is still changing; no unchanged retry or release build is started. Runtime15/Kernel7/new SDK remain NOT_RUN for this Small candidate.

**Blocked audit, third consecutive goal turn:** independent Text code and Lexical410 remain unchanged. RepoMap inputs changed to file-map SHA `4270661c99477c8436ded23780a24e210921e938392f421cf9e2a59d6af4a0d0` and scratch SHA `13ceefdb29550f248234a7d8ff531ec77db1f9bcf2533433d871bcc0842e5b3d`, but the two failing constructs remain: `FundedPayload.memory` has no read, and `InventoryScratch` still discards the `db.execute` usize result without a binding. No Rust/SDK process is live. A hash change alone does not justify repeating the same unresolved failure. These are the existing independent RepoMap writer's hunks; Text does not overwrite them. Goal status is BLOCKED pending source-ready dependency correction. Resume with the same combined owning command, then affected Kernel6/UDS1, fresh release pair binding and original SDK. No full-product completion is claimed.

### Historical compact-cursor closeout verification, 2026-11-16

The preceding blocked audit is historical: both compiler sites are now corrected by the existing RepoMap owner. The current user's resumed request was evaluated against live source. Small/Dense producer, receipt validation, reader and final publication are connected; no additional Text correctness defect was found in the focused static review. No new production patch, engine or port was invented for this turn.

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal) | test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

VERIFIED: run `c2c3d5e8-b512-4a59-a982-ca9036ed978f`, 425 executed/425 PASS/738 filtered/exit0, admission317.310s/compile2m25s/body68.860s. This is Lexical410 and Runtime15 on dirty HEAD `ade5fadc1a084fbb81eba831861e996081d80867`, with the three Text-owned source hashes in the current section unchanged before/after. Runtime includes 129/300-term fixed block counts, 64/8MiB Original scopes, complete membership/position oracles, first- and second-block post-confirm failure with same-operation resume, Dense cross-page reuse, and fresh reopen. Formatting and scoped whitespace checks exited0.

F's current integration run `16c936f8-7f9e-4b90-9762-4769cc84ee3a` is consumed as10/10 PASS across4 binaries/158 filtered/exit0, not rewritten as the older exact7 selection. Its actual selectors cover FileMap/Source transitions, Small old pins/reopen, occurrence paths, fixed UDS BM25 and the process Text policy contract. Native Runtime+Harness normal/build/dev tree has zero Tantivy packages; full workspace retirement remains F's scope.

The already-running combined no-default release under F's existing queue was reused rather than duplicated. Its first release session80090 exited0 after456s compilation; the final same-command cache validation session77274 exited0 in1.46s. The first release PID71229 is terminal. Before admission, three ANN-owned inputs changed; the final binding therefore uses `source-before-cache-check.json` and `source-after-build.json`, not the earlier snapshot. Those2799 inputs and HEAD also match `source-after-sdk.json`. The Native tree result has zero Tantivy packages. Daemon SHA256 is `652ffa299d5747c511c0d38cad8cfdaae419e2623dfe8ad1110ce1271a50352c`, helper is `a3eecb0abadbf7b3e747d1566b0e84c3468fcc86daa6dc5b6c4548b4d82f1cc4`; both local files were independently rehashed. This proves dirty-source candidate binding, not signed installed qualification or an independent ANN handoff.

**Historical compact-cursor SDK result: FAILED.** F's existing resource-admitted Record controller PID9037 terminated after both corpora. Caddy590/3272107B and etcd1421/8666217B both refused ResourceLimited/FULL ACK0, so query/reopen was not reached. Exact denials are in the current section. Both saved traces have matching hashes, COMPLETE_DIAGNOSTIC, health[] and open_requests0. `sdk-results-summary.json` records original file30000/Work536870912B/R-W-M1000000/SDK60, unchanged source, diagnostic Record times32.56s/21.67s, and current Off BLOCKED at load31.640137>8/body0. Diagnostic times are not quiet performance acceptance.

**Remaining work classification:** Small/Directory/Dense code, their production wiring and the current owning regression are VERIFIED at their stated scope. Product SDK resource acceptance is FAILED; quiet SDK60 is BLOCKED, not passed. The next Text code change requires a measured, actual work elimination with the existing logical-edit fees and EOF/CAS/independent seal/readback intact. The current etcd refusal at packed_runs.rs213 is the original logical-edit admission, not a packet-copy operation. Latest complete trace parent-chain analysis gives posting-edit lane33 exclusive91263484B and inventory prepare lane46 exclusive57615227B across861 completed owners. Direct239 failed alias-probe work has only a loose upper bound7451156B, not a demonstrated saving; block_shape itself charges zero bytes. This evidence does not justify a fee discount or an unconditional new producer/prepared API. No unchanged SDK retry or speculative production patch was submitted.

### T-PREPASS-COUNT-DATA: authenticated cardinality DATA reuse

Current user explicitly authorizes code remediation and original Caddy/etcd SDK rerun with512MiB and all existing fees unchanged. The first selected real duplication is packed_term_page discarding authenticated added/removed values, followed by packed_cardinalities repeating the same packet SQL query, payload copy and parse. The per-term logical read remains one. Full PostingCursor CAS/count/EOF/prepass validation remains mandatory. Parent owns text_spool.rs/packed_runs.rs/posting_case_pages.rs; the private cardinality test leaf has one delegated test writer. No Core/Storage fee or cap hunk is taken.

The behavioral RED request is packed_cardinality_reuses_authenticated_term_page_without_packet_reread:129 input terms, first64 keys, cardinality reads64/packet-copy bytes0, then64 fixed edits and EOF/proof validation. The request uses the original canonical lane/mutex/jobs2; production has not been edited before this RED.

Planned typed provenance: bounded Original-funded per-page counts bind to a private per-spool Rc source token, key-page digest and sealed source. Different spools and unexpected SQL epoch changes are rejected. Authorized Top writes may advance the source's expected total-change stamp only after a successful write, with a pre-write epoch check; cached cardinalities are still DATA, not posting-map authority. Derived case pages carry counts and reorder them with keys. Manually constructed pages retain the original SQL validation; a mismatched authenticated page must never silently downgrade.

T-PREPASS-COUNT-DATA actual behavioral RED: nextest d233bd2f-7e7f-4a30-93c1-127b2b85e068 executed1/FAIL1/filtered549/exit100, admission208.815s/compile1m25s/body0.030s, fixed reads65!=64. The new count integrity seal includes keys+added+removed+case flag. Its actual new hash input is charged (fixed64-term input2157B), so the previous speculative bytes0 assertion was replaced with the independently framed hash-cost gold; logical reads64 remained unchanged. Narrow run987c26d0-bf82-4349-b835-8fffa71df822 then passed3/3. The five provenance/corruption/case-pair tests were added afterward; an initial E0716 in a test expected-key temporary was compiler101/body0, fixed with a named lifetime binding. Combined run c7e5e7b1-a374-44a3-84cb-d62644e01f4b executed10: count-cache8 PASS, census-skip newbehavior1 FAIL, exact-census fallback1 PASS; filtered547/exit100.

T-CENSUS-SINGLE-FILL behavioral RED is the actual prepare_inventory caller: sufficient Original with fixed ASCII/Unicode group gold still emits census span1/110B instead of0. Its low-admission fixture retains one exact census with independent3083B gold and SQL fallback. The selected remediation is a checked all-Unicode length bound, not a corpus cutoff: indexed case rows P<=2ceil(N/2), case key bytes K<=13N+P; row backing12P. Pinned Rust1.92 to_lower returns [char;3], each UTF8 scalar<=4B, and the canonical cursor emits nonempty runs separated by at least one delimiter byte. The bound is used only after actual Original admission for both packet and arena; a miss drops the unused larger packet then uses the original exact census. The same cursor/fill/sort/output and key/position caps remain; no rate or limit is changed. Primary bound source: https://raw.githubusercontent.com/rust-lang/rust/1.92.0/library/core/src/unicode/unicode_data.rs (to_lower signature).

Superseded initial broad run8f589b09-6af6-4ab2-b37e-e559d18bea67 executed437:429 PASS/8 FAIL/738skip/exit100 after389.198s admission/13m46s compile/125.902s bodies. Runtime15 all passed; eight Lexical failures expose integration seams: two old diagnostics fixtures still required the now-skipped census span; spool-scoped provenance was incorrectly funded by the reader and outlived that reader; empty EOF pages charged an unnecessary45B hash on each repeat; early epoch corruption changed two existing error-class oracles.

T-PREPASS-FINAL-1 supersedes the preceding source-ready: fund the source token from the first validated producer append before packet effects, record its immutable-source stamp after seal, skip metadata allocation/hash for empty pages, and classify changed sealed scratch as CorruptObject while preserving the independent DensePrepassProof epoch contract. Existing cap/EOF/refund/tamper tests are preserved; diagnostics expectations follow the real omitted span, with fallback3083B still independently tested. The optimized map-census build captured the preceding revision (all2800 inputs matched its own before/after); it must not be promoted to this correction without fresh source/binary binding. No rates/caps/golds were loosened.

### T-PREPASS-FINAL-1 regression and bound candidate

VERIFIED: final owning command is the existing `test-workspace-lane` combined Lexical/Runtime lib rail:

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal) | test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

Run `4f9e0f40-8f95-4ad6-a31f-21cd66e43852`:437 executed/437 PASS/738 filtered/exit0, compile5m12s/body110.156s. The preceding focused repair `3abb4149-8b3c-4bf1-aac9-d37c4635a83b` passed19/19, and new-feature run `7b77ced8-474e-4231-94ef-a18328c5ee36` passed18/18. Scoped formatting and whitespace checks exit0. Final source hashes: text.rs `be6fa449c6817250c8279f69e662e09f6819329f6f66f24112891326b98df0e4`, cold_tokens.rs `46fb4292d58ccb02c8d771f0d087b403a7e61734c5c358313108806edfb4ea38`, packed_runs.rs `1696cba0ecdd4c4b51c79f51013b6bad286968f83e3d52ebf29023a2fa9c6a4e`, posting_case_pages.rs `3fdd08d8b2f50ebb30762411a14ec134a23d56cb315c2921cd6b13f5a3ac5cc9`.

Measured intermediate original SDK DATA (compact-cursor versus map-census, not final qualification): Caddy census589 spans/8,353,147B→0; etcd861 spans/15,357,748B→0. Completed-owner count, nine aggregate inventory-shape counters and fill/prepare byte totals agree, with no new SQL fallback. Caddy successful shapes589=Alias384+Direct204+Empty1; etcd861=Alias622+Direct239. The native_text_inventory_prepare/34 inclusive bytes decrease exactly by the eliminated native_text_inventory_prepare/43 census bytes. The separate build_prepare/43 is RepoMap verify_and_seal and is unchanged. This establishes actual removed traversal; it does not certify per-owner identity or final full ACK. Caddy intermediate deadline cancellation and etcd refusal remain failed SDK bodies. The cardinality sidecar additionally avoids the packet reread while preserving logical reads and charging its new integrity seal; the original append logical edit fee is unchanged.

### Original final-copy SDK and next owning boundary

FAILED: existing original `run-original-sdk.py` ran both corpora with immutable daemon `a00a3d738876a3867e266835781226adc9bfbcb1e67bdfa7a34363f8712732f8` and helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`, unchanged Work536870912B/R-W-M1000000/files30000/SDK60. Caddy usage R828013/W479737/B536870700/M639214 +298B map bootstrap-frame denial. Etcd usage R317834/W479594/B536860274/M699544 +17847B original posting logical-edit denial. Both trace ingests COMPLETE_DIAGNOSTIC, health[], open_requests0 and exact trace hashes; full publication failed, query/reopen NOT_RUN. Off is BLOCKED at27.909668>8.0/body0. Candidate paths include `pair-binding.json`, `input-binding.json`, `sdk-results-summary.json` and complete per-corpus traces outside checkout.

Read-only producer→append→packet review found no justified fee-removal patch: `PostingTermKey::copy` is a real memory copy but WorkBytes0; append's first loop preserves logical edit admission before effects; owner/ref packet dictionaries and ordered packet fastpath already exist. Unordered packet recopy is a real possible cost, but current trace has no unordered count/bytes and does not establish a net saving. Storage map bootstrap and earlier Source/FileMap work remain the existing owners' boundaries. Do not introduce a new engine/fee discount or rerun an unchanged failed SDK candidate.

### T-PAIRED-EOF-EPOCH

The independently added regression initially hit compiler101/body0 due to three unused atomic fetch results. Only explicit result bindings were added to that test leaf. Actual behavioral RED `57b54883-d986-42b3-991f-5868b3dcf975` then executed2:1 PASS/1 FAIL/561 filtered/exit100, compile50.34s/body0.025s. `paired_eof_rejects_stale_sql_even_when_packet_payload_is_unchanged` accepted empty EOF after a same-payload SQL UPDATE; the valid Top-write/no-Work/no-allocation oracle passed. Root cause: both CaseCursor eof flags bypass refill, and PostingCasePages::next checked poison but not the sealed source epoch. The minimal owning fix calls existing `check_page_source` before output allocation on every next; it charges no new Work and allocates nothing. No digest/CAS/EOF validation is removed. VERIFIED: full TextSpool owning regression `72852bce-92a0-4a68-87f3-c9c04d60320c` executed86/PASS86/477 filtered/exit0, compile31.03s/body13.325s. Both new EOF oracles pass; scoped formatting/whitespace checks exit0. This is a narrow final-fix result, not a fresh combined439 pass or original SDK qualification.

**Concrete remaining SDK boundary:** final-copy Caddy FULL reaches the155th MapApply batch:26,893 inline values/26,893 child refs/1,549,252 input wire bytes. This failed MapApply consumes60,689,805B while previous154 successful map applies total5,854,066B. Terminal at existing charge_bootstrap_frame. Review sorted bootstrap frame lifetime/copy before proposing a Storage hunk; existing Storage owner retains this boundary. Etcd FULL is before postings: inventory prepare865 spans, diff780; original logical edit admission refuses at packed_runs.rs318. Text census saving is real, but no trace-based net saving from a new packed-writer algorithm has been established. Inclusive/overlapping child Work is not summed or subtracted into fictitious exclusive costs. Exact selector names matter: native_text_inventory_prepare/43 census is removed; build_prepare/43 RepoMap verify_and_seal is unchanged.

### Final EOF release dependency, source-ready required

The same existing no-default optimized daemon+helper command was attempted after the EOF guard, candidate `/private/tmp/qidx-sdk60-text-copy-ym3jb7a9/eof-guard`. Build FAILED compiler101/body0 after24.756s, not a Native SDK body. During the interval the existing Storage owner changed map.rs/diff.rs/node_wire.rs/original_lookup.rs/value_codec.rs and added map/retained_value.rs. Its pub(super) RetainedMapValue::Many exposes private ManyChildren (`retained_value.rs:25`, definition:11), rejected by deny(private_interfaces). Text does not overwrite that owner's production hunk or loosen the warning gate. The new snapshot contains2802 inputs; preserve failed release log/result outside checkout. Final EOF SDK is BLOCKED by this source dependency; once corrected, recapture the exact admitted source and run the same release/SDK chain. Do not repeat this unchanged compiler failure or promote the earlier paired-cursor binary to the guard revision. Current goal remains active; the SDK resource failures are unresolved.

The Storage owner corrected ManyChildren to pub(super); Text did not edit that production file. The specific compiler cause changed, so release r2 was admitted with a fresh2802-input before snapshot and the same canonical command. No production input drift was observed at the subsequent read-only check. This supersedes the preceding source-ready blocker without deleting the failed attempt; final SDK remains NOT_RUN until the new pair is bound.

### Final guard pair bound; original SDK in flight

VERIFIED build scope only: release r2 exit0/114.572s, before/after2802-input set/hash and HEAD equal, no compiler-error bypass. New immutable daemon `/private/tmp/qidx-sdk60-text-copy-ym3jb7a9/eof-guard/quanta-index-searchd`:14,648,832B/SHA256 `c394552525b0051e3aa3de3c3a492f882ef1f9147e0e0628419f935d399a1f1b`; helper2,890,928B/SHA256 `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. This pair includes the existing Storage owner's compact retained-value work in addition to Text EOF guard. Do not attribute a new SDK prefix delta entirely to Text. Static review found no logical tariff/cap deletion: bootstrap frame/backing still charges actual size and live copied bytes, multi-child Arc has Original backing, public full-width materialization is charged. Storage runtime/unit qualification is not established by this static review.

Original Off preflight on the new pair is BLOCKED at load27.558594>8/body0/exit1, no override. Original Record was then admitted and runs both corpora using the same unchanged runner/helper/inputs/caps; results pending. Build compilation is not product SDK acceptance.

### Final guard original SDK terminal result

FAILED: original runner Record exited1 after both real bodies and returned the shared mutex. Caddy37.55s terminal R874650/W488784/B536848780/M652038 plus38312B at Catalog managed_install.rs108. It completed all155 MapApply calls; the previous copy candidate's155th map apply had failed. Etcd20.87s terminal R317894/W485357/B536770775/M699544 plus139551B at packed_runs.rs835;790 owner diffs completed versus779 in the preceding candidate. Both FULL ACK0; query/reopen NOT_RUN. Both diagnostic ingests COMPLETE_DIAGNOSTIC/health[]/open_requests0, with exact saved trace SHA verified. Immutable binaries and diagnostic input hashes stayed fixed. Original input manifests/all files/commit/query tokens were rechecked after execution.

Correction: etcd835 is sort_work admission before sort and measured_copy864, not an executed second-copy charge. Removing only the second copy leaves that refused charge. No packet-level counters establish how much prior unordered recopy could be saved. Offset-index migration would require writer/reader/seek/merge/digest/fence/bijection validation changes, Original-funded bounds and measured index/SQL/read amplification. It remains conditional, not adopted or mandatory scope. No fee/cap discount or unverified packet codec was written.

The final SDK after snapshot has2802 inputs but seven Storage-owned paths differ from source-before-build-r2; `sdk-results-summary.json` explicitly records source_inputs_unchanged=false and every path. Exact source before/after build matched, so immutable candidate execution remains observable, but it is not current-source acceptance. Those owner edits are preserved; Text does not overwrite or repeatedly rebuild an actively changing dependency. Current Text source/tests remain ready at their stated scopes; full-product512MiB/quiet60 acceptance remains failed/blocked, not complete.

### Goal continuation: current owning rail and packet cost observation

The preceding goal turn made code and execution progress, not completion. Current HEAD remains ade5fadc1a084fbb81eba831861e996081d80867. No earlier Rust/SDK process remained live at continuation preflight; Storage production files then matched the prior SDK-after snapshot. The next combined current Storage+Text+Runtime lib rail was actually admitted, but exited compiler101/body0 after236.938s at new Storage cfg(test) retained_value_cas.rs34: Arc::clone type coercion. A test-only self.0.clone() correction preserved all grant/CAS assertions. Storage-only lib then exited compiler101/body0 after40.169s at four unused atomic fetch results in that same test; explicit return bindings and file-local formatting fixed them. No Storage production hunk, cap, warning gate or oracle was changed by Text.

T-PACKET-COST-OBSERVATION is selected before any offset-index codec: current traces cannot separate completed sort tariff from full row-body recopy. Existing DiagnosticContext/WorkBudget APIs can report accepted Work in NativeTextPostingEdit lanes36(sort),37(actual recopy),38(unordered packet/SQL outcome) without Core/schema edits. Off must run the original operation directly, and Record must preserve Work/result/context. Existing 5-row fixed fixture independently requires365B sort and50B copy, with identical CAS/order/EOF and no new spans for the existing ordered fastpath. The new test-only oracle was added to ordered_packets.rs; its first attempt was compiler101/body0 from one unused DiagnosticSpanIdV1 import, removed without changing the gold. The exact selector RED is currently admitted/queued on the existing lane/mutex; no production observer has been implemented before the behavioral RED.

No existing counter means internal packet row count; SqliteRows and PostingMutations must not be repurposed. Body-copy observation is useful actual work evidence but alone does not qualify index/read/SQL net savings or product acceptance. A new private packet format remains conditional; existing logical sort charge is preserved.

### Packet observation RED and current Storage/packet GREEN

The first exact selector omitted the posting_packed parent module and run5896668d-a948-4ed7-b044-1e40c9b4abc0 executed0/filtered564/exit4; this was NOT_RUN, not a behavioral RED or pass. The corrected unique selector `test(unordered_packet_observation_separates_sort_and_real_row_copy)` actually executed1/FAIL1/563 filtered/exit100 in run560e24e4-decb-40be-ad02-b5f8d10c9b6a (compile0.59s/body0.063s), expected three packet phases versus actual0.

Parent then added lexical-only observe_packet_work and factored the existing unordered body into finish_unordered. The original sort_work charge, strict sorting/duplicate checks,128KiB memory grant, measured_copy, dictionaries, digest, SQL insert and reset order are preserved. Diagnostic Off calls the original closure directly; failed span admission records trace health and still executes the original operation. Scoped contexts restore on result/err. NativeTextPostingEdit/36 is accepted sort Work; /37 is actual body recopy; /38 is the entire unordered finalization and SQL outcome. /38 includes /36 and /37 and must not be added to them or called SQL-only cost. No Core counter, wire, tariff, cap or published policy was changed.

VERIFIED owning command: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-storage -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'package(quanta-index-storage) | test(native::text_spool::tests::posting_packed::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`. Run961e4db1-2446-4dd7-a7d4-abeba87ba925 executed240/PASS240/542 filtered/exit0, compile1m47s/body30.609s. This includes current Storage lib and new observer365B/50B independent gold, ordered fastpath absence and identical Off/Record Work+CAS/order/EOF. Actual corpus packet-copy measurement remains NOT_RUN until a new bound SDK pair; no claim that observability removed more bytes or fixed SDK acceptance.

### Object-only FULL row-count inference (not a new counter)

Source inspection establishes the original native_corpus FULL request precondition: `scale/native_index/corpus.rs` creates a new workspace, reconfigures only profile on genesis, then stages every SourceAdvance owner with expected=None/next=Some(source_ref). Text push_change checks absence of the prior contribution against admitted prior source, and PostingEdits::push accepts only Option<ObjectRef>, rejecting unchanged values. Thus reached FULL posting rows are one-sided None→Object, not arbitrary inline or warm updates.

For those completed unordered initial packets only, existing formulas give sort Work per row=71+term.len and compact copied body=8+term.len, so `(native_text_posting_edit/36 WorkBytes - /37 WorkBytes)/63` derives the row count. Require exact divisibility, both child spans OK, parent packet/38 OK, and ancestry under that fresh FULL request. The fixed5-row oracle gives(365-50)/63=5 independently. This is a source-bound derived count, not a directly emitted counter and not authority to skip validation. Warm Object→Object updates have difference94; mixed typed rows only bound ceil(D/94)<=N<=floor(D/63). Inline test inputs invalidate this object-only formula. The original FULL source preconditions must be rechecked with runner/helper/source binding.

The added write-refusal observer oracle keeps accepted bytes575=365sort+50copy+160dictionary, exact Writes denial, unchanged SQL epoch, same Off/Record usage, restored parent context and successful fixed cursor/EOF retry. Current full Lexical/Runtime lib rail is live; this new test is not yet declared passed. Storage all+packet240 pass is separate scope and is not upgraded to current full441 or SDK success.

### Current full Text/Runtime regression and new bound observer pair

VERIFIED: current combined Lexical/Runtime owning selector executed441/PASS441/738 filtered/exit0 in runa3b61279-86d0-45de-a13e-6f99f45c5f12, compile4m57s/body132.252s. It includes the new SQL-write-refusal/retry observation oracle: accepted575B, exact ResourceLimited/Writes, unchanged SQL epoch, restored diagnostic parent, ordered/CAS/EOF retry, equal Off/Record producer/denied/final Work. This replaces the earlier separate437+86 current-source description for this Text/Runtime scope. Storage all+packet240 pass remains separately scoped and is not added to441 because packet tests overlap.

VERIFIED build scope: `/private/tmp/qidx-sdk60-packet-work-r4dxzzgz` no-default optimized daemon/helper build exit0/201.241s (compiler3m19s), exact before/after2803-input set/hash and HEAD equal. Immutable daemon14,648,832B/SHA256 `9e40b0045b5b593fc66d11e5f12fcdc6d3bb02715fd87025b9f8aa4e62afbb4c`; immutable helper2,890,928B/SHA256 `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. Original driver/corpus/gold/SDK60/Work512MiB/R-W-M1m/file30k and Original scopes remain unchanged. Off preflight BLOCKED/body0 at host load32.425781>8 and14 foreign Rust processes, no override. The existing runner Record is now admitted for both original corpora; body/copy measurements are NOT_RUN until its actual terminal output is recovered.

### Packet observer original SDK terminal and measured format decision

Candidate `/private/tmp/qidx-sdk60-packet-work-r4dxzzgz` Record runner exit1 and mutex returned. Both real original bodies failed `REQUEST_DEADLINE_EXCEEDED` at native_corpus_publication_status (Caddy67.58s process, etcd79.10s process, each SDK60 unchanged). Full ACK0/query/reopen NOT_RUN. Caddy observed usage386,125,553B and etcd496,323,772B, both daemon first_denial cancelled at Storage object.rs865. These are deadline failures during contended Record execution. Off was preflight BLOCKED/body0. Exact source2803/HEAD, both binaries, diagnostic inputs and original full corpus file hashes/commit/query tokens match through SDK.

Caddy raw footer invalid:229,039 attempted,228,666 accepted/written,373 dropped,4 open. Missing begin for span9501 triggers strict parser error at thread_cpu_ns seq50985. Preserve trace as failed diagnostic, do not adjust schema or salvage savings from orphan events. Etcd canonical ingest COMPLETE_DIAGNOSTIC, health[], open0, trace digest6d6855b796d65a9e76b19486c7e257050c9b14bf2c6d1a701aee3594fa32d8e0. Full request span4553 contains117 completed unordered packet/38 parents, each with checked successful sort/36 and recopy/37 children. Sums: sort16,365,685B, body recopy3,304,273B, inclusive packet27,188,860B. The original FULL object-only source and exact per-packet identity `(T−B)/63` derive207,324 rows. Existing insertion/dictionary charges recover57,440B dictionaries; checks pass for32B units and128KiB packet bound. Hypothetical u32 offset index write/copies/first read cost16*N, giving net −12,911B before added readback validation, fragmentation, or later reads.102/117 packets are already negative. Keep current packet wire. Frame these as the completed observed prefix, not extrapolated full-corpus savings or a bytes refusal.

### Borrowed term packet read

The existing term metadata consumer copies the SQLite payload into a temporary Vec, scans it once for order/count/EOF, rescans it for first/last fences, then decodes selected terms and copies every selected term into a cursor scratch key. A 64-term packet of nine-byte keys contains 1,770B, and the full page copies another576B solely to advance a cursor that returns at the page boundary. All 64 returned owned keys and their count integrity hash remain necessary. The scoped reader will borrow the SQLite row payload, verify both fences during its original order scan, and keep the packet inside one callback. SQL EOF is checked before returning tentative owned data. Cursor bytes are copied only when needed for a subsequent packet query. A manually assembled cardinality page can resume from the authenticated expected key it just matched. Preserve SQL read1/4096B per lookup, logical cardinality reads and Original allocations for the returned keys/sidecar.

The two fixed regression selectors are `sealed_term_page_read_charges_only_packet_lookup_and_count_integrity` and `manual_cardinality_page_across_packet_fence_charges_two_lookups`. Expected old versus intended deltas are B8599→6253/R1 for the sealed 64-term page and B11750→8192/R4 for the adjacent manual 063/064 lookup. The pre-change owning RED request is admitted through the existing execution queue and is waiting behind a live Native process build. These are source-derived golds until the actual body runs. Directory alias also has a separately observed owned page key copy before posting staging. Current traces only bound that duplicate by the 4,713,524B sum of all unique inventory keys in the observed etcd prefix. Its borrow API and before-EOF callback effects remain separate scope.

Actual term metadata RED `167ed586-2a06-4dba-a444-8f0055679451`:2 executed/2 FAIL/565 filtered/exit100 after441.935s admission and46.73s compile. Observed exact8599 vs6253 and11750 vs8192, with expected read checks preceding byte failures. The new borrowed TermPacket validates both SQL fences in the order/count/trailing-byte scan, then consumes within a private HRTB SQLite callback. SQL footer is checked before returning owned page/deltas. Actual BLOB backing and redundant cursor copy are removed. Original SQL and logical cardinality charges, source stamp, key/delta integrity and independent posting proof remain. TextSpool run `99268e09-8a30-4051-b8fa-eb59f15c691c` executed90/PASS90/477 filtered/exit0, admission90.858s/compile49.30s/body19.864s. Verified physical-copy reduction2346B/3558B on fixed fixtures with same read counts. This does not claim original corpus completion.

### Borrowed Directory alias descriptor rows

The existing canonical 514-term Directory/reopen fixture measures page-cursor and alias visitor Work independently. Both perform two physical child gets; 514 fixed five-byte keys and two five-byte fences require independent seal tariffs `514*(5+8)+2*(5+43)=6778`. The reference owned MapPage cursor copies2570B of keys. The new alias path borrows the authenticated same keys, so expected alias minus page Work is4208B. Actual RED `a9ae158c-5a7d-4ea8-ad5e-e179ae80907f` executed1/FAIL1/566 filtered/exit100, compile39.89s/body0.052s, old6778 !=4208 after both read-count assertions passed.

Production Directory Cursor now lends each fully checked descriptor and same-block key/count to the cold alias visitor without reserving/allocating/copying a temporary public MapPage. Its existing advance still reads and validates descriptor order, packed child, interval and fence, or leaf count. The alias visitor keeps first-child comparison, per-descriptor and per-key seals, term/anchor bounds, EOF and commit ordering. Generic owned MapPage and its paid copies remain for callers that require ownership. Cursor cannot advance while the borrowed descriptor is live. The cold caller stages only disposable posting edits before final seal; its existing failed-window invalidation remains. The current combined Lexical/Runtime rail is live. Original SDK for this new production revision remains NOT_RUN.

### Current combined dependency status

The combined Lexical/Runtime request exited compiler101/body0 after455.515s holding the shared admission. Runtime test `publication_sqlite_vfs/tests/read_only.rs:847` imports CommitSha from the wrong module (E0432). Production `native_recovery.rs:332` omits the new LegacyHistoryBootstrapRequired refusal variant (E0004). Existing NC-D boot packet assigns the recovery receiver to D; its current owning Runtime request is live. Text does not take over the typed Recovery wire mapping or treat compilation as a test body. Current Text files match the captured borrowed-term and borrowed-directory hashes. Lexical independent scoped owning nextest is submitted on the existing shared lane, ahead of source-bound final release and original SDK reexecution when the production dependency compiles.

The owning Lexical-only request terminated admission124/body0 after600.003s. The admitted History/Runtime request remains live (PID20089 and actual Cargo child20193); do not count a queue receipt as a test. The production exhaustive refusal arm is now present in native_recovery.rs with explicit LEGACY_HISTORY_BOOTSTRAP_REQUIRED code. The test-only CommitSha root import was still unchanged at this observation.

Current existing canonical release candidate `/private/tmp/qidx-sdk60-scratch-projection-r3-jmmd981i` is owned by the shared integration queue (PID30248 child of build-bound.py PID30164). Its source-before snapshot binds2803 inputs and exact current packed_runs.rs/directory.rs/occurrence.rs plus current native_recovery.rs. Reuse this live request by inspecting terminal exit/artifacts and source-before/after binding. Do not launch duplicate optimized build or claim SDK based on this queued artifact. Restart Lexical-only verification after a genuine execution admission transition. Whole current Text/Runtime and original-sdk acceptance remain open.

The existing scratch-projection r3 release reached terminal admission124 after600.647s with zero compiler artifact events and no binaries. Its build-result lists7 source paths changed while queued and diagnostic input drift. This is BLOCKED admission with SDK NOT_RUN. Both specific Runtime source defects have now been corrected by their existing writers: explicit LEGACY_HISTORY_BOOTSTRAP_REQUIRED terminal arm and `quanta_index_contract::lex::CommitSha` import. Current combined owning nextest session76029 is submitted after those changed inputs. Its execution-lock wait is1200s to allow the already-live product/history jobs to return the same shared mutex; SDK60, Work caps, fees, jobs2, selectors and Original contracts retain their original values. No concurrent paired release is live at this capture.

The shared integration queue submitted replacement candidate `/private/tmp/qidx-sdk60-projection-admitted-vtxkvcin` (live resource controller PID74730). Its build script captures source only after canonical admission rather than before waiting. At observation the root has only the script and no approved candidate binaries. Retain this existing release request and inspect its terminal plus actual Text hashes, before/after and SDK custody. Current Text combined request76029 remains live behind admitted Q ownership; no duplicate pair or SDK has been started by Text.

The admitted optimized build `/private/tmp/qidx-sdk60-projection-admitted-vtxkvcin` finished compiler exit0/191.560s with both pinned binaries. Before/after2803 workspace inputs differ only in `crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs`, a test module outside either release target. The recorded `release-target-binding.json` verifies the release target dependency set and hashes; whole-worktree build snapshots differ. Daemon SHA256 `02f923cf5ef991d86784f162682203f92cce7c7810dbba6acc7e4925cc61346c`, helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`; rehashed copied artifacts match. Both Text production hunks and their test leaves match the actual admitted source-before. Original driver SHA is unchanged; Caddy590/3272107B and etcd1421/8666217B source manifests/all file hashes/commits/query tokens rechecked. Existing shared original Off→Record SDK controller PID53466 has been submitted on the same mutex with1800s admission wait. At this observation it is live and waiting. No SDK test result yet. Text combined session76029 remains live in the same queue.

The corrected-input Text+Runtime combined session76029 reached terminal admission124/body0 after1200.012s. The independent release ran and matched both production target dependency sets, but no current combined regression body has executed. The existing original SDK controller53466 is still live at this observation; monitor it rather than duplicate SDK or repeatedly submit the same owning tests behind the same active lock holder. Resume the Text+Runtime owning selector when the queue has an actual execution transition.

### Bound borrowed Text / scratch projection SDK terminal

The original shared SDK controller terminated after both actual Record bodies. `sdk-exit-codes.json` is off1/record1. No Off output roots and no corpus full result/reopen outputs exist. Both traces pass canonical diagnostic ingestion with health[] and open requests0. Caddy first denial bytes536848332+58652/536870912 at Catalog managed_install.rs108 (R878980/W497088/M660342). Etcd first denial bytes536771969+138267/536870912 at packed_runs.rs895 (R317984/W494194/M699544). The latter is the original sort_work admission before sorting and before a new body copy. Both BuildPrepare ResourceLimited / FULL ACK0. The preflight failure is separate from SDK Record byte refusal. No timing/cap overrides or new rates were used.

Whole workspace before/after Release changed only ANN integration test ann_quality.rs; before/after SDK changed only History integration test history_native.rs. These are outside both selected release target dependency graphs. Verified binding is limited to the optimized daemon/helper plus original SDK inputs/driver, not a frozen full-worktree test suite. Code/provenance and first refusal come from the actual release-pair artifacts, not the prior contended deadline sample. Existing current Lexical owning request64152 was submitted after the production release+SDK moved into actual execution; this verifies the isolated Text/Directory scope first. Runtime15 and complete NTX4 product acceptance remain outstanding.

R4 Caddy compared with the prior eof-guard refusal: owner prepares593 and diffs590, all155 MapApply/3 completed with the same input shape. Inventories/diff prep Work falls1,107,087B and postings/35 prep falls1,752,865B. The same final256-input reservation reaches2,859,504B more accepted Work before refusal, so BuildPrepare inclusive total differs only448B. Both are partial reservation prefixes; per-object payload/edge counts are not exposed and do not establish a fee or extra-work regression. R4 Caddy completed114 unordered posting packets and actually recopied3,162,301B. A u32 index costs at least16B/row through initial creation/SQL write/first read, net −69,827B for202,008 rows.

R4 etcd compared with the prior packet-work deadline prefix: same861 completed owner shapes (alias622/direct239). For the first606 completed inventory diff spans in producer order, observed Work fell from124,366,408B to121,343,195B (−3,023,213B). The complete terminal prefix has803 completed owner diffs plus one error. First117 completed packet Work is unchanged; R4 completes166 (294,152 inferred rows, physical body recopy4,664,794B). The167th packet is refused before sorting/recopy. u32 index net through initial first read −41,638B on those166; u16 absolute byte offsets do not represent all128KiB packets and apparent savings omit reader validation/repeated reads/fragmentation. The observed higher progress is not a full corpus Work total or proof that one hunk caused every measured delta. Keep the current packet format and original tariffs.

### Paired case cursor duplicate digest

Source review isolated an actual local rehash in `CaseCursor::fill`: the freshly constructed `packed_term_page` already computes the count seal, then `fetched.check_counts` hashes the identical private page before any consumer or mutation. Candidate change removes only that immediate verification pass. Preserve the sealed-source epoch checks, key/surface ordering, original producer seal, Rc source equality on `push_from`, result paired seal, cardinality-side integrity check, and dense original-stream proof. A fixed two-case oracle was added to `paired_eof_epoch.rs`, selector `paired_page_reuses_untouched_case_count_seals`, test source SHA256 `d40726aea041a5cac663beeb76ee6e4470ac238a3aa70e7d1f823f6bb5d2c1e2`. It seals the producer before snapshot, requires R5/B20746 from five SQL calls, two 74B producer seals,103B paired result seal and three5B cursor copies, plus fixed keys and DF1. Old expected B20894 is source-derived until executed. The live Lexical session64152 was still queued and its Rust body had not started before the test was frozen. No production digest change has been made yet.

Actual case digest RED and Directory owning evidence: nextest `6133ff28-3a4e-44b4-9ef8-4cf0c1b9873f` admitted after715.024s and executed429 across Lexical lib, 428 PASS/1 FAIL/139 filtered/exit100, compile34.02s/body22.340s. Sole failure `paired_page_reuses_untouched_case_count_seals` shows20894 !=20746 with reads5 already matched. It isolates exactly148B (two unchanged74B digest passes). Directory's borrowed alias/reopen fixture and term BLOB fixtures were within the passed428. This is not a429-pass claim.

Applied the one-hunk `CaseCursor::fill` production change: reuse the freshly sealed cursor-owned page, retain source/epoch/case ordering and `push_from` provenance checks, output seal and ordinary cardinality validation. No original logical charge is changed. The prior R4 original SDK failed before this edit and does not cover the new revision. New Lexical owning session8551 submitted the same exact429 scope on the shared mutex after RED, production source frozen. Runtime owning scope and source-bound original SDK for the final revision remain outstanding.

### Final case-seal release request

The post-fix Lexical owning request8551 is still live. No optimized daemon/helper build or original SDK request was live at the submit check, so the changed production revision was submitted once using the existing admitted build-bound producer from the previous candidate. External root `/private/tmp/qidx-sdk60-case-seal-n24abmut`, session94796, same canonical mutex, wait1800s. It takes the source snapshot inside admission and pins both compiler-emitted artifacts before release. The expected case-source SHA256 is `5e9302f129213d94901f62c554b6ba3a35f35aee42f57db97ae7a5b4e693f8b4`. The source-bound optimized build and its original SDK are NOT_RUN until their actual execution evidence. No new lane, build mechanism, fee, Work cap or SDK timeout is introduced.

The case-seal release request94796 terminal exit0 built pinned no-default Native daemon/helper in248.175s. Canonical admission168.830s, held249.979s. All2803 workspace inputs and HEAD match before/after build, as do diagnostic inputs; source includes the exact case hunk. Daemon SHA256 `038ad043d29d62574d3578fd818eb358ebc4936b94ce3ed260f382fd2d553ea0`, helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. Existing original runner with the fixed source manifests/input binding was submitted as session5997 after checking no duplicate live original controller. It reuses the admitted Off→Record wrapper on the same mutex. This candidate includes separately owned Literal and recovery changes, so any SDK delta cannot be attributed solely to the Text rehash hunk. Tests8551 and SDK5997 remain live. Actual bodies are pending.

Catalog denial follow-up (read-only): `reserve_attempt` calls the canonical registration check during child preflight (`managed_install.rs:791`) and again before mutation (`:807-814`), with a full children clone in the canonical helper. This is an actual memory copy repeated within the Catalog-owned transaction. It happens after `charge_object` and does not shrink the existing admitted payload/edge tariff. `encoded_object_len` writes to a length sink, so there is no transient JSON Vec to remove. Same-transaction child proof reuse is already wired. That copy is a separate CPU/RSS opportunity for the existing Catalog owner, with no supported claim it fixes the 512MiB byte refusal. No new Text or Catalog production patch follows from this finding.

The post-fix Lexical request8551 reached terminal admission124/body0 at1800.018s. It adds no case-seal GREEN or429 PASS evidence. Original SDK session5997 remains live, controller PID1513; at this observation a Kernel/Runtime integration command owns the canonical mutex with actual Cargo/nextest descendants. Preserve the pending SDK and avoid re-submitting the Lexical body while the same contended admission state remains. Before retesting, verify a real admission transition and the current Text source. Candidate SDK-before snapshot and build-after match all2803 inputs, and an independent workspace check during the wait found no drift. Current completion still requires actual post-fix unit/integration bodies and original corpus acceptance.

### Case-seal release original SDK terminal

Original SDK session5997 exit1 after actual admission989.240s and held117.219s. Off preflight blocked by host load18.807129>8 and6 foreign Rust processes; zero Off bodies. Record executed both original files and captured typed Work refusals under file30k, bytes536870912, R/W/M1m and SDK60. Caddy process68.08s, Work R881148/W500492/B536860391/M663746, attempted52768B at Catalog managed_install.rs108. Etcd process26.01s, Work R317984/W494194/B536771969/M699544, attempted138267B at packed_runs.rs895. Both FULL ACK0 and search/reopen NOT_RUN. Process wall times are not quiet SDK timings. Etcd usage and denial match R4 exactly, consistent with the last case rehash fix occurring after the first refusal. Caddy capture footer invalid, attempted271909, accepted/written267114, dropped4795, open2; strict parser reports owner-span loss. No whole-prefix Caddy Work delta is attributed from the invalid trace. Etcd COMPLETE_DIAGNOSTIC, health[], open0, trace digest rechecked.

Canonical workspace snapshots before/after build and before/after SDK all match exactly2803 files and HEAD. Diagnostic inputs, both immutable binary hashes, both original manifest/commit/query/file bindings were rechecked. The combined Lexical+Runtime no-default owning command (same Text/TextSpool/Occurrence/Literal plus Runtime dense-install selectors) entered the common mutex immediately, session32393. It ran445 tests in92.862s with444 PASS, one FAIL,739 skips, and exit100. The only failure was F's `native::literal_blocks::tests::shared_owner_prefix_wire_preserves_full_logical_keys`, so the paired count seal, Directory alias, term packet and dense-install tests in this selection passed. The terminal summary preserves the failing name but no assertion text. With the mutex free after the separate F release attempt, the same-package exact Literal selector was admitted in session58481 to obtain the failure or confirm the current hunk. The F build result at `/private/tmp/qidx-sdk60-literal-v4-0ni8ypt2/build-result.json` is exit101, no pinned binary, SDK60 NOT_RUN. It supplies no new SDK result.

F then recorded the current Literal V4 nextest `8768d533-71d4-4d3f-873d-4e7236b7adbe` at41/41 PASS including the exact failing selector. The obsolete duplicate diagnostic58481 was stopped during dependency compilation with exit143/body0 to release the mutex to F's existing release retry. F's retry includes the Catalog SQLite INTEGER read fix and is bound at `/private/tmp/qidx-sdk60-literal-v4-ci2rwa01`. It reuses the existing build + original SDK custody and preserves original Work caps. No new optimized build or SDK runner is started by Text while that controller is live.

The integrated Literal V4 retry compiled in102.128s with Cargo exit0 and both pinned artifacts. The candidate controller stopped before SDK because five workspace inputs changed during the build. The daemon's release dep-info includes four changed production Kernel paths (`lib.rs`, `boot.rs`, `update.rs`, `update/history.rs`) and excludes the fifth test-only Runtime file. The V4 input hashes match, but this is material daemon source drift. The candidate has SDK60 NOT_RUN and R5 remains the latest executed original SDK. F's Literal persisted-seal rail is admitted on the common mutex.

The original driver subsequently executed Record only for both corpora on the immutable compiler artifacts in `/private/tmp/qidx-sdk60-literal-v4-ci2rwa01`, daemon `b33b3a2c36cc93d114d952530fdc6814c9b702a495e924235b97bdef20f85a8e`, unchanged helper. The command unset Work and contended-timing overrides. Both traces are COMPLETE_DIAGNOSTIC, health[], open requests0, and their computed digests match the ingests. Caddy reached the same155th Text map batch and input census as R4, then refused B536829657+48028 at Catalog managed_install.rs108 with R916370/W523129/M686431. Etcd advanced from803 to862 completed inventory diffs and reached a fifth inventory map batch, then refused B536870908+68 at scratch edit_page.rs112 with R333371/W534085/M868115. Both FULL ACK0, query/reopen and Off NOT_RUN. This diagnostic does not resolve the build-time source mismatch.

The complete Literal preparation stage fell from51,151,663 to42,026,185B for Caddy (R4), and186,711,399 to143,008,445B for etcd (R5). Fresh persisted child verification fell from92 to53 children and12,001,971 to8,201,871B for Caddy, and384 to198 children and52,349,516 to34,119,623B for etcd. Child verification and other inner stages are already in the preparation totals. Caddy consumed about10.16MB more in the same failing MapApply/5 prefix, consistent with progress after the Literal saving. Etcd's four completed global inventory checks retain the same35,170 reads/9,137,041B, while the fifth batch stops before a completed global check. The minimum68B refusal is before construction of a retained edit, not an executed68B copy. Keep the existing Work charges and compare producer readback, local per-root construction and global input custody before changing a cursor or storage contract.

`InventoryCursor::next_edit` reads one bounded SQL page and retains `after` by `Arc` clone. The root-specific plan and global cursor in `entry_patch::apply_inner` do decode the same frozen rows separately. Their stamp comparison, global order/CAS and EOF checks, and fresh installed-directory readback are required. Eliminating either scan requires an independent replacement for its proof of the original ordered stream. First freeze a fixed Work RED plus wrong-root, reordered, malformed, and stale-epoch refusals. Keep slot, anchor, root, value, and installed-object custody; dropping the global scan or its charge with no equivalent proof is not a candidate. Separately, compacting per-row Arc/Vec into owned page backing belongs to the shared Core/SQLite contract, and copying an already canonical row wire could save decode/encode CPU while retaining the same owned bytes and Work admission. The current trace does not establish a net Work gain for that separate change.

### Frozen compact inventory verification

Preimages `entry_patch.rs` a0d3d481 / `entry_map/directory.rs` b0eff5e8 were rechecked before editing their Text-owned digest/producer functions. A descriptor seal now authenticates the existing `OrdinalMapEdit::next_wire` instead of re-encoding the identical `MapValue`. `RootInput::next` no longer decodes object values immediately after `Edits::push` has already checked and sealed them. Directory producer CAS/presence tests use the canonical cursor's optional wire slices before the same descriptor/object checks. The transcript, serialization, Work charges, per-root/overall scans, stale-epoch and persisted checks remain intact. Canonical no-default Lexical `test(native::text::)` nextest `d025081b-ed8f-4ed7-ae77-41b477b9bba3` ran268/268,301 skips, exit0, compile90s/actual bodies5.978s. Source hashes `entry_patch.rs` 46b46d82 / `directory.rs` 83c07e9b. The original SDK has not been rerun for these two transparent changes.

The R6 raw trace parent chains show zero descendant spans from each of the four completed global inventory verification scopes on each corpus. Those scopes are compact verification candidates, with exactly15,089R/3,940,716B on Caddy and35,170R/9,137,041B on etcd. Those totals include untouched digest/admission/readback and are not estimates of removable work. The next change retains the independent ordered global SQLite read, but uses borrowed row values for digest, CAS/order/count/EOF and fresh persisted readback, with one bounded previous-key copy. Compact verification does not emit a retained edit. Tree application and unbound generic callers retain the existing owned cursor. The fixed two-descriptor production fixture already checks canonical persisted child reads and reopen; its new isolated existing phase oracle expects2 reads and4356B (4096 query + two existing125B descriptor seals + two5B previous-key copies). The focus command is canonical `test-workspace-lane` no-default Lexical lib, exact `cold_multiblock_alias_prepare_visits_each_descriptor_and_child_once`, session77907. The assertion is pending actual behavioral RED. The borrowed compact production change is pending until that result.

Actual RED `8cb09b5c-1ef5-4b88-9fa3-0cce436a810d` executed the exact production-batch fixture, exit100 with one failure and568 skips. The observed lane41 Work was2R/5072B versus independent2R/4356B; the 716B delta isolates512B of page slots and214B of owned rows, net of the two5B order-key copies required by the new path. The test held the original fee and exact output/child readback checks. The new production implementation selects borrowed rows only on the frozen cold-inventory caller with a fully compact plan and an admitted one-key order buffer. It retains the exact 64-row SQL queries and fees, extra EOF query at an exact page boundary, original per-root stamp, key and ordinal checks, CAS, ready/frozen epoch, and independent installed block/directory readback. The unchanged owned cursor handles Tree and generic callers or unavailable buffer memory. Lexical Text+TextSpool no-default nextest session3781 is the current post-fix request. Added targeted 64/65-row Work, foreign-root and stale-epoch checks; no post-fix Rust or SDK result is claimed before the actual bodies.

Current post-fix Nextest `d3fe5f70-55c2-44d4-b5d7-67e2d510d9f3`, canonical no-default Lexical `test(native::text::) | test(native::text_spool::)`, 360/360 executed PASS,210 skips, exit0, compile19.18s and bodies14.808s. This includes the unchanged4356B/2R oracle, 64/65 row boundary check, foreign root and epoch refusal. Production source SHA256 `text.rs` e5e842ed / `text/entry_patch.rs` 05546b1d / `text_spool.rs` 1f2a641b. No changed fee, cap, SDK or dependency was introduced. Text is source-ready for the next integrated pair. The separate F Catalog `managed_install_trace_tests::` nextest is actually admitted and its prepared controller `/private/tmp/qidx-sdk60-managed-proof-93kedcbg` has no live build yet. Reuse that one canonical controller if it captures these Text paths, and preserve the original 512MiB/SDK60 Caddy·etcd execution and exact build/input custody. No new SDK body for the borrowed verifier has run yet.

F's Catalog proof nextest `b5dff10a-5345-4ce3-ad69-70fb22124ec2` passed11/11. The existing integrated controller process72343 is live on the canonical queue, with both build and original SDK in the same request. Its source binding pins all four current Text paths, Catalog managed install and both Literal files. All seven input hashes were independently matched. The controller captures admitted whole-source before/after, pins daemon and helper, fences the same source before SDK, and runs the unchanged original Off→Record driver. Do not submit another candidate while this handle is live.

That controller compiled both targets with Cargo exit0 in304.302s, but stopped before SDK because the Runtime integration fixture `native_index_uds/read_sessions/query/history.rs` changed. The one changed test is absent from both exact release dep-info files. Both pinned binary hashes and all seven production bindings independently match. After the controller terminated and no duplicate build/SDK controller was live, the same controller and binding files were reused in `/private/tmp/qidx-sdk60-frozen-compact-rebind-frlwdl2i`, session27378, on the same mutex and build lane. The warm rebind preserves the full source gate rather than overriding it. The current candidate SDK remains pending actual execution.

F's existing `resume-sdk.py` ran the actual original SDK under the same mutex. The redundant warm rebind session27378 was cancelled before admission (exit143, body0). The resume verified both immutable binaries and pinned release dep-info files, diagnostic tools, HEAD and complete retained input snapshots. All2803 inputs match from build-after through SDK-before/after; the one build-time test is absent from both release dep-info files. Daemon SHA256 `7550029b4bf8cd3b8f5aeb76ac37f7ae409f41aa296635fd2d8f89f629082bd8`; native_corpus `8862af2296a29bc2010de58cc313c39ad55751481759f89d6c525db6a353f666`. `controller-result.json` reports SDK exit1, release-target binding true. Off exit1 before any corpus bodies. Original caps were file30k, bytes536870912, R/W/M1m and SDK60.

Both actual original Record bodies returned `ResourceLimited`, FULL ACK0 and query/reopen NOT_RUN. Caddy at Catalog `managed_install.rs:108` used R930368/W524351/B536856475/M687653 and requested60612B. Etcd at Storage `map/scratch.rs:115` used R345282/W535089/B536863078/M869040 and requested8192B; refused before allocating the next directory's scratch. Both strict replay files prove `COMPLETE_DIAGNOSTIC`, health[], open0 and exact trace digests. The preceding Caddy and etcd windows each retain their four exact read counts, with lane41 Work 3,940,716→2,351,650B and9,137,041→5,437,582B respectively. Every individual window reduced the expected same-read allocation cost. These are matching-phase measurements, not full corpus Work totals. Etcd completed the same862 diffs and 210 instead of180 directory map applies; the211th `/40` failed as `map_apply/0` at scratch open with zero accepted Work. The failed fifth inventory window had no `/41` completion. Reuse of the existing multi-root map producer across the directory builds is the measured next seam if it can preserve each original stamp, packing ordinal, persisted readback and all-root completion. The old per-slot scratch allocations are real work; no fee removal or SDK-pass estimate follows from the first 8192B refusal.

### Cold directory multi-root staging

Fixed production-frozen fixture `frozen_cold_directories_stage_together_and_reopen_by_original_slot` prepares two distinct514-term directories at slots0 and2 and an empty slot1, independently reopens both, checks disjoint keys, then requires one map stream. Exact nextest `d1c29436-3b80-451c-968a-2971106efffb` ran1 body, exit100/570 skips, with only actual2 versus expected1 map calls. The storage path uses one Scratch open per empty-stream application. No charge constant was changed.

The frozen Text caller collects its directory requests and sends them in slot order to one existing `AttemptContext::apply_stream` with empty bases and Nested packing for each dense map ordinal. Only one source RootCursor and one ProducerState are active. The outgoing ordinal is assigned during the original SQL row allocation, and anchored edits cross the Storage boundary without an ordinal recopy. Each root independently verifies ordering/CAS, row count, descriptors and EOF before completing its digest. Dense output roots and digests are mapped to original slots; the complete independent global stamp and persisted directory/child readbacks remain. Unbound and warm input paths retain their existing behavior. No new Storage/Core/Catalog private writer or port was introduced. Stage `/40` represents the directory batch on this production route, and Storage root build counters represent the number of directories. Because directory nodes remain provisional, the late second-root fixture checks that a count mismatch cannot return any staged map and leaves no result in the fixture store.

Canonical combined no-default Lexical+Storage nextest `f504f2e5-72ef-42c2-9f11-8b76ede414c6` passed369/369,421 skips, exit0, compile35.86s/body18.266s, selectors `test(native::text::) | test(native::text_spool::) | test(packed_subtrees::)`. Text hashes `text_spool.rs` abfa1dfc / `text/entry_patch.rs` 0772e1cb / `text/entry_map/directory.rs` 653d14c7 / fixture a0caad3c. Same existing admitted build and unchanged original SDK group submitted in `/private/tmp/qidx-sdk60-directory-batch-l8x2gr6u`, session17750, after verifying no competing build/SDK controller was live. New original SDK bodies are pending.

Session17750 admitted after53.109s, optimized build116.559s/exit0, immutable daemon `d747a40b098f14eb6c685843012914a6f4eed949710f9876fc7af0c86e7ac664`, helper `8862af2296a29bc2010de58cc313c39ad55751481759f89d6c525db6a353f666`. All four whole-workspace snapshots (2803 files plus HEAD), diagnostic inputs, and both binary hashes match. Off preflight blocked at load46.905273>8 and3 foreign Rust processes, zero bodies. Both original Record bodies ran at file30k / Work536870912B / R-W-M1m / SDK60 and returned ResourceLimited. Caddy diagnostic wall40.44s, R935743/W533073/B536853187/M696677 plus52768B at Catalog managed_install.rs108. Etcd wall15.58s, R362289/W538086/B536870399/M869734 plus5752B at Storage object.rs1840. Controller exit1, lock released, both trace digests independently verified against COMPLETE_DIAGNOSTIC with health[] and open0. Both FULL ACK0, query/reopen NOT_RUN.

Caddy built the same152 directories with root counts18/25/101/8 in4 map applications (previous152). Inclusive directory stage Work fell6,293,336→3,189,396B (3,103,940B), and all590 diffs retained exact Work. Etcd built the same first159 directories (28/28/37/66) in4 applications (previous159); their inclusive directory stage fell8,301,306→5,009,203B (3,292,103B). The fifth window completes55 more directories and its global stamp; successful inventory diffs advance862→869. R8 also contains independent Storage map/resident/scalar_pair changes. These are matched stage comparisons, not whole corpus net cost or a controlled isolation of every individual source hunk.

The new etcd terminal is in `NativeTextInventoryDiff/2 -> NativeTextInventoryRead/32`, before any posting write for that owner, with accepted R2/B11500. Source `visit_cold_inline` reuses the authenticated packed block for its first count via `read_retained`; the adapter already shares the Arc and performs no second physical payload read/copy. `object.rs:1840` is the existing logical wire admission. Do not remove that port fee to treat the refused5752B as a physical duplicate. R23 later replaces the whole redundant first-count logical call with count selection from the current-attempt checked DATA; the retained-read tariff itself stays unchanged, and Unsealed/Ready keeps that boundary. Caddy still needs the remainder of final Catalog installation. Product NTX4 remains open.

### Borrowed bounded inventory plans

R8 previously removed owned SQL rows only from independent global compact verification. Cold, empty-base bounded planning still used each `RootCursor` to allocate a64-slot page plus a retained `OrdinalMapEdit` for each row, even though `resolve_bounded` only needs its key and values to build an admitted layout. In the fixed two-key production-frozen `SmallMap` fixture the expected Work is R4/8504B: each of planning and global verification retains one4096B query, two76B edit digests, and two2B order-key copies. Actual RED `611800c2-c38d-4aaa-869a-4f89198142e6` executed1 body,571 skips, exit100 with9210 !=8504 and unchanged4 reads/output. The706B is the old512B slot page plus two99B owned rows minus the4B order-key copies. No first-count logical read is changed.

Root-specific and global SQL scans now share the existing frozen borrowed visitor and keep their original query bounds, row/page/EOF admissions and independent scans. A single granted previous-key backing services all bounded cold plans and is released before directory construction. The bounded draft shares the same Small/Block/Tree construction policy with ordinary planning. Borrowed rows retain canonical stamp, object-value decoding, empty-prior CAS and final count. Warm, generic and memory fallback routes keep the owned input; Tree materialization still receives the owned global cursor. The fixture also confirms a claimed existing value on an empty base is refused before map application.

Nextest `386d7837-f68f-482e-90b3-e4855a60a673`, no-default Lexical `test(native::text::) | test(native::text_spool::)`, verified362/362,210 skips, exit0, compile29.54s/body16.902s. The fixture's Work8504B and R4, mapped output refs, and wrong expected Conflict all passed. Production hashes `text_spool.rs` 02c22cdb and `entry_patch.rs` 1063c426.

R9 release builds in `/private/tmp/qidx-sdk60-root-plan-borrow-j3kuiahp` (session38198,104.027s) and `/private/tmp/qidx-sdk60-root-plan-rebind-byisbpey` (session1731,109.538s) compiled with exit0. Both controllers stopped with exit2 after detecting a change to the release dependency `crates/quanta-index-storage/src/map.rs` between input captures. Original corpus SDK body count0, Off/Record NOT_RUN. The R8 Record remained the latest executed original at this point. The prepared `resident-cold-08myshdn` request supplied no build or SDK results.

### Compact cold token rows

R8 etcd Record completed1,117 cold token fill stages. They contain1,581,486 case rows, each with a12B pair of offsets plus position. The fill Work calculation includes the actual row bytes; at 8B per row the same inputs account for6,325,944 fewer bytes. This arithmetic describes that recorded prefix and remains separate from the new SDK measurement. Retained alias reads and token/key logical fees remain as before.

Each ephemeral row now uses two32-bit words with a28-bit key start, 10-bit key length minus one and 24-bit position. The 16MiB normalized input caps worst-case key backing below 2^28, the term-key cap is1,024B, and every token run advances through at least one input byte. The actual arena grant and fill Work follow `size_of::<Row>()`. Exact casing, Unicode normalization, position order, SQL fallback and packed occurrence wire retain their existing contracts. The fixed “A a İ” body reproduced the old B88 append delta against independent B64 before the patch (RED `f85c3578-8587-4827-bf3d-2ea9910c804d`, one failed,571 filtered, exit100).

The first compact-row test run (`32b72828-46fb-4a0e-8974-3cbe5b9db82f`) executed363 bodies:359 passed,4 failed. Three fixed physical-size expectations still described12B rows. The fourth needed a smaller amount of free test memory to preserve real optional denial and exercise SQL fallback. The corrected fixed exact arena expectations are22,000B ASCII,84,000B wide keys,173B Unicode, and19,968B with19,000B free for optional denial. Logical group, position, packed wire and fallback assertions still apply. A test-only integer type mismatch in the first correction was fixed before the full rerun.

An independently admitted frozen checkout `/private/tmp/qidx-sdk60-frozen-install-j3nf1gxu` included the same Text production and a pre-update Runtime restore-forward input. Its 2,803-input capture was stable, but the optimized build finished exit101 after582.913s and emitted no executable pair. The stored build artifact stream does not contain the error detail. Original SDK body count0, Off/Record NOT_RUN.

Canonical no-default Lexical `test(native::text::) | test(native::text_spool::)` nextest `c8f633d1-d490-4720-80dd-f56fba993d45` VERIFIED363/363,210 skipped, exit0, compile56.47s/body146.625s. Compact bounds, exact ASCII/Unicode, wide key offset, logical-equivalent packed wire, optional denial before the SQL fallback, and memory refund passed. `cold_tokens.rs` b5501bda and fixed parity fixture `token_spool_batch.rs` 3b7edb09.

R10 `/private/tmp/qidx-sdk60-compact-rows.zGLJV5`, session74904, optimized daemon+helper build exit0 in292.027s. The 2,803 workspace inputs and HEAD match across all four build and SDK captures, and the diagnostic inputs and pinned binaries match. Daemon `0919db4a30ffe7a105f16a20aa62de9c52d981c904483c4b0b49fa2b39a19eee`, helper `5147e069d390029c5d6a03344d82bcb4e4ba6092f989d8815df901db44182278`. The input binding is identical to R8. The driver retains SDK60, file limit30,000, Work536,870,912B and R/W/M1,000,000 each. Quiet Off preflight BLOCKED with body0 at load32.440918>8 and two foreign Rust processes. Both original Record bodies ran and returned exit1. Both first-process traces are COMPLETE_DIAGNOSTIC with no health reports, zero open requests, and a matching SHA-256.

| Original Record | Actual wall | Accepted R/W/B/M | First refused charge | Full ACK · query · reopen |
| --- | --- | --- | --- | --- |
| Caddy590 | 43.23s | 956713 / 543365 / 536141651 / 706969 | 752549B in Storage prepared batch write `object.rs:807` | 0 · NOT_RUN · NOT_RUN |
| Etcd1421 | 18.65s | 362622 / 568439 / 536869310 / 869734 | 17613B in posting edit logical admission `packed_runs.rs:365` | 0 · NOT_RUN · NOT_RUN |

R10 includes independently owned Storage map/resident/scalar and Runtime/Contract changes since R8. Corpus-wide deltas and the later failure locations therefore describe the whole candidate. The lane44 Text fill comparison matched normalized input charge, case shape, owner order and branch across all589 Caddy fills and all1,117 etcd fills. Each owner saved exactly4B per case row. Caddy799,870 rows saved3,199,480B, etcd1,581,486 rows saved6,325,944B. Small root planning lane40 had the same reads/writes in all four Caddy windows and five etcd windows and used12,782B and23,464B fewer respectively. Independent global stamp verification lane41 retained exact reads and bytes across those windows. R10 completed all590 Caddy and944 etcd inventory diffs.

Etcd's terminal64-row flush failed inside the original posting logical write admission, before page-source creation, packet encoding, sort, second row copy or SQL. Earlier lane37 contains197 completed physical reorder copies totaling5,502,583B. An alternative packet representation requires measured net savings after offset and reader costs, separate from the terminal canonical fee. Caddy reached final `Text::apply_postings` and consumed33,025 inline plus18 object top-map rows before the reachable-node install. The terminal Storage admission is actual new batch payload plus256B per object. The 23,288B and16,011B overages describe the first refused charges only. Subsequent work required for FULL ACK, search and reopen remains unqualified.

### Authenticated posting packet boundaries

The packed cursor authenticates in-packet order once and keeps the boundary
witness through packet turnover and retry. The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md) owns this
contract. R11 Caddy and etcd Records refused before ACK at their original
caps. R11 Large passed its two-restart body in
`ef7d80cc-502d-4510-a803-9640e018463c`. Exact binaries, charges, source
capture and result tables remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### Resident packets across cold term windows

The two-slot resident packet cache reuses authenticated payloads across
term windows under its Original grant, source identity and accepted epoch.
R12's two Record requests still failed before ACK. Its selected test-only
source drift and original metrics remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).
The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md) owns cache lifetime and retry.

### Parallel closeout against NTX4

The sealed-writer grant release and existing group-local child validation
are implemented. The R12 owner table is historical and retained in the
[exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction). Current Caddy Record/Large and etcd outcome follow
[the active product boundary](#admission-decision-and-remaining-product-verification).

### Same-wire late materialization for phase-0 packets

The isolated late-emission draft measured a proposed Work change on five
rows. The later production cold term writer is recorded in the [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md).
The draft's exact fixture and proof limits remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### Posting cursor retry after retained-memory refusal

The retained cursor retries the refused row and checks stale-source
identity. The owning no-default Lexical run
`9d6e5066-bc00-49f1-b54d-5424a2fd89cf` passed 365/365. The subsequent
child-census candidate supplied diagnostic Caddy and etcd failures with
release-input drift. Their exact charges, metrics and build identities are
in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### NTX4 structural remediation and acceptance

- **Caddy, Storage done at owner scope.** Its guarded full-reference validation reused 22,696 results and completed all three physical groups in `/private/tmp/qidx-sdk60-guarded-children-retry.AVf3sU`. Storage nextest `30865238-1d41-4b81-a88f-5c998b63bf22` passed 223/223. The captured Storage inputs match live source. The original Caddy Record now refuses 5 reads at an already consumed 1,000,000 in Catalog confirmation. An unrelated production History input changed during the release build. F owns confirmation-time custody and the corresponding fixed fixture. Use the F packet for that implementation and evidence. [F integration](NC-F-integration.md)
- **Etcd, phase-0 run construction connected.** The empty-prior cold entry, including empty-profile Patch, uses the bounded writer described in the [posting custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody). The prior 5,502,583B reorder-copy saving was below the 6,309,495B needed for one read per completed phase-0 packet and its row bytes. The older same-wire late-emission draft remains an experiment. [Lucene term streams](https://raw.githubusercontent.com/apache/lucene/main/lucene/core/src/java/org/apache/lucene/index/TermsHashPerField.java)
- **Owner proof.** The cold fixture must cross both producer batch and phase-0 packet boundaries, compare complete `(term, owner, expected, next)` output against a sorted independent fixture, and exercise refusal and duplicate checks. Track unique terms, output rows, physical packets, sort/repack Work, retained peak and merge passes. Charge logical edits and real staging or merge work. Count compiler success separately from executed bodies.
- **Product proof.** On one stable source-bound daemon/helper pair with the original manifests and caps, Caddy590 and etcd1421 each require a healthy Record, two quiet Off successes, FULL ACK, query, and reopen. Run the same-binary 4,096-file restart check. NTX4 remains open until both corpus paths pass. Public SDK, IPC and durable index formats remain the same.

The first production-connected cold builder passed 366/366 Text and TextSpool cases (`5ad8dd70-4f05-477e-acfa-b90742ed0f8f`, exit0). Its audit exposed two regressions outside the first fixture: a larger mandatory Original grant and extra work for already ordered packets. The revised builder uses the original grant, preserves and reuses a checked wire prefix, and interns the suffix only after the first real ordering inversion. It merges that ordered prefix with sorted suffix terms once at flush. The run merge now borrows heap-head keys from checked packets and retains only one prior term with the existing key representation. The ordered fixture compares wire bytes, complete Work and output under the original small grant. The revised owning result and its cohort are recorded in [the release and Record follow-up](#same-source-release-and-original-record-result).

Catalog's current confirmation already uses a transaction-local full-reference `ChildWindow`. Its preflight proof is consumed by registration; per-parent lifecycle transitions remain separate. The Caddy refusal at `require_child_charged` has no per-reference evidence of an additional same-transaction query. F's separate namespace candidate `/private/tmp/qidx-sdk60-namespace-fusion.Lh6Rg2` has a Caddy-only original Record controller. Reuse its compiler pair for etcd only if both build captures contain the complete Text change, then preserve the source-custody classification of the pair.

F proved the separate Catalog namespace lookup fusion with 387/387 Catalog cases. Its first joint release attempt stopped at compiler exit101 before either SDK body because the hybrid writer assigned a last fence that the immediately following suffix always overwrites. The live Text file `cold_run.rs` `f48be457` removes the assignment. The new prefix/suffix fixture also checks exact order after a refused SQL write, unchanged SQL epoch, and duplicate rejection across that boundary. The corrected Caddy controller `/private/tmp/qidx-sdk60-namespace-text-repair.DpImEH` is live in shared build admission. The same-pair etcd follow-up `/private/tmp/qidx-sdk60-term-runs-etcd-bla20_ul` waits for the Caddy controller and verifies the four current production Text hashes in both build captures. The immutable binaries and the resulting SDK bodies are recorded [below](#same-source-release-and-original-record-result).

#### Original-input and product qualification checks

The pinned manifests and every listed source hash match for Caddy590 (3,272,107B) and Etcd1421 (8,666,217B). Each query path belongs to its checked manifest, and each repository commit matches its binding. The current darwin timing preflight (`env -u QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS python3 tools/ci/timing/check_host_contention.py --expected-os darwin`) exited1 with three foreign Rust processes and one-minute load 48.866699 over the ceiling 8.0. Original clean Off bodies remain `NOT_RUN` at this observation.

The cold term index now uses a fixed per-key fingerprint for a pinned Rust build (`cold_run.rs` `83f329bb`). This keeps the charged exact-key collision chain independent of the randomized `HashMap` placement. Output order still comes from the checked full term and owner keys. This closes a static determinism gap; no corpus collision was observed. The change and the updated etcd binding landed while the Caddy controller was still awaiting admission and before source capture.

The revised Lexical nextest attempt exited124 after 1,800 seconds of shared build-lock wait. It never started Cargo or any test body. The identical selector was submitted again on the current source with a longer admission wait. Caddy remains a single corrected release job, and the etcd watcher takes that exact pair after the Caddy Record terminates. Neither waiting handle is a verification result.

1. Reuse the one admitted daemon/helper build and compare both capture files with the production SHA set. Confirm the daemon compiler dep-info contains the two posting implementation files. If the full capture changes only outside the two release dependency closures, prove that scope from both depfiles. A changed release dependency prevents exact-source acceptance. Rebuild once against the stable input set, using the existing checked overlay only if concurrent production changes require it.
2. Require both original Record runs to produce FULL ACK, search and reopen. Replay each successful output with `python -m tools.benchmark.retrieval.causal_cost_diagnostic native-corpus-record --run-dir <output-root>`, which requires independent first/reopen trace identities and a successful lexical readback in both epochs. On refusal, preserve axis, accepted counters and failed stage from the complete diagnostic. For the Text calculation, `native_text_posting_edit/38` includes its child sort lane (`/39` on hybrid, `/36` on legacy), so do not add those child Work deltas again.
3. Run two quiet Off rounds per corpus with the original helper, manifest and query on that exact daemon, using fresh output roots. Preserve SDK60 and the original file/Work caps. For Large, pin `QIDX_NATIVE_LARGE_RELEASE_BIN` and `QIDX_NATIVE_LARGE_RELEASE_SHA256` to that daemon and run `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --test native_product_process --no-tests fail -E 'test(=large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks)' --test-threads 1`. The 4,096-file, two-restart fixture uses its own larger Work and deadline envelope. Its body and the original SDK60 bodies close distinct requirements.

#### Same-source release and original Record result

The namespace/Text repair build at `/private/tmp/qidx-sdk60-namespace-text-repair.DpImEH` finished in 298.275s, exit0, with 2,807 equal build inputs, equal HEAD and equal diagnostic tools. Daemon `91046e6be599…` and helper `049845718770…` were pinned and rehashed. Both release depfiles include `packed_runs/cold_run.rs` and the daemon also includes the modified Catalog reclaim path. `/private/tmp/qidx-sdk60-term-runs-etcd-bla20_ul` verified the same binaries, unchanged SDK source captures and zero release drift. Both Record traces report `COMPLETE_DIAGNOSTIC` with zero open requests and empty health. Both SDK bodies exited1 before FULL ACK, search or reopen. Clean Off and Large for this daemon are `NOT_RUN`.

| Original Record | First refused Work | Completed prefix and current owning boundary |
| --- | --- | --- |
| Caddy590 | Reads 1,000,000 + 1 at Storage `object.rs:1493` | All five Catalog advance spans passed. The new fourth physical group planned 187 objects and 6,179 external edges over 2,047 full refs. It reused 3,925 guarded child checks before the denial. The first three groups still reused 22,696 in total. F owns Storage follow-up. Measure exact child-ref overlap across groups and the failed validation prefix before widening witness scope. |
| Etcd1421 | Bytes 536,856,479 + 27,703 at Text `packed_runs.rs:36` | 1,045 inventory diffs and 221 packet flushes passed, versus 945 and 197 on the previous candidate. The failed packet's unique-term sort succeeded with 9,149B. Its output-body copy was the next charge, so savings must occur before this allocation and remain sufficient through final merge and posting application. The completed hybrid packet spans replace the old full-row sort/reorder spans. Aggregate costs cover unequal prefixes and cannot be subtracted as a matched saving. |

The first revised Text nextest compilation reached Rust and exited101 because an internal packet test still called the removed owned-row `Packet::next`. Its two calls now exercise the same assertions through `Packet::next_row`. No body ran in that failed attempt. Behavioral RED `428a8d6a-58fb-41b7-aa6a-ec4379d9d70e` executed one body and observed 392B against the 386B independent golden, exit100. The fix compares the unchanged prefix once per suffix term and preserves the full owner check on equal terms. Current-source no-default Lexical nextest `fc03272b-6696-4374-aa2b-f7215f8a7867` passed 369/369 with 210 skipped and exit0. This includes exact Work, unchanged SQL epoch on refused write, funded retry, duplicate key rejection and complete term/owner output. The complete Record traces remain tied to the earlier binaries.

The next larger Text cost to evaluate is materializing phase-0 packets through SQLite. At the exact Etcd1421 refusal, 221 successful disordered packets had already written one SQL row each, and no phase-0 merge read had run. For a packet payload of `p` bytes, omitting that actual SQL insert avoids `2p+6,208B` and one write before the refusal. The existing trace puts an upper bound of 23,006,301B on those writes because `lane38−lane39` still includes payload production, dictionaries and merge comparisons. Future eliminated point reads and EOF probes count separately. Any optional Original-backed run experiment must prove retained headroom, immutable replay after pre-effect refusal, accounted spill to SQLite, final SQL seek for a single phase-0 run, and mixed resident/spilled full tuple and EOF parity under the same caps. Measure the resulting prefix and FULL against the existing original input before adoption.

#### Structural RCA and compatibility decision

The grouped-prefix original Etcd1421 Record at `/private/tmp/qidx-sdk60-prefix-group-p4cz3qql/record` has a healthy complete diagnostic, matching released binaries and exit1. Six workspace source paths changed across build capture, so use this run for diagnosis. The first 221 successful lane38 spans, aligned by packet ordinal, used 561,441B less than the paired baseline. Lane39 Work was identical at those ordinals. Packet222 completed and the next refusal was `object/read_io.rs:168`, at 536,865,220 accepted bytes plus 5,987B. Inventory diff advanced from 1,045 to 1,049 completed owners. The failed leaf is inventory-read lane32 under lexical build, which runs both the authenticated cold-inventory visitor and its nested posting flushes. Charge their costs by exclusive path or matched prefix before treating the lane total as an object-read saving. FULL ACK, search, reopen and phase-0 merge were not reached.

| Owner | Existing implementation | Structural change and acceptance |
| --- | --- | --- |
| Storage / F · Caddy590 | The admitted run holds the union of destination gates until its last group finishes. Physical validation keeps a bounded exact full-reference table per group. Catalog custody has its own reservations, confirmations and lifecycle checks. The refused-prefix overlap observer and its fixture are wired. Confirm their owning run and the actual Caddy census before deciding on cross-group reuse. | Measure previously validated, still-gated full refs in subsequent groups and in the actual failed prefix. If the overlap justifies the table, carry successful path validation through one admitted run under its existing guards. Preserve lazy first-use validation, per-edge local dependency checks and reserve/advance/barrier/confirm order. Prove same-ref reuse, different refs sharing one gate, ungated/capacity fallback, missing/symlink child, concurrent reclaim, and refusal without publication. Compare a full table against the existing per-group baseline so early entries cannot silently erase later in-group savings. |
| Lexical · Etcd1421 | Cold runs already group suffix postings by term and merge the checked ordered prefix. Phase-0 packet emission still encodes and inserts a scratch SQL BLOB, later reopened by the merge reader. Installed inventory readback independently checks root identity, keys and counts. | First encode repeated phase-0 terms once per ordered chain without adding live packet memory. Measure actual packet emission, SQL insert, SQL reload and EOF work separately. Keep a bounded, immutable phase-0 run source with SQL spill as a separately gated candidate. Preserve final SQL seek/EOF and the single-run path. Trace exact occurrence refs across the prior prepare and failing visitor before proposing any independent readback reuse. |

Keep the existing group-local table. The initial cross-group candidate adds one optional bounded table for successful child path checks of prior confirmed outputs from the same install run. Membership uses the whole `ObjectRef`, and reuse requires the still-held governing gate. Allocation or capacity misses continue through the current group-local path. The namespace lease and gates serialize cooperating writers and reclaim. Reuse also requires the same managed namespace binding and the current contract for detecting out-of-band path changes. Eager validation of every edge would check later references before earlier groups have installed their children and shift the point at which effects or errors occur.

The resident Text prototype retains phase-0 bytes only when the Original grant can preserve the mandatory producer and merge envelopes. A resident packet can replace its merge reader's SQL buffer. Admission must account for the actual live backing, including the direct-merge credit, before counting it as a replacement. Prove a tight-scope case where the current spill path succeeds and the candidate either merges or falls back with the same output. Reject a SQL shadow row or any missing `(phase, run, page)` identity before merge. The term encoding below can be evaluated with the current scratch path and does not depend on resident admission. Any later residency change needs measured net Work and exact output equivalence.

The proposed changes are private to one install run or one Lexical attempt. SDK and IPC shapes, catalog state transitions, published object encoding, object identities, and on-disk reopen/restart semantics require no version bump or migration. Keep `posting_packet` final seek/fences and change-count observation, inventory `planned` → `ready`, confirmed occurrence receipts and top validation in their existing SQL authority. Replacing the full scratch database would expand the in-flight contract beyond this RCA. Record analyzers enumerate known counter keys. Ship new diagnostic counters with their matching reader/schema and check old-reader behavior before declaring trace compatibility.

The Caddy design uses exact batch reuse under the authority that guarantees namespace stability. RocksDB groups common immutable-block access within `MultiGet`, and Bazel CAS `FindMissingBlobs` explicitly couples batch presence checks with lifetime protection. Neither source supplies the Quanta reclaim or install guarantee. [RocksDB MultiGet](https://github.com/meta/rocksdb/wiki/MultiGet-Performance) and [Bazel CAS protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto).

The Text design follows term-first stream production and budgeted run spill. Lucene sorts each field's unique terms at flush and streams postings, Tantivy enumerates and sorts its term index before serialization, and PostgreSQL sizes its balanced external merge and read buffers under a single sort budget. Keep the established term chains and copy only the bytes each downstream consumer requires. [Lucene postings flush](https://github.com/apache/lucene/blob/main/lucene/core/src/java/org/apache/lucene/index/FreqProxTermsWriter.java) · [Tantivy postings serializer](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/postings_writer.rs) · [PostgreSQL tuplesort](https://github.com/postgres/postgres/blob/master/src/backend/utils/sort/tuplesort.c).

Run both original SDK60 Records on one stable released pair after the owner-level changes. Require complete healthy traces, FULL ACK, search and reopen for each, two clean Off rounds each, and the same-daemon 4,096-file two-restart case. The 512 MiB cumulative Work limit, 1,000,000 R/W/M limits, 60-second deadline and fees stay fixed. The phase-0 resident prototype has no production merge or executed proof yet.

#### Cold phase-0 term encoding

Native allocation admission checks live Temporary and Retained bytes together, including outstanding credit (`native_resources.rs:1322`). Holding optional packet data during inventory production could consume the next mandatory allocation's headroom. The resident prototype stays isolated pending a bounded producer envelope. Its directory patch scans and verifies the exact SQLite complement of resident run IDs, but a tight mixed-lifetime and merge-time mutation proof is still required.

The [cold posting ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody) records the mounted transient term encoding. Owning compilation, the fixed merge fixture and same-pair SDK execution still need their results.

The owning selector covers Text and TextSpool. Its new fixed 3,544-row oracle checks two input runs, 572 repeated terms, raw packet width 25,050B → 21,618B for run one, an independent lane38 Work golden of 102,206B, every merged row and EOF, and Off/Record Work parity. A separate fixture rejects malformed references even when their scratch digest is recomputed. After a green selector, reuse F's pending Caddy census release for original etcd only if both compiler captures include this exact implementation. SDK qualification still requires the outcomes above.

Owning Lexical proof **VERIFIED**. The first selector stopped during test compilation on a temporary digest borrow, with zero bodies. After binding that digest in the fixture, `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E 'test(native::text::) | test(native::text_spool::)'` exited0 in run `9626b601-4703-4dbf-9818-00284a86bdf2`. Passed371/371, skipped210, compile1m17s, bodies18.907s. The run includes the two new encoding oracles and the existing 131,073-term and etcd-size posting fixtures. The original etcd driver is staged at `/private/tmp/qidx-sdk60-term-backref-k_xrrs0s` with exact production Text hashes. It requires the identical daemon/helper produced by the census release and the unchanged original corpus binding.

Raw span ancestry narrows the separate inventory investigation. Of 155,282,012B and 431,730 writes in the grouped-prefix Etcd1421 lane32 diagnostic, 105,239,440B and every write were in its 3,371 nested lane33 posting flushes. Residual visitor Work was 50,042,572B and 4,177 reads. Lane32 mixes inline and directory aliases and does not break out the actual object routines. Storage's retained read clones the immutable `Arc`, so there is no second payload copy there. Producer authentication and post-apply readback retain separate validation points. Before changing that path, measure exact object-route reads and complete output under failed or mismatched readback.

The same trace's 142,994,133B build_prepare/40 is the inclusive Native Literal prepare. Its source/gram collection and directory/installation spans are nested, including the 198 single installed-block readbacks. Kernel already passes one bounded `AttemptObjectReads` through both member builders. A full-reference hit reuses immutable backing with the existing logical read admission. The trace does not attribute those hits to each member or source ref. A new cross-member stream would need to preserve independently complete Literal raw-byte trigrams and Lexical UTF-8/tokenization, so this is outside the current packet modification.

Two earlier shared releases exited 101 at History validation-only calls, with
zero SDK bodies. The repaired source retains both validations. The exact
compiler runs and paired supervision are in the
[recovery index](../../../ARCHIVE-INDEX.md#2026-11-17-active-ticket-proof-refresh).

#### Original Caddy census with grouped Text encoding

The compiler exited0 after660.367s. All nine census bindings and both binary digests match. Three release inputs changed across the 2,812-file capture (`native_history/build.rs`, `native_history.rs`, and `native_boot.rs`). This pair supports bound Text/Storage diagnosis and does not qualify as a whole-source release. Original Caddy590 Record exited1 with 1,000,000 reads, 618,916 writes, 494,294,051B, and 781,821 mutations accepted. Its next charge was one read at Storage `object.rs:1518`. Strict ingest passed with 244,058 written events, no drops or open spans, and empty health. FULL ACK, query and reopen were not reached.

Four successive physical groups under the failing map apply measured planned guarded full-ref overlap of 0 / 273 / 306 / 350 and completed repeat path checks of 0 / 273 / 306 / 331. Total completed overlap checks in the captured request are **910**, including **331** in the refused group before the denied read. The already reused group-local checks were 8,199 / 7,229 / 7,268 / 3,925. The cross-group counts exclude those local-cache hits, refs first installed as local outputs, and the denied edge. They establish a concrete candidate for Storage reuse and do not establish sufficient remaining Work for FULL. Keep the group-local cache and confirm the managed-namespace validity across group boundaries before adopting cross-group reuse.

F's paired SQL/Work evidence bounds later confirmation at **10,104 reads before dynamic child authentication** for the final group alone. That is greater than all 910 measured repeats. F owns transaction-local namespace authentication reuse, with reauthentication after writes, exact SQL/Work delta, and mutation rollback. Keep the Storage memo as a follow-up to the actual residual. See [F integration](NC-F-integration.md).

#### Original etcd result on the same daemon and helper

The unchanged 1,421-file, 8,666,217B original input used the Caddy daemon and helper with matching SHA-256 digests. The 2,812-file SDK pre/post captures were equal. The EtcD Record exited1 in34.80s wall. The first refused charge is the existing logical posting admission at `packed_runs.rs:401`, with R354,434 / W619,540 / B536,869,801 / M869,734 accepted and 17,524 more bytes requested. The raw footer is complete with 200,304 events, no drops and no open spans. The initial system Python3.9 analyzer could not run the current `zip(strict=True)` validator. Reingesting the existing trace with workspace Python3.12 exited0, reporting `COMPLETE_DIAGNOSTIC`, zero open requests and empty health. No second SDK body was needed.

Against the grouped-prefix Record, all 222 common lane39 sort costs are equal. The 222 ordinal-matched, completed lane38 packet spans used **2,213,358B less** in the new candidate (210 lower, 12 equal, 0 higher). Every saving is a multiple of3, matching the encoded-byte removal and the two actual payload widths in the existing INSERT charge. Original inventory diff completed 1,063 owners versus 1,049, and 226 versus 222 disordered packets. No final merge, FULL ACK, query or reopen was reached. This is pinned-binary diagnosis with the same three History release-input changes as Caddy.

Keep the existing logical posting admission, fee schedule, limits and phase0 integrity. The priority for the next Text change is a measured reduction of actual write/read work before sealing. Compact owner/reference cells can remove 2–4B per eligible row but the previous 222-packet inventory bounds pre-merge savings at 2,349,672B to 4,699,344B even before accounting for prefix re-encoding. Do not treat this upper bound as a remaining budget or as proof of FULL. Evaluate a bounded spill/retention path and the original allocation envelope before adopting another codec variant. The release and SDK diagnostics above leave both original Records FAILED. Clean Off preflight refused at 78.309 one-minute load against8.0 (22 foreign Rust processes). The same-daemon Large and clean Off bodies are NOT_RUN.

For the new 226 completed disordered packets, lane38 minus lane39 reaches20,703,017B. The existing emission charges `P−50` for body plus dictionary bytes and the INSERT charges `2P+6,208`, leaving a nonnegative comparison charge. Thus total payload is at most **6,437,103B**, and the INSERT components at most **14,277,214B** before this refusal. A candidate retaining those packets would need room for that observed payload shape, metadata, and later packets while preserving mandatory allocations and separately metered fallback. These numbers are upper bounds, not measured savings. A positional file spill cannot be connected through the existing `ScratchIoBinding`: the binding covers SQLite VFS scope and namespace custody. A new managed file port would need reservation, exact I/O receipts, cancellation and partial-write cleanup.

The source already compares adjacent term keys in the mandatory `ColdTokens::sort_and_validate` pass. Reuse that exact group count before the single-block alias probe. Once there are over 512 distinct case-prefixed keys, the probe cannot succeed, so skip its actual key/position inspection and vector lookup. The original direct or SQL fallback still receives the fully validated arena. The current diagnostic observed 205 direct owners above that count. Their exact probe cost was not independently metered. This candidate requires the 512/514 boundary oracle and the existing owner/fallback suite before the next joint release.

The count gate and its fixed boundary oracle are applied. At 514 terms the oracle verifies zero alias-probe Work and zero new memory grants, two direct output blocks and every ordered key/position. At 512 terms it verifies an eligible one-block alias, the original 4,608B inspection charge and memory refund. The owning no-default Text and TextSpool selector `5af239de-a369-44be-8de6-d4b7cab44e76` passed372/372, skipped210, exit0. Compile took14.20s and bodies12.921s. The next paired etcd driver at `/private/tmp/qidx-sdk60-alias-count-fdlr4ij6` binds all five Text build inputs including `cold_tokens.rs`. Invoke it with the workspace Python and the one F-owned release containing both owner candidates after their owning proofs. The original SDK bodies are NOT_RUN for the count gate.

#### Next measured Text boundaries

The registered attempt's shared object window intentionally preserves logical read and decode charges. The source's term and position groups cannot replace the independent installed-object readback, which proves persisted references, counts, descriptor boundaries, both seals and EOF before publication. Preserve those checks.

The 953 lane32 owners from the pinned backreference Record consumed50,508,862B after excluding nested posting edits. The 202 spans with more than two reads represent34,064,564B of that exclusive total and are the confirmed multi-read tail. A cold Directory cursor currently starts a fresh map scan at each descriptor. A retained bounded descriptor page could eliminate repeated root/path traversal. Its potential map work is a subset of those exclusive totals. It needs an Original-funded page/retained key grant at the backend admission boundary, intact child verification, and boundary/EOF/output and failure-oracle tests. A scan8 failure after charged reads cannot be retried as if the scan had never happened. Changing the page-prefetch order also changes the observable error prefix. No unconditional Text-only fallback was adopted.

The original Etcd manifest and all 1,421 file hashes were independently checked. On the 1,392 entirely ASCII files, the exact token character/length rules find 927,696 indexable runs. 626,163 repeat the same raw term within an owner and span3,069,185 raw bytes. The two case keys for these repetitions would occupy7,390,696B if every such source used the cold arena. This is an input-shape bound across the whole ASCII subset, with no claim about the executed prefix or net Work. A sorted-source interner must preserve NFC and both case forms, the exact-census assertion, case-sensitive positions and replay fallback. The current arena grant covers capacity before filling; a shorter used key buffer alone does not return that grant. Count new hash/compare, row movement and temporary Original costs before choosing a design.

An isolated candidate oracle at `/private/tmp/qidx-map-scan-oracle.sJyhXo/descriptor-scan-work.patch` applies cleanly to the current Storage source in a dry run. The fixed 32-descriptor trie compares actual scalar scan1 and page scan8 Work, complete ordered output, cancellation, and admission/refund. No production code is changed by the oracle. Compile and body are NOT_RUN. Use the measured pair before committing to the backend admission seam. The existing paired etcd watcher is bound to the prepared F release `/private/tmp/qidx-sdk60-caddy-catalog-bound.81rgVy` and remains in intake until F's release controller is active.

#### Combined Catalog and exact alias-count candidate

F's Catalog lib selector `b3cbd4fb-9a03-49b5-8e5a-a88122855c92` passed390/390, exit0. The exact fixture observed one namespace SELECT in each of reserve and confirm (54 and37 reads) and proved reauthentication, refusal, rollback and reopen after a queue trigger changes the binding. Catalog and Text owning checks now cover this candidate. The single shared optimized release `/private/tmp/qidx-sdk60-caddy-catalog-bound.81rgVy` acquired the build-test lock and started compiling. Its prebuild capture matches all nine census-bound inputs and all five Text inputs, including the 512-term alias gate. The live etcd watcher observed that exact controller and will reuse both binary hashes after the Caddy Record. Fresh original Record, Off and Large results remain pending.

#### Original SDK results and structural decision

The candidate above compiled in271.720s, exit0. All 2,812 build inputs were identical across build and both SDK captures. Both original SDK runs rehashed the same daemon `f87b6b3d239a…` and helper `eb9ffec3e4d5…`, with no Work or request-limit override. Strict ingest reported `COMPLETE_DIAGNOSTIC` with empty capture health and zero open requests for both first-process traces.

| Original SDK Record | Outcome | Evidence |
| --- | --- | --- |
| Caddy590, 3,272,107B | VERIFIED | FULL publication in50.457s from submit, exact 590-file inventory ACK, FileMap/literal/text search and independent reopen search. SDK and both-trace verifier exited0. The successful BuildPrepare span used951,711 reads, 621,676 writes, 492,696,487B and786,109 mutations. |
| Etcd1421, 8,666,217B | FAILED | `BuildPrepare` refused17,445B after536,855,013B, with R348,315 / W623,254 / M869,734. The first denial is existing logical posting admission at `packed_runs.rs:401`. Completed1,072 inventory diffs and228 disordered phase-0 packet writes. The final posting merge, FULL ACK, search and reopen were not reached. |

Comparing the 1,117 ordinal-matched inventory preparation spans at `native_text_inventory_prepare/46` with the previous backreference Record shows1,370,449 fewer bytes, across207 lower and910 equal pairs. Reads, writes, mutations and the parent token-stage Work match. Completed inventory diff advanced from1,063 to1,072. The required logical debit is where the exhausted budget becomes visible. Upstream packing, re-encoding, SQLite scratch and repeated map scans all consume the same budget. The matched preparation savings and the one next debit do not measure the budget required for the remaining349 inventory owners and downstream apply.

1. **Catalog lifecycle, implemented.** Keep transaction-local binding, write-triggered reauthentication and per-parent lifecycle checks. The paired original passes this public Caddy Record path. The earlier910 measured repeat cross-group child probes did not establish enough savings to close the older terminal, so further cross-group memoization is outside the current repair.
2. **Directory traversal, measure first.** `Cursor::advance` performs `scan(limit=1)` for each descriptor. The backend `scan_with` rebuilds its path and stack and reopens the same root on each call. The current EtcD result has204 completed lane32 spans with more than two reads, R2,724 and34,167,943B after subtracting nested posting lane33. That includes child loads and seal checks. Use the fixed scalar/paged Storage oracle to measure the actual repeated trie work, then compare an attempt-bound root/continuation cursor against it. Keep bounded grants, exact ordered fences, independently loaded child identity/counts, seals and EOF. Prefer incremental continuation when eager pages would move future validation ahead of the current child. Optional page admission can choose scalar only before additional Work or input effects. After a selected path has started, propagate refusal/cancellation/corruption and poison the attempt.
3. **Phase-0 posting, structural prototype.** Reuse the existing sorted run merge and publication path. Prototype a generic one-head owner cursor inside prepared inventory, then emit a bounded sorted multi-page phase-0 run into the existing merge. A selector requiring all alias owners would skip all five observed mixed batches on the preceding Record. The generic path must cover inline aliases, directory aliases and ordinary owners, preserve the logical per-edit fee and exact `(term, owner)` order, retain grouped term backreferences, and fund all simultaneously live cursor heads through Original. Compare actual packet copy, SQL I/O, heap Work and peak grants against the reference. Keep sorted runs disposable until the last input seal and posting completion. Use neither full-attempt cursor retention nor a new spill port without measured net benefit and explicit funding.

An internal cursor and paired private `QITXPR02/03` writer/reader change need no SDK, IPC, or durable-format migration. Restart reclaims old scratch as orphaned allocation. Preserve the durable posting formats, semantic output policy, expected-view CAS and one atomic FULL ACK. Changing the persistent dense descriptor/page would change output identity and require old-view/reopen compatibility work. Splitting externally visible ingestion would change the source and ACK contract.

Reference implementations for the chosen boundaries are [LevelDB's same-block iterator reuse](https://github.com/google/leveldb/blob/main/table/two_level_iterator.cc), [SPIMI's bounded runs and merge](https://nlp.stanford.edu/IR-book/html/htmledition/single-pass-in-memory-indexing-1.html), [Lucene's flush and commit boundaries](https://lucene.apache.org/core/10_3_2/core/org/apache/lucene/index/IndexWriter.html), and [RocksDB's recoverable manifest groups](https://github.com/facebook/rocksdb/wiki/MANIFEST). These support the design, not a Work forecast. Lucene's RAM threshold is a flush trigger, while this request's 512MiB Work is cumulative and unchanged.

Clean Off preflight refused with one-minute load54.816406 against8.0 and11 foreign Rust processes. Clean Off bodies and the 4,096-file two-restart body for this daemon are NOT_RUN. The next product candidate must pass both original healthy Records and both quiet Off rounds per corpus plus the same-daemon Large check.

#### Executed Storage traversal oracle and mutation ceiling audit

The frozen oracle at `/private/tmp/qidx-descriptor-oracle-run.yY3zYDIs/snapshot` checked 3,252 copied paths against Main, including the Storage `map.rs` preimage `0e20ad2a…`. Only the snapshot received the oracle. `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-storage --lib --no-tests fail -E 'test(=map::tests::descriptor_shaped_scan_pages_measure_exact_work_without_changing_rows)' --no-capture` passed one body, exit0, run `60e838fc-9d06-44aa-9bc0-70e247b61447`. Scalar pages used66 reads and85,925B. Eight-row pages used10 reads and26,019B for the identical32 ordered `(key, value)` rows. Both had zero writes/mutations. Cancellation recorded zero Work. A refused eight-row retained capacity recorded a sticky memory denial, so scalar fallback must be selected before an optional attempt records that denial. These savings describe the fixed trie.

The isolated DFS continuation is **VERIFIED** at the Storage library scope. The first shared attempt ended before admission. The isolated compile then reached Rust and stopped at E0382 with zero bodies. The corrected snapshot reborrows the mutable reader for each loop iteration. Nextest `fb3d316d-be69-4d95-b50e-6a531082a768` passed the expanded fixed fixture, exit0. Continuing one traversal and returning one row at a time used3 reads and17,664B. This saves63 reads and68,261B versus scalar scan1 and7 reads and8,355B versus scan8. Exact rows and `has_more`, zero-Work EOF, resumed fence, cancellation, sticky failure and fund release passed. The full no-default Storage library run `a02e0be0-baa8-4a92-9ea5-ace7365f9542` passed226/226 with1 skipped, exit0. Both executed in the warm copied checkout with cooperative admission disabled for the isolated crate. The verified patch at `/private/tmp/qidx-scan-continuation-prototype/scan-continuation-verified.patch` has SHA-256 `9494318bd172b5c4e36cb392799acd5955c48905a3a341bbd5fad6434f83a647` and applied cleanly in that dry check. At this prototype checkpoint Main runtime sources were untouched. The later mounted implementation and executed SDK results are recorded below.

The production scan visit now preserves page1 lookahead order, uses the same WorkBudget, and retains descriptor/child validation and attempt custody. Text actually mounts `FileObjectStore::ScopedReads`; the Kernel selects `OwnerScope` only for Semantic preparation. The former proposal to require an `OwnerScope::read_node` specialization was not a Text dependency. Original admission is protected by the funded discardable DFS, fresh-path capacity check, and decoded-child release before the next map page. Fixed Storage and real UDS multiblock tests cover the mounted implementation. Whole-SDK performance attribution retains the source-comparison limits recorded below. The grouped multipage prototype is a separate, unadopted performance design. Current `ColdRun` already emits grouped phase-0 packets and the existing merge consumes them. Extending one run across grouped pages needs a bounded writer, the corresponding reader validation and independent output/EOF/Work/Original tests. It is not an unfinished connection required by the implemented scan/replay repair.

The etcd Record already charged790,743 token mutations over1,117 complete source preparations and869,734 mutations total before its byte refusal. The direct and SQL paths each admit one mutation per indexed run. At the time of this Record, refusal of both direct output and funded arena replay could cause the SQL fallback to reparse and bill its actual second traversal. The captured prefix has no SQL output branch. The fixed direct-fill test retains exactly one mutation per indexed run even when the old packet allocation disappears. Phase-0 posting lanes33/38/39 add zero mutations. Directory traversal also adds zero mutations in the executed oracle. The existing generated-Large harness enforces a token-mutation floor before its own daemon execution and uses a separate profile when that floor exceeds the fixed limit.

The canonical census of the original 1,421-file etcd manifest is **VERIFIED**. The shared command ended before admission with exit143. A dedicated cache lane then ran the same audit once through `./scripts/cargow --lane token-census-lane run --locked --offline -p quanta-index-lq-text-normalizer --example original_token_census` in the copied source, with `QUANTA_INDEX_RESOURCE_ADMISSION=0` for this standalone census. It compiled in3.94s and exited0. The audit uses the production `TokenCursor::try_new_bounded` NFC and `is_indexable` path. All 1,421 file hashes and the exact manifest were checked before and after. All 35 captured normalizer, vendored NFC, lockfile and toolchain inputs match the released daemon.

The complete corpus contains **1,009,664 indexed token runs**, including927,696 in1,392 ASCII files. The count is valid; the earlier inference that it was an unconditional SDK mutation floor is superseded by the [producer-reuse correction](#final-main-revalidation-and-mutation-rca). That inference assumed a newly staged row for every run. Current verified prefix and whole-body reuse can avoid those row writes without changing fees. The original SDK still submits one `SourceTransition` with `FIXED_FOREGROUND_WORK_LIMITS`. The observed SDK failure at this checkpoint was bytes, not a separately executed mutation refusal; complete current SDK mutation demand remains to be measured.

The read-only fallback audit confirmed a separate duplication case. If the direct output grant and SQL replay credit were both refused, the former path could tokenize the already validated source again. The production disposition and the 257-token no-credit oracle are recorded in [Directory scan continuation and SQL replay](#directory-scan-continuation-and-sql-replay). This is a specific duplicate traversal, not a proof that every future producer must exceed the fixed mutation cap.

The first no-default Large run on daemon `f87b6b3d239a…` reached the shared `native_product_process` test driver and exited101 with zero bodies. The compiler rejected two unused publication-handoff fields in `tests/native_product_process/native_boot.rs` because their only consumer is a `legacy-tantivy` test. The fixture now gates both fields, their computation and initialization on that feature. The default-off Native cross-root refusal test is unchanged. The exact Large selector acquired the build/test lease again with the same pinned daemon after the failed command terminated. The corrected driver compiled in2m13s, and nextest `8aefe725-648f-4fcb-8a4b-7e8aeacfd510` started the one selected body with56 skipped. Process inspection observed the exact pinned daemon serving `native-config.json`. The body **VERIFIED** in143.167s, exit0 (1 passed, 56 skipped). It published 4,096 files and compared source bodies, owner identities, repository and global ranks across two daemon restarts. Current source capture finds only this fixture change and the previously reviewed History `cfg(test)` change. Both release dependency files exclude the changed `native_boot.rs` test fixture. The post-body 2,812-file capture preserved that same two-path difference and HEAD. The daemon and helper digests still match their source-bound SDK runs.

F's existing paired Caddy Off qualification `/private/tmp/qidx-sdk60-caddy-catalog-off.mwOL3o` owns both rounds for the released pair. The duplicate waiter was stopped before any SDK body. The retained 20-minute quiet-host window terminated with **0/2 rounds started**. The last gate in that window was load24.569336 against8.0 and11 foreign Rust processes. Both bodies remain NOT_RUN. The post-Large canonical preflight refused at load35.196777 and19 foreign Rust processes. When admission is available, bind the retry to the same original input and binaries and account for the exact test-only fixture change alongside the previously reviewed History test change. The full product criterion still requires the original etcd Record and both etcd Off rounds on the same accepted pair.

#### Admission decision and remaining product verification

See [the current TEXT acceptance ledger](#current-nc-text-acceptance) for the
latest candidate and the remaining quiet Off runs. The following admission
decision belongs to the earlier Directory candidate.

Reuse the bound Caddy Record and same-daemon Large PASS for their recorded
candidate. The post-reboot Caddy quiet Off window also ended at **0/2**.
The pinned daemon and helper hashes match the original Record.
[F's admission record](NC-F-integration.md#f-로컬-합류--2026-11-16) owns
the retry and host outcome. The earlier assertion that the original etcd
completion condition inherently conflicts with the mutation cap is superseded
by the producer-reuse correction below. Current production keeps all fees,
the 512MiB byte limit, each1,000,000 count limit and the60s deadline. A changed
producer must be tested against the original atomic FULL SDK contract; an
unchanged failed candidate or a weaker target cannot close NTX4.

No alternate funding profile is selected for this closeout. The original `native_corpus` directly selects fixed Work and has no environment or CLI limits override. Keep the exact1,421-file manifest, one FULL source transition, pinned daemon, result identities and reopen checks. The former1,088,655 extrapolation assumed every raw token run required a fresh mutation; it is not a current bound or funding target. Current acceptance requires the original fixed profile after code changes, not an explicitly enlarged diagnostic budget.

#### Directory cursor allocation repair

The [Directory custody contract](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody)
is implemented. Borrowing the admitted fence removes a temporary lease and a
key copy per descriptor transition. The owning tight-memory oracle and the
remaining Native SDK verification are recorded below.

On dirty HEAD `ade5fadc1a084fbb81eba831861e996081d80867`, the first no-default Lexical compile stopped at two existing fixture errors. The planner test now imports `LqPredicateArg`, and the SourceRecord fixture clones its prior record before constructing the invalid variant. The corrected fixture build then ran the independent tight-memory case on the old Directory production path. Nextest `fc44b627-a895-48c1-9c7c-ed6a17faaa65` returned `ResourceLimited` with one executed failure and exit100. The fixture independently funds one exact leaf parse and checks the two output rows, EOF, two child reads and grant release.

After the repair, `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E 'test(native::text::tests::occurrence_blocks::) | test(native::text::tests::shared_entry_maps::) | test(planner::tests::) | test(changed_body_enters_token_build_and_bad_reuse_metadata_refuses)'` passed74/74, skipped509, exit0 (`ed9051a9-f98e-429c-bee7-be904997ec55`). Coverage includes cold multiblock readback, malformed intervals, late child refusal, retry and funded memory release. The Directory source passes its format check. Native SDK for the modified production source is NOT_RUN. The cached pair for Caddy Clean Off remains the previously source-bound release.

The cached-pair Caddy Clean Off controller at `~/Library/Caches/quanta-index/native-sdk-caddy/off-20261117.7LqaiU` verified both original executable hashes and the original manifest. Its admission window ended at0/2 without starting an SDK body. The direct follow-up timing check exited1 at load76.753418 against8.0 with19 foreign Rust processes. Clean Off remains blocked. The earlier Caddy Record and Large results remain scoped to the pinned release.

#### Directory scan continuation and SQL replay

The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#sequential-cold-alias-scan-and-sql-replay)
records the mounted forward Directory scan and borrowed SQL rows. Storage
continues from the last delivered fence after either callback memory pressure
or excess DFS capacity from an earlier, deeper path. SQL fallback consumes
the validated arena in 64-row batches. It retains the existing row bindings,
Scratch admission and fees without a second Rust token backing. The patterns
are consistent with [Lucene `TermsEnum`](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/TermsEnum.html)
and [SQLite parameter binding](https://www.sqlite.org/c3ref/bind_blob.html).

The complete Core, Storage and Lexical no-default lib rail is **VERIFIED**.
The fixed 3,252-path source cohort at
`/private/tmp/qidx-ss-cold-proof.47fQi2Xz/snapshot` has the same dirty HEAD
`ade5fadc1a084fbb81eba831861e996081d80867`. The 16 edited Rust paths matched
Main at capture. The final check found 15 still identical and a later
test-only addition/assertion edit in `native/text.rs` from the concurrent
repository-path lane. The production hunks in that file match the cohort.
The copied checkout and isolated build cache avoid the occupied shared
build slot. This run used `QUANTA_INDEX_RESOURCE_ADMISSION=0` and
`QUANTA_INDEX_TARGET_GC=0` only for isolated test execution, without changing
product limits or making a quiet timing claim.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-core -p quanta-index-storage \
  -p quanta-index-lexical --lib --no-tests fail --no-fail-fast \
  --status-level fail --final-status-level fail
```

After forcing compiler freshness for all edited lib paths, nextest
`8e9d485d-bee4-4280-9799-f6d8e86e2f2e` exited 0 with
**1,172/1,172 passed**, two existing skips, in 17.009s. The new regression
checks cover actual FileObjectStore and ScopedReads traversal, exact scalar
Work after optional state eviction, deep-to-shallow capacity release,
decoded-child release before the next map read, cancellation and sticky
genuine refusal. The SQL fixture exhausts all 257 token mutation admissions
before fallback, funds only the live input backing, and compares independent
positions and packed output. The SQL writer fixture also checks the fixed
two-row write and 132-byte charge with zero extra Rust memory grants.
An earlier test expected refusal at the removed SQL string allocation. It
now verifies successful insertion and those unchanged fees and rows.

The actual Native UDS composition rail is also **VERIFIED**.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-searchd-runtime \
  --test native_index_uds --no-tests fail --no-fail-fast \
  --status-level fail --final-status-level fail -E 'test(text_entry_maps::)'
```

Nextest `cd228003-4186-42a7-8dcf-b4b0c7f67fa7` exited 0 with
**4/4 passed**, 72 outside-selector skips, in 5.784s. The new fixture publishes
257 distinct ASCII words through `SourceAdvance`, crosses the 512-term packed
envelope, and verifies first/middle/last query owners and identical ranked
hits after service reopen. The other selected fixtures cover DSL published
files, DELTA/delete/noop/pinned old views, and old-layout refusal. The initial
copied test mtime left the new fixture absent from the cached binary. An
explicit source timestamp refresh, compiler rebuild and selector listing
resolved that problem before this four-body run. All edited lib paths were
also forced fresh for the final 1,172-body result above.

The cursor-only qualification above ended before a new original SDK run.
Its canonical quiet preflight exited 1 at one-minute load 56.19043 against
8.0 with 34 Rust processes classified as foreign by the gate. Earlier Caddy
Record and Large evidence remains bound to its original release. The original
etcd input contains1,009,664 indexable runs; this raw census is not the mutation
count of the later reusable producer. The1,000,000-mutation ceiling and fee
schedule remain unchanged.

#### Fresh source-bound original SDK closeout

The resumed closeout uses
`/Users/songmin/Library/Caches/quanta-index/native-sdk-caddy/remaining-20261117-1ca5jn5t`.
The release was compiled from Main `3a222aa58497113199a9b1205d5389c10309571c`.
All 2,945 captured source/build inputs, including shared Cargo path packages,
match across the before/after/final build captures. Main advanced to
`49b4e7187f02eaf64bf4d7ec36c72d794b757bd7` before SDK execution. The complete
post-Record source inventory still matches every captured input and the path
set. This is source-snapshot qualification with the recorded dirty inputs.
It makes no clean-checkout or quiet-host claim.

| Scope | Exact target or selector | Observed result |
| --- | --- | --- |
| Current Core, Storage, Lexical | `-p quanta-index-core -p quanta-index-storage -p quanta-index-lexical --lib` | **VERIFIED**, nextest `95da40f6-6f64-4cd5-8311-6acaade81a1d`, 1,177/1,177, two existing skips, 18.503s, exit 0 |
| Native UDS Text | `-p quanta-index-searchd-runtime --test native_index_uds -E 'test(text_entry_maps::)'` | **VERIFIED**, nextest `10bdae21-13db-4753-90f0-13cb05c940a8`, 4/4, 73 outside-selector skips, 5.620s, exit 0 |
| Same-daemon Large | `-p quanta-index-searchd-runtime --test native_product_process -E 'test(=large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks)' --test-threads 1` | **VERIFIED**, nextest `49d9659e-636c-4bb1-987b-9406071ed3bc`, 1/1, 56 outside-selector skips, 149.270s, exit 0 |

These rails use `./scripts/cargow --lane test-workspace-lane nextest run
--locked --offline --no-default-features --no-tests fail --no-fail-fast
--status-level fail --final-status-level fail` with the selected arguments
above. Local tests ran in the existing warm isolated checkout after updating
all captured inputs from Main. Large received the pinned daemon path and SHA
through the existing `QIDX_NATIVE_LARGE_RELEASE_BIN` and
`QIDX_NATIVE_LARGE_RELEASE_SHA256` parameters.

The shared DSL build was admitted but its compiler pair showed no CPU/output
progress during observation. Our first lib waiter timed out before admission;
the second was cancelled before admission. Both have zero executed bodies.
The successful isolated tests and release compilation used the supported
cooperative-admission and sccache switches set to 0, with target GC disabled.
The optimized release reused Main's warm release-profile cache and exited 0
in 3m14s with this command.

```sh
./scripts/cargow --lane test-workspace-lane build --release --locked --offline \
  --no-default-features -p quanta-index-searchd-runtime --bin quanta-index-searchd \
  -p quanta-index-searchd-harness --bin native_corpus
```

Record bodies were serialized under the existing SDK mutex. Quiet Off retains
the shared build mutex and canonical timing gate. Product limits and tariffs
did not change. The pinned daemon SHA-256 is
`7e128d78010b26562999bdf26203e553a58437b15e2f36132b1b3f0730d4f607`.
The helper SHA-256 is
`5887adf028ffbcdc656c427dbc9c0e9f3db8f97985c9bc825aa9dc2b135e4a92`.

Both original manifests, complete path sets, file hashes, repository commits,
query witnesses and source-byte digests were checked before and after each
Record body. The missing etcd temporary source directory was restored from
its original commit to persistent cache. The original source-root argument
resolves to that exact 1,421-file, 8,666,217B tree.

Caddy Record is **VERIFIED**. The original 590-file FULL published in
42,099.394ms and passed FileMap, Literal and Text readback before and after
daemon restart. `PYTHONPATH=. .venv/bin/python
tools/benchmark/retrieval/causal_cost_diagnostic.py native-corpus-record
--run-dir <output>/caddy-record-1 --out <output>/caddy-record-validated.json`
exited 0 for both saved traces. They have empty capture health, zero
dropped/unwritten events and zero open spans/requests. The external validator
report owns the trace verdict. Semantic and Hybrid are outside this original
corpus selector.

Etcd Record is **FAILED**, exit 1, with a **COMPLETE_DIAGNOSTIC** first trace.
At `packed_runs.rs:401`, the original posting-edit admission had accepted
R348,240 / W623,894 / B536,855,442 / M869,734 and refused another 17,612B
against 536,870,912B. It completed 1,073 inventory diffs and refused the next.
FULL ACK, search and reopen were not reached. The canonical `native-ingest`
validator exited 0 with empty health, zero omitted and zero open requests.
The trace SHA-256 is
`df3ea6bcefaba9aafb8f5efeb0ba61253c6a0f44e25067546a8e882d43ea825d`.
Other production paths also changed since the earlier release, so this result
does not isolate the performance effect of the scan or SQL repair.

The original raw-token census was also rerun against that candidate's normalizer.
The standalone census is outside the checkout in `<output>/token-census`.
Its canonical `cargow --lane token-census-lane run --locked --offline
--manifest-path <output>/token-census/Cargo.toml -- <restored-original-source>`
exited 0. All 1,421 paths and source hashes, the manifest, and 39 current
normalizer/NFC/Cargo/toolchain inputs were checked before and after. The
standalone lock pins the release's exact normalizer dependency versions,
including tinyvec 1.11.0. The production `TokenCursor::try_new_bounded` and
`is_indexable` path counted **1,009,664 indexed runs**, of which 927,696 are
in 1,392 ASCII files. There are no invalid-UTF-8 skips. The executable SHA-256
is `a63f119a573b1069bda332820ca81e62c958a6ea2cf96e5ffd6ae37caa42c651`.
`token-census/result-binding.json` owns the fresh census. The observed SDK
denial remains bytes. The former claim that this raw count proved mandatory
token mutations above the cap is superseded by verified producer reuse below.
The existing resume path restarts preparation and has no token-position checkpoint. Each
separate SourceAdvance publishes its own view, so splitting one FULL into
several such operations changes the selected atomic publication contract.

Caddy quiet Off is **BLOCKED**. The initial round preflights returned 75 at
one-minute load 29.103027 against 8.0. The follow-up owner observed nine
canonical gate refusals, with the timestamped window spanning 12:15:04 to
12:23:25 KST after an initial unstamped probe around 12:14. Load ranged from
26.541504 to 44.12207 and foreign Rust processes from 0 to 12. New competing
builds started during the window. Both required Off bodies remain unexecuted.
`caddy-off-gate-ledger.json` and the saved controller output own these
observations. Polling ended at this persistent external block, with no
background controller and no foreign process interruption. The user then
selected code-first work. This authorizes the code pass below and leaves the
other running jobs intact.

The scan/replay implementation and its current-source local, original Record
and same-daemon Large executions are closed at their reported scope. Whole
product acceptance remains incomplete. It requires quiet Off admission and
resolution of the original etcd FULL fixed-profile failure. The original
512MiB, per-axis count limits, fee schedule, file cap and SDK60 contract remain
the selected acceptance criterion. The observed byte refusal is an SDK result;
the independently executed raw-token census is not a current mutation floor.

#### Code-first completion review

After the user's code-first direction, three independent source reviews
checked the actual caller chain, failure paths and phase-0 producer/consumer.
All 17 inspected boundary files, including Kernel dispatch, Core budget/port,
Storage adapters and Text/token/packet implementations, are byte-identical to
the source-bound release tested above. The review found no additional
reachable implementation defect. The existing 1,177 lib, four UDS, original
SDK and same-daemon Large results retain their stated scopes.

| Code boundary | Current disposition |
| --- | --- |
| Cold Directory scan | Implemented and mounted through Text's real ScopedReads; funded continuation, scalar tail, child identity/seal/EOF and cancellation are covered. |
| Cold SQL replay | Implemented from the validated arena; borrowed batches keep existing admission/fees; cached bindings clear on Result success/failure; later errors abort the attempt-local spool. |
| Cold phase-0 packet and merge | Existing ColdRun grouped packet writer and merge consumer are connected. The original per-edit admission at the etcd refusal remains the selected tariff. |
| Grouped multipage run prototype | Optional new performance implementation, requiring both a bounded writer and corresponding reader/proofs; it is not a missing call to an already complete producer. |

The prototype-only wording above was corrected to prevent the completed Text
scan from being reassigned as unfinished work. No new regression patch was
supported by a reachable failing witness in this code pass. Quiet-host
qualification and the original etcd funding contradiction remain the product
acceptance residuals described above.


#### Multipage writer implementation and regression closeout

The follow-up request implements the previously optional prototype. It does
not change the selected 512MiB Work limit, per-edit tariffs, count limits,
SDK deadline or durable Text format. Prior release receipts above remain
historical; they do not qualify this new implementation.

`PreparedInventoryChanges` now admits bounded cold owner windows, merges one
checked head per owner by `(term, owner)`, and streams the existing posting
edit API into an ordered phase-zero run. The writer emits consecutive pages
without sorting/recopying an owner-major packet. The existing grouped private
scratch format is readable for every cold phase-zero page, including run 0.
An Original-funded run seal records complete page and row counts. Readers
reject missing/extra pages, row-count mismatch and cross-page order changes.
Unfinished, cancelled or partially failed runs poison the disposable spool.

Inline and ordinary packed inventories borrow authenticated block keys;
they do not create a one-row MapPage before copying the heap head. Directory
aliases prefetch bounded descriptor pages through the canonical single-DFS
visitor, then open one checked child at a time. Key/descriptor seals and token
totals must pass at every owner EOF before completion. Directory selection
requires current-attempt canonical producer proof and an explicit backend
memory bound. FileObjectStore and ScopedReads provide that capability; the
ScopedReads continuation avoids retaining window child leases in its outer
cache. Unsealed legacy layouts and unknown backend bounds keep their existing
route. A later optional-credit miss processes only the unread suffix; it
never replays an already prepared prefix. Spool-owned metadata is parent
funded only after the window credit succeeds. General spools decline the
optional route without a mandatory resource failure.

The meaningful pre-change RED executed the existing producer and writer
with the new read-only SQL layout oracle. Nextest
`8c68baba-622c-462a-bd32-f49a4a4a038f` ran one body and failed because no
phase-zero page beyond page 0 existed. The initial internal-credit assertion
failure was not used as the behavioral witness.

Regressions cover the production batch path with 1,831 exact postings across
an optimized inline prefix and legacy Directory/Small suffix; a separate
ordinary packed owner; a 65-owner/1,950-posting later-credit miss with no
prefix replay; complete fixed tuples and EOF; fixed independent alias seals;
wrong attempt/reference/count/token totals; original memory refund; grouped
multipage writer/merge; missing, extra and corrupt second pages; partial
write/cancellation poisoning; and identical Off/Record Work. The real Native
UDS regression publishes four owners with 257 terms each and verifies first,
middle and final terms, case variants, exact owners/paths and ranked hits
before and after reopening the service.

Verification runs use the existing warm isolated checkout
`/private/tmp/qidx-ss-cold-proof.47fQi2Xz/snapshot`, based on the previously
validated dependency/source cohort plus the owned changes. Thirteen complete
owned source/test files and the three capability/helper hunks were compared
with Main. Concurrent unrelated Core/Storage hunks were deliberately not
copied. This is focused local regression proof, not a clean Main/exact-commit
or performance qualification. The supported cooperative-admission, target-GC
and sccache switches were 0, as in the earlier isolated runs.

Final command/result entries below include the last lint repairs.


| Final local scope | Observed result |
| --- | --- |
| Core + Storage + Lexical libraries | **VERIFIED**, 1,186/1,186 passed, two existing skips, 22.217s; nextest `ffe8ab3f-d1e8-47d0-ae23-a6cde2f7dc0d`, exit 0. |
| Native UDS Text | **VERIFIED**, 5/5 passed, 73 outside-selector skips, 5.876s; nextest `5a6ab793-b1ef-41e3-8e2b-ddd946d792fa`, exit 0. |
| Hexagonal dependency boundaries | **VERIFIED**, `just rust-hexagonal`, exit 0. |
| Changed Core module boundary | **VERIFIED**, `python3 tools/ci/lint/check-cargo-modules-snapshot.py --packages quanta-index-core`, exit 0, unchanged module tree. |
| Default guarded module check | **FAILED**, `just rust-cargo-modules`, exit 1: concurrent Contract legacy import/source-restore module drift; Core and Kernel unchanged. No unrelated baseline was rewritten. |
| Strict Clippy | **FAILED**, selected package rail below exits 101 on existing diagnostics. Initial dependency-lint execution also fails vendor ciborium MSRV. The final `--no-deps` Storage/Lexical producer reports 487 existing errors and zero primary diagnostics on the added production lines. This is not a green Clippy claim. |
| Current implementation original SDK / quiet timing | **NOT_RUN**. Earlier original Caddy/etcd and Large receipts above do not cover the multipage changes. The unconditional etcd mutation-floor claim is corrected in the RCA below; complete-inventory-only reuse remains insufficient under the current edit streams. |

Commands executed in the isolated checkout:

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-core -p quanta-index-storage \
  -p quanta-index-lexical --lib --no-tests fail --no-fail-fast \
  --status-level fail --final-status-level slow

./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-searchd-runtime --test native_index_uds \
  --no-tests fail --no-fail-fast -E 'test(text_entry_maps::)' \
  --status-level fail --final-status-level fail

./scripts/cargow --lane test-workspace-lane clippy --locked --offline \
  --no-default-features --no-deps -p quanta-index-storage \
  -p quanta-index-lexical --lib --message-format=json -- -D warnings
```

The first complete library run exposed six general-spool regressions:
packing cold occurrence owners does not imply that the posting spool itself
uses cold mode. The optional parent-preparation entry point now declines a
general spool without changing its mandatory error/refusal semantics. All
six existing alias handoff/retained-read/cancellation/partial-completion
regressions pass in the final whole-library result. Final lint repairs also
borrow large metadata at the new cursor/writer seams, keep one inline anchor,
and use checked arithmetic; no new allow-attribute or tariff exception was
introduced. The implementation and focused functional regression scope are
closed. Repository-wide lint, exact-Main/release and performance/product
qualification remain the explicit separate scopes above.


#### Final Main revalidation and mutation RCA

##### 04.4 TEXT regression migration, 2026-11-22

The Legacy IDs below are taken from Git `d8a490565`. Each row preserves the
public behavior where an equivalent Native or pinned old-Q oracle exists.
An old-Q SDK body verifies compatibility with that executable. Native
parity requires a Native body. A partial mapping keeps its missing
assertion open. The 43 `sealed_manifest` integrity IDs
remain with the existing Lexical DATA/Runtime owners. Root/F owns the
disposition inventory and final shipping gate.
[04.4 owner and acceptance](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**Native test locations.** [Query fixtures](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query.rs),
[repository predicates](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/repo_predicates.rs),
[published fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/text_entry_maps.rs),
[layered fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs),
[boundary rebuild](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/text_shard_contract.rs).
All Native rows use the existing configuration and tariff.

**Pinned old-Q.** The focused command pins daemon SHA-256
`c013632a9770a5b1966b7d25896866946993476d7f7797957347b1554c4ebcd7`.
`sourcegraph_text_route_authorities_share_one_indexed_fixture` runs in
`legacy_text_route_hellgate` and checks public SDK text queries on the same
published fixture. Test names in the tables name source oracles.
Execution status is recorded separately.

| Removed `tantivy_smoke` ID | Owning test and retained assertion |
| --- | --- |
| `exact_path_constraint_is_applied_before_limit_and_on_index_no_scan_v1` | Native `exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally` checks a lower-ranked file at limit 1. Old-Q `l1_real_daemon_sdk_domain_primitives_and_symbol_windows` checks an exact scoped manual scan. |
| `exact_path_constraint_only_query_treats_dsl_metacharacters_as_literal_v1` | Native `exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally` covers typed literal path identity in an anchored query. Native Text rejects empty content queries. `symbol_dispatch_admits_only_typed_exact_path_as_constraint_only_authority_v1` checks the separate Symbol dispatch contract. |
| `independent_chunk_clear_refuses_and_file_tombstone_preserves_symbols_v1` | Old-Q `sdk_tombstone_only_generation_replaces_active_composite_and_removes_both_query_views` checks `INVALID_REQUEST`, absent G2 directories, unchanged active head, and preservation of the unrelated Symbol and G1 views after a whole-file tombstone. Native `layered_text_full_patch_pinned_views_and_restart` owns whole-owner deletion and retained Text across restart. [Test and evidence](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#legacy-sdk-custody) |
| `index_no_reuses_content_tokens_within_document_without_cross_document_leak` | Old-Q `lexical_full_fidelity_matrix` checks a same-document keyword, phrase and content conjunction and a cross-document miss. `every_syntax_case_spelling_and_route_answers_the_golden_set` covers query and case spellings. Native UDS file containment checks same-source conjunction. |
| `language_constraint_is_pushed_into_one_pre_limit_candidate_query_v1` | Old-Q `lexical_full_fidelity_matrix` checks the unrestricted and Python-filtered `top_k=1` hits, exact OR membership, and identical candidate order with a duplicate language. `l1_real_daemon_sdk_domain_primitives_and_symbol_windows` checks scoped language contradictions. Native `native_text_dsl_fixture_returns_exact_published_files` verifies typed refusal of ordinary language filtering. The internal one-query Tantivy assertion is retired. [SDK22 oracle](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#legacy-sdk-custody) |
| `symbol_content_authority_shapes_fail_closed_on_initial_and_replayed_batches` | The same-named old-Q SDK test checks Symbol keyword results, `count:all`, Text Symbol projections, and typed parser and planner refusals after the initial publish and after same-batch replay and daemon restart. [Test and evidence](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#legacy-sdk-custody) |
| `tantivy_applies_query_boost_to_scores` | Old-Q `explain_score_traces_share_one_indexed_fixture` checks a 2.5x score/explanation and `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks 5x ordering. Native Text rejects boost. |
| `tantivy_count_all_keeps_the_page_and_reports_the_exact_total` | Old-Q `e2e_exact_count_window::count_all_keeps_the_page_and_reports_the_exact_total` checks exact total, unchanged page and cursor. Native Text rejects count:all. |
| `tantivy_executes_file_has_content_predicate_phrase_and_regex` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks scoped phrase hits and misses. The post-freeze assertion adds scoped regex hits and a wrong-path miss to the same selector. Native `file_contains_substrings_intersect_on_one_published_source_before_top_k_over_uds` covers substring scope. |
| `tantivy_executes_index_no_full_scan_with_scoped_content_predicate` | Old-Q `l1_real_daemon_sdk_domain_primitives_and_symbol_windows` covers the manual file filter. The route fixture pins the candidate lists for grouped scoped OR and AND-NOT, and the typed refusal for a directive on just one OR branch. See the focused follow-up for execution status. |
| `tantivy_executes_native_predicate_aliases` | Native `repository_path_aliases_and_scalar_file_matchers_gate_the_source_repository` checks repo.has.path and repo.contains.path. Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks repo.contains.content. The file.contains.content alias remains unbound to an executed replacement. |
| `tantivy_executes_numeric_content_predicates` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks numeric file and repo predicates against the one published 123 witness and absent 404. |
| `tantivy_executes_phrase_adjacency_without_unordered_match` | Native `phrase_query_matches_only_adjacent_terms_through_native_uds` checks ordered, reversed and gapped source text. |
| `tantivy_executes_repo_allow_list_across_indexed_source_repo_ids` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks exact source repo allow-list rows. Native `legacy_repo_metadata_and_repo_surface_are_explicitly_unavailable_on_native_text` checks typed refusal. |
| `tantivy_executes_repo_file_path_and_lang_filters` | Native `exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally` covers exact path; `repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` covers scoped repo file/language. Ordinary repo/lang filters have distinct Native refusal and old-Q coverage. Three-way conjunction remains partial. |
| `tantivy_executes_repo_has_content_predicate_under_or_and_not` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks repo content in OR and NOT with fixed owner paths. Native rejects repo.has.content. |
| `tantivy_executes_repo_has_file_predicate_as_repo_gate` | Native `repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` checks the repo gate before rank. |
| `tantivy_executes_repo_has_file_predicate_under_or_and_not` | Native `repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` checks Boolean repo gates and a pinned earlier view. |
| `tantivy_executes_repo_has_file_predicate_with_lang_matcher` | Native `repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` and `repository_language_gate_refuses_missing_metadata_over_uds` cover correlated language and missing metadata. |
| `tantivy_executes_repo_metadata_filters_when_bundle_payload_is_typed` | Old-Q `end_to_end::repo_metadata_filters_share_one_indexed_fixture` checks combined positive filters, while `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks typed repo metadata predicates. Exact per-filter miss/reopen remains partial. Native refusal is tested separately. |
| `tantivy_executes_scoped_file_content_predicates_across_boolean_contexts` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks anchored name/path phrase, OR and NOT. Native `file_contains_substrings_intersect_on_one_published_source_before_top_k_over_uds` checks conjuncts evaluated on the selected Source. Numeric, multiple scalar and language permutations remain partial. |
| `tantivy_executes_simple_path_term_surface_without_boolean_path_leakage` | Native `native_select_path_combines_path_and_body_matches_after_cold_reopen` checks body/path distinction, NOT, paging and reopen. |
| `tantivy_executes_supported_type_and_select_filters` | Native `native_select_path_combines_path_and_body_matches_after_cold_reopen`; old-Q `a_projection_reports_its_collapsed_universe_exactly` verifies the public path projection count. Other Legacy type/select surfaces are individually mapped or remain partial. |
| `tantivy_executes_whole_document_regex_and_rejects_false_positive` | Native `alternation_regex_returns_documents_matching_any_branch` and old-Q `lexical_full_fidelity_matrix` check complete-document branch truth and non-matching candidates. |
| `tantivy_honors_case_sensitive_keyword_queries` | Old-Q `every_syntax_case_spelling_and_route_answers_the_golden_set` checks fixed case-sensitive and folded query results. |
| `tantivy_index_no_language_refuses_without_stored_authority` | Native `native_text_dsl_fixture_returns_exact_published_files` checks typed language refusal. Exact old-Q manual scan with missing stored authority remains open. |
| `tantivy_index_round_trip` | Native `native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold`, `native_text_dsl_fixture_returns_exact_published_files` and `block_queries_preserve_fixed_gold_across_delta_delete_noop_and_reopen` cover publishing, known results and reopen. Old-Q `lexical_full_fidelity_matrix` covers remaining Legacy route spellings. |
| `tantivy_phrase_sidecar_uses_text_authority_and_case_rules` | Old-Q `every_syntax_case_spelling_and_route_answers_the_golden_set` checks phrase/case rules. Native `phrase_query_matches_only_adjacent_terms_through_native_uds` checks source-derived phrase truth. The post-freeze assertion binds case-sensitive and folded results to the staged Source reference. The adapter sidecar implementation retires. |
| `tantivy_regex_sidecar_verifies_authoritative_text` | Native `case_folded_literal_regex_agrees_with_the_keyword_route` and `native_text_preview_keeps_rank_and_source_coordinates_across_reopen` check the admitted regex and selected Source. Retire the adapter-sidecar representation. |
| `tantivy_repo_has_content_phrase_and_raw_string_true_gate_narrows_by_indexed_source_repo_id` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks phrase/raw gate rows by source repo. Native refusal is separately covered. |
| `tantivy_repo_has_content_true_gate_narrows_by_indexed_source_repo_id` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks cross-repo positive and absent owner paths. |
| `tantivy_repo_has_file_true_gate_narrows_by_indexed_source_repo_id` | Native `repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` plus `repository_path_aliases_and_scalar_file_matchers_gate_the_source_repository` check explicit source repository correlation. |
| `tantivy_search_all_materializes_full_scope` | Old-Q `a_tied_corpus_walks_page_by_page_in_path_order` and Native `global_text_pages_keep_original_view_across_publication_and_release` verify complete traversal over one pinned view. The direct adapter materialization API retires. |
| `tantivy_select_file_collapses_multiple_chunks_per_path` | Old-Q `select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total` verifies distinct file paths and exact collapsed count after publishing 15 chunks in one file. Native Text operates on source owners and refuses select:file. |
| `tantivy_select_file_fills_page_after_more_than_ten_duplicate_chunks` | Same old-Q test checks raw top two duplicate-file hits and the next distinct file in the projected top two. |
| `tantivy_select_file_top_ten_covers_ten_files_despite_fifteen_duplicate_chunks` | Same old-Q test checks the complete fixed set of ten file paths and exact terminal count. |
| `tantivy_select_path_collapses_multiple_chunks_per_path` | Old-Q `a_projection_reports_its_collapsed_universe_exactly` and Native `native_select_path_combines_path_and_body_matches_after_cold_reopen` cover path page/count and identity. The post-freeze old-Q `select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total` adds the `select:path` same-path multi-chunk assertion. |
| `tantivy_select_repo_uses_canonical_representative_when_scores_tie` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks one result per repo. The equal-score representative tie remains open. Native select:repo refusal is covered. |
| `tantivy_top_k_stabilizes_keyword_path_surface_without_losing_content_hits` | Native `native_select_path_combines_path_and_body_matches_after_cold_reopen` compares mixed path/body and continuation. Exact old top-two candidate order remains partial. |
| `tantivy_type_path_collapses_multiple_chunks_per_path` | Native Text rejects type:path. The shared disposition inventory binds the pinned old-Q SDK test `process_readiness_owner_v1::e2e_exact_count_window::type_path_collapses_multiple_chunks_before_top_k_and_reports_exact_total`. Its fixed oracle is two distinct paths and `Exact(2)`, with continuation at `top_k=1`. This selector was added separately from the TEXT 27 + 3. Its owner attempt was **BLOCKED** at admission, with the body **NOT_RUN**. [Old-Q body](../../../../crates/quanta-index-searchd-runtime/tests/e2e_exact_count_window.rs#L249) · [Root/F disposition](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#044-id별-유지퇴역-판정) |
| `tantivy_type_repo_collapses_to_repo_representative` | Old-Q `sourcegraph_text_route_authorities_share_one_indexed_fixture` checks one representative per source repo via type:repo. Native refusal is separately covered. |

| Removed `text_authority_shards` ID | Owning test and retained assertion |
| --- | --- |
| `a_delta_refuses_a_corrupt_selected_base_page` | The old adapter is retired. The Native TEXT writer regression uses a selected V2 posting page consumed by SourceAdvance compaction and checks the recoverable operation contract below. **NOT_RUN**. |
| `a_whole_corpus_layout_generation_is_refused_by_text_authority_format` | Reuse the focused DATA body `legacy_existing::lexical_existing_whole_corpus_manifest_requires_rebuild`, **VERIFIED · 1/1 · exit 0** on the `3128c36a…` input, Nextest `3739620b-a0ff-451d-897f-e5a32fc146eb`. It checks typed rebuild refusal and verification after restoration. The adapter open and inherited DELTA entrypoints retired with the format. Current release evidence custody remains with DATA and Root/F. [DATA evidence](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#044-id별-유지퇴역-판정) |
| `an_unsealed_source_batch_cannot_leave_index_authority_skew` | Retire the adapter `seal=false` entry. Source atomicity retains `every_preflight_refusal_over_raw_ipc_changes_nothing` and `sdk_publishes_carry_the_canonical_digest_and_refusals_record_nothing`, and Native layered restart checks published state. Exact partial old-generation shape remains outside the Native Text API. |
| `boundary_documents_answer_like_an_independent_rebuild` | Native `native_boundary_delta_matches_independent_full_and_cold_reopen` checks replacement, rename, removal and insertion across 128 document boundaries using independent FULL and fixed owner/path results. |
| `changed_native_root_is_refused_at_both_doors` | Retire the direct adapter manifest/open doors. `lexical_existing_real_generation_authenticates_full_physical_bytes` and `native_boot_selection_verification_and_selected_serve_preserve_real_publication` preserve persisted read authentication and Native Boot refusal. Exact three-fault/two-door matrix remains partial with DATA/BOOT. |
| `one_scope_delta_preserves_unaffected_paged_segments` | Native `layered_text_full_patch_pinned_views_and_restart` and `layered_text_dense_layers_split_one_partition_and_survive_restart` cover unchanged owners and immutable layer reuse. Retire the old shard-write count and file-byte layout oracle. Native Work is independently qualified. |
| `selected_native_pool_page_faults_are_typed` | The legacy pooled-page query wire is retired. The Native TEXT reader regression checks typed online refusal for three physical posting-page faults below. **NOT_RUN**. |

The retained `sealed_commitment_cost` IDs already use the public SDK and the
external daemon. `file_admission_counts_verified_changed_sources_separately`
checks changed-file admission counters. `a_delta_seal_rehashes_coverage_but_inherits_other_unmodified_files`
checks rehash work and immutable file reuse. Their existing target remains the
owner. The disposition inventory records the ID decision. The runner below
records the executed outcomes.

**Focused run.** The selection binds **27 exact tests** to the current
post-retirement working-tree source manifest and Cargo.lock
`3128c36a74df…`, with the exact old-Q digest above. Cargo metadata
verified five ungated binaries, including the retained module selectors
under `process_readiness_owner_v1` and `runtime_fast_suite`. The 01:58 KST
attempt returned **BLOCKED · 0/27 · exit 124** at its 60-minute shared
build admission limit. The 02:58 KST retry was stopped at 03:34 KST
before admission (**0/27 · exit 143**). The dedicated external SSD run
uses the same source manifest, Cargo.lock, daemon digest and 27 exact
names. Its Cargo build concurrency is capped at two. It collects the
Nextest JSON inventory and JSONL terminals from the same prepared binaries
with pre/post SHA checks. Neither shared attempt started Rust compilation
or test bodies.
[First terminal](</Users/songmin/Library/Caches/quanta-index/nc-text-04-4.brp832zm/result.json>) ·
[Pre-admission retry log](</Users/songmin/Library/Caches/quanta-index/nc-text-04-4-retry.gt8i6_f2/nextest.log>) ·
[External run controller](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/controller.log>)

The external baseline hit **FAILED** during compilation of
`runtime_fast_suite`. The shared fixture carried a module-level
`#[expect(dead_code)]` that no surviving declaration fulfilled. The
working tree already removes that attribute. The replay keeps the
original inputs and selectors except for those four fixture lines. It
applies the same fix to the follow-up input. No test assertions changed.
[Compiler diagnostic](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/baseline/build.log>) ·
[Corrected baseline manifest](</Users/songmin/Library/Caches/quanta-index/nc-text-04-4-lint-source.wzj7wj5g/baseline/source-manifest.json>) ·
[Replay controller](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/lint-fixed/controller.log>)

The corrected baseline compiled and ran **27/27**. Nextest
`14bdc16b-3050-4f40-9b00-46d878b6a257` returned **FAILED · exit 100**,
with **25 PASS / 2 FAIL**, including **2/2 PASS** on retained cost
contracts. The original failing run remains intact. Its Nextest suite
failure prevents the whole run from satisfying the release evidence
parser.
[Nextest log](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/lint-fixed/baseline/nextest.log>) ·
[Exact inventory](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/lint-fixed/baseline/inventory.json>) ·
[Raw test events](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/lint-fixed/baseline/events.jsonl>)

<details>
<summary>Corrected baseline · 27 exact Rust terminals</summary>

- `quanta-index-lexical::sealed_commitment_cost a_delta_seal_rehashes_coverage_but_inherits_other_unmodified_files` · **PASS**
- `quanta-index-lexical::sealed_commitment_cost file_admission_counts_verified_changed_sources_separately` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::global_text_pages_keep_original_view_across_publication_and_release` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::preview::native_select_path_combines_path_and_body_matches_after_cold_reopen` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::regex_alternation::alternation_regex_returns_documents_matching_any_branch` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::regex_alternation::case_folded_literal_regex_agrees_with_the_keyword_route` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::repo_predicates::repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::repository_query_contract::legacy_repo_metadata_and_repo_surface_are_explicitly_unavailable_on_native_text` · **FAIL**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::repository_query_contract::repository_path_aliases_and_scalar_file_matchers_gate_the_source_repository` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::text_query_contract::exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::text_query_contract::file_contains_substrings_intersect_on_one_published_source_before_top_k_over_uds` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds read_sessions::query::text_query_contract::phrase_query_matches_only_adjacent_terms_through_native_uds` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds text_entry_maps::block_queries_preserve_fixed_gold_across_delta_delete_noop_and_reopen` · **PASS**
- `quanta-index-searchd-runtime::native_index_uds text_entry_maps::native_text_dsl_fixture_returns_exact_published_files` · **PASS**
- `quanta-index-searchd-runtime::native_product_process segmented_layered::layered_text_full_patch_pinned_views_and_restart` · **PASS**
- `quanta-index-searchd-runtime::native_product_process text_shard_contract::native_boundary_delta_matches_independent_full_and_cold_reopen` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_exact_count_window::a_projection_reports_its_collapsed_universe_exactly` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_exact_count_window::count_all_keeps_the_page_and_reports_the_exact_total` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_exact_count_window::select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_lexical_full_fidelity::lexical_full_fidelity_matrix` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_ranked_pages::a_tied_corpus_walks_page_by_page_in_path_order` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 e2e_unicode_text_semantics::every_syntax_case_spelling_and_route_answers_the_golden_set` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 legacy_explain_score_trace::explain_score_traces_share_one_indexed_fixture` · **PASS**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 legacy_l1_query_contract_sdk::l1_real_daemon_sdk_domain_primitives_and_symbol_windows` · **FAIL**
- `quanta-index-searchd-runtime::process_readiness_owner_v1 legacy_text_route_hellgate::sourcegraph_text_route_authorities_share_one_indexed_fixture` · **PASS**
- `quanta-index-searchd-runtime::runtime_fast_suite end_to_end::repo_metadata_filters_share_one_indexed_fixture` · **PASS**

</details>

**Failed-body repairs.** The Native SDK rejects unsupported repository
predicates during parsing as `Lexical(ParseFail)`. The seven server-side
Legacy-filter refusals still assert `InvalidInput`. The L1 probe used the
same old-Q SHA and Cargo-linked SDK. It found eight mismatches on the
Sourcegraph spelling of `repo.has.content` and `file.has.content` across
the indexed/manual and typed-language combinations. Native
`lang:python ...` returned its typed no-token error in the probe. The
L1 source now selects the predicate spelling per syntax and includes
the route, query and typed constraints in failure messages.
The diagnostic probe locates the failing cells. The Root batch below
records the subsequent executions of the repaired selectors.
[Native rejection assertions](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/repository_query_contract.rs) ·
[Old-Q L1 assertions](../../../../crates/quanta-index-searchd-runtime/tests/legacy_l1_query_contract_sdk.rs) ·
[Pinned diagnostic matrix](</private/tmp/qtext44-l1.RLBdp8/matrix.log>)

**Post-freeze assertions.** The follow-up overlays three changed test files
onto the frozen manifest and Cargo.lock used by the 27-selector job.
The guarded continuation ended after the two baseline failures.
`rustfmt --edition 2024 --check` passed for the three overlay files.
The three changed bodies went to the same Root batch as the two
repaired selectors.
[Follow-up manifest](</Users/songmin/Library/Caches/quanta-index/nc-text-04-4-lint-source.wzj7wj5g/changed/source-manifest.json>) ·
[Same-binary collection](</Volumes/Extreme SSD 1/build-cache/nc-text-04-4.3xq1d9zm/proof.py>)

- `process_readiness_owner_v1::e2e_exact_count_window::select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total`
  also checks `select:path` on the same 15-chunk source. The ordinary top two
  contain the same file. The projected top two contain two paths, and the
  complete page contains the fixed ten paths. Native SourceCut maps each
  `(source_repo_id, path)` to one owner and refuses duplicate claims, so
  public multi-chunk collapse stays on old-Q. The Native `select:path`
  fixture independently checks equal relative paths in different repos.
  [Old-Q projection](../../../../crates/quanta-index-searchd-runtime/tests/e2e_exact_count_window.rs#L192) ·
  [Native source claim](../../../../crates/quanta-index-kernel/src/source/repository_claims.rs#L30)
- `process_readiness_owner_v1::legacy_text_route_hellgate::sourcegraph_text_route_authorities_share_one_indexed_fixture`
  checks scoped regex on the existing phrase fixture. Path/name scope and
  regex together select `docs/colors.md`, while the same regex on
  `src/corp-a.rs` returns zero rows. Native `file.contains` admits a single
  raw substring and the Native UDS test applies an exact path separately.
  Scoped phrase and regex predicates stay in the old-Q suite along with
  `index:no`.
  [Old-Q scoped predicate](../../../../crates/quanta-index-searchd-runtime/tests/legacy_text_route_hellgate.rs#L745) ·
  [Native admission](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L79)
- `native_index_uds::read_sessions::query::text_query_contract::phrase_query_matches_only_adjacent_terms_through_native_uds`
  checks a folded lowercase phrase against published `Lemon yellow banana`,
  an exact mixed-case hit and a lowercase `case:yes` miss. Positive hits
  carry the expected owner and the staged Source reference from the exact
  published view.
  [Native phrase](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/text_query_contract.rs#L62)

**Remaining owner evidence.** The focused selectors can establish only their
listed assertions. Lexical compatibility carries the pinned old-Q
manual-scan isolation, scoped `index:no` and missing-language refusal,
alias and matcher permutations, exact repository representative tie,
and mixed path/body ranking gap. Symbol/public SDK owns the unrelated Symbol
survivor and initial/replay content-shape assertions. DATA/BOOT and Runtime
own the corrupt or absent selected root/page and shared
FileBody query/scrub/restart fault evidence. Root/F reconciles the proofs
against the original IDs and accepts the final disposition. Release intake
also binds the accepted observations to the committed candidate. The
separate old-Q `type:path` replacement is tracked there.
[Owner evidence and gate](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**Root batch handoff.** The live old-Q route test adds the remaining
public assertions to one existing selector,
`process_readiness_owner_v1::legacy_text_route_hellgate::sourcegraph_text_route_authorities_share_one_indexed_fixture`.
The retained old-Q SDK route checks the file-content alias, scoped
`index:no` Boolean truth and typed missing-language refusal, the
repository/path/language intersection, scoped numeric and language
matchers, and typed rejection of two scalars. The fixed ranking witnesses
are equal-score `alpha`/`zeta` with representative `alpha`, and ordered
`[beta, eta]` for bounded mixed path/body search. A second namespace in
the same daemon publishes the private, unarchived fork with two rows.
Each independent metadata miss and the positive control are checked
before shutdown and after reopening the same state.

The post-freeze continuation and failure repairs share one final
candidate selection. This covers the new route assertions and its
earlier scoped regex in one test body:

| Cargo binary and exact Rust test | Assertion |
| --- | --- |
| `native_index_uds::read_sessions::query::repository_query_contract::legacy_repo_metadata_and_repo_surface_are_explicitly_unavailable_on_native_text` | Native SDK parse refusal and server filter refusals |
| `native_index_uds::read_sessions::query::text_query_contract::phrase_query_matches_only_adjacent_terms_through_native_uds` | Phrase case handling and selected Source identity |
| `process_readiness_owner_v1::e2e_exact_count_window::select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total` | `select:path` collapses the same 15-chunk path |
| `process_readiness_owner_v1::legacy_l1_query_contract_sdk::l1_real_daemon_sdk_domain_primitives_and_symbol_windows` | Sourcegraph predicate spelling and typed rejection |
| `process_readiness_owner_v1::legacy_text_route_hellgate::sourcegraph_text_route_authorities_share_one_indexed_fixture` | Scoped regex and the expanded public old-Q fixture |

Root Nextest `37fa24fc-97af-47de-b46c-2a3722e99b49` ran the five listed
selectors plus the separate old-Q `type:path` case. Five bodies passed,
including both Native bodies, old-Q L1 and both projection cases. The
route fixture failed on the anchored path matcher. The 25 earlier PASS
results remain tied to the frozen baseline.
[Root batch terminal](/private/tmp/root-text-type-path-focused.k5i9gb7e/output.log)

**Scoped Sourcegraph follow-up.** Nextest
`b8279b10-1829-4b4b-bf1d-2d103f70ef20` ran one route body and returned
**FAILED · exit 100**. The ungrouped `index:no A OR B` reached its
`BRIDGE_TRANSLATE_FAIL` assertion successfully. The following
`index:no ripens AND NOT file:contains(name:colors.md, lemon)` returned
without a typed error, which failed the old refusal assertion. The
terminal records no candidate paths for this response.
[Original terminal](/private/tmp/root-text-type-path-focused.ong6quih/output.log) ·
[Ungrouped OR error](/private/tmp/root-text-type-path-focused.6rg5l368/output.log)

In the live fixture, the grouped `index:no (A OR B)` expects exactly
`docs/colors.md`, and the ungrouped query checks the typed error. The
bridge compares query metadata across OR branches. The `name:` inside
`file:contains(...)` is a predicate argument, so negation preserves
the predicate leaf. The fixture puts `ripens` only in `docs/colors.md`.
That file matches the predicate inside `NOT`, leaving an empty set for
AND-NOT. The existing Native fixture carries its own OR/NOT expected
owners. Root's existing follow-up `2d060575-0169-44b4-9e47-76855e2f2e1e`
verified the route body **1/1 PASS · exit 0** in 19.845 seconds. It
matched the live route file `00b1a176…`, Cargo.lock `aa589363…` and
the fixed old-Q pin at readback. The one body exercised the grouped OR
hit, the ungrouped OR error and the empty AND-NOT candidate set.
[Old-Q assertions](../../../../crates/quanta-index-searchd-runtime/tests/legacy_text_route_hellgate.rs#L1013) ·
[Sourcegraph bridge](../../../../crates/quanta-index-lq-bridge/src/translator.rs#L162) ·
[Native OR/NOT assertions](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/preview.rs#L351) ·
[Existing Root follow-up terminal](/private/tmp/root-text-type-path-focused.cowhcu5i/output.log)

The existing bare manual path hit, the 123/404 numeric cases, the
repo-content alias and indexed scoped phrase/regex checks stay in their
original fixtures.
[Live test and writer](../../../../crates/quanta-index-searchd-runtime/tests/legacy_text_route_hellgate.rs)

##### 04.4 remaining assertions source handoff

The shared ledger lists 12 groups covering 22 original TEXT IDs in
`text_authority_shards.rs` and `tantivy_smoke.rs`. Each result applies to
its recorded inputs. The pinned old-Q **1/1 PASS** in `2d060575…` remains
in the earlier run record. The Source/Symbol **2/2** and SDK22 **22/22**
have separate local receipts. Final candidate evidence remains **NOT_RUN** for
the corresponding release roster entries.
[Code and receipts](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#legacy-sdk-custody) · [Shared source inventory](../../../../tools/ci/inventory/lexical-test-disposition.toml)

| Deleted test IDs | Current selector and assertion |
| --- | --- |
| `a_delta_refuses_a_corrupt_selected_base_page` | `native_product_process::segmented_layered::layered_text_delta_refuses_a_corrupt_selected_base_posting_page` selects the posting page consumed by the 16th layer append. While the page is damaged, the exact Text read must return `CorruptObject`. Submit and same-key resume must both return `Unknown`. The retry must acquire a new fence while the journal stays `Pending` and the original head and Source cut remain visible. After physical restoration, the same key must publish a new view with the retained and new owners. The SDK write assertions cover operation recovery. The writer's internal `CorruptObject` classification is source-traced. **NOT_RUN**. |
| `an_unsealed_source_batch_cannot_leave_index_authority_skew` | SearchPlane `ingest_dispatcher::tests::idempotency::staged_unsealed_publication_is_refused_before_source_reservation` proves the seal refusal, zero preflight/apply, and an absent Source event. `process_readiness_owner_v1::e2e_ingest_preflight::every_preflight_refusal_over_raw_ipc_changes_nothing` checks durable state and a valid publish following preflight probes. The exact staged `seal=false` then valid publication with visible Source/Text comparison remains with SearchPlane and Runtime. |
| `boundary_documents_answer_like_an_independent_rebuild` | Added regex and literal byte-substring probes to `native_product_process::text_shard_contract::native_boundary_delta_matches_independent_full_and_cold_reopen`. Fixed owners cover three base regex hits, the removed raw hit, both surviving regex hits and the new raw hit. The existing observation matrix compares DELTA, cold reopening and independently published FULL. Native runs the raw literal through `file.contains`. |
| `changed_native_root_is_refused_at_both_doors` | `legacy_existing::lexical_existing_refuses_committed_native_root_and_overlays_from_real_old_q` covers the DATA open path. `native_product_process::native_boot::native_boot_selection_verification_and_selected_serve_preserve_real_publication` damages the selected manifest object. DATA/BOOT still owns the selected **Text root** online read refusal. |
| `one_scope_delta_preserves_unaffected_paged_segments` | Added a one-owner replacement to `native_product_process::segmented_layered::layered_text_dense_layers_split_one_partition_and_survive_restart`. It checks replacement of the changed partition, exact unaffected partition and registered segment IDs, and the replacement and unaffected owners through cold reopen. |
| `selected_native_pool_page_faults_are_typed` | `native_product_process::segmented_layered::selected_native_text_posting_page_faults_are_typed` selects a `QTXPOST1` leaf from the published Text artifact. Exact-view SDK queries must return `CorruptObject` for byte modification, removal and a symlink to the original bytes. After restoration the query must return the original path, owner and Source. `legacy_existing::lexical_existing_refuses_changed_or_uncommitted_artifacts` retains the separate persisted DATA assertion. New Native body **NOT_RUN**. |
| `exact_path_constraint_is_applied_before_limit_and_on_index_no_scan_v1`, `exact_path_constraint_only_query_treats_dsl_metacharacters_as_literal_v1` | `native_index_uds::read_sessions::query::text_query_contract::exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally` covers Native Chunk top-k. Added typed exact-path Chunk and Symbol top-1 checks and a typed manual Chunk scan to `process_readiness_owner_v1::legacy_l1_query_contract_sdk::l1_real_daemon_sdk_domain_primitives_and_symbol_windows`. Its Native syntax check on old-Q requires the literal quote/metacharacter path to produce the exact Chunk and Symbol for a blank query and an unconstrained blank to return wire `InvalidRequest` (domain `InvalidContract`). The Sourcegraph bridge refuses an empty source string in parsing. |
| `tantivy_executes_phrase_adjacency_without_unordered_match`, `tantivy_phrase_sidecar_uses_text_authority_and_case_rules` | Added the exact `Lemon Yellow Banana` Source excerpt and staged Source identity to `native_index_uds::read_sessions::query::text_query_contract::phrase_query_matches_only_adjacent_terms_through_native_uds` using the existing adjacency and case fixture. |
| `independent_chunk_clear_refuses_and_file_tombstone_preserves_symbols_v1`, `symbol_content_authority_shapes_fail_closed_on_initial_and_replayed_batches` | The two `runtime_fast_suite::sdk_frontdoor::lifecycle_tests` bodies cover typed clear refusal with absent G2 state and view preservation, and Symbol content refusals on initial and replayed publication after restart. Both passed in `ee648484-d869-41ff-af38-f472c64b6462` on `f08990c96`. [Exact selectors and evidence](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md#legacy-sdk-custody) |
| `index_no_reuses_content_tokens_within_document_without_cross_document_leak`, `language_constraint_is_pushed_into_one_pre_limit_candidate_query_v1`, `tantivy_executes_index_no_full_scan_with_scoped_content_predicate`, `tantivy_honors_case_sensitive_keyword_queries`, `tantivy_index_no_language_refuses_without_stored_authority` | `process_readiness_owner_v1::e2e_lexical_full_fidelity::lexical_full_fidelity_matrix` passed in SDK22 after `assess_indexed_language_union` pinned exact membership and duplicate rank invariance. The indexed top-1 control, same-document conjunction and cross-document miss also ran in that body. The existing route fixture owns grouped and ungrouped scoped OR, AND-NOT and typed `index:no` language refusal. `e2e_unicode_text_semantics::every_syntax_case_spelling_and_route_answers_the_golden_set` owns the case matrix. [SDK22 evidence](NC-F-integration.md#sdk22-old-q-custody-20261121) |
| `tantivy_executes_simple_path_term_surface_without_boolean_path_leakage`, `tantivy_executes_supported_type_and_select_filters`, `tantivy_top_k_stabilizes_keyword_path_surface_without_losing_content_hits` | Added the false content AND to `native_index_uds::read_sessions::query::preview::native_select_path_combines_path_and_body_matches_after_cold_reopen`. The same fixture already asserts the path-only hit and content NOT survivor. Reuse `runtime_fast_suite::sdk_frontdoor::search_tests::sdk_query_frontdoor_routes_history_runtime_and_structural_truth` for both old-Q Symbol type/select dialects and `runtime_fast_suite::sdk_frontdoor::matrix_tests::sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth` for Symbol name. Added the exact four Chunk IDs for positive Lexical `type:file` to `process_readiness_owner_v1::legacy_l1_query_contract_sdk::l1_real_daemon_sdk_domain_primitives_and_symbol_windows`. `process_readiness_owner_v1::legacy_text_route_hellgate::sourcegraph_text_route_authorities_share_one_indexed_fixture` owns the mixed top-two order. |
| `tantivy_executes_whole_document_regex_and_rejects_false_positive`, `tantivy_regex_sidecar_verifies_authoritative_text` | Added the original version regex and false-positive bait to `native_index_uds::read_sessions::query::preview::native_text_preview_keeps_rank_and_source_coordinates_across_reopen`. The ordinary result and preview must agree, and the verified Source excerpt is exactly `const VERSION: &str = "v1.2.3-rc.4";`. |

**4K SourceAdvance path diagnostics · 2026-11-22.** Tree and Layered share
`PathWrites::write_batch`. `NativeTextPostings/50` records accepted logical
Work for the scratch INSERT submission, including Work accepted before a
failed batch returns. A successful batch emits `sqlite_batches=1` and
`sqlite_rows` equal to its edit count. Lane 49 continues to cover
`PathWrites::finish`, including the sorted cursor and map installation.
Compare the lanes within the same request using closed spans. Read the batch
and row counters from the raw spans. The existing ingest summary exposes
the Work axes. An incomplete capture carries only the observed prefix.
The producer keeps the existing SQL batches, value format, and budget.
[Writer](../../../../crates/quanta-index-lexical/src/native/text_path.rs#L370) ·
[Attribution plan](../../nov-21-bugbash-source-advance-4k/01-attribution.md#원인-판별)

The focused release Nextest selection covers scratch edit and error parity,
equal charged Work, and the successful and failed batch counters. The
01:30 KST check found the run alive in shared build admission with both test
bodies **NOT_RUN**. [Execution log](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/proof-repair.4pps9jzo/lexical.log)

The user explicitly requested final Main compilation, both original SDK
corpora, Large and two quiet Off rounds, preserving the 512MiB and fees.
The current request does not promote historical binary receipts to the new
multipage source. New disposable output root:
`/Users/songmin/Library/Caches/quanta-index/native-sdk-caddy/closeout-20261117-o2gex5bs`.

**Correction to the historical mutation-floor claim.** This paragraph describes
the pre-prefix-program checkpoint; later prefix-program results supersede its
complete-inventory-only cost bound. 1,009,664 is the exact
sum of indexable runs over all 1,421 files. It is not an unconditional bound
on a producer that can reuse already verified canonical data. The complete
manifest/hash set and 39 current normalizer/NFC/lock/toolchain inputs were
checked before and after the new standalone census. Full-byte comparison of
position-framed sensitive NFC token sequences gives 1,397 distinct complete
inventories and 996,382 unique-inventory runs, with 13,282 duplicate runs in
24 files. Raw-byte equivalence has the same class count; whitespace or NFC
variants add no additional complete-inventory reuse opportunities. The
current first64 inventory cache can reuse only `scripts/OWNERS` from
`.github/OWNERS` (17 tokens) in the original order. Its current token demand
therefore remains at least 1,009,647 before other Work.

Even ideal **complete-inventory-only reuse** plus the existing Source owner,
Source path and Text owner edit streams costs at least
996,382 + 3 * 1,421 = 1,000,645 mutations. The latter edits are actual separate
map operations; this bound excludes owner-manifest recoding, Literal,
Catalog registration/protection and other map/commit work. It is conditional
on the current row-producing strategy and is not an impossibility theorem
for new internal representations. No fee discount, budget reset, source
filter, multi-SourceAdvance publication or raised fixed limit was applied.
The historical Record accepted M869,734 before its **byte** refusal. Of that
prefix, token-fill lane44 accepted M790,743; the remaining M78,991 must not be
confused with a single removable duplicate fee or summed again through its
nested diagnostic stages.

A separate exact prefix census explores a genuine structural alternative.
Original-order longest-prefix matching and independently sorted adjacent LCP
agree: 891,038 unique canonical prefix-trie edges, and a theoretical maximum
118,626 shared-prefix runs across all prior owners. A frozen first64 cache
can share only22,620 runs. These are upper bounds, not realized Work savings.
Avoiding new token Row/key construction requires borrowing an already
validated prefix program and merging its sorted case/position groups with
new suffix data. Copying the old Rows into a new arena or merely removing M1
would not implement that reduction. Group/alias encoding, Original memory
custody, probe/copy cost and per-owner EOF/seals still require validation.
No production prefix cache has been adopted from this bound alone.

The bound census commands and their exact source/binary bindings are in
`token-reuse-census-binding.json`,
`canonical-inventory-census/result-binding.json` and
`prefix-inventory-census/result-binding.json` under the external output root.
All executed bodies exited0; none is an SDK acceptance or timing result.

Main release attempt1 executed the canonical release build and failed at
four RepoMap legacy-import compiler errors. Three minimal visibility/export/
unused-result repairs were applied. The attempt also observed eleven changed
source paths during the build, so no output binary was accepted. Attempt2
then reached ten Runtime module/import/authentication wiring errors in the
concurrent legacy-import/recovery work. Those exact seams are under review;
failed builds and source drift do not produce an accepted candidate pair.
Quiet admission currently refuses one-minute load41.71875 against8.0;
no new Off body has executed. Foreign jobs have not been stopped.


**Measured prefix-program candidate (isolated source, 2026-11-18).** A bounded
64-entry, recipe/attempt-local cache compares exact NFC bytes and borrows the
original canonical token rows for a proved prefix. The current TokenCursor
must agree through the complete UTF-8 delimiter; suffix rows retain absolute
run positions, including excluded long runs. Complete-body matches retain the
already ready program directly. Flat programs retain canonical row slabs, not
ancestor programs; both case surfaces and canonical occurrence writers stay
unchanged. Original grants fund the cache, keys, Arc backing and preallocated
locator/slab buffers before suffix mutations. Source/NFC/output allocations can
discard inactive cached data through the original WorkBudget. No Work axis is
reset and all new comparisons, scans and metadata writes charge actual bytes.

The disposable original-etcd probe checked the original manifest and all 1,421
file hashes before and after. Token preparation accepted M905,316 and
B179,918,492, saving 104,348 actual token mutations against 1,009,664. All
1,421 owner groups and block shapes matched independent full-body preparation;
985 single-block packed payloads matched byte-for-byte. The remaining owners
were group/shape checked, not complete multi-block wire checked. Its Original
memory peak was 71,873,462B and all grants returned to zero after cache drop.
This excludes occurrence installation, Catalog and all other SDK stages: the
remaining 94,684 mutations are not a claim that full SDK acceptance will pass.
The probe is outside Main and will not be promoted as a permanent test.

The subsequent isolated Core/Storage/Lexical lib rail passed 1,199/1,199
(nextest `aab2c3ae-f0ba-443d-900c-85b125856679`, 24.601s bodies, three
preexisting/diagnostic skips). It includes typed prefill-buffer admission,
exact/midword/multibyte boundary controls, two-generation reuse, LRU/memory
lifetime and WorkBudget handoff controls. Original-SDK and Large qualification
remain separate. The Main release-main-6 build produced a source-bound pair
from 2,979 identical before/after/final inputs; it predates the prefix candidate.
Its original Caddy Record passed FULL/search/reopen at 38,662.6ms. That binary
receipt must not be reused to qualify the later prefix implementation.


**Main promotion and current proof boundary.** The 15 reviewed source/test
paths were promoted to Main after the above probe and focused regressions.
Disposable corpus-probe source was excluded; unrelated existing Text hunks
were preserved. Additional canonical Small/Directory refusal and two-owner
ordered-posting regressions passed 7/7 (`9058ede2-15a8-4eae-b992-394e1d255015`).
Main release7 failed only because an old alias helper became test-only; its
availability was corrected to match its callers. Release8 then compiled
successfully but observed two concurrent Legacy source changes, so it did
not publish a qualified pair. The next source-bound build remains pending.

For clarity, the **pre-prefix** release6 original etcd Record failed the byte
axis, not mutation: accepted B536,867,162, attempted4,349 against536,870,912,
M869,734 at `storage/object.rs:2109`. Its trace is COMPLETE_DIAGNOSTIC with
health[] and open/omitted0. This result does not execute the promoted prefix
program or canonical mixed-owner cursor. Main hexagonal boundaries pass;
the Core module-tree check currently fails three unrelated owner changes
(protected_source_routes, SourceRecordWire and RepositoryKeyAdmission).
The new WorkBudget method does not alter that module inventory. No unrelated
baseline was regenerated by this Text change.


**Measured R9 regression and structural correction.** Both R9 original
Records failed the byte cap. Matching all589 Caddy preparation owners gives
lane34 B28,049,640→72,003,350 (+43,953,710); lane35 adds3,494,176B.
SourceApply and inventory MapApply Work were identical. The primary new
cost was prefix-program preparation, not an unavoidable fee or mutation
floor. Its reservation, materialization and merge independently scanned the
same prior locator list. The corrected typed grant uses the independently
proved `prefix_indexed*2`, preallocates selected/output buffers before suffix
mutation, selects the old prefix once and merges that exact selection. Its
scratch read/write bytes remain charged. A second local duplicate came from
reconstructing an uncached OccurrenceReader for each Small/MapBlock term;
one bounded owner-local checked reader now preserves the original retained
adapter boundary and avoids repeat packed parsing.

Corrected original-etcd token probe: M905,316 unchanged, B163,915,892
(**16,002,600 actual bytes fewer**) with all1,421 groups/shapes and985
single-block wires matching full-body reference. Original peak71,873,582B;
all grants refunded after drop. Corrected snapshot Core/Storage/Lexical
1,204/1,204 and Main Lexical20/20 passed. A subsequent private ownership
repair keeps selected/output Vec backings ahead of their leases on every
error path; snapshot16/16 passed. Its Work and outputs are unchanged.
Main full Core/Storage/Lexical1,207/1,207 also passed after binding an
intentionally unused atomic test result in SourcePaths. These scoped
results do not replace either original fixed-limit Record.


**R15 and term-map representation conflict (2026-11-18).** The original
release15 traces are complete diagnostics (health empty; no open/omitted
spans), not successful SDK acceptance. Reusing an already authenticated held
occurrence block allowed etcd to reach861 prepared owners instead of350,
and Caddy to finish all509 posting pages, but both still exhausted WorkBytes.
No limit, fee, corpus, deadline or atomic FULL boundary was changed.

The exact original-etcd census contains133,221 full case keys (68,690
sensitive +64,531 folded). The best possible prefix-only token trie has891,038
edges. Keeping one top-map edit per key therefore costs at least1,024,259
mutations before any other stage. This is a lower bound on that particular
representation, not on all possible implementations. The actual bounded
prefix probe costs905,316 token mutations, so the current per-key map is
already incompatible before considering remaining stages.

The active candidate replaces term→posting map rows with upper-full-key
fence→immutable-page rows. Each page contains at most64 exact full case keys
and16KiB; posting values and physical child custody are preserved. Cold,
warm COW, overlay and durable queries use one canonical representation.
The output policy changes and rejects the previous term-map artifact rather
than introducing a legacy-reader fallback. A second candidate persists only
fully confirmed cold dense roots in attempt-local scratch SQL, with an exact
canonical physical-root check on SQL hits; direct-slot collisions can reuse
confirmed installation without changing managed-install fees.

The winner tree already on Main reduces the fixed62-owner inventory flush
fixture from1,042,188B to1,000,686B (41,502B fewer,3.98%); its focused20/20
snapshot regressions passed. This fixture delta does not predict SDK totals.
Dictionary writer cold128→2 pages, warm replace/delete/split/no-op, wrong CAS
and late page-install failure regressions passed6/6 in the snapshot
(`18313b78-2b40-4c07-8741-6a5ff940ca06`). The first full new-layout rail found
11 failures among617 bodies; six physical-layout fixtures and query duplicate
lookup were repaired. Overlay membership now resolves one immutable term
posting across masked owners; its original100-read test envelope is retained
and11 query/overlay/policy bodies passed (`b816d756-d412-459c-ae02-d362efb2aa89`).
These are pre-promotion local checks; the final Main/full SDK gates remain open.


**R17 Main and original-input execution.** Main full Lexical passed631/631
with one preexisting skip (`b9d9b795-c1da-4ea2-93e8-b2d77463b79a`), and
Native UDS Text passed5/5 (`1311ad54-f355-4439-855b-54aca1e2376b`). The
concurrent FileContains fixture was migrated to canonical term pages without
changing source bytes or query golds. Hexagonal boundaries and diff-check pass.
The source-bound release17 pair is daemon SHA256
`30f9d845c0ff965ca0a425777472a53893d089f927b7d4473892f2cb7eaf310b`,
helper `354eaaaec8151fec4a42d6534e9381c2b40ab62e4afeae31b1acf619b7092358`.
Both original Record bodies failed as shown above, with complete diagnostics,
empty health and no open requests. Large and final quiet Off remain NOT_RUN.

Caddy posting lane39 drops50,564,317B relative toR15, but one `put_batch` per
term page increases Catalog reserve calls by283. The repaired writer stages
at most32 encoded pages with their Original grants, transfers DATA without
payload copies, validates every install receipt before anchors, and poisons
publication after any partial install. The actual two-page batch topology
failed2/2 on the old writer, then passed the relevant16/16 after repair
(`8369ca65-8677-4183-831b-4335c4005f5a`). Its SDK effect is not yet measured.

For etcd's completed prefix, SourceApply B13,001,611 is outside BuildPrepare.
BuildPrepare B521,125,397 contains disjoint member scopes Text367,025,402,
Literal142,994,133 and RepoMap10,936,871. Literal collect66,849,529 and
publish73,362,843 are different work; nested install/readback must not be
added again. Source DATA caching already exists, and retained reads preserve
wire/decode fees. No capability or independent proof is removed merely because
its cost is large. The current investigation targets the cold Text producer's
intermediate per-term staging operations and cursor representation.


**R18: bounded page installation and token-row corrections.** Main
normalizer+Lexical passed673/673 (one existing skip), nextest
`a5a46ac1-52fa-4a61-9922-b22d99d98b8a`. The source-bound R18 release pair
has2,990 unchanged before/after/final inputs, HEAD1b16e127. Daemon SHA256
`878cf4d339049e3512622899ba1d091636067d773b07c3f7bb9c0a9f7a5026da`,
helper `3e0c64bc2ba836c09230d33ac8536f4e4420dc457dc9ab3162bb65c1cc03df8e`.
The conditional normalizer cursor leaves the first suffix run unconsumed,
removing the full prefix rewind/rescan; rejected/error predicates retain the
same run and absolute position. A checked55-bit row locator occupies8B rather
than12B without an ancestor-program DAG or a new persisted representation.

The isolated original1421-file token diagnostic passed all file hashes,
groups/shapes and985 single-block wire comparisons: M905,316 unchanged,
preparation B163,915,892→146,707,365, peak Original63,673,370B and grants
refunded after cache drop (`3560fe61-c6cd-4546-af3d-5aaffab76fd4`). It remains
an isolated source-order token measurement, not a full SDK result.

Both fixed R18 SDK Records failed: Caddy R999,970+39>1,000,000,
B508,097,872/W557,776/M665,733 at Catalog child confirmation; etcd
B536,870,872+80>536,870,912, R287,562/W440,704/M630,489 in the cold run.
Both canonical traces are COMPLETE_DIAGNOSTIC with empty health, zero open/
omitted and2 matched requests. The controller initially used Python3.11
without jsonschema; the existing traces were successfully re-ingested using
the repository Python3.12 environment without rerunning either SDK body.
Large and quiet Off remain NOT_RUN on this failed candidate.

Only matched completed stages establish savings: Caddy inventory preparation
B105,732,509→98,814,061 (-6,918,448); etcd's matching865 preparations
B188,355,609→178,064,463 (-10,291,146). Caddy Catalog reserve calls fall
865→597 with the32-page batching, but the new final MapApply progresses
farther and still exhausts reads in confirm. Its denied child has36 registered
children (attempted39=36+3); the trace does not identify its ObjectRef/type.
Source transaction-local child proofs cannot be reused across transactions as
unqualified ObjectRefs. Any combined DATA/map installation must preserve
CAS/order/EOF, child-first registration, namespace, GC and rollback custody.

The snapshot-only global cold-merge experiment initially appeared to reduce
spill work, but that preliminary measurement omitted head movement and heap
swap charges. It is superseded by the fully billed rejection below and is not
an optimization result or a Main candidate.


**Prototype decision and exact descriptor handoff.** The journal/global
inversion experiment is rejected for Main. Complete head movement and heap
swap accounting overturned its preliminary benefit: fixed65-owner old64-spill
B1,032,692, one-walk global heap1,285,930 and compact-u32 winner1,313,038.
No fee was waived and no benefit oracle weakened. This prototype, its SQL
journal, observer and diagnostic dependencies are excluded from promotion.

The selected minimal repair carries the producer's exact physical descriptor
count to the existing64-owner cold path. An ordinary Directory's original
`counts[slot]` is term-row count U, whereas an aliased plan uses anchor count.
`ProducerState::descriptors` is the actual E and becomes valid only after full
producer EOF. A root/attempt/slot-bound receipt passes E to cursor admission,
which still checks actual descriptor EOF. Storage's former comparison
`max_entries > MAX_BATCH_ROOTS` mixed value count with root batch width; the
checked canonical-tree resource formula remains2E and the actual root-write
batch limit remains64. The original SDK caps and fees are unchanged.

The actual-source diagnostic prepared1421 owners and1,009,664 tokens. Of274
ordinary Directories, authenticated descriptor counts are2..121:271 are≤64
and3 exceed64. With exact E and independently admitted resources, every1421
owner has a supported credit bound. This establishes applicability/resource
admission only, not SDK512MiB Work acceptance. The raw FileObjectStore/
TestScratch diagnostic's SourceEOF Work differs from the Managed SDK rail
and cannot be promoted as its impossibility proof or acceptance result.

Main's real NativeLedger credit suite passed8/8
(`2f6e8ffd-24f6-48ac-aaa8-ba2485543445`). It includes a Temporary child
surviving scoped-credit close within an active execution, release while a later
credit has live DATA, stale-scope refusal, and zero final charges. No production
memory accounting API was changed. Final Main and fixed SDK qualification of
the selected minimal descriptor repair is still pending.

The minimal receipt repair subsequently passed Main Lexical+Storage876/876
(two existing skips), nextest2649a57b. A final UDS run51f7aa6c exposed an
existing fixture integration failure: shared scenarios now include repository
predicates, but the DSL fixture staged legacy sources without repository
identity. Its input now uses StageRepositorySources with an explicit RepoId,
matching the canonical harness. All nine fixed scenario golds and production
identity checks remain unchanged; the Text and repository predicate UDS
selectors then passed7/7 (89 unrelated skips,12.276s), nextest
af068446-0f2c-4e68-94d0-291a0c3c976b. No SDK acceptance follows from this
fixture correction.


**R19 and the next bounded producer correction.** R19 bound HEAD732bea600
and2,994 unchanged before/after/final inputs. Daemon SHA256
`590ad2f2cec1ab7522702b97823f17485996c3051777fe3a72b0823f00a7896e`,
helper `8638e5fe657043a240bc5547c63c81c44e32d547166873280e1dbbb46a66a860`.
Both original Records failed with canonical COMPLETE_DIAGNOSTIC, health[],
open/omitted0,2 matched requests and no source/input drift. Caddy has exactly
the R18 refusal/counters. Etcd refuses at packed_runs.rs logical edit admission,
B536,860,965+17,787, R287,293/W401,559/M630,489. Every comparable completed
stage is unchanged. The missing R18 diff/read spans show a changed later cold
route, but capped terminal totals cannot establish net savings. Final merge
runs only after inventories, so optimizing that later merge cannot avoid this
observed refusal. Large and quiet Off remain NOT_RUN on this failed pair.

The new local WinnerTree advances the selected cursor and replaces the old
head once, removing the real intermediate None slot write. Fixed62-owner
multipage tuple gold drops B1,000,686 to856,686: exactly1,800 selected rows
with one80B header transfer removed each. The7 focused regressions include
late owner EOF corruption, zero accepted packet rows, run poison and complete
Original refund. No logical posting edit fee changes.

The page writer now fills the existing16KiB payload envelope with at most255
wire children. Its row ceiling2338 is derived from the minimum seven-byte
row. A deterministic512-slot child index removes linear reference scans;
full-reference collision checks, hash/probe/slot costs and Original grants
remain. Parsed offsets use exact-count storage and reject impossible counts
before allocation. Fixed2-row wire is unchanged;128 short terms use one page,
and a1000-byte-key fixture crosses the actual16KiB boundary at16 rows.
The output policy independently hashes to7d1c00fb... for admission31 and rejects
the old fixed64-row e6a8773f... policy as well as the former per-term map.
The first broad run passed916/920; three helpers assumed page count equalled
ceil(rows/64) and one froze the prior policy digest. Their corrected byte/
child/semantic oracles and explicit old-policy rejection then passed920/920,
nextestdee4d6d5-bc60-4650-a1c2-48e6c8e4a344 (two existing skips).
These are local results; R20 original SDK acceptance is still pending.


**R20 rejected, R21/R22 original results.** R20 compiled successfully but
source closure detected a changed production Kernel update.rs. It emitted no
qualified pair and ran no SDK. The Source language/input-lifetime owner then
acknowledged completed production edits and a stable validation window.
R21 and R22 bind HEAD70543fac and2,994 identical inputs before/after build,
after copying binaries and after both original Records. R21 daemon
4fa4abf4299a98194ad2f2d9290489846e442738bc8492f80926d02076b8b63c;
R22 daemon2f1a5c42c5a8c27a953cfd4c1a40bfcdd6aea901a21fb6ddb38dac41eb5f64e9;
helperf1e043f9f18ad466026cc489ed77b3d44e2cb601804d03670b8d7317cbac1d81.

Both Caddy Records and validators passed on the original590 files, with FULL
and reopen, source drift0 and complete diagnostics. R21 submit→published
was28.951s; R22 was24.156s. They are Record results, not a valid quiet-host
performance comparison. For the identical completed R22 Text segment, Work
bytes decrease14,019,920 while R/W/M remain identical. R21's full-operation
usage was not emitted: root/seal/publish counters are absent. Do not invent
an operation total by adding overlapping scopes. SourceApply B5,637,475 and
BuildPrepare B466,235,197 are separately observed complete disjoint stages.

R21 etcd still refused at the cold winner comparison, B536,870,905+15,
M630,489. R22 removes the Head array completely: cursor keys borrow already
verified DATA, and the only owned key is copied into the output edit. The
fixed62-owner scope drops856,686 to707,886B, exactly60 initial80B headers and
1,800 replacement80B headers removed. A persisted-packet late EOF regression
passed with run poison and Original refund. The subsequent MapBlock metadata
accounting correction bills its actual8B Option layout, so the narrow and
whole suite need rebinding to that final source.
R22 etcd advanced to926 completed owner preparations plus the927th error,
then refused a real DATA read at B536,870,207+15,758, R288,512/W514,650/M670,254.
The different capped prefix cannot be used as a whole-corpus savings figure.
All traces are COMPLETE_DIAGNOSTIC, health[], open/omitted0 and input/source
drift0. No FULL ACK exists for etcd.

R21 UDS7/7 passed0095d5cd-975d-4ad1-b0a1-5a4a1d00dbc8. Its exact bound
daemon also passed the4,096-file/two-restart Large body in101.149s,
7cf5a77b-9455-4585-ba0b-67db2a3cbed0. This proves functional source/owner/rank
persistence for that binary, not SDK512MiB or quiet performance. Runtime
credit8 plus actual ManagedStore fixture passed9/9,6575b125-4aad-4ea5-b740-27683224e9b7.
The separate/joined fixture reads764→716, SELECT398→387 and COMMIT4→2
are Ingress-scope measurements of the existing scalar port. No Core/Kernel
joined stream API was added, and that fixture is not the cause of Caddy's
production result. Final same-candidate Large and all four strict quiet Off
bodies remain open.


**R23 pre-build refinement (2026-11-18).** The 256-row minimum-position
index is implemented with prefill Original grants and billed metadata creation,
reads and selected-locator writes. The exact original 1,421-file component A/B
keeps 905,316 token mutations, 1,009,664 indexed runs and fixed output checks;
prepare Work decreases146,707,365→145,002,529B (1,704,836B). Including
independent group/shape/alias/wire verification gives167,457,775→165,752,939B;
peak Original increases33,716B and final live memory is0. These component
figures do not qualify the fixed-profile SDK.

Literal membership generation now seeds a 24-bit rolling gram with two bytes
and reads one new byte per window, preserving exact raw-byte grams and the
original mutation rate. The old three-byte windows reread two overlapping
bytes. All1,421 original bodies have length≥3; their fixed total8,666,217B
implies removed reads17,323,908B (25,990,125→8,666,217B). A fixed independent
short/raw/UTF-8/high-byte membership and Work oracle failed before the change
(2aff3f19), then the focused release rail passed3/3 (baf569ea). Persisted block
readback/sealing is unchanged. Combined release libraries pass925/925
(51efd446-2984-48b7-a098-13062ff210e8). These changes are not claimed to solve
the original etcd512MiB refusal before a fresh source-bound SDK body.

The R22 Text residual is exactlyR5,608/W401,684/B158,779,870/M1,024 after
subtracting the disjoint inventory-prepare and inventory-map direct children.
Its completed861-owner prefix has400,544 canonical case terms and
4,713,524 unique key bytes. The protected posting append fee is therefore
400,544×130+4,713,524=56,784,244B and400,544 logical writes. This is not
duplicate work to refund. The remaining101,995,626B mixes independent DATA
readback, tournament, packet copies/SQL, source cursors and completion work;
the existing trace does not assign it to one cause. Occurrence identify then
prepared install does hash the same canonical wire again, but selected wire
bytes are not isolated: the entire70,423,689B occurrence-output span is only
a loose upper bound. A broad Core prepare-once API or Source body cache is
not justified by that bound; the latter already has a shared4MiB
AttemptObjectReads DATA window. Original1,009,664 runs are not a mutation
lower bound after exact canonical program reuse. Final SDK mutation
acceptance remains unverified.


**R23 single checked-DATA stream.** The canonical cold packed cursor now
derives its first count from the block it just independently fetched and fully
parsed, instead of issuing an immediate duplicate logical `read_retained`. A
stored provenance field limits this to PreparedCanonical; Unsealed/Ready
retains the old count admission. The inline alias sibling is reached only
after current-attempt/layout/descriptor checks and uses its same independent
get/parse for count/key/EOF sealing. The private borrowed MapBlock visitor also
keeps its checked DATA across64-row boundaries; public `next_page` and generic
PackedInventoryCursor are unchanged. No retained-read fee constant changes.

Independent fixed oracles require one packed canonical get and0 retained calls
versus Unsealed's one retained read (R1/B168 difference), and a130-row borrowed
map stream crosses two64-row boundaries with no retained calls (R2/B4,888
removed). Cancellation and callback refusal leave the same unread row available
for retry. Initial persisted get refusal/corruption still poisons the failed
batch without owner/root publication; the existing retained-read fee/refusal
oracle is unchanged. The sibling alias behavior test failed on the old
ResourceLimited path (f1efe900). Combined final release Normalizer+Lexical+Storage
passes928/928,2 skipped, a889eb6b-29fd-41b6-bdcd-0c731e62b4c3, exit0. Hexagonal
boundaries pass. Module discipline fails on the unchanged Semantic
`src/legacy/mod.rs:28` facade implementation.

The existing diagnostic owner-window lane32 now covers cursor, winner and its
nested64-row posting flush lane33. Registry admission/run start precede this
span and final run sealing follows it, so lane32 is not the entire cold-build
cost. Off executes the same original closures without diagnostic contexts.
R23 release compilation passed with2,994 identical before/after/final source
inputs at HEADc7647c67. Its post-Caddy capture found five added, unmounted
`quanta-index-searchd-harness/src/legacy_reference*` Rust files. None of the
2,994 bound inputs changed; no module/build/Cargo reference or daemon/helper
dep-info names those five files. The original strict whole-tree controller
stopped before etcd. Continuation must record this exact unused-source
addition scope, retain every original bound hash and binary check, and refuse
any changed existing input or newly mounted source. It must reuse the actual
Caddy body and cannot call the enlarged2,999-file tree identical to the build
snapshot. The observed Caddy result reports FULL/reopen success at22,685.6ms;
canonical trace validation and original etcd continuation remain pending.


**R23 original Record terminal and attribution (2026-11-18).** Daemon
fa1eb765ffecf8c94d5bfc8b43758fc16ad7888a0575fcedcc2c3eef869205e5 and
helperf1e043f9f18ad466026cc489ed77b3d44e2cb601804d03670b8d7317cbac1d81
executed the exact original corpus with unchanged512MiB/R-W-M1m/SDK60. Caddy
Record plus canonical validator is VERIFIED:590files, FULL, reopen,
22,685.6ms. Complete diagnostics have health[], open/omitted0. Its
BuildPrepare B452,215,277→441,356,619 (−10,858,658), Text lexical
B404,397,262→400,079,278 (−4,317,984), same W/M. Literal collect removes
exactly6,540,674B on original3,272,107 body bytes and590files. Caddy's final
postings stage is unchanged atB222,985,471; new child-span coverage is not
additional Work and cannot be compared to old coverage as cost growth.

Original etcd is FAILED/ACK0:1,065 owners prepared, then
B536,870,898+16/536,870,912 at the real key digest in
text/entry_map/occurrence.rs:102, R299,990/W529,024/M766,150. The candidate
advances139 completed owners beyond R22 but has no FULL result. Literal's
collect stage removes exactly17,323,908B, with unchanged4,272 mutations.
Its Text parentB384,349,281 contains disjoint preparation214,754,019,
inventory-map20,446,805 and14 cold windows145,207,579; only3,940,878 remains
outside those children. Nested posting-editB81,431,473 is included inside
windows. Window-only cost is63,776,106B; window1 alone26,082,703B (41%).
That window emits103,030 rows. The four large ASCII diagram owners in this
cohort contribute75,424 canonical terms/2,283,935 unique key bytes. Short
owner inputs contribute about1% of rows, so another small-owner shortcut
does not target the dominant observed work.

Source authority is scoped precisely:2,991 original bound files stay equal;
three Runtime test helper files changed and five unmounted harness files
were added. Both actual release dependency manifests exclude these eight
paths and both executable hashes remain pinned. Exact exception hashes and
all other source hashes were checked before/after the etcd body. The initial
whole-workspace controllers correctly refused drift; continuation records
production dependency stability and does not claim an identical full tree.
Subsequent mounting of the shared Legacy helper changes the next candidate
closure and requires a fresh final build.

R23 Native UDS and Large reruns failed compilation withE0432/body0 while the
other owner moved the Legacy process helper but had not exported it from the
harness. Their requested bodies are NOT_RUN, not behavior failures. The
canonical owner is completing that export; no duplicate helper was added by
this chat. Latest whole Lexical strict Clippy is FAILED531 and is not a
clean gate; Hexagonal passes. Quiet preflight is BLOCKED by foreign Rust
processes and host load, no override or foreign cancellation. All final
quiet Off bodies remain NOT_RUN.


**R24 measured comparison candidates, acceptance pending.** Repeated long-key
comparison in the dominant window is addressed by a bounded64-slot machine-word
prefix cache. This follows the exact-order abbreviated-key pattern in
[PostgreSQL SortSupport](https://github.com/postgres/postgres/blob/master/src/include/utils/sortsupport.h):
a nonzero prefix comparison must preserve full byte order, while an equal
prefix requires the authoritative remaining-key comparison. The Native
candidate uses big-endian u64 only for keys longer than8 bytes; shorter/mixed
keys use the original full comparator. All512B initialization/Original backing,
16B prefix read/write per long head and8B cached comparison are billed. Fixed
64 distinct32B heads change56,748→53,676B including those new costs. Multirow
head refresh, equal-prefix suffix, short/long and owner-tie goldens are required
before adoption; a local fixture does not establish original-corpus ROI.

The token-program merge candidate finds an exact consecutive suffix range
below the fixed prefix head through exponential then binary search. Existing
locator/row/slab ownership, position order and Original prefill grants remain.
Additional probes bill their row metadata and complete key comparisons. This
uses the range-search idea described by
[CPython's merge design](https://github.com/python/cpython/blob/main/Objects/listsort.txt),
whose analysis also notes that such searching can require more comparisons on
interleaved input. Native has no adaptive heuristic threshold deciding
correctness. Its fixed long-suffix case changes57,640→42,120B while alternating
keys change4,366→5,374B. Both preserve complete canonical groups/positions and
refund all Original memory. The actual1,421-file component A/B against
145,002,529B prepare remains the adoption gate. These changes do not remove
token mutations and do not qualify the original fixed-profile etcd SDK.


**Shared helper integration retry.** After the benchmark owner exported its
canonical Legacy process helper, the same no-default Native UDS selector
executed7/7 PASS (78 skipped), de17efcb-d73c-4de3-8b54-dcb386f8385d, exit0.
The20m16 compile wait and25.035s test bodies are separate; neither is a quiet
SDK timing result. R24 combined libraries initially executed931 bodies with
929PASS/two Work-golden failures: the u64 prefix array adds exactly512B to the
short-head fixture and gallop adds48B from three extra two-Row probes to the
three-generation fixture. New long-key replacement, shared-prefix suffix,
short/long and owner-tie behavior passed. Goldens now describe those actual
operations; no budget or tariff was relaxed. A subsequent retry compiled
before bodies with a concurrent History-export caller/argument mismatch;
the owning CLI source then added the required argument, and the same
combined retry remains necessary. These interrupted/failed rails are not
current full-suite qualification.


**NoFeatures language-normalization construction seam.** A later combined
release retry stopped withE0433/body0: Native `repo.has.file(lang:...)` called
the canonical engine-independent `normalize_language`, but the entire
metadata_normalize module was mounted only underlegacy-tantivy. The module
is now shared; only its already existing language helper is unconditional.
The other seven Legacy helpers and their two Legacy-only imports retain
feature gates. No second normalizer, Legacy engine enablement, tariff or
source-language inference was introduced. The existing same-file authenticated
language/Boolean/refund/unknown-metadata tests cover the Native caller in the
next full no-default run. Compiler failure is not behavior RED.

The raw-token chunk alternative was measured without Main edits or Cargo.
All original1,421 manifest files and hashes matched. In1,392 ASCII files
(927,696 indexable raw runs), after excluding an even stronger117,360-run
previous-owner common-prefix reuse, a fixed64-token boundary exposed only
2,112 additional exact reused runs; fixed256-token boundaries exposed256.
The29 Unicode-containing files were not evaluated canonically by this
ASCII-only diagnostic. This does not prove an impossibility for adaptive
chunks or Unicode normalization equivalence; it supplies no basis for
prioritizing a new chunk/Merkle IR over the measured current candidate.
It is not a fixed-profile SDK mutation result.


**Original1,421-file merge component gate.** The exact existing ignored
snapshot probe executed1/1 PASS,57caf472-2121-41d4-b146-cac075bf140d. Only
Main program275145bd and test-helpercff9d658 were copied into the existing
snapshot; rejected global-journal code was not promoted. Before/after manifest
and every file hash match. PreparationB145,002,529→138,079,428
(−6,923,101B/4.77%), M905,316 unchanged, optional misses0.
All1,009,664 indexed runs,1,244 reused owners,985 wire comparisons and
nonalias shapes/1,819 blocks/max121 match; leased Original peak63,707,086B
and final live0. The measured verification-inclusive total is158,829,838B.
The candidate stays on Main on this component evidence; this does not
qualify original SDK512MiB, final M, quiet timing or Large.


**Current mutation representation audit.** Current `root.terms` stores packed
dictionary-page upper fences, not one root `MapEdit` per canonical term.
`term_dictionary_write::prepare` and PageDraft installation charge their actual
B/R/W; Storage ordinal map charges M1 per emitted anchor/owner edit. The
133,221 global case terms therefore are not an additional133,221 mutations.
Starting from the actual full token-component M905,316 leaves94,684.
The wire row limit alone implies at least57 dictionary anchors; conditionally
adding1,421 distinct owner edits gives a lower bound906,794 before the other
Source/Literal/metadata/inventory work. Byte/child splits and all remaining
physical edits must still be measured in one complete SDK attempt. This
representation does not prove an M1m impossibility, nor does this lower bound
qualify the full SDK. No mutation tariff or funding was changed.

The leak-visible current library retry executed935/935 PASS,2 skipped,
leaky0 (`9859667c-8ab6-4463-9b29-86e9d3db7f32`). One concurrently added test
makes it a different cohort from the earlier934-pass run; the earlier leak
name remains unknown. R26 compiled both production executables successfully
in2m49s but changed Catalog producer source during the build, so its source
binding correctly failed. Concurrent test changes are considered separately
only through both actual binary dependency closures; no production-source
change or old blanket exception is accepted.


**R30 cold Literal factored source seal.** The previous logical-key seal already
front-coded adjacent full keys. It was not hashing35B on every membership.
The cold V4 producer now also factors its independently declared owner prefix
once per block, then front-codes the virtual `(gram3, owner_suffix)` key. The
source builder validates every key before sealing; persisted readback first
checks exact expected V4 header width/bytes, then decodes every key and checks
EOF. The middle-owner P/Q/P regression rejects a forged body despite matching
endpoint/header hints. Warm transcript and published wire/API remain unchanged.
The new width/prefix hash input is charged; no coefficient or budget changed.
Focused release12/12 and the937-library cohort passed, with independent review.

In original1,421-file etcd, the completed Literal stage changes125,670,225
to107,609,301B, saving18,060,924B with R20,110/W5,675/M9,600 unchanged.
This is a completed-stage comparison, not an extrapolation from equal caps.
Caddy completed Literal changes35,477,439 to32,357,665B (−3,119,774),
also with identical R/W/M. Both R30 SDK bodies later reached their original
60s deadline, and the trace shows cancellation without a denied Work charge
or I/O failure. Complete trace/health[]/open0 do not turn that into acceptance.
The old R28 Caddy control still completed in49.464s. Identical early-stage Work
with varying wall time and the contended host are evidence consistent with
contention, not proof that every new-source timing difference is external.

The current strict host preflight refuses load47.659668 above8 and15 foreign
Rust processes. No override, foreign cancellation or SDK timeout increase
was used. Root-budget cancellation is tied to peer hangup; keeping the daemon
after client60 and reading status would not continue the same cancelled work.
OwnedWork would be a different execution path and is not substituted here.

The matched14-window u64 prefix experiment was rejected after original-corpus
measurement: unchanged direct posting children/rows but+624,431B exclusive
inventory walk. Only that optimization was reverted; baseline WinnerTree,
borrowed cursor, gallop and new ordering regressions remain. Revert focused
release3/3 passed. The subsequent cache Entry lookup-reuse candidate passed
focused release19/19 (`6709362a`) and independent review of its final guard.
The lookup plan exclusively borrows the same cache, and preparation owns the
same source cursor until insertion or rewind. It reuses the exact predecessor/
successor LCP and insertion position; an LRU eviction adjusts that position
without another search. Empty-owner/key-copy and optional Original fallback
remain covered. Fresh original1,421-file A/B passed1/1 each: A996b438e
B138,079,428/M905,316; B56bd55ef B131,949,754/M905,316. The measured
6,129,674B saving preserves all1,009,664 tokens,1,244 reused owners,985
alias wire comparisons and nonalias shapes/1,819 blocks/max121. Optional
misses are0, Original peak63,707,086B is unchanged, and final live0 is
asserted after dropping the cache. The printed pre-drop live5,636,951B is
not a leak result. Both runs bind every original manifest/file digest. The
component diagnostic uses its existing10GB Work/1800s budget; it does not
qualify original SDK512MiB/60s or final mutation total.


**R31/R32/R33 qualification boundary.** R31 source copy was rejected before
Cargo because Catalog read_only.rs changed. R32 compiler101 exposed a private
entries field caller; Main already used the canonical entries() accessor and
the next cohort reused that fix. R33 compiler0 is a compilation fact only:
Full3libs then failed before test bodies while selecting an H-cohort normalizer
dependency. Source3023/pair/validator/config hashes alone did not prove the
transitive compiled dependency's origin. These failed and rejected receipts
are preserved.

**R34 build-custody RCA and repair.** The one-off runners forced the Main
target plus PRESERVE=1 from a different frozen checkout, while copy2 preserved
older source mtimes. The selected H normalizer metadata lacked the method that
is present in Main and the frozen source. These observations match Cargo's
documented shared-path-dependency/freshness failure mode; the exact mtime
freshness decision is an inference without a Cargo fingerprint trace.
[Cargo issue12516](https://github.com/rust-lang/cargo/issues/12516).

R34 removes that opt-out and uses the already existing checkout-local target
6320c5494b2a81b9/test-workspace-lane through the canonical wrapper. Snapshot
content copies get fresh mtimes, while Main files stay untouched. Source
digests, Cargo JSON package/source/output origins, freshly compiled normalizer
rlib and its source metadata are checked. Registry reuse remains locked by
Cargo.lock. No whole-cache clean, new lane/worktree, source shim, budget,
tariff or timeout change is used. Nextest uses the separately frozen current
Main config784802 with an explicit config-file argument. The R34 full-build
controller used a50GiB AND2.75% operational precheck. This is not a repository
or SDK requirement: AGENTS/Core/Playbook, cargow and resource_admission do not
define it. Reapplying that cold-build estimate to incremental library/Large
verification was an execution-controller error; those additional calls are
removed without changing the bound build controller or inventing a lower
threshold. Actual Cargo/OS failures still fail verification. Root's canonical
GC preview reclaimed0 and performed no deletion. R34 compiler/test/SDK verdicts
require actual terminals. Original SDK512MiB/60s and quiet Off remain open.
