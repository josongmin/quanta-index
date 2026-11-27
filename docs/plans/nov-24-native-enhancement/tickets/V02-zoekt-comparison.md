# V02 · Zoekt 동일 조건 비교 준비와 실행

## 현황

Zoekt `b55ce727df8c`는 Git FULL·DELTA, 무변경 생략, shard 재적재·정리와 공개 검색을 제공한다. Native와 게시·결과 단위가 다르다. [09.3 비교 정책](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)의 09.3a 출하 게이트 결정은 열려 있으며 기본안은 비게이트다. 09.3b에는 Caddy·etcd·Large 코퍼스와 질의 대응 초안이 있다. etcd의 로컬 Zoekt↔원문 파일 집합을 확인했다. 고정 Linux 후보 입력과 Zoekt `AB`/`BA` 5쌍은 `NOT_RUN`이다. 기존 Native Record·Large나 구 후보 영수증은 Zoekt 짝비 증거가 아니다. 구 서명 릴리스 대비 [07 고정 비교](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#원-e07과-비교)는 별개다.

**로컬 etcd 정확성 기준.** upstream `be3acf4ed017efb7008beb08585d890e5364ce93`의 1,489 Git blob 중 보존된 입력 1,421파일의 경로와 blob digest를 전부 대조했다. 이 파일 집합을 Go 1.27.3, clean Zoekt `b55ce727df8cfbed76b8a1420445fd0cf93f3256`의 `zoekt-index -disable_ctags -parallelism 2`로 색인했다. `zoekt -v -l`과 `rg -l --hidden --no-ignore`의 파일 집합은 대소문자 구분 `RegisterClusterHandlerServer` 1건, 비구분 1건, `Register[A-Za-z]+HandlerServer` 3건, 고정 무결과 0건에서 **4/4 `VERIFIED` · exit 0**이었다. 질의별 `FilesSkipped`·취소·`ShardsSkipped`는 0이다. 로컬 기준은 Git FULL·DELTA나 지연 측정에 사용하지 않는다. 원 Caddy 590파일 트리와 원 Caddy·etcd manifest 경로는 현재 부재하다. [etcd 기준 실행](/Users/songmin/Library/Caches/quanta-index/v02-zoekt-oracle.ZTqaPG/etcd-oracle.log) · [고정 Zoekt 인덱서](https://github.com/sourcegraph/zoekt/blob/b55ce727df8cfbed76b8a1420445fd0cf93f3256/cmd/zoekt-index/main.go) · [원본 입력 기록](../../nov-12-native-cutover/tickets/NC-F-integration.md#원본-sdk60-입력-복구와-실행-검증)

**Native 대조 시도.** 같은 1,421파일·8,666,217바이트를 파일별 SHA-256이 들어간 manifest로 로컬 NoFeatures `native_corpus`에 넘겼다. `SegmentedLayeredV2`의 프로필 게시와 Source 스테이징까지 통과했다. FULL 게시 상태 대기에서 기존 SDK 60초 기한을 넘겨 `REQUEST_DEADLINE_EXCEEDED`와 **exit 1 `FAILED`**로 종료했다. FULL ACK·검색·재오픈까지 도달하지 않아 짝 정확성은 미완료다. 이후 호스트 사전 검사는 1분 부하 196.12(상한 8.0)와 외부 Rust 프로세스 9개로 차단됐다. 서버 병목은 미확정이다. Native 쪽 최적화 빌드의 FULL·조회·재오픈 재현과 RCA는 기존 TEXT 07.3 SDK60 실행으로 인수한다. V02는 그 후보와 입력을 받아 공통 질의의 정확성 및 Zoekt 짝 비용을 판정한다. [TEXT 07.3](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#sdk60와-large) · [Native 실행](/private/tmp/v02-native-etcd.A6aqMu/native.log) · [고정 입력 검증](../../../../crates/quanta-index-searchd-harness/src/scale/native_index/corpus.rs#L520)

## 작업 범위

- [09.3a–b](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)에 실행 전 게이트 여부·비율 기준, 고정 Zoekt 빌드/옵션·Native 후보, Linux·SSD·캐시, commit·파일 manifest와 제외 규칙을 기록한다. Git branch 입력과 미저장 worktree 입력은 별개 프로필로 표시한다.
- FULL, 한 파일 반복 편집·삭제·rename·no-op, shard 재적재·정리, 검색을 같은 원본에 매핑한다. Zoekt의 delta fallback·tombstone·개별 shard 게시와 Native의 Source cut·Catalog 뷰 차이를 결과에 표시한다. [Native 보강 범위](../readme.md#적대적-감사와-zoekt-비교)
- 질의 대응표에 대소문자·regex 방언·repo/branch/revision·file/lang 필터, 파일 대 line/chunk 결과 단위, top-k·전체 순회, `SearchOptions`의 shard/전체/표시 상한과 잘림 신호를 고정한다. 각 공통 질의의 파일 집합·원문 위치를 독립 정답과 먼저 대조한다.
- 언어 필터 비교에는 Native SDK `.language_any_of([language])` 또는 `searchctl code-search --language CODE`와 Zoekt `lang:LANG`을 사용한다. 비교에 사용할 언어별 파일 집합을 독립 manifest로 대조한다. Native 질의 문자열에 `lang:`을 넣으면 공개 CodeSearch 파서가 거절한다. [SDK 제약](../../../../crates/quanta-index-sdk/src/lexical.rs#L1014) · [CLI 인자](../../../../crates/quanta-index-searchctl/src/native/code_search.rs#L103) · [문자열 문법](../../../../crates/quanta-index-contract/src/query/code_search.rs#L555)
- 준비는 Q01–Q03 구현과 병행할 수 있다. 측정은 정확성·범위 합격 후 기존 [05 Bench 오너](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md)와 [07 출하 비교 레인](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md)에서만 수행한다.
- 비교 검증기는 `abstained` 행에서 비어 있지 않은 후보 목록·건수, 양성 히트, 무정답에 대한 비어 있지 않은 결과 신호를 거절한다. 모순 4건이 수정 전 집계에 들어갔고, 수정 후 파일 비교·랭킹 리플레이·다제품 집계 Python 회귀 **183/183 `VERIFIED`**. [관측 검증](../../../../tools/benchmark/retrieval/lexical_file_comparison.py#L1256) · [회귀](../../../../tools/ci/tests/test_lexical_file_comparison.py#L471)
- 외부 capture 재생과 Native 채점이 실제로 읽은 suite·query-pack의 SHA-256을 동일 입력에 결속한다. 실행 중 파일을 바꾼 뒤 원복해 전후 경로 검사에서 사라진 불일치 3건이 수정 전 통과했다. 수정 후 파일 비교·랭킹 리플레이·다제품 집계 Python 회귀 **188/188 `VERIFIED`**. [입력 결속](../../../../tools/benchmark/retrieval/lexical_file_comparison.py#L1580) · [원복 회귀](../../../../tools/ci/tests/test_lexical_file_comparison.py#L180)

## 완료 조건

- [ ] [09.3c](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)의 공통 기능은 양쪽 결과가 독립 정답과 일치하고, 거절·timeout·잘림을 원 분모에 남긴다. 불일치는 해당 Q/Source 결함 오너에게 넘긴다.
- [ ] [09.3d](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)의 `AB`/`BA` 5쌍에서 FULL, 게시→검색 가능 시각, 동시 읽기 p95/p99, 쓰기 바이트·peak 디스크·RSS를 원값과 신원에 결속한다. 비율 합격선은 첫 실행 전에 09.3에 확정한다.
- [ ] Zoekt 비교가 09.3a에서 비게이트이면 결과를 참고 지표로 기록한다. 게이트로 승인된 경우에만 그 사전 기준을 V03에 전달한다. Native 고유 ACK 재생·고정 뷰는 [08](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 절대 수용으로 판정한다.

## 의존·인계

Q01–Q03의 공개 검색 정확성·범위, S01–S02/D01–D02의 생산·복구 결과가 측정 대상 후보를 정한다. [04 유지 오라클](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)과 [06 퇴역 후보](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)를 재사용한다. V02는 별도 출하 게이트나 경쟁 실행기를 등록하지 않는다.
