# Finalizing — 웨이브 실행 계획

상태·담당·다음 조건은 [README의 8개 에픽](README.md#epics), 실행 결과는 [VALIDATION](VALIDATION.md), 독점 파일/함수는 [OWNERSHIP](OWNERSHIP.md)만 갱신한다. 이 문서는 실행 순서와 입출력 계약이다. 새 티켓·상태 원장·담당을 만들지 않는다. 전체 종료 분모는 [E01–E08 / R01–R40](DOD.md#final-dod) 그대로다.

E02의 최신 코드·검증 상태는 [에픽 원장](README.md#e02)에 있다.
이전 배정·회귀 수·실행 식별자는 [복구 색인](../../ARCHIVE-INDEX.md#plan-and-ticket-deduplication)에서 찾는다.

## Wave map

| 웨이브 | 작업과 산출물 | 병렬 범위 / 합류 | 종료 조건 |
| --- | --- | --- | --- |
| W0 — 잔여·수정권 확정 | 최신 caller·dirty hunk·기존 실행 결과를 대조해 실제 미완료만 추림 | Root가 공용 접합을 소유. 기존 owner는 자신의 경계만 확인 | 실제 결손마다 단일 writer와 기존 oracle 확정. 완료 코드·동일 실행 재발급0 |
| W1 — 독립 코드 마감 | 실제 Source/query 공급, SDK Native 소비·provider admission·unchanged typed artifact 공급, 확인된 Runtime/PUB 실패 custody, SEARCH/lexical·PHYSICAL·IDE/BOOT 수리 | 아래 독점 lane을 병렬 진행. 공급 내부는 공통 ABI 확정 후 소비자/재취득을 분리. 공용 parent·feature/export는 Root 또는 기존 SDK parent owner가 결합 | 정적 경계/서명·원인/partial/drop 계약 충족. 실제 typed 공급과 W2 연결점 제출. helper 존재만으로 공급 완료 처리0 |
| W2 — 실제 게시·소비 연결 / CP1 | initial receipt/authority → pending/V9 → terminal Native generation/root → 기존 CAS/ACK → SDK/AST/BOOT | FULL/delta 입력 준비는 분리. 공용 writer parent는 Root, SDK parent는 PUB 한 writer | 성공과 거절 모두 실제 caller→consumer 연결. ordinary 우회·guard 재진입·합성 authority 없음 |
| W3 — 영향 compile·owner 회귀 / CP2 | 합류 코드의 Runtime·SDK/CP·Q owning 검증 | 기존 요청 재사용. 같은 host heavy 작업은 admission 아래 직렬 | 컴파일과 실제 선택 본문 각각 통과. 실패 원인 수리 뒤 동일 rail 재검증 |
| W4 — 제품·복구 / CP3 | 공개 FULL/delta/delete/query, 5 durable cut, 원4K, IDE/BOOT candidate | 각 기능의 영향 CP2와 입력이 준비되면 실행. SEARCH 원4K·독립 IDE/BOOT gate는 E02의 미완료를 전역 대기 조건으로 삼지 않음 | cold gold·same-operation 복구·오류/취소/환급·실제 consumer 수용. 독립 결과는 W6에서 합류 |
| W5 — 성능·운영 / CP4 | 초기/증분, Large/XL, mixed, Off/Record, 압박·유지보수, 실제 IntelliJ 비교 | 해당 기능을 수용한 고정 source/profile별 진행. 공유 host 성능 부하는 직렬 | 품질 AND latency/처리율 AND 총자원/유지보수 한도 충족 |
| W6 — 통합 A → 전환 → B | pre-switch A → 기존 default 전환 → post-switch B/full-suite/CI/release | 기존 E06/E08 owner가 앞 결과를 소비 | 모든 필수 DoD 수용. 필수 FAILED/BLOCKED/NOT_RUN0 |

**계획된 코드 마감은 W1/W2, 전체 종료는 W6다.** 아래 도식은 영향 기능별 의존 경로이며 전역 실행 barrier가 아니다. W3–W5에서 확인한 결함은 원 owner가 수리하고 영향 gate를 다시 통과해야 한다. SEARCH 제품 검증은 무관한 Runtime 접합이나 전체 Semantica campaign 완료를 기다리지 않는다. 설치/provider/client 입력 준비도 W0부터 병렬이며 입력 부재는 해당 gate만 BLOCKED다.

## Next execution batch

- **E01.** 기존 Source/query → planner → writer와 실제 receipt/artifact를 [E01 계약](E01-producer-generation.md) 및 [W205 owning 범위](VALIDATION.md#w205-proof-20261030)로 수용한다. 실제 관리 입력·독립 cold gold와 늦은 실패·취소·환급을 확인한다.
- **E02.** 아래 [E02 합류 순서](#e02-shared-join-order)를 사용한다. 실제 DELTA 공급과 각 owning 및 설치 회귀는 [현재 E02](README.md#e02)에서 판정한다.
- **E03–E08.** Lexical·원4K·IDE/Auto·SDK/BOOT·성능·운영은 해당 입력이 준비된 경계에서 진행한다. [에픽 원장](README.md#epics)의 유효한 결과를 W6가 소비한다.

## E07 Native 인덱싱 RCA 전환

현재 판단과 신규 패킷은 [bugbash SSOT](../nov-5-indexing-bugbash/README.md),
[구현·검증 계획](../nov-5-indexing-bugbash/WORKPLAN.md)이 소유한다.
이전 C/R/F 패킷과 실행 순서는 [보존 본문](../../history/nov-5-indexing-bugbash/E07-WAVES.md)에 있다.
기존 구현·과거 요청을 새 잔여 작업으로 재발급하지 않는다.

<a id="e07-closeout-plan-20261030"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-closeout-plan-20261030)

<a id="e07-remaining-rca-plan-20261031"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-remaining-rca-plan-20261031)

<a id="e07-residual-packets-20261102"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-residual-packets-20261102)

<a id="e07-wal-policy-execution-20261103"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-wal-policy-execution-20261103)

<a id="e07-entry-map-rca-plan-20261103"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-entry-map-rca-plan-20261103)

<a id="e07-entry-map-contract-parallel-tests-20261103"></a>
[이전 계획 이력](../../history/nov-5-indexing-bugbash/E07-WAVES.md#e07-entry-map-contract-parallel-tests-20261103)

## W1 — 독점 병렬 묶음

[OWNERSHIP](OWNERSHIP.md#parallel-implementation-schedule)에서 함수별 단일 작성자를 확인한다.
기존 Root·PUB·SEARCH·PHYSICAL·AST/BOOT/Auto의 공용 parent와 private 구현을 분리한다.
실제 caller 연결, typed 실패·partial·최종 drop, 독립 oracle과 실행 범위를 반환한다.

## E02 shared join order

<a id="e02-residual-wave-plan"></a>
<a id="e02-supplier-subwaves"></a>
<a id="e02-next-parallel-batch"></a>

1. **W0/W1 — 공급과 공용 합류.** 현재 Source/Full/Solver의 실제 모듈 closure와 producer→consumer를 대조한다. 공유 Source hunk와 Runtime/SDK parent는 기존 단일 writer가 합류한다. 기존 Native 조회·memo·Trace/Store/binding 및 GC 계약은 [E02 생산 경계](E02-publication-recovery.md#production-join-contracts)를 따른다. Provider는 [현재 owned Host와 별도 수용](E02-publication-recovery.md#configured-native-provider-status-20261030-c)을 사용한다.
2. **W2.0 — complete successor.** unchanged checked owner + dirty checked output − delete를 동일 transition/target에 합성한다. History·Chunk·Graph/CallGraph·RepoMap의 complete closure와 target-bound export는 기존 E01 공급을 소비한다. no-op·delete·rename·recreate와 unchanged sibling을 독립 cold FULL gold와 비교한다.
3. **W2.1 — authority·member·V5.** initial authority·metadata를 한 번 이동하고 Required member → target V5 → 최고 Source/Output 종료 → final check → prior exact V9를 검증한다. 조기 거절은 member I/O 전, 늦은 거절은 successor V9와 visibility 전이다. 기존 abort·partial·typed cause를 보존한다.
4. **W2.2 — V9·CAS·ACK·restart.** 기존 coordinator에서 동일 operation/attempt/target과 원 cap으로 begin/reconcile/claim/dispatch를 수용한다. Unbound Dispatching은 Pending이고 bound ACK loss는 exact replay를 사용한다. 중복 visibility/CAS와 재stage 없이 committed-source 소비·cleanup·최종 환급을 확인한다.
5. **D owning → installed.** 현재 [D 단일 큐와 SDK witness](VALIDATION.md#e02-peer-ownership-refresh-20261106)를 사용한다. 기존 RQR-E 설치 owner가 적격 Linux image·expected SHA256·daemon을 공급한 뒤 [다섯 durable cut](E02-publication-recovery.md#five-durable-cuts)과 genuine DELTA 복구를 기존 oracle로 수용한다.

E02의 [CP1–CP3 계약](E02-publication-recovery.md#e02-checkpoint-acceptance)과
[현재 E02](README.md#e02)가 각 범위의 상태를 소유한다.
RepoMap의 전체 map 순회·해시·직렬화는 W5 비용에 포함한다.
독립 E01 initial Source/Graph commit과 E03–E07 gate는 해당 입력으로 진행한다.

## W3 — CP2 영향 compile·owner 회귀

- Q는 Justfile/`./scripts/cargow`, S Runtime은 기존 owning QBC, SDK/CP·TSJS는 기존 등록 요청을 재사용한다. 같은 host heavy 실행은 한 큐다.
- 기존 E02 요청에 더해 실제 변경된 DELTA caller·member·최종 검사·ACK/restart의 영향 본문을 수용한다. [기존 결과](VALIDATION.md)는 해당 source에 결속한다. Host/Linux는 각 플랫폼의 적격 Source·feature·실행 결과를 기록한다.
- canonical wire/digest·역순 manifest·duplicate/missing/unknown/foreign/trailing JSON·before-write/RPC 거절·stale owner/epoch/revision CAS를 독립 expected로 검증한다.
- 성공/거절/cancel/panic/unwind·late Source·payload-before-backing·마지막 drop 환급과 Vector artifact/undo를 실제 본문으로 확인한다.
- compile0, 선택 본문 수, 본문 결과를 분리한다. module filter는 lib(test) 전체 컴파일을 줄이지 않는다. canonical 53본문의 feature별 28/25를 구분하고 실행하지 않은 본문을 성공으로 세지 않는다.
- 새 실패는 현재 source/원인과 대조해 원 owner에 되돌린다. feature 축소·쉬운 selector·assertion 제거·private API의 무근거 확장으로 우회하지 않는다.

## W4 — CP3 제품·복구

- 실제 FULL/delta/delete/rename/recreate/no-op/Resync·전체 affected domain·RepoMap/lexical/semantic/public query를 cold-target gold/P2I01–08로 검증한다. Q native와 Semantica 연동 결과를 구분한다.
- [Facts2 + first-FULL3의 5 durable cut](E02-publication-recovery.md#process-proof-routing)을 같은 operation/target/policy로 수용한다. helper(None)·재수집·새 key·budget reset으로 대체하지 않는다. discovery는 per-operation Original을 발급하지 않고 unbound Dispatching은 Pending을 유지한다.
- E02는 위5 cut과 별도로 [genuine first-FULL→DELTA 복구](E02-publication-recovery.md#e02-checkpoint-acceptance)를 수용한다. 실제 DELTA의 persist/claim/V9/ACK 유실·재시작을 current source와 signed SQL worker provenance에 결속한다. first-FULL 양성이나 owner-unit GREEN만으로 DELTA process 수용을 대체하지 않는다.
- SEARCH는 [원4K](E04-semantic-ann.md#original-4k-gate)의 aggregate completion → Q64/filter recall → reopen/delta/path/overlay를 원 cap/deadline으로 검증한다. 원인/수리/입력 변경 없는 실패 재실행은 하지 않는다.
- actual editor overlay/save/좌표/action·Fast/Deep/Auto와 BOOT candidate verify/serve/reopen·predecessor rollback을 수용한다. 이 단계는 candidate/opt-in이다.
- installed searchd·SQL worker 절대 경로와 신뢰된 expected SHA/provenance, 승인된 Linux target·실제 provider/client가 필요한 gate는 그 입력을 먼저 확인한다. Docker 존재나 자체 계산 hash는 설치 authority가 아니다.

## W5 — CP4 성능·운영·실제 IDE 비교

- 측정 전에 source/dependency/binary·hardware/profile·corpus·독립 품질 gold·cap/latency/표본 기준을 고정한다. ANN 원4K·Large/XL 기능·Linux2CPU/6GiB 운영·native/IDE100K profile은 구분한다.
- full bootstrap·고정 delta/no-op·N/H/M/history/fanout·1/100/1000파일 batch·지속 churn을 기존 harness로 측정한다. CPU/bytes/objects/IO/provider·maintenance debt·actual total RSS/scratch/physical peak를 포함한다.
- [E07 운영 계약](E07-performance-operations.md#fixed-profiles-and-oracles)의 Large/XL ACK/search/reopen·원960초 mixed·long pin/pressure/fault/shutdown·직렬 교대 Off/Record≥3쌍을 수용한다.
- [IntelliJ 수용](E08-integration-release.md#actual-intellij-acceptance)은 같은 workflow·언어/capability·품질·cache/hardware 비교와 RFC absolute gate를 함께 요구한다. fixture는 실제 editor 비교를 대신하지 못한다.
- bulk/sort/spill·group commit·bounded compaction·재사용은 관측된 비용 원인의 기존 seam에서만 실험한다. budget 확대·실패 표본 제외·유지보수 이연으로 합격시키지 않는다. hint 선택 O(M + 2VM) 등 남은 비용도 실제 작업량에 포함한다.

## W6 — 통합 A→default 전환→B

1. A에서 E01–E07 필수 기능·5 cut·원4K·성능/운영·IDE/IntelliJ·설치 authority 결과를 대조한다. 유효한 기존 결과를 통합 명목으로 전부 재실행하지 않는다.
2. A 수용 후 기존 E06 daemon/CLI/launcher default를 전환한다. predecessor·rollback·단일 Catalog head를 유지한다.
3. B에서 post-switch smoke·rollback·cleanup·current coherent full-suite/strict/public API/module·exact-source hosted CI/release를 수용한다.
4. [R01–R40](DOD.md#requirements)·P2I/UX/AUTO·필수 에픽 DoD를 모두 대조한다. 조건부 N/A만 원 계약의 근거로 허용한다. CODE_QUALIFIED·DEPLOYED·ACTIVATED·ROLLBACK_PROVEN은 구분한다.

## Checkpoint and retry rules

- 착수 직전 최신 exact path/function·실제 authority/caller·dirty/claim·기존 oracle를 확인한다. 같은 hunk는 한 writer, 공용 parent는 지정 통합자만 수정한다. 무관한 repo 청결이나 다른 campaign 완료를 기다리지 않는다.
- 코드 배치가 정적으로 합류되면 그 영향 compile/owner 회귀를 한 번 수행한다. 제품·성능은 source와 필수 입력이 준비된 checkpoint에 모은다. checkpoint에서 결함이 나오면 해당 절단만 수리한다.
- 새로운 진행률 원장·중복 queue·one-off repo evidence 파일을 만들지 않는다. source/input/profile이 달라진 결과를 섞어 성공으로 올리지 않는다.


E01의 실제 SDK/precommit 실패·수리·source별 실행 결과는 [R76 SDK](VALIDATION.md#r76-installed-sdk-pta)·[R77 Source read](VALIDATION.md#r77-precommit-source-read)에만 기록한다. 이 문서에 Source/claim/통과 수를 복제하지 않는다.


<a id="e02-l1-proof-order-20261030"></a>
<a id="e02-l1-closeout-order-20261030"></a>
## E02 DELTA L1 — final integration to owning proof

[L1 소유권](OWNERSHIP.md#e02-l1-owner-boundary-20261030)과
[현재 증거·잔여](VALIDATION.md#e02-l1-d-closeout-20261031)를 사용한다.

1. D가 A/B/C의 현재 postimage와 공용 SDK parent·fixture·consumer ABI를 대조한다.
2. 기존 QBC lane에 root/inner manifest, lock, feature, 실제 supplier가 닫힌 cohort를 입장시킨다.
3. 기존 D 큐에서 owning compile과 선택 본문을 실행한다. 적격 설치 입력이 준비되면 genuine SDK·독립 process를 별도로 수용한다.
