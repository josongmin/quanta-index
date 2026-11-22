# Native 단일 엔진 전환 계획

이 문서는 2026-11-16 `ade5fadc1`에서 고정한 설계·수용 범위다. 현재 작업 상태와 실제 본문은 [Nov-20 통합 원장](../nov-20-native-cutover-closeout/tickets/README.md) 및 [06 퇴역 티켓](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)에서 판정한다. [E06 전환](../oct-22-finalizing/E06-sdk-service-cutover.md), [E08 통합](../oct-22-finalizing/E08-integration-release.md), [Native 구현 ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md)과 [수용 기준](DOD.md)을 함께 적용한다.

[Tantivy 퇴역 계획](TANTIVY-RETIREMENT.md)은 원래의 오너 경계, 벤치와 삭제 조건을 보존한다. 코드·Cargo/lock/vendor 정리는 현재 소스에서 완료됐고, 실제 Linux 출하 빌드·설치·복구·제품 수용은 남았다. 최신 범위는 [06.3–06.5](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)를 따른다.

P07 Rust·공개 SDK/CLI Published의 실제 입력·검증은 [F 최종 패킷](tickets/NC-F-integration.md#p07-final-actions-20261115)을 따른다. Source/Host/Native 복구의 마감 범위와 기존 오너에서 인수할 입력은 [NC-D 잔여 패킷](tickets/NC-D-boot.md#d-current-disposition-20261116)에 있다. 초기 Source binding·cold graph/DFG·normal fixture의 완료 구현을 재사용한다. 코드 작성·Main 통합·컴파일·signed 제품 수용은 각 영수증의 범위에서 판정한다.

## 현재 종료 판정

**코드 퇴역 완료, 출하 수용 미완료.** 현재 소스의 Tantivy 계열 0건과 전체 타깃 컴파일은 [06의 실제 검사](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md) 범위에서 확인됐다. Linux release·서명 번들·복구·SDK60·독립 품질과 기본 전환은 각 담당 원장의 별도 결과가 필요하다.

Current candidate/run/terminal details are maintained only by the linked owners.
This plan keeps fixed closure boundaries; a historical focused pass or failure
retains its original source, input and product scope.

| Closure scope | Owner and original acceptance |
| --- | --- |
| Caddy/etcd SDK60 | [TEXT](tickets/NC-TEXT-build-final.md#current-nc-text-acceptance) owns source-bound acceptance for both original corpora. Verify FULL, search, reopen, resource usage and two quiet Off rounds per corpus. Run Large with the same daemon under the [fixed acceptance criteria](DOD.md#기존-고정-성능-및-자원-기준). |
| ANN and final restart | [B](tickets/NC-B-semantic.md#current-remaining-acceptance) owns dedicated ANN4K and whole SDK proof, affected-input quality refresh and timeout RCA. Preserve independent exact/selected strata, original quality/admission, old pin/delete/overlay/final/restart gates. Actual current disposition remains in that ticket |
| Independent Native holdout | [F](tickets/NC-F-integration.md) owns the actual source roster, labels/qrels, DEV/EVAL split, SDK/daemon capture and signed release/config/policy binding. A source-only roster or replay fixture is not evaluation. Preserve the [original sample/quality/statistical gates](DOD.md#독립-홀드아웃-실행과-판정) |
| Native caller and installed recovery | Source/V9/FD6/Q, Initial/Watch ancestry, typed RepoMap and Host travel through the existing producer and receiver. [D1-D3](tickets/NC-D-boot.md#current-work-packets-20261115) owns signed normal rollback, fresh restore-forward and postpublication acceptance. The [F queue](tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16) records current static proof and the signed fixture for a settled nonempty Host delivery. |
| Legacy History migration | Reuse the [History ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover) for exact inventory preflight, multi-generation FULL, retained replay and checkpoint handoff. [H](tickets/NC-H-history.md#current-actionable-backlog--2026-11-16-re-audit) owns History replay, the successor and lost-ACK cases. [D](tickets/NC-D-boot.md) and the Q Source/RepoMap owners own preservation of sibling active SearchCorpus and RepoMap heads. [F](tickets/NC-F-integration.md) owns the signed installed run |
| Shipping engine retirement | [06.2–06.3](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md) owns completed source/feature/dependency/lock/vendor removal. [06.4–06.5](../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md) requires the actual Linux release, signed bundle and installed recovery. Preserve the prior external old-Q comparison pair and recovery-only physical verifier. |

## 종료 범위

Native Literal·Text/BM25·FileMap과 Catalog/Kernel을 재사용한다. Semantic은 Quanta의 원본 벡터와 exact scoring을 유지하고 LanceDB 리전으로 ANN을 제공한다. 기본 SDK·CLI·daemon·런처·벤치가 같은 Native 제품 경로를 사용한다. 활성 head·쓰기 권한·GC/보존 권한은 각각 하나다.

Tantivy 우선 전환은 유지할 호출을 이전하고 workspace에서 Tantivy를 제거한 설치 후보로 [T 단계](DOD.md#tantivy-우선-퇴역-t-단계)를 검증한다. 격리된 후보에서 기본 Native 호출, 설치 복구, SDK60·Large·독립 Hybrid를 확인한다. 전역 기본값은 전체 통합 A 통과 후 출하본을 고정해 전환하고, 실제 설치 환경에서 통합 B를 검증한다. LanceDB는 Native Semantic의 ANN 라이브러리로 사용하고 Quanta Catalog가 리전의 게시와 보존을 담당한다. 이전 Legacy 비교·복귀에는 별도로 보존한 이전 릴리스의 바이너리·설정·상태 루트를 사용한다. Zoekt는 격리된 비교 벤치에서만 사용한다. SQLite Catalog, 해시·정규식·직렬화 라이브러리, 임베딩 모델과 제공자는 인프라로 허용한다. 제공자 계약은 벡터 생성과 그 예산·출처까지다.

브레이킹 변경은 새 공개 SDK/IPC 버전, 읽기·관리 소켓 분리, Hybrid 파일 결과·증거, Native ANN 디렉터리 포맷에 적용한다. 새 제품은 새 버전으로 연결하고 구 버전·루트·opcode 조합은 상태를 건드리기 전에 형식 오류로 거절한다. 지원 기능, 내구성, 자원 상한과 [R01–R40 전체 제품 수용](../oct-22-finalizing/DOD.md)을 적용한다. 공개 기능의 퇴역은 실제 호출자 조사와 릴리스 계약에 기록한다. 저장 형식은 새 루트에서 원본으로 재구축하고, 이전 루트는 이전 바이너리와 한 쌍으로 보존한다.

## 구현 계약

공개 Native Query·Control, 같은 view의 Hybrid와 파일 신원, traversal·coverage,
exact/ANN 계약은 [구현 ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#decision)을
따른다. 실제 상위 호출자의 기능·필터·정렬·페이지와 명시적 퇴역은 위 담당이 수용한다.

Hybrid는 DEV에서 lexical·dense·RRF를 고정 비교한다. 후보 누락을 먼저 해결하고,
같은 후보 풀에서 융합 때문에 순위가 밀리면 정규화한 점수 결합을 비교한다.
그 뒤에도 순위 문제가 남을 때만 고정 개수의 상위 후보를 재정렬한다.
DEV에서 최종 후보 하나와 예산을 고정한 뒤 독립 EVAL을 연다.
[검색 설계](../../adr/NOV-12-001-native-query-and-semantic-engine.md#native-search-design-basis) ·
[홀드아웃 판정](DOD.md#독립-홀드아웃-실행과-판정)

## 레인별 착수와 인계

[활성 작업 목록](../oct-22-finalizing/README.md#활성-작업-문서)과 각 티켓이 현재
배정·인수·검증을 관리한다. 완료된 A/C 공급은
[구현 ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#closed-query-supplier-scope)을
재사용한다. 최초 배정과 임시 대여 기록은
[복구 색인](../../ARCHIVE-INDEX.md#2026-11-17-native-dispatch-retirement-and-six-ticket-intake)에 보존한다.

| 기존 인계 ID | 현재 책임과 완료 기준 |
| --- | --- |
| CLOSE-BOOT-01 | D가 S의 원 Source·Watch·outbox/ACK와 Q receiver/selection을 소비한다. 실제 rollback·restore-forward에서 EOF·fence·typed 거절·선택 fsync와 동일 operation을 검증한다. |
| CLOSE-CALLERS-01 | F가 공급된 SDK 기본 경로와 Query wire를 소비하고 실제 SDK·CLI·API/MCP caller에서 기능·권한·명시 퇴역을 수용한다. |
| CLOSE-BENCH-01 | F가 출하 예정 SDK·daemon과 실제 capture, 독립 qrels·negative·후보 풀 및 판정을 동일 입력에 결속한다. |
| CLOSE-EVIDENCE-01 | D/F가 기존 결과를 source·입력·실행 범위별로 인수하고 영향받은 acceptance만 다시 실행한다. |

해당 writer가 소유 hunk와 private mod/import/test 등록을 함께 맡는다. F는
인수한 공용 parent·daemon 조립·Cargo/lock·suite registry를 결합한다.
실제 생산자·소비자·수명·typed 거절을 확인하고, source_ready 인수의
exact hunk·source SHA·caller·회귀 ID를 대조한다. 입장·실행은
[기존 F 큐](tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)와
[공용 수정 경계](../oct-22-finalizing/OWNERSHIP.md)를 따른다. P11 배포·활성화·
rollback은 등록 action, 승인 Linux 대상과 독립 observer로 수용한다.

## 실행 순서

아래는 전체 Native 단일 엔진 수용 순서다. 이번 Tantivy 작업의 중간 릴리스에는 [Tantivy 우선 실행 순서](TANTIVY-RETIREMENT.md#실행-순서와-담당)를 적용한다.

1. **Freeze functionality and inputs.** Existing producer/caller owners reuse the accepted Query contracts and enumerate actual SDK, CLI, API/MCP and benchmark behavior, permissions, filters and results. F preserves the previous signed Legacy comparison/recovery pair and freezes equivalent inputs, hardware, budgets and independent evaluation rules.
2. **출하 후보 구성.** 완료된 Legacy 엔진·Cargo 정리를 인수하고 A/B/C의 조회·ANN·공개 wire와 D의 daemon·launcher를 같은 후보에 결속한다. Native Semantic의 LanceDB 등록과 실제 release recipe의 의존성·번들을 확인한 뒤 후보를 동결한다. [퇴역 체크리스트](DOD.md#최종-제거-및-증거-체크리스트)
3. **통합 A.** 동결한 Semantica·Quanta 제품 쌍에서 SDK60, 표준 Native Large, 원 E07, LanceDB ANN, 독립 홀드아웃 및 설치 FULL→동일 Native view 조회→재시작 1→DELTA/delete/no-op·ACK 유실/재시도→재시작 2를 판정한다. 모든 필수 결과는 출하 예정 바이너리와 설정에 결속한다. [전환 전 게이트](DOD.md#전환-및-퇴역-판정)
4. **기본 경로 전환.** 통합 A가 통과한 후보로만 진행한다. Legacy 쓰기를 막고 미결 작업을 정산한 뒤 원본 manifest와 event high-water를 고정한다. 두 루트의 source 입력과 Native operation·SourceCut·BOOT를 대조하고 선택 CAS로 SDK·CLI·런처·벤치의 기본 경로를 연다. 권한 있는 쓰기 경로는 전환 전후 각각 하나다. [기존 복구 계약](../../operator/state-cutover-runbook.md#rollback-boundary)
5. **통합 B와 퇴역 완료.** 실제 기본 호출자·재시작·재생·복구를 실행한다. 전환 기준점 이후 게시 증가와 양쪽의 미결·미전달 작업이 0건일 때 이전 쌍으로의 복귀를 검증한다. 기준점 이후 첫 영속 커밋 뒤에는 ACK 수신 여부와 관계없이 restore-forward와 원본 이벤트 재생을 검증한다. 전체 workspace·API/module·exact-source CI와 출하 검사 후 이전 루트는 pin·재생·보존 계약에 따라 정리한다. [E08 통합 B](../oct-22-finalizing/E08-integration-release.md#integration-gates)

Root는 실패한 gate의 최초 원인과 영향 레인만 다시 연다. 수정 후 관련 소스·바이너리·설정이 달라진 판정만 재실행하고, 출하 후보의 필수 결과가 모두 충족되면 알고리즘 비교와 전환 작업을 종료한다. [최종 종료 기준](DOD.md#전환-및-퇴역-판정)

## 현재 증거에서 이어갈 순서

[F's current queue](tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)
owns exact source, admission and current compiler/body results. Public API/module
changes use the actual `just rust-public-api` / `just rust-cargo-modules` output.
Reuse valid scoped results and rerun the affected original gate when relevant
source, binary, config or inputs change. The scope owners above retain their
own product evidence; integration A/switch/B closes only under the original
[DOD](DOD.md#전환-및-퇴역-판정).
