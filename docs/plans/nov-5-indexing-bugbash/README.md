# Native 인덱싱 bugbash

2026-11-09 KST 기준 Native 초기 인덱싱 RCA·후속 구현 SSOT다. 공유 작업 트리에는
다른 담당자의 dirty 변경이 있으며, 아래 검증은 각 frozen source에만 귀속한다. 제품 계약은
[R01–R40 DoD](../oct-22-finalizing/DOD.md)를 따른다.

| 문서 | 역할 |
| --- | --- |
| [RCA](RCA.md) | 실패 원인·실측·인과관계의 한계 |
| [WORKPLAN](WORKPLAN.md) | 현재 계약·코드 소유·검증·채택 조건 |
| 이 README | 현재 판정·다음 실행 |

## 현재 판정

**r50 `050e6280…`의 로컬 Large Off 제품 검증은 `VERIFIED`다.**
4,096 sources·seed42·원 server 120초에서 최초 ACK 27.622초, 첫 검색
120.563ms, 재오픈 검색 59.067ms였다. 증분·삭제·이전/최신 view도 확인했다.
로컬 prebuilt capture의 build binding은 `UNPROVEN`, 정식 matched-host
성능 수용은 `NOT_RUN`이다.

**XL은 `FAILED`다.** r51 Off에서도 32,768-source의 단일 SourceAdvance가
BuildPrepare 중 원 120초에 도달했다. r51 Record의 손실 없는 child trace는
SourceApply 32.029초, BuildPrepare 85.057초다. 최초 ACK·검색·재오픈은
미도달이다. B75 이후 XL 제품 실행은 `NOT_RUN`이다.
[XL RCA](RCA.md#r51-xl-off의-재실패).
**r53 `a6003311…` Mixed Off는 `FAILED`다.** 갱신 ACK를 세 번 받은 뒤
고정 view 검색이 IPC Read deadline 834ms에 도달했다. 같은 요청의 query
preparation은 629ms였고 reader에는 미도달했다. 마지막 pin은 363.641초에
조기 해제됐다. 원 600초 pin과 해제 후 정산은 이 실행에서 검증하지 못했다.
r51은 600.475초에 pin을 해제했고 61회 poll 뒤에도
`UnretiredView`, `ReclaimQueue`, 새 `TerminalCustody`가 남았다.
[Mixed RCA](RCA.md#r53-mixed의-preparation-실패와-실제-custody-진전).

B20 `def73607…`은 dense Posting의 witness를 bounded page에 묶는다. 집중 회귀
253개와 Large Record 제품 경로가 통과했다. 같은 pair의 Off는 원 120초에 실패했다.
디렉터리 형상 거절과 warm 국소 편집을 보완한 `13214505…`는 집중 회귀 254개가
통과했다. 최적화 r30 pair의 Large Off는 최초 ACK 87.815초, 첫 검색 187.5ms,
증분·삭제·이전 pin·재오픈·drain까지 완료했다. 같은 pair의 Record는 최초 ACK를
126.355초에 거절했다. 실패 trace에서 최상위 Map 설치와 Catalog 경계를 확인했다.
[B20 작업 계획](WORKPLAN.md#b03-b20--dense-posting-물리-노드-감소).

B21은 최상위 terms/owners Map의 root별 노드 수와 실제 설치·COMMIT 비용을
분리한다. r31 Record에서 terms 12,394개와 owners 274개가 확인됐다.
Record 수집기의 child trace 중복 파싱을 제거했다. terms 물리 노드 감소는
B22에서 별도로 검증한다. [B21 RCA](RCA.md#b21--최상위-map-물리-노드-귀속과-진단-검증).
B21은 cold inventory의 기존 최대 창 256개를 기본값으로 사용한다. 고정 소스
`60d74310…`의 f72는 새 Storage 테스트의 임시 lock guard lifetime에서 컴파일
실패했다. packed Diff 회귀의 helper shadowing도 확인해 수정했다. `89a062d2…`에서
B21과 packed Diff 신규 회귀가 통과했다. 기존 VFS 테스트 두 개가 쓰기 없는 scope에
빈 계측을 기대해 실패했다. 해당 테스트만 고친 `01c4bb4d…`의 통합 집중 405개가
통과했다. 최적화 r31도 원 Large Off·Record 제품 경로를 통과했다.

B22는 terms root 전용 QIMAP005와 혼합 형식 reader를 공유 작업 트리에 반영했다.
격리 소스 `58d69e24…`의 Rust 회귀 409개와 최적화 r32 Large가 통과했다.
최상위 terms·owners Map의 실제 write와 file sync는 각각 12,668→5,546회,
설치 배치는 50→22회, Catalog COMMIT은 63→28회였다. 공유 작업 트리의 P04
배치 조회 위임은 유지했다.
[B22 계획](WORKPLAN.md#b03-b22--terms-map의-물리-노드-감소).

B25는 r32에서 발견한 부팅 형식 불일치를 수정한다. v18 구버전은 같은 v18
표식 아래 QIMAP005를 읽을 수 없으므로 Native 상태를 v19로 분리했다.
공유 작업 트리에 runtime marker·Catalog stamp·회귀를 반영했다.
[B25 계획](WORKPLAN.md#b03-b25--qimap005-저장소의-구버전-부팅-차단).

B19 `046d2c02…`는 cold inventory와 인증된 단일 `QITXOC01`의 key·ref가
완전히 일치할 때 그 객체를 직접 사용한다. 집중 회귀 252개를 통과했다.
[B19 계약](WORKPLAN.md#b03-b19--단일-packed-occurrence를-초기-inventory로-직접-사용).

B18 `ccd5ab92…`는 Block·Tree contribution을 owners Map에 인라인하고 source
record와 inventory root를 실제 child로 유지한다. 별도 contribution 파일과 해당
Catalog 전이를 제거한다. 새 layout policy로 이전 member를 재인덱싱해야 한다.
Lexical·Kernel 집중 회귀 220개를 통과했다.
[B18 구현·검증](WORKPLAN.md#b03-b18--native-owner-contribution의-별도-파일-제거).

B17은 기본 설치 창을 4배치로 복원했다. gate 경합에서 부분 획득을 해제하고
atomic advance의 논리 예약·확인을 분리해 집계한다.
[B17 리뷰 RCA](RCA.md#b17--설치-창-리뷰-보완).

r32의 원 Large 최초 ACK는 Off 46.419초와 Record 48.192초였다. 이전 제품 기준선은
`0a97e8589daa41b736cb6d59545b0116bb3b89c6`의 r4다. B19 로컬 Off와
Record는 `BuildPrepare` deadline으로 첫 ACK에 실패했다. B16→B18→B19의
첫 Posting 전 파일 write·fsync는 각각 12,288→8,192→4,096회, Catalog COMMIT은
160→128→64회다. B19 Record는 전체 980개 Posting 페이지를 처리하고 최상위
terms/owners Map 물리 설치에서 중단됐다. 같은 pair의 Medium은 ACK·검색·증분·삭제·
재오픈을 완료했다. [B19 실행 RCA](RCA.md#b19-실행-결과).

B14 `60db2fcbdf498783da3b9639a70671b6eaaa23f5`는
cold inventory 물리 배치와 Catalog advance를 묶고 최상위 map scratch
비용을 분리한다. [B14 작업 계획](WORKPLAN.md#b03-b14--cold-inventory-물리-배치와-연속-catalog-advance).
동일 Large cold inventory 입력에서 물리 배치 88→44, Catalog COMMIT 176→88을
확인했다. 공유 Mac의 Off·Record는 모두 원120초에 `BuildPrepare`에서 실패했다.
동일 바이너리의 Medium은 전체 ACK·검색·증분·삭제·재오픈을 완료했다.
Large의 최상위 map 분리 계측은 Record가 해당 단계에 도달하기 전에 종료됐다.
[B14 실측](RCA.md#b14--물리-그룹-축소와-기존-advance-연결).

B15는 inventory posting edit의 중복 인코딩·예약 조회와 물리 writer의 중복
`fstat`을 제거했다. B15 `794468fb…`의 Record는 세 번째 Posting page 물리
설치까지 진행했다. B14의 마지막 거절 지점, 동일 입력 구간의 CPU와 wall,
호스트 관측은 [B15 RCA](RCA.md#b15--inventory-scratch와-물리-writer-병목)에 기록했다.

r5 정상 cached pair build는 완료됐고 Medium은 fixed 512MiB 진단 cap, Large Off/Record는
원120초 deadline에서 각각 실패했다. typed inventory confirmation fee 후보
`a6083fb22480786d5e8172057b6fbe291af26ee7`의 r6 build는 완료됐으나 Medium은
같은 진단 cap에서 다시 실패했다. B03-B2의 8KiB/512-term 구현은 frozen source에 합류했다.
f20 Native는 통과했지만 Kernel은 테스트 helper의 E0507 컴파일 오류로 body0이었다.
테스트 파일2개만 교정한 `a60ac6576a0ca2cae306ee68a014b11c3736246f`의 f21은
새 Native case1개와 Kernel14개를 통과했다. r7 정상 cached build는 완료됐지만
Medium은 fixed512MiB 진단 cap, Large Off는 원120초 deadline에서 실패했다. 후속 B03-B3는
`39f4a9c3bd82843aea2b390411d8ec531a0d3b2c`의 Native297개·Kernel14개 집중
회귀를 통과했다. 테스트 helper 산술만 고친 `996230439fbf187661c7462b018c113c8b3de1bd`의
f25 변경 case1개도 통과했다. f24 strict Clippy는 전역33개 타 owner 진단으로
FAILED지만 Native 진단0이다. r8 정상 cached build는 완료됐고 Medium은 fixed512MiB
진단 cap에서 실패했다. lo8 Off와 lr8 Record도 원120초 deadline에서 실패했다.

후속 B03-B4는 Catalog의 작은 node 배치에서도 전체 child 수에 맞는 기존
transaction-local proof window를 사용하도록 수정했다. Storage의 empty-base CAS
중복 키 변환·버퍼 재할당도 제거했다. frozen `213908833f462447fc229519d1d6f1472e978644`의
f26 집중423개는 통과했다. f27 Storage/Catalog strict는 기존 Storage test 변환
lint2건으로 FAILED, 소유 변경 파일 진단0이다. r9 pair build는 완료됐지만
lo9 Off·lr9 Record도 원120초에서 FAILED다. lr9는 첫64 term page를 완료하고
둘째 page apply에서 멈췄다. 상세 계약과 제외한
4K 단일 Posting Block 안은 [WORKPLAN](WORKPLAN.md#b03-b4--small-node-배치의-transaction-local-child-proof)에 있다.

현재 Main의 B5는 **dense Posting Tree의 비교 witness와 실제 inventory custody를
분리하는 코드**다. producer·query·overlay·정책 gold와 독립 oracle가 합류했다.
`5e41d204…`의 f31 Native301개·Kernel lifecycle15개는 전부 통과했다.
f32 strict는 기존33개+새 진단11개(중복 포함)로 FAILED였다. resolver 타입 별칭과
테스트 borrow만 교정한 현행 `3a2ad5dd…`에서 f35 변경2회귀가 통과했다.
f36 strict는 전역33개 기존 진단으로 FAILED, 소유 파일 진단0이다. f31 전체316개와
composite 집중 proof이며 현행 source의 전체 suite 재실행으로 주장하지 않는다.
r10 pair build는 완료됐다. 같은 source의 lo10 Off와 lr10 Record 모두 원120초
제한에서 FAILED다. B5 기능 검증은 f31/f35 범위다.
[B5 계약](WORKPLAN.md#b03-b5--dense-posting-tree의-비교-witness와-실제-custody-분리)을 따른다.

B6는 빈 base의 희소 Posting 준비 조회를 묶고 기존 shard의 반복 생성 호출을
제거한다. Storage73개와 수정된 `c7b508a…`의 Native303개·lifecycle2개가
통과했다. 현행 `c5c691e…`의 f41 변경2개도 통과했다. f42 strict는 기존
35개로 FAILED이며 이번 변경 파일의 진단은0개다. r11 pair build는 완료됐고,
공용 실행 lease의 lo11 Large Off는 원120초 deadline에서 FAILED다.
[B6 계약](WORKPLAN.md#b03-b6--희소-posting-준비-조회와-기존-shard-생성-호출).

B7은 빈 term root에서 같은 접미사의 두 case를 한 페이지에 놓고 Storage의
기존 동일 편집 재사용을 활용한다. `cf318f0…`의 f43 Native305개와 lifecycle2개가
통과했다. f44의 모듈 주석 진단은 Rust 본문 변경 없이 교정했다. f45·r12·lo12와
후속 lr12 Record의 판정은 아래 표에 있다. [B7 검증 이력](WORKPLAN.md#b7-검증-및-분기),
[B8 재검증](RCA.md#b8-재검증), [B9](WORKPLAN.md#b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기)는 선행 작업이다.

Medium fixed1M/512MiB는 harness 기본 **진단 profile**이다. E07의 Large 제품 조건은
별도 explicit 67,108,864 reads/writes/mutations·549,755,813,888B logical funding과
server dispatch120초, full ACK·search/reopen이다. ANN4K의1M reads/512MiB 요청 조건도
별개다. Medium의 cap 거절은 그 진단 실행의 FAILED이며 Large 실행 입장 거절이나
제품 P0 실패 판정의 선행조건이 아니다. 논리 work cap은 RSS·disk 상한도 아니다.

B18은 Lexical·Kernel 5개 파일이며 f68·r27 고정본은 `ccd5ab92…`다. B19는
Lexical·Kernel 14개 파일이며 f69·r28 고정본은 `046d2c02…`다. 공유 Main의
다른 작업 변경과 전체 workspace qualification은 별도 범위다.

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
| B03-B1 candidate-source03 | VERIFIED | 당시 source freeze에서 Root는 공유 HEAD/index를 수정하지 않고13개 파일 local snapshot `d8e2c52…`를 만들었다. 현재 Main HEAD 불변 주장 아님 |
| r5 정상 cached pair build | VERIFIED | `candidate-build-03.py` build exit0, source `d8e2c52d…`, scale/daemon SHA 기록. managed-fresh·release 자격은 아님 |
| mr5 Medium fixed512MiB Record | FAILED | 실제 full_event_ack 거절: 536,866,122B 사용 후 14,133B charge 실패, `BuildPrepare`; producer exit1, 외부 timeout 없음. child 39,223 events 완결. full ACK 없음 |
| lo5 Large 원120초 Off·새 성능 효과 | FAILED | 같은 r5 pair로 원 seed42/cap/120초 실행: request124.436초에서 `BuildPrepare` deadline, producer226.899초·exit1. numeric cap·memory·external-resource 거절 없음. full ACK 없음 |
| lr5 Large 원120초 Record | FAILED | 요청120.654초 deadline, producer exit1·full ACK 없음. child trace는858,005 events 기록/4,698 dropped/7 open으로 PARTIAL; 도달 prefix만 분석 |
| 최신 typed confirmation fee 후보 집중 회귀 | VERIFIED | `a6083fb…`의15개 소유 파일 local snapshot. f18 `c49b2a1…` E0716 compile FAILED/body0 뒤 test-only 교정. f19 Native293/293·lifecycle13/13 통과, 각558/65 제외. 제품 결과는 별개 |
| r6 정상 cached pair build | VERIFIED | `a6083fb…` source의 build exit0. managed-fresh·release 자격은 아님 |
| mr6 Medium fixed512MiB Record | FAILED | 실제 full ACK 전 536,864,032B 사용 후 10,692B charge가 Catalog managed_install에서 거절. producer exit1. child 45,923 events 완결, I/O 실패0 |
| a608 Large 원120초 | NOT_RUN | 새 pair의 Large 실행 결과 없음. 이전 d8e lo5/lr5 실패를 새 후보 결과로 승격하지 않음 |
| B03-B2 f20 Native 집중 회귀 | VERIFIED | frozen `878be6c9…`에서295개 통과·558개 제외. Kernel과 제품 수용은 별개 |
| B03-B2 f20 Kernel 집중 회귀 | FAILED | 테스트 helper `occurrence_blocks.rs:56`의 E0507 컴파일 실패, 실행 body0. 새 source로 교정 |
| B03-B2 f21 집중 회귀 | VERIFIED | `a60ac657…`에서 Native 새 distinct-vector case1개 통과·853개 제외, Kernel14개 통과·65개 제외. f20 대비 제품 코드 불변·테스트2파일 교정; composite local proof이며 정확 source의 전체 Native suite 아님 |
| B03-B2 r7 정상 cached build | VERIFIED | `a60ac657…`의 build exit0. l10ctl은 build/tool/env/source 일치를 확인. managed-fresh·release 아님 |
| B03-B2 mr7 Medium fixed512MiB Record | FAILED | 536,869,205B 사용 후12,164B Catalog `managed_install.rs:108` charge 거절. producer exit1·full ACK 없음, physical2147 writes/3,680,139B·I/O 오류0 |
| B03-B2 lo7 Large 원120초 Off | FAILED | producer157.629초·exit1, full-event ACK 미완료. 요청124.194초에서 Catalog `managed_install.rs:151` deadline, cap·memory·external 거절 없음 |
| B03-B3 QITXEM02 f22 집중 회귀 | VERIFIED | external `39f4a9c3…` 19개 소유 입력/8개 변경. 정확 source Native297개 통과·558개 제외(3.371초), Kernel14개 통과·65개 제외(114.062초), 두 terminal exit0 |
| B03-B3 f23 lexical strict Clippy | FAILED | `39f4a9c3…` `--keep-going` exit101·전역38개=타 owner33개+새 Native test helper5개 |
| B03-B3 f24 strict Clippy | FAILED | `99623043…` 테스트 helper 산술1파일 교정 뒤 전역33개 타 owner 진단으로 exit101, Native 진단0 |
| B03-B3 f25 변경 case | VERIFIED | `99623043…` Native 변경 case1개 통과·853개 제외, source guard0. f22 Native297·Kernel14와 composite 집중 proof이며 Native298개로 합산하지 않음 |
| B03-B3 r8 정상 cached build | VERIFIED | `99623043…` build exit0. managed-fresh·release 자격 아님 |
| B03-B3 mr8 Medium fixed512MiB Record | FAILED | 536,864,010B 사용 후9,988B Catalog `managed_install.rs:108` 거절. producer15.888초·exit1, full ACK 없음. child49,370 events 완결·loss/open/lease0, 물리2364 writes/2,256,813B·I/O 오류0 |
| B03-B3 lo8 Large 원120초 Off | FAILED | `99623043…` producer159.174초·exit1, full ACK124.654초 미완료. `BuildPrepare`의 Storage `map/completed_group.rs:140` deadline; 15,381,374R/8,331,877W/9,933,434,999B/3,283,526M, numeric·memory·external 거절 없음 |
| B03-B3 lr8 Large 원120초 Record | FAILED | 같은 source·binary pair, producer185.890초·exit1, full ACK125.721초 미완료. 같은 `BuildPrepare`/`completed_group.rs:140` deadline. child841,911 events 완결·loss/open/lease0, 물리31,630 writes/43,770,402B·I/O 오류0 |
| B03-B4 f26 집중 회귀 | VERIFIED | frozen `21390883…`, Storage85·Catalog27·Native297·Kernel lifecycle14 전부 통과. 각각86·278·558·65 제외. 새 SQL128-child oracle 포함, 전체 workspace·제품 수용 아님 |
| B03-B4 f27 Storage/Catalog strict Clippy | FAILED | 같은 source lib+tests·keep-going exit101. 기존 `object/read_only/tests.rs` 변환 lint2건, 소유 변경 파일 진단0. 전체 strict green 아님 |
| B03-B4 r9 정상 cached build | VERIFIED | `21390883…` pair build exit0·4분28초. source/config/dependency/tool guard 유지, prebuilt `UNPROVEN`·정식 qualification 아님 |
| B03-B4 mr9 Medium 진단 | FAILED | 실제 mr9 producer18.446초·ACK9.282초 미완료. 536,863,832B 뒤9,968B charge 거절. 과거 mr8 summary 경로 오류는 외부 correction으로 정정 |
| B03-B4 lo9 Large120 Off | FAILED | producer163.804초·ACK125.522초 미완료. completed_group.rs:140 deadline, numeric·memory·external 거절 없음. 실제 lo9 원본 사용 |
| B03-B4 lr9 Large120 Record | FAILED | producer158.351초·ACK124.534초 미완료. samplerOff,896,891 events COMPLETE·loss/open/lease0. 첫 page 완료·둘째 apply deadline; full ACK/search/reopen 미도달 |
| B03-B5 f28 Native compile | FAILED | `25ea314…` 새 adapter의 readonly OrdinalMapEdit 직접 수정과 helper Result 누락 등4 compiler 진단·body0. 원본 immutable cursor와 funded physical copy로 교정 |
| B03-B5 f30 Native 회귀 | FAILED | `cc928411…` compile 통과,298pass/3fail·558 제외. 세 fault fixture가 Posting Tree를 ObjectRef로 읽던 setup 실패; 실제 inventory에서 fault 대상을 읽도록 수정, corruption/cancel 기대값 유지 |
| B03-B5 f31 Native 회귀 | VERIFIED | `5e41d204…` Native301개 통과·558 제외. witness wire·CAS·9→8→9와 기존 corruption/cancel 포함. Kernel·제품 실행과 별개 |
| B03-B5 f31 Kernel lifecycle | VERIFIED | 같은 source15개 통과·65 제외. 새 실제 FileStore/Catalog9→8→9·query positions·old pin·GC·reopen 포함; 전체 workspace·Large 수용 아님 |
| B03-B5 f32 lexical strict | FAILED | 같은 source lib+tests keep-going exit101, 기존33개+소유11 raw 진단(고유9개). callback 복잡도·테스트 복사/lock scope를 교정 |
| B03-B5 f33 변경 회귀 compile | FAILED | `0fd71c5…` 테스트 helper의 owned/borrowed 인자 순서1건 E0308·body0. 제품 알고리즘 변경 없이 수정한 `3a2ad5dd…`로 재검증 |
| B03-B5 f35 변경 회귀 | VERIFIED | `3a2ad5dd…` witness 전환·CAS/malformed/late-stream2개 통과·857 제외. f31 Native301·Kernel15와 composite local proof; 추가2개를 Native303으로 합산하지 않음 |
| B03-B5 f36 lexical strict | FAILED | 같은 source 전역33개 기존 진단·exit101, 소유 변경 파일0. f32의 신규 raw11개 교정. 전체 strict green 아님 |
| B03-B5 r10 정상 cached build | VERIFIED | `3a2ad5dd…` pair build exit0·6분31초. source/config/dependency/tool 확인, prebuilt `UNPROVEN` |
| B03-B5 mr10 Medium 진단 | FAILED | producer11.970초·ACK5.073초 미완료. 536,868,054B 뒤 text spool의 기존 4,096B charge 거절 |
| B03-B5 lo10 Large120 Off | FAILED | producer154.318초·ACK123.507초 미완료. BuildPrepare의 `object.rs:628` deadline. 수치·메모리·외부 자원 거절 없이 전체 ACK·검색·재오픈 미도달 |
| B03-B5 lr10 Large120 Record | FAILED | producer168.426초·ACK127.683초 미완료. BuildPrepare의 `object.rs:863` deadline. sampler Off. 정상 child footer 890,079 events·loss/open0 |
| B03-B6 f37 Storage / Native compile | VERIFIED / FAILED | `16a2610…` Storage object73개 통과·98 제외. Native는 optional 메모리 lease를 다시 감싼 E0308·body0. Storage 입력을 유지한 채 수정 |
| B03-B6 f39 Native / lifecycle | VERIFIED | `c7b508a…` Native303개·558 제외, lifecycle2개·78 제외. 희소 SQL 실행 횟수, dense fallback, 순서·EOF, old pin·reopen 포함 |
| B03-B6 f40 strict | FAILED | `c7b508a…` 기존35개와 lease 필드 초기화 진단2개. 구조체 초기화 시 rows와 lease를 함께 넘기도록 수정 |
| B03-B6 f41 변경 회귀 | VERIFIED | `c5c691e…` 희소 prepass·상한 fallback2개 통과·859 제외. f37/f39 집중 검증과 합성 근거 |
| B03-B6 f42 strict | FAILED | `c5c691e…` Lexical·Storage lib+tests 기존35개·exit101. 이번 변경 파일0개 |
| B03-B6 r11 정상 cached build | VERIFIED | `c5c691e…` pair build exit0·4분45초. source/config/dependency/tool 확인 |
| B03-B6 lo11 Large120 Off | FAILED | producer155.742초·ACK123.772초 미완료. BuildPrepare의 `managed_install.rs:598` deadline. 원 예산 유지, 공용 실행 lease, 전체 ACK·검색·재오픈 미도달 |
| B03-B6 lr11 Large Record | NOT_RUN | 공용 lease 대기에서 자체 취소, producer0. 새 B7 검증으로 이동 |
| B03-B7 f43 Native / lifecycle | VERIFIED | `cf318f0…` 305개·2개 통과, 각각 558개·78개 제외. 페이지 경계·범위·순서·EOF, pin·reopen 포함 |
| B03-B7 f44 strict | FAILED | `cf318f0…` 전역37개. 소유 모듈의 첫 doc 문단 길이 진단1개가 lib/test에서 중복 보고됨 |
| B03-B7 f45 strict | FAILED | doc-only `e3e8512…` exit101. 기존 전역35개, 소유 진단0개. r12 build로 진행 |
| B03-B7 r12 정상 cached pair build | VERIFIED | `e3e8512…` exit0. source/config/dependency/tool 재검사와 두 실행 파일 해시 확인 |
| B03-B7 lo12 Large120 Off | FAILED | `e3e8512…` producer160.101초·ACK124.763초 미완료. `object.rs:593`의 BuildPrepare deadline. 전체 ACK·검색·재오픈 미도달 |
| B03-B7 후속 trace 소비자 | VERIFIED | `map_apply` 집계·설치 phase 소비자 집중 Python 15개. Rust와 실제 capture는 별도 |
| B03-B7 r13 진단 pair build | VERIFIED | `654d204…` exit0. Core·Storage·Searchd trace 변경 포함, 두 실행 파일 고정 |
| B03-B7 lr12 Large Record | FAILED | producer159.261초·ACK124.740초 미완료. 정상 child footer 923,752 events·loss/open0, 실제 재사용64 확인 |
| B03-B8 f46 집중 컴파일 | FAILED | `58ce478…`에 Main의 phase-count test fixture 파일이 누락돼 E0308, body0. 실패 terminal 보존 |
| B03-B8 f47 집중 회귀 | FAILED | `6ef1167…` 10개 실행, B8 손상·EOF 및 Storage/Core 포함 9개 통과. 기존 Rust/schema sync label 순서 oracle 1개 실패 |
| B03-B8 f48 schema oracle | VERIFIED | `6df7803…`에서 실패한 1개 통과·191개 제외. f47의 Rust 본문 동일·sync 허용값 동일, 순서만 수정 |
| B03-B8 r14 정상 cached pair build | VERIFIED | `6df7803…` exit0. source/config/dependency/tool 재검사 및 두 실행 파일 고정 |
| B03-B8 lo13 Large120 Off | FAILED | producer149.444초·ACK123.344초 미완료. Posting cardinality loop에서 BuildPrepare deadline, 원 예산 유지. 전체 ACK·검색·재오픈 미도달 |
| B03-B9 f49 trace 소비자 | VERIFIED | `ac5f198…` Python 집중31개, 485개 제외. `managed_object_advance`와 V4 격리 포함 |
| B03-B9 f49 Rust | FAILED | `ac5f198…` scalar 테스트 fixture의 새 필드 초기화 누락 E0063, body0. 기존 결과 보존 |
| B03-B9 f50 Rust | FAILED | fixture를 고친 `572f432…`에서 사전검증 값의 명시적 폐기 누락으로 `unused-results`, body0 |
| B03-B9 f51 Rust 집중 회귀 | VERIFIED | `d2c6ff8…` 39개 통과·968개 제외. atomic rollback/retry/reopen, 둘째 파일 미생성, barrier/device, map·trace 포함 |
| B03-B9 r15 정상 cached pair build | VERIFIED | `d2c6ff8…` exit0, 소스·설정·의존성·도구·두 바이너리 확인 |
| B03-B9 lo14 Large Off | FAILED | `d2c6ff8…` producer159.143초·ACK123.651초 미완료. BuildPrepare deadline, 전체 ACK·검색·재오픈 미도달 |
| B03-B9 lr13 Large Record | FAILED | producer156.479초·ACK123.934초 미완료. child COMPLETE 1,009,588 events·loss/open0. 첫 Posting page의 동일 5,871 input에서 COMMIT46→35 |
| B03-B10 f52 trace 소비자 | VERIFIED | `7cedcb7…` Python 집중34개·485개 제외. writer 세 phase 포함 |
| B03-B10 f52 Rust 집중 회귀 | VERIFIED | `7cedcb7…` 16개 통과·1,215개 제외. 65 contribution의 1회 물리 설치·2개 SQL page, writer phase·symlink/device·B9 custody |
| B03-B10 r16 정상 cached pair build | VERIFIED | `7cedcb7…` exit0. 두 바이너리와 소스·설정·의존성·도구 검증 |
| B03-B10 lo15 Large Off | FAILED | producer169.385초·ACK125.545초 미완료. BuildPrepare deadline. 검색·재오픈 미도달 |
| B03-B10 lr14 Large Record | FAILED | ACK124.883초 미완료. child partial drop485/open1. 4,096 contribution의 관측 설치64→16, writer ticket 준비7.011초 |
| B03-B11 f53 Runtime writer 집중 | VERIFIED | `add8f4c…` 12개 통과·508개 제외. 생성 전 전체 charge, 미생성 거절, 취소 환급, 실제 Catalog replay |
| B03-B11 r17 정상 cached pair build | VERIFIED | `add8f4c…` exit0. 두 실행 파일과 소스·설정·의존성·도구 확인 |
| B03-B11 lo16 Large Off | FAILED | producer160.818초·ACK124.449초 미완료. BuildPrepare deadline. 원 기준 유지, 검색·재오픈 미도달 |
| B03-B11 lr15 Large Record | FAILED | producer155.661초·ACK124.080초 미완료. child COMPLETE 1,005,615 events·loss/open0. 동일 첫 Posting page의 ticket 준비 CPU2.507→1.334초 |
| B03-B12 f54 Runtime·Storage 집중 | FAILED | `8f114a2…` 18/19개. retarget retry가 allowance 종료 전에 snapshot을 비교함. directory sync identity 보강과 함께 교정 |
| B03-B12 f55 Runtime·Storage 집중 | VERIFIED | `2e19d9d…` 23개 통과·676 제외. shard ABA, 잘못된 objects FD 거절과 pending 보존, 생성 취소·원본 FD, scalar/batch·Catalog replay |
| B03-B12 r18 정상 cached pair build | VERIFIED | `2e19d9d…` exit0. source/config/dependency/tool 전후 확인과 두 실행 파일 해시 결속 |
| B03-B12 lo17 Large Off | FAILED | producer156.646초·ACK124.819초 미완료. Storage batch settlement 뒤 checkpoint에서 BuildPrepare deadline. 검색·재오픈 미도달 |
| B03-B12 lr16 Large Record | FAILED | producer179.860초·ACK147.588초 미완료. BuildPrepare deadline. Child COMPLETE 1,100,192 events·loss/open0. 동일 세 Posting page에서 writer 경계 CPU7.004→12.928초 |
| B03-B13 f56 집중 컴파일 | FAILED | `3a94726…`에서 ANN fault 테스트의 `PathBuf` import 누락, body0. 테스트 import만 추가한 `44486c9…`에서 재검증 |
| B03-B13 f57 Runtime·Storage 집중 | VERIFIED | `44486c9…` 23개 통과·676 제외. retarget 시 create 거절·양쪽 namespace 무변경·환급, B12 fsync identity와 Catalog replay 포함 |
| B03-B13 r19 정상 cached pair build | VERIFIED | `44486c9…` exit0. source/config/dependency/tool 전후 및 두 실행 파일 해시 확인 |
| B03-B13 lo18 Large Off | FAILED | producer153.303초·ACK124.150초 미완료. Storage batch settlement 뒤 BuildPrepare deadline. 검색·재오픈 미도달 |
| B03-B13 lr17 Large Record | FAILED | producer153.167초·ACK124.491초 미완료. 최상위 terms/owners map install에서 BuildPrepare deadline. Child COMPLETE 1,203,874 events·loss/open0 |
| B03-B14 f58 집중 컴파일 | FAILED | `b4c8b57…`의 Storage 테스트에서 `OrdinalMapEdit` 생성자 오류·body0. Packed alias ordinal 가드와 함께 `98d496b…`에서 교정 |
| B03-B14 f59 집중 컴파일 | FAILED | `98d496b…`의 64MiB 초과 테스트에서 불변 등록값 직접 수정·body0. `c5d16a3…`에서 기존 생성자로 교정 |
| B03-B14 f60 집중 회귀 | VERIFIED | `c5d16a3…` 237개 통과·1,637 제외. cold ordinal·4그룹 gate/실패·합계64MiB 거절·실제 Catalog advance 포함 |
| B03-B14 r20 build admission | BLOCKED | 공용 lock pathname이 삭제돼 exit126·컴파일0. 삭제된 inode의 기존 보유가 끝난 뒤 같은 고정 소스로 공용 경로를 재개 |
| B03-B14 r21 최적화 pair build | VERIFIED | `c5d16a3…` exit0. source/config/dependency/tool 전후와 daemon/harness 두 바이너리 해시 확인 |
| B03-B14 lo19 Large Off | FAILED | producer76.573초·ACK17.280초 미완료. `BuildPrepare`에서 WorkBudget 거절 없이 `ResourceLimited`. cold block staging의 별도64개 상한을 교정 |
| B03-B14 mb14 Medium Record 진단 | FAILED | r21의 별도256-source 실행. ACK1.709초 미완료. occurrence128개 한 그룹이 설치된 뒤 InventoryMapApply 거절. Child COMPLETE 17,472 events·loss/open0 |
| B03-B14 f61 교정본 집중 | VERIFIED | `60db2fc…` 237개 통과·1,637 제외. cold128 staging 승인, 일반64개 상한, packed alias·합계64MiB·4그룹 실패 및 Catalog advance |
| B03-B14 r22 build custody | BLOCKED | Cargo 컴파일 종료 뒤 공용 lock pathname 소실·exit126. 캐시된 pair는 같은 소스의 공용 build 재확인에 사용 |
| B03-B14 r23 build artifact | BLOCKED | 동일 고정본 Cargo exit0, 두 executable이 cache fresh여서 fresh artifact 수집기가 거절. 전용 cache의 해당 bin fingerprint만 분리하고 pair를 재생성 |
| B03-B14 r24 최종 pair build | VERIFIED | `60db2fc…` exit0, 두 bin 재생성 및 source/config/dependency/tool·두 바이너리 해시 확인 |
| B03-B14 lo20 Large Off | FAILED | producer168.849초·ACK123.633초 미완료. cold staging 통과 뒤 Storage의 batch namespace checkpoint에서 원 BuildPrepare deadline. 검색·재오픈 미도달 |
| B03-B14 lr19 Large Record | FAILED | producer167.910초·ACK125.878초 미완료. inventory edit scratch의 원 BuildPrepare deadline. Child COMPLETE 609,309 events·loss/open0. Posting·최상위 map·검색·재오픈 미도달 |
| B03-B14 mb15 Medium 기능 | VERIFIED | 동일 r24 pair, seed42·256-source. producer exit0·27.317초. full ACK7.931초, 첫 검색230ms, 증분·삭제·no-op ACK, 재오픈·검색·삭제 유지 및 최종 drain 완료 |
| B03-B14 mb15 원 capture | FAILED | 구형 collector가 `map_apply`의 thread CPU lane을 거절. producer exit0과 별개인 capture CLI exit2는 원 기록에 보존 |
| B03-B14 v5 collector·오프라인 trace 재검증 | VERIFIED | timeline pytest 529개 통과. 보존된 mb15 first/reopen 모두 COMPLETE·loss/open0, 각각 80,832/1,875 events. 소스·단계 수용은 위 Medium 기능 범위에 한정 |
| B03-B15 f62 Scratch·Storage 집중 회귀 | VERIFIED | `794468fb…` 37개 통과·171 제외. 64행 정확한 byte 과금, page cap·취소, no-replace·batch barrier·managed advance 포함 |
| B03-B15 r25 최적화 pair build | VERIFIED | `794468fb…` Cargo exit0·15분29초. source/config/dependency/tool과 daemon/harness 두 바이너리 해시 확인 |
| B03-B15 lo21 Large Off | FAILED | producer173.977초·full ACK125.313초 미완료. Storage batch namespace checkpoint의 `BuildPrepare` deadline. 검색·재오픈 미도달 |
| B03-B15 lr20 Large Record | FAILED | producer171.040초·full ACK125.649초 미완료. inventory 완료 뒤 세 번째 Posting page 물리 설치에서 `BuildPrepare` deadline. Child COMPLETE 1,045,729 events·loss/open0 |
| B03-B15 mb16 Medium 기능 | VERIFIED | 동일 r25 pair, seed42·256-source. producer exit0·13.925초, product artifact 검증 완료. full ACK4.789초, 첫 검색65.5ms, 증분·삭제·no-op ACK, 재오픈·검색·삭제 유지 및 drain 완료 |
| B03-B15 mb16 Record 진단 | VERIFIED | canonical profile 검증과 first/reopen COMPLETE·loss/open0, 각각 61,916/1,739 events. prebuilt capture의 정식 성능 수용은 `NOT_RUN`이며 capture 전체 상태는 `PARTIAL_DIAGNOSTIC` |
| B03-B16 f63 Storage·Catalog 집중 회귀 | VERIFIED | `adf4c8ed…` 9개 통과·474 제외. 8개 gate union·9개 거절·64MiB 한도·후반 advance 실패 prefix와 실제 Catalog 원자 전이 확인 |
| B03-B16 r26 최적화 pair build | VERIFIED | `adf4c8ed…` Cargo exit0·8분03초. source/config/dependency/tool과 daemon/harness 두 바이너리 해시 확인 |
| B03-B16 lo22 Large Off | FAILED | producer172.751초·full ACK126.587초 미완료. posting cardinality 조회의 `BuildPrepare` deadline. 검색·재오픈 미도달 |
| B03-B16 lr21 Large Record | FAILED | producer176.526초·full ACK126.392초 미완료. 첫 Posting page의 radix 구축에서 `BuildPrepare` deadline. 물리 설치·검색·재오픈 미도달. Child COMPLETE 834,775 events·loss/open0 |
| B03-B16 mb17 Medium 기능 | VERIFIED | 동일 r26 pair, seed42·256-source. producer exit0·22.170초, product artifact 검증 완료. full ACK8.035초, 첫 검색250.2ms, 증분·삭제·재오픈과 검색·drain 완료 |
| B03-B16 mb17 Record 진단 | VERIFIED | canonical profile과 first/reopen trace가 검증됐다. 69,876/2,539 events·loss/open0. 정식 성능 수용은 `NOT_RUN`이며 capture 전체 상태는 `PARTIAL_DIAGNOSTIC` |
| B03-B17 f65 Storage·Catalog 집중 회귀 | VERIFIED | `366ebb5…` 18개 통과·468 제외. 4배치와 5배치 사전 거절, 경합 gate prefix 해제, 중간 실패·barrier·논리 예약/확인 수 및 실제 Catalog advance 포함 |
| B03-B17 최신 소스의 원 Large120·동시 요청·저메모리 E2E | NOT_RUN | B16 원 Large 결과는 `FAILED`. B17 집중 테스트와 별도 범위 |
| B03-B18 f66 Lexical·Kernel 집중 회귀 | FAILED | `5324009a…` 216개 통과·4개 실패·728 제외. 네 실패는 Tree wrapper를 기대한 기존 테스트 경계다. |
| B03-B18 f67 Lexical·Kernel 집중 회귀 | FAILED | `9808ca57…` 테스트 helper의 cursor edit move에서 E0507. 실행 본문 0개. |
| B03-B18 f68 Lexical·Kernel 집중 회귀 | VERIFIED | `ccd5ab92…` 220개 통과·728 제외. Block·Tree wire와 wrapper 부재, 혼합 fallback·취소·rename·pin·GC·재오픈 포함. |
| B03-B18 r27 최적화 pair build | VERIFIED | `ccd5ab92…` Cargo exit0·5분34초. source/config/dependency/tool과 두 바이너리 해시 확인. |
| B03-B18 lo23 Large Off | FAILED | producer178.480초·full ACK127.637초 미완료. Storage `map/completed_group.rs:140`의 `BuildPrepare` deadline. numeric·memory·external 거절 없음. 검색·재오픈 미도달. |
| B03-B18 lr22 Large Record | FAILED | producer189.990초·full ACK127.996초 미완료. 첫 Posting page 설치 중 Storage `object.rs:862`의 `BuildPrepare` deadline. child footer COMPLETE·889,561 events·loss/open0. |
| B03-B18 mb18 Medium 기능 | VERIFIED | 동일 pair·seed42·256 sources. producer61.220초·exit0·product artifact 검증. full ACK25.024초·첫 검색468.5ms, 증분·삭제·no-op ACK, 재오픈·검색·삭제 유지·drain 완료. |
| B03-B18 mb18 Record 진단 | VERIFIED | canonical profile과 first/reopen trace 검증. 각각 117,706/3,387 events·loss/open0. prebuilt capture는 PARTIAL_DIAGNOSTIC, 정식 성능 수용은 NOT_RUN. |
| B03-B19 f69 Lexical·Kernel 집중 회귀 | VERIFIED | `046d2c02…` 252개 통과·702 제외. 단일 512-key packed inventory, 다중 Tree, 정확한 mismatch·subset·malformed, 증분 Block fallback·pin·GC·재오픈 포함. |
| B03-B19 r28 최적화 pair build | VERIFIED | `046d2c02…` Cargo exit0·7분43초. source/config/dependency/tool과 두 바이너리 해시 확인. |
| B03-B19 lo24 Large Off | FAILED | producer166.371초·full ACK125.563초 미완료. Storage `object.rs:766`의 `BuildPrepare` deadline. numeric·memory·external 거절 없음. |
| B03-B19 lr23 Large Record | FAILED | producer156.996초·full ACK124.227초 미완료. Posting 980개 페이지 완료 후 최상위 Map 파일 설치 중 `object.rs:862` deadline. child COMPLETE·1,268,416 events·loss/open0. |
| B03-B19 mb19 Medium 기능 | VERIFIED | 동일 pair·seed42·256 sources. producer15.495초·exit0·product artifact 검증. full ACK4.975초·첫 검색119.5ms, 증분·삭제·no-op·pin 검색·재오픈·drain 완료. |
| B03-B19 mb19 Record 진단 | VERIFIED | canonical profile과 first/reopen trace 검증. 각각 62,290/1,923 events·loss/open0. prebuilt capture는 PARTIAL_DIAGNOSTIC, 정식 성능 수용은 NOT_RUN. |
| B03-B20 f70 Lexical·Kernel 집중 회귀 | VERIFIED | `def73607…` 253개 통과·702 제외. 257행 물리 경계·CAS/stream 거절·9↔8·pin·GC·재오픈 포함. |
| B03-B20 r29 최적화 pair build | VERIFIED | Cargo exit0·5분58초, source/config/dependency/tool 및 두 바이너리 해시 확인. |
| B03-B20 lo25 Large Off | FAILED | producer165.463초·full ACK124.935초 미완료. Storage `object.rs:821` 설치 경계에서 `BuildPrepare` deadline. |
| B03-B20 lr24 Large Record 제품 | VERIFIED | 동일 pair·4,096 sources·seed42. producer167.455초·exit0, 최초 ACK104.337초·첫 검색180.7ms. 증분·삭제·no-op·pin 검색·재오픈·drain 완료. |
| B03-B20 lr24 Large Record 진단 | VERIFIED | canonical profile과 first/reopen trace 검증. 876,074/2,563 events·loss/open0. prebuilt 캡처는 PARTIAL_DIAGNOSTIC이며 정식 성능 비교는 NOT_RUN. |
| B03-B20 f71 warm 국소 편집·형상 보완 | VERIFIED | `13214505…` 집중 회귀 254개 통과·702 제외. r30과 Large는 이 결과와 별도. |
| B03-B20 r30 최적화 pair build | VERIFIED | `13214505…`의 Cargo exit0·5분18초. source/config/dependency/tool 및 두 바이너리 해시 확인. 원 Large 실행과 별도. |
| B03-B20 lo26 Large Off 제품 | VERIFIED | `13214505…`·4,096 sources·seed42. producer exit0·131.170초, 최초 ACK87.815초·첫 검색187.5ms·재오픈 검색112.5ms. 증분·삭제·no-op·이전 pin·drain 포함. 로컬 prebuilt 성능 수용은 NOT_RUN. |
| B03-B20 lr25 Large Record 제품 | FAILED | 같은 r30 pair·입력. full ACK126.355초, `BuildPrepare`의 Catalog protection checkpoint에서 server deadline. 실패 child footer860,849 events·drop/open0. 검색·재오픈 미실행. |
| B03-B21 Record child 검증과 Map root 보고 | VERIFIED | 캡처·검증 Python 테스트 520개 통과·2 제외. Rust 계측 실행과 Large 검증 wall 재측정은 NOT_RUN. |
| B03-B21 f72 cold inventory와 Map 귀속 | FAILED | `60d74310…` Python 520개 통과·2 제외. Storage 회귀가 임시 lock guard를 빌려 E0716, 테스트 실행 전 실패. |
| B03-B21/B23/B24 f73 통합 집중 | NOT_RUN | `50644fa7…` Python 548개 통과·2 제외. packed Diff test의 helper shadowing E0618을 확인한 뒤 Rust 빌드를 중단했다. |
| B03-B21/B23/B24 f74 통합 집중 | FAILED | `89a062d2…`에서 403 통과·2 실패·1,258 제외. 새 Map·cold 256·packed Diff·Catalog read-only 회귀는 통과. 기존 VFS 두 테스트의 zero-scope 계측 기대를 수정. |
| B03-B21/B23/B24 f75 통합 집중 | VERIFIED | `01c4bb4d…`에서 405개 통과·1,258 제외. 실행 body 39.812초. |
| B03-B21/B23/B24 r31 최적화 pair | VERIFIED | `01c4bb4d…`의 Cargo exit0·32분34초. source/config/dependency/tool 및 두 바이너리 해시 확인. |
| B03-B21/B23/B24 r31 Large Off | VERIFIED | `lo27/execution.json`과 summary 해시에서 `01c4bb4d…`·4,096 sources·seed42 확인. 최초 ACK79.470초·검색580.8ms·재오픈 검색189.2ms. 제품 경로 exit0. |
| B03-B21/B23/B24 r31 Large Record | VERIFIED | `lr26/execution.json`과 summary 해시에서 `01c4bb4d…`·4,096 sources·seed42 확인. 최초 ACK77.573초·검색385.3ms·재오픈 검색205.0ms. 제품·canonical exit0. |
| B03-B22 f76 통합 집중 | FAILED | `4d3924da…`에서 `node_wire.rs` unused variable의 `-D warnings`로 테스트 실행 전 실패. |
| B03-B22 f77 통합 집중 | FAILED | `ca4785c9…`에서 새 scan 회귀가 `MapPage` 역참조에서 값을 이동하려 해 E0507. 테스트 실행 전 실패. |
| B03-B22 f78 통합 집중 | VERIFIED | `58d69e24…`에서 409개 통과·1,258 제외. 실행 body 36.220초. packed wire·spill·CAS·재오픈과 Catalog pin·회수 포함. |
| B03-B22 r32 최적화 pair | VERIFIED | `58d69e24…`에서 Cargo exit0. 실행 전후 scale·daemon 해시, source, config 및 workload 대조. |
| B03-B22 r32 Large Off | VERIFIED | `lo28`·4,096 sources·seed42. 최초 ACK46.419초·검색95.888ms·재오픈56.732ms. producer exit0·79.526초. |
| B03-B22 r32 Large Record | VERIFIED | `lr27`·동일 source/pair/fixture. 최초 ACK48.192초·검색103.202ms·재오픈44.830ms. producer exit0·75.448초, canonical profile 검증 완료. |
| B03-B22 공유 Storage 합류 f79 | VERIFIED | `f54536fb…`의 공유 Map/P04 조합 69개 통과·116 제외. packed batch oracle와 pending owner-scope batch 포함. |
| r33 공유 전체 소스 최적화 빌드 | VERIFIED | `6af72944…`의 Cargo exit0·14분45초. 소스·의존성·설정·도구·pair 해시를 대조했다. |
| r33 공유 전체 소스 Large120 | NOT_RUN | v19 부팅 경계가 필요해 v18 pair의 제품 재실행을 r34로 이동했다. |
| B03-B25 v19 상태 경계 f80 | VERIFIED | `645f4edf…`에서 32개 통과·1,244 제외. v18 Catalog 및 marker 거절, 원본 보존, crash-WAL, v19 재오픈 포함. |
| B03-B25 r34 최적화 pair | VERIFIED | `645f4edf…` Cargo exit0·7분02초, 소스·의존성·설정·도구·pair 해시 확인. |
| B03-B25 r31↔r34 교차 부팅 | VERIFIED | 각자의 v18/v19 root에서 부팅·종료 성공. 상대 root는 둘 다 exit70, socket 미생성, 기존 상태 파일 6개 해시 유지. |
| B03-B25 r34 Large Off | VERIFIED | `lo30`의 최초 ACK43.227초·검색264.365ms·재오픈52.279ms. producer exit0·67.010초. |
| B03-B25 r34 Large Record | VERIFIED | `lr29`의 최초 ACK44.613초·검색97.961ms·재오픈67.804ms. producer exit0·72.275초, canonical profile 검증 완료. |
| Lexical sealed wire 인벤토리 | VERIFIED | `just rust-wire-inventory` 통과. 현재 manifest 23, 판독 가능한 이전 body 22, publication attempt 22를 실제 형식에 맞춰 등록했다. |
| Lexical manifest 버전 회귀 | NOT_RUN | v23 발행과 legacy 거절 메시지의 fixture를 맞췄다. 기존 v22 판독 회귀와 묶어 실행한다. |
| r34 XL Off 로컬 진단 | FAILED | 32,768 sources·seed42. staging184.633초 완료 뒤 `BuildPrepare`가 server deadline으로 거절됐다. work cap 미소진. ACK·검색·재오픈 미실행. |
| r34 XL Record 제품 | FAILED | `lr30xl`도 `BuildPrepare`의 server deadline에서 거절됐다. ACK·검색·재오픈 미실행. |
| r34 XL Record 병목 귀속 | VERIFIED | 실패 child footer 2,226,147 events·drop/open0. server 요청120.099초 중 SourceApply46.947초·부분 LexicalBuild65.605초. Posting·최종 COMMIT 미도달. |
| Mixed 960초 Python oracle | VERIFIED | 72개 통과. 이전 독립 창을 거절하고 warmup tail 중복 합산과 load 기준 pin 보존을 검사한다. |
| Mixed 960초 Rust f81 | FAILED | `5f3b3117…`의 취소 회귀에 `-D unused-results` 한 건, body 실행 전 컴파일 실패. 해당 테스트의 반환값 처리를 교정했다. |
| Mixed·Lexical Rust f82 | FAILED | `044140e5…`에서 inventory 변조 회귀의 SQLite `execute` 결과 미사용으로 컴파일 실패. 삽입 행 1개를 oracle로 검증하도록 수정했다. |
| Mixed·Lexical Rust f83 | VERIFIED | `e6131171…`에서 32개 통과·1,151 제외. 공통 epoch·warmup 경계·ReadSession 600초 겹침과 v22/v23 manifest 포함. |
| Mixed r35 최적화 pair | VERIFIED | `e6131171…` Cargo exit0·3분32초. r36은 같은 mixed 코드와 B27을 함께 검증한다. |
| B03-B27 Source path packing f84 | VERIFIED | `ae2aaf64…` 9개 통과·201 제외. Source 경로 QIMAP005 실체, 130-owner 경로 교환, 이전 view 및 cold reopen 회귀 포함. |
| B03-B27 r36 Large 설치 실측 | VERIFIED | `ae2aaf64…` Record `lr31`. 최초 ACK42.650초·검색293.950ms·재오픈129.100ms. Source Map write·fsync 각1,095→727, 배치6→4, 중첩 COMMIT9→6. |
| B03-B26 r36 연속 Mixed | FAILED | `ae2aaf64…` `lm01` producer exit1, 83.519초. Warmup 434/434 검색 중 413 실패. 발행 2개 ACK, 현재 검색 확인 1개, missed tick 7개. |
| B03-B28 최초 occurrence 후보 | VERIFIED | `43688ad9…` f85 `nextest` 19개 통과·916 제외. r37 최적화 pair build exit0. 적대 입력 비교 비용은 후속 정렬 index로 제한한다. |
| B03-B28 후속·B03-B29 staging·Mixed 진단 f86 | FAILED | `77adb7bc…` Kernel의 test-only 직접 dependency 참조로 컴파일 exit101. 테스트 body 미실행. |
| B03-B28 후속·B03-B29 staging·Mixed 진단 f87 | FAILED | `f34402df…` Kernel 의존 경계 수정 후 Harness가 aggregate 실패 필드를 창별 상태에 써서 E0609. 테스트 body 미실행. |
| B03-B28 후속·B03-B29 staging·Mixed 진단 f88 | VERIFIED | `4cd18942…` 라이브러리 53개·UDS 4개 통과. 128개 설치, 129개 거절, occurrence wire와 기존 검색·재오픈 포함. r38 최적화 pair build exit0. |
| B03-B30 요청 예산 기반 pin 검증 f89 | VERIFIED | `4477cb45…` 라이브러리 19개·UDS/Kernel 27개 통과. writer 대기 deadline, 기존 pin의 취소·만료·회귀와 검색·재오픈 포함. r39 최적화 pair와 제품 실행은 별도다. |
| B03-B30 r39 최적화 pair | VERIFIED | `4477cb45…` build exit0. 고정 소스·설정·도구와 scale/daemon 바이너리 해시를 대조했다. |
| B03-B30 r39 Large Off `lo32` | VERIFIED | `4477cb45…`·4,096 sources·seed42. 최초 ACK 40.050초, 첫 검색 113.546ms, 재오픈 검색 103.388ms. 증분·삭제·no-op·이전 view와 제품 검증 완료. |
| B03-B30 r39 XL Record `lr31xl` | FAILED | `4477cb45…`·32,768 sources·seed42. Staging 120.360초 완료, StageSources 256회. 최초 ACK는 `BuildPrepare` deadline에서 거절됐다. 검색·재오픈 미실행. |
| B03-B30 r39 Mixed Off `lm02` | FAILED | 첫 warmup tick ACK 6.419초 뒤 현재 검색에서 `ResourceLimited`. 검색 168건 중 156건 실패. 첫 서버 검색 거절 표본 5건은 Interactive 승인 단계였다. |
| B03-B31/B32 source record·pin 갱신·Mixed 진단 f90 | VERIFIED | `9b820bb9…` 라이브러리 49개·UDS/Kernel 29개 통과. 3MiB 본문 재사용, 갱신 취소·응답 손실, 중단된 검색 스케줄의 큐 집계 포함. |
| B03-B33/B34 packed alias 슬롯·Interactive 예약 f91 | FAILED | `a82c0231…` 집중 회귀 56/57. `TextSpool`이 유지하는 packet lease 8,208B를 spool 종료 전에 0으로 검사한 테스트 한 건 실패. UDS 미실행. |
| B03-B33/B34 packet lease 교정 f92 | VERIFIED | `b7c17056…` 집중 57/57, 실제 Native UDS 4/4. spool 종료 전 소유와 종료 후 완전 환급을 각각 검증했다. |
| B03-B33/B34 r40 최적화 pair | VERIFIED | `b7c17056…` build exit0. 소스·설정·의존성·도구·두 바이너리 해시를 확인했다. |
| B03-B33/B34 r40 Large Off `lo33` | VERIFIED | `b7c17056…`·4,096 sources·seed42, producer exit0. 최초 ACK 39.080초, 첫 검색 78.934ms, 재오픈 검색 46.083ms. 증분·삭제·no-op·이전 view 검증 완료. 로컬 제품 범위다. |
| B03-B33/B34 r40 Mixed Off `lm03` | FAILED | `b7c17056…` producer exit1, 59.764초. 최초 tick ACK 4.321초 뒤 일반 Exact `current_witness`에서 `ResourceLimited`. 검색 완료 91개 중 76개 실패, due 220개 중 66개 queue full. |
| B03-B35 cold ASCII token scratch | NOT_RUN | `468fa8f3…` 후보에서 선택적 메모리가 필수 예약을 잠식하는 P2를 정적으로 확인했다. 적용을 철회하고 원 SQLite 경로로 복원했다. |
| B03-B36 일반 Text 실패 귀속·ReadView 예산 f93 | VERIFIED | `a9a42465…` 집중 53/53, 실제 Native UDS 4/4. 일반 Exact Text 실패 표본과 ReadView validation의 원 요청 예산 연결을 포함한다. |
| B03-B36 r41 Mixed Off `lm04` | FAILED | `a9a42465…` producer exit1, 49.311초. 첫 ACK 3.162초, 최신 witness 3.426초, 이전 witness 통과. 이후 `next_snapshot_open`의 `retained_control/control_admission`에서 즉시 거절됐다. |
| B03-B37 Control 승인 대기 | NOT_RUN | 같은 ledger의 원 Control envelope에서 일반 Text 준비와 retained control에 bounded 대기를 연결한다. 기존 즉시 Control 호출은 유지한다. |
| B03-B37/B38 Control 승인·retained 세션 공유 검색 f94 | FAILED | `1907d1bc…` 동시성 테스트 fixture의 scoped thread 참조 수명 E0597. 테스트 body 미실행. |
| B03-B37/B38 fixture 교정·Release 대기 보호 f95 | FAILED | `8c605917…` Runtime 54/54(기존 설정 테스트 1개 nextest `leaky`). Kernel 테스트의 불필요한 `mut`가 `-D warnings`로 거절됐다. Kernel body·UDS 미실행. |
| B03-B37/B38 집중 회귀 f96 | VERIFIED | `f3de87e3…` Runtime 54/54, Kernel 23/23, 실제 Native UDS 4/4. 기존 설정 테스트의 `leaky` 재현 없음. |
| B03-B37/B38 r42 Mixed Off `lm05` | FAILED | `f3de87e3…` producer exit1, 170.588초. warmup 14건·measure 12건 발행 뒤 `held_read_tick` deadline. 117개 tick 중 90개 late, 검색 완료 4,003건 중 3,949건 실패. |
| B03-B39 만료 tick 갱신 순서 f97 | FAILED | `14f7998f…`에서 잘못된 바이너리 테스트 타깃을 선택해 body 0개. |
| B03-B39 만료 tick 갱신 순서 f98 | VERIFIED | 동일 소스 `--lib`에서 Mixed·retained harness 18/18. deadline 경계와 missed 분모를 확인했다. B39 이후 960초 제품 실행은 NOT_RUN. |
| B03-B40/B41/B42 Catalog SQL·Mixed view·Posting scratch f99 | FAILED | `d2f2c14f…` Catalog 15/15, Posting 24/25. 추가 readback의 Retained grant를 Temporary 전용 fixture가 거절했다. harness·UDS 미실행. |
| B03-B40/B41/B42 Posting readback fixture 교정 f100 | VERIFIED | `1376643a…` Catalog 15/15, Posting 25/25, harness 18/18, 실제 Native UDS 4/4. 쓰기 Temporary와 readback Retained grant를 각각 확인했다. |
| B03-B40/B41/B42 r43 최적화 pair·Large Off `lo43` | VERIFIED | `1376643a…` 빌드와 로컬 제품 artifact 검증 통과. 최초 ACK 27.923초, 첫 검색 53.868ms, 재오픈 54.233ms. 별도 성능 자격은 NOT_RUN. |
| B03-B40/B41/B42 r43 XL Off `xo43` | FAILED | `1376643a…`·32,768 sources. Staging 93.397초. 원 서버 deadline은 BuildPrepare의 물리 설치 중 만료됐다. harness ACK phase 140.304초. 검색·재오픈 미실행. |
| B03-B40/B41/B42 r43 Mixed Off `lm06` | FAILED | `1376643a…` producer exit1, 540.878초. 116건 발행, 487 tick 중 370 late, 완료된 검색 16,412건 중 16,287건 실패. held 갱신 네 번째 ACK 뒤 검색 timeout. |
| B03-B43 namespace FD 신원 재사용 f101 | VERIFIED | `3d2d0690…` storage 31/31, 실제 Native UDS 4/4. live path 권한·rename·교체 거절 포함. B43 제품 실행은 NOT_RUN. |
| B03-B44 batch와 Lexical 확장 f102 | FAILED | `0f42a5b0…` Storage 13/13. Lexical 178/182, occurrence packet 메모리 계상 4건 실패. UDS 미실행. |
| B03-B43 변경 전 occurrence 대조 f103 | FAILED | `3d2d0690…`에서 같은 4건 모두 동일하게 실패했다. B44 이전 B33의 spool 수명 계상과 회귀 기대값 차이다. |
| B03-B44/B45 첫 합류 f104 | FAILED | `c293238f…` Storage 13/13, Lexical 179/182. 메모리·취소로 SQL에 들어가지 않은 분기와 retained grant 호출 수 기대값을 추가 교정했다. |
| B03-B44/B45 합류 f105 | VERIFIED | `74a344ac…` Storage 13/13, Lexical 182/182, Mixed/retained harness 19/19, 실제 Native UDS 4/4. old view·재오픈·BM25·release를 포함한다. 검증 소유 파일 6개의 공유 작업 트리 일치를 확인했다. |
| B03-B44/B45 r44 Large Off `lo44` | VERIFIED | `74a344ac…` producer exit0. ACK 27.389초, 첫 검색 47.374ms, 재오픈 검색 39.019ms. Python 3.9의 artifact oracle은 `zip(strict=...)`에서 실패했고, 원 artifact와 양쪽 바이너리의 해시를 대조한 뒤 Python 3.12에서 같은 oracle을 재검증했다. 제품 재실행 없음. 정식 성능 자격은 NOT_RUN. |
| B03-B44/B45 r44 XL Off `xo44` | FAILED | `74a344ac…`·32,768 sources. Staging 93.388초. 원 server 120초가 BuildPrepare에서 만료됐다. checkpoint는 packed occurrence vector lookup, ACK phase 138.490초. 검색·재오픈 미도달. |
| B03-B44/B45 r44 Mixed Off `lm07` | FAILED | `74a344ac…` producer exit1, 706.147초. warmup 17건·measure 167건, 600.434초 release와 마지막 pin 확인. 61회 조회 후 settlement `Pending`, 검색 22,023/22,288 실패. |
| Capture Python 사전 거절 | VERIFIED | capture가 프로젝트 최소 Python 3.10보다 낮은 interpreter를 producer 실행 전에 거절한다. Python 3.9 direct capture 거절과 Python 3.12 CLI help를 확인했다. r44 고정 실행 소스와 별도다. |
| B03-B46/B47 f107 | VERIFIED | `b2ec5a78…` hardlink subset·disk 관측·Mixed lifecycle·Pending/성공 스키마 29/29. Capture Python 154개 통과·managed-build 전용 2개 제외. 공유 코드 일치, 이 변경을 포함한 제품 재실행은 NOT_RUN. |
| B03-B48/B50 첫 집중 f108 | FAILED | `f28b922a…` Catalog 12/13. fixture에 terminal custody가 남아 view보다 먼저 차단됐다. 기존 custody API로 정리한 뒤 두 차단을 순서대로 확인하도록 교정했다. |
| B03-B48/B50 후속 f109 | FAILED | `a6ac8bab…` Catalog 13/13. Lexical은 Posting binding에 남은 미사용 인자 2건으로 body 실행 전 컴파일 거절. 기존 경로 호출과 함께 제거했다. |
| B03-B48–B50 통합 f110 | VERIFIED | `5d830c1e…` Catalog 13/13, Lexical 185/185, 실제 UDS 4/4. cold 512→1 scratch, 긴 key fallback, 잘못된 seal·anchor 거절, batch lookup·singleton·늦은 손상 거절을 포함한다. 소유 파일 12개의 공유 일치와 구조 검사를 확인했다. |
| B03-B48–B50 r45 Large Off `lo45` | VERIFIED | `5d830c1e…` producer exit0, 제품 artifact oracle 통과. ACK 31.789초, 첫 검색 46.048ms, 재오픈 검색 48.149ms. Prebuilt binding UNPROVEN, 정식 성능 NOT_RUN. |
| B03-B48–B50 r45 XL Off `xo45` | FAILED | `5d830c1e…`·32,768 sources. Stage 100.750초. BuildPrepare에서 원 server 120초 만료, ACK phase 137.642초. 첫 차단은 deadline, checkpoint는 객체 읽기, search/reopen 미도달. |
| B03-B48–B50 r45 Mixed Off `lm08` | FAILED | `5d830c1e…`, 704.012초에 중단. 180건 ACK, 479 tick 지연. 600.539초 마지막 pin 제거, 61회 후 `Pending`과 60초 초과. 같은 snapshot에서 TerminalCustody 차단 관측. |
| B03-B51–B53 집중 f111 | FAILED | `0e3ebea3…` Catalog 19/19. Lexical 185/186, 기존 reader policy golden이 현재 저장 layout digest와 달랐다. 새로운 packed 읽기·인증 member 테스트는 통과했다. UDS 미도달. |
| B03-B51–B53 집중 f112 | VERIFIED | `8684fe0c…` Catalog 19/19, Lexical 186/186, 실제 UDS 4/4. 현 layout SHA-256 golden 교정, writer 예약 중 custody 후보 읽기, 인증 member GET 1회, packed count의 scalar SQL 제거를 검증했다. 공유 작업 트리의 소유 파일 5개를 검증된 바이트로 반영했다. |
| B03-B51–B53 r46 Large Off `lo46` | VERIFIED | `8684fe0c…` producer exit0, ACK 32.938초, 첫 검색 64.130ms, 재오픈 검색 78.135ms. 원 120초와 제품 oracle 통과. 정식 성능 NOT_RUN. |
| B03-B51–B53 r46 XL Off `xo46` | FAILED | `8684fe0c…`·32,768 sources. Stage 99.610초. BuildPrepare 원 120초 만료, ACK phase 141.811초. 공용 scratch batch SQL 이후 checkpoint, 검색·재오픈 미도달. |
| B03-B51–B53 r46 XL Record `xr46` | FAILED | 동일 source·binary, BuildPrepare 120초 거절, ACK phase 143.769초. Daemon trace 256MiB 상한으로 partial, 종료된 prefix span은 RCA 진단에만 사용했다. |
| B03-B54–B56 집중 f113 | VERIFIED | `647dce43…` Catalog 29/29, Runtime 11/11, 실제 UDS 2/2. Custody 독립 cap, 고정 snapshot의 oldest, 인증 inode 재사용을 포함한다. 공유 작업 트리의 소유 파일 11개를 반영했다. |
| B03-B54–B56 r47 Mixed Off `lm09` | FAILED | `647dce43…`, cooperative lease. Custody 32행에서도 707.722초에 중단. 대상 pin 해제·61 poll, 60초 정산 실패. oldest 20874 동일, 194 ACK, 464 tick 지연. |
| B03-B57–B60 집중 f114 | FAILED | `c0f336be…` Core 2/2, Scratch test의 SQLite 이전 limit 반환값을 쓰지 않아 컴파일 거절. Scratch body 0, 이후 scope 미도달. Hexagonal 통과, module-cycle 진단은 직전 cut과 동일. |
| B03-B57–B61 집중 f115 | FAILED | `2e6aafe2…` Core 2/2, Scratch 32/32, Catalog 19/19. Lexical에서 scratch connection과 token budget 참조 타입 2건을 거절했다. Lexical body 0. |
| B03-B57–B61 집중 f116 | FAILED | `16c5b4fe…` Core 2/2, Scratch 32/32, Catalog 20/20, Lexical 198/198. Runtime 의존 Lexical production build에서 test-only width helper를 dead-code로 거절했다. Runtime·UDS body 미도달. |
| B03-B57–B61 집중 f117 | FAILED | `2f8a31b6…` Core 2/2, Scratch 32/32, Catalog 20/20, Lexical 198/198. Runtime telemetry에서 page의 released 값 바인딩 누락 2건, Runtime body 0. |
| B03-B57–B61 집중 f117·f118 영향 검증 | VERIFIED | Runtime telemetry만 고친 `930e30da…`에서 Runtime 6/6·실제 UDS 4/4. 위 252개 검증의 소스는 byte 일치. Hexagonal 통과, module-cycle 진단은 r47과 동일. 검증된 소유 파일 15개를 공유 작업 트리에 반영했다. |
| B03-B57–B61 r48 Large Off `lo48` | VERIFIED | `930e30da…` 최적화 pair, producer exit0, ACK 29.751초, 첫 검색 52.270ms, 재오픈 검색 72.126ms. 원 oracle 통과. |
| B03-B57–B61 r48 XL Off `xo48` | FAILED | `930e30da…`, cooperative lease, Stage 105.752초, ACK phase 141.836초. BuildPrepare에서 원 server 120초, 설치 결과 대조 checkpoint. 논리 reads 3,354,864, 첫 검색·재오픈 미도달. |
| B03-B57–B61 r48 Mixed Off `lm10` | FAILED | `930e30da…`, cooperative lease. 첫 실패는 publisher의 이전 뷰 검색 2초, 272.413초에 중단. 53건 ACK, 검색 7,291/7,391 실패. 초기 op2 custody 9,984행 제거, 600초 held·settlement 미도달. |
| B03-B62 집중 f119 | VERIFIED | `fd91adfc…` Catalog 20/20, 실제 UDS 2/2. 같은 writer transaction의 GC binding 재사용과 cached DELETE를 공유 작업 트리에 반영했다. Mixed 성능은 NOT_RUN. |
| B03-B63 집중 f120 | VERIFIED | `d4e0a5dc…` Runtime 2/2, 실제 UDS 4/4. 타이밍과 custody 카운터, 기존 pinned BM25·retained·reopen 회귀를 확인하고 소유 파일 2개를 공유 작업 트리에 반영했다. |
| B03-B62–B63 r49 Mixed Off `lm11` | FAILED | `d4e0a5dc…` 최적화 pair. 724.781초에 중단, 600.550초 마지막 pin 해제, 61회 정산 poll, oldest ready20874 유지. 22,185/22,305 검색 실패, 150건 ACK. |
| B03-B62–B63 r49 XL Record `xr49` | FAILED | `d4e0a5dc…` 동일 최적화 pair. SourceApply 31.740초, BuildPrepare 85.141초, ACK phase 140.130초. Child trace 3,144,831 events, 손실·미종료 span 0. 제품 ACK·검색·재오픈 미도달. |
| B03-B64–B67 집중 f121 | VERIFIED | `050e6280…`, `scripts/cargow --lane test-workspace-lane nextest run --locked --offline`. Catalog 22/22, Lexical 177/177, Runtime 3/3, 실제 UDS 5/5. 검증한 5개 소유 파일을 공유 작업 트리에 반영했다. |
| B03-B64–B67 r50 Large Off `lo50` | VERIFIED | `050e6280…` 최적화 pair, producer exit0. ACK 27.622초, 첫 검색 120.563ms, 재오픈 검색 59.067ms. 증분·삭제·이전 pin·종료 oracle 통과. prebuilt build binding `UNPROVEN`, 정식 성능 수용 `NOT_RUN`. |
| B03-B68–B71 집중 f122 | FAILED | `15b34295…` Catalog 21/21, Scratch 33/33. 신규 lexical test의 가변 budget 참조와 ordinal 타입 누락으로 lexical 컴파일 exit101. Runtime·UDS `NOT_RUN`. |
| B03-B68–B71 집중 f123 | FAILED | 테스트 타입을 수정한 `34f30387…`에서 lexical memory meter의 unused-results lint로 컴파일 exit101. Runtime·UDS `NOT_RUN`. |
| B03-B68–B71 집중 f124 | VERIFIED | 테스트 meter를 고친 `1482d857…`에서 Lexical 192/192, Runtime 9/9, 실제 UDS 3/3. 변경이 없는 f122 Catalog 21/21·Scratch 33/33과 합계 258개. 소유 파일 7개를 공유 작업 트리에 반영했다. |
| B03-B68–B71 r51 XL Off `xo51` | FAILED | `1482d857…` 최적화 pair. staging 98.897초, ACK phase 141.436초, BuildPrepare 물리 설치 child-validation checkpoint에서 원 server 120초. ACK·검색·재오픈 미도달. |
| B03-B68–B71 r51 Mixed Off `lm12` | FAILED | `1482d857…`, producer exit1. 704.736초에 종료, 600.475초 마지막 pin 해제, 61회 정산 poll 끝 `Pending`, head=floor176. 초기 op2 제거, 검색 22,106/22,311 실패, ACK172. |
| B03-B68–B71 r51 XL Record `xr51` | FAILED | 동일 pair. staging 105.159초, ACK phase 142.171초, BuildPrepare에서 원 120초. Child trace 3,159,263 events·손실 0. Posting 표본 21개에서 `raw_execute` 약 71%, capacity 약 5%. |
| B03-B72 집중 f125 | VERIFIED | `802e12b5…` Kernel 23/23, Runtime 3/3, 실제 UDS 2/2. 실행 분리 계측과 기존 검증 순서를 확인해 5개 파일을 공유 작업 트리에 반영했다. |
| B03-B73 집중 f126 | VERIFIED | `d310d040…` Catalog 22/22, 실제 UDS 2/2. 고정폭 32행 prefetch, 논리 계상·page+1·rollback을 확인하고 소유 파일을 공유 작업 트리에 반영했다. |
| B03-B72–B73 r52 Mixed Off | FAILED | `d310d040…`, 338.958초에 producer exit 1. 두 번째 갱신 뒤 held search가 726ms IPC deadline에 도달했다. 검색 8,134건 중 8,058건 실패. 마지막 pin 조기 해제 확인, 원 600초 유지와 settlement는 NOT_RUN. |
| B03-B75 집중 f127 | VERIFIED | 격리 소스 `e36a150e…`, Lexical 192개와 UDS 2개 통과. 32바이트 이하 Posting 키의 별도 할당과 grant를 제거했다. 기존 inventory cursor 변경을 보존한 exact patch를 공유 작업 트리에 반영했다. XL 성능은 NOT_RUN. |
| B03-B76 집중 f130 | VERIFIED | 격리 소스 `b05b0c4e…`, Kernel 24개·runtime 3개·UDS 2개 통과. 최종 Kernel pin 검증 전 Interactive 슬롯을 반환하고 기존 오류 우선순위·일곱 검증을 유지한다. exact patch를 공유 작업 트리에 반영했다. Mixed 제품 수용은 NOT_RUN. |
| B03-B78–B79 집중 f131 | VERIFIED | 격리 소스 `7272bac5…`, Runtime 14개와 UDS 3개 통과. B79는 exact patch를 공유 작업 트리에 반영했다. |
| B03-B76/B78–B79 최종 집중 f132 | VERIFIED | 격리 소스 `932f8e9f…`, Kernel 1개·Runtime 14개·UDS 3개 통과. B78 세부 계측과 B76 회귀의 lint 보정을 반영했다. XL 및 960초 Mixed 제품 수용은 NOT_RUN. |
| B03-B76/B78–B79 Kernel f133 | VERIFIED | `a6003311…`, reader 타이머와 기존 Source cache 상수의 동등한 lint 보정 뒤 Kernel 24개 통과. exact patch 반영. |
| Kernel·Runtime 집중 strict Clippy | FAILED | Kernel의 두 오류를 수정했다. Runtime lib는 기존 source의 scheduler 2건, retention 진단 2건, retained 타이머 문서 2건, publication 3건에서 실패했다. 기능 회귀와 별도 판정이다. |
| r53 Mixed Off | FAILED | `a6003311…`, producer exit1·474.416초. 세 번째 갱신 뒤 held search의 834ms IPC timeout. 검색 12,226건 중 12,209건 실패. 363.641초에 마지막 pin 해제, 600초 유지와 settlement는 NOT_RUN. 호스트 load1 16.79–106.35. |
| B81 Catalog 검증 계측 f135 | VERIFIED | 최종 `3c034df4…`에서 Catalog 28개·실제 UDS 2개, Catalog strict Clippy 통과. Off clock 비호출과 Record의 성공·clock 회귀 구간을 확인했다. exact patch를 공유 소스에 반영했다. |
| r54 Mixed Record | BLOCKED | `3c034df4…` 빌드는 완료했으나 capture와 harness가 Mixed의 Record 모드를 지원하지 않는다. capture preflight에서 거절돼 producer body 0, 제품·진단 데이터는 NOT_RUN이다. |
| 정식 overhead·XL·Mixed·출시 수용 | NOT_RUN | 로컬 실패 진단과 별도 범위 |

B9 고정 입력46개 중 Main과 다른 두 파일은 `map.rs`의 lookup batch와
`text_reader.rs`의 member binding 조회다. B9의 설치 경계와 계측 hunk는
동일하다. Main의 두 read 경로 변경은 r15/lo14/lr13 검증에 포함되지 않는다.
B11 고정 입력48개의 Main 차이는 위 두 read 경로와 Runtime capacity의 별도
Sidecar parent-anchor 변경이다. r17/lo16/lr15는 B11의 full-bound 변경을 포함한다.

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
phase work delta 누락은 수리·집중 검증을 마쳤다. B03-B1의 owner-local
term→positions dictionary block, B2의 8,192B/512-term 확장, B3의 QITXEM02
inventory child dictionary는 각 소유 집중 회귀를 통과했다. 당시의 Large120
실패는 위 r43 로컬 제품 결과로 갱신했다.
mr5 Medium에서는 이 경로가 실제 활성화돼 managed reserve 입력이 5,230→1,635개,
inventory occurrence 검증이 3,083→512회로 줄었다. 하지만 Prepare의 accepted logical
bytes는 261,371,848→321,302,214B로 늘어 fixed512MiB 진단은 여전히 실패했다.
이 수치는 서로 다른 r4/r5 source·host 실행의 단계별 작업 계수이며 속도 개선율이 아니다.
당시 a608 후보는 planned inventory의 검증된 Null/Object 필드를 실제 wire 길이로 charge한다.
행당 accepted work 감소 예상은 prior 없는 경우 1,068B, prior Object 경우 972B다.
기존 사전 reserve 절차와 하드 SQLite page cap을 유지한다. 이 수치는 코드 산술이며 제품
성공이나 CPU·wall 개선 실측이 아니다. mr6 trace의 Prepare는 mr5보다54,602,568B
적게 charge돼 정확히 51,126행×1,068B와 맞았다. Postings를 더 진행했지만 여전히
512MiB 한도에서 거절됐다.

- 빈 prior inventory의 Bootstrap 또는 초기 Patch에서 현행 QITXOC01은 payload≤8,192B / canonical term rows≤512로 묶는다. B1의 4,096B/128행 정책은 retired다.
- 같은 positions vector는 block 안에서 공유한다. singleton/oversize/변경된 delta term은 leaf다.
- current-policy reader는 leaf/block과 QITXEM02 inventory를 읽는다. B1·B2의 구 output_policy는 typed refusal이다.
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

1. r32에서 설치 증폭 감소를 확인했고, 공유 코드의 r34에서도 같은 물리 작업량과
   원 Large 제품 경로를 검증했다. 이전 실패 source의 재실행은 종료한다.
2. 배포에서는 v18 바이너리·root 쌍을 보존하고 v19 root를 producer에서 재구축한다.
   실제 운영 전환과 release qualification은 `NOT_RUN`이다.
3. E07의 XL, 고정 mixed query p95/p99·지역 비용·pin/GC는 별도 수용이다. 정식
   trace overhead는 source/build binding을 갖춘 같은 바이너리의 적격 호스트
   Off·Record 비교로 판정한다. 현재 공유 호스트의 순차 진단은 이 범위를 충족하지 않는다.

Core/Storage/Catalog/SDK·기존 inventory Block/P1-A와 다른 사용자 채팅의 기능을
재구현하지 않는다. 세부 파일 one-writer와 실패/폐기 조건은 WORKPLAN이 소유한다.

## 과거 근거

<a id="원본-가용성"></a>

51b70b34의 mr2/lo2/lr2, C16/C17와 이전 실행 기록은 [RCA](RCA.md) 및
[이전 VALIDATION](../oct-22-finalizing/VALIDATION.md#e07-indexing-versus-host-rca-20261105)의
역사적 근거다. 과거 `nov03-e07-wal-policy`/`e7n1105` 원본 경로는 현재 부재해 그대로의
replay는 BLOCKED다. 현재 source의 새 측정과 혼동하지 않는다.

기존 [E07](../oct-22-finalizing/E07-performance-operations.md)·WAVES는 이 SSOT의
진입점이며 과거 본문은 [history](../../history/nov-5-indexing-bugbash/)에 보존한다.
새 현재 상태·후속 작업은 이 세 문서에서만 갱신한다.
