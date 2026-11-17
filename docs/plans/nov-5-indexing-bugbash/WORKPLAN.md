# 구현·검증 계획

Status: `EXECUTION CONTRACT AND HISTORICAL PROOF INDEX`.

완료된 B01–B83·Alias/Posting·A–E·F1/F2 구현/예약 기록은 현재 작업 큐에서 퇴역했다.
상시 구현 계약은 [Native custody ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md),
Query/ANN은 [Query ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md),
실행기/claim은 [실행 ADR](../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md)이 소유한다.
이 문서는 남은 실행 계약과 원 증거의 위치만 유지한다.

## 현재 작업과 작성권

| 범위 | 유일한 현재 경로 |
| --- | --- |
| source별 제품 판정 | [bugbash README](README.md#현재-판정). 과거 Large 통과·XL/Mixed 실패와 후행 후보를 분리한다. |
| Native Text 구축 | [NC-TEXT](../nov-12-native-cutover/tickets/NC-TEXT-build-final.md). B03의 옛 allocator/codec 후보를 새 구현으로 발급하지 않는다. |
| ANN·Query·History·설치 복구 | [현재 Native 계획](../nov-12-native-cutover/README.md). B/C/H/D의 실제 잔여와 writer를 소비한다. |
| 출하 후보·SDK60·XL/Mixed·독립 비교 | [NC-F](../nov-12-native-cutover/tickets/NC-F-integration.md). 기존 source/binary/selector의 유효 결과를 재사용한다. |
| C0·설치 DELTA 및 provider | [B05](../nov-12-bugbash-c0-p09/tickets/B05-final-verification.md)·[provider/P07](../nov-12-native-cutover/tickets/NC-F-integration.md#p07-final-actions-20261115). 원 실행기·공급·입장을 중복하지 않는다. |
| 전체 범위·독점 hunk·최종 수용 | [Oct-22 SSOT](../oct-22-finalizing/README.md)·[수정 경계](../oct-22-finalizing/OWNERSHIP.md#exclusive-boundaries)·[DoD](../oct-22-finalizing/DOD.md). |

과거 채팅·PID·reservation·동결 snapshot·중간 NOT_RUN은 현재 작성권이나 살아 있는
실행 증거가 아니다. 해당 활성 티켓과 실제 caller/dirty hunk·canonical admission을
대조한 뒤 진행한다. 다른 source의 focused GREEN은 현재 제품·설치·release 수용이 아니다.

## 실행에 유지할 원 조건

- 기존 F1/F2의 Large/XL은 각각4,096/32,768 source·seed42·server dispatch120초다.
  full ACK·current/old 검색·delta/noop/delete·drain/reopen을 실제 동일 pair로 확인한다.
- 원 Mixed는 Off50QPS·1Hz·960초·pin600·renew120/maxgap180·settle60이다.
  요청·관측·행정 timeout과 제품 deadline을 구분한다. quiet-host와 원 work/memory/disk
  상한·거절·전체 자원/환급 조건을 낮추지 않는다.
- SDK30k/60초·Work512MiB·R/W/M 각1M, 표준 Large 두 OS 재시작, ANN/holdout, 정식 Off/Record
  overhead와 Linux 운영·release는 각각 원 scope로 판정한다. [Native DoD](../nov-12-native-cutover/DOD.md)·[E07 profile](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles).
- 계수의 단위·정확한0/미관측·실패-prefix·성공 전체·물리 I/O를 구분한다.
  완료된 B01/B02를 재구현하지 않고 [Diff 계수 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#inventory-diff-observation)을 소비한다.

<a id="final-audit-execution-20261110"></a>
## 2026-11-10 최종 감사 실행 게이트

Q0–Q6는 11월 10일 감사에서 적용한 게이트다. 당시 A–E·검색 배정은
[복구 색인](../../ARCHIVE-INDEX.md#2026-11-17-restart-and-cursor-contract-reconciliation)에 보존한다.
현재 작업과 결과는 [활성 티켓](../oct-22-finalizing/README.md#활성-작업-문서),
그때의 제품 결과와 비용은 [README](README.md#현재-판정)와 [RCA](RCA.md)가 소유한다.

최종 감사에서 Core→Storage→Kernel→Native occurrence/union receipt와
`AttemptObjectReads` forwarding은 이미 연결돼 있음을 확인했다. Mac native RSS도
후행 D 구현에 반영됐다. 이 두 항목을 옛 미접합·미구현 목록으로 다시 발급하지 않는다.
Catalog strict8건의 후행 수리·scoped strict 성공과 현 통합 전체 strict는 다른 경계다.

| 게이트 | 기존 담당·다음 작업 | 종료 조건과 재사용 경계 |
| --- | --- | --- |
| Q0 입력·중복 대조 | 같은 소스·target·feature·selector·witness를 대조한다. | 중복 body를 합산하지 않고 원 증거를 재사용한다. |
| Q1 영향 회귀·strict | 기존 오라클의 영향 compile/본문·typed 거절·정산을 검증한다. | 서로 다른 소스의 strict 진단과 본문 결과를 분리한다. |
| Q2 제품 입력·binary binding | 같은 coherent source/config/deps/tools로 pair를 만들고 source·실행 파일·vendor lock·입력 지문을 대조 | 실제 build/provenance binding과 실행 전후 일치. 무시된 vendor lock 누락·partial capture·controller 오류를 숨기지 않음 |
| Q3 XL·Mixed 수용 | 기존 SourceApply·Text 설치·Catalog queue 오너가 실패 원인을 확인한다. | XL120초 ACK/검색/재오픈과 Mixed50QPS·1Hz·960초·pin600·settle60 및 원 자원·환급 조건을 유지한다. |
| Q4 cold Native·검색 | 기존 검색 담당의 Literal/Text·Map/ASCII/SDK/Legacy/ranking 경계 유지. LiteralV3/Text 반복 데이터·발행 비용과 Legacy 입력 동결을 검증 | 원60초·파일3만·512MiB·읽기1M 및 원 holdout/gold. lexical 대비 열세인 hybrid를 개선 완료로 기록하지 않음. Q3 source와 다르면 결과도 분리 |
| Q5 Linux·최종 통합 | 기존 SUP/P09 성공물과 호환 searchd를 소비해 설치DELTA/재시작·source-finisher·거절후write/RPC0를 실행. 최신 필수 selector와 generated inventory를 대조 | immutable ELF/SHA/receipt·source/guest/ABI 수락, 미등록/누락 witness0. 성공 공급물 없이 설치 수락하지 않고 worker를 별도 레인에서 중복 빌드하지 않음 |
| Q6 원래 전체 수용 | 원 E01–E08·R01–R40/통합A→default전환→B/full-suite/CI/release·운영·제품 비교 계약으로 결속 | 각 원 요구사항의 final source/input/feature/toolchain과 실실행 증거 일치. focused GREEN·Large 성공·푸시만으로 전체 완료하지 않음 |

Q0 뒤 독립된 소스 수리·정적 검토는 기존 담당이 병행한다. coherent 배치가 있으면
그 영향 회귀를 먼저 실행할 수 있으며 모든 구현 종료까지 독립 검증을 막지 않는다.
같은 호스트의 무거운 실행·공유 parent hunk·shared index는 기존 소유 큐/작성자를 유지한다.
입장 전 capacity·메모리·lock·lease·incumbent를 확인하고 과거 디스크 수치·PID를 재사용하지 않는다.

source/custody 거절, 자원/lock 입장 실패, compiler RED, runtime assertion 실패,
binary 공급 부재는 별도 실패 원인이다. 수리한 postimage와 실제 source/receipt가
연결될 때만 영향 검증을 갱신한다. P04/P08의 수락은 [완료 기록](../../ARCHIVE-INDEX.md#2026-11-12-publication-and-sidecar-closeout)을
재사용하고 P09 설치DELTA와 정식 성능·release 수락을 독립 유지한다.

### 실행 준비·본문·증거 가용성

Q0/Q1은 기존 registry에서 package·Cargo target/name·features/defaults·toolchain·selector와
예상 witness 집합/필수 개수를 실행 전에 고정한다. test rail은 필수 expected/selected/executed/passed가
일치하고 실제 passed가0보다 커야 수락한다. 필수 본문의 ignored/skipped·중복 실행·다른 selector의
통과로 분모를 채우지 않는다. 원 계약의 명시적 제외는 따로 기록하며 사후에 expected를 줄이지 않는다.
`check`/`build` 성공은 해당 compiler/artifact 범위이며 실행 본문0의 test/Product 통과가 아니다.

Q2/Q5는 실제 binary·원 terminal receipt/stdout/stderr의 bytes/해시·source/request·guest/ABI 결속,
설치/서명 신뢰 입력과 public readback을 확인한다. 원 receipt가 사라졌거나 partial capture만 있으면
기존 보존/복구 authority로 동일 identity를 증명하거나 새 coherent source에서 영향 rail을 실행한다.
경로 문자열·다른 코호트의 성공·수동 receipt 복원으로 현재 build binding을 만들지 않는다.

중단·locator 부재 때는 실제 incumbent와 canonical terminal 상태부터 확인한다. 살아 있는 실행을
복제하지 않고, 예약 담당·holder·반환 조건과 source lease를 기록한다. 준비되지 않은 source를
기다리며 다른 필수 공급자의 Rust 슬롯을 점유하지 않는다. 원 자원 하한·timeout·분모는 유지한다.
source 인계와 proof 준비·실행·terminal 판정은 구분하고, 통과한 source의 필수 입력 변화가 있으면
그 영향 rail만 재검증한다. 공급물 부재는 해당 소비자만 막으며 독립 호스트 검증의 전역 차단이 아니다.

## 고정 제품 계약

- Large4,096-source SourceAdvance의 서버 dispatch120초와 기존 명시적 work caps를 사용한다.
- 정상 ACK 뒤 current/old search, delta/noop/delete, drain/reopen을 검증한다.
- FULL·auto1000, GC·reserve·physical capacity와 원 memory/work 거절 계약을 적용한다.
- identity, 실제 payload 형식, root/namespace 현재성, cancellation과 오류 settlement를 검증한다.
- Medium은 동일 UDS 경로의 진단 입력이다. Large 완료와 정식 overhead를 각각 판정한다.

## 필요한 독립 검증

| Scope | 최소 oracle |
| --- | --- |
| 계수 | 신규·prior·중복·RAM/spill·실패를 독립 입력으로 계산, 관측 없음과0을 구분 |
| 저장/검증 | 잘못된 identity·형식·count, confirm 실패, 다른 attempt, 잘못된 backend 결과 거절 |
| 수명 | memory/work 원 한도, retained release, cancel/drop/retry, 부분 성공 settlement |
| Off/Record | Off의 관측 작업 부재와 동일 제품 결과, complete footer·loss/lease/open0 |
| 실제 경로 | UDS 요청에서 현재 canonical packet과 public port/wrapper를 실제로 거침 |
| 성능 | 각 arm의 source/binary 결속, 같은 입력·작업량, 원 deadline, 완료 ACK/search/reopen |

기존74개 fresh-claim·487개 Python 회귀 등은 해당 고정 소스의 근거로 재사용한다.
변경하지 않은 전체 workspace와 별도 플랫폼을 매 패킷마다 실행하지 않는다.
전체 release·Linux 운영·XL/mixed와 정식≤5% overhead는 상위 E07/E08 분모를 유지한다.

## 완료 패킷과 증거의 처분

B01/B02 계수, B03의 검증 재사용·occurrence/Map/Posting 설치, B83 retained read,
Alias/SQL·A–E 합류와 F1/F2 실행 준비는 신규 티켓으로 발급하지 않는다. 현재 설계는
위 ADR과 실제 producer→caller→consumer가 소유하며, 옛 storage version·후보·
배치 크기·소유 채팅은 동결 구현 이력이다. 원 XL/Mixed·SDK60·설치·release 수용은 남는다.

실패·측정 원본은 [RCA](RCA.md), 다른 원장에 없는 행은 [보존 ID](README.md#다른-원장에-없는-실행-id의-원문-행),
정리 직전의 정확한 문서·명령·절단별 계약은 [복구 색인](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation)에서 회수한다.
아래 인용은 다른 RCA에 없는 원문 식별자를 보존한다. 그 안의 당시 "현재"·
미실행·예약·채팅 지시는 역사 기록이며 지금 수행할 명령이 아니다.

The historical quoted body (510 lines) is retained byte-for-byte in the
[exact recovery preimage](../../ARCHIVE-INDEX.md#2026-11-16-bugbash-evidence-index-compaction).
It includes the original commands, source/run hashes, failed attempts,
negative sensitivity runs, owner/handoff history and then-current limits.
Those records are not new source readiness, a live reservation or another queue.
Implemented Alias/packed Diff/Posting/read-custody contracts remain in the Native
ADR above. Fixed Q0-Q6, product/resource/overhead/Original gates and the current
Native ticket boundaries are unchanged. Distinct historical results must not be
added together or reused across changed proof inputs.


## 과거 패킷별 복구와 현재 계약

옛 fragment는 아래 회수 표로 연결한다. 원 RCA에 같은 절이 있으면 그 절을,
그 외에는 정확한 dirty 원문 복구 경로를 제공한다. 현재 수용은 상단 활성 패킷을 따른다.

| 과거 절 | 원문·증거 위치 |
| --- | --- |
| <a id="e-source-audit-20261111"></a><a id="e-코드-대조-후-검증-잔여--2026-11-11"></a>E 코드 대조 후 검증 잔여 — 2026-11-11 | [원문](RCA.md#e-source-audit-20261111) |
| <a id="final-remaining-audit-20261111"></a><a id="최종-잔여-재구현-없이-닫을-세-패킷"></a><a id="f1f3-종료-기록과-현재-인계"></a>F1–F3 종료 기록과 현재 인계 | [원문](RCA.md#final-remaining-audit-20261111) |
| <a id="실제-위임과-간섭-금지"></a>실제 위임과 간섭 금지 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="후속-단일-예약--완료된-빌드-재사용"></a>후속 단일 예약 — 완료된 빌드 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="f1에서-확인한-실제-source-gap"></a>F1에서 확인한 실제 source gap | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="f1-controller-인계에서-보완할-경계"></a>F1 controller 인계에서 보완할 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="직전-종료-상태와-현재-재개"></a><a id="11월1112일-종료와-실행권-반환"></a>11월11–12일 종료와 실행권 반환 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="session-hardening-20261110"></a><a id="session-hardening-범위귀속검증"></a>Session hardening: 범위·귀속·검증 | [원문](RCA.md#session-hardening-20261110) |
| <a id="실행-담당-재확인--2026-11-11"></a>실행 담당 재확인 — 2026-11-11 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="alias-sql-implementation-20261110"></a><a id="현재-코드-패킷-구현영향-회귀-완료"></a><a id="aliasposting-구현-완료와-당시-회귀"></a>Alias·Posting 구현 완료와 당시 회귀 | [원문](RCA.md#alias-sql-implementation-20261110) |
| <a id="검증-증거와-범위"></a>검증 증거와 범위 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="남은-수용--기존-native-실행-owner-유지"></a>남은 수용 — 기존 Native 실행 owner 유지 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="remaining-cost-audit-20261110"></a><a id="추가-rca-후-남긴-두-패킷"></a><a id="완료된-aliasposting-후보-처분"></a>완료된 Alias·Posting 후보 처분 | [원문](RCA.md#remaining-cost-audit-20261110) |
| <a id="packed-diff-rca-20261110"></a><a id="2026-11-10-packed-diff-할당-제거-패킷"></a>2026-11-10 Packed Diff 할당 제거 패킷 | [원문](RCA.md#packed-diff-rca-20261110) |
| <a id="posting-product-rca-20261110"></a><a id="정규화-후보의-제품-rca-실행"></a>정규화 후보의 제품 RCA 실행 | [원문](RCA.md#posting-product-rca-20261110) |
| <a id="posting-value-normalization-20261110"></a><a id="2026-11-10-posting-정규화-수리검증-패킷"></a>2026-11-10 Posting 정규화 수리·검증 패킷 | [원문](RCA.md#posting-value-normalization-20261110) |
| <a id="xl-mixed-resume-20261110"></a><a id="2026-11-1011-xlmixed-재개-실행"></a>2026-11-10–11 XL·Mixed 재개 실행 | [원문](RCA.md#xl-mixed-resume-20261110) |
| <a id="후속-준비-기존-구현-소비와-실행기-수리"></a>후속 준비: 기존 구현 소비와 실행기 수리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="0346-kst-후속-소스와-입장-재확인"></a>03:46 KST 후속 소스와 입장 재확인 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="현재-구현과-다음-수용"></a><a id="ae-완료-구현과-동결-후보-이력"></a>A–E 완료 구현과 동결 후보 이력 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="병렬-채팅-배정--2026-11-09-결과-갱신-2026-11-10"></a>병렬 채팅 배정 — 2026-11-09, 결과 갱신 2026-11-10 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="코드-직접-대조-재감사--현재-소스와-검증-경계"></a>코드 직접 대조 재감사 — 현재 소스와 검증 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="소스-준비-범위와-부모-검증-selector"></a>소스 준비 범위와 부모 검증 selector | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="r56-이후의-최소-코드검증-경계"></a>r56 이후의 최소 코드·검증 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="2026-11-10-추가-개선-후보와-중복-제외"></a>2026-11-10 추가 개선 후보와 중복 제외 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="causal-measurement-packet-20261110"></a><a id="q3-원인-분해-계측-패킷--2026-11-10"></a>Q3 원인 분해 계측 패킷 — 2026-11-10 | [원 증거](RCA.md#causal-measurement-repair-20261110) |
| <a id="d-verification-closeout-20261110"></a><a id="d의-잠금-해제-후-검증-마감--2026-11-10"></a>D의 잠금 해제 후 검증 마감 — 2026-11-10 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="2026-11-09-정적-재감사의-구현-패킷"></a>2026-11-09 정적 재감사의 구현 패킷 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b29--stagesources-설치-트랜잭션-감소"></a>B03-B29 — StageSources 설치 트랜잭션 감소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="mixed-거절-경로-분리"></a>Mixed 거절 경로 분리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b31--source-record-재읽기와-mixed-갱신-경계"></a>B03-B31 — Source record 재읽기와 Mixed 갱신 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b33--packed-occurrence의-pending-슬롯-재사용"></a>B03-B33 — Packed occurrence의 pending 슬롯 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b34--interactive-예약의-bounded-fifo"></a>B03-B34 — Interactive 예약의 bounded FIFO | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b35--cold-ascii-owner의-토큰-scratch-후보-철회"></a>B03-B35 — Cold ASCII owner의 토큰 scratch 후보 철회 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b36--일반-text-실패-귀속과-readview-예산"></a>B03-B36 — 일반 Text 실패 귀속과 ReadView 예산 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b37--control-transient-거절-대기"></a>B03-B37 — Control transient 거절 대기 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b38--retained-세션의-공유-검색"></a>B03-B38 — Retained 세션의 공유 검색 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b39--만료-tick의-retained-갱신-차단"></a>B03-B39 — 만료 tick의 retained 갱신 차단 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b40--catalog-pin-검증의-sql-준비-재사용"></a>B03-B40 — Catalog pin 검증의 SQL 준비 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b41--mixed-publisher의-직전-인증-view-재사용"></a>B03-B41 — Mixed publisher의 직전 인증 view 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b42--posting-scratch의-고정-폭-owner-key"></a>B03-B42 — Posting scratch의 고정 폭 owner key | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b43--열린-namespace-디렉터리의-fd-신원-재사용"></a>B03-B43 — 열린 namespace 디렉터리의 FD 신원 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b44--warm-posting의-이전-terms-map-배치-조회"></a>B03-B44 — warm Posting의 이전 terms Map 배치 조회 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b45--mixed-held-readsession과-발행-스케줄-분리"></a>B03-B45 — Mixed held ReadSession과 발행 스케줄 분리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b46--디스크-census의-중복-집계-제거"></a>B03-B46 — 디스크 census의 중복 집계 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b47--pending-settlement-응답-보존"></a>B03-B47 — Pending settlement 응답 보존 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b48--catalog-settlement의-첫-차단-조건"></a>B03-B48 — Catalog settlement의 첫 차단 조건 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b49--검색-posting-페이지의-ownersource-map-배치"></a>B03-B49 — 검색 Posting 페이지의 owner/source Map 배치 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b50--xl-단일-packed-inventory의-한-행-scratch-seal"></a>B03-B50 — XL 단일 packed inventory의 한 행 scratch seal | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b51--terminal-custody-후보-조회의-writer-예약-제거"></a>B03-B51 — terminal custody 후보 조회의 writer 예약 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b52--같은-요청의-인증된-text-member-재사용"></a>B03-B52 — 같은 요청의 인증된 Text member 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b53--packed-inventorydiff의-scalar-memo-조회-제거"></a>B03-B53 — packed InventoryDiff의 scalar memo 조회 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b54--custody-root의-독립-페이지-한도"></a>B03-B54 — custody root의 독립 페이지 한도 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b55--동일-settlement-snapshot의-oldest-ready-sequence"></a>B03-B55 — 동일 settlement snapshot의 oldest ready sequence | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b56--동일-티켓-검사에서-인증된-inode-재사용"></a>B03-B56 — 동일 티켓 검사에서 인증된 inode 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b57--custody-고정-행-조회와-트랜잭션-순회"></a>B03-B57 — custody 고정 행 조회와 트랜잭션 순회 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b58--승인된-cold-owner의-canonical-token-그룹"></a>B03-B58 — 승인된 cold owner의 canonical token 그룹 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b59--cold-posting-전용-최대-128행-scratch-배치"></a>B03-B59 — cold Posting 전용 최대 128행 scratch 배치 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b60--packed-diff-count의-순차-ordinal"></a>B03-B60 — packed Diff count의 순차 ordinal | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b61--custody의-진행과-거절을-분리"></a>B03-B61 — custody의 진행과 거절을 분리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b62--한-custody-page의-reclaim-인증-재사용"></a>B03-B62 — 한 custody page의 reclaim 인증 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b63--retained-searchtext의-실패-단계-시간"></a>B03-B63 — retained SearchText의 실패 단계 시간 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="mixed와-xl-후속-비용-귀속"></a>Mixed와 XL 후속 비용 귀속 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b83--retained-read의-검증내구성-경계"></a>B83 — retained read의 검증·내구성 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="xl-후속--posting-scratch와-설치-경계"></a>XL 후속 — Posting scratch와 설치 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b30--pin-검증의-원-요청-예산-전파"></a>B03-B30 — Pin 검증의 원 요청 예산 전파 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b28--cold-occurrence-위치-배열의-중복-비교-축소"></a>B03-B28 — Cold occurrence 위치 배열의 중복 비교 축소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="xl-단일-공개의-남은-계약"></a>XL 단일 공개의 남은 계약 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b27--source-경로-map의-물리-노드-감소"></a>B03-B27 — Source 경로 Map의 물리 노드 감소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b26--mixed-실행-창과-관측-계약-일치"></a>B03-B26 — Mixed 실행 창과 관측 계약 일치 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b25--qimap005-저장소의-구버전-부팅-차단"></a>B03-B25 — QIMAP005 저장소의 구버전 부팅 차단 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b24--읽기-전용-catalog의-record-이벤트-감소"></a>B03-B24 — 읽기 전용 Catalog의 Record 이벤트 감소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b23--동일-inventory-diff의-packed-data-재사용"></a>B03-B23 — 동일 Inventory Diff의 packed DATA 재사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b22--terms-map의-물리-노드-감소"></a>B03-B22 — Terms Map의 물리 노드 감소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b21--최상위-map-물리-노드와-record-검증-비용"></a>B03-B21 — 최상위 Map 물리 노드와 Record 검증 비용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b20--dense-posting-물리-노드-감소"></a>B03-B20 — Dense Posting 물리 노드 감소 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b19--단일-packed-occurrence를-초기-inventory로-직접-사용"></a>B03-B19 — 단일 packed occurrence를 초기 inventory로 직접 사용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b18--native-owner-contribution의-별도-파일-제거"></a>B03-B18 — Native owner contribution의 별도 파일 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b17--설치-창의-메모리gate관측-보완"></a>B03-B17 — 설치 창의 메모리·gate·관측 보완 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b16--posting-물리-배치의-catalog-전이-창"></a>B03-B16 — Posting 물리 배치의 Catalog 전이 창 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b15--inventory-scratch와-물리-writer-반복-제거"></a>B03-B15 — Inventory scratch와 물리 writer 반복 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b14--cold-inventory-물리-배치와-연속-catalog-advance"></a>B03-B14 — Cold inventory 물리 배치와 연속 Catalog advance | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b13--native-writer의-중복-parent-검사-제거"></a>B03-B13 — Native writer의 중복 parent 검사 제거 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b12--native-writer의-held-parent-생성과-publish"></a>B03-B12 — Native writer의 held parent 생성과 publish | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b11--native-writer의-최초-전체-크기-예약"></a>B03-B11 — Native writer의 최초 전체 크기 예약 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b10--cold-contribution-배치와-writer-잔여-비용-분리"></a>B03-B10 — cold contribution 배치와 writer 잔여 비용 분리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기"></a>B03-B9 — writer birth 검증 공유와 인접 Catalog 경계 합치기 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b7--cold-posting의-두-case-표면을-같은-설치-배치에-배치"></a>B03-B7 — cold Posting의 두 case 표면을 같은 설치 배치에 배치 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b7-검증-및-분기"></a>B7 검증 및 분기 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b8--cold-root의-build와-scratch-인증을-같은-순회에서-수행"></a>B8 — cold root의 build와 scratch 인증을 같은 순회에서 수행 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b6--희소-posting-준비-조회와-기존-shard-생성-호출"></a>B03-B6 — 희소 Posting 준비 조회와 기존 shard 생성 호출 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b5--dense-posting-tree의-비교-witness와-실제-custody-분리"></a>B03-B5 — dense Posting Tree의 비교 witness와 실제 custody 분리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b4--small-node-배치의-transaction-local-child-proof"></a>B03-B4 — small-node 배치의 transaction-local child proof | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b3--inventory-block-child-dictionary"></a>B03-B3 — inventory block child dictionary | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b2--owner-local-block의-물리-설치-단위"></a>B03-B2 — owner-local block의 물리 설치 단위 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="execution-plan"></a><a id="b03-b1b3-실행-계약이력--2026-11-07"></a>B03-B1–B3 실행 계약·이력 — 2026-11-07 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="중복-제외와-소유-경계"></a>중복 제외와 소유 경계 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b01"></a><a id="b01--계수와-유효성-계약"></a>B01 — 계수와 유효성 계약 | [현재 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#inventory-diff-observation) |
| <a id="b02-현행-계수-계약"></a>B02 현행 계수 계약 | [현재 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#inventory-diff-observation) |
| <a id="b02"></a><a id="b02--최소-계측과-소비자-연결"></a>B02 — 최소 계측과 소비자 연결 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="현재-소스와-정확한-잔여"></a>현재 소스와 정확한 잔여 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03"></a><a id="b03--검증-결과-전달과-구조-수리"></a>B03 — 검증 결과 전달과 구조 수리 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="선택표와-필요한-관측"></a>선택표와 필요한 관측 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-a"></a><a id="b03-a--검증된-결과를-실제-소비자까지-전달"></a>B03-A — 검증된 결과를 실제 소비자까지 전달 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b"></a><a id="b03-b--lexical-occurrence-저장-단위를-묶기"></a>B03-B — Lexical occurrence 저장 단위를 묶기 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b-후속-계약-검토--원-기준-정정"></a>B03-B 후속 계약 검토 — 원 기준 정정 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b1--codecselector-abi-고정"></a>B03-B1 — codec·selector ABI 고정 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b03-b1-실제-합류와-검증-큐"></a>B03-B1 실제 합류와 검증 큐 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="계약-합류-후-병렬-소유"></a>계약 합류 후 병렬 소유 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="b04"></a><a id="b04--합류검증제품-수용"></a>B04 — 합류·검증·제품 수용 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="현재-고정본과-실제-실행"></a>현재 고정본과 실제 실행 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="중단-확인-후-캐시-재사용-재개"></a>중단 확인 후 캐시 재사용 재개 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="새-원-large-off-결과와-단일-record-진단"></a>새 원 Large Off 결과와 단일 Record 진단 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="현재-후속-코드-고정본"></a>현재 후속 코드 고정본 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="실행-수와-완료-기준"></a>실행 수와 완료 기준 | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="root-integration-follow-up--fixed-sdk60-and-independent-file-holdout"></a>Root integration follow-up — fixed SDK60 and independent file holdout | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="resumed-current-source-rust-execution-2026-11-10"></a>Resumed current-source Rust execution (2026-11-10) | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="rrss-parallel-follow-up-2026-11-11"></a>RR/SS parallel follow-up (2026-11-11) | [원문](../../ARCHIVE-INDEX.md#2026-11-15-completed-implementation-packet-consolidation) |
| <a id="native-final-executor-20261111"></a><a id="최종-실행기의-f1f2-인계"></a>최종 실행기의 F1/F2 인계 | [원문](RCA.md#native-final-executor-20261111) |
| <a id="native-final-executor-retry2-20261111"></a><a id="단일-900초-예약의-종료"></a>단일 900초 예약의 종료 | [원문](RCA.md#native-final-executor-retry2-20261111) |
| <a id="native-final-executor-7200-20261111"></a><a id="단일7200초-예약-입장-성공-timing-preflight-차단"></a>단일7200초 예약: 입장 성공, timing preflight 차단 | [원문](RCA.md#native-final-executor-7200-20261111) |
| <a id="native-index-rca-tools-20261111"></a><a id="native-병목-진단-후속"></a>Native 병목 진단 후속 | [원문](RCA.md#native-index-rca-tools-20261111) |

## 보존한 원 증거 참조

이 링크들은 이전 source의 결과·입력을 회수하기 위한 참조이며 현재 실행 상태가 아니다.

- [최종 실행 게이트](#final-audit-execution-20261110)
- [F1–F3 종료 기록](#final-remaining-audit-20261111)
- [고정 제품 계약](#고정-제품-계약)
- [Native custody ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#consolidated-implementation-boundaries)
- [Native custody ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#decision)
- [B10 RCA](RCA.md#b10--posting-전-contribution-설치)
- [B11 RCA](RCA.md#b11--최초-전체-writer-예약-검증)
- [B12 RCA](RCA.md#b12--writer-parent-custody와-catalog-commit-잔여-비용)
- [B13 RCA](RCA.md#b13--writer-cpu-회수와-최상위-map-잔여)
- [요청 전체 분해](RCA.md#b13-요청-전체-원인-분해)
- [단계별 수치](RCA.md#b14--물리-그룹-축소와-기존-advance-연결)
- [B15 RCA](RCA.md#b15--inventory-scratch와-물리-writer-병목)
- [B16 RCA](RCA.md#b16--posting-writer와-catalog-commit)
- [리뷰 RCA](RCA.md#b17--설치-창-리뷰-보완)
- [B18 RCA](RCA.md#b18--blocktree-owner-contribution-인라인)
- [B23 RCA](RCA.md#b23--inventory-diff의-packed-data-재사용)
- [B24 RCA](RCA.md#b24--읽기-전용-catalog-scope의-record-이벤트-감소)
- [B8 RCA](RCA.md#b8-재검증)
- [B9 RCA](RCA.md#b9의-수리-경계)
- [코드 RCA와 채택하지 않은 가설](RCA.md#causal-measurement-repair-20261110)
- [인과관계 한계](RCA.md#current-causal-rca)
- [RCA](RCA.md#lr2-오프라인-분석-실제-도달-경로와-다음-코드-범위)
- [코드 경계와 검증 명령](RCA.md#native-cold-repeated-work-20261111)
- [원 실패 분석](RCA.md#native-f2-xl-mixed-terminal-20261112)
- [실행기 감사](RCA.md#native-final-controller-hardening-20261111)
- [소유권과 검증 책임 감사](RCA.md#native-ownership-structural-audit-20261112)
- [코드와 m2 근거](RCA.md#native-path-and-mixed-diagnostics-20261111)
- [후속 결과](RCA.md#native-product-hot-path-repair-20261111)
- [입력과 실측 사례](RCA.md#native-run-triage-20261111)
- [검증 책임과 진단 범위](RCA.md#native-source-body-single-read-20261112)
- [측정과 증거 결속 변경](RCA.md#native-trace-custody-and-stage-time-20261112)
- [원 집계](RCA.md#r56-improvement-candidates)
- [B7 Record 분석](RCA.md#계측-후-b7-record)
- [새 RCA](RCA.md#새-lo2-tracing-off에서도-원120초-실패-재현)
- [취소 주입과 검증 명령](RCA.md#유예-본문-검증-중-취소와-재개)
- [실행 원장](README.md)
- [Linux unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html)
- [SQLite 트랜잭션](https://sqlite.org/lang_transaction.html)
- [SQLite WAL](https://sqlite.org/wal.html)

Recorded local artifact locations:

- `/Users/songmin/.codex/qib6-_0r6mac7/resume-02.py`
- `/Users/songmin/.codex/qib6-_0r6mac7/s`
- `/Users/songmin/.codex/qidx-rca-snapshot-etX5Ij/posting-build-2`
- `/private/tmp/qidx-f3a-20261111-zzx8k19o`
- `/private/tmp/qidx-xl-mixed-rca-z1a_82he/source5`
- `~/.codex/artifacts/nov-5-indexing-bugbash/b02-before-20261105-sk6a9gkl`
