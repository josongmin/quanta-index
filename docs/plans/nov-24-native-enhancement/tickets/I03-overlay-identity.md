# I03 · 미저장 버퍼의 저장소별 격리

우선순위 **P1**. 다중 저장소의 미저장 검색을 제공하는 기능의 필수 조건이다.

## 현황

현재 main 작업 트리의 오버레이 신원은 인증된 `(repository, path)`다. 서로 다른 repo의 같은 경로는 공존하고, 같은 repo의 중복과 잘못된 witness·저장소로의 저장은 거절한다. [lexical 충돌·masking](../../../../crates/quanta-index-lexical/src/native/text_reader.rs#L1071) · [wire witness·hit repository](../../../../crates/quanta-index-contract/src/native_protocol/headless.rs#L59) · [저장 검사](../../../../crates/quanta-index-searchd-runtime/src/native_overlay_transport.rs#L77). [10 진행 기록](../../nov-20-native-cutover-closeout/tickets/10-native-ide-agent-gaps.md#진행-기록)은 두 repo의 수정·삭제·생성, 재시작, 저장소별 저장과 publisher lease 회귀를 집중 실행한 범위에서 `VERIFIED`로 기록한다. 공식 public-api baseline은 고정 nightly 부재로 `BLOCKED`; 현행 최종 후보 전체 수용으로 승격하지 않는다.

기존 파일 수정에서 오버레이 경로를 생략하면 기준 Source 경로를 저장 요청과 선택 결과에 결속한다. 동일 본문을 다른 경로로 게시하려면 오버레이에서 경로를 명시해야 한다. 삭제 저장은 `next=None`을 요구한다. 인증 중 같은 요청이 먼저 저장을 마치면 뒤따른 요청은 결속된 요청 신원으로 재시도를 판정한다. 이름 변경·삭제·동시 재시도 회귀는 `NOT_RUN`이다. [경로 인증](../../../../crates/quanta-index-searchd-runtime/src/native_overlay_transport.rs#L233) · [저장 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_overlay_uds/repository_identity.rs#L485) · [경쟁 회귀](../../../../crates/quanta-index-searchd-runtime/src/native_overlay.rs#L1507)

## 작업 범위

- 오버레이 소유권을 인증된 repo·경로 및 owner에 결속한다. 같은 저장소 내부의 실제 충돌은 명시적으로 거절한다. 수정·삭제·새 파일과 기존 Source masking은 유지한다.
- 저장 CAS가 같은 owner·Source를 게시한 뒤 해당 오버레이만 해제한다. 데몬·publisher 재시작 뒤의 기존 세션 거절과 호출자 재제출을 확인한다.

## 완료 조건

- [ ] 서로 다른 두 repo의 `src/lib.rs`를 동시에 편집한다. A를 저장해도 B의 미저장 버퍼는 남고, 삭제·새 파일에서도 결과가 섞이지 않는다.
- [ ] 경합·publisher 유실·재시작에서 이전 세션은 거절되고, 새 세션에 다시 올린 텍스트만 미저장 결과로 읽힌다. 저장된 결과에는 원본 종류가 명확하다. [기존 오버레이 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_overlay_uds/overlay_search.rs)
- [ ] 두 repo의 동일 NFC 경로와 repo A의 미저장 NFD 경로를 `Latest`로 SourceAdvance 전후 조회하는 회귀를 추가했다. repo A의 가림이 repo B에 번지지 않고, NFC·NFD 원본 바이트가 구분되며, repo A의 동일 NFC 중복은 거절되어야 한다. Nextest `c6fa7d37-b39c-490d-a089-95a955bf4e2a`에서 이 본문은 **1/1 `VERIFIED`**였다. Unicode 입력의 재시작 조합과 서명 설치 수용은 `NOT_RUN`이다. [새 fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_overlay_uds/repository_identity.rs#L447) · [Source 경로 규칙](../../../../crates/quanta-index-contract/src/index/source_path.rs#L9)

## 의존·인계

오버레이 fixture는 바로 준비한다. [Q02](Q02-query-scope.md)·[S01](S01-corpus-reconciliation.md)의 repo 신원 규칙을 받는다. `text_reader.rs`의 오버레이 블록을 소유하고, 공용 필터·wire·SDK 변경은 [V03](V03-signed-release.md)와 합류 순서를 맞춘다.
