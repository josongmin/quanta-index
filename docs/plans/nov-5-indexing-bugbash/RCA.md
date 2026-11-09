# Native 초기 인덱싱 구조적 RCA

현행 상태는 [README](README.md), 구현 순서는 [WORKPLAN](WORKPLAN.md)이 소유한다.
분석 기준은 저장된 C16/C17 기록, `51b70b34`·`86e6130e` 이력,
r4 제품 고정본 `0a97e858`과 B1 제품 후보 `d8e2c52d`다. 후자는 외부 local
snapshot source이며 r5 Medium 진단 body는 fixed work cap, lo5/lr5 Large Off/Record는
원120초 deadline에서 각각 실패했다. typed-fee 후보 `a6083fb2`는 r6 build 통과 뒤
mr6 Medium에서 다시 fixed512MiB 진단 cap으로 실패했다. B2 r7의 Medium 진단 cap·
Large120도 FAILED다. B3 mr8 Medium 진단 cap과 lo8 Off·lr8 Record Large120도 FAILED다.
과거 성능 숫자는 기존 VALIDATION에서 가져왔으며 해당 과거 외부 원본은 부재다.
아래 mr2와 r4 실행은 각각 source와 바이너리 전후 일치를 확인한 별도 cohort이며 과거 replay가 아니다.
[현재 가용성](README.md#과거-근거)은 별도로 기록한다.

## 최신 B4 실행과 B5 코드 수선

고정 소스 `213908833f462447fc229519d1d6f1472e978644`의 f26 집중423개는
Storage85·Catalog27·Native297·Kernel14 전부 통과했다. f27 Storage/Catalog
lib+tests strict는 기존 Storage `object/read_only/tests.rs` 변환 lint2건으로
FAILED, 소유 변경 파일 진단0이다. r9 정상 cached pair build는4분28초에 완료됐다.
이는 local source-bound build이며 managed-fresh·전체 Main qualification이 아니다.

- mr9 Medium 진단은 full ACK9.282초 미완료, producer18.446초·exit1이다.
  740,061R/464,764W/536,863,832B/236,679M 뒤9,968B charge가 기존512MiB
  진단 cap을 넘어 FAILED다. Large의 선행조건으로 쓰지 않는다.
- lo9 Off는 full ACK125.522초 미완료, producer163.804초·exit1이다.
  12,653,629R/7,616,728W/8,967,287,949B/2,602,795M에서 원120초 deadline으로
  실패했다. cap·memory·external refusal이 아니다.
- lr9 Record는 full ACK124.534초 미완료, producer158.351초·exit1이다.
  15,638,380R/8,502,561W/9,967,787,855B/3,447,298M에서 같은 deadline이다.
  두 실행의 실패 관측 위치는 Storage `map/completed_group.rs:140`이며
  동일한 source·binary pair에 자원 sampler는 둘 다Off였다.

lr9 child896,891 events는 COMPLETE이며 dropped/unwritten/open/lease0이다.
실패 LexicalBuild는 wall109.084초/CPU68.677초, Postings는 wall63.131초/
CPU39.771초다. 첫64-term page의 apply는32.329초/CPU23.082초에 완료했고,
둘째 page apply는27.287초/CPU16.433초에서 실패했다. 첫 page의 직접 자식은
reserve5.455초·install10.666초·confirm14.108초·scratch envelope0.670초다.
Catalog reserve+confirm은19.563초(60.5%)다. 둘째 page도 Catalog17.711초
(64.9%)로 주 비용이다. 첫 page는41×256+10=10,506 input slot, 둘째는
23×256=5,888 slot을 처리했다. r8의 첫 page38×256 실패와 진행 endpoint가
다르므로 wall 비율이나 counter 차이를 격리된 B4 speedup으로 주장하지 않는다.

lo9 시작 직전 load30.48·swap 약28GiB, lr8의 wall−CPU와 많은 page miss는
호스트 경합·blocking이 공존함을 시사한다. 어느 wait가 host paging인지 정확히
분해한 증거는 없다. 또한 Catalog의 실제 SQL·CPU·checkpoint 비용도 있으므로
host-only로 결론 내리지 않는다. 비정상적으로 반복되는 같은 실행을 추가하지 않고
실제 edge 작업을 줄이는 B5를 구현한다.

B5는 posting의 dense Tree `ObjectRef`를 **비교 전용 digest witness**로 바꾼다.
원래 query는 Contribution inventory도 읽어 posting ref와 정확 비교했다. 새 query는
같은 inventory의 실제 ref를 얻어 witness와 비교한 뒤 그 ref만 load한다. 참조
일치·Some→Some CAS·source pin·TF/positions 검사를 버리는 순수 membership unit은
선정하지 않았다. Root→owners→Contribution→inventory가 실제 occurrence의
물리 GC 경로이며, witness는 역참조·등록 권한이 아니다. Small≤8의 기존 명시
참조는 유지해32B inline ABI와 수만 singleton의 객체 수를 보존한다.
새 소스의 기능 proof는 f31(`5e41d204…`) Native301·Kernel15와,
타입 별칭·테스트 borrow만 교정한 현행 `3a2ad5dd…`의 f35 변경2개 composite다.
f36 strict는 소유 진단0/전역33개 기존 진단으로 FAILED이며 전체 green이 아니다.
r10 pair build를 완료했고 lo10 Off와 lr10 Record 모두 원120초 제한에서
FAILED다. 계약·소유·테스트는
[B5 계획](WORKPLAN.md#b03-b5--dense-posting-tree의-비교-witness와-실제-custody-분리)을 따른다.

## B5 동일 Posting 배치 구간 비교

`3a2ad5dd…`의 lo10 Off는 ACK123.507초·producer154.318초에 실패했다.
lr10 Record는 sampler Off, ACK127.683초·producer168.426초에 실패했다.
두 실행 모두 BuildPrepare deadline이며 전체 ACK·검색·재오픈은 미도달이다.
lr10 child footer는890,079 events와 loss/open/lease0을 기록했다.

lr9와 lr10에서 첫 dense page의 완료된 처음27개 256-input 배치(6,912개)를
맞춰 비교했다. 아래 시간은 같은 종류의 직접 자식에 한정한다.

| 구간 및 계수 | lr9 B4 | lr10 B5 |
| --- | ---: | ---: |
| Catalog reserve wall / thread CPU | 3.401s / 2.189s | 2.023s / 0.470s |
| 객체 install wall / thread CPU | 7.019s / 5.210s | 14.391s / 6.091s |
| Catalog confirm wall / thread CPU | 8.703s / 6.001s | 6.855s / 2.594s |
| Catalog pager misses | 1,090,193 | 83,149 |
| Catalog VFS write calls | 67,478 | 33,647 |
| Catalog VFS requested bytes | 857,017,120 | 388,452,656 |

lr10은 첫 Posting page의 28번째 설치 정산에서 멈췄다. lr9는 첫 page
42개 배치(10,506 input)를 완료한 뒤 둘째 page에서 멈췄다. 같은 앞27개
배치에서는 Catalog의 반복 작업이 줄었고 객체 설치의 wall−thread CPU가
약1.809초에서8.300초로 늘었다. lr10 시작 직전 load61.76·swap3.84GiB와
다른 실행 cohort의 조건 차이가 있다. 스케줄링과 파일시스템 대기의 정확한
비율은 측정되지 않았다. 위 계수는 쓰기 요청량이며 장치 실측 속도 비교가 아니다.

같은 27개 install의 phase packet에서 namespace/create13,851회는
wall3.654→9.336초, CPU2.956→4.210초였다. settlement54회는
wall0.357→1.613초, CPU0.332→0.417초였다. 두 구간이 install wall 증가분
7.372초 중 약6.938초를 설명한다. 파일 동기화6,912회의 wall은
0.375→0.441초였다. B6는 기존 shard에 대한 반복 `mkdir` 호출을 제거한다.
각 객체의 live shard 검사, device와 inode 검증, 동기화 및 정산은 계속 수행한다.

Record 첫 실행은 도구의 mount device 식별자 변경으로 producer 시작 전에
거절됐다. 15개 경로의 나머지 epoch 메타데이터가 동일함을 검사한 뒤 새
runtime epoch를 고정했다. 재실행의 환경 차이는 PATH 한 키이며 canonical
도구, frozen source, 실행 파일 guard는 유지했다.

## B6 로컬 Large

`c5c691e…`의 lo11 Off는 공용 실행 lease를 확보한 뒤 실행했다.
producer155.742초·ACK123.772초에 실패했고, BuildPrepare의
`managed_install.rs:598`이 deadline을 관측했다. 이 줄은 `try_reserve_new`가
기존 행을 발견한 뒤 `require_reserved` 전에 작업량을 청구하는 지점이다.
Off 실행은 child trace가 비어 있어 해당 분기 시간이나 도달한 Posting 페이지를
알 수 없다. 사용량은13,349,970R/
8,296,258W/10,899,408,339B/3,160,481M이다. 수치·메모리·외부 자원
거절 없이 전체 ACK·검색·재오픈 미도달이다.

45초 안정화 관측의 1분 load는18.52~21.96이었다. 실행 전후의 호스트 전체
swap in/out은16KiB page 기준81,536/58,576개였다. 공용 lease는 같은 lock을
사용하는 Cargo 작업을 직렬화했지만 호스트 전체를 격리하는 조건은 아니다.
진행 끝점이 다른 lo10과의 사용량 차이는 B6 속도나 회귀를 증명하지 않는다.

실행 요약 controller의 mr8/lo8 경로가 남았던 두 요약 오류는 외부
`r9/controller-correction.json`과 `l13ctl/controller-correction.json`에 정정했다.
위 수치는 실제 `mr9`·`lo9`·`lr9/execution.json`과 refusal 원본이다. source/binary
guard는 유지됐고, 잘못 읽은 과거 summary를 새 제품 결과로 사용하지 않는다.

## B7에서 분리할 비용

- **반복 트리 생성.** cold Posting의 seed42 소스 투영에서 DF>8인
  Sensitive/Folded term 쌍157개의 owner와 witness 편집 입력이 같다. B7은
  두 case를 같은 페이지에 놓는다. Storage는 같은 base와 편집 count 및
  fingerprint가 일치하고 양쪽 scratch 재검증이 끝난 경우에만 이미 만든 root를
  공유한다. 기존 root를 만드는 비용은 줄일 수 있고, 양쪽 cursor·CAS·scratch
  쓰기와 재검증은 남는다. 기존 도달 DAG 중복 제거와 Catalog의 기존 객체
  채택 때문에 root 생성 감소량과 실제 파일 설치 감소량은 다를 수 있다.
  두 case 범위를 읽는 SQL·버퍼의 추가 비용도 함께 측정한다.
- **물리 설치.** B5의 동일 prefix에서 늘어난 install wall 7.372초 중
  6.938초가 namespace/create와 settlement에 있다. 현재 namespace/create는
  Store root·shard 확인과 writer의 runtime allowance 및 파일 생성을 합산한다.
  settlement는 synced 정산과 이어지는 budget checkpoint를 포함한다.
  B6의 기존 shard mkdir 제거와 별도 Runtime owner의 parent anchor 단일 경로는
  이미 작업 범위에 있다. live namespace, ticket, 파일 및 디렉터리 sync에는 각각
  수명·공간·내구성 조건이 있어 정적 감사만으로 더 제거할 호출은 확인되지 않았다.
- **호스트 대기.** 같은 prefix의 install wall−thread CPU는 1.809초에서
  8.300초로 늘었다. 파일시스템·capacity lock 대기와 CPU deschedule이 모두
  포함될 수 있다. B6의 호스트 전체 swap 증가는 경합이 있었다는 정황이다.
  해당 프로세스의 stall 시간과 원인별 비율은 미확정이다. B5/B6의 Off도 실패해
  Record 계측 비용이 deadline 실패의 필수 조건은 아니다.

측정에서 **생성 생략**, **실제 설치량 감소**, **남은 설치 대기**를 따로 판정한다.
B7 검증은 [이력](WORKPLAN.md#b7-검증-및-분기)에 있다. 현재 수정과 실행은
[B9 계획](WORKPLAN.md#b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기)을 따른다.

`e3e8512…`의 lo12 Off도 원 120초 조건에서 FAILED다. producer160.101초,
full ACK124.763초 미완료이며 `object.rs:593`의 batch child validation 진입
checkpoint가 BuildPrepare deadline을 관측했다. 사용량은
8,658,991R/5,917,929W/6,968,641,937B/1,581,697M이며 수치·메모리·외부 자원
거절은 없다. 실행 직전 45초의 1분 load는38.92~75.36이었다. Off는 세부
trace가 없고, lo11과 끝점·호스트 조건이 달라 사용량 차이만으로 B7 효과를
판정하지 않는다.

### 계측 후 B7 Record

`654d204…`의 lr12는 원 120초에서 FAILED다. producer159.261초,
full ACK124.740초 미완료다. Catalog child adopt 후 checkpoint에서 deadline을
관측했다. 세부 child capture는 COMPLETE, 923,752 events, loss/open/lease0이다.
원본은 외부 `qib6-_0r6mac7/lr12`의 execution·refusal·child trace에 있다.

| lr12의 도달 범위 | wall / thread CPU | 작업량 |
| --- | ---: | --- |
| Posting 전체 | 61.332s / 18.959s | 첫 page 완료, 둘째 page 실패 |
| Posting map apply | 60.829s / 18.708s | 두 page에서 후보64 / 검증 재사용64 / build 시도64 |
| Posting reserve | 5.550s / 0.788s | 46 packets |
| Posting install | 27.428s / 9.603s | 46 packets 성공, 11,759 inputs |
| Posting confirm | 19.792s / 4.417s | 45 packets 성공, 1 실패 |
| reserve·confirm 내부 SQLite COMMIT | 21.179s / 3.921s | 91회 |
| 설치 중 namespace 검사 | 1.180s / 0.387s | 11,805회 |
| 설치 중 writer 생성 | 10.515s / 5.587s | 10,387회, allowance와 실제 open 포함 |
| 설치 중 파일 sync | 4.336s / 0.273s | 10,387회 |

첫 page는16.559초/CPU9.211초, 둘째는44.270초/CPU9.497초였다.
두 page 모두32개씩 root build를 생략했고 설치 입력은 각각5,871/5,888개였다.
이는 재사용 동작의 직접 증거다. 다른 term 구간과 호스트 조건의 B5 대비
제품 속도 비교에는 쓰지 않는다. 표의 install 내부 phase와 상위 wall은
포함 관계다. non-Posting에도 writer 생성21,896회, wall10.159초/CPU9.039초가
있었다. reserve·confirm wall25.342초 중 21.179초가 COMMIT이었다. namespace
미세 최적화보다 writer와 기존 내구성을 유지하는 커밋 횟수 절감을 우선한다.
같은 managed packet 안에서 COMMIT 구간에 포함된 VFS sync는228회·wall5.966초였다.
모두 같은 FD의 full sync 성공을 기록했다. COMMIT 전체를 sync syscall이나
Catalog lock wait 한 가지로 설명할 수 없다.

실행 중 외부 host 표본32개의 load1은21.91~64.54였다. 표본 구간156.49초 동안
호스트 전체 swap in/out은 16KiB page 기준937,474/892,837개였다. 첫·둘째
page의 wall−CPU 차이와 일치하는 경합 정황이며, 호스트 paging·파일시스템·
lock 대기의 정확한 인과 배분은 없다. Runtime parent anchor 작업을 이 lane에서
중복 구현하지 않는다. B8은 관측된 재사용 leader의 두 scratch 순회를 한 번으로
줄이는 수리이며 나머지 물리 설치 비용을 별도로 검증한다.

### B8 재검증

f47에서 새 손상 거절·빈 EOF를 포함한 Rust 동작9개가 통과했다. sync label의
Rust/schema 순서 불일치 1개는 enum 집합을 유지한 채 수정해 f48에서 통과했다.
두 근거 사이에 Rust 본문 변경은 없다. 최종 `6df7803…`의 r14 pair build도
통과했다. 원 Large lo13 Off는 producer149.444초, ACK123.344초 미완료로
FAILED다. `text_spool.rs:1464`의 Posting cardinality loop에서 deadline을
확인했다. 사용량은13,240,653R/8,294,801W/10,364,017,241B/3,219,987M이며
수치·메모리·외부 자원 거절은 없다. 전체 ACK·검색·재오픈은 미도달이다.

lo13 실행 직전 load1은18.87~27.57이었다. 실행 전후 호스트 전체 swap in/out은
16KiB page 기준167,903/129,587개였다. lo12보다 더 많은 작업량을 처리했지만
호스트 조건과 도달 끝점이 달라 B8의 제품 속도 효과는 미확정이다. lr12에서
cardinality 두 page에 걸린 시간은0.251초였다. lo13 checkpoint만 보고
cardinality를 새 병목으로 선택하지 않는다.

현재 Catalog API의 예약 COMMIT은 파일 생성 전에 provisional intent 및
reclaim 보호를 영속화한다. 확인 COMMIT은 다음 chunk가 사용할 child를
등록한다. 이 계약과 현재256개/bytes cap을 유지한 채 두 COMMIT을 합치는
로컬 경로는 확인되지 않았다. 다음 구조 수리는 물리 writer의 allowance·
temp anchor·open 비용을 구분하고 기존 Runtime parent-anchor 변경의 범위를
대조한다. Catalog는 독립 namespace나 내구성 완화를 도입하지 않고
반복 가능한 COMMIT·sync 비용을 기준으로 transaction 경계 재설계를 평가한다.

### B9의 수리 경계

같은 lr12의 첫·둘째 Posting page에서 wall은16.559→44.270초, thread CPU는
9.211→9.497초다. 각 page의 COMMIT은5.420/1.814초와15.759/2.107초
(wall/CPU), writer 생성은4.354/3.141초와6.161/2.446초였다. 둘째 page는
기존 파일1,372개도 검증했다. 같은 실행 안에서도 대기가 크게 달라졌고,
off-CPU를 호스트 paging이나 저장장치 대기 한쪽에 모두 귀속할 수 없다.

수리 가능한 코드 비용은 writer의 동일 FD 재조회와 두 packet 사이의 연속
Catalog COMMIT이다. 두 packet이 모두 찬 lr12와 비슷한 범위라면 46개의
reserve·confirm 쌍이 요구한 최대92회를 69회로 줄이는 구조다. 이 값은
구조상 횟수이며 새 제품 실행의 wall 개선으로 사용하지 않는다.
[B9 계약과 검증](WORKPLAN.md#b03-b9--writer-birth-검증-공유와-인접-catalog-경계-합치기).

`d2c6ff8…` lo14 Off는 producer159.143초, ACK123.651초 미완료로 FAILED다.
`map/completed_group.rs:140`에서 BuildPrepare deadline을 확인했다. 사용량은
14,010,583R/8,609,851W/11,188,135,972B/3,442,595M이며 수치·메모리·외부
자원 거절은 없다. 실행 중 host load1은25.08~31.58이다. 결과는 원 제품 실패다.
Off checkpoint와 작업량만으로 실행 구간별 비용·제품 개선을 판정하지 않는다.

같은 pair의 lr13 Record도 producer156.479초, ACK123.934초 미완료로 FAILED다.
`fence_counter.rs:167`에서 BuildPrepare deadline을 확인했다. child는 COMPLETE,
1,009,588 events·loss/open/lease0이다. 첫 두 Posting page는 완료했고 세 번째
page가 중단됐다.

| 동일한 첫 Posting page | B7 lr12 | B9 lr13 |
| --- | ---: | ---: |
| 물리 install packets / inputs | 23 / 5,871 | 23 / 5,871 |
| SQLite COMMIT 횟수 | 46 | 35 |
| SQLite COMMIT wall / thread CPU | 5.420s / 1.814s | 5.142s / 2.131s |
| Map apply wall / thread CPU | 16.559s / 9.211s | 15.977s / 9.938s |

lr13 전체 Posting 도달 구간의 원자 전환은27회 성공·1회 실패였고, 성공한
COMMIT은84회다. 성공한 전환 하나가 두 원래 Catalog 단계의 commit 하나를
제거했다. 전체 Posting은47.164초/CPU25.768초, install은18.852초/CPU12.061초,
그 안의 writer 생성은11,796회·8.495초/CPU6.744초다. Catalog COMMIT은
13.347초/CPU5.244초였다. 종료 지점과 호스트 조건이 달라 전체 속도 비율은
구하지 않는다. 코드의 transaction 수 절감은 동일 prefix에서 직접 확인됐다.

Off/Record의 실행 중 host load1은 각각25.08~31.58 / 20.11~39.06이다.
동시 표본에서 호스트 전체 swap in+out은 약9.19/5.94GiB다. CPU와 wall의
차이는 대기 시간을 드러내지만 프로세스 paging·장치 I/O의 분리는 남는다.
다음 writer 계측은 ticket 준비와 실제 create/open 구간에만 필요하다. 별도
Runtime parent-anchor 작업의 수용 결과와 대조하고 중복 경로를 만들지 않는다.
원120초 전체 ACK·검색·재오픈은 열린 상태다.

### B10 — Posting 전 contribution 설치

lr13의 실패한 LexicalBuild에서 Posting 전 구간은64.160초였다. inventory
prepare22.369초, map apply13.590초, inventory diff9.700초와 직접 자식인
contribution reserve/install/confirm3.507/5.513/6.243초가 포함된다.
contribution은64개씩64회 물리 설치했고 두 Catalog 단계에 COMMIT128회가
필요했다. cold에서 기존256개 물리 상한까지 묶으면 4,096개 기준 설치16회,
COMMIT32회가 되는 구조다. scratch SQL은 계속64행이며 현재 수치는 코드
구조에서 계산한 상한이다. 실제 횟수와 시간은 B10 실행으로 확인한다.

나머지 inventory occurrence→entry map은 다음 root를 읽기 전에 앞 packet의
확인이 필요하다. 두 경계는 그대로 유지한다. writer의 Runtime FD 인증은
owner/mode까지 전달하지 않으므로 Storage의 검사와 임의로 합칠 수 없다.
이번 변경은 기존 directory FD 검증과 writer 비용 분리까지 적용한다.
[B10 계약](WORKPLAN.md#b03-b10--cold-contribution-배치와-writer-잔여-비용-분리).

`7cedcb7…`의 lo15 Off와 lr14 Record는 BuildPrepare deadline으로 FAILED다.
producer169.385/170.784초, ACK125.545/124.883초 미완료이며 검색·재오픈은
미도달이다. lr14의 harness trace는 닫혔지만 daemon child는 1,008,882 events,
drop485·open1인 PARTIAL이다. 아래 값은 그 child에서 도달한 구간이다.

| 도달 구간 | 관측 결과 |
| --- | --- |
| contribution 설치 | 동일 4,096 input에서64→16 packets |
| LexicalBuild의 inventory·Posting 밖 COMMIT |130→34회 |
| Posting writer ticket 준비 |13,439회·7.011s wall / 5.587s thread CPU |
| Posting writer create/open |13,439회·1.634s / 1.077s |
| Posting writer 생성 witness |13,439회·0.148s / 0.112s |

동시 host load1은 Off21.53~33.21, Record16.51~28.51이었다. 표본 중 호스트
전체 swap in+out은 약50.35/37.54GiB다. 이 수치로 프로세스 stall을 직접
분리할 수 없고 두 실행을 tracing overhead 비교에 사용하지 않는다.

ticket 준비가 writer의 주 비용이다. 코드 대조에서 Native writer가 1바이트
예약과 최종 크기 증액을 연속 실행하는 것을 확인했다. 기존 claim 안에서
최종 bound를 예약하면 두 번째 root·maintenance 검사와 growth ticket 갱신을
제거할 수 있다. 생성 직전과 publish 경계의 독립 인증은 유지한다.
[B11 계약](WORKPLAN.md#b03-b11--native-writer의-최초-전체-크기-예약).

### B11 — 최초 전체 writer 예약 검증

`add8f4c…` f53의 Runtime writer12개와 r17 pair build는 통과했다. lo16 Off와
lr15 Record는 원120초에서 FAILED다. producer160.818/155.661초,
ACK124.449/124.080초 미완료이며 전체 ACK·검색·재오픈은 미도달이다.
Off는 `object.rs:773`, Record는 `text_spool.rs:1464`의 BuildPrepare checkpoint에서
deadline을 확인했다. lr15 daemon child는 COMPLETE, 1,005,615 events·loss/open0이다.

| 동일 첫 Posting page | B10 lr14에서 관측 | B11 lr15 |
| --- | ---: | ---: |
| 실제 writer 생성 | 5,871 | 5,871 |
| ticket 준비 wall / thread CPU | 2.809s / 2.507s | 1.366s / 1.334s |
| Posting map apply wall / thread CPU | 13.855s / 9.132s | 15.288s / 5.851s |

두 번째 완료 page도 writer 생성4,635회로 같았고 ticket 준비 CPU는
1.860→1.384초였다. 첫 page에서 CPU가 줄면서 wall은 늘었으므로
제품 속도는 따로 판정한다. B11은 1바이트 예약·증액 경로 제거와 동일 구간
ticket CPU 감소를 확인했다. 4,096 contribution의 16 packets도 complete child에서
재확인했다.

Off/Record의 동시 load1은 각각23.18~29.95 / 14.18~23.96, 표본 중 호스트
전체 swap in+out은 약55.09/68.83GiB다. 제품 수용 미완료를 호스트 한 가지로
귀속하지 않는다. 남은 writer temp anchor는 별도 Runtime owner 코드와 합류 후
동일 parent FD가 실제 create·정산에 이어지는 범위를 확인한다. contribution
wrapper 제거는 기존 정책과 이전 reader가 읽는 persisted 값의 호환 설계가
선행돼야 하므로 이번 변경에 포함하지 않았다.

## B12 — writer parent custody와 Catalog COMMIT 잔여 비용

B11의 동일 앞 두 Posting page는 물리 설치 그룹23/24개, Catalog COMMIT35/36회다.
COMMIT 합계는 wall13.021초와 thread CPU3.254초다. 같은 managed-object parent
아래의 유일한 COMMIT 구간과 시간 포함관계로 VFS 이벤트를 매칭했다. WAL sync
164회와 DB sync46회는 모두 성공·same FD·full-default였고, 해당 VFS 구간은
총5.913초였다. settlement와 checkpoint까지 합친 구간 합집합은11.847초다.
VFS sync 구간에는 native 호출과 사후 witness 검증이 함께 들어간다. 따라서
off-CPU9.767초를 디스크 대기와 호스트 스케줄링으로 정확히 나눌 수는 없다.
Catalog의 WAL, `synchronous=FULL`, `fullfsync=ON`, IMMEDIATE write transaction과
기존 인접 두 설치 그룹의 atomic advance를 확인했다.

연속 그룹까지 advance를 확장하면 두 page의 COMMIT이 이론상71→49회다.
현재 구현은 combined 64MiB, sorted union gate, 동일 attempt, 전 단계의 물리
내구성을 함께 보존한다. Rolling 연결에는 기존 provisional custody를 유지하면서
gate를 재획득할 때 GC·취소·재시도·교차 실행이 안전하다는 증명이 필요하다.
한 단계의 SQLite 설정을 바꾸거나 트랜잭션 경계를 제거해 이 비용을 줄일 근거는 없다.

B12는 최종 claim에서 연 부모 FD를 임시 이름에 재사용하고, 생성과 no-replace
publish를 held FD에서 실행한다. 미등록 FD도 기존 qualified cleanup에 연결했다.
초기 고정본 `8f114a2…` f54는18/19개였다. Retarget 후 retry case가 ticket의
allowance 종료 전 snapshot을 비교해 실패했고, 종료 뒤 실제 환급을 확인하도록 교정했다.

추가 감사에서 경로로 연 대체 shard를 fsync한 뒤 원래 shard가 복원되는 ABA를
확인했다. Live pathname과 held parent가 정산 시 다시 같아져도 실제 rename
대상에 대한 fsync를 증명할 수 없다. B12 후속은 기존 barrier·fsync 횟수를
유지하면서 실제 sync한 objects와 shard FD의 식별자를 정산에 전달한다.
Objects FD는 전체 batch의 정산이 끝날 때까지 보유한다. Runtime은 기존 nofollow
ancestor walk에서 두 식별자와 원래 held shard 및 inode를 확인한다.
`2e19d9d…` f55는 실제 fsync 후 ABA, 잘못된 objects inode, 원본 FD 취소,
scalar/batch·Catalog replay를 포함해23개가 통과했다.

같은 고정 소스의 Large Off/Record는 둘 다 원120초 `BuildPrepare` deadline으로
실패했다. Off는 Storage batch settlement 뒤 checkpoint, Record는 다음 native
writer admission에서 거절됐다. client ACK 구간은124.819/147.588초, producer는
156.646/179.860초다. 숫자 work cap·memory·외부 admission 거절은 관측되지
않았으며 검색·재오픈에는 들어가지 못했다. Record child는 COMPLETE이고
1,100,192 events, loss/open0이다.

B11과 B12 Record에서 정상 완료한 동일 첫 세 Posting page를 대조했다.
두 실행 모두 Catalog 입력18,015개·실제 writer14,486개·COMMIT107회다.

| 구간 | B11 thread CPU | B12 thread CPU |
| --- | ---: | ---: |
| writer ticket prepare | 3.962초 | 4.838초 |
| writer open | 0.908초 | 2.455초 |
| publish/retire | 1.451초 | 3.437초 |
| settlement | 0.682초 | 2.197초 |
| 네 writer 구간 합계 | 7.004초 | 12.928초 |
| Catalog COMMIT | 5.566초 | 5.939초 |
| MapApply | 22.613초 | 30.190초 |

동일 작업의 writer CPU 증가는 새 parent 확인·FD 인증 경계에 집중된다.
B12 Off/Record의 host load1은14.41~21.26/15.83~56.00, 호스트 전체 swap
in+out은 약39.92/73.94GiB였다. Off/Record의 ACK 차이는 순수 trace overhead로
귀속할 수 없다. B13은 fsync identity 보존과 실제 open·rename 직전 확인은
유지하면서 그 사이의 중복 parent 확인을 줄인다.

## B13 — writer CPU 회수와 최상위 map 잔여

`44486c9…` f57은 Runtime·Storage23개를 통과했다. r19 pair build를 고정해
seed42·4,096 source·16 repo·원120초로 실행했다. Off/Record의 producer는
153.303/153.167초, client ACK 구간은124.150/124.491초였고 둘 다 full ACK
미완료다. Record daemon의 실제 요청은120.149초에 거절됐다. 양쪽 모두
Storage batch settlement 뒤 checkpoint에서 `BuildPrepare` deadline에 걸렸다.
정량 work cap·메모리·외부 자원 거절은 없었다. 검색·재오픈은 미도달이다.

Record child는 COMPLETE, 1,203,874 events, loss/open0이다. B12와 동일한
앞 세 Posting page의 입력18,015개·writer14,486개·COMMIT107회에서 네 writer
구간 thread CPU가12.928→8.269초로 감소했다. 세부 구간은 ticket 준비
4.838→2.381초, open2.455→1.821초, publish/retire3.437→2.409초,
settlement2.197→1.657초다. MapApply CPU는30.190→19.561초였다. Off/Record의
host load1은16.41~23.30/14.39~30.81, 호스트 전체 swap in+out은
약17.42/33.05GiB여서 순수 tracing overhead는 판정할 수 없다.

B13 Record는 개별 Posting page 이후 `NativeTextPostings` lane39의 최상위
terms/owners `apply_stream`에 도달했다. 실패 prefix의 MapApply는
wall7.057초·thread CPU4.423초였다. 물리 설치는14개 그룹·3,584개 writer,
Catalog COMMIT은20회·wall2.664초·thread CPU1.039초였다. Lexical BuildPrepare
112.737초 중 Posting 전체 구간은58.437초였다. 최상위 map의 전체 실행 시간은
관측되지 않았다. 이 위치를 deadline의 마지막 비용으로 식별했고, 제품 전체
120초 통과 조건은 열어 둔다.

### B13 요청 전체 원인 분해

`lr17`의 닫힌 daemon child에서 실패한 요청의 dispatch·build span과 하위
구간을 재집계했다. Daemon은 queue admission 이후 원120초 budget을 발급한다.
Harness client와 외부 producer는 각각600초 상한이다. Trace상 서버
queue wait는 반올림0초였고, 120.149초는 실패한 daemon 요청 span이다. Client
ACK 구간124.491초에는 read-view·mutation staging·submit·status가 들어가며,
producer153.167초는 별도 source staging과 프로세스 수명도 포함한다.
Record의 reads는 원 cap의 약21%, bytes는 약2%에서 deadline이 먼저 거절했다.
[Native 정책](../../../crates/quanta-index-searchd-runtime/src/native.rs#L104),
[서버 budget](../../../crates/quanta-index-ipc/src/server.rs#L1870),
[harness client 상한](../../../crates/quanta-index-searchd-harness/src/scale/native_index.rs#L70).

| 구간 | Wall | 관측 thread CPU |
| --- | ---: | ---: |
| Daemon 요청 | 120.149초 | — |
| BuildPrepare 진입 전 | 7.349초 | — |
| Lexical build | 112.737초 | 60.952초 |
| ↳ Posting 전체 | 58.437초 | 30.574초 |
| ↳ Inventory 준비·diff·map 적용 | 44.456초 | 24.271초 |
| ↳ 나머지 lexical 구간 | 9.844초 | — |

같은 BuildPrepare prefix의 관리 객체 설치는254개 그룹·39,555개 입력·실제
writer33,019개였다. 물리 설치 span은 wall37.625초·thread CPU25.449초다.
이 중 데이터 write 호출의 wall 합계는1.880초였고, writer 준비·open·
publish/retire·settlement의 thread CPU 합계는20.700초였다.
Catalog 관리 COMMIT은453회·wall32.290초·thread CPU9.361초였다. 동일
관리 호출 부모와 COMMIT 구간 포함관계로 WAL/DB VFS sync994회를 매칭했고,
그 구간은13.772초다. 이 값에는 native sync와 VFS witness가 포함된다.
별도 객체 shared barrier254회의 wall은4.595초다. 모두 위 단계에 이미
포함된 값이다. Lexical wall과 관측 thread CPU의 차이51.785초에는 sync,
스케줄링, 계측 범위 밖의 대기가 포함될 수 있다. 호스트 swap과 daemon의
대기를 같은 양으로 환산할 수 없다.

마지막 `terms/owners` 적용에서는 기존 top scratch를 읽어 MapScratch에 edit를
다시 기록한다. 중복 staging은 코드에서 확인된다. 관측된7.057초 prefix에는
물리 설치14개 그룹3.146초와 Catalog COMMIT20회2.664초도 들어간다. 중복
staging의 독립 시간과 최상위 map의 완료 시간은 미측정이다. 해당 시점의
`object.rs:1049`는 settlement 이후 deadline checkpoint다.
[top stream](../../../crates/quanta-index-lexical/src/native/text.rs#L9981),
[Storage edit staging](../../../crates/quanta-index-storage/src/map.rs#L2073).

완료된 첫 네 Posting page의 물리 그룹90개는 기존 pair advance에서
COMMIT136회였다. 각 페이지에서 모든 인접 그룹을 연쇄로 묶는다면 수학적
최솟값은 합계94회다. 절감량으로 사용할 수치는 아니다. 새 계약에서
64MiB·sorted gate·provisional custody와 취소·GC·재시작을 보존해야 한다.
현재 file/dir sync와 Catalog WAL/FULL/fullfsync는 내구성 경계다.
[Catalog 연결 정책](../../../crates/quanta-index-catalog/src/connection.rs#L465).

로컬 Off·Record 모두 실행 전 host settle 조건을 충족하지 못했다. 원 E07은
전용 Linux aarch64 2CPU/6GiB·local SSD와 paired Off/Record를 요구한다.
같은 고정 실행 파일의 원120초 Off를 경합이 줄어든 환경에서 먼저 확인한다.
여전히 실패하면 별도 진단 후보의 Native 서버 budget을 늘려 완주 시간을
측정한다. Client watchdog과 Query 서버 budget은 이 Native 제한을 바꾸지
않는다. 진단 후보의 완주 결과는 원120초 수용과 분리한다.
[E07 고정 profile](../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles).

## B14 — 물리 그룹 축소와 기존 advance 연결

B13의 동일 Large fixture에서 occurrence와 inventory block은 각각
64개 객체 × 64그룹이었다. Posting의 큰 그룹은 대부분 이미256개였다.
초기 empty-base inventory 창을128개로 확대하고 SQL row page64는 분리했다.
Reachable Map은 합계64MiB 이내의 최대4개 기존 물리 packet을 하나의
sorted gate union과 연속 Catalog advance로 처리한다. 각 packet의
파일·디렉터리 barrier와 정확한 FD 정산이 다음 advance보다 먼저 끝난다.

초기 r21 Large Off는17.280초의 ACK 구간에서 `BuildPrepare`의
`ResourceLimited`로 실패했고 `WorkBudget.first_denial`은 `none`이었다.
같은 pair의 별도 Medium Record는 occurrence128개 물리 설치를 완료한 뒤
첫 `NativeTextInventoryMapApply`에서 실패했다. child17,472 events,
loss/open0이다. 빈 inventory에서 64개를 초과하는 Block을 준비한 후
공용 staging 생성자의64개 검사에 걸리는 소스 경로와 일치한다.
`60db2fc…`는 cold inventory만 기존 물리 packet 상한을 사용하는 생성자로
연결했다. ACK·검색·재오픈 판정은 이 교정본의 실행에 귀속한다.

교정본 Large Record의 B13 `lr17` 대 B14 `lr19` 비교는 occurrence와 inventory
block 각각의 동일한 첫 2,816개 입력으로 제한한다. 각 경로에서 44×64가
22×128 물리 배치로 줄었다. 두 경로 합산 물리 배치는 88→44, reserve·confirm
Catalog COMMIT은 176→88이다. 실제 설치하는 고유 객체 수는 동일하다.
`lr19`는 이후 inventory scratch 단계에서 종료돼 전체 요청의 절감량은 미측정이다.

Large Off `lo20`은 batch namespace checkpoint에서, Record `lr19`는 scratch
SQLite edit write에서 원120초 deadline을 관측했다. 두 지점은 예산이 소진된
시점이다. 공유 Mac의 1분 load 관측 범위는 각각 28.48–37.95와 37.03–123.56이다.
실행 사이 경합 차이 때문에 ACK 차이로 코드 속도나 Record 오버헤드를 산출할 수 없다.
두 실행 모두 ACK 미완료였고 `lr19`는 최상위 map에 도달하지 않았다.

같은 pair의 Medium `mb15`에서 첫 최상위 terms/owners map은 1,512.5ms였다.

| 단계 | Wall time |
| --- | ---: |
| Cursor·검증·재적재 | 6.3ms |
| 위 단계에 포함된 edit scratch 쓰기 54회 | 2.6ms |
| Scratch replay·radix 구축 | 19.6ms |
| Scratch COMMIT | 10.1ms |
| 물리 설치 | 1,436.8ms |

물리 설치 구간에서 그룹 크기는 256·256·120개였다. Catalog COMMIT은 최초
reserve 1회, advance 2회, 최종 confirm 1회였다. 네 개 map 하위 단계의 합계는
edit scratch 쓰기를 중복 계상하지 않으며, 상위 span은 이 밖의 작업도 포함한다.
수집기는 v5 `map_apply` lane의 CPU counter와 부모 관계를 검사한다. 교정한
수집기로 보존된 first/reopen trace를 오프라인 검증했다. Medium 수치를 Large의
최상위 map 비용으로 환산할 근거는 없다.

## B15 — Inventory scratch와 물리 writer 병목

`lr19`가 120초 제한을 관측한 경로는 `BuildPrepare → LexicalBuild →
NativeTextInventoryDiff → NativeTextPostingEdit/33`이다. `edit_write.rs`에서
`raw_execute()`가 반환된 직후 checkpoint가 deadline을 거절했다. 마지막 호출은
135ms였다. 최상위 terms/owners `MapApply`는 미도달이며 앞에서 관측된 Storage
`map_apply` 34개는 모두 member assembly 소속이다. 22개 inventory map 배치는
EntryMap Block 파일을 설치했다.

| 동일 seed42·corpus의 비교 구간 | B13 `lr17` wall / thread CPU | B14 `lr19` wall / thread CPU |
| --- | ---: | ---: |
| 성공한 inventory posting edit 첫 11,769회 | 3.837 / 2.329초 | 18.032 / 3.296초 |
| 첫 occurrence 2,816개 물리 설치 | 4.319 / 2.546초 | 8.051 / 3.148초 |
| 첫 inventory Block 2,816개 물리 설치 | 4.111 / 2.551초 | 10.632 / 3.256초 |
| 위 occurrence Catalog COMMIT 88→44회 | 3.969 / 0.626초 | 7.521 / 0.426초 |
| 위 inventory Block Catalog COMMIT 88→44회 | 5.039 / 1.206초 | 9.809 / 1.441초 |

Posting edit 비교는 성공 span 순번을 맞췄다. trace에 행별 식별자는 없다.
`lr19`의 inventory diff 하위 posting edit 전체는 11,770회·wall 19.125초·thread CPU
3.324초이고, 중첩된 SQLite VFS lane22의 wall은 14.148초다. 따라서 두 시간을
합산하면 중복이다. 초기 posting edit는 owner 사이에서 유지되고 64행을 SQL 한 번으로
실행하므로 해당 호출 수 자체를 중복 쓰기로 분류할 근거가 없다.

`lr19`의 물리 설치는 새 writer 17,800개를 처리했다. 누적 wall/thread CPU는
ticket 준비 9.065/5.137초, open 5.952/3.835초, publish·retire 9.672/5.163초,
settlement 5.645/3.241초, 데이터 쓰기 2.291/0.649초였다. Barrier는 wall
3.215초·thread CPU 약 7ms였고 물리 쓰기와 sync 오류는 0건이었다.

`lr19` LexicalBuild의 wall 105.240초 중 해당 thread CPU는 26.239초였다.
같은 실행의 호스트 load1은 37.03–123.56, 호스트 전체 swap in+out은 약 46GiB였다.
비교 구간에서 wall 증가가 CPU 증가를 크게 초과했고 횟수가 줄어든 COMMIT에서도
같은 패턴이 관측됐다. 이 차이에는 SQLite 및 파일시스템 대기, lock 대기와
스케줄링 지연이 포함된다. trace가 각 항목의 시간이나 quiet-host 결과를 직접 측정하지 않는다.

코드에서 제거한 작업은 동일 값의 과금용 재인코딩, 동일 scratch 예약량 재조회,
동일 inode 확인 직후의 배치 writer 재조회다. 물리 파일 수와 내구성 sync 횟수,
논리 work caps는 기존 계약을 따른다. 이 변경의 120초 수용은 [실행 결과](README.md#현재-판정)에 기록한다.

### B15 교정본의 재검증

| 같은 비교 구간 | B14 `lr19` wall / thread CPU | B15 `lr20` wall / thread CPU |
| --- | ---: | ---: |
| 성공한 inventory posting edit 첫 11,769회 | 18.032 / 3.296초 | 2.892 / 2.429초 |
| 첫 occurrence 2,816개 물리 설치 | 8.051 / 3.148초 | 3.666 / 2.613초 |
| 첫 inventory Block 2,816개 물리 설치 | 10.632 / 3.256초 | 4.047 / 2.682초 |

`lr20`은 occurrence와 Block을 각각 32×128개 설치했다. 두 경로 합계 Catalog
COMMIT은 128회였다. 첫 두 Posting page는 각각 5,871·6,029개 입력과
23·24개 물리 배치를 처리했다. B13 동일 페이지의 COMMIT 35·36회 대비 29·30회였다.
세 번째 Posting page는 17개 물리 배치를 시작한 뒤 namespace checkpoint에서
원 `BuildPrepare` deadline에 걸렸다. 실패한 page의 MapApply 9.382초 중
물리 설치 lane은 7.825초였다. 최상위 map과 ACK·검색·재오픈은 미도달이다.

호스트 load1은 `lo21` 39.05–52.64, `lr20` 33.46–51.92였고 swap in+out은
각각 6.82GiB, 0.26GiB였다. `lr19`의 46GiB swap과 환경이 달랐다. 동일 구간의
CPU 감소는 관측됐으나 코드 단독 wall 개선폭이나 120초 여유는 이 비교로 확정할
수 없다. swap이 적었던 Record에서도 deadline이 남았으므로 swap 수치만으로
실패 전체를 설명할 수 없다.

Medium `mb16`은 동일 r25 pair의 256-source 기능 검증이다. ACK 4.789초,
첫 검색 65.5ms, 증분·삭제 ACK 및 검색, 재오픈 후 검색과 최종 drain이 완료됐다.
두 child trace와 canonical profile이 검증됐다. Large 120초와 경합을 통제한
비교는 계속 열린 상태다.

## B16 — Posting writer와 Catalog COMMIT

B15 `lr20`에서 세 번째 Posting page는 17×256개 입력의 물리 배치를 시작했다.
16개 배치가 barrier에 도달했고 마지막 배치는 namespace checkpoint의 원120초
deadline으로 종료됐다. 이 페이지의 MapApply 9.382초 중 설치 lane은 7.825초였다.
설치 lane 안에 아래 writer·COMMIT·barrier 구간이 포함된다.

| 구간 | 횟수 | Wall / thread CPU |
| --- | ---: | ---: |
| 신규 writer ticket 준비 | 2,615 | 0.705 / 0.638초 |
| 신규 writer open | 2,615 | 0.555 / 0.488초 |
| 게시 및 writer 종료 | 2,615 신규 파일의 처리 구간 | 0.731 / 0.618초 |
| 정산 | 배치별 정산 구간 | 0.401 / 0.391초 |
| 기존 객체 바이트 검증 | 1,591 | 1.124 / 0.594초 |
| Barrier | 16 | 0.322 / 약 0.001초 |
| Catalog reserve COMMIT | 5 | 0.447 / 0.147초 |
| Catalog advance COMMIT | 12 | 1.755 / 0.865초 |
| Catalog confirm COMMIT | 4 | 0.443 / 0.203초 |

COMMIT의 wall−thread CPU는 1.430초, 신규 writer의 네 구간 합계는 0.257초다.
SQLite와 파일시스템의 대기, lock 대기, 호스트 스케줄링이 모두 이 차이에 들어갈 수 있다.
기존 두 Posting page의 같은 입력·물리 배치 수에서 B13→B15 COMMIT은 71→59회,
wall 7.291→9.643초, thread CPU 2.488→3.262초였다. B13 host load1 14.39–30.81,
B15 33.46–51.92로 다르고 swap 변화는 반대 방향이어서 단독 원인을 확정할 수 없다.

기존 writer가 수행하는 held-parent·원본 FD 인증, no-replace 게시, 파일 및 디렉터리
동기화, 정산과 기존 객체의 바이트 검증은 유지한다. 수정한 반복은 최대4그룹마다
마지막 confirm과 다음 reserve를 각각 COMMIT하던 경계다. 최대8그룹을 연결해
동일한 23·24개 물리 배치가 64MiB 창에 들어갈 때 첫 두 페이지의 COMMIT은
구조상 29→26회, 30→27회다. 창 안의 메모리 승인과 256개 gate 상한은 유지되며,
실제 동시 요청의 gate 대기와 Large 수용 여부는 실행 결과에 귀속한다.

### B16 교정본의 재검증

`adf4c8ed…` 집중 회귀의 8그룹 성공 경로는 reserve 1회, advance 7회,
confirm 1회 순서였다. 9그룹 및 64MiB 초과는 첫 효과 전에 거절됐고, 후반
advance 실패 시 이후 파일은 만들어지지 않았다. 합계 9회 트랜잭션의 전이
순서는 확인했다. B15의 첫 두 Large 페이지를 같은 입력으로 비교한 횟수는 미측정이다.

`lo22` Off는 posting cardinality 쿼리의 checkpoint에서, `lr21` Record는 첫
Posting page `MapApply/3`의 radix 구축에서 `BuildPrepare` deadline을 관측했다.
`lr21`은 두 inventory 경로 각각 4,096개를 설치했지만 Posting의 `MapApply/5`
물리 설치에는 진입하지 못했다. 따라서 B15 세 번째 페이지의 writer 시간이나
COMMIT 횟수와 B16 Large를 직접 비교할 수 없다. 수치 work cap 거절은 없었다.

Off와 Record의 호스트 load1은 각각 25.21–40.74, 30.32–54.69였고 호스트 전체
swap in+out은 각각 86.78GiB, 55.63GiB였다. B15 `lr20`은 0.26GiB였다.
이번 실패 위치는 B15보다 앞이었으며 대기 시간을 B16 코드 변화에 귀속할 수 없다.
같은 r26 pair의 Medium에서는 ACK·검색·증분·삭제·재오픈과 최종 drain이 통과했다.
[실행 판정](README.md#현재-판정).

### 2026-11-09 후속 감사

- Main의 cold case-paired Posting, 분리 cardinality 범위와 childless witness
  구현은 B16 실행 소스와 동일하다. f61에서 case 페이지 경계·전체 키·잘못된
  surface와 witness Small/Tree 전이·CAS 회귀가 통과했다.
- 같은 첫 Posting 페이지에서 scratch write lane2는 두 실행 모두 3,767회다.
  B15 `lr20` wall/thread CPU는 0.511/0.447초, B16 `lr21`은 2.769/0.857초다.
  `lr21`의 cardinality는 0.267초에 완료했고, radix는 0.458초 진행 뒤 deadline에
  걸렸다. 실패 지점만으로 cardinality나 radix를 새 병목으로 선정할 근거는 없다.
- `mb17`의 Posting 페이지는 각각 최대 2개 물리 배치다. f63의 8그룹 전이와
  실제 Catalog 원자 전이는 검증됐으나, 일반 Posting 호출이 4그룹 경계를 넘을 때의
  실제 COMMIT 수를 관측한 제품 trace는 없다. 5–8개 물리 배치를 만드는 한 페이지에서
  이전 4그룹과 현 8그룹의 COMMIT 수, 같은 입력·출력·내구성 순서를 대조하는 것이
  가장 좁은 다음 검증이다. 원 Large 120초의 full ACK·검색·재오픈은 별도 수용 조건이다.

## B17 — 설치 창 리뷰 보완

B16의 64MiB는 wire 합계다. `MapBuffer`의 backing, `PreparedObject`의 live wire와
설치 전 `RegistrationBatch`가 요청 메모리를 함께 사용한다. 8배치는 기존 4배치에
들어가던 요청도 `MemoryScope`가 작은 경우 거절할 수 있다. 제품 경로에서 절감이
검증된 4배치 창으로 복원했다. 임의의 `ResourceLimited`를 잡아 재시도하면 이미
실행한 Catalog·파일 효과와 충돌할 수 있으므로 창을 설치 시작 전에 고정한다.

이전 gate 획득은 뒤쪽 gate가 잠겼을 때 낮은 순번의 prefix를 보유한 채 대기했다.
새 경로는 순서대로 한 번 시도한 뒤 충돌 시 prefix를 전부 놓고, 그 gate 하나만
기다린 뒤 다시 시도한다. 전체 gate를 얻은 뒤의 내구성·Catalog 전이 순서는 같다.
단위 회귀는 고의로 뒤쪽 gate를 잡고 실패한 union의 앞쪽 gate가 즉시 비는지 확인한다.

이전 관측은 atomic advance의 이전·다음 객체를 모두 confirm에 더했다. 새 관측은
이전 확인과 다음 예약을 각각 센다. 예를 들어 단일 객체 4배치가 성공하면 논리
예약·확인은 각각 4건이며 Catalog 전이는 reserve 1 + advance 3 + confirm 1이다.
한 advance 호출의 wall time은 기존 `confirm_elapsed`에 한 번만 기록한다.
atomic rollback은 두 논리 방향의 실패에 각각 반영한다. 기존 진단 필드와 trace
스키마는 유지한다. 이전 raw 요약의 reserve/confirm 수치는 이번 결과와 의미가 다르다.

### Large 수용 종료가 지연된 이유

B14–B16의 원 Large Off/Record 6회는 모두 `BuildPrepare`에서 서버 deadline을
맞았다. ACK 구간 123.633–126.587초는 실패가 반환된 시점이다. 완주에 4–7초가
더 필요하다는 계산에는 쓸 수 없다. B15 Record는 세 번째 Posting 물리 배치까지
도달했고 B16 Record는 첫 Posting 페이지의 radix 구축에서 종료됐다. B15 Record와
B16 Record의 호스트 전체 swap in/out은 0.26GiB와 55.63GiB였다. 서로 다른
진행 구간과 호스트 조건으로 전체 임계 경로와 필요한 절감량을 정하지 못했다.

B15의 동일 첫 두 페이지 COMMIT 71→59회는 직접 관측했다. B16의 8배치 추가
효과는 Large에서 미관측이며 B17은 메모리와 gate 검토에 따라 4배치를 복원했다.
B17의 18개 집중 회귀는 해당 코드 경계를 검증한다. 최신 소스의 최적화 pair와
Large ACK·검색·재오픈은 별도다.

종료 기준은 고정된 최신 소스·입력·work caps로 원 Large 서버120초의 full ACK를
받고 검색 및 재오픈 후 검색까지 확인하는 것이다. 경합을 통제한 Off가 실패하면
같은 소스의 Record와 전체 임계 경로를 볼 수 있는 완주 진단을 별도로 묶는다.
진단에서 확인된 반복 비용과 필요한 절감량으로 다음 코드 범위를 정한다. 진단 결과는
원120초 수용에 포함하지 않는다. 공유 호스트의 load·swap으로 실행 비교가 오염되면
해당 결과는 로컬 진단으로 남기고 수용 근거를 분리한다.

### 동일 작업량에서 첫 Posting 진입이 41초 밀린 구간

seed42의 B15 `lr20`과 B16 `lr21`은 각각 전체 4,096 owner의 inventory를 완료했다.
서버 요청 시작에서 첫 Posting 진입까지 70.668→112.041초로 41.373초 늘었다.
`NativeSourceApply`는 11.228→12.447초였다. Lexical 진입 후 Posting 전까지는
58.304→98.881초로 40.577초 증가했다. 같은 구간 thread CPU는 Lexical의
전체 CPU에서 Posting 자식 CPU를 빼면 약 33.43→37.80초다. 따라서 약 36초는
해당 스레드 CPU 증가로 설명되지 않는다.

| 겹치지 않는 Posting 이전 구간 | 두 실행의 횟수 | B15 wall / thread CPU | B16 wall / thread CPU |
| --- | ---: | ---: | ---: |
| occurrence·inventory Block·contribution 물리 설치 | 80배치, write·file sync 각각 12,288회 | 16.924 / 11.493초 | 29.040 / 12.981초 |
| 같은 배치의 Catalog COMMIT | 160회 | 14.332 / 3.146초 | 25.324 / 4.383초 |
| Inventory diff 내부 posting edit | 17,899회 | 5.221 / 3.826초 | 11.103 / 4.600초 |

세 행은 trace 시간 구간이 겹치지 않으며 합계 28.99초가 늘었다. VFS lane22는
상위 COMMIT 또는 edit에 포함되므로 위 수치에 더하지 않는다. B16의 유일한 제품
코드 변경은 reachable Map 설치 창 4→8이었다. 위 80배치는 occurrence 32×128,
Block 32×128, contribution 16×256의 직접 `put_batch` 경로로 처리돼 이 창의
대상이 아니었다. 처음 Posting을 시작할 때 B16에 남은 예산은 약 8초였다.
B15 첫 페이지 MapApply만 14.564초였고 그중 13.159초가 물리 설치 lane이었다.
두 호스트의 swap in/out 0.26GiB와 55.63GiB는 강한 교란 증거이며, 이 trace로
파일시스템 대기와 스케줄링의 개별 기여를 분리할 수 없다.

확인된 코드 후보는 `native/text/contribution.rs`의 인라인 경계다. 현재 Small만
인라인으로 바뀌고 Block·Tree는 `ContributionWrites::flush`에서 별도 파일로
설치한다. 같은 입력에서 contribution 파일만 4,096개이며 물리 설치와 reserve·confirm
전체가 9.920→18.840초였다. 기존 `MapValue::Inline`에 Block/Tree 구분과 token count를
담고 source record와 inventory root를 실제 child 두 개로 유지할 수 있다. 구현 시
기존 object/Small decoder와 새 tag, GC·pin·재오픈 계약을 함께 검증한다. Owners map의
wire와 객체 수가 달라질 수 있어 위 wall 전체를 순 절감으로 간주하지 않는다.

## B18 — Block·Tree owner contribution 인라인

이전 `ContributionWrites::flush`는 `Small` 외의 Block·Tree contribution을 별도
Object로 설치했다. 새 codec은 기존 MapValue wire 안에서 `0x80` Block과 `0x81` Tree,
8바이트 BE token count, 정확한 두 child `[source record, inventory root]`를 사용한다.
Map root의 직접 참조가 소유권을 유지한다. reader는 기존 Object·Small과 새 tag를
구분하고 잘못된 폭·child 수·알 수 없는 tag를 거절한다. output policy에는 새 layout
stamp를 넣었다. 이전 정책의 member를 새 프로파일로 선택하려면 새 artifact가 필요하다.

같은 B15/B16 prefix에서 이 변경이 직접 겨냥한 범위는 4,096개 wrapper와 32회
Catalog COMMIT이다. occurrence와 inventory Block은 각각 4,096개, 32×128개 배치로
남는다. 두 경로는 occurrence가 durable/confirmed된 뒤 Block을 만드는 계약을 사용한다.
현재 cold inventory 창128과 posting edit의 64행 SQL 배치를 재구현하지 않는다.
이후 Owners map의 설치량도 함께 보아 실제 순절감을 판정한다.

### B18 실행 결과

`ccd5ab92…`의 f68 집중 회귀 220개와 r27 최적화 build가 통과했다. 동일 pair의
seed42·4,096 sources·server 120초 Off `lo23` 및 Record `lr22`는 `BuildPrepare`
deadline으로 full ACK가 끝나지 않았다. `lo23`은 완료 그룹 scratch의 checkpoint,
`lr22`는 첫 Posting page 물리 설치의 namespace checkpoint에 걸렸다. 둘 다
numeric·memory·external-resource 거절은 없다. ACK 대기는 127.637/127.996초다.
공유 호스트 load1은 각각 34.24–65.27/36.46–72.42, swap in+out은 72.21/122.40GiB다.
해당 값은 호스트 구간 전체이며 한 요청의 swap량이나 tracing overhead로 환산할 수 없다.

`lr22` child trace는 889,561 events, drop/open 0으로 정상 종료했다. 같은 seed의
첫 Posting 전 완성 구간을 비교했다. 아래 선택 span의 시간 중복은 0이다.

| 구간 | B15 `lr20` | B16 `lr21` | B18 `lr22` |
| --- | ---: | ---: | ---: |
| Lexical 진입부터 첫 Posting까지 wall | 58.304초 | 98.881초 | 72.902초 |
| 실제 write / file sync | 12,288 / 12,288 | 12,288 / 12,288 | 8,192 / 8,192 |
| contribution 파일 설치 배치 | 16 × 256 | 16 × 256 | 0 |
| 전체 물리 배치 / wall | 80 / 16.924초 | 80 / 29.040초 | 64 / 16.742초 |
| Catalog COMMIT / wall | 160 / 14.332초 | 160 / 25.324초 | 128 / 22.791초 |
| scratch posting edit / wall | 17,899 / 5.221초 | 17,899 / 11.103초 | 17,899 / 6.486초 |

동일 prefix에서 write·fsync 각각 4,096회와 COMMIT 32회 감소가 확인됐다. wall의
25.980초 차이는 호스트가 달라 순수 코드 효과로 판정할 수 없다. B18에서는 여전히
occurrence와 inventory Block이 각각 4,096개를 쓴다. 첫 Posting page는 20×256개
설치 그룹을 시작했고 Catalog COMMIT span 24회·5.818초 뒤 중단됐다. 최상위
terms/owners Map 단계와 전체 ACK·검색·재오픈에는 도달하지 않았다.

동일 r27의 `mb18` Medium 256 sources는 full ACK 25.024초, 최초 검색 468.5ms,
증분·삭제·no-op ACK, pin 검색, 재오픈·검색·삭제 유지·drain을 완료했다. producer
exit0과 canonical artifact/profile을 검증했다. 전용 호스트 기준의 정식 성능 비교는
`NOT_RUN`이다.

### B19가 제거하는 inventory 파일

Large generator의 ASCII 파일에는 본문 120행과 고정 anchor 3행이 있다. 파일당
본문 고유 term은 최대 120개, 공통 term은 최대 23개다. case prefix를 두 가지로
써도 최대 286 keys다. 가장 긴 본문 고유 term은 14자, 공통 term은 27자다.
두 case의 position vector가 동일하므로 `QITXOC01`의 보수적 상한은
`16 + 2×120×19 + 2×23×32 + 2×143 + 4×367 = 7,802B`다.
이는 512 keys와 8,192B 한도 안이다. 파일별 고유 anchor가 객체마다 남는다.
고정 anchor만으로도 Small 한도 8 keys를 넘긴다. 따라서 이 입력의 `QITXEM02`
파일은 이미 저장된 단일 `QITXOC01`의 key를 되풀이하는 경우다.

일반 입력에 이 상한을 적용하지 않는다. B19 생산자가 실제 draft와 단일 occurrence의
key·ref·count를 검증한 뒤에만 파일 생략을 결정한다. 후보 인증 읽기와 key 비교는
새 비용이며, 파일·COMMIT 횟수와 실행 wall을 분리해 판정한다.

### B19 실행 결과

`046d2c02…`의 f69는 252개 통과, r28은 최적화 pair build를 통과했다. 같은
pair의 원 Large Off `lo24`와 Record `lr23`은 첫 ACK 전에 `BuildPrepare`
deadline으로 종료됐다. ACK 대기는 125.563/124.227초였다. numeric·memory·external
거절은 없다. Record child는 1,268,416 events, drop/open 0으로 종료했다.
호스트 load1은 Off 24.90–35.20, Record 22.91–30.37이고 swap in+out은
54.80/78.64GiB였다. B18과 부하가 달라 아래 wall 차이를 순수 코드 효과로
사용할 수 없다.

| 같은 Large 첫 Posting 전 | B18 `lr22` | B19 `lr23` |
| --- | ---: | ---: |
| Lexical 진입부터 첫 Posting wall | 72.902초 | 32.598초 |
| 실제 write / file sync | 8,192 / 8,192 | 4,096 / 4,096 |
| QITXEM02 물리 설치 배치 | 32 × 128 | 0 |
| 전체 물리 설치 배치 / wall | 64 / 16.742초 | 32 / 4.556초 |
| Catalog COMMIT / wall | 128 / 22.791초 | 64 / 3.945초 |
| scratch posting edit / wall | 17,899 / 6.486초 | 17,899 / 4.262초 |

남은 `NativeTextInventoryMapApply`의 32회·3.062초에는 후보 인증·파싱·CAS 검사가
포함된다. 실제 managed install은 0회다. B18/B19 합계로 같은 prefix의 파일 write와
file fsync가 각각 8,192회, COMMIT이 96회 줄었다.

`lr23`은 Posting 페이지 980개를 모두 처리했다. 그중 실제 물리 설치가 있는
7개 페이지의 MapApply는 62.886초다. 이미 동일 편집 root 265개를 재사용하고
143개를 만들었다. 96개 물리 배치의 입력 23,683개에서 실제 write·fsync는 각각
17,147회였고 설치 wall은 27.854초였다. 별도 COMMIT 122회는 19.969초였다.
둘은 이 MapApply 구간의 약 76%다. 현재 singleton과 Tree Posting 메타데이터는
이미 인라인이므로 이 비용을 wrapper 설치로 분류하지 않는다.

최상위 terms/owners MapApply는 6.077초 뒤 중단됐다. lane1 cursor·검증·scratch
쓰기 0.388초에는 lane2 edit 쓰기 0.255초가 포함된다. lane3 replay·radix build
0.697초, lane4 scratch COMMIT 0.103초는 성공했다. 실패한 lane5 물리 설치는
4.713초다. 이 prefix에서 6개 물리 배치가 시작됐고 실제 write·fsync 각각
1,403회, Catalog COMMIT 7회가 끝났다. 최초 거절은 설치 중 namespace checkpoint다.
따라서 마지막 중단 원인은 scratch가 아닌 파일 설치 경계의 deadline이다.
완료 전 중단된 구간으로 전체 설치 시간이나 여유분을 추산하지 않는다.

같은 pair의 `mb19` Medium 256 sources는 full ACK 4.975초, 최초 검색 119.5ms,
증분·삭제·no-op, pin 검색, 재오픈·검색·삭제 유지·drain을 완료했다. producer
exit0, 제품 artifact, canonical profile 및 first/reopen trace를 검증했다. 비교
환경과 managed-fresh build binding이 없는 로컬 캡처의 정식 성능 수용은 `NOT_RUN`이다.

## B20 — Dense Posting page 묶음과 최상위 Map 설치

B19 Record의 980개 Posting 페이지 중 첫 네 페이지가 lane37 61.372/63.164초를
썼다. 이 구간에 물리 설치 90/96회와 Catalog COMMIT 113/122회가 집중됐다.
B20의 `def73607…`은 dense Posting의 정렬 owner와 occurrence witness를
256행 이하의 childless 페이지에 넣고 실제 page child를 디렉터리가 소유한다.
원본 입력의 CAS·전체 stream digest·EOF를 확인한 뒤 leaf와 디렉터리를 설치한다.

| 동일 seed42·4,096 sources Record | B19 `lr23` | B20 `lr24` |
| --- | ---: | ---: |
| lane37 전체 Posting page wall | 63.164초 | 9.784초 |
| lane37 설치 배치 | 96 | 27 |
| lane37 실제 write / file sync | 17,147 / 17,147 | 1,969 / 1,969 |
| lane37 Catalog COMMIT | 122 | 54 |
| lane37 파일 설치 / COMMIT wall | 27.854 / 19.969초 | 4.744 / 2.251초 |

B20 Record의 최상위 terms/owners Map은 완주했다. MapApply는 wall 35.683초와
thread CPU 23.079초, 그중 lane5 설치는 wall 34.691초와 CPU 22.235초였다.
Managed 물리 설치 span의 wall은 20.010초, Catalog COMMIT 합계는 10.855초다.
실제 파일 write·fsync는 각각 12,668회였다. COMMIT 63회는 설치 50배치를
4배치씩 묶는 현재 계약의 `50 + ceil(50 / 4)`와 일치한다.
설치 하위 phase 중 writer ticket 준비는 4.339초, open은 3.465초,
게시·퇴역은 4.409초, settlement는 2.790초였다. 실제 write는 0.746초,
file sync는 0.821초, barrier는 1.167초였다. 부모와 자식 span은 합산하지 않는다.
cursor·scratch·radix build와 scratch COMMIT은 중첩을 제외하면 0.964초다.
현재 Storage radix는 짧은 terminal을 이미 부모에 인라인한다. 이 입력의 key는
인라인 한도인 128바이트보다 짧으므로 terminal 길이 한도 상향의 근거는 없다.

`lo25` Off는 full ACK 대기 124.935초에 `BuildPrepare` 설치 중 deadline으로
실패했다. `lr24` Record의 제품 artifact는 최초 ACK 104.337초·첫 검색 180.7ms,
증분·삭제·no-op·이전 pin·재오픈·drain을 완료했다. Record trace의 first footer는
876,074 events, reopen은 2,563 events로 모두 drop/open 0이며 canonical profile도
통과했다. Off/Record의 host load1은 각각 27.22–43.05/32.82–50.75, swap in+out은
27.31/16.57GiB였다. 두 실행은 조건이 달라 tracing overhead나 host 기여율은
확정할 수 없다. 제품 artifact는 검증됐으며 정식 matched-host 비교는 `NOT_RUN`이다.

후속 리뷰는 불가능한 singleton fence·중복 child를 거절하고, warm 편집을
dirty leaf 범위로 제한했다. B20 첫 고정본의 warm 전체 순회와 보완 소스의
국소 편집 성능은 별개다.

보완 소스 `13214505…`의 `lo26` Off는 원 4,096 sources·seed42·server 120초에서
최초 ACK 87.815초, 최초 검색 187.5ms, 재오픈 검색 112.5ms였다. 증분·삭제·no-op,
이전 pin 및 drain을 완료했고 producer는 131.170초에 exit0, 제품 artifact가 검증됐다.
ACK는 서버 기준의 해당 요청이며 producer wall은 그 앞뒤를 포함한다. 관측 load1은
22.36–36.18, 시스템 swap in+out은 26.36GiB였다. `lo25`와 소스 및 host 상태가
달라 두 실행의 차이를 warm 보완이나 host에 각각 귀속할 수 없다.

같은 r30 pair의 `lr25` Record는 full ACK126.355초에 거절됐다. 첫 denial은
`BuildPrepare`의 Catalog `managed_install.rs:844` protection 확인 전 deadline이다.
읽기·쓰기·바이트·mutation과 메모리 거절은 없었다. raw first footer는 860,849
events·drop/open0으로 닫혔다. 제품 증거는 실패이며 검색·재오픈은 실행되지 않았다.

| 최상위 Map, 첫 full 요청 | B20 `lr24` Record 완료 | B20 보완 `lr25` Record prefix |
| --- | ---: | ---: |
| MapApply wall / thread CPU | 35.683 / 23.079초 | 32.695 / 16.521초 |
| lane5 reachable install wall / CPU | 34.691 / 22.235초 | 31.136 / 15.536초 |
| 물리 배치 / write / file sync | 50 / 12,668 / 12,668 | 38 / 9,728 / 9,728 |
| 성공한 Catalog COMMIT / wall / CPU | 63 / 10.855 / 4.705초 | 47 / 11.501 / 3.321초 |

`lr25`는 38번째 물리 배치의 인접 advance 중 protection 확인에서 종료됐다.
관측 COMMIT 47개 자체는 성공했다. lexical wall은 86.443→103.094초로 늘고
thread CPU는 56.503→50.645초로 줄었다. `lr25`의 시스템 swap in+out은
40.32GiB였다. 동일 pair Off와 비교해도 host 조건이 달라 tracing CPU와 실행 대기를
분리해 귀속할 수 없다. 9,728개 파일을 설치한 중단 위치와 별개로, 해당 구간의
반복 설치 비용은 남아 있다. 정식 성능 수용은 `NOT_RUN`이다.

`lr25`의 860,849 이벤트 중 SQLite I/O `lane22`가 28,521 span·399,294 이벤트를
생성했다. runtime maintenance의 zero-write 10,406 span은 약 145,684 이벤트다.
실제 write·sync·거절 관측을 유지하면서 빈 scope의 패킷을 없앨 수 있는지 확인한다.
완주한 `lr24`의 전체 이벤트 수가 876,074로 더 많았으므로 이벤트 증가만으로
`lr25`의 deadline을 설명할 수 없다. 캡처 후 raw 검증 최적화는 ACK 이후 비용이다.

## B21 — 최상위 Map 물리 노드 귀속과 진단 검증

`lr24`에서는 terms와 owners가 같은 lane5 설치와 cross-root batch를 공유한다.
같은 물리 설치 비용을 두 root에 중복 귀속할 수 있으므로 root별 분포는 아직
미측정이다. B21의 `MapApply` 6·7은 `NativeTextPostings` 39 아래에서 순서대로
terms·owners의 staged 노드 `NewObjects`를 기록한다. 두 root에서 공유하는 노드는
기존 완료·durable mark에 따라 처음 도달한 root에 한 번 귀속한다. 실패 span의
값은 처리된 prefix이며, 성공 span도 실제 write 횟수·바이트·시간과 별도다.
제품 보고서는 해당 ancestry에서 두 lane이 모두 유효한 경우 요청별 staged 수를
최대 128건까지 집계한다. 이전 trace에는 이 필드가 없다.

고정 fixture는 owner key를 `[5; 16] || BE128(1..=4096)`으로 만든다. 모두
32바이트이며 QIMAP004의 128바이트 terminal 인라인 한도 안에 든다. nibble trie를
재구성하면 분기는 깊이 60·61·62·63에 각각 1·1·16·256개다. 첫 cold Full의
owner 물리 쓰기는 최대 274개이고, 실측 12,668개에서 terms는 **최소 12,394개**다.
이는 소스에서 계산한 하한이다. root별 파일 실측은 별도로 기록한다.

다음 구조 변경 후보는 terms에 적용하는 bounded immutable leaf page다.
기존 `QIMAP004` 읽기, 정확한 child edge,
순서 보존, 변경된 page의 copy-on-write, 이전 pin, 회수 및 장애 복구가 필요하다.
가변 fanout radix와 정렬 durable leaf의 설계 선례는 [ART](https://db.in.tum.de/~leis/papers/ART.pdf),
[HOT](https://15721.courses.cs.cmu.edu/spring2019/papers/08-oltpindexes2/p521-binna.pdf),
[SQLite file format](https://www.sqlite.org/fileformat.html)이다.

seed42 생성기를 재구성한 8,609,531바이트의 corpus digest는 `lr24`와 일치했다.
ASCII tokenizer에서 raw term 31,340개, 두 case surface의 key 62,680개가 나왔다.
각 posting 값을 owner와 초기 occurrence witness의 동일성으로 표현한 정적 radix
모델은 고유 terms 노드 12,394개를 냈다. owners 274개를 더하면 기존 trace의
물리 설치 12,668개와 일치한다. 두 surface의 같은 subtree는 이미 dedupe된다.
부모에 terminal-only child subtree를 한 단계 인라인하고 child 한도 85를 적용한
모델은 terms 5,272개, 합계 5,546개다. 기존 배치당 256개와 창 4개가 먼저 차는
경우 설치/COMMIT은 50/63에서 22/28이 된다. 새 형식의 실측이나 ACK 예측은 아니다.

B22 구현은 원래 node bitmap과 terminal bitmap 뒤에 packed bitmap을 둔다.
각 packed slot의 본문은 하나의 terminal-only QIMAP004 node다. 부모가 값의 실제
edge를 순서대로 소유한다. byte·edge 한도를 넘기는 slot은 독립 객체로 유지하고
부모를 준비하기 전에 stage한다. 기존 물리 identity는 그대로 읽으며, 새 형식을
기록한 namespace는 QIMAP004 전용 reader로 열 수 없다. 제품 롤백 경계를 검증한다.

### B25 부팅과 저장소 형식의 불일치

r32의 QIMAP005와 r31의 QIMAP004 전용 reader는 모두 v18 marker와 Catalog
stamp를 사용한다. 구 runtime의 boot는 물리 객체 closure와 요청·profile을
검증하지만 Text Map 도메인을 decode하지 않는다. 구 runtime이 새 Map을
포함한 v18 namespace에서 초기 검사를 통과하고 조회에서 `CorruptObject`를
반환할 수 있는 경로를 소스에서 확인했다.

B25는 기존 runtime marker와 Catalog stamp를 v19로 맞춘다. 이전 root와 binary는
pair로 보존하고, 새 root는 명시적 producer rebuild로 만든다. 객체 decoder의
QIMAP004 지원과 이전 Native state root의 admission은 별개다. 실제 교차 바이너리
검증에서 r31/v18과 r34/v19는 각자의 root에 소켓을 열고 정상 종료했다. 서로의
root에서는 exit70, 소켓 미생성, 기존 상태 파일 6개 해시 보존을 확인했다. 구 바이너리는
새 root를 foreign entry로, 새 바이너리는 구 root를 explicit rebuild로 거절했다.

r34 `645f4edf…`는 공유 코드와 일치하는 v19 source다. Off `lo30`와 Record `lr29`의
최초 ACK43.227초·44.613초, 첫 검색264.365ms·97.961ms, 재오픈 검색52.279ms·67.804ms를
확인했다. 두 producer는 exit0, 증분·삭제·no-op·이전 pin·재오픈·drain을 완료했다.
Record의 first/reopen footer는 674,977/717 events, drop/open 0이며 canonical 검증을
통과했다. 첫 Full의 terms 5,272개·owners 274개, write/file sync 각각 5,546회,
물리 배치 22회·COMMIT 28회가 r32와 같았다. Top Map wall/CPU는 9.074/6.459초였다.
Off load1 30.32–40.32·swap 이동8.294GiB와 Record load1 22.24–28.10·swap 이동
0.164GiB는 서로 다른 호스트 부하다. 이 두 실행으로 tracing overhead를 판정하지 않는다.

### r34 XL Off의 첫 거절

`lo31xl`은 동일 r34 소스와 바이너리로 64×512 source·seed42·113,019,656바이트를
실행했다. Large 명시 work cap, server dispatch120초, outer watchdog900초를 실행
전에 고정했다. 소스 staging은 184.633초에 완료했다. 원 operation seq2의
`SourceAdvance`는 `BuildPrepare`에서 deadline을 처음 거절했고, harness의
`full_event_ack`는 mutation staging을 포함해 154.718초 뒤 실패했다. producer
exit1·358.449초, outer timeout은 없었다. ACK·검색·재오픈은 실행되지 않았다.

거절 시 usage는 reads28,854,305, writes19,430,441, bytes26,313,528,668,
mutations6,144,405다. 모두 각 cap 아래였다. `object.rs:821`은 설치 직전 child
검증 checkpoint이며, 해당 줄만으로 누적 병목을 귀속할 수 없다. Off 실행 중
호스트 load1은 17.61–28.47이었다. 동일 Record에서 lexical prepare·posting·
물리 설치·Catalog COMMIT의 wall/CPU와 작업량을 분리한다.

XL은 별도 기능 gate다. Large의 제품 120초를 유지한다. 현재 구현에서 단일
`SourceAdvance`가 source apply, `CoreRequired` Text 구축, seal, Catalog commit을
하나의 요청 deadline에서 수행한다. Work cap 증가나 outer watchdog 연장으로
이 첫 거절은 해결되지 않는다.

### r34 XL Record 단계 귀속

동일 XL Record `lr30xl`도 `BuildPrepare`의 deadline으로 거절됐다. 소스 staging
194.181초와 `full_event_ack`159.818초를 기록했다. daemon child trace는
2,226,147 events로 정상 닫혔고 drop·unwritten·open0이다. 검색·재오픈 product
artifact는 없고 capture의 정식 상태는 `FAILED`다.

원 seq2 server 요청 120.099초의 직접 단계는 Catalog admission lock 7.365초,
SourceApply 46.947초와 부분 LexicalBuild 65.605초다. SourceApply는 OwnerPrepare
25.943초와 SourceMapApply 20.504초를 포함한다. LexicalBuild에서는 inventory
prepare 37.713초와 inventory diff 15.777초가 관측됐다. 관리 객체 설치 합계
16.211초는 inventory의 하위 비용이다. Posting, 최상위 Posting Map, seal,
최종 Catalog COMMIT에는 도달하지 않았다. 사용량 reads29,311,082,
writes19,655,022, bytes26.69GB, mutations6,204,511는 cap 아래였다.

같은 r34 Large Record의 SourceApply는 5.150초였고, LexicalBuild 35.960초는
Posting까지 완료한 수치다. XL에서는 source 준비·Map 물리 쓰기와 Text inventory
단계의 비용을 각각 줄여야 한다. 부분 LexicalBuild만으로 65.605초이므로 단일
작은 설치 변경으로 XL 120초 완료를 추정할 수 없다. 긴 구축을 여러 source
event로 나누면 중간 view가 공개된다. 단일 ACK의 의미를 유지하는 장기 설계는
내구 있는 준비 단위와 원자적 공개 경계를 분리해야 한다. 이 방향은 독립적인
불변 segment를 구성한 뒤 commit에서 공개하는 구현과 부합한다.
[Lucene IndexWriter](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/IndexWriter.html),
[Tantivy architecture](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md).

같은 XL Record의 Source OwnerPrepare 25.943초에서 `RecordGet` 32,768회는
11.913초, `BodyGetValidate` 32,768회는 11.904초였다. Base owner lookup은
32,768회·0.062초였다. SourceApply와 Lexical은 4MiB attempt read cache를
공유한다. 입력은 약 113MB이며, 이 기록에 cache hit/miss는 없다. B27 path Map
packing은 별도 단계다. Source record와 body의 재읽기를 줄이려면 같은 attempt의
인증된 ref를 재사용하는 범위와 UTF-8·길이·child 확인을 유지해야 한다.

### B27 Source 경로 Map 실측

XL Record SourceMapApply는 20.504초였다. 해당 하위 범위에서 파일 write·fsync가
각각 8,792회, 설치 36배치, 중첩 COMMIT 46회였다. 독립 radix 모델의 path 6,605와
owners 2,185, 고정 객체 2개가 파일 수와 일치한다. B27은 v19의 기존 QIMAP005를
path root에만 선택한다. path 값은 인라인이고 물리 child가 없다.

같은 Large42의 r34→r36 Record에서 SourceMapApply 하위 실제 write·fsync는
각각 1,095→727회, 설치 배치는 6→4회, 중첩 COMMIT은 9→6회였다. 모델의 path
819→451개와 일치한다. 관측 Map wall은 1.591→1.058초였다. 호스트가 다른 두
실행의 wall 차이를 코드 절감량으로 환산하지 않는다. r36 최초 ACK는 42.650초였고
검색·증분·삭제·이전 pin·재오픈을 완료했다. XL의 2,944개 감소는 모델이며
동일 XL의 새 결과는 `NOT_RUN`이다.

### XL staging의 반복 Catalog 트랜잭션

r34 XL은 32,768개를 32개씩 1,024회 동기 적재했다. Record staging은
194.181초, Off는 184.633초였다. Record 요청 합계 189.413초, p50 175.582ms,
p95 316.853ms다. 각 요청은 동일한 ingress에서 물리 설치 1회와 Catalog
reserve·confirm 각각 1회를 수행했다. Catalog lock 범위는 2,048회·126.088초,
lane 3 COMMIT은 2,048회·106.556초, SQLite VFS `xWrite`는 853,044회·요청
7.392GB였다. 하위 구간이 겹치므로 이 시간들은 합산하지 않는다.

`StageSources`는 이미 본문과 레코드를 같은 durable batch에 쓰며 동일한 본문을
합친다. B29는 256개 물리 객체 상한에서 128개 입력을 허용해 반복 트랜잭션을
줄인다. 객체마다 필요한 내구 설치와 8MiB 한도는 유지한다. 이 구간의 개선은
전체 소요 시간에 반영되며, ACK 안의 `BuildPrepare` deadline은 따로 남는다.

### r39 XL Record의 구조적 감소와 첫 거절

`lr31xl`은 동일 seed42·32,768개·113,019,656B 입력에서 StageSources
1,024→256회, daemon 전체 요청 1,289→521회를 기록했다. Staging은
194.181→120.360초였다. r34와 r39의 load1 범위가 15.86–28.54와 8.13–11.70으로
달라 wall 차이의 코드 기여율은 추정하지 않는다.

원 seq2는 `BuildPrepare`의 occurrence batch 설치 결과 ref 확인 중 원 120초
deadline으로 거절됐다. SourceApply는 53.226초·CPU27.031초, 부분 BuildPrepare는
62.905초·CPU44.587초였다. SourceMapApply는 r34→r39에서 20.504→14.467초,
Catalog lock hold 46→31회, SQLite xWrite 77,452→54,301회였다. 같은 비교에서
OwnerPrepare는 25.943→38.281초, CPU16.009→17.408초였다. wall 증가량을 단일
호스트 요인으로 확정할 수 없다. 부분 inventory prepare 35.472초와 diff 15.671초는
BuildPrepare 내부 범위다. Posting 완료·최종 publish·검색·재오픈은 미도달이다.
Trace footer는 2,072,143 events, drop·unwritten·open0이었다.

`SourceAdvance` 서버 span은 120.134초였다. harness의 Staging 120.360초,
event-ACK 149.740초는 서로 다른 측정 범위다. `OwnerPrepare`에서 32,768개
`RecordGet`은 합계 17.619초·CPU7.725초, `BodyGetValidate`는 합계
17.898초·CPU7.924초였다. 두 하위 span이 OwnerPrepare wall의 92.8%를
차지한다. 이 수치는 B31이 줄이는 Text의 후속 record 재읽기와 구분한다.
SourceAdvance는 별도 StageSources 요청이 설치한 ref 외에도 승인된 기존 ref를
받는다. 현재 stage 간 내구 인증서는 없다. 최초 읽기는 실제 object ref, 경로와
inode, payload digest, record·body 구조와 UTF-8을 검증한다. 이를 보존하면서
경로 검증을 배치화하려면 namespace 교체와 파일 race를 포함한 custody 설계가
필요하다. 현재 trace에는 경로 검사와 데이터 읽기를 분리할 정보가 없다.

부분 `InventoryPrepare` 8,899개 span에서 lane34 tokenizer·scratch 입력은
4.913초·CPU4.762초, lane35 정렬 그룹 조회·packed 준비는 18.269초·CPU10.513초였다.
두 구간은 각각 8,865개 span이다. lane35는 객체 준비와 설치를 포함하므로
18.269초 전체를 SQLite 비용으로 귀속할 수 없다. `InventoryDiff`의 15.671초에는
`PostingEdit` 9.102초가 포함된다. 하위 합계는 상위 시간에 중복 합산하지 않는다.

### r36 Mixed 워밍업 실패

`lm01`은 `ae2aaf64…`에서 Large42 초기 ACK 41.130초, 첫 검색 151.841ms,
증분 1.039초, 삭제 3.423초를 완료했다. Mixed warmup의 dispatched/completed
434개 중 413개가 실패했다. 1Hz 발행은 seq6·7 모두 ACK 8.218초·6.400초를
받았고 첫 발행의 최신 검색 확인은 8.656초였다. 다음 발행은 ACK 이후 head 조회
또는 검색 확인에서 `ResourceLimited`를 반환했고 snapshot 정리도 같은 오류를
반환했다. Missed tick은 7개다. 600초 retained read와 900초 measure는 미실행이다.

모든 offered query가 현재 snapshot의 단일 `ReadSession`을 공유한다. 서버가
`with_ticket`에서 entry mutex를 검색 전체에 걸쳐 보유하므로 한 세션 내 검색은
직렬이다. `ReadView`와 retained search 실행은 동일 Interactive ledger를 이용하며
Control과 Interactive의 즉시 승인 거절도 `ResourceLimited`가 된다. 호스트
load1은 8.26–13.49였다. 기존 artifact는 첫 검색 오류와 서버 거절 위치를 잃어
세션 대기, Catalog 대기, scheduler 거절과 호스트 영향을 분해할 수 없다.
후속 계측은 원 요청의 단계와 예산 거절 축을 bounded sample에 남긴다.

`ReadTicket::validate`가 사용하는 `SqliteCatalog::validate_ticket`은 blocking
writer mutex에서 `BEGIN IMMEDIATE`를 연다. session inode·pin·fence·만료를
검증하며 clock이 전진한 성공 검증마다 durable `native_work_clock_v1`을 갱신하고
COMMIT한다. 기존 Kernel 호출 수 회귀에서 retained 준비·실행 검증은 5번이며,
registry 전후 검증을 포함하면 7번이다. 50 QPS에서 이는 초당 최대 350번의
검증 경로 진입을 뜻한다. 각 호출의 물리 write 횟수나 `lm01` 원인 비율을
계산한 수치는 아니다. 동일 millisecond에는 clock 갱신을 생략한다. B30은 원 요청
예산과 trace를 mutex 대기까지 연결한다. writer 트랜잭션과 clock floor를 줄이는
변경은 pin·renew·clock 회귀와 함께 별도 검증이 필요하다.

`HeldRead::tick`은 1초 발행 tick의 예산으로 갱신을 요청한다. 종전 서버 갱신은
같은 session entry lock 안에서 예산 없는 writer lock을 기다렸다. B31은 writer
대기와 COMMIT 전 checkpoint에 원 예산을 전달한다. 해당 경로가 `lm01`의 원인이었는지
확인할 갱신 거절 증거는 없다. 같은 tick에서 held read나 정리가 먼저 실패하면
종전 artifact의 `scheduled` 집계가 해당 tick을 누락했다. 후속은 도달 시점에 집계한다.

### r39 Mixed의 Interactive 승인 거절

`lm02`는 72.281초에 producer exit1로 종료했다. Mixed 이전 Large 최초 ACK는
45.099초였다. 첫 warmup tick의 seq6은 ACK 6.419초 뒤 `current_witness`에서
`ResourceLimited`로 거절됐다. 168건의 검색이 끝났고 156건이 실패했다. 최초
검색 실패는 retained unique 검색 task1이었다. 서버의 첫 5개 retained 검색
표본은 `interactive_admission`에서 로컬 예산 거절 기록 없이 `ResourceLimited`를
반환했다. 나머지 표본 3개는 취소였다. 현재 결과로 CPU·I/O·메모리 중 실제 거절
축을 구분할 수 없다.

고정 profile은 CPU·I/O 용량이 각 5단위이며 maintenance를 포함한 보호 class
floor가 5단위를 사용한다. 같은 단위의 Interactive 예약을 동시에 추가 수용할
여유가 없다. host load1 8.59–10.77은 관측 조건이며, 서비스 ledger의 거절을
호스트 영향으로 환산하지 않는다. B34의 예약 대기로 이 충돌을 처리한 뒤에도
1Hz 발행·50 QPS와 같은 시각대의 latency를 각각 다시 검증해야 한다.

r31 Large Record 단일 변경은 StageSources 0.129초와 event ACK 1.003초가
순차적으로 1.132초였다. 단일 삭제는 별도 staging 없이 ACK 1.930초였고,
서버 `BuildPrepare` 1.538초 중 lexical 1.436초와 그 하위 Posting 1.359초가
기록됐다. harness는 한 tick의 발행을 마친 뒤 다음 tick에 진입한다. 두 단일
샘플은 당시 실행에서 1초 주기를 넘겼으며, Mixed의 sustained 처리량이나
코드와 호스트의 기여율을 수치로 확정하기에는 표본이 부족하다.
현재 warm packed posting은 `Posting::checked_df`와 `resolve_packed_warm`에서
동일 prior directory를 각각 읽고 decode한다. 변경되지 않은 페이지는 이미 COW로
유지된다. 제거 가능한 직접 반복은 변경 posting당 directory 1회이며, 1Hz에서의
절감 시간은 미측정이다.

### r40 Mixed의 발행 지연과 일반 검색 거절 분리

`lm03`은 Large 초기 ACK 36.929초와 증분·삭제 검색을 마친 뒤 Mixed
warmup에서 producer exit1로 종료했다. 첫 tick의 seq6 ACK는 4.321초였다.
그 뒤 최신 head 확인은 통과했고, 일반 Exact `SearchText` witness가
`ResourceLimited`를 반환했다. 첫 offered 실패는 retained unique task4의
UDS read timeout 1,998ms로 ACK보다 앞섰다. 검색 완료 91개 중 76개 실패,
도달 220개 중 queue full 66개였다. 서버의 retained 실패 표본 8개는 모두
`Cancelled`였고 session custody·준비·실행에 분포했다.

기존 서버 표본은 일반 Text를 수집하지 않아 witness 거절의 단계는 확정할 수
없다. B36은 일반 Text용 독립 제한 표본을 추가하고 ReadView의 pin validation을
원 요청 예산에 연결한다. r40 Off 실행 중 load1 6.92–8.90을 관측했다.
ACK 지연과 retained 검색의 Catalog/session/transport 대기 비율은 미확정이다.

### r41 Mixed의 다음 snapshot Control 거절

`lm04`는 첫 tick을 3.162초에 ACK하고 3.426초에 최신 witness를 확인했다.
이전 view도 통과했다. 다음 `Open`에서 서버 표본이 `retained_control`의
`control_admission`과 `first_local_denial=None`을 기록했다. 이 실행의
중단점은 기존 2초 검색 deadline이나 lexical work cap에서 발생하지 않았다.
검색 완료 81개 중 62개 실패, 도달 177개 중 queue full 33개다. 첫 UDS read
timeout 1,997ms는 첫 ACK 전이었다. load1은 4.85–6.14였다. Control 거절을
대기로 바꿔도 이미 관측된 3.162초 ACK와 3.426초 freshness는 별도로 남는다.

B38의 공유 query guard는 같은 세션의 전체 검색 직렬화만 줄인다.
Catalog의 `validate_live_ticket`은 여전히 OS guard/inode·pin fence·만료를
확인하고 writer mutex에서 `BEGIN IMMEDIATE`와 COMMIT을 수행한다. 동일
millisecond에는 clock UPDATE를 생략한다. durable floor를 유지하므로, 남은
병목을 확인할 때 mutex 대기, clock UPDATE 여부, COMMIT 시간을 분리한다.

### r43 XL Off에서 재현된 BuildPrepare deadline

`xo43`은 32,768개 source staging을 93.397초에 마쳤다. 최초 ACK phase는
140.304초에서 실패했다. daemon은 원 120초 request deadline, `BuildPrepare`,
`object.rs:862`의 install loop checkpoint를 기록했다. 첫 denial은 work 수치
상한과 별개이며, 이 checkpoint는 deadline을 감지한 위치를 나타낸다. trace Off에서도
발생했다. 검색·재오픈은 미실행이다. load1은 4.94–7.40 범위였다.

### r42 Mixed의 실제 진행과 만료 tick

`lm05`는 warmup 60 tick에서 14건, measure 57 tick에서 12건을 발행하고
각각 최신·이전 witness를 확인했다. 두 window에서 90 tick이 늦었다. 검색
완료 4,003건 중 3,949건이 실패했고, 도달 6,001건 중 queue full은 1,935건이다.
첫 검색 실패는 1,534ms 큐 대기 뒤 465ms UDS timeout이었다. 첫 ACK는 5.802초,
부분 measure의 freshness p95는 7.004초였다. load1은 5.27–7.17이었다.

최종 `held_read_tick`은 전체 index 116이었다. tick의 1초 budget 만료는
held origin 기준 117.082초인데, 첫 갱신은 같은 origin의 120초 이후에만
진입한다. publisher가 late 확인 전에 `HeldRead::tick`을 실행해 이미 만료된
budget으로 갱신을 시도했다. cleanup의 Release는 120.245초에 시작했고 원 pin
해제가 확인됐다. B39는 만료 tick을 먼저 기록하고 건너뛴다. 이 수리는 관측된
갱신 중단 경계를 고치며, 1Hz·50 QPS 수용 조건은 그대로다.

### r43 Mixed의 갱신 진행과 남은 제품 지연

`lm06`은 warmup 16건과 measure 100건을 발행했다. 총 487 tick 중 370개가
늦었다. 도달한 검색 24,350건 중 7,874건이 queue full이었고, 완료된 16,412건
중 16,287건이 실패했다. 새 mutation 준비 시각으로 measure의 제출부터 ACK까지를
분리했다. 이 구간의 중앙값은 2.742초, freshness p95/p99는 6.634/7.663초였다.
load1은 4.20–18.65였다. 제출 구간 내부의 Catalog, build, 물리 설치 비율은
이 실행에서 측정하지 않았다.

held-read는 약 120.178·240.791·362.791·486.898초에 네 번 갱신 ACK를 받았다.
네 번째 갱신 뒤 witness 검색이 tick의 남은 166ms에서 실패했다. cleanup은 같은
pin을 Release했고 마지막 pin 해제를 확인했다. 600초 보유와 settlement는
미도달이다. 최초 120초 갱신 전에 만료 tick을 호출하던 r42 중단은 재현되지 않았다.

### r44 Mixed의 held 해제와 settlement 거절

`lm07`은 원 50 QPS·1Hz 부하 아래 warmup 17건과 measure 167건을
발행했다. 661 tick 중 477개가 늦었고, 예정 query 33,051건 중
queue full은 10,699건, 완료 22,288건 중 실패는 22,023건이다. 첫 실패는
unique 검색으로 2초 중 1.707초를 scheduler 대기열에서 소모했다.
measure freshness p95/p99는 5.453/5.807초였다. 이 값은 중단된 창의 표본이다.

독립 held worker는 원 1초 tick에서 네 번 갱신했고 갱신 뒤 witness를
통과했다. load start 후 600초를 보유했으며 pin 시작 기준 600.434초에
Release ACK와 마지막 pin 제거를 확인했다. 다음 60초 내 61회 조회 뒤
마지막 응답은 `Pending`이었다. cleanup과 snapshot cleanup은 오류가 없다.

Catalog의 `Pending`은 target pin/floor, terminal work·custody, 미퇴역 view,
GC queue와 object lifecycle의 보수적 검사에서 반환된다. 증거는 어떤
조건이 실제 차단했는지 보존하지 않았다. query snapshot과 held 제품
session은 같은 Kernel의 Catalog guard를 사용한다. 단순히 query 세션이
별도 guard라서 차단됐다고 판정할 수 없다. epoch/global debt 검사가
지속 발행과 충돌할 가능성은 있으나 60초·정리 정책 변경의 근거는 미확정이다.

### r45 Mixed의 Terminal Custody 차단

`lm08`은 Large 4,096-source·seed42에서 704.012초에 중단됐다.
warmup 20건과 measure 160건이 ACK를 받았고 661개 tick 중 479개가
늦었다. 33,051 예정 검색 중 10,695개가 queue full로 탈락했다. 완료
22,292건 중 21,958건 실패, 최초 실패는 원 2초 중 scheduler 1.572초와
UDS read 427ms였다. 짧은 초기 샘플에서 session custody, query preparation,
control admission, query execution의 취소가 보였다. 한 구간에 비용을
귀속할 수 없다.

held는 네 번 갱신한 뒤 600.539초에 Release ACK와 마지막 target pin 제거를
확인했다. 61회 poll 후 60초 수용에 실패했다. 한 Catalog snapshot에서
`Pending(TerminalCustody)`, target 5, head=floor 169, target pin 없음이
관측됐다. 마지막 wire 응답은 `Pending`, head=floor 184, target pin 없음이었다.
한 차단 표본을 전체 61회에 일반화하지 않는다.

현재 차단 predicate는 같은 workspace/epoch의 terminal custody 전체를 본다.
어느 operation이 남았는지는 아직 관측되지 않았다. Large의 첫 full
SourceAdvance는 같은 ingress에 4,096개의 고유 본문과 4,096개의
SourceRecord를 설치한다. op2에 이전되는 이 8,192 custody rows는
mutation·map·adapter 객체가 제외된 하한이다. Work→Views→Custody 순환,
20ms 간격, 한 custody page당 한 행으로는 공용 lock·SQL을 제외해도
최대 16.7행/초, op2 ingress 하한에 약 492초가 걸린다. 이어진 발행과
더 큰 물리 객체 집합은 별도 debt다. B51은 조회 writer 예약만 줄이고 이
처리 상한을 변경하지 않는다.

B54는 Work/View 페이지와 custody root 페이지를 분리해 기존 한 transaction의
bounded release를 재사용한다. 글로벌 debt0 predicate와 60초는 유지한다.
별도 측정에서 가장 오래된 ready sequence, release rows/round, floor/phase
거절을 확인해 초기 적체·새 발행·maintenance admission을 구분한다.

### r47 Mixed의 같은 oldest custody 잔류

`lm09`는 B54–56의 32행 custody 페이지와 같은 1Hz·50QPS를 사용했다.
소스 `647dce43…`, cooperative lease, shared macOS에서 707.722초에 중단했다.
warmup 18·measure 176 ACK, 늦은 tick 464/661, 검색 실패 21,881/22,297,
queue full 10,690건이다. 첫 실패는 원 2초에서 scheduler 1.498초,
UDS read 약 502ms였다. measure freshness p95 5.536초·p99 7.521초다.

600.509초에 마지막 pin을 제거했지만 61회 뒤 `Pending`이었다.
custody 표본 1·16·32·48·60번째에서 oldest ready 20874가 같았다.
head와 exact floor는 184→187→190→195→198로 전진했다. 같은 ready row가
관측 기간에 계속 남아 있었음이 확인됐다. release된 자식 행 수, queue
크기, scope admission·floor 거절은 측정되지 않았다. 초기 대형 operation,
bounded sweep 재방문 빈도, SQLite 행당 비용을 실제 진행률로 구분한다.
배치 폭만 늘린 결과는 기존 60초 oracle을 충족하지 못했다.

### r48 Mixed의 이전 뷰 검색 중단과 정리 진행

`lm10`은 소스 `930e30da…`, cooperative lease에서 272.413초에
중단됐다. 최초 실패는 publisher의 `old_view_witness`, UDS read
1,999ms였다. 이후 snapshot drain의 해제도 같은 IPC 제한에서 실패했다.
warmup 16·measure 37 ACK, 늦은 tick 157개, 완료 검색 7,391건 중
7,291건 실패와 queue full 3,556건이다. 최초 검색 실패는 scheduler
1.565초와 UDS read 약 434ms였다. held는 한 번 갱신했고 cleanup이
마지막 pin을 해제했다. 원 600초 보유 및 settlement poll은 미도달이다.

opt-in progress는 r47의 20874를 초기 full op2로 확인했다. 45초 간격
다섯 표본에서 해당 operation의 커밋된 삭제는
1,152→4,288→6,624→8,416→9,984행이었다. 마지막 표본의 전체 custody
삭제는 30,787행·987페이지이고 oldest에 행이 더 남았다. floor 4,409회 중
17회가 거절됐고 maintenance admission·custody probe·release 거절은
0회였다. 표본까지 op2가 느리게 정리됐으며 page timeout 롤백은 없었다.
중단 전 확인된 병목은 이전 뷰 검색 지연이다. r47의 60초 정산과 별개로
기록한다. B62는 같은 page에서 반복하던 GC binding 검증과 SQL 준비를 줄인다.

### r49 Mixed의 정산 실패와 검색 처리율

`lm11`은 `d4e0a5dc…`의 고정 pair에서 724.781초, producer exit 1로
끝났다. 4회 갱신 뒤 600.550초에 마지막 pin 해제를 확인했다. 61회 poll
뒤에도 `Pending`이었으며 head와 Exact floor는 모두 154였다. 1·16·32·48·60번째
poll의 첫 blocker는 `TerminalCustody`, oldest ready는 모두 20874였다.
마지막 진행 표본은 전체 2,961 page에서 92,251행, 초기 op2에서 26,368행을
커밋했다. custody probe·release 거절은 없었다. 이 결과는 해제 후 60초 안에
실제 backlog를 소진하지 못한 경우다. 마지막 표본 이후 처리량은 집계하지 않는다.

검색 22,305건 중 22,185건 실패, queue full 10,681건, ACK 150건,
missed-late 511건이었다. 최초 검색 실패 task 16은 queue에서 1,539ms를
보낸 뒤 retained search 응답 읽기에서 460ms로 만료됐다. 서버의 첫 여덟
실패는 모두 request ID 1이었고 session custody 2건, query preparation 3건,
execution 2건, Control admission 1건에서 취소됐다. 후속 요청만 받는 B63
실패 타이밍은 0건이었다. 후속 요청의 실패 표본과 매 요청 새 counter를 쓰는
offered query 표본을 각각 제한해 서로 소진하지 않게 해야 한다.

고정 ledger의 5단위에서 각 보호 class가 1단위를 유지하므로 Interactive는
동시에 1개다. 50 QPS를 검색만으로 처리하려면 점유 시간 평균이 20ms 이하여야
한다. retained 검색당 7회 writer transaction도 같은 Catalog를 공유한다.
호스트는 실행 시작부에 load1 27–39였고 다른 실행과 일치하지 않는다.
각 병목의 wall 기여량 및 로컬 개선률은 아직 산출할 수 없다.

### r51 Mixed의 custody 진행과 정산 미수용

`lm12`는 `1482d857…`에서 704.736초, producer exit1로 종료했다.
4회 갱신 뒤 600.475초에 마지막 pin을 해제했다. 61회 poll의 최종 상태는
`Pending`, head와 Exact floor는 모두 176이었다. 초기 op2의 ready20874는
해제 전에 소진됐다. 마지막 진행 표본은 3,721 page·115,926행을 커밋했고
가장 오래된 작업은 op170/ready24723이었다. custody probe·release 거절은 0이다.

해제 뒤 `UnretiredView` 다음 `ReclaimQueue`가 첫 blocker로 관측됐다.
이후 새 publication의 `TerminalCustody`가 다시 나타났으며 ready 표본은
24611에서 24779로 전진했다. 개별 poll의 blocker 표본과 마지막 `Pending`
상태는 서로 다른 증거다. 해제 창 전체에서 어느 blocker가 가장 오래
유지됐는지와 reclaim 큐의 정확한 잔량은 현재 관측으로 알 수 없다.

r51 전체 704.736초에서 maintenance floor round는 11,918회, custody
처리량은 3,721 page·115,926행이었다. 20ms tick 설정에서 실제 평균은
초당 16.9 round, 5.28 custody page였다. 고정 Work→Views→Custody 순환은
Work/View가 비지 않으면 custody에 약 3회 중 1회만 배정한다. 추가
page가 전체 round 시간을 늘릴 수 있으므로 round당 분배율과 초당
실제 처리량을 별도로 판정한다. 정산은 target pin 외에도 해당 epoch의
custody와 전역 reclaim 부채가 남으면 Pending을 유지한다.

검색은 22,311건 중 22,106건 실패, queue full 10,676건이었다. 172건 ACK,
487개 missed-late였으며 960초 측정은 중단됐다. 최초 검색 실패 task12는
queue에서 1,280ms를 쓴 뒤 retained IPC read에서 720ms로 만료됐다.
호스트 load1 5.41–8.45와 r49의 5.73–38.95는 서로 다른 환경이다. 초기
custody 차단이 해소된 사실과 검색·정산 기준 미충족을 각각 기록한다.

### Mixed retained 검색의 Catalog 경계

retained SearchText는 registry pre/post 2회, Kernel profile pre/post 2회,
stage resolution 완료 1회, 실행 pre/post 2회의 Catalog validation을
거친다. 각 검증은 현재 session lock·inode, session/pin row와 durable
clock을 확인하고 commit한다. 같은 connection mutex와 `BEGIN IMMEDIATE`를
사용하므로 성공률 100%인 50 QPS에서 이 경로는 초당 350회 transaction을
요청한다. 새 시각이 floor를 전진시킬 때 clock row를 기록한다. 검증 경계
사이에 admission wait, profile read와 결과 변환이 있다.

B56은 lock 인증에서 이미 읽은 dev/inode를 같은 호출의 SQL row 확인에
사용해 중복 fstat 한 번과 inode blob 할당 두 번을 제거한다. 일곱 검증의
횟수는 유지한다. 정식 귀속은 Control wait·준비·Interactive wait·검색과
Catalog lock/commit의 시간 및 관측 손실을 별도로 확인해야 한다.

r51 `lm12` 초반 fresh-request 성공 표본 8건은 Interactive 대기
121.645–421.229ms, 중앙값 240.096ms였다. 예약 후 실행은
42.906–327.087ms, 중앙값 98.843ms였다. 이 실행 구간은 Kernel 전후
ticket validation과 reader를 포함한다. 가장 빠른 표본도 단일 슬롯의
50 QPS 필요 평균 점유 20ms보다 길었다. 오류 표본 5건은 모두 취소였고,
그중 session custody에서 끝난 경우가 4건이다. 초기 표본이므로 지속부하
분포나 lexical 단독 시간을 나타내지 않는다. 다음 진단은 `query.reader.query`
와 실행 전후 ticket validation을 직접 분리한다.

r52 `lm13`의 첫 fresh-request 성공 표본 8건에서 슬롯 점유 내 실행
중앙값은 68.978ms다. reader 중앙값 12.940ms, 실행 전후 ticket validation
합산 중앙값 47.911ms다. 6건의 reader는 7.635–18.744ms였고, 2건은
528.009ms와 698.285ms였다. 요청별 `before`, `reader`, `after`를
같은 span에서 기록했다. 이 표본으로 느린 두 요청의 query class나
전체 50 QPS 분포는 확정할 수 없다. 호스트 load1 범위는 16.02–41.45,
중앙값 22.12로 r51의 5.41–8.45와 다르다. 실행은 338.958초에 producer
exit 1로 끝났다. 첫 실패는
`held_read`의 갱신 직후 고정 view 검색에서 IPC Read deadline 726ms였다.
갱신 ACK는 120.317초와 241.375초에 도착했고 마지막 pin은 243.687초에
해제됐다. Publisher 취소는 후속 결과다. 원 600초 pin과 해제 후 정산은
`NOT_RUN`이며, r51의 정산 실패는 여전히 열려 있다.

같은 실패 요청의 서버 wall time은 Control admission 16.847ms,
Interactive admission 0.001ms, query preparation 274.534ms, session custody
357.351ms, query execution 65.167ms였다. 실행 중 reader는 3.343ms,
전후 ticket 검증은 18.070ms와 43.739ms였다. 마지막 stage는 `session_custody`,
결과는 `Cancelled`였다. 이 요청의 직접 지연 구간은 preparation과 custody다.
별도로 B76은 fresh 요청의 단일 Interactive 슬롯을 마지막 Kernel
ticket 검증 전에 반환한다. 변환 메모리와 기존 오류 우선순위는 f130에서
확인했다. 호스트 경합의 기여도와 sustained 50 QPS 효과는 현재 표본으로
산정할 수 없다.

B76이 옮기는 경계를 r52의 같은 8건에 대입해
`query_execution - after_ticket_validation + result_conversion`을
계산하면 중앙값은 35.888ms이고 20ms 이하는 2건이다. 이는 각 작업의
시간이 그대로라는 가정에서 슬롯 점유를 재분류한 값이다. B76 실행
시간이나 50 QPS 수용 결과로 사용하지 않는다.

r52 타이머는 outer custody 진입과 반환, query plan과 Kernel 준비를
각각 합산한다. 최초 실패에 대한 mutex·SQLite·호스트 대기 분해는
`NOT_RUN`이다. Record의 기존 `CatalogLockWait`·`CatalogLockHold`로
mutex 대기와 점유를 분리할 수 있다. 점유 내부의 transaction 시작,
row 검증, clock, COMMIT 분해는 별도다. 단계 통합은 durable clock,
pin 만료와 fence 검사를 유지해야 한다.

### r53 Mixed의 preparation 실패와 실제 custody 진전

`lm14`는 `a6003311…`의 동일 환경·최적화 pair에서 producer exit1,
474.416초로 종료했다. 갱신 ACK는 120.388·241.380·362.412초에 도착했다.
세 번째 갱신 뒤 고정 view 검색이 834ms IPC deadline에 도달했다.
같은 conn12841/request6의 서버 wall은 Control admission 32.333ms,
entry/pre custody 136.232ms, query preparation 629.304ms였다. 최초
원인은 held read이며 publisher는 후속 취소다. 이 요청은 Interactive
admission과 reader에 미도달했다.

마지막 pin 해제는 363.641초에 확인됐고 settlement poll은 0이다. 원
600초 pin·60초 정산은 `NOT_RUN`이다. 검색 12,226건 중 12,209건이
실패했고 queue full은 5,861건이다. 부하 도달은 48,000개 중 18,151개이며
publisher는 warmup 10건과 measure 46건을 발행했다.

fresh-request 성공 표본은 4건이다. reader는 6.300–9.049ms, 실행 전
ticket 검증은 9.426–365.714ms, 실행 후 검증은 57.964–176.109ms,
registry entry/pre는 10.135–31.186ms, exit/post는 41.090–165.379ms였다.
새 계측은 registry 시작과 종료를 분리했지만 검증 내부의 mutex 대기,
SQL·durable clock·COMMIT 비율은 아직 구분하지 않는다.

마지막 retention 표본은 7,049 round·1,881 custody page·59,174행이다.
초기 op2를 벗어나 oldest op56/ready22593까지 전진했고 custody probe와
release 거절은 0이었다. 전체 실행 중 host load1은 93개 표본에서
16.79–106.35, 중앙값 37.53이었다. r51·r52와 직접 처리율이나 개선율을
비교할 수 없다. 기본 Large 단계의 ACK 52.838초와 첫 검색 189.865ms는
성공했다. 증분·삭제·old pin 확인도 완료했으나 전체 Mixed 제품 수용은
실패했다.

### r54 Mixed Record의 실행 전 거절

`r54`의 `3c034df4…` 최적화 pair 빌드는 성공했으나 Mixed Record 요청은
capture preflight에서 `Native mixed load requires Off trace mode`로
거절됐다. Rust harness도 동일 조합을 거절한다. producer body는 0이고
실행·진단 상태는 `NOT_RUN`이다. 제어기의 뒤따른 `FileNotFoundError`는
없는 execution receipt를 읽은 오류이며 제품 실패의 원인이 아니다.

B82 후보는 같은 Mixed Off의 기존 선택된 retained-request 진단에
Catalog의 여섯 구간을 연결한다. 요청 처리의 동기 스레드와 같은 Catalog
객체에 scope를 묶고 시간·호출·실패 횟수를 고정 크기로 모은다. 중첩
관측은 복원하고 잘못된 종료 순서는 관측을 무효화한다. 결과는 요청 안의
여러 ticket 검증을 합산한 값이다. 원 타이머의 각 stage와 합산하지 않는다.
이 계측을 포함한 r55는 RCA 실행으로 분류하며 성능 qualification은
`NOT_RUN`으로 둔다.

별도 Native SDK read-cap 조사에서 현재 Catalog admission은 새 Attempt
객체의 reserve에 27회, unique confirmation에 20회의 논리 read를
계상한다. 12,524개가 각각 이 두 경로를 거쳤다면 최소 588,628회다.
이는 보고된 InventoryMapApply 754,642회의 78.0%에 해당하는 코드
산술이며 실제 SQL 호출 수나 wall 기여량은 아니다. 현재 경로는
transaction 내 child 인증, sequence counter, registration preflight를
이미 공유한다. 개별 custody·intent·GC·metadata 검증이 남으므로 27/20을
즉시 낮출 근거는 확인되지 않았다. 실제 statement별 계수와 work delta를
대조한 뒤 제거할 SQL과 계상을 함께 정해야 한다.

### Mixed 관측기 자체 비용

r43 `lm06`의 537.094초 native observation epoch에서 fixture 전체
allocated-block census 1,048회에 wall 합계 116.576초, macOS child RSS
4,326회에 32.233초가 기록됐다. 직렬 sampler의 probe wall 합계는 epoch의
27.7%다. 서버와 시간이 겹치므로 제품 latency에서 이 합계를 차감하지 않는다.
Mixed Off는 canonical trace가 꺼져도 운영 profile의 periodic resource
관측은 유지한다. r44 `lm07`은 703.049초 epoch에서 fixture census
231.960초, child RSS 35.765초를 기록했다. probe wall 합계는 38.1%다.
현재 결과는 query·publisher가 관측과 어떤 자원을 얼마나 경합했는지
분리하지 않는다.

macOS는 100ms child RSS마다 `ps`를 실행하며, 500ms disk census는 각
경로의 `st_blocks`를 직접 읽고 scope별 inode 중복을 제거한다. 캐시된
논리 bytes를 physical allocation으로 사용하거나 hardlink/샘플 기준을
바꾸지 않는다. Darwin의 직접 RSS와 bulk directory attribute 조회는
각각 API·오류·실측 parity를 별도로 검증할 후보다.
[Apple XNU process attributes](https://github.com/apple/darwin-xnu/blob/main/bsd/sys/proc_info.h) ·
[Apple XNU getattrlistbulk](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2).

### r44 XL Off의 재실패

`xo44`는 동일 32,768 sources·seed42의 staging을 93.388초에 끝냈다.
원 120초 server deadline으로 BuildPrepare가 거절돼 harness ACK phase는
138.490초에서 실패했다. 첫 denial은 work count 또는 메모리가 아닌 deadline이다.
checkpoint는 packed occurrence의 `VectorIndex::locate`
(`text_occurrence/block.rs:55`)였다. r43의 물리 설치 checkpoint와 위치가
다르다. 별도 단계 계측이 없는 Off 실행으로 두 위치의 인과 비율을 추정하지 않는다.

실행 중 load1은 4.22–6.65였다. r43의 4.94–7.40보다 낮아도 원 실패가
재현됐다. load1은 CPU 경합·장치 지연의 정확한 분해가 아니다. r44의
logical reads는 57,799,380회, bytes는 54,158,120,563B였다. r43과 비교해
거절 시점까지의 작업량은 늘었으나 단계별 단위 처리량이나 성공을 증명하지 않는다.
첫 검색·재오픈은 미도달이다.

### r45 XL Off의 BuildPrepare 거절

`xo45`는 B48–50을 포함한 `5d830c1e…`에서 32,768 sources·seed42의
staging을 100.750초에 완료했다. 원 server 120초의 첫 차단은
BuildPrepare deadline이다. 실패까지의 ACK phase는 137.642초다.
checkpoint는 `object/read_io.rs:165`의 객체 bytes budget charge다.
원 객체 읽기는 namespace·shard·파일 identity·길이·digest 검증을 한다.
이 위치는 시간 초과를 관찰한 지점이다. r44의 occurrence vector checkpoint와
함께 단일 함수의 elapsed time이나 디바이스 병목을 증명하지 않는다.

거절 시 logical work는 reads 42,238,827회, writes 31,882,956회,
bytes 43,773,309,145B, mutations 12,173,147회였다. 실행 load1은
5.96–10.15로 r44의 4.22–6.65보다 높았다. 서로 다른 실패 prefix의
logical work 감소로 B50의 처리량이나 절감 시간을 계산할 수 없다.
ACK·첫 검색·재오픈은 미도달이다.

다음 XL 진단은 같은 fixture와 deadline에서 BuildPrepare의
inventory prepare·posting scratch·installed block seal·physical object
read를 분리한다. 기존 trace에서 확인한 59,861회 posting flush는
InventoryDiff에 포함된 비용이다. 서로 겹친 span을 합산하지 않으며,
full Record 진단과 Off 제품 수용은 각각 판정한다.

### r46 XL Record의 완료 prefix

`xr46`는 같은 binary·source `8684fe0c…`의 로컬 진단이다.
staging 106.676초, ACK phase 143.769초에서 실패했다. 원 server 120초의
BuildPrepare deadline은 Catalog object confirm의 lifecycle checkpoint
`managed_install.rs:844`에서 관측됐다. 최초 ACK·검색·재오픈은 미도달이다.

daemon Record는 256MiB에서 partial이다. 2,612,635개 기록, 857,584개
미기록, 열려 있는 span 8개가 footer에 남았다. 아래 집계는 full 요청
BuildPrepare 시작 165.891초에서 마지막 기록 222.633초 사이의 완료 span이다.

| BuildPrepare 완료 prefix | 건수 | 누적 wall |
| --- | ---: | ---: |
| Inventory prepare 전체 | 13,107 | 33.973초 |
| 그 안의 token spool / ordered group·pack | 각 13,056 | 6.641초 / 15.768초 |
| Inventory Diff | 12,844 | 15.563초 |
| Diff 안의 Posting scratch batch | 88,337 | 10.294초 |
| Inventory MapApply | 51 | 3.213초 |
| Managed object install | 102 | 10.433초 |
| Managed install 안의 Catalog COMMIT | 204 | 7.783초 |

COMMIT은 설치의 하위 구간이고, 설치 중 일부는 prepare와 겹친다.
전체 daemon의 별도 object stderr counter는 reserve/confirm 690회,
물리 write·file sync 각각 93,016회다. startup/staging과 BuildPrepare가
함께 들어 있다. scope 전체 reserve 35.005초, confirm 46.774초를 위
BuildPrepare 합계에 더하지 않는다. 완료 prefix에서 물리 write 0.275초,
file sync 0.380초, directory sync 0.153초, barrier 1.818초가 관측됐다.
계측된 설치 비용을 분리하면 Catalog COMMIT이 파일 write나 sync보다 크다.

cold token의 owner-local 인메모리 그룹, packed Diff의 ordinal 재사용,
Posting 전용 scratch batch를 각각 검증한다. 범용 SQL 경로의 hard file
cap과 bind limit를 유지한다. SQLite의 변수 한도는 연결 설정으로도 낮출
수 있으므로 batch 확대는 현재 connection에서 확인한다.
[SQLite limits](https://sqlite.org/limits.html).

### r48 XL Off의 설치 결과 checkpoint

`xo48`은 B57–61을 포함한 `930e30da…`, cooperative lease에서
32,768 sources·seed42 staging 105.752초, ACK phase 141.836초였다.
원 server 120초의 첫 차단은 BuildPrepare deadline이다. checkpoint는
`OccurrenceWrites::flush`의 `put_batch` 반환 이후 설치 결과 대조였다.
최초 ACK·검색·재오픈은 미도달이다.

거절까지 reads 3,354,864, writes 9,916,186, bytes 8,007,602,345,
mutations 13,064,994였다. r46의 35,834,681 reads와 12,806,904 mutations는
별도 실패 prefix다. 논리 scratch charge 감소는 확인했으며 두 prefix의
wall을 동등한 throughput으로 해석하지 않는다. 이번 load1은 6.81–8.92였다.

r46 Record에서 같은 full 요청 span은 131.412초에 시작했고 SourceApply는
134.496–165.890초의 31.394초, 그 안의 owner prepare는 20.929초였다.
32,768건에서 RecordGet 9.678초, BodyGetValidate 9.661초, record
decode/path 검증 0.053초다. Source map apply는 10.118초였다.
BuildPrepare는 165.891초에 시작했다. 두 단계는 원 요청 deadline을 공유한다.
Lexical은 SourceApply가 검증한 body를 토큰화할 때 다시 읽는다. 현재
4MiB attempt cache로는 XL body를 모두 유지할 수 없다. 단계 사이의 bounded
body 전달을 검토하려면 최초 인증과 뒤의 tokenizer 바이트 공급을 같이 보존해야
한다. r48의 새 scratch 경로 이후 실제 CPU·Catalog 시간은 다시 분리한다.

### r49 XL Record의 120초 귀속

`xr49`는 `d4e0a5dc…`의 최적화 pair와 원 server 120초를 사용했다.
staging 95.055초, ACK phase 140.130초, producer exit 1이다. 거절은
BuildPrepare 중 Storage 객체 decode의 bytes charge에서 관측됐다.
reads 3,484,721, writes 10,854,356, bytes 8,767,119,331,
mutations 14,383,635였다. 최초 ACK·검색·재오픈은 미도달이다.

daemon child trace는 3,144,831 events, 누락·drop·open span 모두 0이다.
실패한 SourceAdvance는 116.020초에 시작했다. 아래 wall은 해당 요청 안의
inclusive 시간이다. 부모와 자식 구간을 합산하지 않는다.

| 단계 | 건수 | 누적 wall | 누적 thread CPU |
| --- | ---: | ---: | ---: |
| SourceApply | 1 | 31.740초 | 22.921초 |
| SourceApply의 RecordGet / BodyGetValidate | 각 32,768 | 9.879초 / 9.607초 | 6.383초 / 6.584초 |
| SourceApply의 map apply | 1 | 10.299초 | 8.267초 |
| BuildPrepare의 lexical build | 1 | 85.140초 | 69.893초 |
| Inventory prepare의 token spool / ordered group·pack | 각 22,261 | 4.733초 / 23.429초 | 4.719초 / 17.954초 |
| Inventory Diff의 Posting scratch batch | 75,723 | 17.123초 | 16.448초 |
| Managed install | 173 | 16.967초 | 13.644초 |
| Managed install의 Catalog COMMIT | 346 | 11.022초 | 4.779초 |

BuildPrepare는 85.141초에 종료했다. Inventory Diff는 22,016개 owner에서
25.904초였고, Inventory MapApply는 86회 5.517초였다. Managed install의
물리 write 0.376초, file sync 0.522초, directory sync 0.280초와 barrier
2.807초는 하위 집계다. Catalog mutex wait는 346회 합계 0.002초 미만이었다.
설치 173회 중 87회(10.458초)는 group·pack 안에서 실행됐다. 해당 설치를
제외한 group·pack inclusive 잔여는 12.971초다. 설치의 `managed_object_inputs`
폭은 95·96·160·161개였다. 256행 inventory 창을 약 160개와 96개로
쪼개는 1MiB occurrence staging 한도가 실제 실행에서 작동했다.
이 실행에서는 Catalog의 COMMIT과 tokenizer·scratch 자체의 CPU 비용이
각각 관측됐다. 호스트 load1 4.86–11.64는 r48과 다른 조건이며, Record를
Off 속도 개선량으로 사용하지 않는다.

ASCII token 준비에서 Unicode case iterator의 중복 순회를 제거하는 B65는
이 중 일부를 겨냥한다. 그룹·패킹에서 키별 정렬과 allocation, Posting
scratch에서 실제 VM 실행, 설치에서 reserve/confirm의 고정 비용을 각각
분리해야 남은 절감 규모를 계산할 수 있다. SourceApply와 lexical은 한 시도의
4MiB read cache를 공유한다. 보유된 객체는 `read_retained`, 창 밖의 객체는
재인증 읽기를 거친다. 단계 간 전달을 바꾸려면 memory lease와 최초 인증을
보존해야 한다.

### r51 XL Off의 재실패

`xo51`은 B68–71 `1482d857…`에서 staging 98.897초와 ACK phase
141.436초를 기록했다. producer exit 1, 원 server 120초가 BuildPrepare의
물리 설치 child-validation checkpoint `object.rs:821`에서 관측됐다.
reads 3,552,143, writes 11,324,294, bytes 9,144,965,034,
mutations 15,024,737이었다. 호스트 load1은 5.71–7.07이었다. r48 Off보다
거절 시점까지 계상한 작업량이 많지만 동일한 phase prefix나 matched-host
비교가 성립하지 않는다. 최초 ACK·검색·재오픈은 미도달이다.

같은 pair의 `xr51` Record도 producer exit1이다. SourceApply 32.029초,
BuildPrepare 85.057초, staging 105.159초, ACK phase 142.171초였다.
거절 checkpoint는 Storage 설치 파일의 `object.rs:1095`이며, child trace
3,159,263 events에 누락·drop·open span이 없다. host load1은 5.41–7.86이다.
BuildPrepare는 22,016 owner의 Inventory Diff 27.106초, Posting flush
75,723회 18.001초, Managed install 173회 17.410초, 그 하위 Catalog
COMMIT 345회 12.089초를 기록했다. 겹친 stage를 합산하지 않는다.

B69의 같은 flush parent에 귀속된 21개 표본은 전체 3.305ms, capacity
admission 0.177ms, SQLite `raw_execute` 2.338ms, 나머지 0.790ms다.
표본 중앙값은 각각 0.107·0.007·0.055·0.033ms다. 해당 표본에서 capacity
확인은 약 5%, `raw_execute`는 약 71%다. 실행 비용에는 `(term, owner)`
B-tree 적재가 포함된다. 이 표본의 비율을 전체 18.001초에 외삽하지 않는다.
다음 코드 후보는 삽입 순서나 인덱스 구축 비용을 줄이는 쪽이다. 4GiB scratch
안에서 임시 정렬·중복 감지·정확한 term/owner 순서를 증명해야 한다.

외부 임시 디렉터리의 SQLite 3.50.4 prototype은 소문자 합성 fixture의
434,176 canonical 행을 세 layout으로 적재해 정렬된 결과 digest 일치를
확인했다. 각 layout 3회 중 적재·seal 중앙값은 기존 0.547초, append 후
정렬 삽입 0.539초, 동일 case 쌍 저장 0.189초였다. main DB 크기는 각각
40.89·79.08·20.23MiB였다. append 정렬의 실행 범위 0.499–0.981초는
기존 0.539–0.615초와 겹쳤다. 임시 정렬 파일의 peak 및 제품의 Unicode·혼합
case·page 경로는 이 prototype 범위 밖이다. 같은 suffix·owner·old/new ref를
증명한 쌍만 압축하고, term page·cardinality·cursor에 원래 case 키를
확장하는 계약이 있어야 제품 경로를 바꿀 수 있다.

### B53 — packed Diff의 term당 scalar memo

정적 감사에서 `validated_new_count_observed`는 exact packed cursor가 있어도
RAM scalar memo와 scratch `SELECT ref,validated_count`를 먼저 확인했다.
scalar `Validated(count)`는 leaf에만 기록되며 packed block은 key별 값이
다르다. 64개 기본 RAM 창에 들지 않은 packed ref는 각 term에서 같은
SQL을 조회한 뒤 같은 cursor로 돌아왔다. B53은 같은 ref로 확인된 cursor를
scalar memo보다 먼저 사용한다. 최초 retained read와 block/key 검증,
기존 independent seal, zero-cache 경로는 유지한다.

seed42 XL 생성기를 모델링한 결과 32,768개 파일의 case-prefixed term은
파일당 422–446개, packed payload는 6,274–6,846B로 alias 상한 안이다.
모두 packed 분기에 도달한다고 가정한 최대 조회 후보는 약 1,383만–1,461만
건이다. 실제 경로 비율과 wall 절감은 r46 실행 전까지 미측정이다.
별도로 alias seal과 Diff cursor가 동일 block을 각각 파싱한다. 두 번째 GET을
줄이더라도 독립 persisted seal과 batch 전체 확인 순서를 유지해야 한다.

### r46 XL Off의 scratch checkpoint

`xo46`은 B51–B53과 f112 golden 교정을 포함한 `8684fe0c…`에서
32,768 sources·seed42 staging을 99.610초에 완료했다. 최초 120초
server deadline은 다시 BuildPrepare에서 만료됐다. harness ACK phase는
141.811초, 최초 ACK·검색·재오픈은 미도달이다. checkpoint는
`scratch-sqlite/src/edit_write.rs:175`의 공용 batch `raw_execute`
직후다. 이 코드가 처리한 batch의 caller와 누적 시간을 Off에서 알 수 없다.

거절 prefix의 reads는 35,834,681회, writes 33,561,826회, bytes
45,571,418,304B, mutations 12,806,904회다. r45보다 reads가 6,404,146회
적고 writes가 1,678,870회 많다. 다른 완료 prefix이며 실행 load1의
범위는 4.29–12.41이었다. 이 차이로 처리량이나 B53의 wall 절감을 계산하지
않는다. 같은 binary의 Record를 cooperative lease에서 실행해
inventory/Diff와 physical install/COMMIT의 완료·실패 span을 나눈다.

### r43 후속 감사의 비용 경계

- **XL 물리 객체.** 이전 r39 Record에서 완료된 inventory owner 8,865개에
  lexical prepare 물리 write·fsync가 각각 8,864회였다. 이 근접 수치와 단일
  packed block을 inventory로 재사용하는 cold 경로를 대조했다. per-owner
  block 크기나 write batch 상한 증가로 이 물리 객체 수를 크게 줄일 근거는
  없다. cross-owner 묶음은 `(owner, term)` 선택자와 delta 격리, pin/GC,
  crash/reopen 계약을 바꾸므로 이 구간의 귀속이 확정될 때 별도로 판단한다.
- **XL SourceApply.** cold owner의 record와 body를 각각 한 번 검증한다.
  같은 시도에서 재접근한 인증 객체는 기존 4MiB window를 사용한다.
  r39 RecordGet 17.619초와 BodyGetValidate 17.898초는 inclusive wall이다.
  원 측정으로 물리 get, hash, syscall, 스케줄 대기 비율을 분리할 수 없다.
  StageSources의 선행 검증을 임의의 참조를 받는 SourceAdvance 권위로 사용하지 않는다.
- **Mixed 대기.** 첫 broad query는 원 2초 중 1.517초를 harness 대기열에서
  보냈고 IPC에 약 483ms가 남았다. 제출→ACK의 2.742초 중앙값과 함께 보면
  발행 처리량 및 검색 대기 문제는 재현됐다. 이 표본만으로 서버 query 자체의
  실행 시간, Catalog COMMIT 비중 또는 호스트 경합의 기여율을 단정할 수 없다.
- **확인된 반복.** warm Posting metadata가 한 immutable terms root의 최대
  64개 키에 scalar lookup을 호출했다. 기존 `ScopedReads::lookup_batch`는
  같은 scope의 인증 DATA window를 우회했다. B44는 scope 내 배치 경로와
  Posting 호출을 함께 바꾼다. 기존 wrapper와 df 검증 및 독립 seal은 유지한다.

- **XL scratch flush.** r39의 Posting scratch flush 59,861회·9.102초에는
  64행 묶음의 실제 B-tree 삽입이 포함된다. 한 spool transaction 안에서
  이미 packet당 한 SQL을 실행한다. inventory batch 사이에 잔여 행을 합쳐도
  실행 감소는 약 0.2% 이하다. 9.102초 전체를 추가 COMMIT으로 계상할 수 없다.
- **XL inventory Map readback.** 같은 r39의 완료된 34개 배치에서
  `NativeTextInventoryMapApply/2`는 wall 6.494초·thread CPU 6.211초,
  work reads 7,686,081회·work bytes 4,822,761,332B, work writes 0이었다.
  work는 물리 read bytes나 장치 I/O가 아니다. 이 단계의 CPU와 읽기 반복은
  물리 fsync와 구분된다. 단일 packed block owner의 실제 수와 SQL 행 비용을
  나눠 보고, 독립 seal을 유지하는 bounded alias witness가 제거할 범위를 정한다.
- **회귀 기대값.** f102에서 실패한 occurrence 테스트 4개는 f103의 B44
  이전 소스에서 동일하게 실패했다. 차이는 spool이 살아 있는 동안 보유하는
  `PacketEvidence` grant 8,208바이트와 일치한다. 거절된 packet의 자체
  grant와 spool 소유 grant를 분리하고 spool drop 뒤 0으로 복귀하는지 확인한다.

### B22 Large 실측

r31 `01c4bb4d…`와 r32 `58d69e24…`는 동일 corpus digest, Native config digest,
4,096 sources·seed42, 120초 dispatch, 명시 work cap을 사용했다. 각 pair의 소스·
두 바이너리 해시와 실행 artifact 해시를 대조했다. 다음 수치는 first full 요청의
최상위 Map과 그 하위 trace에 해당한다.

| 지표 | r31 Record `lr26` | r32 Record `lr27` |
| --- | ---: | ---: |
| terms / owners staged | 12,394 / 274 | 5,272 / 274 |
| 실제 write / file sync | 각각 12,668 | 각각 5,546 |
| 물리 설치 배치 | 50 | 22 |
| Catalog COMMIT (`SqliteWork/3`) | 63 | 28 |
| 최상위 Map wall / CPU | 27.557 / 18.543초 | 12.464 / 6.720초 |
| COMMIT wall / CPU 합계 | 8.756 / 3.718초 | 4.095 / 1.220초 |

물리 객체 수가 56.2% 줄었고, 정적 모델의 5,546개와 실제 write·file sync가
일치한다. r32의 Off·Record 최초 ACK는 46.419초·48.192초, 첫 검색은
95.888ms·103.202ms, 재오픈 검색은 56.732ms·44.830ms였다. 두 제품 실행의
producer exit는 0이었다. Record first/reopen은 677,649/957 events, drop/open 0이며
canonical profile을 검증했다. prebuilt 형식의 managed build binding은 `UNPROVEN`이다.

r32 Record에서 first full의 `lexical_build`는 wall 42.028초, CPU 28.047초였다.
이 안의 Posting은 22.009초, 최상위 Map은 12.464초다. 별도 source staging은
Record 15.071초와 Off 21.643초였고 첫 ACK의 server dispatch 구간에 포함되지 않는다.
r32 Off load1은 21.69–29.97, Record는 13.82–37.28이었다. 측정 중 호스트 전체
swap in/out은 각각 15.758/37.214GiB였다. syscall·배치·COMMIT 감소는 코드 변경의
직접 증거다. 호스트 상태가 달라 전체 wall 차이의 코드 기여율과 tracing overhead는
분리할 수 없다.

Source staging의 `lr27` 15.071초는 SDK 상한 32개를 적용한 128회 요청과
일치한다. 각 요청은 본문과 레코드를 하나의 내구 설치로 묶는다. 인코딩 합계
0.010초, Catalog lock wait 0, lock hold 7.521초, 물리 설치 7.052초였다.
32개 상한을 높이면 트랜잭션 반복은 줄일 수 있지만 8,192개의 내구 객체 설치는
남는다. 현재 120초 ACK 계약 밖의 시간으로, 이 측정에서 상한 변경을 요구하는
실패는 확인하지 못했다.

`lr24`는 producer 종료 후에도 Record 검증을 계속했다. 캡처와 제품 검증기가
동일한 authenticated raw child를 각각 엄격 파싱하던 경로를 한 번으로 줄였다.
제품 검증기가 raw custody와 엄격 파싱을 담당하며, 독립 호출자가 파싱 결과를
전달하면 raw와 대조한다. 캡처·제품 검증 Python 테스트 520개가 통과했다.
managed build receipt가 없는 prebuilt 경로의 테스트 2개는 제외됐다.
이 변경의 Large 검증 시간 재측정은 `NOT_RUN`이다.

`lr24` 첫 Full의 `build_prepare`는 86.583초다. 그 안의 lexical build는
wall 86.443초와 CPU 56.503초다. inventory prepare 20.785초 중 32개 물리
flush가 12.646초였고, inventory diff 9.866초와 inventory MapApply 3.671초가
이어졌다. SourceApply 앞의 owner prepare 8.683초 중 record get은 3.846초,
body get·UTF-8 확인은 4.002초였다. 해당 단계와 Lexical은 이미 같은 attempt
read cache를 사용한다. 현재 trace로 cache miss 절감량과 host 대기를 분리할 수 없다.

첫 Full의 cold inventory 32개 설치 배치는 각각 128 owner였다. 각 배치는
하나의 물리 설치와 reserve/confirm을 수행한다. B21은 cold 기본 창을 기존 hard
bound인 256으로 채운다. warm/명시 설정 상한 64, occurrence 1MiB, source 8MiB,
기존 사전 메모리 승인은 유지한다. 같은 fixture에서 바이트 한도가 먼저 차지
않으면 16배치·32 COMMIT까지 줄어든다. 배치별 live memory 증가와 실제 감소량은
고정 소스 `60d74310…`의 Large에서 확인한다.

## B23 — Inventory Diff의 packed DATA 재사용

`lr24` 첫 Full의 Diff 4,096회는 wall 9.866초와 CPU 8.300초였다. 그 안에서
posting edit 5.155초, inventory read 2.168초, occurrence read 1.858초였다.
count memo는 이미 selector 재검증을 1,142,420회 생략했다. 남은 중복은 동일
Diff에서 packed inventory cursor가 읽고 검증한 블록을 첫 count가 다시 해석하는
경로다. attempt read cache에 따라 실제 디스크 read 절감량은 달라진다.

새 경로는 같은 ref의 cursor가 검증된 DATA를 보유할 때 count에 재사용한다.
첫 count의 adapter `read_retained`와 객체 일치 확인을 수행한다. 전체 merge 및
token 합계가 일치하면 `CheckedBlock`과 grant를 기존 2슬롯 reader로 이동한다.
재사용 설정 0, 기존 cursor/CAS 검사, 오류 후 미채택을 검증 범위에 포함한다.
f74의 집중 packed Diff 회귀는 통과했다. Large 절감량 측정은 `NOT_RUN`이다.

## B24 — 읽기 전용 Catalog scope의 Record 이벤트 감소

`lr25`에서 확인한 빈 `lane22`를 줄이기 위해 상위 Catalog의 I/O 계측을 첫
write/sync까지 지연한다. Scratch의 기존 활성화 경로를 사용한다. 중첩 Catalog의
reentry 경계와 실행된 callback의 카운터 묶음, sync·거절 관측은 유지한다.
프로파일 Python 검증은 28개 통과했다. f74에서 read-only·실제 write·sync·checkpoint
회귀는 통과했고, 이전 VFS 두 테스트가 빈 scope의 lane22를 기대해 실패했다.
고정 fixture의 scalar arm은 이전에도 write 0건이었다. 기존 row/custody와 양수 write
counter 기대를 보존하고 empty scope 관측만 수정했다. `01c4bb4d…`의 f75에서
통합 집중 405개가 통과했다. Large 이벤트 수·ACK 재측정은 진행 중이다.

`r31`의 Off `lo27`은 최초 ACK79.470초, 최초 검색580.8ms, 재오픈 검색189.2ms였다.
증분·삭제·no-op·이전 pin·drain을 완료했고 producer exit0·117.782초였다. load1
18.14–23.42, swap 이동8.504GiB, 5초 간격 sampled daemon RSS 최대95.53MiB다.
이전 Off와 host 상태가 달라 wall 차이의 코드 기여분은 미측정이다.

`lo27ctl`과 `lr26ctl`의 보조 요약은 이전 실행 경로를 참조한다. 결과는 각 현재
`execution.json`과 artifact 해시, r31의 source/binary 해시를 직접 대조해 판정한다.
후속 r32 컨트롤러는 동일 run 경로를 생산·수집에 사용하고 source/binary/mode가
현재 실행과 일치하는지 검사한다.

같은 r31 pair의 Record `lr26`도 최초 ACK77.573초, 최초 검색385.3ms, 재오픈
검색205.0ms로 통과했다. 증분·삭제·no-op·이전 pin·drain과 product/canonical
검증을 완료했다. producer exit0·118.690초, 첫/reopen footer 706,957/1,125 events,
drop/open0이다. load1 22.50–36.64, swap 이동4.072GiB, sampled daemon RSS
최대89.72MiB다. Off·Record는 로컬 제품 검증이고 matched-host 비교는 `NOT_RUN`이다.

첫 Full의 cold inventory 물리 설치는 `lr24` 32×128개에서 `lr26` 16×256개로
줄었다. `lr26`의 최상위 Map은 terms 12,394개·owners 274개를 canonical report에
기록했고 실제 write/file sync도 각각 12,668개였다. 50개 물리 배치와 63회 COMMIT은
동일했다. Map wall/CPU는 27.557/18.543초, COMMIT wall/CPU는 8.756/3.718초다.
B22의 물리 노드 감소를 비교할 실행 기준이다.

첫 child 이벤트는 `lr24` 876,074에서 `lr26` 706,957로 19.3% 감소했다.
`lr26`의 `SqliteWork/22`는 18,718개이며 runtime maintenance의 zero-write scope는
21개다. 같은 유형의 `lr25` zero-write scope 10,406개와 대조된다. 이 이벤트 수와
별도로 ACK 차이는 host·코드가 함께 바뀐 구간이다.

별도로 Inventory scratch의 `UPDATE root=root-256`은 `WITHOUT ROWID`의
`(root,k)` PK를 실제로 이동한다. 현재 이 작업이 old/new 행 envelope를 모두
사전 예약한다. 같은 첫 Full의 inventory flush에서 managed reserve/install/confirm을
제외한 wall은 최대 약 2.19초다. 안정 키와 단일 packet 가시성으로 이 이동을 없애는
경우에도 마지막 충돌 재검사, 전역 cursor 거절, marker 뒤 optional memo 실패 동작과
추가 index 비용을 함께 검증해야 한다. 해당 스키마 변경은 보류한다.

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
한도로 채택 판단한다. 원 Large120초·별도 ANN 조건을 완화하지 않는다. Medium의
fixed512MiB는 진단 입력이며 Large 수용의 hard gate가 아니다.

후보 source는 Main의13개 파일을 외부 안정 `s3`에 복사해 local snapshot으로 고정했다.
그 source freeze 당시 Root는 공유 Main HEAD/index를 바꾸지 않았다. 이후 다른 작업의
커밋까지 불변이라는 뜻은 아니다. `candidate-source-03.json`의 focused status는
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
mr6 Medium 진단 body는 FAILED다. Large120은 NOT_RUN이다.

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
실제 Medium fixed 진단은 FAILED이며, B03-B2의 8KiB/512-term 실험은 같은
진단 cap으로 비교한다. 별도 Large 제품 cap은 변경하지 않는다.

### B03-B2 코드 후보와 아직 없는 제품 증거

다음 후보는 한 owner의 occurrence dictionary block을 기존 `QITXOC01` wire/reader
경로에서 4,096B/128행에서 **8,192B/512 canonical term**으로 확장한다. 같은
positions vector 공유, singleton/oversize leaf, nonempty-prior Patch의 unchanged
old ref 보존,64행 SQL flush·planned-confirmation row/ref binding은 유지한다.
`a6083fb2` 대비6개 파일 변경을 포함한 소유 입력15개를 외부 stable `s3`의
`878be6c9e4e33dd35b9c80cc5b450c2cf3f6f099`에 고정했다.

새 output policy(admission `[31;32]`)의 literal digest는
`b2c1df18b452485d69ef4e687b7a3519f07cfff76fe25f0a665d8706258d7d95`다.
직전 4KiB/128행 policy `eef15862…`와 leaf-only `559e0482…`는
`validate_instance`의 typed refusal oracle로 남겼다. 동일 wire magic만으로
다른 bounds의 old object를 현재 policy로 재해석하지 않는다. 테스트 소스에는
512행/8KiB·Full·cancel/memory 및 실제 FileObjectStore/SqliteCatalog의
260-selector 한 block·public 첫/마지막 selector·delta·pin/reopen case가 포함됐다.
기존 lifecycle case가 마지막 pin·GC를 다룬다. f20 Native는295개 통과·558개 제외로
VERIFIED다. Kernel은 새 test helper의 `StoredObject` deref를 통해 registration
children을 move한 E0507 컴파일 실패로 body0이었다(FAILED). 또한 큰 inventory의
contribution에는 occurrence ref 260개 대신 EntryMapBlock root 한 개만 연결된다.
Main oracle는 이 root의 인증된 positional children과 독립 260-key `QITXEM01`
golden을 비교하고 public 첫/마지막 selector reader를 유지하도록 교정했다.
제품 코드 불변·테스트2파일만 교정한 frozen `a60ac6576a0ca2cae306ee68a014b11c3736246f`의
f21은 Native 새 distinct-vector case1개 통과·853개 제외, Kernel14개 통과·65개 제외다.
f20 Native295개 통과·558개 제외와 합쳐 집중 composite local proof는 VERIFIED다.
정확 a60 소스의 전체 Native suite는 미실행이다. r7 정상 cached build는 완료됐지만
mr7 Medium은 536,869,205B 사용 후12,164B charge가 Catalog
`managed_install.rs:108`에서 fixed512MiB 진단 cap에 거절됐다. reads878,940/writes573,536/
mutations351,513, physical2147 writes·3,680,139B이며 I/O 오류는 없었다.
lo7 Large Off는 원120초 요청124.194초에서 Catalog `managed_install.rs:151` deadline으로
FAILED다(producer157.629초, full ACK 없음). reads15,477,290/writes11,733,540/
bytes11,341,756,074/mutations6,738,039이며 cap·memory·external 거절은 없었다.
Large Off 결과는 B2가 원120초 제품 목표를 해결하지 못했다는 증거다. Medium
fixed512MiB 실패는 별도 진단 결과다. seed42 투영
Medium512→256, Large12,288→4,096 block은 코드 기반 추정이며 설치 호출·CPU·wall·SLO
개선 실측이 아니다.

B3는 B2의 owner-local occurrence block이 성공적으로 공유되더라도 QITXEM01
inventory가 selector마다 같은 child ref를 반복 등록하는 중복을 겨냥한다. B2 r7
Medium/Off 실패만으로 이 중복이 남은 지배 비용이라고 단정할 수 없다. 새
QITXEM02는 정렬 row마다 고유 child slot을 저장하고 registration에는 first-use
고유 ref만 둔다. 기존 public key→ref mapping과 pin/GC authority는 보존한다.
Kernel 독립 fixture는 초기 증분에서 a/b의 기존 block을 재사용해10행/고유4 child,
중간 Tree 뒤 복귀할 때 새 leaf5개로10행/고유5 child가 되는 차이를 gold로 둔다.
정확 policy SHA는 old B2 `b2c1df18…` 재현 뒤 new `14c6d531…`로 산출했다.
19개 소유 입력/8개 변경의 external frozen `39f4a9c3bd82843aea2b390411d8ec531a0d3b2c`에서
f22 Native297개 통과·558개 제외(3.371초), Kernel14개 통과·65개 제외(114.062초)로
집중 회귀는 VERIFIED다. f23 lexical strict Clippy는 exit101/FAILED다. 전역38개
진단은 기존 타 owner33개와 Native 새 distinct-vector 테스트 helper5개(정수 나눗셈2,
산술 부작용3)로 분리된다. 테스트 helper 산술1파일만 고친
`996230439fbf187661c7462b018c113c8b3de1bd`의 f24 strict Clippy도 exit101/FAILED지만
진단33개는 모두 타 owner이고 Native 진단은0이다. f25 Native 변경 case1개는
통과·853개 제외/source guard0으로 새 소스의 기능 영향을 확인했다. f22 Native297개·
Kernel14개와 합친 집중 proof이며 Native298개 실행으로 합산하지 않는다. r8 정상 cached
build는 exit0이지만 mr8 Medium은 536,864,010B 사용 후9,988B charge가 Catalog
`managed_install.rs:108`에서 fixed512MiB 진단 cap에 거절됐다(producer15.888초·exit1).
reads892,017/writes446,724/mutations219,407, physical2364 writes·2,256,813B,
file sync2857/confirm2857(physical reuse493), barrier40·I/O 오류0이다. child trace는
49,370 events 완결·dropped/unwritten/open/lease0이다. lo8 Large Off도
producer159.174초·exit1, `full_event_ack`124.654초 미완료로 FAILED다.
`BuildPrepare`의 Storage `map/completed_group.rs:140` deadline이며 누적 사용은
15,381,374 reads/8,331,877 writes/9,933,434,999B/3,283,526 mutations이다.
numeric cap·memory·external resource 거절은 없었다. Medium의 512MiB와 달리
Large는 explicit 67M/512GiB 논리 funding으로 실행했다.

같은 `99623043…` binary pair의 lr8 Large Record도 full ACK125.721초 미완료,
producer185.890초·exit1로 FAILED다. 원인은 `BuildPrepare`의 Storage
`map/completed_group.rs:140` deadline이며 13,486,261R/7,747,424W/
9,054,086,583B/2,728,115M에서 numeric·memory·external 거절은 없었다.
물리31,630 writes·43,770,402B, I/O 오류0이다. child V5 footer는841,911 events,
dropped/unwritten/open/lease0으로 COMPLETE다. 이 수치는 제품 성공이나 정식
Off/Record overhead 비교가 아니다.

닫힌 child의 실패 LexicalBuild 한 span은 wall112.262초/CPU58.303초,
13,207,048R/7,643,573W/8,977,635,886B/2,645,991M이다. 선택된 하위 inclusive
단계는 Prepare wall20.516초/CPU11.430초·5,971,958,518B, Map
13.870초/6.490초·1,486,154,106B, Diff9.774초/7.388초·384,541,527B,
Postings51.408초/26.034초·1,093,627,763B다. 실패 Postings lane37 자체는
wall50.629초/CPU25.836초·1,093,365,205B다. ManagedObjectInstall interval을
중복 없이 합친 wall union은 실패 Lexical 안28.636초, lane37 안13.196초다.
Prepare lane34/35와 Diff는 각각4,096 owner span, Map은64 batch까지 도달했다.
Postings lane37은 실패한1 span이다. posting-edit17,949 span은 parent chain상
InventoryDiff 아래이며 lane37의 자식은0개다. 이 수를 첫64 posting map의 edit
수로 사용했던 해석을 철회한다. ManagedObjectInstall은 실패 Lexical 안230개
최외곽 span/CPU16.888초, lane37 안38개/CPU7.657초다. 두 집합 안에서 중첩된
ManagedObjectInstall은0개였다. 따라서 설치 interval 밖의 wall은 각각 약83.626초와
37.433초지만 원인을 단정할 수 없다. reserve31.524초·confirm55.870초는 처리 전체의
inclusive 집계이므로 위 union과 더하지 않는다. 상위·하위 inclusive 시간을 합산하거나
전체 wall에서 임의로 빼지 않는다.

**Postings51초의 직접 자식 재분해:** 실패 lane37의 wall50.629064초/CPU25.835966초
중 직접 자식 interval union은48.022905초이며 미계측 간격은2.606159초다.
아래 직접 자식끼리는 중첩하지 않는다. 표의 CPU가 없는 구간을 host wait로
치환하거나 wall−CPU를 전부 scheduler 경합으로 해석하지 않는다.

| lane37 직접 자식 | 수 | wall 초 | thread CPU 초 |
| --- | ---: | ---: | ---: |
| ManagedObjectConfirm | 38 | 23.641956 | 11.003578 |
| ManagedObjectInstall | 38 | 13.195895 | 7.657016 |
| ManagedObjectReserve | 38 | 9.816076 | 4.140055 |
| SqliteWork lane22 write envelope | 5,187 | 1.368979 | 미계측 |

38개 reserve/install/confirm triplet은 각각256 input slot을 처리했고 모두ok다.
이는9,728 registration input slot이며 고유 물리 write 수와 동일하지 않다.
confirm 내부 COMMIT은16.331305초/CPU6.000718초, reserve 내부 COMMIT은
2.975408초/CPU0.370181초다. 합계76 COMMIT19.306713초 중 checkpoint/backfill
lane25는39회10.571434초/CPU4.610249초이며 COMMIT에 포함된다. confirm의
preflight3.922416초, register/transfer2.969227초, protection/lifecycle0.383599초도
서로 다른 SQL phase다. reserve preflight는5.526690초, mutation은1.105664초다.
catalog lock **hold**33.303790초는 이 SQL을 감싼 시간이다. 실제 lock wait는
reserve0.153324초/confirm 약9µs로, hold를 lock contention으로 부르지 않는다.

lane37의 VFS lane22 요청량은118,343 xWrite calls/1,355,749,752B다. 직접
scratch envelope는10,176 calls/41,680,896B, Catalog confirm은103,095 calls/
1,170,958,816B, reserve는5,072 calls/143,110,040B다. checkpoint lane25의
87,055 main-DB writes/620,584,960B는 이 합계의 부분집합이다. 그중 confirm
38회 checkpoint는86,210 writes/615,071,744B이며 COMMIT 안에 들어 있다.
이들은 **VFS 요청** 계수다. 장치 물리 write, fsync 시간, WAL frame 수나 장기
reader 때문에 checkpoint가 반복됐다는 증거로 승격하지 않는다. 실제 I/O·admission
refusal 계수는0이다. `ManagedObjectInstall`의 object-file write 계수와 합치지 않는다.

같은76개 Catalog hold의 pager 전후 delta는 cache hits6,617,975/misses1,799,043/
writes168,321/spills0이다. 시작 시 읽어낸 설정은 page4,096B·cache49,152KiB
(48MiB)·autocheckpoint1,000pages다. miss는 cold read·eviction·B-tree access·host
paging을 구분하지 않는다. spill0과48MiB 설정만으로 cache 부족을 결론 내리거나
메모리 cap을 올리지 않는다.

따라서 현재 실측 주 대상은 catalog custody의 SQL·durable COMMIT과 physical
install이다. `completed_group.rs:140`은 이 작업 뒤 순회 중 deadline을 관측한
checkpoint이며 그 위치만으로 hash table 결함을 뜻하지 않는다. 기존 same-base
root dedup을 재구현하거나, 자동 checkpoint/FULL 내구성을 끄는 안은 제외한다.
빈 base의 CAS nibble 중복 변환과 known-size buffer 기하 재할당은 별도 소규모
소스 낭비로 확인해 제거했다. 이 두 수정의 영향 검증과 Large 개선량은 NOT_RUN이다.

추가 source 수리 B03-B4는 같은 transaction의 child proof window다. 개별 node의
child가64 이하면 전체 batch의 child가64를 넘더라도64개만 기억하던 capacity
shortcut을 제거했다. 기존의 total 기반 capacity와4,096 unique cap·포화 후 전체
재검증·원 grant를 그대로 사용한다. 어떤 실제 registration batch에 반복 ref가 몇
개인지는 기존 trace에 ref 식별자가 없어 산출하지 못했다. 단위 SQL oracle로64개
이후의 중복 인증 제거와 late corrupt/refusal rollback을 검증하고 제품 영향은 새
source에서 분리 판정한다. 큰 Posting Block 단순 확장은 query 전체 재검증과
증분 전체 재작성·4K 경계 cliff 때문에 NOT_SELECTED다.

lr8의 실행 중 periodic 자원 관측은 child RSS1,362표본/최대96,419,840B
(최대 표본 간격1,646.7ms), fixture allocated330표본/최대370,106,368B와 그 안의
scratch146,673,664B(최대 간격1,671ms·missing7)다.
aggregate process RSS·child process I/O는 이 Mac에서 unsupported이며 표본은 연속 peak를
보장하지 않는다. 따라서 E07 aggregate RSS gate는 NOT_RUN이다. disk traversal
observer wall35.230초, child RSS probe31.801초는
누적 관측 비용으로 request wall에서 빼거나 원인 비율로 해석하지 않는다.
`lr8/host-pressure-diagnostic.json`의 load37–38, swapin91,780/swapout40,124와
압축·해제 약105만 건은 **terminal 이후 17:45:19–29 UTC의 10초** 관측이다.
실행 중 host 병목의 직접 증거가 아니다. 실패 Lexical의 wall−CPU 약53.959초에는
대기·I/O·스케줄링이 섞여 있다. 이 local Record와 Off의 시간 차이로 tracing
overhead나 코드 speedup을 산출하지 않는다.

동일 seed42 Medium의 mr7→mr8 child trace에서 inclusive selected work bytes는
InventoryPrepare 265,135,902→265,135,902B(동일), InventoryMap
147,199,614→67,700,478B(−79,499,136), Diff28,293,989→16,305,857B(−11,988,132),
Postings69,624,440→180,638,964B(+111,014,524)다. LexicalBuild 전체는
532,363,153→532,358,034B(−5,119)다. 단계들은 inclusive이므로 합산하지 않는다.
물리 write bytes는 3,680,139→2,256,813B(−1,423,326)지만 write 횟수는
2147→2364(+217)다. 각 실행은 다른 시간·호스트 경합을 포함하므로 속도 개선율로
읽지 않는다. 관측상 물리 bytes는 줄었지만 제품 cap 실패는 해결되지 않았다.
신규 term의 `token_groups` live SELECT당4,096B는 코드 계약 대조에서 의도된
scratch page touch tariff로 확인됐다. 이 비용을 없애는 가격 변경은 하지 않는다.
Medium fixed cap의 누적 기여와 Large deadline의 지배 원인을 동일시할 근거도 없다.

중복 map build 제거를 새 B4로 추가하는 안은 제외한다. Storage
`map.rs:1880–2080`은 동일 base·edit count·fingerprint의 후보 root를 이미 공유하고,
재사용 전 양쪽 scratch cursor를 EOF까지 검증한다. 같은 최적화를 다시 구현해도
Large `BuildPrepare`의 확인된 신규 비용 단위를 제거하지 못한다.

Query 예산은 별도로 소스 점검했다. 실제 bench Text `read_need`는 64MiB이고,
64-slot `CheckedBlock`의 20,512B layout 예약과 `PreparedRows` backing을 함께
계산하면 2,625,536B다. 최대 payload 524,288B와 작은 `StoredObject` overhead를
합쳐 약 3.16MB charge이며 성장 peak는 steady보다 크지 않다. 이전의 약 1.8MB
추정은 layout과 backing의 동시 funding을 빠뜨렸다. Kernel은 registry ceiling과
held reservation을 대조하고 SDK는 need를 override하지 않는다. 4MiB 테스트 helper는
제품 sizing 증거가 아니며 더 작은 custom need는 `ResourceLimited`로 거절될 수 있다.
이 분석은 메모리 계약 적합성의 코드 산술로, 실제 RSS·query latency·retained-read
수용은 r7의 Medium/Off 실패만으로 확인되지 않았다.

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
일은 허용하지 않는다. 원 Large120초, Medium fixed512MiB 진단, ANN4K의 별도
1M reads/512MiB·SDK30초/worker10초를 혼합하지 않는다. Large·ANN gate는 완화하지 않는다.

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

## r4 초기 판단과 당시 해결 방향

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

[WORKPLAN](WORKPLAN.md)의 당시 B03-B1 계획은 확인된 r4 실패를 기준으로
owner-local occurrence block의 codec·producer·reader·정책과 profile-first 초기
Patch 활성화를 합류하고 원 예산·독립 oracle로 판정하는 것이었다. 이후 B2의
8,192B/512-term 확장과 B3 QITXEM02까지 구현·집중 검증했다. 당시 고정본은
Large120에 실패했다. 후속 r34 판정은 [현재 원장](README.md#현재-판정)에 있다.
현재 B02의6개 검증·재사용 계수 생산자와 소비자는 소유 회귀까지 합류했다.
최신 r4의 정상 cached build와 실제 Medium/원 Large Off·Record 캡처는 완료됐고
모두 위 실패로 기록했다. 과거 C17의8,799회 get을
신규 재검증만의 비용으로 재분류하거나 새 성능 수리의 효과로 해석하지 않는다.

## Root의 Legacy etcd 60초 SDK 검증

2026-11-09 KST의 고정 Legacy 입력은 1,421개 파일이다. 원 SDK I/O 제한은
60초이며 physical inventory 예약 한도는 30,000개다. Native 직접 발행의
512MiB·100만 work 한도 검증은 별도 경로다.

계측 실행의 SDK 요청은 61.569초에 서버 발행을 마쳤다. Native 발행
20.974초, File 발행 13.697초, Native changed-segment 검증 3.865초,
File seal 5.353초였다. 시간은 중첩 구간을 합산하지 않고 해석한다.

저장된 gram 경로 635,648행은 리프당 256행 제한 때문에 MapPage
2,523개로 나뉘었다. dictionary 압축 뒤 payload와 의존 수를 검사하는
전용 wide gram leaf를 추가했다. 새 leaf는 최대 1,024행·논리 참조
128개·의존 객체 128개·32KiB payload다. 일반 bootstrap은 256행을
사용하고 변경된 wide leaf의 delta는 기존 일반 형식으로 다시 작성한다.
새 reader는 기존 mode 0·1과 새 mode 2를 함께 읽는다.
기존 binary는 mode 2를 읽지 못하므로 새 상태를 발행한 뒤에는 해당
형식을 읽는 binary가 필요하다.

고정 빌드 `legacy-route-key-v5`의 원 SDK 실행 두 번은 발행
52.367초·53.127초, 활성화 ACK 103.774ms·59.733ms로 모두 종료 코드
0이었다. lexical·semantic·Hybrid 응답을 받았다. 같은 635,648행의
실제 MapPage는 632개로 줄었고 첫 실행 state는 10,951개 파일이었다.
소스·바이너리와 입력 바인딩은 외부 실행 묶음
`/Users/songmin/.codex/a88-gq0uxqug/etcd-native-goal-NMu4RbQ5/legacy-route-key-v5`
에 있다. 이는 해당 로컬 고정 빌드의 SDK 반복 통과이며 호스트 간 성능
비교를 수용한 결과는 아니다.

wide 형식을 포함한 별도 current-source Large 표준 테스트는
`binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`
1/1, 실행 39.265초로 통과했다. 서로 다른 두 프로세스의 전체 source와
순위 페이지를 비교했고 실행 전후 소스가 일치했다. 기록은 같은 외부
상위 경로의 `legacy-route-key-v5-large-current/nextest.log`에 있다.
Native Large120 SourceAdvance 검증과는 구분한다.
