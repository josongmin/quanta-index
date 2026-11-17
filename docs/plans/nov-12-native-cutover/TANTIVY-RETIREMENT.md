# Tantivy 퇴역과 Native 전환 실행 계획

이번 Tantivy 제거 후보의 종료 조건은 격리된 설치 후보의 기본 호출자가 Native로 동작하고, 현재 저장소의 코드·전체 Cargo 그래프·lockfile·vendor·후보 번들에서 Tantivy 계열이 0건인 상태다. 실제 기본 선택 활성화는 전체 통합 A 이후의 E06에서 수행한다. Lance와 자체 ANN은 기존 담당과 [전체 Native 단일 엔진 수용](DOD.md#최종-제거-및-증거-체크리스트)을 따른다.

**단계 경계:** 이 문서의 T 단계는 Tantivy 제거 후보의 독립 수용이다. T의 설치 기본 호출 리허설은 격리된 후보 선택 안에서 실행한다. 전체 Native 통합 A/B는 Tantivy와 Lance 모두 0건이라는 기존 기준을 유지한다. T 완료를 전체 Native 출하 승인이나 전역 E06 활성화로 사용하지 않는다. Lance 오너의 작업은 별도이며 Tantivy 코드 제거를 기다리게 하는 선행 조건으로 추가하지 않는다. [T 단계 판정](DOD.md#tantivy-우선-퇴역-t-단계)

기록된 Native-only release와 로컬 SDK·Large 실행은 각 후보의 증거다. 전체 workspace·lock·vendor·출하 번들 퇴역은 아직 별도 수용이다. 원 SDK의 후보·첫 거절·수리 범위는 [NC-TEXT](tickets/NC-TEXT-build-final.md#현재-원-sdk-진단과-다음-수리-경계), 통합 판정은 [NC-F](tickets/NC-F-integration.md#2026-11-15-잔여-코드-재감사)가 소유한다. 이 계획에 이전 회차의 실패 수치나 후보 상태를 현재 상태로 복제하지 않는다.

## 기능 우선 최종 감사와 갭

현재 원문 패턴 검토는 설계 선택의 근거다. 실제 전환에서 먼저 닫을 것은 아래 유지 기능과 게시·복구 경계다. 독립 hunk의 구현·계측·평가 입력 준비는 병렬 진행할 수 있지만, 최적화 수치로 미완료 기능을 대신 수용하지 않는다.

| 우선순위·갭 | 현재 있는 것과 빠진 것 | 최소 기능 종료 조건·기존 오너 |
| --- | --- | --- |
| 1. Native History 실제 게시·조회 — 생산자·서버·caller 연결 | 원본·ACK 보존, 복수 세대 FULL과 후보 재시도, Q checkpoint export와 S의 최초 seal 전 인계, 활성 Source/History DELTA가 코드에 연결됐다. 서명 설치·재시작, Q commit과 S V9 terminal 사이 복구, 여러 Legacy 활성 revision의 이전은 [H 원장](tickets/NC-H-history.md#current-actionable-backlog--2026-11-16-re-audit)이 관리한다. | 기존 History/생산자/C·실제 caller 오너와 F가 같은 source cut·입력 carrier를 단일 member/Catalog CAS에 연결해 검증한다. 작은 FULL→recency/relevance·필터·페이지→DELTA→이전 view→reopen을 기존 oracle로 대조한다. 기존 전체 데이터 이전은 실제 source authority의 완전성으로 별도 판정하며 target에서 기대값을 만들지 않는다. |
| 2. Source·첫 ACK·설치 복구 — 생산 연결과 실행 | 초기 ordinary Source binding·Initial/Watch ancestry·cold graph/DFG·normal fixture의 현재 구현을 재사용한다. S에 반영된 D1–D3와 source-only Main·compiler/body·signed normal/postpublication 수용을 분리한다. 과거 Source7 미실행이나 API 미공급 목록을 새 구현으로 재발급하지 않는다. | D의 기존 Source/Store·producer worker와 F가 실제 Source→ACK→Q head를 독립 대조한다. 기존 Q txn/lease의 bounded Published census와 기존 Source intent/completion getter·단일 typed decoder를 재사용한다. 격리된 signed 후보에서 forward→rollback→restore-forward·ACK 유실·재시작을 실행한다. 새 Source issuer/store·AST head의 lexical 대용·delivery와 operation의 1:1 가정은 도입하지 않는다. |
| 3. 원 SDK60 게시 | 고정 후보의 Caddy Record FULL·검색·재오픈 및 동일 daemon Large가 통과했다. Caddy quiet Off는 본문 0/2이며 etcd Record는 Work bytes에서 거절됐다. | etcd는 토큰 mutation 하한도 100만을 넘는다. 기존 Text 오너가 고정 입력·한도를 충족할 생산 경로를 수리한다. 실제 Record·Off·Large와 최종 후보의 영향 재검증은 [Text 원장](tickets/NC-TEXT-build-final.md#admission-decision-and-remaining-product-verification)을 따른다. |
| 4. Hybrid 최종 판정 — 설치 신원·필수 지표·독립 입력 | native_file_runtime의 선택적 Linux peer/config/policy 결속, native_file_decision과 기존 decision CLI의 실제 family replay·MRR/Recall·식별자·무정답·표본/저장소·latency 판정 코드는 있다. quality_admit/reject는 replay 범위이며 runtime_qualified·product_default_decision은 false다. 실제 Linux SDK 독립 capture·설치/운영·blind EVAL은 별도 수용이다. | F3/Root는 현재 F-CODE-1과 실제 SDK 입력을 소비해 기존 판정기를 실행한다. 새 adapter·집계기를 중복 구현하지 않는다. 동일 universe의 독립 qrels와 원 품질 기준을 유지하고 fusion 변경은 DEV 손실 근거가 있을 때만 판단한다. |
| 5. Tantivy 제거 후보 — 잔여 조립/의존성 코드 | ordinary SDK/CLI가 Native를 기본 선택한다. Runtime과 workspace 기본 Cargo 그래프의 Tantivy는 0개다. 명시적 daemon Legacy 조립, dep/patch·lock·vendor가 남아 있다. 유지 기능을 먼저 Native로 이관해야 한다. | F가 해당 기능 오너의 인수를 소비해 코드·전체 그래프·lock·vendor·후보 번들 Tantivy 0건을 만든다. 같은 최종 후보에서 유지 호출·SDK60·Large·품질·설치 복구를 검증한다. Lance는 별도 기존 Semantic 범위다. |

직접 확인한 호출 근거: [Native transport의 Legacy 요청 거절](../../../crates/quanta-index-sdk/src/client.rs#L53) · [일반 CLI의 Native 분기](../../../crates/quanta-index-searchctl/src/lib.rs#L83) · [Native CLI History](../../../crates/quanta-index-searchctl/src/native/history.rs#L16) · [기존 History SDK dispatch](../../../crates/quanta-index-sdk/src/history.rs#L1461) · [추가된 History 요청](../../../crates/quanta-index-contract/src/native_protocol/read_session.rs#L197) · [현재 서버 dispatch](../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport.rs#L90) · [Native History 입력 검증](../../../crates/quanta-index-search-plane/src/native_history.rs#L272) · [family 재생 CLI](../../../tools/benchmark/retrieval/evaluator.py#L4491).

**이미 닫힌 범위의 재사용:** 기존 C74의 canonical file-source binding·SDK/Runtime 소비자와 C77의 Published metadata endpoint는 해당 입력의 기존 완료 결과를 소비한다. 같은 문제를 새 Query 레인으로 발급하지 않는다. History readback·기존 alias/단일-block zero-copy·Catalog 원자 전환·Large helper도 재작성하지 않는다. 현재 미완료 History 기능과 기존 C 닫힌 endpoint는 별도 hunk다.

**최소 변경 원칙:** 현재 순서는 유지 기능 연결/인수 → 같은 출력과 실패 계약의 수정 → 원 SDK/설치 실행 → 최종 품질·퇴역 판정이다. Caddy의 기존 batch 재사용은 좁은 수정으로 검증한다. etcd의 SPIMI형 내부 표현, borrowed multiblock, Hybrid의 CC는 해당 비용·손실이 확인된 경우만 적용한다. 새 영속 포맷·공용 IR·추가 엔진·추가 모델을 이 감사의 기본 작업으로 만들지 않는다. 기존 DOD 수치와 운영 게이트는 유지한다.

**History 원본과 checkpoint:** 원 발행·ACK가 남은 세대는 기존 replay를 사용한다. 유실 세대에는 같은 WAL snapshot·writer fence와 실제 replay boundary를 인증한 checkpoint가 필요하다. [History 구현·수용 계약](../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover)

**History integration:** The [History ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#native-history-publication-and-read-contract) holds the producer replay, catalog and candidate contracts. H/producer/caller own the remaining migration, and F consumes the shared integration. Keep the original single CAS/root, profile, DATA lifetime, filters, scoring, cursor and EOF/completeness oracles.

**Source·복구:** ordinary Source·ACK의 signed loan, Q join, cold graph/DFG, normal cycle·ACK 유실 fixture를 재사용한다. [D 잔여 패킷](tickets/NC-D-boot.md#d-current-disposition-20261116)이 영향 owning과 동일 서명 입력의 설치 본문을 검증한다. 원 생산자의 ACK 및 Source custody는 기존 계약을 따른다.

**패턴과 수용의 차이:** 아래 Lucene/RocksDB의 내구화·원자 게시와 SPIMI의 bounded posting 구성은 설계 근거다. 현재 기능 연결·SDK60·설치 복구·독립 EVAL이 끝나기 전에는 이 후보를 SOTA 성능이나 퇴역 완료로 판정하지 않는다. WAND·새 ANN·새 fusion·추가 엔진은 이번 기능 종료의 선행 조건이 아니다.

## 브레이킹 변경과 유지할 계약

- 기본 바이너리에는 Native 런타임만 조립한다. 설치 런처는 검증한 선택의 설정·digest를 사용해 `serve-native`를 명시한다. 인자 없는 수동 기동과 예전 `serve --state-root`는 명시 오류로 종료한다. Legacy IPC/SDK, History continuation, 저장 포맷은 새 릴리스에서 끊을 수 있다. 새 포맷과 호환 경계를 명시하고 이전 루트는 이전 릴리스가 보유한다. 새 루트는 authoritative source와 replay에서 재구축한다. 구 버전 요청과 루트는 쓰기 전에 거절하며 자동 Legacy fallback은 제거한다. [현재 데몬 파서](../../../crates/quanta-index-searchd/src/cli/command.rs#L48)
- **유지 확정** 범위는 실제 사용 중인 Semantica History와 검색·발행 제품 흐름이다. History는 기존 commit/ref/tag/diff 생산자를 사용해 Native의 같은 Catalog view에 게시한다. 동일 workspace/view에서 recency·relevance, 필터, 페이지 순서를 검증하고 새 커서는 view·query·order에 결속한다. 기존 이벤트 중복·재정렬·삭제, 고정 view의 다음 페이지, 재시작 및 다시 읽기까지 독립 oracle로 확인한다. 구 포맷 호환을 위해 두 writer를 띄우지 않는다. [현재 요청 계약](../../../crates/quanta-index-contract/src/query/requests.rs#L638) · [기존 History 검색 oracle](../../../crates/quanta-index-lexical/src/history_text_index/tests.rs#L146)
- **History 이관 입력.** [History ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover)의 원본 replay와 checkpoint export를 사용한다. 보존된 History 행·ACK·high-water를 결속하고, 이동·삭제된 ref/tag와 발행 시점 metadata를 독립 oracle로 대조한다. signed FULL·successor DELTA와 여러 활성 head의 이관은 [H](tickets/NC-H-history.md#current-actionable-backlog--2026-11-16-re-audit)가 수용한다.
- **History 검색은 행 의미를 유지한다.** commit과 diff는 같은 Native view 안에서도 서로 다른 BM25 통계를 쓴다. author·ref·time·diff 필터는 순위 창을 자르기 전에 적용하고, 커서 뒤 전체 `matched`를 정확히 계산한다. 고정 소수점 점수·committer time·SHA·path 동점 순서와 기존 BM25 oracle을 재사용한다. 상위 후보가 전부 필터에서 떨어져도 뒤의 적격 행이 페이지에 나오는 회귀를 둔다. [History 랭킹·필터 계약](../../../crates/quanta-index-core/src/domains/lexical/history_text.rs#L182)
- 기존 Semantica 호출·통합 오너는 구현된 C 공급 계약을 재사용하고 확정 후보 전에 실제 호출별 `유지` 또는 `퇴역`을 기록한다. 범위는 ordinary SDK/CLI, Semantica API/MCP와 벤치의 lexical·semantic 검색, Symbol, RuntimeMetadata, Structural, seed/cluster/explain, RepoMap, diagnostics 및 구 DTO다. History·Structural의 게시 경로도 확인한다. `유지`는 새 SDK 경로와 독립 기능 oracle을, `퇴역`은 실제 호출 제거·공개 버전 경계·거절 회귀를 요구한다. 기존 필수 호출의 `UnsupportedNativeQuery`만으로 통과할 수 없다. 분류 없이 남은 설치 호출이 하나라도 있으면 전환을 진행하지 않는다. [실제 호출 목록](../../adr/NOV-12-001-native-query-and-semantic-engine.md#closed-query-supplier-scope)

## 실행 순서와 담당

| 순서 | 담당과 작업 | 완료 조건 |
| --- | --- | --- |
| 1. 이전 릴리스와 비교 입력 고정 | F가 마지막 호환 Linux **서명 Q/S 설치 쌍 전체**를 저장소 밖의 지속 경로에 보존한다. 데몬·Semantica resident/생산자·CLI·SQL worker·런처, rulepack/model 정책·설정·선택과 복귀 루트, SDK 벤치 도구와 소스 식별자를 함께 고정한다. Caddy·etcd 원 입력과 홀드아웃도 고정한다. | 보존한 묶음 자체로 재기동·게시·조회 및 복귀 연습을 실행한다. 유지할 호출의 결과·순위·필터·페이지·오류·권한 oracle을 고정한다. 과거 임시 디렉터리나 경로가 살아 있다는 가정으로 완료 처리하지 않는다. |
| 2. History 및 실제 호출 이전 | H는 기존 Native History 게시·조회와 원본 이관을 검증한다. F와 기존 Semantica/SDK/CLI 오너는 남은 실제 호출을 이전한다. Query wire와 cursor는 [완료 ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md#closed-query-supplier-scope)을 사용한다. | 유지·퇴역 목록의 실제 본문 결과, 기존 입력의 유지 회귀와 구버전 거절을 확인한다. |
| 3. 설치·복구와 교차 저장소 검증 이전 | D와 F가 기존 cold graph, 최초 Legacy ACK·게시 후 coverage, resident→Q helper, RepoMap·launcher·serve와 설치 회귀를 인수한다. 교차 저장소 proof 오너는 기존 실행 경계를 이전한다. | 실제 Linux의 같은 서명 Q/S 후보에서 FULL·검색·정상 복구 cycle·DELTA/delete/no-op·ACK 유실·재시도·게시 후 replay·두 재시작을 검증한다. [단일 D 실행 목록](tickets/NC-D-boot.md#current-work-packets-20261115) · [교차 저장소 실행기](../../../scripts/verify-repomap-cross-repo.sh#L58) |
| 4. 삭제가 반영된 최종 후보 자격 검증 | F가 아래 전체 삭제를 적용한 **미승격 릴리스 후보**를 만든다. 기존 main 작업을 소비하고 격리가 필요한 검증에만 단일 임시 체크아웃을 사용한다. Native 기본 진입점·벤치 연결·Cargo/CI·API/wire/module 변경을 후보에 포함한다. | 후보의 서명 Linux 번들과 Q/S 바이너리, profile·config·source·SDK 벤치를 해시로 고정한다. 그래프 0건, 설치·복구 본문, 성능 표의 필수 기준이 같은 출하 후보에서 모두 **VERIFIED**다. |
| 5. 검증한 후보 승격과 기본 전환 | D의 설치 경로에서 새 루트를 선택 CAS로 활성화한다. F가 검증한 소스 정리와 같은 바이너리·설정을 기본 배포에 승격한다. 활성화 뒤 실제 기본 호출을 다시 점검한다. | 일반 SDK·CLI·런처·Semantica와 벤치가 Native를 선택한다. 승격 중 바이너리·설정 변경이 있으면 영향 검증을 먼저 재실행한다. |

2번과 3번의 소스 작업은 독립적으로 진행한다. 3번의 설치 본문과 4번 성능 본문은 최종 바이너리가 나온 뒤 같은 후보에 대해 닫는다. 같은 파일의 공용 mount·Cargo/lock은 F가 한 번 합친다. 무거운 검증은 기존 단일 실행 큐를 사용한다. B14·F1/F2의 기록된 source별 결과는 원 범위로 보존하고, [P07의 현재 수용 절단](tickets/NC-F-integration.md#p07-final-actions-20261115)을 재사용한다. 별도 검증 체크아웃이 필요하면 Root가 하나만 만들고 사용 직후 제거한다. [수정권과 실행권](README.md#레인별-착수와-인계)

F가 먼저 확정할 인계 입력은 이전 릴리스의 실제 Linux 실행 파일과 비교 호출자, 재현 가능한 Caddy·etcd manifest, C의 실제 호출 목록, D의 서명 설치 묶음과 생산자 복구 슬롯이다. 각 인계에 오너·경로·대상 소스·expected body를 적고, 이미 진행 중인 같은 입력을 재사용한다. 성능 실행은 Caddy·etcd 첫 Off 본문과 필수 History/설치 본문부터 확인한다. 여기서 제품 실패가 나면 나머지 반복을 예약하기 전에 해당 경로를 수리한다.

Current independent inputs, their original recovery records, qrels and required strata are owned by [F](tickets/NC-F-integration.md#원본-sdk60-입력-복구와-실행-검증). Preserve the actual Caddy/etcd manifests and file hashes, DEV/EVAL separation, source/release commitments and signed Q/S recovery inputs. Earlier small holdout backups or source-only rosters do not supply current labels, capture or product acceptance.

### 코드 착수 순서

[활성 작업 목록](../oct-22-finalizing/README.md#활성-작업-문서)의 담당이 현재 source와
남은 수용을 관리한다. 기존 writer는 완료된 producer·caller·recovery 연결과 원
독립 oracle를 재사용한다. History의 checkpoint 인계, 복수 세대 FULL, 활성
DELTA·commit 직후 재시도의 설치 검증과 여러 Legacy 활성 revision의 이전은 [H 원장](tickets/NC-H-history.md#current-actionable-backlog--2026-11-16-re-audit)이
관리한다. D는 같은 요청의 recovery와 두 차례 재시작을, Text는 원 SDK60을 검증한다.

F는 기존 Large 바이너리 결속과 Native quality 판정을 사용하고, Mixed의 서명된
release daemon과 caller receipt 결속 및 독립 SDK 평가를 수용한다. 설치·벤치·
퇴역 합격선은 아래와 [DoD](DOD.md)에 있다.
[배포 바이너리 결속](../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md#native-large-executable-binding)

## 구조 감사와 최종 실행 계획 — 2026-11-15

Text의 단일 구현·증거 원장은 [NC-TEXT](tickets/NC-TEXT-build-final.md)다.
아래 기존 창 배치 통합과 Work 계측을 새 최초 구현으로 발급하지 않는다.
패턴 레퍼런스와 당시 설계 원문은 [복구 색인](../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation)에서 회수한다.

### 채택 근거와 적용 순서

[Text ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody)의 기존 token cursor·packed occurrence·child-first 설치와 Catalog CAS를 사용한다.
기존 reserve/confirm과 게시 등 내구성 경계를 지키고, 후속 DAG/prepared-run과 토큰 표현 변경은 측정과 독립 oracle로 결정한다.

### 1. Caddy — 기존 자식 우선 배치에 leaf와 directory를 함께 설치

적격 cold window의 leaf+directory 설치와 입장 전 separate 경로는 구현됐다.
실패 정산·입장 거절과 COMMIT 비교의 원 실행은 NC-TEXT에 보존한다.
SDK60·전체 GC·Direct 수용은 원 입력으로 판정한다.
[구현 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody) ·
[NTX1a 본문 결과](tickets/NC-TEXT-build-final.md#ntx1a-runtime3-실제-green)

### 2. etcd — 누적 Work를 줄이는 생산 경로 확정

Work/shape 계측, single-fill, compact row와 packet 경계 검증을 재사용한다.
같은 원본 etcd에서 첫 거절은 posting Work bytes이고, 실제 토큰 census
1,009,664개는 고정 mutation 한도 100만을 독립적으로 초과한다.
생산 경로의 두 한도를 함께 검증하고 원 수용 기준을 적용한다.
[Text 판정과 남은 검증](tickets/NC-TEXT-build-final.md#admission-decision-and-remaining-product-verification)

### 3. Hybrid — 후보 손실·순위 손실과 판정 연결 분리

- **기존 판정 코드 재사용:** native_file_profile/native_file_runtime과 native_file_decision/decision CLI는 실제 source/release/config/policy·family replay와 필수 품질/표본/latency gate를 연결한다. runtime_qualified·product_default_decision은 계속 false며 controlled capture 재생도 설치/운영 수용이 아니다. [F 현재 구현·한계](tickets/NC-F-integration.md#이-채팅의-잔여-코드-범위--2026-11-15)를 따르고 실제 Linux SDK 독립 입력·960초 운영/설치 결과를 원 기준으로 수용한다. 다른 C5 보고서 relabel·플래그 변경으로 승격하지 않는다.
- **알고리즘 변경 조건:** 현재 RRF `k=60`과 lane depth를 기준선으로 고정한다. DEV에서 양성의 lexical/dense/union Recall과 oracle 순위를 비교한다. union 밖의 양성은 기존 reader/ANN 오너에게 입력·적격 조건·깊이의 재현으로 넘긴다. union 안에서만 밀리는 손실이 재현되면 같은 후보·latency budget의 RRF와 normalized convex combination 한 종류를 통제 비교한다. 정규화·missing-lane 점수·가중치 grid/선택 규칙·동점·negative 임계값을 실험 전에 정하고 DEV에서만 선택한다. 이 비교가 없으면 fusion을 바꾸지 않는다. 채택한 정책 하나를 EVAL 전에 동결하고 원 producer/consumer를 같이 이전한다.
- **최종 데이터:** 저장소/fork/query-family가 DEV와 분리된 최소 100개 답변 가능 질의·12개 저장소, 층별·negative·identifier 표본과 blind top10 qrels를 기존 DOD대로 확보한다. 동일 file universe/view/적격 조건/lexical depth의 실제 SDK capture를 짝짓는다. 표본 수는 파일럿 분산에 따라 더 커질 수 있다.
- **판정:** MRR@10 +0.02 및 저장소 쌍별 95% 하한 >0, Recall 하한 ≥−0.01, 식별자 MRR ≥−0.01, 무정답 오탐 차이 ≤+0.01을 유지한다. timeout·부분 결과를 제외하지 않는다. 홀드아웃 미달은 해당 후보의 실패이며, 같은 홀드아웃으로 조정 후 재승격하지 않는다. 승리를 구현 일정의 보장 결과로 쓰지 않는다. [현재 미자격 replay](../../../tools/benchmark/retrieval/native_file_evidence.py#L773) · [독립 평가 기준](DOD.md#독립-홀드아웃-실행과-판정)

### 4. 공용 합류·Tantivy 퇴역과 최종 실행

| 순서 | 실행 및 인수 조건 |
| --- | --- |
| 독립 준비 | 구현된 Text 설치·Work 계측·F 판정 경로를 재사용한다. 측정으로 확인한 실제 잔여만 정확한 함수 hunk에서 인수한다. 같은 parent는 기존 writer가 합친다. B/ANN·History·D/설치와 F의 P07은 각 활성 패킷의 조건을 따른다. |
| 좁은 검증 | 변경된 Core/Storage/Text의 독립 oracle와 소유 범위 회귀를 실행한다. 소스·입력·바이너리가 일치하는 기존 회귀와 Large 결과를 재사용한다. Rust 실행은 F의 기존 큐를 사용한다. |
| 최종 T 후보 조립 | 유지할 실제 호출·History 이전을 소비한 뒤 F가 Legacy Tantivy 코드·feature·공용 dep/patch·lock·vendor·bundle를 제거한다. 전체 workspace와 실제 release 그래프 모두 Tantivy 계열 0건이어야 한다. 격리된 설치 후보의 Native 기본 호출과 명시 거절을 구분한다. Lance 잔여는 별도로 보고한다. |
| 원 한도 SDK60 | 조립한 같은 후보에서 Caddy·etcd 각각 Off 2회+Record 1회를 실행한다. **파일 30,000·Work 512MiB·읽기/쓰기/변경 각 100만·SDK I/O 및 submit→ACK 60초**를 유지한다. Full ACK·검색·재오픈·source digest·Record 완전성을 모두 확인한다. diagnostic만 가능한 호스트에서는 성능 통과로 판정하지 않는다. |
| 회귀·품질·설치 인수 | 최종 후보에서 표준 Large의 두 OS 재시작, 새 독립 Hybrid 평가, 기존 D의 실제 설치 게시·조회·재시작·복구를 검증한다. 이전 릴리스 및 삭제 전 Native와 동등 기능의 5쌍 성능 회귀, Mixed/ANN과 전체 workspace/API/module/CI의 기존 필수 범위를 닫는다. 최종 후보와 무관한 소스 변경 때문에 유효 증거를 폐기하지 않는다. |
| 승격 | T 단계는 Tantivy 제거 후보의 수용으로 판정한다. 전체 Native A/B는 Lance 0건까지 포함한 기존 전체 기준을 별도로 충족한 뒤 판정하며, 전역 기본 선택의 E06 활성화는 통합 A 이후다. 각 범위에서 실제 `FAILED`·`BLOCKED`·`NOT_RUN`을 표시하고 코드 완료·로컬 검증·운영 출하를 합쳐 부르지 않는다. |

현재 최초 구현은 기존 staging/설치 계약과 persisted wire를 재사용한다. 후속 측정이 계약 변경의 필요성을 입증하면 source-level breaking change를 허용하고 producer·consumer·회귀를 함께 이전한다. 저장 포맷·wire·게시/GC 계약 변경은 별도로 명시한다. 새 worktree는 만들지 않는다.

### 설치 복구의 정확한 경계

실제 생산자 admission을 닫고 미결·미전달 작업을 정산한 뒤 데몬을 종료한다. 저장소별 신원·source high-water·replay floor와 Native BOOT의 operation·SourceCut을 비교한다. 생산자 fence와 deployment lock은 선택 CAS와 디렉터리 fsync까지 유지한다. 전환 후 Native 추가 영속 게시가 0건인 경우에만 보존한 이전 Q/S 릴리스·루트로 rollback하고 양쪽 프로세스를 기동해 기존 SDK를 검증한다. 첫 추가 영속 게시 후에는 ACK가 유실돼도 rollback을 거절하고 새 Native 루트에 restore-forward 및 replay를 실행한다. stale/foreign/증거 누락, 중단과 재시도에서 잘못된 head 게시·중복 논리 발행·원본 유실은 각각 0건이다. 실제 signed producer를 붙이는 양성 실행과 복구 거절 회귀를 둘 다 확인한다. [설치 복구 계약](../../operator/state-cutover-runbook.md#installed-offline-recovery)

F와 D는 기존 P11 deployment·activation·rollback 등록을 사용한다. 현재 `staged`인 action·독립 observer·승인 Linux 설치 타깃을 연결해야 정식 실행이 가능하다. Semantica Linux 설치 recipe의 x86-64와 E07 운영 사양의 ARM64는 증명 범위를 나눈다. 같은 소스·feature·policy의 릴리스에서 대상별 바이너리 해시를 별도로 고정하고, 각 호스트에서 그 대상의 바이너리로 해당 본문을 실행한다. 같은 ELF 해시 요구는 같은 대상 안에서 적용한다. [P11 등록](../../../tools/ci/proof-authority.toml#L530) · [D의 현재 설치·복구 패킷](tickets/NC-D-boot.md#current-work-packets-20261115)

## 벤치 실행과 합격선

비교 기준은 동일 기능을 수행하는 **이전 호환 릴리스**, 퇴역 변경의 회귀 기준은 같은 기능을 갖춘 **삭제 전 Native 후보**다. 두 기준을 각각 계산한다. Linux 하드웨어·캐시 조건·콘텐츠/모델·요청·필터·결과 단위·후보 깊이를 고정하고, 새 루트에서 baseline/candidate를 교대로 각 5회 실행한다. 시작 순서는 쌍마다 `AB`, `BA`로 바꾼다. 같은 기능에서 submit→ACK와 회차별 조회 p95의 짝비 중앙값은 각각 `candidate / baseline ≤ 1.05`다. 다섯 쌍의 원값과 실패·기한 위반 건수를 함께 보고한다. 고정 절대 기준도 모든 필수 구간에서 만족한다. 두 경로의 기능 계약이 맞지 않으면 유지하는 공통 기능을 비교하고 새 Native 기능은 절대 기준과 독립 oracle로 검증한다. [기존 합격선](DOD.md#이번-이전의-추가-승격-기준)

| 벤치 | 실행 경로 | 합격선 |
| --- | --- | --- |
| Caddy·etcd SDK60 | 기존 `native_corpus`에 고정 source root/manifest SHA/commit, 질의 경로·토큰, 출하 `--native-daemon-bin`, 회차별 새 `--output-root`를 전달한다. 각 입력 cold Off 2회·Record 1회가 원 반복 게이트이며, 회귀 비교는 동등한 Off 5쌍이다. 동일한 앞 두 Off 회차는 두 기준에서 함께 사용한다. 단발 검색 시간으로 조회 p95를 만들지 않는다. | Caddy 590파일·etcd 1,421파일, **30,000파일·512MiB Work·읽기/쓰기/변경 각 100만**. SDK I/O 제한과 submit→FULL ACK **60초**를 모두 만족한다. Full ACK, 검색, 재오픈, source digest 일치. Record 추적 누락 0. [실제 호출자](../../../crates/quanta-index-searchd-harness/src/bin/native_corpus.rs#L27) |
| 표준 Large와 운영 Mixed | Large는 기존 `native_product_process`의 `native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks` 본문을 사용한다. Mixed는 F1/F2의 `causal_cost_capture --backend index-native --native-mixed-load` 및 원 검증기를 사용한다. 출하본 결속은 아래 선행 작업을 적용한다. [Large 본문](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/large_restart.rs#L385) · [Mixed 실행기](../../../tools/benchmark/retrieval/causal_cost_capture.py#L1235) | Large 재시작 fixture는 4,096파일·`LARGE_SEED=5864059738136528177`·기존 funding/300초 준비·서버 120초다. OS 프로세스 두 번 재시작 후 소스·owner·저장소별/전역 순위가 일치해야 한다. seed 42는 별도 F1/F2 규모 실행에서 사용한다. Mixed 전용 Linux ARM64 2 CPU/6GiB/local SSD에서 60초 warmup·900초 50QPS·1Hz 변경, 조회 p95/p99 **100/500ms**, freshness **1/5초**, RSS/스크래치/물리 **2/4/64GiB**. [원 운영 기준](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles) |
| History와 lexical 품질 유지 | 기존 Semantica History 게시·검색 계약을 고정해 commit·diff·필터·정렬·페이지·재시작의 독립 기대값을 확인한다. 양쪽에서 동등한 공개 SDK History 요청으로 조회 p95를 비교하고, 유지할 lexical 케이스는 기존 DSL 및 E07 gold를 사용한다. [History 독립 순서](../../../crates/quanta-index-core/src/domains/lexical/history_text.rs#L208) | 필수 기능과 gold 일치. 변경된 수치화 점수는 독립 oracle로 확인하고 유지할 정렬·동점 규칙과 페이지 누락/중복 0건을 증명한다. 성능은 같은 기능의 5회 비교 기준을 적용한다. |
| 독립 Hybrid 대 Native lexical | 고정 blind qrels·negative·source universe로 native-file-profile capture와 evaluator family/cohort replay를 실행하고 기존 decision.py의 native-file-cohort 판정을 소비한다. 실제 Linux peer/config/policy와 입력 commitment가 필요하다. quality replay와 runtime/설치/제품 기본값 판정을 구분한다. | 최소 100개 답변 가능 질의·12개 저장소, 지정 층별 표본 충족. 파일 MRR@10 **+0.02 이상**, 저장소 단위 쌍별 95% 하한 **>0**. Recall@10 하한 **≥−0.01**, 식별자 MRR 차이 **≥−0.01**, 무정답 오탐 차이 **≤+0.01**. 경로명·저장소명 보정 금지. |

### 출하 바이너리와 벤치 연결의 선행 작업

| Boundary | Implemented source or actual remaining work |
| --- | --- |
| 표준 Large 출하 바이너리 입력 | Implemented: the existing helper consumes the release path/SHA pair, verifies each start and final bytes, and has mismatch/replacement regressions. Reuse the [executable-binding contract](../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md#native-large-executable-binding) and [F source-scoped results](tickets/NC-F-integration.md#tantivy-퇴역-코드-착수). Actual signed shipping-candidate execution retains its separate gate. |
| Mixed 관리 빌드는 `bench`만 받는다. 기존 사전 빌드 입력은 증명 수준이 진단이다. | 기존 `causal_cost_capture`의 입력 검증·동일 실행 검증을 확장한다. Q 데몬은 canonical `release` 빌드와 서명 설치 번들의 같은 SHA로, 벤치 호출 도구는 자신의 고정 빌드 receipt로 검증한다. build/profile/설치 SHA 불일치, 실행 중 산출물 교체 또는 서명 없는 출하 입력은 거절한다. 기존 bench-profile 측정은 원 E07 범위에서 사용한다. 출하 판정은 동일 조건으로 실제 release 데몬을 실행한다. [관리 빌드](../../../tools/benchmark/retrieval/causal_cost_build.py) |
| Native replay·품질 판정의 기존 공급 | Reuse the [quality/controlled-capture contract](../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md#native-quality-and-controlled-capture). Actual independent SDK/Linux peer, labels, signed release and runtime/installed evidence remain required; existing runtime/product flags stay false. |

출하 디렉터리 외부에 실행 매니페스트·회차 종료 결과와 필요한 비교 입력을 보존한다. 준비, submit→ACK, 검색, 재오픈을 따로 기록한다. 타임아웃·거절·부분 응답은 실패·기한 위반으로 센다. 환경 조건 위반으로 무효인 회차만 재실행한다. 기준본의 제품 실패로 시간 비율을 계산할 수 없으면 그 회차를 무한 반복하거나 성공만 골라 비교하지 않는다. 실패를 보고하고 해당 비교의 결과를 보류한다. SDK60이 Work 한도에서 막히면 최초 실패 배치와 단계별 논리 Work·파일/RSS를 확인한다. Caddy posting object batch와 etcd inventory map batch부터 확인하고, 원 한도에서 원인을 수정한다. [마지막 실제 진단](tickets/NC-F-integration.md#f--original-sdk60-and-native-buildprepare-budget--2026-11-14)

### SDK60 단일 회차 실행

기존 `build-test.lock`을 잡은 뒤 해당 호스트의 quiet-host 사전 검사와 실제 SDK를 같은 자원 실행권에서 호출한다. `cache_root`는 `./scripts/quanta-index-env.sh`가 출력한 경로다. 나머지 변수는 고정 실행 매니페스트의 절대 경로와 값으로 채운다. `trace_mode`는 `off` 또는 `record`다. `state_tmp`는 지정 local SSD의 짧은 절대 경로에 만든 회차별 빈 디렉터리다. 실제 mount/device 및 Unix socket 경로 여유를 사전에 확인해 기록한다. `run_dir`는 새 결과 경로이며 `preflight_receipt`는 그 밖의 기존 부모 디렉터리에 둔다. Native·외부 Legacy baseline·삭제 전 Native의 상태 디스크 조건을 맞춘다. [실제 상태 경로 생성](../../../crates/quanta-index-searchd-harness/src/scale/native_index/corpus.rs#L724)

```sh
python3 tools/ci/resource_admission.py \
  --lock "$cache_root/resource-admission/build-test.lock" \
  --wait-seconds 300 --timeout-seconds 900 -- \
  sh -c 'unset QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS; export TMPDIR="$1"; python3 tools/ci/timing/check_host_contention.py --expected-os "$2" --receipt "$3" --run-id "$4" && shift 4 && exec "$@"' \
  sh "$state_tmp" "$expected_os" "$preflight_receipt" "$run_id" "$native_corpus" \
  --source-root "$source_root" --manifest "$manifest" --manifest-sha256 "$manifest_sha" \
  --repository-commit "$source_commit" --query-path "$query_path" --query-token "$query_token" \
  --native-daemon-bin "$native_daemon" --output-root "$run_dir" --trace-mode "$trace_mode"
```

`native_corpus`는 Native 전용이다. 이전 호환 릴리스는 그 릴리스의 기존 Legacy SDK 비교 호출자로 실행한다. 최종 삭제 전에 양쪽 호출자의 실행 파일·hash·명령을 외부 비교 매니페스트에서 검증한다.

SDK 종료값 0과 `result.json.status` 외에도 파일 수·manifest·실행 파일 SHA, `submit_to_published_ms ≤ 60000`, 게시·재오픈의 view 및 질의 witness를 각각 확인한다. `native_corpus`는 게시 후와 재오픈 후 exact view의 전체 FileMap을 입력 파일에 대조하고 같은 SourceCut의 각 owner를 staged ref에 대조한다. 전체 inventory readback은 검색·ACK 표본을 기록한 뒤 실행한다. Record는 `python -m tools.benchmark.retrieval.causal_cost_diagnostic native-corpus-record --run-dir "$run_dir"`로 재생한다. 첫 데몬과 재오픈 trace의 SHA-256을 실행 결과와 대조하고 정상 종료, 서로 다른 capture ID, drop/unwritten/open span 0을 확인한다. [SDK inventory 검증](../../../crates/quanta-index-searchd-harness/src/scale/native_index/corpus.rs#L643) · [Record 재생](../../../tools/benchmark/retrieval/causal_cost_profile.py#L1063)

## 삭제 범위와 최종 확인

출하 후보의 실제 Native ANN 파일 소비와 전체 의존성을 함께 확인한다.
SDK의 모드 이름이나 Legacy trace의 NativePublish 대신 실제 검색·History·복구
경로와 동일 바이너리의 전이 의존성을 대조한다. 전환에는 같은 SourceCut과
원 producer의 high-water·미결·전달/ACK가 필요하다. 첫 Native 영속 커밋 뒤에는
restore-forward를 적용한다. 실패·거절·타임아웃·미판정 파일을 원 평가 분모에
유지하고 공개 N3/N4/N5는 DEV에 사용한다. [복구 계약](../../operator/state-cutover-runbook.md#rollback-boundary) ·
[원 평가 기준](DOD.md#독립-홀드아웃-실행과-판정)

- Legacy Tantivy lexical·History writer/reader, `tantivy-fst` scope matcher, `legacy-tantivy` 기능과 동작 이관이 끝난 Legacy `serve` 조립을 제거한다. Native 복구가 호출하는 기존 물리 포맷 해시·seal 검증기는 Tantivy를 의존하지 않으므로 보존한다. [복구 소비자](../../../crates/quanta-index-searchd-runtime/src/native_boot.rs#L895)
- Legacy 빌드 프로필 및 이 엔진이 필요한 harness·scan experiment·테스트를 Native 경로로 이전하거나, 역사적 성능 비교를 위해 고정한 외부 릴리스에서 실행한다. CI/test authority, DSL proof·wire inventory, `deny.toml`을 실제 소비자와 같이 갱신한다. [테스트 등록](../../../tools/ci/test-authority.toml#L994) · [wire inventory](../../../tools/ci/inventory/wire-surface.toml#L461)
- `Cargo.toml`의 workspace dep·patch·exclude, `Cargo.lock`의 해당 패키지와 `vendor/tantivy-0.22.1`을 삭제한다. 최종 `cargo tree --workspace --all-features --target all --edges normal,build,dev`와 각 배포 대상의 실제 release 그래프에 아래 패키지 차단 검사를 건다. `cargo metadata --locked --all-features`의 패키지 이름, lockfile, vendor 및 출하 번들도 **0건**이어야 한다. 회귀를 막는 금지 정책과 과거 검증 기록은 유지한다. [현재 workspace 정의](../../../Cargo.toml#L179)
- 최종 제거 커밋에서 canonical release·workspace tests/API/module/CI 및 기본 설치 경로를 검증한다. 그래프·번들·설치 기록은 같은 최종 바이너리 해시를 가리켜야 한다. 삭제 뒤 바이너리나 실행 설정이 바뀌면 해당 성능·설치 본문을 다시 실행한다. [최종 수용](DOD.md#전환-및-퇴역-판정)

최종 workspace 소스 검사는 아래 명령을 사용한다. 전체 플랫폼의 normal/build/dev 의존성, metadata, lockfile, workspace 선언과 vendor를 검사한다. Legacy 기능이 남아 있는 후보는 실패한다. 출하 대상별 release 그래프와 서명 번들은 실제 설치 후보 자격 검증에서 확인한다.

```bash
just rust-check-tantivy-retirement
```

실제 release의 compiler artifact를 검사할 때는 기존 설치 검증기가 확정한 Q 데몬 SHA-256을 함께 넘긴다. 이 명령은 SDK60 release의 원 컴파일 기록과 보존 바이너리를 읽고, 해당 SHA와 설치 검증에서 확인한 SHA를 대조한다.

```bash
uv run --frozen --offline python tools/ci/check-tantivy-retirement.py \
  --release-build "$release_evidence_dir" --expected-daemon-sha256 "$installed_q_sha256"
```
