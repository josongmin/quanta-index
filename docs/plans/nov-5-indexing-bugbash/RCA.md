# Native 초기 인덱싱 구조적 RCA

현행 상태는 [README](README.md), 구현 순서는 [WORKPLAN](WORKPLAN.md)이 소유한다.
분석 기준은 저장된 C16/C17 기록, `51b70b34`·`86e6130e` 이력,
r4 제품 고정본 `0a97e858`과 B1 제품 후보 `d8e2c52d`다. 후자는 외부 local
snapshot source이며 r5 Medium 제품 body는 work cap, lo5/lr5 Large Off/Record는
원120초 deadline에서 각각 실패했다. typed-fee 후보 `a6083fb2`는 r6 build 통과 뒤
mr6 Medium에서 다시 원512MiB cap으로 실패했다. 다음 B03-B2는 제품 미실행이다.
과거 성능 숫자는 기존 VALIDATION에서 가져왔으며 해당 과거 외부 원본은 부재다.
아래 mr2와 r4 실행은 각각 source와 바이너리 전후 일치를 확인한 별도 cohort이며 과거 replay가 아니다.
[현재 가용성](README.md#과거-근거)은 별도로 기록한다.

## B03-B1 후보 자체의 검증 결함

이 절은 r4의 원 Large120 실패 원인과 별도다. B1의 공개 zero-reuse 설정이 block
cache에도 적용되도록 수리했고, mutable main의 helper 리팩터 중 컴파일 문제 이후
12개 파일의 외부 clean source에서만 영향 검증하도록 바꿨다.

`cc6b1e1`/f5는 Native109pass/1fail에서 멈췄다(182개 fail-fast 미실행,558개 제외).
원인은 decoder row key의 end offset이 뒤의2B vector selector까지 포함해 실제 존재하는
term을 찾지 못한 것이다. 기대값을 바꾸지 않고 key를 읽은 직후의 end를 저장하도록
수리했다. `36a883a`/f6는291pass/1fail이었고, 마지막 실패는 test port의
registered bytes가 전체 wire 길이가 아닌 fixture 오류였다. 실제 object-v1
framing/identity로 교정한 `642f74a`/f7은 Native292개·558개 제외를 통과했다.
f7 Kernel은 stale generic fixture로 FAILED, 같은 source의 f8은9pass/3fail이었다.
current MapBlock fault target, Inline의 cap35>32, Shared64 batch 내 pending dedup
reserve oracle을 수정한 `4b06b203e636b2e303652a3f55429b506c2fcd01`/f9는
Kernel12pass·65개 제외·125.827초, source/tool guard0으로 VERIFIED다. f10 strict
Clippy는 당시6개 lib 지적으로 FAILED였다. B1 소유2개를 수정했고 f16의 lib/tests
`--keep-going`에서 Native 소유 진단0을 확인했다. 전체는 다른 owner 진단33개로 FAILED다.
후속 mr5 Medium과 lo5/lr5 Large Off/Record 결과는 아래와 같이 모두 FAILED다.
이 후보 결함을 기존 r4 deadline의 원인으로 소급하지 않는다.

f11 Tools03의 old Record epoch는 PATH identity guard가 producer body0으로 거절했다.
15개 path에서 `st_dev` 33→30만 바뀌고 hash/inode/mtime 등은 동일함을 검사한 새
Tools03 invocation epoch를 고정했다. guard를 우회하지 않았다. f12는 입장 대기
prefix 뒤 PID/open FD가 없고 실행 body가 없어 NOT_RUN으로 종료됐다.

새 epoch의 f13은 source347에서 Native292pass, Kernel13 중11pass/2fail이었다.
두 실패의 raw JSON leaf 가정은 실제 public leaf/block reader 계약이 아니어서
TF/positions 검증으로 바꿨다. f15 source `68ea46a…`는 Native292pass, Kernel13 중
12pass/1fail이었다. 남은 실패는 내용이 같아도 다른 ref mapping에 같은 block id를
기대한 oracle 오류다. 동일 ordered ref mapping을 재구성하는 추가 Tree→Block roundtrip
검증으로 교정했다. physical map wire/ordered children 및 pin/GC 검증은 보존했다.
후보 `d8e2c52de9f0e058475aa25e3d746cdad3015672`의 f17 변경 transition test는
1pass·77개 제외·6.337초다. d8e2와68ea 차이는 이 Kernel test 파일1개이므로
f15 Native292+기존 lifecycle12와 f17 변경 case1의 **composite local proof**는
VERIFIED다. 같은 exact commit의 full suite나 제품 수용은 아니다.

### 실제 Large 진입과 후보 활성화의 불일치

실제 harness는 step995에서 첫 profile, step1059에서 첫 SourceAdvance Patch를 만든다.
f9까지의 `pack_owners` Bootstrap-only 조건은 이 full Large 경로에서 block을 켜지
못했다. 따라서 f9의 기능 검증은 원 Large120 수리의 활성화나 속도 향상을 입증하지
않는다. 공유 Main은 `Bootstrap` 또는 `Patch(delta.prior.owners.0.is_none())`에서
packing하도록 수정했고, Sparse의 nonempty prior leaf/old-ref bound는 유지했다.
profile-first 초기 Patch의 공통 pin/GC/rebuild를 다루는 Kernel case는 f17에서
통과했다. 후보 source의 전체 Kernel13 동시 실행은 NOT_RUN이며, 위 composite
proof 범위만 VERIFIED다.

seed42의 Large 원입력8,609,531B는 확인됐다. 약33,716 unique leaf2.94MB에서
12,288 block23.93MB로의 변화는 현재 코드로 계산한 **Large 추정**이며 제품 측정이 아니다.
물리 객체 수 감소가 예상되는 반면, block header/term과 cold 단일-term read의 bytes
증가 위험이 있다. 원 metric별 region bounds, E07 fixed mixed query SLO, work/memory
한도로 채택 판단한다. 원120초·512MiB·ANN 조건을 완화하지 않는다.

후보 source는 Main의13개 파일을 외부 안정 `s3`에 복사해 local snapshot으로 고정했다.
공유 Main HEAD/index는 바뀌지 않았다. `candidate-source-03.json`의 focused status는
VERIFIED다. `candidate-build-03.py`는 같은 controller env에서 normal r5 build와
Medium Record를 직렬 실행해 terminal을 남겼다. build exit0은 VERIFIED이며 Medium은
실제 full ACK 전 bytes cap에서 FAILED다. controller의 `pipeline_completed=true`와
전체 exit0은 제품 성공을 뜻하지 않는다: 내부 `medium_cli_exit_code=2`,
`medium_execution.exit_code=1`이다. lo5 Large Off도 아래와 같이 FAILED다.

### r5 후보 Medium의 비용 이동과 남은 거절

`d8e2c52d`의 mr5는 seed42/256 source/367,590B, fixed
1,000,000 reads·writes·mutations 및 536,870,912 bytes, Record에서 실제
`full_event_ack`를 실행했다. producer exit1·elapsed11.029초·outer timeout=false다.
`first_denial=bytes`: accepted usage는 686,015R/520,972W/536,866,122B/302,050M,
이어 scratch `edit_write.rs:79`의 14,133B charge가 거절됐다. `failed_stage=BuildPrepare`,
물리 write/fsync/directory-sync/barrier 실패0, full ACK·reopen·search는 미도달이다.
이 거절은 정확한 work cap 시행이며 테스트 fixture 오류나 host deadline이 아니다.

mr5 first child V5는 39,223 events, 7,684 spans, complete, health/orphan/dropped/
unwritten/open/lease 0이다. LexicalBuild lane2의 도달 prefix는 668,388R/514,386W/
532,360,166B/296,839M, wall3.299497초·thread CPU1.690297초다. SourceApply는
10,686R/3,387W/3,991,727B/3,608M, wall0.352170초·CPU0.197080초다.
하위 span은 모두 inclusive라 합산하지 않는다.

| mr4→mr5, 같은 Medium 입력의 Lexical 하위 | accepted bytes | mr5 span 결과·시간 |
| --- | --- | --- |
| InventoryPrepare lane2 | 261,371,848→321,302,214 (+59,930,366) | 260 ok, wall1.071781초·CPU0.518836초 |
| InventoryMapApply lane2 | 148,640,926→147,263,286 | 4 ok, wall1.098860초·CPU0.597989초 |
| InventoryDiff lane2 | 29,935,047→28,837,917 | 256 ok, wall0.364964초·CPU0.300533초 |
| Postings lane2 | 70,301,512→12,845,512 (실패 prefix) | lane37 error, lane38/39 미도달 |

Prepare의 lane34 tokenizer는256 ok, wall0.069646초·CPU0.058463초;
lane35 owner path는256 ok, wall0.086755초·CPU0.068366초다. 둘에는 work delta
counter가 없어 +59.93MB를 어느 세부 charge site에 나눌 수 없다. r4→mr5 managed
reserve input은5,230→1,635, install input은5,224→1,629, 실제 object write bytes는
3,298,069→3,566,935B다. InventoryDiff next-occurrence validation은3,083→512,
RAM reuse13,952→50,614, spill reuse34,091→0이다. B1이 객체·검증 호출을 줄인
사실과 Prepare logical bytes 증가가 동시에 관측됐다. 각 run의 work cap이 동일해
Lexical accepted bytes 총량은 거의 cap에 묶인다. 따라서 Postings prefix 감소를 전체
work 절감이나 Large 속도 향상으로 해석하지 않는다. 다른 소스·실행 환경의 wall/CPU
비율도 정식 성능 비교가 아니다.

l6ctl은 build/runtime PATH drift에서, l7ctl은 tool mount `st_dev` epoch 변화에서
각각 producer body0으로 거절됐다. lo5는 모든 tool metadata를 새 epoch에서 검사한
l8ctl로 시작했고, r5와의 PATH-only 차이는 별도 local runtime cohort로 선언했다.
가드를 완화하지 않았다. lo5의 직접 `execution.json`과 refusal은 source
`d8e2c52d` 및 r5 pair SHA를 기록한다. Large 원 입력 seed42/4,096 source,
reads/writes/mutations 각각67,108,864 및 bytes549,755,813,888, trace Off,
server dispatch120초·outer600초다. 실제 producer exit1/226.898901초,
`full_event_ack` incomplete wall124.435900초이며 `first_denial=deadline`,
`failed_stage=BuildPrepare`, checkpoint는 storage `object.rs:628`이다. accepted
usage는10,550,723R/8,042,018W/8,296,724,947B/4,545,263M이고 numeric cap,
memory, external-resource refusal은 없다. full ACK/search/reopen에 미도달했다.
같은 source에서 B1이 활성화돼도 원 Large120은 닫히지 않았다.

`l8ctl/terminal.json`의 요약 `execution.elapsed_seconds=145.286325`는 이전
`lo4/execution.json`을 읽은 controller copy bug다. 이 요약값은 lo5 결과로 쓰지 않는다.
`lo5/execution.json`의226.898901초와 lo5 refusal/CLI exit를 직접 판정한다.
원 terminal을 덮어쓰지 않고 controller 교정 기록을 별도로 남긴다. 이 local cohort는
PATH-only drift와 CLI `build_binding=UNPROVEN` 때문에 managed-fresh·matched-host
성능 비교 자격이 없다. 실패한 원120초 제품 조건만 관측했다.

같은 r5 pair/source의 lr5 Large Record도 producer exit1이다. server request는
120.654초, full ACK phase는127.697초 도달 prefix 뒤 deadline에서 거절됐다.
`first_denial=deadline`, `failed_stage=BuildPrepare`, checkpoint는 storage
`object.rs:593`; accepted usage는10,362,905R/7,918,927W/8,141,195,237B/
4,482,173M이다. full ACK/search/reopen은 미도달이다. Native child V5는
858,005 events written/4,698 dropped/7 open/0 leases로 **partial**이다. parser가
닫힌 span을 판독했어도 완전 캡처 또는 전체 작업 계수로 승격하지 않는다.

lr5 실패 request의 SourceApply는 wall15.897초/CPU4.269초, LexicalBuild는
wall103.728초/CPU42.419초다. Lexical의 선택된 accepted work prefix는
10,083,692R/7,815,076W/8,064,744,528B/4,400,049M이다. 하위
InventoryPrepare lane2는2,860 spans, wall41.232초/CPU14.563초,
4,979,364,938B; InventoryMap lane2는44 spans, wall31.917초/CPU14.817초,
2,250,442,352B; Diff lane2는2,815 spans, wall11.428초/CPU7.386초,
508,500,555B다. ManagedObjectInstall 132 spans는 wall54.302초/CPU18.740초로
Lexical 하위와 중첩한다. lane34/35는2,816 owners, 44 map batches×64이며
deadline 시점에1,280 owners가 남았다. Postings는 완료되지 않았다. 물리 진단의
23,630 writes/58,508,801B와 write/fsync 실패0은 도달 prefix만 나타낸다.
포함 관계인 span 시간을 합산하거나 r4와의 속도 비율·tracing overhead로 읽지 않는다.

### typed inventory confirmation fee의 charge-site 수리

mr5의 Prepare 비용 증가를 소스에서 재검토했다. `admit_inventory_confirmation`은
planned inventory `p/n`을 `optional_map_value`로 인증하고 `n`을 실제 installed
`refs`의 Object로 확인한 뒤에도, 각 행의 읽기와 PK 이동 사전 charge에
`MAX_MAP_VALUE_WIRE_BYTES=195`를 `p/n` 두 필드 모두에 적용했다. 이 행은 실제로
`p=None` 또는 Object, `n=Object`만 허용한다. Null은 1B로 보수적으로 charge하고
Object의 canonical wire는 33B다. 기존 `key+2×195+72` envelope를 인증 후
`key+(1 또는 33)+33+72`로 바꾸었다. 읽기 1회와 PK 이동 2회에 쓰이므로 planned
행당 accepted logical fee 차이는 prior Null 1,068B, prior Object 972B다.
실제 행 수와 남은 Postings 작업을 모르면 총 절감량·Medium 통과 여부를 확정할 수 없다.
원 cap을 늘리거나 charge를 생략하지 않았다.

같은 bounded SQL batch, pre-install `reserve_for_write`, confirmed-record scratch 합산,
물리 설치 뒤 count/ref 재검증 및 SQLite 하드 page cap을 유지한다. `p`가 Inline이거나
`n`이 Object가 아니면 기존 typed corruption을 유지한다. literal codec/fee 테스트는
Null/Object 20,884B와 Object/Object 20,980B를 고정 gold로 확인하고 각각 exact−1
예산의 거절에서 planned row가 Ready로 보이지 않음을 검증하도록 추가했다.
이 변경은 **code-level fee correction**이다. CPU/wall 개선이나 제품 수용의 실측이 아니다.

첫 frozen 후보 `c49b2a1fa0e3dc30d88e6189ef0cc0ddbac25bfa`는 Main에서 소유
15개 파일을 복사했다. f18은 새 테스트의 `check_eq!`에서 임시 encoded 값의 참조 수명
E0716으로 컴파일 FAILED, 실행 body0이다. 실제 생산자 결함 판정은 아니다. 기대값이나
제품 코드는 바꾸지 않고 encoded 값을 지역 변수에 묶은 test-only 교정을 새 frozen
`a6083fb22480786d5e8172057b6fbe291af26ee7`에 고정했다. f19 focused Native·Kernel
terminal은 Native exit0/lifecycle exit0이다. Nextest는 Native293pass/558 제외,
Kernel lifecycle13pass/65 제외를 실제 실행했다. 이 exact source의 영향 집중
scope는 VERIFIED다. 같은 source의 r6 normal cached pair build는 exit0,
mr6 Medium 제품 body는 FAILED다. Large120은 NOT_RUN이다.

### mr6: typed fee 절감의 실제 크기와 다음 거절

r6는 `a6083fb22480786d5e8172057b6fbe291af26ee7`의 정상 cached pair build
exit0이다. 같은 controller 환경의 mr6 Medium fixed512MiB Record는 producer
exit1/13.664초, outer timeout=false, full ACK 전 거절됐다. `first_denial=bytes`,
accepted 874,591R/573,176W/536,864,032B/350,737M 뒤 Catalog
`managed_install.rs:108`의 10,692B charge가 실패했다. numeric bytes cap 시행이며
memory/external-resource refusal, 물리 write/fsync/dir-sync/barrier 실패는 없다.
물리 진단은2,403 writes/3,946,025B다. r6 controller의 완료 flag는 제품 성공이 아니다.

mr6 child V5는45,923 events/8,713 spans, complete, health/orphan/dropped/unwritten/
open/lease0이다. mr5와 같은 Medium 입력에서 InventoryPrepare lane2 accepted bytes는
321,302,214→266,699,646B로 **54,602,568B 감소**했다. 이는 첫 인덱싱의 prior
Null/Object 51,126 planned 행×행당1,068B와 정확히 일치한다. 코드에서 특정한
generic195B fee의 제거가 실제 WorkBudget에 반영됐다는 근거다. MapApply의
147,263,286B와 Diff의28,837,917B는 그대로이며, Postings lane37 실패 prefix는
12,582,998→67,183,476B로 진행했다. 결과적으로 512MiB 전체 cap에서 더 뒤의
Catalog 설치 사전 charge가 거절됐다. LexicalBuild selected prefix는
856,964R/566,590W/532,358,076B/345,526M, wall5.745559초·CPU2.953458초다.
이 nested/실패-prefix 시간은 총 완료 시간이나 mr5 대비 속도 개선율이 아니다.
실제 Medium 수용은 FAILED이며, B03-B2의 8KiB/512-term 실험은 원 cap을 유지한다.

## 2026-11-06 재감사 판정

확정된 직접 실패는 C17·51b70b34·0a97e858 Large의 단일 SourceAdvance dispatch120초 소진이다.
코드에서 확인한 구조적 비용은 작은 불변 객체의 물리 파일 단위 영속화와 여러 단계의
검증·map/SQL 처리다. 이것만으로 전체 지연의 지배율이나120초 불가능을 확정하지 않는다.
51b70b34의 원 managed 빌드는 terminal/receipt 없이 중단됐다. 해당 프로세스와
log FD 부재를 확인한 뒤 같은 target의 normal cached build를 재개해 exit0으로 마쳤다.
mr2와 최신 mr4는 모두512MiB 논리 work cap에서 실패했다. 같은 r4 pair의 원 Large
Off/Record도 dispatch120초 deadline으로 실패했다. 최신 결과는 바로 아래에 기록하고,
이후 mr2/lo2/lr2·C17 절은 해당 시점의 역사적 근거로 읽는다.

| 분류 | 확인한 내용 | 판단 한계 |
| --- | --- | --- |
| 직접 원인 | C17 SourceApply18.419초+완료하지 못한 Lexical100.595초 | 실패 prefix이며 seal/commit까지 끝난 전체 시간이 아님 |
| 구조적 증폭 | 배치 안에서도 새 객체마다 파일 생성·쓰기·file fsync·게시. directory sync/barrier만 공유 | API256 batch를 물리256객체의 단일 쓰기로 해석하지 않음 |
| 입력 규모 | seed42 Large4,096파일/8,609,531B. Medium256파일/367,590B | 파일 수16배, bytes23.42배. 동일 작업당 선형 시간은 미입증 |
| 호스트 조건 | Lexical wall100.629초, thread CPU29.479초, 차이71.150초 | scheduler·동기 I/O·blocking을 분해하지 못해 호스트만의 시간으로 부를 수 없음 |
| 부수 수리 | B02 계수·context mask·Python 검증기와 해당 테스트 | 관측·거절 정확도 개선이며 Large 속도 개선 증거가 아님 |
| 수용 공백 | 최신 source-bound Large120 ACK/search/reopen 없음 | 과거 다른 소스의 성공·테스트 수로 대체 불가 |

### 최신 r4 고정본: 정상 빌드와 세 실패 실행

외부 `~/.codex/qib6-_0r6mac7/s3/`의 고정 source는
`0a97e8589daa41b736cb6d59545b0116bb3b89c6`다. `r4/terminal.json`은
normal cached resume build exit0과 Catalog 집중37개 통과를 기록한다.
이 빌드는 managed-fresh가 아니다. 실행 세 개의 source, scale binary, daemon binary는
각 실행 전후 동일했다. 그러나 `l4ctl/m5ctl/l5ctl/terminal.json`의 build/capture
filtered environment에는 `PATH` 한 key의 drift가 있으며 `build_binding=UNPROVEN`이다.
첫 `m4ctl` 시도는 이 drift에서 body0으로 거절됐고 `mr4`를 만들지 않았다. 후속 세
실행은 실제 producer body를 실행했지만 source/pair 일치와 제품·canonical 수용은
서로 다른 판정이다. CLI exit2는 실패 producer/capture 자격 부족을 나타내며 빌드 실패로
해석하지 않는다.

| 실행 | 제품 상태와 경과 | 직접 거절 | raw/종료 근거 |
| --- | --- | --- | --- |
| `mr4` Medium fixed Record | producer exit1, outer timeout=false, 12.356839초; product `UNVERIFIED` | bytes cap512MiB: used536,865,857B, 다음10,600B 거절 | `mr4/execution.json`, `mr4/causal-timeline.jsonl.native-medium-children/first.jsonl`, `first.stderr` |
| `lo4` 원 Large Off | producer exit1, outer timeout=false, 145.286325초; product `UNVERIFIED` | `first_denial=deadline`, `failed_stage=BuildPrepare`; checkpoint는 `text.rs`의 token/key 처리 | `lo4/execution.json`, `lo4/stderr` |
| `lr4` 원 Large Record | producer exit1, outer timeout=false, 146.810698초; product `UNVERIFIED` | `first_denial=deadline`, `failed_stage=BuildPrepare`; checkpoint는 `managed_install::confirm`의 lane8 packet loop | `lr4/execution.json`, `lr4/causal-timeline.jsonl.native-large-children/first.jsonl`, `first.stderr` |

원 Large의 명시 한도는 reads/writes/mutations 각각67,108,864,
bytes549,755,813,888이고 단일 dispatch120초다. `lo4`의 거절 시 usage는
10,865,924R/6,884,981W/6,577,808,745B/4,331,086M,
`lr4`는11,227,154R/7,126,249W/6,791,074,318B/4,509,605M이다.
두 실행 모두 숫자 한도·memory·external resource 거절이 아니다. Off도 실패하므로
tracing만 제거해 해결되는 조건은 아니다. 두 실행은 도달 prefix가 다르므로
145.286/146.811 비율을 tracing overhead로 해석하지 않는다.

`mr4`의 닫힌 child는 V5 56,184 events, complete, dropped/unwritten/open/lease/health/
orphan0이다. WorkBudget의4계수는 **accepted logical work의 inclusive delta**이며
실패한10,600B charge와 물리 I/O는 포함하지 않는다. LexicalBuild lane2의2 span은
945,790R/582,069W/532,359,833B/396,516M, wall6.718217초/CPU4.884094초다.
SourceApply lane3은10,686R/3,387W/3,991,727B/3,608M,
wall0.304436초/CPU0.165732초다. Lexical의 bytes는 최종 used의99.16%다.
Prepare lane2 260개는261,371,848B, MapApply lane2 4개는148,640,926B,
Diff lane2 256개는29,935,047B, Postings lane2는70,301,512B다.
오류는 Postings lane37→Postings lane2→LexicalBuild→BuildPrepare로 전파됐다.
lane38/39는 미도달이다. 이 하위 계수는 Lexical 안에 중첩되므로 더하지 않는다.
물리 진단은5,230 writes/3,298,069B, failed confirm1,
write/fsync/directory-sync/barrier 실패0이다. 같은 cap 실패가 남아 있어
File get hash와 Catalog registration encode 수리가 제품 조건을 닫았다고 볼 수 없다.

`lr4`의 닫힌 first child는 V5 637,289 events, complete,
SHA256 `171b3978decc9fa29faac4a0ff6b247ed939e582f7388fd84572a21834002b04`,
dropped/unwritten/open/lease/health/orphan0이다. 실패 request wall120.152688초,
LexicalBuild lane2 2 span은 wall114.628386초/호출 thread CPU66.000876초,
accepted work10,947,993R/7,022,413W/6,714,625,437B/4,427,495M이다.
SourceApply lane3은 wall5.107407초/CPU3.680325초,
accepted work168,467R/52,751W/68,528,213B/56,564M이다.

| `lr4` Lexical 하위 | span/결과 | Inclusive wall 초 | 호출 thread CPU 초 | accepted work bytes |
| --- | ---: | ---: | ---: | ---: |
| InventoryPrepare lane2 | 2,730 ok | 45.074986 | 24.980680 | 3,754,808,912 |
| InventoryMapApply lane2 | 41 ok/1 error | 45.413534 | 26.987565 | 2,165,304,826 |
| InventoryDiff lane2 | 2,624 ok | 12.218451 | 9.881738 | 486,786,729 |
| Postings lane2/35–39 | 미도달 | 0 | 0 | 관측 span 없음 |

같은 순서의 나머지 accepted work는 Prepare
4,521,661R/3,150,630W/1,319,433M, MapApply
4,119,832R/3,050,320W/3,047,632M, Diff
610,726R/757,280W/0M이다. 모든 해당 span에4개 계수가 존재한다.
실패 ancestor는 ManagedObjectConfirm lane3→InventoryMapApply lane2→LexicalBuild→
BuildPrepare→request다. 마지막 confirm은 wall0.526133초이며 deadline 검출 위치가
전체 지연의 지배 함수라는 뜻은 아니다. child의 물리 진단은39,288 writes/42,915,388B,
failed confirm1, write/fsync/directory-sync/barrier 실패0이다. Lexical
wall−호출 스레드 CPU는48.627510초로 동기 I/O·blocking·scheduler 대기가 섞일 수 있다.
반면 Prepare45.075초/MapApply45.414초의 실제 실행 경로와 CPU도 확인됐다.
코드 단독 또는 호스트 단독 원인, 단계별 exclusive 기여율은 이 trace로 확정하지 않는다.
각 stage의 span·work delta는 상위 Lexical에 포함되고 sibling 집합도 합산하지 않는다.
완전한 진단 child는 실패 prefix의 증거이며 full ACK/search/reopen이나 정식
Off/Record 비교 성공의 증거가 아니다.

### 새 mr2: 시간 실패와 논리 작업량 실패의 분리

고정본 `51b70b3470ecbb58beebf757831eb53db85aade6`의 정상 cached bench pair는
Cargo exit0이다. 외부 `qib6-_0r6mac7/r2/normal-build-result.json`이 결과를 기록한다.
빌드의 admission758.451초와 compile39분39초는 제품 dispatch120초의 일부가 아니다.
원 managed-fresh 실행의 중단 기록과 prebuilt build_binding=UNPROVEN은 보존한다.

`mr2/execution.json`은 Medium fixed Record가23.202859초에 producer exit1로 끝났음을
기록한다. 외부 timeout이 아니며 full SourceAdvance가 BuildPrepare/NativeTextPostings에서
`Terminal(Refused(ResourceLimited))`가 됐다. reads963,417/writes588,655/
mutations401,727/bytes536,865,857을 사용한 뒤 bytes10,600 청구가 거절됐다.
남은5,055B보다 큰 청구였으며 거절된10,600B는 used에 더하지 않는다.

거절 위치 `Catalog::managed_install::charge_object`는 **마지막 관측 지점**이다.
해당 객체의 canonical registration JSON 길이 J와 child 수 E에 대해 각 reserve/confirm이
`4J+256E`를 청구한다. compact 저장 codec과 별개로 보수적 논리 work를 유지하는 계약과
독립 fee oracle가 있다. 이 위치만으로512MiB를 Catalog가 모두 썼다거나 잘못된 중복
과금이라고 판단하지 않는다. 객체별 row/64개 SQL packet과 최대256개 설치 batch를
확인했으며 이 경로의 전체 SourceAdvance 재직렬화·전체 operation 반복 protect는 찾지 못했다.

추가 증폭 후보는 TextSpool의 term_page 반환 term당4,096B와 confirmed-posting SQL
probe당4,096B, Prepare token_groups의 source별 신규 term당4,096B다. map scratch는
64-row query/INSERT당 기본 비용이므로 이를 term당4KiB와 섞지 않는다. 모든 비용은
요청 전체의 누적 work이며 실제 쓰기3,298,069B와 나눠 disk write amplification이라
부를 수 없다. stage별 work delta 또는 청구원 ledger 없이 주범 비율은 미확정이다.

first child V5 failure trace는71,097 events, loss/health/open/lease0이다. Diff256개는
요청18의 full build prefix에 속한다. prior0/next3,083/RAM13,952/spill34,091/equal0/
validation failure0이다. 재사용 반환48,043회와 실제 next 검증3,083회를 관측했고,
OccurrenceRead lane31은wall1.473913초/CPU0.616303초다. nested inclusive 시간은
합산하지 않는다. 이 Medium 실패에서 occurrence 재검증이 주원인이라는 근거는 없다.

**실행 순서 정정:** fixed Medium512MiB 성공은 original Large 실행의 제품 조건이 아니다.
Large는 원래 explicit reads/writes/mutations67,108,864·bytes549,755,813,888와 dispatch120초다.
mr2 실패를 유지하고 동일 binary의 Large Off를 독립 판정한다. Medium 재funding으로
실패를 덮거나 ANN4K의1M/512MiB·SDK30초/worker10초를 완화하지 않는다.
과거 C16/C17 Medium은 문서상 explicit funding이므로 원 숫자가 부재한 지금의 fixed
mr2와 직접 속도·비용 회귀 비교를 하지 않는다.

### 새 lo2: tracing Off에서도 원120초 실패 재현

같은 source/pair/filtered environment의 Large Off는 producer exit1, outer timeout=false,
elapsed178.514856초다. source와 양쪽 binary는 전후 동일하다. Full source staging은
33.003335초로 별도 요청이며 full_event_ack 관측129.481174초에는 서버 dispatch와
통신·상태확인·정리 시간이 포함된다. 이를 서버 deadline129초라고 해석하지 않는다.

직접 거절은 `first_denial=deadline`, `failed_stage=BuildPrepare`다. 당시 누적 work는
reads7,914,339/writes4,979,851/bytes4,715,789,654/mutations3,163,350이며 원 한도
67,108,864/67,108,864/549,755,813,888/67,108,864보다 작다.
denied_charge/denied_memory/external_resource_refusal은 없다. 거절 checkpoint는
managed_install.rs:817이며 그 함수가 전체 지연을 만들었다는 뜻은 아니다.

이 실행은 trace Off에서 실패했으므로 tracing overhead만 제거해 해결되는 실패가 아니다.
Off에는 Text 세부 phase가 없어 mr2의 작은 입력만으로 Large 실패 구간을 확정하지 않는다.
당시 계획된 동일 pair의 Large Record1회(`lr2`)로 단계·실패 ancestor를 진단했다. 두 실패
prefix의 도달 작업이 다르면 Off/Record elapsed 비율을 overhead라고 보고하지 않는다.

mr2의 세부 실패는 첫 changed-term page64개의 lane37 `entry_patch::apply`다.
lane35 두 span·lane36 한 span은 성공했고 wrapper prep/put/top-map인 lane38/39는
미도달이다. lane37은wall2.005342초/CPU0.930751초이며 상위 Postings2.006600초에
포함된다. 설치 packet4개가 각각256개의 물리 객체를 성공적으로 설치했고4번째
Catalog confirm이 누적 work bytes에서 거절됐다. 해당 물리 phase의 관측 실패는0이다.
이를 wrapper 또는 최종 top-map 비용이라고 부르지 않는다.

**mr2 당시 관측 공백:** 그 source의 SourceApply/Text 시간 span은 stage별 WorkUsage
delta를 발행하지 않았다. mr2의 work_bytes 한 건도 이전 요청의 member assembly에
속했다. 현재 `0a97e858`에서는 기존 span에4개 accepted delta를 발행하며,
위 `mr4/lr4` 첫 child에서 모두 관측했다. 부모 inclusive와 자식 delta를 합산하거나
실패한 청구를 accepted delta에 넣지 않는다. 계측은 성능 수리 자체가 아니다.

### 새 lr2와 실행 직후 호스트 관측

동일 pair의 Large Record도174.409383초에 producer exit1로 끝났으며 outer timeout은
없다. BuildPrepare의 deadline이고 work bytes2,635,718,937, denied charge/memory/
external resource refusal은 없다. source/pair 전후 동일이다. Off의 bytes4,715,789,654와
도달 작업이 다르므로174.409/178.515 비율을 tracing overhead로 해석하지 않는다.

child 전체의 quiescent 물리 진단은23,273 writes/23,073,002B, file sync23,273,
directory sync17,868, barrier257, reserve257/confirm257이며 failed confirm1이다.
물리 write/file sync/directory sync/barrier 실패는0이다. 누적 시간은 reserve26.945초,
confirm40.820초, write2.316초, file sync7.197초, directory sync0.882초, barrier6.192초다.
이는 child 전체 누적값이며 단계별·exclusive 시간 또는 syscall 기여율로 바로 합산하지 않는다.

실행 종료34초 뒤02:41:58–02:42:00 KST의 bounded `top -l 2 -s 1 -n 0` 관측에서
16CPU 호스트 load average53.19, CPU idle0–0.4%, physical memory47GiB 사용,
compressor24GiB와 unused117MiB를 확인했다. 두 번째 interval의 swap-in36,258/
swap-out34,420 pages도 관측했다. **호스트 경합과 메모리 압박은 실제 관측 사실**이다.
이 짧은 실행 직후 샘플로 요청 전체의 scheduler/I/O 대기 비율이나 조용한 호스트의
완료 시간을 계산하지 않는다. 코드가 효율적이라는 증명도 아니다. 동일 소스·고부하
조건에서 무변경 Large를 반복하지 않고 기존 failure trace로 다음 수리 owner를 고른다.

### lr2 오프라인 분석: 실제 도달 경로와 다음 코드 범위

기존 V5 parser로 닫힌 first child를 검증했다.451,717 events, complete,
health/drop/unwritten/open/lease0이다. 실패 요청은 id168/control/connection169,
wall120.097193초이며 BuildPrepare→LexicalBuild→InventoryMapApply lane2→
ManagedObjectConfirm(64 inputs)에서 deadline을 관측했다. trace의 완전성과 제품 성공은
다르다. full ACK/search/reopen은 미도달이고 canonical product 검증도 없다.

| 실패 요청의 관측 범위 | span 수 | Inclusive wall 초 | 호출 thread CPU 초 |
| --- | --- | --- | --- |
| SourceApply lane3 | 1 | 24.864602 | 6.292227 |
| LexicalBuild | 1 | 94.511617 | 32.011491 |
| InventoryPrepare lane2 | 1,040 | 40.306306 | 14.048999 |
| InventoryMapApply lane2 | 16(15ok/1error) | 32.479142 | 9.941968 |
| InventoryDiff lane2 | 960(all ok) | 16.059402 | 6.148245 |
| OccurrenceRead lane31 | 11,087 | 8.603131 | 3.782397 |

각 행은 해당 span의 inclusive 합이다. 부모·자식 또는 nested 같은 stage의 시간과
CPU를 합산해120초 기여율을 만들지 않는다. 특히 OccurrenceRead는 Diff 안에 있다.
최종 오류 MapApply는0.635719초이며 그 마지막64-input confirm은0.034997초다.
거절이 그 지점에서 감지됐다는 사실만으로 마지막 confirm이 지배 병목인 것은 아니다.

Diff 계수는 prior0/next11,087/RAM59,456/spill198,293/equal0/failure0이다.
재사용 반환257,749회가 이미 존재한다. 완료된 owner별 inventory 항목 수와 실제
토큰 그룹 방문 수는 동일하지 않다. 같은 body의 inventory reuse는 token traversal을
건너뛸 수 있으므로 계수 합계에4KiB를 곱해 token-group 비용 하한을 계산하지 않는다.

설치 aggregate의 write phase 호출은 가장 가까운 Text 조상별로 다음과 같다.

| Text 조상 | 관측 packet 수 | write phase 호출 수 |
| --- | --- | --- |
| InventoryPrepare lane2 | 16 | 2,203 |
| InventoryPrepare lane35 | 37 | 9,472 |
| InventoryMapApply lane2 | 16 | 1,024 |
| InventoryDiff lane2 | 0 | 0 |
| LexicalBuild 직접 하위 | 15 | 960 |

이것은 도달한 설치 phase의 호출 수이며 고유 ref·성공 게시 파일 수·device write 수·
단계별 실제 bytes delta가 아니다. 위 source-bound 호출 대조상 Prepare subtree의
설치 생산자는 OccurrenceWrites::flush이므로11,675개 write phase 호출은 그 경로에
귀속된다. 포맷 후보의 이득이나 query 비용을 검증하지 않고 전부 제거 가능하다고
계산하지 않는다. nearest Text별 reserve/confirm input 합은 미산출로 남긴다.

**lr2 당시 코드 후보 결정:** B03-A count 전달은 우선 수리에서 보류한다. B03-B owner별 bounded
occurrence block의 최소 codec/producer/reader·독립 fixture를 먼저 검토할 근거는 확보했다.
단 전체 포맷 채택은 exact payload dedup 손실·query read bytes·증분 locality를 반증하는
기존 계약을 통과해야 한다. Posting Block은 이 Large 실패가 Postings 이전이므로 우선
패킷에서 제외한다. Catalog의 기존 transaction-local child proof/allowance 수리를
같은 의미로 다시 구현하지 않는다.

B03-B의 추가 계약 결손도 보완한다. 현재 physical packet은 여러 file/ordinal을 담는다.
`(ref,canonical stored term)` selector를 쓰는 block에 서로 다른 owner의 동일 term과
다른 positions를 같이 넣으면 선택이 모호하다. **block 내용은 한 owner의 logical
inventory로 제한**하고 완성된 block들만 기존 물리 put_batch에서 함께 설치한다.
동일 bytes의 정확한 block alias는 허용하지만 cross-owner mixed-content block은 별도
selector/권위 계약 없이 허용하지 않는다. 이 보완은 설계 계약이다.

### 후속 코드 수리와 B03-B 계약 재검토

B03-B를 구현 전에 producer/reader/독립 oracle로 대조했다. 현재 FileObjectPort::get은
객체 전체를 읽고 wire와 decoded payload bytes를 청구한다. 두 term을 block으로 묶으면
cold 단일-term 조회에서도 다른 term의 payload를 읽는다. query-local cache는 첫 read를
없애지 못한다. 기존 leaf는 positions만으로 ref를 만들기 때문에 term/owner/surface 간
exact payload alias도 존재한다. block에 term key를 넣으면 이를 잃을 수 있다.

증분에서도 `previous == identified_leaf_ref`는 prior block과 항상 달라 동일 positions를
변경으로 오판한다. 이를 고치려면 prior block 전체를 term별로 인증·선택해 비교해야 한다.
이 결손은 포맷을 바로 제품 개선으로 인정하지 못하게 한다. 다만 **모든 cold 단일-term
read bytes가 단1B라도 증가하면 영구 반려**라는 조건은 원 제품 계약이 아니라 이 RCA가
자기 부과한 과도한 기준이었다. [원 RFC](../../rfc/oct-10-native-incremental-vnext.md)의
metric별 선고정 region bound와 고정 query mix/SLO, E07 warm lexical top20
p95≤100ms/p99≤500ms, 기존 work/memory budget이 채택 기준이다. cold read 증가는
측정·보고하고 해당 한도와 latency 안에서 판단한다. 결과를 본 뒤 threshold를 만드는
일은 허용하지 않는다. 원 Large120초/Medium512MiB/ANN4K의 별도1M·512MiB·
SDK30초/worker10초를 혼합하거나 완화하지 않는다.

재검토된 B03-B1은 **새 current policy** 안에서 owner-local term→positions-vector
dictionary(최대4,096B/128 term rows)를 구현했다. 동일 positions는 block 내부
vector를 공유하고 singleton/oversize/변경된 delta term은 leaf로 둔다. `(ref,term)` count 선택은
완전 인증된 bounded layout에 묶고 ref-only scalar memo는 leaf에만 사용한다.
구 policy의 silent fallback/rewrite/drop은 없으며, 새 policy 안의 old/current
generation·reopen·pin을 보존해야 한다. 실제 구 refs/pins이 남은 cutover는 호환 실행
artifact를 보존하거나 거절해야 한다. Codec·producer·독립 oracle 합류 뒤 f15
Native292개와 기존 Kernel lifecycle12개, f17 변경 transition1개의 composite local
proof는 VERIFIED다. 같은 exact source의 전체 Kernel13은 NOT_RUN이다. 새 정상
cached pair build는 VERIFIED지만 mr5 Medium과 lo5 Large Off는 각각 FAILED다.
집중 body만으로 채택을 선언하지 않는다.

실제 코드에서는 다음 두 결손을 수리했다.

1. **File get의 같은 wire 이중 SHA:** `read_io::ReadWire`는 해당 module의 private
   bytes/reference를 가지며 길이·read_exact·EOF·digest 확인이 끝나야만 생성된다.
   FileObjectStore/FileObjectReader의 get은 이 reference-bound 값을 소비해 공통 body
   decoder로 간다. scratch/resident의 raw decode는 길이와 SHA를 유지한다. namespace,
   FD owner, allocation lease, work charge와 cancellation 순서는 보존한다. 파일 read의
   동일 wire hash는2회에서1회로 줄지만 Large 시간 절감률은 아직 미측정이다.
2. **인덱싱 단계의 work delta 누락:** Core WorkBudget의 공통 helper가 기존 WorkReads/
   WorkWrites/WorkBytes/WorkMutations를4개 모두 accepted usage 차이로 기록한다.
   underflow는 health만 남기고 부분 family를 발행하지 않는다. Kernel WorkStage가 이
   helper를 재사용하고 SourceApply가 WorkStage를 사용한다. Text는 LexicalBuild와
   Prepare/Map/Diff/Postings/Root의 owner lane2, Postings의 bounded35–39 step에 연결한다.
   per-object read/edit span에는 추가4계수를 붙이지 않는다. Off 직접 실행·원 product
   결과·grant를 보존하며 실패한 청구는 delta에 넣지 않는다. nested delta는 inclusive다.

실제 두 File get 경로에서 metadata 확인 뒤 memory-admission hook으로 동일 inode를
truncate/append하는4가지 분기를 결정적으로 검증했다. 이 테스트에서 처음 나온
unused-results compile 오류2개는 수정했고, 그 후 집중 배치는 다음처럼 통과했다.

- VERIFIED: `./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-core
  -p quanta-index-storage -p quanta-index-lexical --lib -E 'package(=quanta-index-core) |
  package(=quanta-index-storage) | (package(=quanta-index-lexical) & test(native::text))'
  --test-threads 2` →741 passed/602 excluded. 신규 Core5개와 실제 File short-read/EOF,
  기존 Native Text/손상/namespace/취소·메모리 oracle를 포함한다.
- VERIFIED: Kernel lib `cargow --lane test-workspace-lane check -p quanta-index-kernel
  --lib --locked --offline`; SourceApply 연결 컴파일까지다.
- VERIFIED: Hexagonal boundary와 Core/Kernel module snapshot, 영향 rustfmt/diff 검사.
- NOT_RUN: 강화한 Kernel source_trace integration body와 새 정상 pair의 실제 UDS4계수.
  해당 integration의 SDK→Lance dev graph는 이 test cache에 없어 독립 fresh 재빌드를
  시작하지 않고 정상 pair 큐와 분리해 남긴다. 기존 strict 전체 FAILED도 승격하지 않는다.

당시 소유 소스12개를 합류한 `86e6130e`의 `r3` 실행 계획은 이력이다. 이후
`0a97e858`의 `r4` normal cached pair가 exit0으로 끝났고 동일 binary의 `mr4/lo4/lr4`
제품 body도 실행됐다. 결과는 위와 같이 모두 실패이며 managed-fresh·canonical
qualification은 성립하지 않는다. 과거51b 실패를 새 patch의 결과로 바꾸지 않는다.

추가 가설인 inventory Block child dictionary는 기존 positional child ref의 중복을
줄일 수 있으나 현 Catalog는 SQL edge INSERT 전에 이미 sort/dedup한다. row 수만큼
edge SQL이 감소한다고 계산하면 안 된다. 새 codec/policy·old pin 계약과 기존 P1-A
소유 파일을 건드리므로 이번 수리와 병렬 구현하지 않는다.

### 단순 읽기 최적화만으로 닫을 수 없는 이유

C17 occurrence read lane31의 도달 span은 wall10.442초/CPU3.925초다.
그 값도 prior와 신규 검증을 함께 포함한다. 신규 validated-count 전달만 고쳐 전체
100.629초 Lexical 문제를 해결한다고 말할 근거가 없다. 이 수치는 전체4K 완료 효과의
상한이나 ETA가 아니라 관측된 prefix의 직접 읽기 비용이다.

Medium Prepare의 신규 객체3,083개는 source당 약12.04개이고 SQLite requested
write23,096,256B는 원문367,590B의 약62.83배다. 이는 해당 phase의 작업 증폭이지
device write amplification이나 최종 디스크 배율이 아니다. 객체 단위를 줄이는 축이
namespace/create·file sync·Catalog/GC 반복을 함께 건드릴 수 있다는 설계 근거다.
durability를 낮추거나 이미 검증된 payload인 척하는 변경의 근거는 아니다.

### 최신 코드와 과거 측정을 혼동하지 않는 결정

현재 EntryMap은 Small/Block/Tree를 갖고, Block은 최대512개 행을 별도 payload/key
상한 안에서 묶는다. layout은 Text policy digest에 포함된다. 이미 공유 트리에 있는
이 변경을 다시 구현하지 않으며 C17 수치로 그 개선 효과를 인정하지도 않는다.
Block은 inventory map 객체 수를 줄이는 축이고 term occurrence의 파일 단위 저장을
모두 없애는 구조는 아니다.

`r4`의 work delta와 물리 진단을 기준으로 occurrence 저장 단위 수리를 먼저 검토한다.
남은 비용이 occurrence 재검증이면 저장 권위와 수명을 가진 count 결과 전달을 검토한다.
전역 Storage container 개편 전에 owner-local Lexical occurrence positions block의
효과를 원 budget/latency와 독립 oracle로 측정한다.
Storage API를 유지할 수 있지만 ref-only count 캐시, 증분 ref fanout, query의 반복 block
읽기와 기존 positions dedup 손실을 함께 해결해야 한다. 이 비용이 개선을 상쇄하면 폐기한다.
구체적인 선택·소유·검증·중단 조건은 [확정 실행 순서](WORKPLAN.md#execution-plan)와
[B03-B](WORKPLAN.md#b03-b)가 소유한다. 전역 저장 포맷/GC 재설계는 기본안에서 제외한다.

현재 Native 경로의 object-kind별 물리 census는 미관측이다. legacy LexicalStore의
kind aggregate를 Native로 가져오지 않는다. 51b70b34와 동일한 현재 Text 호출 경로에서는
Prepare subtree의 설치가 OccurrenceWrites::flush의 put_batch로만 이어지고 inventory
map 설치는 별도 sibling이다. 이 고정본에서 관측한 Prepare의 physical packet은
occurrence 설치로 귀속할 수 있으므로 이를 위한 추가 trace scope는 불필요하다.
옛 C17 source byte는 부재하므로 이 증명을 C17 전체 계수의 새 해석으로 소급하지 않는다.

### 해결이 지연된 진행 방식의 원인

1. 최근 B02 패킷은 계측과 검증기 결함을 닫았지만 큰 비용을 제거할 B03 수리를
   선택하지 않은 채 조건부 상태로 남겼다. 회귀 통과를 제품 성능 해결 진척과 구분해야 했다.
2. C16/C17의 도달 작업과 호스트 조건이 달라 인과 비교를 닫지 못했다.
   이후 원본 source/binary/trace 캐시도 없어져 과거 비교를 그대로 재개할 수 없게 됐다.
3. 새 고정본의 중단된 fresh 빌드와 정상 cached 재개에 긴 시간이 들었다. 현재 build는
   완료됐으며 이는 제품 인덱싱120초와 별개인 검증 소요 시간이다.
4. 마지막 좁은 테스트를 긴 managed build 뒤의 같은 admission 큐에 넣어300초
   입장 대기 제한으로 종료124가 됐다. 테스트 본문은0개 실행됐으며 이는 인덱서 실패가
   아니라 검증 실행 순서의 오류다. 같은 대기를 반복 실행하지 않는다.

추가 계측이나 새 티켓을 늘리지 않고 완료된 r4 failure trace로 수리 범위를 제한한다.
기존 build를 반복하거나120초·work/durability 계약을 완화하지 않는다.

## 실패를 발생시킨 조건

Large는4,096개 소스의 full SourceAdvance다. 소스 staging은 별도 요청으로 먼저 끝난다.
서버는 queue admission을 끝낸 뒤120초 dispatch 예산을 만들며 요청 저장·실행 입장, SourceApply,
CoreRequired Text build, seal, publication commit이 그 예산을 공유한다.
클라이언트600초와 capture900초는 서버 예산을 늘리지 않는다.
제품 queue wait10초는 dispatch 예산 생성 전의 별도 한도이며 C17 BuildPrepare 실패와
현재 Cargo admission300초 대기를 서로 합산하지 않는다.

C17 Record는 SourceApply18.418817초 뒤 실패한 Lexical100.594692초를 실행했다.
두 직렬 단계가119.013509초를 사용했고 다음 설치 예약의 checkpoint에서 deadline이
관측됐다. 거절 감지 위치를 해당 예약 함수의 성능 병목으로 해석할 수 없다.

실패 시 논리 work는 reads3,439,760, writes2,105,887, bytes1,981,181,324,
mutations1,343,714였다. 각 cap은67,108,864,67,108,864,549,755,813,888,
67,108,864다. 관측된 물리 write/sync/barrier 실패는0이다.
이 카운터는 서로 다른 계수 단위이며 OS syscall 수나 RSS로 변환할 수 없다.

근거는 [C17 종료 기록](../oct-22-finalizing/VALIDATION.md#e07-fixed-work-decision)과
[실제 dispatch](../../../crates/quanta-index-searchd-runtime/src/native.rs),
[SourceAdvance orchestration](../../../crates/quanta-index-kernel/src/update.rs)이다.

## 비용 분해

| C17 Large의 도달 범위 | Wall 초 | 호출 스레드 CPU 초 |
| --- | ---: | ---: |
| SourceApply | 18.419 | 6.843 |
| Lexical, configure를 포함한2개 span | 100.629 | 29.479 |
| Prepare의 토큰화+SQL, lane34의768개 child | 1.079 | 0.433 |
| Prepare의 grouped read/occurrence, lane35의768개 child | 33.807 | 10.574 |
| Prepare namespace/create, 신규 쓰기9,374개 | 14.745 | 6.450 |
| Map external-child validation | 10.917 | 3.322 |
| occurrence read, lane31의8,799회 get | 10.442 | 3.925 |

Prepare·Map·Diff와 그 하위 비용은 Lexical에 중첩된다. 표의 행을 합산할 수 없다.
768개 child는 완료한 전체4K의 비율이 아니다. 8,799회 get은 신규·기존 객체 읽기가
공유하는 lane이며 신규 객체 재읽기만의 수로 확정하지 않는다.

실패 전230개 physical install packet과20,390개 입력이 성공했다.
전체 capture는418,380events,41,836,325B이고 loss/health/orphan/open span/lease가0이다.
이것은 정상적으로 관측된 실패 prefix의 증거다.

기록에 남은 외부 원본 투영 경로는
`~/Library/Caches/quanta-index/audits/nov03-e07-wal-policy/record-analysis-c17ir1-physical.json`이다.
C17 실행 소스는 `8e94ca5c0f5200f2f923fd237b9e4d8a83d059b4`였다.

## 구조적으로 비용이 커지는 경로

### 작은 객체의 영속화

Text는 파일별 term occurrence와 inventory/map을 불변 객체로 만든다.
고유 occurrence payload를 한 packet에 모아 `AttemptContext::put_batch`로 설치한다.
Storage는 객체별 namespace/currentness 확인, 임시 파일 생성, 쓰기, file sync,
게시와 retirement를 수행한 뒤 디렉터리 sync와 배치 barrier를 수행한다.

따라서256개를 한 API 호출로 전달해도256개 파일의 처리 비용은 발생할 수 있다.
단위 객체 수는 경로 검사와 inode·Catalog·sync 비용을 증폭한다.
필요한 내구성과 인증을 만족시키는 표현의 크기와 검증 결과 전달 방식은 설계 선택이다.

완료한 C17 Medium은256소스,367,590source bytes에서 Prepare 신규 객체3,083개,
동일 Prepare의 SQLite xWrite1,371회와23,096,256requested bytes를 관측했다.
이 숫자는 저장·Catalog 작업 증폭을 보인다. 모두 불필요한 bytes라는 증거는 아니다.

경로는 [OccurrenceWrites::flush](../../../crates/quanta-index-lexical/src/native/text.rs),
[put_batch](../../../crates/quanta-index-core/src/index/attempt.rs),
[install_prepared_batch_with_barrier](../../../crates/quanta-index-storage/src/object.rs)다.

### 설치 뒤의 occurrence 검증 읽기

`diff_inventory`는 contribution의 token 합계를 확인하고 posting edit를 만들기 위해
position count를 사용한다. 새 reference는 `validated_new_count`를 거쳐 최초 읽기에서
저장 객체의 전체 payload 형식을 검증한다. 이미 검증한 결과는 attempt-bound memo와
spill에서 재사용한다.

현재 install 결과는 등록된 reference이며 검증된 occurrence count 권한이 아니다.
생성 당시 배열 길이나 미검증 SQL 값을 곧바로 count로 쓰면 저장 객체 손상 감지가
약해진다. 새 객체의 저장·검증 결과를 재사용하려면 그 결과를 보증하는 생산자와
유효 수명, identity/payload 일치, 실패 시 무효화를 계약으로 정해야 한다.

경로는 [validated_new_count와 diff_inventory](../../../crates/quanta-index-lexical/src/native/text.rs),
[load_count](../../../crates/quanta-index-lexical/src/native/text_occurrence.rs),
[immutable object port](../../../crates/quanta-index-core/src/index/ports.rs)다.
신규/기존·고유/반복·memo hit/miss 분리가 추가 수리 선택의 선행 조건이다.

## 2026-11-06 추가 코드 대조

### 같은 읽기에서 두 번 해시하는 경로

`FileObjectStore::get`은 `read_wire` 뒤 `decode_stored_wire`를 부른다.
read_io::read_wire_with_owner는 EOF와 전체 wire digest를 검증하고,
decode_stored_wire는 같은 소유 buffer의 digest를 다시 계산한다.
read-only File adapter도 이 순서를 사용한다. 이는 소스로 확인한 중복 계산이다.

다만 decode_stored_wire는 scratch/resident map에서도 쓰이므로 공통 함수의 digest
검사를 삭제하면 안 된다. 수리한다면 reference와 불변 bytes를 결속한 private
read 결과만 검증된 layout decoder로 전달하고, 외부/scratch wire의 인증은 유지해야 한다.
이 중복의 실제 CPU 기여는 미측정이다. 작은 leaf에서의 미세 개선을 Large120 해결로
주장하지 않는다. 별도 포맷 개편 전 검토할 수 있는 좁은 후보로 남긴다.

### 이미 존재하는 캐시 안의 반복 탐색과 여러 순회

- changed body의 각 term은 prior identity 비교 전 encode/identify를 수행한다.
  unchanged면 설치를 건너뛰지만 이 비교 준비 비용은 발생한다.
- changed term은 prepare_inventory, needs_flush, push에서 최대256개 pending 목록을
  반복 탐색한다. RAM 확인 창은64개이며 창 밖은 attempt-local SQLite spill을 조회한다.
  신규 cache 부재가 아니라 bounded 탐색·spill 정책의 비용 후보이다.
- inventory compact plan/CAS 원본 결속, 결과 inventory diff, contribution token 합계와
  posting edit 생성은 별도 순회다. 목적이 다른 검사를 같은 중복으로 취급해 제거하지 않는다.

Prepare lane35 CPU가 작고 namespace/sync 또는 wall 대기가 크면 선형 탐색 가설의
우선순위를 내린다. lane31과 next 검증 호출이 작으면 first-validation 전달 후보도 내린다.
추가 전역 set/cache나 계수는 이 분석만으로 도입하지 않는다.

### inventory Block으로 posting 비용까지 해결됐다고 볼 수 없음

현재 `EntryMap::validate_for(Posting)`은 Block을 거절한다. Block은 inventory에만
적용되고 큰 posting map은 Tree 경로를 쓴다. NativeTextIndex::prepare는 모든 source
change와 잔여 inventory/contribution을 처리한 뒤 apply_postings, write_root를 수행한다.
Prepare 중 deadline 실패한 prefix에는 이 후반 작업이 끝났다는 증거가 없다.
`mr4`는 Postings lane37에서 실패했고 `lr4`는 MapApply에서 실패해 Postings에
도달하지 않았다. 이 미도달 범위를 완료한 전체 초기 구축으로 외삽하지 않는다.

Bootstrap과 Patch는 같은 NativeTextIndex::prepare를 사용하며 SourceInputCursor와
base 유무가 다르다. cold bootstrap에는 prior가 없으므로 unchanged/no-op 최적화의
성공을 초기 구축 속도의 증거로 바꾸지 않는다.

### 긴 빌드와 제품의 semantic 실행을 분리

Runtime/harness는 semantic crate를, semantic은 LanceDB/Lance를, LanceDB는
DataFusion을 무조건 의존한다. 현재 동일 정상 제품을 유지하면서 이 의존을 제거하는
feature rail은 없다. 따라서 Text 전용 fixture도 이 graph를 컴파일한다.
하지만 해당 fixture의 profile은 CoreRequired Text1개, provider_units0이고 semantic
설정이 없다. semantic provider는 config.semantic이 Some일 때만 등록한다.
Lance/DataFusion 컴파일을 제품 SourceAdvance의 embedding 실행 비용으로 해석하지 않는다.

resume02는 admission758.451초(약12분38초) 뒤 실제 compiler에 입장했다.
이는 Cargo 입장 대기이며 제품120초와 별개다. 기존 target의 cached artifact를
재사용하고 있으므로 fresh 전체 빌드를 다시 시작한 상태가 아니다. artifact row 수를
빌드 완료율이나 남은 시간으로 환산하지 않는다.

build stderr의 MallocStackLogging 경고는 원인이 미확정이다. 입력 manifest의
Malloc/DYLD/Rust profiler env key는 부재했고 resume script는 filtered env를 build와
제품 capture subprocess에 전달한다. 메시지만으로 malloc tracing 활성화나 제품
tracing overhead를 주장하지 않는다. 정식 Off/Record는 동일 launch env/관측 모드로 판정한다.

## 이미 존재하거나 수리한 비용 절감

| 항목 | 현재 코드에서 확인된 범위 |
| --- | --- |
| 설치 배치 | 최대256개, 고유 payload1MiB 제한, SQL/owner64 경계 |
| 중복 제거 | 배치 내 exact identity, occurrence 확인 참조와 검증 count memo/spill |
| 변경 생략 | unchanged occurrence와 COW root 재사용 |
| 저장 표현 | empty/singleton inline, Small/Block 등 기존 표현 |
| child 검증 | 배치 내 참조 short-circuit와 transaction 내부 exact set |
| 파일 생성 | nonreplacing fresh claim으로 중복 anchor 관측 제거 |
| allowance | 공유 트리에 별도 accounting-only 검증 변경 존재. 기존 담당의 hunk |
| checkpoint | 기존8-page 처리와 WAL batching. FULL/auto1000 계약 유지 |
| trace 소비 | Python의 단일 Operation-root 가정을 unique IO-summary root 선택으로 수리 |

일반적인 배치·dedup·캐시 추가를 새 결손으로 발급하지 않는다.
공유 파일에 이미 존재하는 allowance 변경을 이 RCA의 신규 수리로 복제하지 않는다.

## 호스트 영향과 아직 구분되지 않은 부분

C17 Lexical의 wall−호출 스레드 CPU는71.150초다. 스케줄 대기, 동기 I/O,
락·blocking과 다른 스레드의 작업이 섞일 수 있다. 완료 시 호스트는16 logical CPU에
load108.46이었고 swap 사용도 관측됐다. foreground Catalog mutex wait는129회 합계
59,792ns로, 그 mutex가 전체 지연의 주원인이라는 근거는 없다.

호스트 경합은 기여 가능성이 높은 조건이다. 그 기여율, device service time,
건강한 호스트에서의 전체4K 완료 시간은 현재 trace로 확정되지 않는다.
C16/C17의 진행 prefix와 환경이 달라 총시간이나 객체당 CPU로 수정 효과를 단정할 수 없다.
Medium과 Large의 파일 크기·term·객체 분포도 다르다.
현재 generator는 Medium80행, Large120행의 본문 loop와 각각3개 고정 행을 쓴다.
총 생성 행은21,248→503,808로 약23.71배다. hit/symbol의 누적 threshold도 다르다.
입력 근거는 `scale.rs`의 TIER_MEDIUM/TIER_LARGE와 generate_scoped_corpus/generate_file,
과거 bytes는 VALIDATION의 C13IO1과 C17 Medium 종료 기록이다.

## 측정·완료 판정에서 생긴 별도 문제

Python child verifier는 Operation root가 하나라고 가정했다. 실제 Native trace는
lifecycle과 Catalog 설정의 독립 root를 갖는다. Rust verifier는 이미 이를 허용했다.
Python도 complete IO summary를 가진 root 하나를 선택하고 그 아래의 성공한 boot를
확인하도록 수리했다. split/incomplete/duplicate summary와 실패한 lifecycle은 거절한다.

C17 Medium 생산자는26.892초에 종료0, 전체 ACK15.845초와 검색·재오픈을 완료했다.
기존 wrapper의 FAILED는 보존했고 같은 원본의 제품·canonical offline 검증이 통과했다.
487개 영향 테스트 통과·2개 skip과 고정 consumer의12개 테스트 통과는 이 소비자 수리의
근거이며 Large 성능 개선 근거로 사용하지 않는다.

### RR 후속 검증기 종료 순서 수리

종료 이벤트 뒤의 sync counter가 종료 시각과 동일한 timestamp를 가지면
Python이 받아들이는 반례를 확인했다. Rust는 Operation root가 아직 열려 있어야
그 counter를 수용한다. 기존 Python 회귀는 timestamp가 더 늦은 경우를 다뤘다.

canonical parser가 Native I/O summary의 세 key를 처리할 때 Operation root가 이미
종료됐는지 확인하도록 수리했다. summary reader는 같은 counter key 집합을 재사용한다.
writer 이벤트 순서가 보존되는 parser에서 거절하므로 timestamp가 같아도 판정할 수 있다.

지원 스키마2–5, first/reopen, 세 counter의24개 종료 후 반례를 모두 거절하고,
같은 범위의 종료 직전 동일 시각24개 입력은 허용한다. legacy schema1의 기존 계약도
독립 회귀로 확인했다. 기존 다중 Operation root와 손상 회귀까지 집중65개가 통과했다.
아래 영향 배치는1,024개 통과·2개 skip,21.57초로 끝났다.

```sh
.venv/bin/python -B -m pytest -p no:cacheprovider -q --tb=short \
  tools/ci/tests/test_causal_cost_timeline.py \
  tools/ci/tests/test_causal_cost_profile.py \
  tools/ci/tests/test_causal_cost_capture.py \
  tools/ci/tests/test_causal_cost_canonical.py
```

skip2개는 prebuilt 실행에 managed-build receipt가 없는 기존 조건이다.
Ruff E9/F와 scoped diff 검사도 통과했다. 이 수리는 offline 검증의 거절 계약을
닫았다. 이후 `lr4`는 다른 고정 소스의 Large 실패 prefix를 다시 측정했지만
scheduler/I/O 기여율과 원120초 수용은 여전히 미확정이다.

날짜별 계획과 상태를 계속 추가하면 다른 source의 실패·코드 수리·미실행 수용이
섞인다. 또한 원본을 휘발 가능한 캐시 경로로만 인용하면 캐시가 없어진 뒤 그대로
재검증할 수 없다. 당시 VERIFIED를 취소하는 사유와 현재 replay가 BLOCKED인 사유를
구분해야 한다. 이 폴더는 현재 판단과 후속 패킷을 소유하고 과거 실행은 이력으로 인용한다.

## 최종 판단과 필요한 해결 방향

### B02에서 드러난 진단 context의 기존 계약 불일치

WorkBudget의 `scoped_diagnostic_context(None)`은 기존 context를 상속한다.
Lexical 호출부는 이를 명시 mask로 설명했으나 실제로는 Diff span 생성 실패 뒤에도
하위 read가 외부 context를 다시 사용할 수 있었다. B02가 새로 만든 회귀가 아니라
계수 회귀를 추가하면서 드러난 기존 seam이다.

inherit 계약을 바꾸지 않고 Core에 `scoped_diagnostic_override(Some/None)`을 추가했다.
None은 명시 mask, Some은 교체이며 기존 scoped request의 RAII 복원을 공유한다.
제품 요청 identity·deadline·권한·사용량은 그대로다. Core331개와 Text154개가 통과했고,
Diff admission 실패 oracle도 제품 결과·work 동등성과 하위 trace 차단을 확인했다.
이 수리는 관측의 귀속을 바로잡으며 인덱싱120초 비용 개선의 실측 근거는 아니다.

- 직접 실패 원인은 인덱싱을 포함한 단일 SourceAdvance 예산의 소진이다.
- 토큰화보다 객체 설치·검증 경로의 실측 비용이 크다.
- 구조적 작업 증폭은 확인됐으며, 안전하게 제거할 개별 작업은 계약과 계수로 입증해야 한다.
- 120초 초과의 불가피성, 호스트 안정화만으로의 통과, 최신 전체 소스의 수용은 미입증이다.
- 직전 감사에서는 Text·Storage·Native dispatch·kernel update의 주요 파일이 C17과 같았다.
  과거 C17 고정 컷이 없어 그 byte 비교를 갱신할 수 없다. 새 고정본의 수용은 별도로 필요하다.

[WORKPLAN](WORKPLAN.md)은 확인된 r4 실패를 기준으로 B03-B1 owner-local
occurrence block의 codec·producer·reader·정책과 profile-first 초기 Patch 활성화를
합류하고, 고정 원 예산과 독립 oracle로 수용을 판정한다. 효과가 없거나 기존
query/delta 계약을 위반하면 폐기한다.
현재 B02의6개 검증·재사용 계수 생산자와 소비자는 소유 회귀까지 합류했다.
최신 r4의 정상 cached build와 실제 Medium/원 Large Off·Record 캡처는 완료됐고
모두 위 실패로 기록했다. 과거 C17의8,799회 get을
신규 재검증만의 비용으로 재분류하거나 새 성능 수리의 효과로 해석하지 않는다.
