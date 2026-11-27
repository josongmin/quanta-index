# I01 · 원문 매치 전수 순회

우선순위 **P1**. 전체 검색·치환이나 자동 편집이 이 API의 결과를 전제로 나갈 때 그 기능의 P0다.

## 현황

현재 main 작업 트리에 [매치 페이지·커서 계약](../../../../crates/quanta-index-contract/src/native_protocol/text_matches.rs), [lexical 스캔](../../../../crates/quanta-index-lexical/src/native/text_match_scan.rs), Runtime·[SDK](../../../../crates/quanta-index-sdk/src/client/native_text_matches.rs)·[CLI](../../../../crates/quanta-index-searchctl/src/native/matches.rs) 경로가 있다. 커서는 조회 뷰와 정확한 Source 신원을 보유한다. [10 진행 기록](../../nov-20-native-cutover-closeout/tickets/10-native-ide-agent-gaps.md#진행-기록)의 계약·lexical 집중 회귀는 `VERIFIED`다. 현행 로컬 NoFeatures Nextest `67075a3d-7644-4d9e-9197-ff89c6776b1c`에서 공개 전수 조회 **3/3 `VERIFIED` · exit 0**이다. 두 case mode의 `rg` byte 범위와 페이지, 게시 뒤 고정 뷰, 교체된 Source의 커서 거절, 예산·취소의 전체 페이지 거절을 확인했다. 승인 Linux 설치본 수용은 `NOT_RUN`이다. [실행 로그](/Users/songmin/Library/Caches/quanta-index/native-i01-scrub-current.qo35qi/nextest.log)

줄바꿈이 섞인 대안·선택 반복 regex에서 줄 안의 실제 매치를 허용한다. 필수 줄바꿈 패턴은 본문을 읽기 전에 계속 거절한다. Regex HIR 분류 독립 실행 **16/16**, lexical Nextest `78ad82f7-4105-4e16-ae80-33feba21adf3` **10/10 `VERIFIED`**다. 파일·byte 범위, UTF‑8·CRLF, 페이지, 후보 상한과 취소를 확인했다. [HIR 분류](../../../../crates/quanta-index-lexical/src/native/text_match_scan.rs#L130) · [원문 범위 회귀](../../../../crates/quanta-index-lexical/src/native/text/tests/match_scan.rs#L583)

**출하 정적 검사.** `TextMatchContinuation`의 Serialize·Deserialize와 `NativeTextMatchesResponse`의 Serialize를 명시적으로 구현했다. 기존 커서 CBOR 키와 응답 envelope의 필드 순서, 확장 맵에 대한 정규 토큰 거절과 기존 오류를 대조한다. SDK lib Nextest `8c2b5dcf-d26d-4c66-8ab3-00c0393cbb3f`는 **2/2 `VERIFIED` · exit 0**이었다. 이 변경과 Semantic 통계 codec을 포함한 11월 25일 `just rust-derive-allowlist`도 **`VERIFIED` · exit 0**이었다. [I01 SDK 선언](../../../../crates/quanta-index-sdk/src/client/native_text_matches.rs#L30) · [RepoMap 통합 게이트](../../nov-23-repomap-retirement/04-integration-acceptance.md)

선행 NoFeatures 빌드는 Native ANN의 오류 변환 함수가 legacy 모듈 조건에 걸려 E0433으로 실패했다. 공유 typed 원인 변환은 모든 빌드에서 제공하고 legacy 래퍼에만 조건을 두었다. 이어 드러난 Source scrub의 `unused-results` 세 건은 격리 결과를 명시적으로 바인딩하고 오류는 이전처럼 전파하도록 수정했다. 다음 빌드와 제품 본문이 통과했다. [오류 모듈](../../../../crates/quanta-index-semantic/src/lib.rs#L36) · [원인 변환](../../../../crates/quanta-index-semantic/src/errors.rs#L29) · [실행 로그](/Users/songmin/Library/Caches/quanta-index/native-i01-scrub-current.qo35qi/nextest.log)

## 작업 범위

- **wire 등록.** 현행 main은 `TextMatches`·`TextMatchPage`에 82, 세션의 `SearchTextMatches`에 19를 사용한다. 기존 History 77과 DSL 17을 보존한다. 원 binary 골든에서 각 프레임의 variant가 유지되는지 수용한다. [요청·응답 레지스트리](../../../../crates/quanta-index-contract/src/native_protocol.rs#L1515) · [세션 레지스트리](../../../../crates/quanta-index-contract/src/native_protocol/read_session.rs#L337)
- **합류 검증 순서.** 요청·응답·세션 wire 골든을 먼저 실행하고, 공개 전수 매치에서 두 case mode의 독립 바이트 정답, 고정 뷰를 유지하는 다음 페이지, 다른 Source·query에 재사용한 cursor 거절, 취소·예산 한도의 전체 페이지 거절을 확인한다. CLI는 명시적 마지막 페이지와 원문 Source identity를 공개한다.
- 고정 뷰의 원문 매치 조회를 공개 Runtime·SDK·CLI까지 연결한다. repo·경로·검증된 Source 신원과 원문 `[start_byte, end_byte)`, 페이지·취소·거절·완료 상태를 유지한다. 커서는 owner뿐 아니라 **Source 신원 전체**를 묶는다.
- Literal·Text의 원문 검증을 재사용한다. 현재 계약은 줄 단위 leftmost-first 비중첩 매치이며 줄을 넘는 패턴과 빈 매치는 거절한다. 편집 호출자는 적용 직전에 Source 신원을 다시 확인한다. 기본 SDK의 대표 위치와 파일 점수는 Q01, 파일 순위 페이지 비용은 P01 소관이다.

## 완료 조건

- [ ] 독립 원문 스캔과 파일·byte 범위 집합이 일치한다. UTF‑8·NFC·CRLF, 동일 파일 반복 매치, 페이지 경계, 멀티라인 명시적 거절과 재게시 중 고정 뷰를 포함한다.
- [ ] 예산 만료·취소·소스 변경을 시험한다. 불완전 결과에 완료 신호가 없고, 바뀐 원문에 이전 범위를 적용하지 않는다.
- [ ] 공개 소비자가 조회를 끝까지 실행하고 마지막 페이지의 완료를 확인한다. [ripgrep JSON](https://docs.rs/grep-printer/latest/grep_printer/struct.JSON.html) · [Sourcegraph 결과 형식](https://sourcegraph.com/docs/code-search/types/search-jobs)

## 의존·인계

독립 골든은 바로 준비한다. [Q01](Q01-sdk-codesearch.md)의 파일 결과 신원과 [Q02](Q02-query-scope.md)의 범위 계약을 인수한 뒤 공개 API를 연결한다. 공용 wire·SDK 등록 패치는 [V03](V03-signed-release.md)에 합류한다.
