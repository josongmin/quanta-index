# S01 · 저장소 코퍼스와 최신 뷰 대조

우선순위 **P0**. Source 생산자의 파일 집합·신원·최신성 계약을 소유한다. Q의 질의 필터는 [Q02](Q02-query-scope.md) 소관이다.

## 현황

Semantica에는 저장소 스캔 진입점과 수집 정책이 있다. Q SDK `reconcile_complete_universe`는 호출자가 선언한 완전한 집합에서 빠진 파일을 삭제로 해석하므로 Watch 이벤트 유실을 자체 발견하지 않는다. 기존 [02.6](../../nov-20-native-cutover-closeout/tickets/02-semantica-runtime-sdk.md#완료-조건)의 포함·제외 manifest는 열려 있고, 이벤트 유실·오프라인 변경·HEAD·ignore 변경 후 재대조는 [INC-03](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-보강-계획에서-추가한-케이스)에 `NOT_RUN`으로 남아 있다. E07 freshness도 출하 후보에서 미수용이다. [S 스캔](https://github.com/josongmin/semantica-codegraph-v2/blob/94db39f03ee51d573628cdf2bf87a395b8050b6d/packages/analysis/quanta-v2/crates/quanta-sdk-session-backend-owner/src/repository_indexing_v1.rs#L1358) · [SDK 완전 집합 계약](../../../../crates/quanta-index-sdk/src/preparation/implementation.rs#L960) · [E07 운영 기준](../../oct-22-finalizing/E07-performance-operations.md#dod)

## 작업 범위

- commit 또는 worktree, repository identity, revision/HEAD, 수집 정책 버전을 입력에 결속한다. 추적·미추적·ignore, 테스트·설정·문서·생성물, 바이너리·대용량·symlink·LFS의 포함 여부와 제외 사유를 정해 파일·원문 해시의 독립 manifest를 만든다. 파서가 지원하는 파일과 원문 검색 코퍼스를 구별한다. [현행 Semantica 수집](https://github.com/josongmin/semantica-codegraph-v2/blob/94db39f03ee51d573628cdf2bf87a395b8050b6d/packages/analysis/quanta-v2/crates/quanta-sdk-session-backend-owner/src/repository_indexing_v1.rs#L1358) · [수집 정책](https://github.com/josongmin/semantica-codegraph-v2/blob/94db39f03ee51d573628cdf2bf87a395b8050b6d/packages/analysis/quanta-v2/crates/quanta-shared-scan-policy/src/lib.rs#L118)
- FULL과 DELTA 뒤 게시 Source cut·파일 집합·해시를 독립 manifest와 대조한다. Watch 이벤트 누락/overflow, 생산자 중단 중 수정, HEAD·브랜치 전환, ignore/수집 정책 변경에는 재스캔·재대조를 수행하고 최신성 미확정 상태를 최신 뷰로 광고하지 않는다. 파일 이동은 이전 키 삭제와 새 키 추가를 확인한다.
- 인증한 repo·revision·path·lang 메타데이터 및 레거시 레코드의 미확인 필드를 [Q02](Q02-query-scope.md)에 전달한다. repo 또는 lang을 증명할 수 없는 뷰는 해당 범위 질의를 명시적으로 거절하도록 계약을 고정한다. [Source 레코드](../../../../crates/quanta-index-contract/src/index.rs#L143)

## 완료 조건

- [ ] 빈 저장소, 두 repo의 동일 상대 경로, 언어 밖의 원문, 삭제·rename·no-op, 제외 정책별 파일을 FULL 및 DELTA 독립 manifest와 대조한다. 누락을 무근거 tombstone으로 처리하거나 다른 repo 결과를 섞으면 실패다.
- [ ] Watch 이벤트 유실·재시작, 오프라인 편집, HEAD/브랜치·ignore 정책 전환 뒤 실제 파일 집합과 게시 Source cut을 재대조한다. 불일치 중 최신성 상태·오류를 공개하고 보정 후 SDK Latest와 Exact 뷰를 확인한다.
- [ ] 설치 SDK·Watch의 편집→검색 freshness를 원 E07 profile의 p95≤1초/p99≤5초와 대조한다. 이전 코호트의 실행 결과를 현재 서명 후보 합격으로 승격하지 않는다. [E07](../../oct-22-finalizing/E07-performance-operations.md#dod)

## 의존·인계

기존 [02.6](../../nov-20-native-cutover-closeout/tickets/02-semantica-runtime-sdk.md#완료-조건)의 Semantica Source/Watch 생산자와 SDK 준비 입력이 소유 경계다. [INC-01](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25)의 연결된 C1 target·committed seal 경계와 잔여 V1 호환 비용, [S02](S02-single-edit-delta.md)의 한 파일 비용, [INC-03](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25)의 설치 조회를 같은 Source cut으로 결속한다. Q02에는 인증 메타데이터와 거절 조건을, [D01](D01-durable-recovery.md)에는 게시·재시작 입력을 인계한다.
