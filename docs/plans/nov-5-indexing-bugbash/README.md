# Native 인덱싱 bugbash

2026-11-07 KST 기준 Native 초기 인덱싱 RCA·후속 구현 SSOT다. 공유 작업 트리에는
다른 담당자의 dirty 변경이 있으며, 아래 검증은 각 frozen source에만 귀속한다. 제품 계약은
[R01–R40 DoD](../oct-22-finalizing/DOD.md)를 따른다.

| 문서 | 역할 |
| --- | --- |
| [RCA](RCA.md) | 실패 원인·실측·인과관계의 한계 |
| [WORKPLAN](WORKPLAN.md) | 현재 계약·코드 소유·검증·채택 조건 |
| 이 README | 현재 판정·다음 실행 |

## 현재 판정

**원 Large 4,096-source / server 120초는 미해결이다.** 이전 제품 기준선은
`0a97e8589daa41b736cb6d59545b0116bb3b89c6`의 r4다. 마지막 제품 실행
소스는 `d8e2c52de9f0e058475aa25e3d746cdad3015672`(외부 `s3`)다.
r5 정상 cached pair build는 완료됐고 Medium은 원 512MiB work cap, Large Off/Record는
원120초 deadline에서 각각 실패했다. typed inventory confirmation fee 후보
`a6083fb22480786d5e8172057b6fbe291af26ee7`의 r6 build는 완료됐으나 Medium은
같은 cap에서 다시 실패했다. B03-B2의 8KiB/512-term 구현은 frozen source에 합류했고
집중 회귀와 제품 검증은 아직 결과 전이다.

| 범위 | 판정 | 실제 결과·한계 |
| --- | --- | --- |
| r4 normal cached harness/daemon build | VERIFIED | Cargo exit0, source/config/dependency/tool 전후 일치. managed-fresh·release qualification은 아님 |
| Catalog registration 영향 테스트 | VERIFIED | 37개 통과·267개 제외. 최종 wire 재인코딩 제거와 literal gold 포함 |
| mr4 Medium fixed 512MiB Record | FAILED | producer12.357초·exit1. used536,865,857B+10,600B에서 거절. full ACK 없음 |
| lo4 Large Off / 원120초 | FAILED | producer145.286초·exit1, BuildPrepare deadline. numeric cap·memory·external resource 거절 없음 |
| lr4 Large Record / 원120초 | FAILED | 요청120.153초·producer146.811초, managed_install packet loop deadline. Postings 미도달 |
| mr4/lr4 닫힌 child 진단 | VERIFIED | 각각56,184/637,289 events, loss·health·open span·lease 오류0. 실패 prefix 관측이며 제품 성공 아님 |
| File read·phase work delta 영향 배치 | VERIFIED | Core/Storage/Native Text741개 통과·602개 제외. Kernel lib check·hexagonal·Core/Kernel module 검사 통과 |
| B03-B1 Native·Kernel 집중 회귀 | VERIFIED | 후보 source `d8e2c52…`: f15 Native292개/558 제외와 기존 lifecycle12개, f17 변경 transition1개/77 제외의 composite local proof. d8e2↔68ea 차이는 Kernel test 파일1개. 정확 commit의 전체 재실행·제품 수용 아님 |
| profile-first 초기 Patch·leaf/block oracle | VERIFIED | f13 11pass/2fail, f15 12pass/1fail의 raw JSON/ref 동일성 가정 제거. public reader TF/positions와 동일 ordered ref mapping 재구성으로 옮긴 f17 해당 case1pass. 기존 pin/GC와 physical map 검증 유지 |
| strict Clippy 전체 | FAILED | f16 `--keep-going` lib/tests 40→33개 다른 owner 진단. Native 소유0. 전체 green 아님 |
| B03-B1 candidate-source03 | VERIFIED | Main HEAD/index 불변인13개 파일 local snapshot, source `d8e2c52…`. focused proof와 실제 제품 실행을 분리 |
| r5 정상 cached pair build | VERIFIED | `candidate-build-03.py` build exit0, source `d8e2c52d…`, scale/daemon SHA 기록. managed-fresh·release 자격은 아님 |
| mr5 Medium fixed512MiB Record | FAILED | 실제 full_event_ack 거절: 536,866,122B 사용 후 14,133B charge 실패, `BuildPrepare`; producer exit1, 외부 timeout 없음. child 39,223 events 완결. full ACK 없음 |
| lo5 Large 원120초 Off·새 성능 효과 | FAILED | 같은 r5 pair로 원 seed42/cap/120초 실행: request124.436초에서 `BuildPrepare` deadline, producer226.899초·exit1. numeric cap·memory·external-resource 거절 없음. full ACK 없음 |
| lr5 Large 원120초 Record | FAILED | 요청120.654초 deadline, producer exit1·full ACK 없음. child trace는858,005 events 기록/4,698 dropped/7 open으로 PARTIAL; 도달 prefix만 분석 |
| 최신 typed confirmation fee 후보 집중 회귀 | VERIFIED | `a6083fb…`의15개 소유 파일 local snapshot. f18 `c49b2a1…` E0716 compile FAILED/body0 뒤 test-only 교정. f19 Native293/293·lifecycle13/13 통과, 각558/65 제외. 제품 결과는 별개 |
| r6 정상 cached pair build | VERIFIED | `a6083fb…` source의 build exit0. managed-fresh·release 자격은 아님 |
| mr6 Medium fixed512MiB Record | FAILED | 실제 full ACK 전 536,864,032B 사용 후 10,692B charge가 Catalog managed_install에서 거절. producer exit1. child 45,923 events 완결, I/O 실패0 |
| a608 Large 원120초 | NOT_RUN | 새 pair의 Large 실행 결과 없음. 이전 d8e lo5/lr5 실패를 새 후보 결과로 승격하지 않음 |
| B03-B2 owner-local 8KiB/512-term block | NOT_RUN | `878be6c9…`의15개 소유 입력 local snapshot(이전 a608 대비6개 변경), 코드 합류 완료. f20 Native/Kernel 집중 배치는 admission 대기·terminal 없음; 제품 r7 미시작 |
| 최신 공유 전체 소스 Large120·정식 overhead·XL·mixed·출시 | NOT_RUN | 외부 frozen source의 집중 결과와 별도 범위 |

r4 CLI는 prebuilt pair의 `build_binding=UNPROVEN`을 유지한다. 첫 Medium controller는
build와 runtime PATH가 달라 producer 실행 전에 거절했다(body0, NOT_RUN). 이후
private tool aliases·실행 파일 epoch를 재검사하고 PATH-only 차이를 선언한 별도 local
runtime cohort에서 mr4/lo4/lr4를 실행했다. matched-build 환경이라고 주장하지 않는다.
Off/Record 도달 작업이 달라 tracing overhead·speedup 비교도 성립하지 않는다.
f11 Tools03의 이전 Record epoch는 producer body0으로 거절됐다. 새 Tools03은15개
path의 `st_dev` 33→30 차이만 확인하고 hash/inode/mtime 등은 같음을 검사해 새
invocation epoch로 고정했다. 가드를 완화하지 않았다. f12는 build 입장 대기 prefix만
남기고 PID/open FD가 없어 NOT_RUN으로 종료됐으며, f13 이후에는 새 epoch를 썼다.

## 확인한 병목과 현재 수정

lr4 LexicalBuild는 wall114.628초 / thread CPU66.001초다. 그 안의
InventoryPrepare45.075초 / CPU24.981초, InventoryMap45.414초 / CPU26.988초,
Diff12.218초 / CPU9.882초는 inclusive 값이므로 합산하지 않는다. SourceApply는
wall5.107초다. 호스트 대기가 공존하지만 전체 실패를 host-only로 설명할 근거는 없다.
수치 work cap·물리 write/fsync 실패 없이 lexical 준비 단계에서 deadline을 소진했다.

File get의 중복 SHA, Catalog registration의 버리는 재인코딩, SourceApply/Text의
phase work delta 누락은 수리·집중 검증을 마쳤다. 후속 구현은 **B03-B1 owner-local
term→positions dictionary block**이며, Native·Kernel 집중 범위는 통과했다.
mr5 Medium에서는 이 경로가 실제 활성화돼 managed reserve 입력이 5,230→1,635개,
inventory occurrence 검증이 3,083→512회로 줄었다. 하지만 Prepare의 accepted logical
bytes는 261,371,848→321,302,214B로 늘어 원512MiB 조건은 여전히 실패했다.
이 수치는 서로 다른 r4/r5 source·host 실행의 단계별 작업 계수이며 속도 개선율이 아니다.
새 후보는 planned inventory의 검증된 Null/Object 필드를 실제 wire 길이로 charge한다.
행당 accepted work 감소 예상은 prior 없는 경우 1,068B, prior Object 경우 972B다.
기존 사전 reserve 절차와 하드 SQLite page cap을 유지한다. 이 수치는 코드 산술이며 제품
성공이나 CPU·wall 개선 실측이 아니다. mr6 trace의 Prepare는 mr5보다54,602,568B
적게 charge돼 정확히 51,126행×1,068B와 맞았다. Postings를 더 진행했지만 여전히
512MiB 한도에서 거절됐다.

- 빈 prior inventory의 Bootstrap 또는 초기 Patch에서 payload≤4,096B / canonical term rows≤128로 묶는다. 초기 Patch 조건은 현재 후보 source에 포함됐다.
- 같은 positions vector는 block 안에서 공유한다. singleton/oversize/변경된 delta term은 leaf다.
- 현재 reader 하나가 current-policy leaf/block을 읽는다. 구 output_policy는 typed refusal이다.
- Block count는 `(ref, canonical term)`으로 선택한다. ref-only scalar RAM/spill memo는 leaf만 사용한다.
- 증분에서 동일 positions의 기존 block ref를 유지한다. 바뀐 term 때문에 다른 term posting을 재작성하지 않는다.
- Build2/query64 bounded DATA window와 원 memory/work/cancel/retained-read 정책을 유지한다.

실제 harness는 step995에서 첫 profile을 만들고 step1059에서 첫 SourceAdvance를
**Patch**로 호출한다. f9까지의 Bootstrap-only 선택은 원 Large full 경로에서 block을
켜지 못한다. 후보의 `pack_owners` 조건을 Bootstrap 또는
`Patch(delta.prior.owners.0.is_none())`로 넓혔다. Sparse의 nonempty prior leaf와
old-ref bound를 유지하며 f17의 집중 case가 통과했다. seed42의
입력8,609,531B는 확인된 값이다. 약33,716 unique leaf2.94MB → 12,288 block23.93MB는
코드 기반 추정이며 측정 결과가 아니다. 객체 수 감소와 cold bytes 증가를 함께 검증한다.

이전의 “cold read bytes가 조금이라도 늘면 block을 폐기”는 이 문서가 추가한 과도한
기준이었다. 원 gate는 metric별 region bounds와 고정 query mix/SLO·메모리/work cap이다.
Cold read amplification은 측정·보고하고 원 gate로 채택 여부를 결정한다. 원120초와
별도 ANN4K1M reads/512MiB·SDK30s/worker10s를 완화하지 않는다.

## 남은 순서와 중복 제외

1. B1 소유 Native·Kernel 집중 회귀는 composite local proof로 닫았다. 정확 source의 전체 재실행이나 제품 자격으로 승격하지 않는다.
2. typed inventory confirmation fee 수리의 f19 집중 검증과 r6 build는 통과했지만
   mr6는 원512MiB에서 실패했다. 비용 제거 자체는 확인됐고 제품 문제는 남았다.
3. B03-B2의 8KiB/512-term owner-local block 구현은 합류했다. f20에서 literal
   codec·64/256행 초과 fanout·실제 저장/query/delta/pin/reopen을 집중 검증하고,
   통과 뒤 같은 원 예산의 r7 제품 실행을 판정한다.
4. lo5/lr5 Large 원120초 Off/Record도 실패했다. `BuildPrepare`의 실제 비용과
   물리 설치 prefix를 확인한다. lr5 partial trace로 overhead·속도 비율을 주장하지
   않는다. l6ctl/l7ctl의 body0 guard 거절은 Large 제품 실패로 세지 않는다.
5. 채택에는 E07 고정 mixed query의 p95/p99·지역 비용·pin/GC 조건도 필요하다.
   단일 query 성공이나 기능 회귀 통과로 이 범위를 완료하지 않는다.

Core/Storage/Catalog/SDK·기존 inventory Block/P1-A와 다른 사용자 채팅의 기능을
재구현하지 않는다. 세부 파일 one-writer와 실패/폐기 조건은 WORKPLAN이 소유한다.

## 과거 근거

51b70b34의 mr2/lo2/lr2, C16/C17와 이전 실행 기록은 [RCA](RCA.md) 및
[이전 VALIDATION](../oct-22-finalizing/VALIDATION.md#e07-indexing-versus-host-rca-20261105)의
역사적 근거다. 과거 `nov03-e07-wal-policy`/`e7n1105` 원본 경로는 현재 부재해 그대로의
replay는 BLOCKED다. 현재 source의 새 측정과 혼동하지 않는다.

기존 [E07](../oct-22-finalizing/E07-performance-operations.md)·WAVES는 이 SSOT의
진입점이며 과거 본문은 [history](../../history/nov-5-indexing-bugbash/)에 보존한다.
새 현재 상태·후속 작업은 이 세 문서에서만 갱신한다.
