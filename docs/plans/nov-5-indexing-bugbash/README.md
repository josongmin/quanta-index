# Native 인덱싱 bugbash

2026-11-12 KST. 이 문서는 Native 초기 인덱싱의 현재 판정과 진입점이다.
실제 제품·성능 판정은 아래 `현재 판정` 절에 source별로 보존한다.
원 R01–R40 합격선은 [Oct-22 DoD](../oct-22-finalizing/DOD.md),
후행 전환 후보의 통합·제품 수용은 [Nov-12 Native 전환](../nov-12-native-cutover/README.md)과
[NC-F](../nov-12-native-cutover/tickets/NC-F-integration.md)가 소유한다.

| 문서 | 권위 |
| --- | --- |
| 이 README의 `현재 판정` | source별 제품 결과, 원 기준, 최신 잔여 |
| [RCA](RCA.md) | 동결 source의 실패 원본·실측·인과관계와 종료된 시도 |
| [WORKPLAN](WORKPLAN.md) | 현재 수용 경로·실행 계약·고유 증거/퇴역 패킷 복구 |

저장된 Native 실행은 capture 폴더 또는 `-control` 폴더를 지정해 조사한다.

```sh
.venv/bin/python -m tools.benchmark.retrieval.causal_cost_diagnostic native-triage --run-dir "$RUN"
```

제품 결과, 호스트 사전 차단, 실패 단계, 작업 거절과 staging 진행을 구분한다.
`--trace`는 daemon 구간·유실 상태, `--json`은 구조화 출력을 추가한다.
[진단 절차](RCA.md#native-run-triage-20261111)

## 수용의 범위

- source별 Large/XL/Mixed 결과는 아래 [현재 판정](#현재-판정) 한 곳에서 유지한다.
  동결 pair의 본문 종료와 실패 원인은 [RCA](RCA.md#native-f2-xl-mixed-terminal-20261112)에 있다.
- 원 XL은 seed42·32,768 source·server dispatch120초의 최초 ACK·검색·재오픈,
  원 Mixed는 50QPS·1Hz·960초·pin600초·settle60초를 요구한다.
  timing/work/memory/disk와 case별 child terminal은 [원 기준](../nov-12-native-cutover/DOD.md)을 따른다.
- 후행 전환 후보는 NC-F에서, SDK60·Large 두 OS 재시작·N5·workspace lint와
  Linux 공급·DELTA는 각각 기존 담당과 입장 조건에서 판정한다.
  [원 구현·수용 계획](WORKPLAN.md#final-remaining-audit-20261111)

## 완료된 소스 작업과 과거 기록

alias handoff·packed Diff·Posting 정규화·packed-run scratch의 상시 계약은
[Native ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#consolidated-implementation-boundaries)에 통합했다.
Session profile·Hybrid·종료 응답은 [Query ADR](../../adr/NOV-12-001-native-query-and-semantic-engine.md)이 소유한다.
당시 회귀·source·중간 pair의 수치와 실패는 [WORKPLAN](WORKPLAN.md)·[RCA](RCA.md)에 남긴다.
후보 구현의 완료는 원 XL/Mixed·SDK60·현재 source 수용이 아니다.
아래 옛 앵커는 이력 링크를 유지하며, 현재 제품 판정은 다음 절만 따른다.

<a id="final-remaining-audit-20261111"></a>
<a id="session-hardening-20261110"></a>
<a id="alias-sql-implementation-20261110"></a>
<a id="remaining-cost-audit-20261110"></a>
<a id="packed-diff-rca-20261110"></a>
<a id="posting-product-rca-20261110"></a>
<a id="posting-value-normalization-20261110"></a>
<a id="xl-mixed-resume-20261110"></a>

<a id="현재-판정"></a>
## 동결 구축 소스의 판정

2026-11-12 SourceApply와 Native Text 본문 검증 합류의 집중 통합 12개는
**VERIFIED**다. 동결 `824629e4`의 Large ACK 27.281458초·검색·재오픈은 **VERIFIED**다.
후속 XL Off·Mixed Off·XL Record는 실제 실행에서 각각 **FAILED/exit1**로 종료했다.
[본문 종료와 실패 분석](RCA.md#native-f2-xl-mixed-terminal-20261112)은 해당 동결 입력에
귀속한다. 후행 SourceApply·Native Text 및 NC-A/B/C/D를 합친 후보의 원 XL/Mixed
수용은 **NOT_RUN**이며 [Native 전환 F](../nov-12-native-cutover/tickets/NC-F-integration.md)가
통합한다. [코드·검증 기록](RCA.md#native-source-body-single-read-20261112)

[11월 10일 감사 게이트](WORKPLAN.md#final-audit-execution-20261110)는 당시 검증
순서와 원 기준을 보존한다. 현재 Linux 공급·DELTA·통합 작업은
[활성 티켓](../oct-22-finalizing/README.md#활성-작업-문서)을 따른다.

| 동결 입력 | Large | XL | Mixed | 판정 한계와 원 기록 |
| --- | --- | --- | --- | --- |
| r4 `0a97e858` | Off·Record `FAILED`, BuildPrepare 120초 | — | — | Medium `mr4` fixed 512MiB `FAILED`; [원 RCA](RCA.md#최신-r4-고정본-정상-빌드와-세-실패-실행) |
| r5–r9 | Large 첫 ACK `FAILED` | — | — | Medium fixed 512MiB도 각 입력에서 `FAILED`. 단계별 byte/child 수와 build/compile 경계는 [RCA](RCA.md#r5-후보-medium의-비용-이동과-남은-거절)·[WORKPLAN](WORKPLAN.md#b03-b3--inventory-block-child-dictionary)에 보존 |
| r10–r31 | 소스별 `FAILED`와 이후 국소 성공 | — | Mixed 별도 | B5–B21의 코드·회귀·당시 제품 결과는 [WORKPLAN](WORKPLAN.md)·[RCA](RCA.md)의 각 packet으로 귀속; 뒤의 성공을 앞선 source에 전이하지 않음 |
| r32 `58d69e24` | Off 46.419초, Record 48.192초 `VERIFIED` | — | — | B22 QIMAP005와 B25 v19 marker는 별도 계약. [원 결과](RCA.md#b22-large-실측) |
| r40–r50 | 실행된 로컬 Large는 source별 `VERIFIED` | 실행된 XL은 원 120초 `FAILED` | 실행된 Mixed는 부하·pin·정산 `FAILED` | 단계별 원인은 [RCA](RCA.md#r49-mixed의-정산-실패와-검색-처리율)에 source별로 기록 |
| r51 `1482d857` | — | Off·Record `FAILED`, BuildPrepare 120초 | `lm12` 600.475초 pin 해제 뒤 61회 poll에도 Pending, `FAILED` | [XL](RCA.md#r51-xl-off의-재실패) · [Mixed](RCA.md#r51-mixed의-custody-진행과-정산-미수용) |
| r55 `b7ffae26` | — | — | `FAILED`, held search 555ms IPC timeout; Catalog 대기/COMMIT 관측 | pin 600초·정산 미검증. [원 RCA](RCA.md#r55-mixed의-catalog-대기와-반복-commit) |
| r56 `2275203` | Off ACK 33.689초·검색·재오픈 `VERIFIED` | Off·Record `FAILED`, BuildPrepare 120초 | `lm16` `FAILED`, 검색 3,995/4,022 실패·pin 121.212초 조기 해제 | 제품 `execution.json`은 성공; 외부 `p56ctl` exit2는 부분 진단 분류 오류. capture `PARTIAL_DIAGNOSTIC`, binding `UNPROVEN`, 정식 성능 `NOT_RUN`. [원 RCA](RCA.md#r56-동결-pair-실행과-현재-코드-경계) |
| r57 `813cb26` | Off ACK 45.700초·검색·재오픈 `VERIFIED` | `xr57` BuildPrepare 120초 `FAILED` | `lm17` `FAILED`, 검색 11,050/11,067 실패·pin 334.568초 조기 해제 | 로컬 pair build exit0, child와 전체 판정 분리. 최신 D 변경은 포함하지 않음. [실행/한계](RCA.md#r56-동결-pair-실행과-현재-코드-경계) |
| F1/F2 `824629e4` | ACK 27.281458초·검색·재오픈 `VERIFIED` | Off·Record 각 body1 `FAILED/exit1` | Off body1 `FAILED/exit1` | 2026-11-12 08:14 KST 종료. XL ACK 미완료, Mixed held-read 정산·원 부하 미달. 후행 NC 후보 판정은 별도. [원 영수증 분석](RCA.md#native-f2-xl-mixed-terminal-20261112) |

<a id="native-large-funding-boundary"></a>
**원인 경계:** Medium fixed 1M reads/512MiB는 진단 profile이다. E07 Large는
seed42·4,096 source·원 server dispatch 120초에 별도 explicit
67,108,864 reads/writes/mutations·549,755,813,888B logical funding과
full ACK·search·reopen을 요구한다. XL은 32,768 source와 같은 120초다.
Mixed는 50 QPS·1Hz·960초·pin 600초·renew 120초/max gap 180초·
settle 60초의 원 부하와 정산 기준을 유지한다.
ANN4K의 1M reads/512MiB, SDK 30초·worker 10초도 독립 조건이다.
논리 cap은 RSS·disk 상한이 아니며, Medium의 cap 거절을 Large 제품 실패의
선행조건이나 P0 증명으로 쓰지 않는다. tracing Off/Record의 도달 작업이
다르면 overhead·speedup 비교를 하지 않는다.

**코드 합류와 검증:** A–E의 caller·export·테스트 등록과 B async WorkUsage,
E prepared run·union receipt·`AttemptObjectReads` forwarding을 대조했다.
정적 미연결은 추가로 확인하지 못했다. v8c/d/e/h의 고유 Rust 144건과
Python profile 396/396은 소스별 집중 결과다. `2275203` scratch strict
Clippy는 9개 진단으로 `FAILED`; scoped Core/Kernel module 검사는 통과했다.
후속 `813cb26`은 lib 61/61·Python profile 402/402·hexagonal이
`VERIFIED`다. 전체 workspace 또는 XL/Mixed 제품 수용으로 전용하지 않는다.
[소유·검증 경계](WORKPLAN.md) · [E 코드 감사](RCA.md#e-source-audit-20261111)

<a id="native-cli-search-reopen-20261112"></a>
**CLI·관측기 로컬 검증:** r57 prebuilt와 harness의 recipe/input-domain
불일치를 검사 완화 없이 현행 daemon 빌드로 해소했다. 고정 사본으로 Small
20회 갱신·검색·재오픈과 RSS sampler 수명, 3-file CLI의 FileMap·Literal·Text
검색·재오픈, stdout/disk JSON·manifest 일치를 확인했다. 이 결과를 원
XL/Mixed·Linux·전체 release 수용으로 승격하지 않는다.
[명령과 입력 경계](WORKPLAN.md#d-verification-closeout-20261110)

## 구현·진단 이력의 압축 인덱스

B1–B82의 source별 구현·회귀·제품 실패는 [WORKPLAN](WORKPLAN.md)과 [RCA](RCA.md)에서 회수한다.
B25의 구 marker/Map 형식은 당시 이력이며 현행 형식으로 재사용하지 않는다.
B35의 선택적 cold ASCII scratch 후보는 [철회 기록](WORKPLAN.md#b03-b35--cold-ascii-owner의-토큰-scratch-후보-철회)이며 미구현 작업이 아니다.
중복 요약의 원문은 [복구 색인](../../ARCHIVE-INDEX.md#2026-11-13-ownership-and-ticket-consolidation)에 보존했다.

### 다른 원장에 없는 실행 ID의 원문 행

The 103 unique execution-ID/status rows are indexed in the
[recovery entry](../../ARCHIVE-INDEX.md#2026-11-16-bugbash-evidence-index-compaction).
Their complete original result/count/limitation cells and source identities are
retained in the exact README preimage. FAILED, body0, partial aggregate and
NOT_RUN/BLOCKED remain distinct; later local fixes do not rewrite old outcomes.
This index creates no new code, execution or current-product qualification.

QITXOC01의 현행 빈 prior Bootstrap/초기 Patch 조건은 payload≤8,192B와
canonical term rows≤512를 유지한다. 같은 positions vector는 block 안에서
공유하고 singleton/oversize/변경 delta term은 leaf다. reader는 leaf/block과
QITXEM02를 읽으며 B1/B2의 구 output policy는 typed refusal이다.
Build2/query64 DATA window와 원 memory/work/cancel/retained-read 정책을
유지한다. 객체 수 감소와 cold bytes 증가를 함께 측정하며, cold read bytes의
증가만으로 폐기하지 않는다. 원 gate는 metric별 region bounds·query mix/SLO·
memory/work cap이다. [계약과 원본](WORKPLAN.md#b03-b3--inventory-block-child-dictionary)

<a id="확인한-병목과-현재-수정"></a>
## 과거 비용 진단의 귀속

lr4·mr5/mr6·r57의 비용과 수리는 [동결 RCA](RCA.md#최신-r4-고정본-정상-빌드와-세-실패-실행)에 있다.
부모/하위 wall·CPU·실패-prefix 계수는 중복 합산하거나 속도 향상률로 해석하지 않는다.
Posting batch별 COMMIT 가설과 관측기 wall/표본 한계는 [원 진단](RCA.md#additional-static-audit-20261110),
완료된 Q3의 실제 회귀는 [증거 참조](WORKPLAN.md#causal-measurement-packet-20261110)를 따른다.
현재 후보의 지배 비용·Native Text 수리는 NC-TEXT/NC-F가 실제 source에서 판정한다.

## 남은 순서와 중복 제외

| 순서 | 실제 잔여 | 경계 |
| --- | --- | --- |
| 1 | 후행 Native 후보의 XL 32,768 source 원 120초 ACK·검색·재오픈 | r56/r57와 `824629e4` 실패는 각 입력에 유지. 새 후보에서 정확한 지배 비용을 계측하고 [NC-F](../nov-12-native-cutover/tickets/NC-F-integration.md)가 결합 |
| 2 | Mixed 50 QPS·1Hz·960초·pin600초·settle60초 | 이전 조기 검색·정산 실패를 source별로 유지. queue/IPC, Catalog COMMIT·reader, old-view witness와 원 부하를 분리 |
| 3 | SDK60·Large 두 OS 재시작·N5 및 정식 성능/출시 수용 | 원 제품·정식 운영 수용은 별도. 현재 owner와 입장·소스 결속을 따른다. [Oct-22](../oct-22-finalizing/README.md) |

B75·B76·B78·B79·B81·B82 및 Posting page 순회와
`ConfirmedOccurrenceRecord` 설치 전/후 경계는 기존 구현이다.
`RootPackingPlan`, Storage empty-stream·multi-root, cold inventory cursor,
ASCII token·SDK cap과 다른 owner의 dirty File authority/Posting page는
중복 구현하지 않는다. 소스 변경만으로 검증 완료나 실행 중 여부를 추정하지
않는다. 현재 root/format과 binary는 원 producer·migration 계약으로 결속한다.
옛 v18/v19 후보는 현재 rebuild 지시가 아니다. 세부 실행 계약은 [WORKPLAN](WORKPLAN.md),
현행 writer와 정확한 hunk는 Native 활성 티켓을 따른다.

## 과거 근거

<a id="원본-가용성"></a>
51b70b34의 mr2/lo2/lr2, C16/C17과 이전 실행은 [RCA](RCA.md) 및
[이전 VALIDATION](../oct-22-finalizing/VALIDATION.md#e07-indexing-versus-host-rca-20261105)의
역사 근거다. `nov03-e07-wal-policy`/`e7n1105` 원본 경로가 현재 없어
동일 replay는 `BLOCKED`; 현재 source의 새 측정과 혼동하지 않는다.
구 E07·WAVES 원문은 [history](../../history/nov-5-indexing-bugbash/)에 있다.
후행 수용은 [Native 전환](../nov-12-native-cutover/README.md)이 소유한다.

### 종료된 F1/F2 예약 기록

<a id="native-final-executor-20261111"></a>
<a id="f1-pair-동결과-첫-f2-입장"></a>
<a id="native-final-executor-retry2-20261111"></a>
<a id="f2-900초-입장-재시도"></a>
<a id="native-final-executor-7200-20261111"></a>
<a id="f2-7200초-예약의-preflight-차단"></a>

| 동결824629e4의 당시 입장 | 원 기록 |
| --- | --- |
| F1 pair 결속 exit0·첫 F2 wait0 lock busy/body0 | [원본](RCA.md#native-final-executor-20261111) |
| 단일900초 EX 예약 만료·해제, 제품0 | [원본](RCA.md#native-final-executor-retry2-20261111) |
| EX/project 입장 뒤 timing600초 load22.57959/8.0 거절·제품0·예약 반환 | [원본](RCA.md#native-final-executor-7200-20261111) |
