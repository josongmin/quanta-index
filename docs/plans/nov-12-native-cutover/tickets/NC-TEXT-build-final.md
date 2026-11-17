# NC-TEXT · Native Text 구축 최종 작업 패킷

2026-11-17 KST 갱신. 사용자가 승인한 최종 설계의 구현·인수 계획과 검증 결과다.

이 패킷은 [전체 퇴역 계획](../TANTIVY-RETIREMENT.md#구조-감사와-최종-실행-계획--2026-11-15)의 Native Text 구현 순서를 구체화한다. 기존 History·Caller·Source·설치 복구·Hybrid·T/A/B/E06의 담당과 종료 조건은 변경하지 않는다. [작성·실행 경계](../README.md#레인별-착수와-인계)와 [원 수용 기준](../DOD.md)을 유지한다. 공용 TANTIVY-RETIREMENT/DOD/원장은 F가 소유하고 이 채팅은 이 전용 패킷을 갱신한다.

동일 소스로 결속한 daemon `f87b6b3d239a…`와 helper의 원본 Record에서 Caddy590은 FULL·검색·재시작 검색 **VERIFIED**, etcd1421은 512MiB Work에서 **FAILED**다. 원본 etcd의 인덱싱 가능한 토큰 1,009,664개가 단일 요청의 mutation 100만 한도를 초과한다. Lexical 372/372, Catalog 390/390과 격리 Storage 226/226은 각 owning 검증 범위다. Caddy Clean Off는 입장 0/2, 동일 daemon Large는 4,096파일·두 번 재시작 **VERIFIED**다. [NTX4 판정과 남은 검증](#admission-decision-and-remaining-product-verification)

## 1. 고정한 결정과 현재 상태

최종 구조는 **제한된 용어 집계 → 기존 정렬 run 병합 → 자식 우선 스트리밍 설치 → 단일 Catalog 게시**다. 저장소 이름·문서 수 추정·임의 빈도 cutoff로 실행 경로를 나누지 않는다. flush는 실제 객체 수·encoded wire·승인된 Original 메모리와 backend 준비 한도로 결정한다.

| 경계 | 현재 구현 | 이 패킷의 결정 |
| --- | --- | --- |
| 토큰 생산 | canonical cursor, occurrence별 case key/8B Row, 정렬·position 그룹, SQL fallback; Original 승인 시 Unicode 상한으로 census 재순회를 생략하고 한 번 fill | 기존 논리 요금·키/position cap·exact census fallback을 유지한다. 별도 term/key/position pool은 측정 근거가 있을 때만 조건부 채택한다. |
| 전치·병합 | 128KiB posting packet·정렬 run·bounded heap merge·term page | 재사용한다. 새 외부 정렬기·전역 term 사전·영속 IR을 추가하지 않는다. |
| 설치 | Existing cold eligible windows use install_coalesced; unsupported/warm/local admission retains install_separate. Directory DATA, Shared Small and ConfirmedColdRoots cross-page reuse are already source implementations. | Reuse current StagedObjectBatch/install receipts and original admission/ref/EOF/CAS checks. NTX1b whole streaming expansion is conditional after actual cost evidence, not a missing first coalescer or prerequisite for the present SDK candidate. |
| 공개 | reserve → 파일 write/sync/barrier → confirm → 최종 Catalog root CAS | 유지한다. 객체 확정과 view 공개를 같은 동작으로 취급하지 않는다. 파일 I/O 동안 SQL write transaction을 열어 두지 않는다. |
| 여러 packet의 원자 전환 | 내부 `install_prepared_run` 존재, 공개 prepared 계약은 childless 전용 | NTX3로 후순위 분리한다. NTX1 전에 새 공개 port를 필수 발급하지 않는다. 기존 occurrence prepared-run은 재사용한다. |

**초기 비교 기준.** [원본 SDK 기준 실행](NC-F-integration.md#cold-bounded-posting-원본-순회)의 release SHA-256은 `16d8d0ee68669003c072a134443d1af6ad895864ec7f6e8f5b43e116982493bc`다. Caddy는 60.074초 취소·ACK 0, etcd는 Work 536,870,906B에서 추가 80B 거절·ACK 0이다. SQLite COMMIT은 1,784회·30.056초이고 WAL xSync는 1,992회·14.103초다. 포함 관계가 있는 시간은 합산하지 않는다.

당시 ordered-packet 후보 `packed_runs.rs`의 SHA-256 `a24b5c6bbd05391fcc69f97fe03670b7ab8c650256783b956f53669797c91667`에는 ordered packet의 재정렬·재복사 생략이 있다. 위 SDK 후보에 포함된 파일 SHA는 `6f4f2e0506625d8d03084a414c4df8c46d2cc29f78fd6c1800c1f1ef3149c485`다. 두 후보를 같은 것으로 비교하지 않는다. 해당 변경의 집중 Text/TextSpool/Literal nextest `920b5d10-c4af-4051-8ef8-57c0ce2d5918`은 361/361 VERIFIED지만 원본 SDK 수용은 NOT_RUN이다.

| 패킷 | 내용 | 선행 입력 | 현재 실행 상태 |
| --- | --- | --- | --- |
| NTX0 | ordered-packet 후보의 SDK 비교 기준선 | 관련 source·입력·binary 결속 | 이전 Record FAILED, Caddy reads·etcd bytes 한도 및 quiet Off 입장 차단 |
| NTX1a | Implemented cold-window leaf+directory coalescer | 현재 계약과 실제 caller RED; 정량 비교에는 NTX0 | Runtime3·Lexical367·Kernel2·Storage2·UDS1 VERIFIED; 원SDK 후속 실행 |
| NTX1b | 자식 우선 스트리밍 설치기 — 최종 구조 | NTX1a 결과와 정확한 후속 hunk 인수 | NOT_APPLICABLE — 최초 구현 뒤 확대 판단 |
| NTX2A | etcd 배타적 Work·입력 형상 분해 | NTX0와 같은 원본·계측 계약 | Work·shape 7·parser 621 VERIFIED; 실제 SDK 실패 prefix 분포 확인, full ACK 아님 |
| NTX2B | 제한된 term/position 생산자 | NTX2A의 비용·Original 입장 근거 | NOT_RUN: conditional adoption; distinct key-byte interning is not in current ColdTokens; adopt only after measured cost and Original admission. It is not an unconditional exit task. |
| NTX3 | packet 간 prepared-run 확대 | NTX1 이후에도 해당 경계가 병목이라는 실행 근거 | NOT_APPLICABLE — 현재 실행 범위 밖 |
| Shared Small | DF2..8 term별 wrapper를 고정 크기 공유 block으로 저장 | 실제 Caddy reads 거절 RCA·Root exact hunk loan | Descriptor 후보 Lexical407·Kernel6·Runtime12 VERIFIED; 원 SDK 읽기 절감 확인, 수용 FAILED |
| Directory DATA | source-owned fence/kind/terms/child를 scratch→builder로 전달 | 현재 사용자의 `$ss` 명시 실행 승인 | Lexical407·Kernel6·Runtime12·UDS1 VERIFIED; 원 SDK Record FAILED, quiet Off BLOCKED |
| Confirmed cold Dense | 같은 attempt의 root를 term page 간 재사용; obsolete Small SQL/index 퇴역 | 실제 Store/Catalog 3page RED | Lexical408·Runtime13, 보강 all-slot Runtime1·Kernel6/UDS1 VERIFIED; 새 Small 변경 뒤 영향 범위 재검증 |
| Small cross-page | case별 bounded draft와 fitting EOF tail 병합 | 실제129term RED: 기존5블록 != 고정2블록 | 최종 Text 보완 합류 후 Lexical422+Runtime15 =437/437 VERIFIED. F의 이전 통합10 결과와 현행 SDK 수용은 구분한다. |
| T-PREPASS-COUNT-DATA | 인증된 term-page counts를 전달하여 동일 packet의 cardinality 재조회·복사·parse 제거 | 원 logical read64 유지, 독립 integrity hash2157B gold, foreign/epoch/case/Top 회귀 | 최종437/437 VERIFIED; 생산자 예산 수명·EOF 무작업·원 오류 분류 보완 포함 |
| T-CENSUS-SINGLE-FILL | 승인된 Unicode 상한 arena로 기존 cursor 한 번 fill | 실제 caller RED와 exact fallback3083B·Unicode/empty-owner 고정 oracle | 최종437/437 VERIFIED; 중간 원 SDK에서 실제 census8,353,147B/15,357,748B 제거 확인 |
| NTX4 | 같은 최종 후보의 제품 수용 | 채택한 구현의 소스 결속과 원본 입력 | Caddy Record VERIFIED, etcd Record FAILED 및 고정 mutation 초과 확정. Quiet Off NOT_RUN, 동일 daemon Large VERIFIED. |

**코드 검증: VERIFIED(각 범위).** 인증된 cardinality DATA 재사용과 Unicode 상한 single-fill로 실제 재조회·복사·census 재순회를 제거했다. 원 logical read/edit/sort 요금, EOF/CAS/독립 seal/readback 및 Work512MiB를 유지한다. 중복 제거 단계 owning nextest `4f9e0f40-8f95-4ad6-a31f-21cd66e43852`는437/437 PASS/738skip/exit0(Lexical422+Runtime15)이다. 이후 실제 RED로 재현한 paired cached-EOF epoch 공백은 기존 검사 한 줄을 연결했고 TextSpool86/86 PASS(`72852bce-92a0-4a68-87f3-c9c04d60320c`)로 검증했다. 서로 다른 소스 결과를523건 또는 새439건 통과로 합산하지 않는다. 원 SDK에서 실제 Text census 제거는 Caddy8,353,147B/etcd15,357,748B이며 정확한 selector는 `native_text_inventory_prepare/43`이다.

**이전 실행 후보:** `/private/tmp/qidx-sdk60-text-copy-ym3jb7a9/eof-guard`. Text EOF guard와 기존 Storage 담당자의 compact retained-value 변경을 함께 빌드했다. 첫 compiler101은 ManyChildren 공개 범위 수정 뒤 같은 명령의 r2에서 해소됐고 release exit0/114.572s·전후2802개 입력/HEAD 일치를 확인했다. Immutable daemon SHA256 `c394552525b0051e3aa3de3c3a492f882ef1f9147e0e0628419f935d399a1f1b`, helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. 원 Caddy590/3272107B·etcd1421/8666217B, 파일30000/Work536870912B/R-W-M1000000/SDK60은 유지했다. Input manifest·전 파일·commit·query token은 SDK 뒤에도 일치한다.

**이전 원 SDK: FAILED.** 원 Record Caddy37.55s는 B536848780+38312, Catalog `managed_install.rs:108` 예약 admission에서 거절됐다. Etcd20.87s는 B536770775+139551, `packed_runs.rs:835`의 기존 `sort_work` 선청구에서 거절됐다. 해당 packet의 정렬과 두 번째 body 복사는 아직 실행되지 않았다. 양쪽 FULL ACK0/query·reopen NOT_RUN, COMPLETE_DIAGNOSTIC·health[]·open_requests0·trace SHA 일치·parent exit1·mutex 반환이다. Record 시간을 quiet 성능으로 쓰지 않는다. Off는 load27.558594>8로 본문0 BLOCKED이며 override하지 않았다.

**현재 소스와의 경계:** 최종 SDK 동안 기존 Storage 담당자가 map.rs/diff.rs/node_wire.rs/original_lookup.rs/retained_value.rs/value_codec.rs 및 sparse_branches test를 다시 수정했다. SDK 전후 binary·diagnostic input·원 입력은 같지만 공용 생산 소스7개 hash는 달라졌다. 따라서 위 결과는 빌드 전후 동일했던 고정 후보의 실패 증거이며 현재 공용 소스의 SDK qualification이 아니다. Text hunk 동일성은 해당 SDK 캡처 구간에 한정한다. 이후 packed_runs.rs 해시 변경은 현행 소스와 별도로 대조하고, 기존 담당자의 source-ready 뒤 변경 영향을 받은 owning 검증·최종 pair·원 SDK 증거를 재결속한다. 전체 제품 수용은 완료하지 않았다. 정렬 요금만 지우거나, 조건부 offset-index codec을 미측정 순절감으로 채택하지 않는다.

**Text·dense-install 범위 VERIFIED.** No-default Lexical·Runtime 결합 session32393은445건 실행,444 PASS,1 FAIL,739 skip, exit100이다. Text term BLOB·Directory borrow·paired count seal 보완과 Runtime dense-install은 통과 범위다. 유일한 실패인 F의 Literal V4 고정 wire는 이후 독립 집중 nextest `8768d533-71d4-4d3f-873d-4e7236b7adbe`에서 다른 Literal 회귀와 함께41/41 PASS했다. 이 두 실행을 결합 회귀 전체 PASS로 합산하지 않는다.

**이전 전체 소스 결속 원 SDK: FAILED.** `/private/tmp/qidx-sdk60-case-seal-n24abmut`의 재해시 제거 최적화 daemon `038ad043d29d62574d3578fd818eb358ebc4936b94ce3ed260f382fd2d553ea0`와 기존 helper를 결속했다. 빌드·SDK 전후 2803개 입력과 HEAD, 바이너리·진단 입력·원 corpus 해시가 일치한다. 원 Work512MiB·SDK60 Record에서 Caddy는 Catalog `managed_install.rs:108`의 536860391+52768B, etcd는 `packed_runs.rs:895` 정렬 승인의 536771969+138267B에서 거절됐다. 둘 다 FULL ACK0, query/reopen NOT_RUN. Etcd COMPLETE_DIAGNOSTIC·health[]·open_requests0, Caddy trace는4795 dropped/2 open·invalid로 진단 집계 제외(최초 거절은 daemon의 Work event). Quiet Off preflight는 부하18.807129>8 및 외부 Rust6으로 body0 BLOCKED다.

**후속 Literal V4 고정 바이너리 진단도 FAILED.** `/private/tmp/qidx-sdk60-literal-v4-ci2rwa01` Record는 Caddy536829657+48028B의 Catalog 예약, etcd536870908+68B의 scratch edit admission에서 거절됐다. Caddy590·etcd1421, 512MiB·SDK60 원 설정이다. 두 trace는 COMPLETE_DIAGNOSTIC, digest 일치, health[]·open_requests0이다. Etcd inventory diff 성공803→862, Literal 준비 Work는 Caddy−9125478B(R4 대조), etcd−43702954B(R5 대조)다. 빌드 중 daemon 소스가 변경된 고정 바이너리 진단이며 FULL ACK0, query·reopen 및 Off NOT_RUN이다. 현행 제품 수용은 미완료다.

**이전 릴리스 결속 원 SDK Record FAILED.** Borrowed compact 검증과 Catalog proof를 함께 포함한 `/private/tmp/qidx-sdk60-managed-proof-93kedcbg`의 원 설정 결과다. 같은 네 검증 window의 원 읽기 수를 유지하며 Caddy1,589,066B·etcd3,699,459B 절감, 소유 Text/TextSpool360 PASS. Caddy는 Catalog536856475+60612B, etcd는 다음 directory scratch 초기화536863078+8192B에서 거절됐다. 두 trace COMPLETE_DIAGNOSTIC, health[]·open_requests0. 두 FULL ACK0, 검색·재오픈 NOT_RUN, quiet Off 본문0.

**다중 디렉터리 R8 원 SDK Record FAILED.** `/private/tmp/qidx-sdk60-directory-batch-l8x2gr6u` 전체 소스·원 corpus·바이너리 결속이다. Caddy의 동일152 directory는 map 호출152→4, 단계 Work6,293,336→3,189,396B다. Etcd의 동일 첫159 directory는 호출159→4, 단계 Work8,301,306→5,009,203B이며 다섯째 window까지 완료했다. Caddy Catalog536853187+52768B, etcd inline alias count의 retained read536870399+5752B 첫 거절이다. FULL ACK0·검색/재오픈 NOT_RUN, Off 본문0, 두 trace 정상. 공유 Storage resident 변경도 후보에 포함됐다. 당시 Lexical/Storage369건 VERIFIED.

**R10 원 SDK Record FAILED.** `/private/tmp/qidx-sdk60-compact-rows.zGLJV5`의 8B cold token 행과 차용 root 계획을 포함한 후보를 결속했다. 같은 token fill의 Work가 Caddy18,160,768→14,961,288B, etcd39,442,536→33,116,592B로 줄었다. Etcd 완료 inventory diff869→944. Caddy Storage 객체 배치 쓰기536141651+752549B, etcd posting 기본 논리 쓰기536869310+17613B에서 거절됐다. FULL ACK0·검색/재오픈 NOT_RUN, Off는 호스트 부하32.44·외부 Rust2로 본문0, 두 Record trace 정상. 당시 Text/TextSpool363건 VERIFIED.

**R11 원 SDK Record FAILED.** `/private/tmp/qidx-sdk60-posting-boundary.IxSFVi`는 posting 순서 증거를 packet 경계에서만 복사한다. Caddy509개 posting 단계의 R/W/M이 같고 Work153,172,444→144,896,196B다. 직전 맵 배치 설치 거절을 지나 Catalog536815680+62356B에서 거절됐다. Etcd 완료 inventory diff944→945, Storage 디코딩536870602+4644B에서 거절됐다. FULL ACK0·검색/재오픈 NOT_RUN, Off 부하82.81로 본문0, 두 Record trace 정상. Text/TextSpool364건 및 같은 R11 daemon의 표준 Large를 VERIFIED했다.

**R12 고정 후보 원 SDK Record FAILED.** `/private/tmp/qidx-sdk60-resident-packets.VuJCaR`는 연속 cold term window에서 검증된 posting packet을 재사용한다. Caddy의 동일509개 posting 적용에서 reads351,711→350,769, Work144,896,196→91,570,976B. Caddy는 Storage child path의 read1,000,000+1에서, etcd는 diff945개 뒤 posting sort의536836313+138583B에서 거절됐다. 두 FULL ACK0·검색/재오픈 NOT_RUN, Off 부하36.37로 본문0. Text/TextSpool364건 VERIFIED. SDK 중 외부 통합 테스트 소스 한 개가 바뀌었고 두 release 의존성과 실행 바이너리는 동일했다.

## 2. NTX0 — 현재 패치가 포함된 기준선

- F의 기존 release/SDK 실행 큐에서 관련 source와 원 입력을 결속해 후보를 만든다. 새로운 빌드 큐·worktree·서명 설치 후보를 이 패킷에서 만들지 않는다.
- ordered packet은 inventory diff의 `PostingEdits::flush → PostingRuns::append → flush_pending`에서도 실행된다. etcd의 최종 posting map 단계 미진입을 이유로 이 패치의 영향을 배제하지 않는다.
- 같은 Caddy 590파일·etcd 1,421파일에서 게시·검색·재오픈·source digest와 최초 거절을 확인한다. 실패하면 실패 prefix·완료 작업량을 그대로 기록한다.
- Record는 원인 진단, quiet Off는 성능 수용에 사용한다. Off 미입장은 BLOCKED/NOT_RUN으로 구분하고 contended Record의 시간으로 60초 통과를 만들지 않는다.
- **인계:** candidate source/binary·입력·명령·관측 결과·정확한 실패 단계. 집중 회귀나 Large 성공을 SDK60 성공으로 복사하지 않는다.

## 3. NTX1 — 기존 batch 통합에서 스트리밍 설치기로

이 절의 source-ready SHA·재실행 대기 문구는 NTX1a/계측의 단계별 이력이다. 현재 실행권·후보·Small 진행 상태는 §1과 §7의 현재 표를 따른다. 이력의 이전 PASS를 새 후보의 수용으로 재사용하지 않는다.

**변경 함수:** [검증된 cold draft 경계](../../../../crates/quanta-index-lexical/src/native/text/entry_patch.rs#L962), [dense 설치기](../../../../crates/quanta-index-lexical/src/native/text/dense_posting/write.rs#L469), [wrapper·Top caller](../../../../crates/quanta-index-lexical/src/native/text.rs#L12053). 기존 Text writer가 함수 hunk를 맡고 같은 parent hunk는 한 writer가 합친다.

### NTX1a — 현재 작업 창 전체의 leaf+directory 통합

2026-11-15 F의 직접 인수 지시에서 이 채팅의 최초 production 수정은 `dense_posting::write::install`의 현재 작업 창으로 한정됐다. `entry_patch.rs`/`text.rs`의 추가 parent 수정은 필요성이 확인된 정확한 hunk만 원 writer에게 인수한다.

1. source EOF/digest 검증을 끝낸 현재 창의 새 leaf ref를 `identify`로 먼저 구하고 그 ref로 directory를 구성한다. 재사용 leaf를 포함한 child 인증은 기존 계약을 사용한다.
2. 창 전체의 새 leaf+directory 합집합이 256객체·aggregate wire 64MiB 및 payload/children/backend 준비/출력 slots의 동시 Original 승인을 만족하면 기존 `StagedObjectBatch`에 leaf들→directory들 순서로 넣어 한 번 설치한다. term마다 별도 batch를 만드는 방식은 기본안으로 사용하지 않는다.
3. 창이 적합하지 않으면 첫 효과 전 판단으로 기존 공동 leaf batch→directory batch 경로를 유지한다. 미승인 자체를 terminal memory denial로 기록하지 않는다. 이미 준비한 ref/payload를 재사용하고, 실제 재계산이 있으면 그대로 차지한다. 설치 시작 후 실패는 fallback이나 size 변경 없이 반환한다.
4. 실제 Store+Catalog+복수 Draft caller에서 두 batch COMMIT 4→2, 동일 root/rank/child edge·GC·reopen, 늦은 parent 실패와 자원 거절을 먼저 RED/GREEN으로 검증한다. 표준 SDK·원 예산은 그대로다.

이는 최종 스트리밍 구조의 첫 적용 단위다. 전체 스트리밍 queue·wrapper 통합·prepared API 확장을 이 첫 hunk에 함께 넣지 않는다.

### NTX1b — 최종 생산·소비 계약

1. 첫 object 설치 효과 전에 해당 페이지 원본의 CAS·순서·행 수·EOF와 prepass epoch를 모두 검증한다. 검증 중에는 payload/draft 준비만 한다.
2. 검증된 Draft를 root별로 소비한다. exact payload+children으로 예상 ref를 식별하고 leaf → directory → wrapper의 준비된 순서로 queue에 넣는다. 원시 Draft backing은 마지막 사용 후 해제한다.
3. parent의 child는 같은 packet의 앞쪽 객체, 이전 packet에서 확정된 객체 또는 인증된 기존 객체여야 한다. 예상 ref는 DATA이며 설치·게시 권한이 아니다.
4. queue의 실제 객체 수 ≤256, aggregate wire ≤64MiB를 지킨다. 현재 Draft rows + queue payload/children + backend wire/registration/shard 준비 + 출력 slots의 합산 peak를 Original에서 승인한다. `put_batch_memory_bound`만으로 caller staging까지 승인됐다고 간주하지 않는다.
5. 다음 객체의 encode/append 전에 한도를 판단한다. queue가 있으면 검증된 child-first prefix를 설치하고 계속한다. 단일 객체와 필수 backend 준비도 승인할 수 없으면 ResourceLimited다. 효과 없는 사전 판단에서만 축소할 수 있다. 설치 시작 뒤 거절·I/O 오류를 받아 크기를 줄여 재시도하지 않는다.
6. 각 `put_batch` 반환의 길이·ordinal·객체 ref를 예상값과 대조한다. 모든 필요한 설치 확인이 끝난 뒤 기존 Top 편집과 최종 root 공개를 완료한다. 일부 객체가 provisional로 설치돼도 실패한 attempt의 published root는 바뀌지 않는다.

### NTX1a 독립 RED와 DoD

- NTX1a는 실제 Store+Catalog+복수 Draft caller fixture에서 한 packet에 들어가는 leaf+directory 입력을 고정한다. 기존 두 batch의 COMMIT 4회를 2회로 줄이고 같은 객체/root identity·조회·reopen·child edge/GC를 확인한다. 이 수치를 모든 페이지의 보장치로 확대하지 않는다.
- 256객체·encoded wire 경계, backend credit 미승인, 재사용 leaf와 복수 root에서 효과 전 admission/fallback을 확인한다. term마다 무조건 flush해 기존 공동 배치보다 transaction을 늘리는 구현은 채택하지 않는다.
- same-packet earlier child와 인증된 기존 child, foreign/missing child, late parent 거절·취소·short-write·sync/confirm 실패, 설치 receipt ordinal/ref 변조에서 published Top root 미변경·기존 view 조회·fresh retry/reopen 정산을 확인한다. 기존 cold 페이지의 stale epoch/late CAS/count/EOF 오류 회귀도 재사용한다.
- 객체 수·payload·등록 의미와 논리 Work 요금을 보존한다. 실제 제거한 호출·복사·검증과 Work 차이를 대응시킨다. 최종 Caddy SDK60 및 NTX4 통과 전에는 제품 성능 개선 완료로 닫지 않는다.

실제 Store+Catalog fixture는 Kernel의 [native_text_services](../../../../crates/quanta-index-kernel/tests/native_lifecycle.rs#L7403)와 COMMIT/custody 관측 회귀를 재사용한다. 제품 query/view/reopen은 Runtime의 [기존 Native Text SDK/UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds.rs#L2609)를 소비한다. mock authority의 호출 수를 실제 SQLite COMMIT 수로 보고하지 않는다.

### NTX1b 추가 DoD

- 256개를 넘는 입력, 실제 wire/Original 한계에서 prefix flush하고 same-packet earlier child와 previous-confirmed-packet child를 모두 검증한다. wrapper는 같은 queue에서 해당 child ref가 준비된 뒤에만 추가한다.
- 두 번째 packet 또는 후속 directory/wrapper의 reserve·write·sync/barrier·confirm 실패에서 published Top root 미변경과 attempt retry/reopen 정산을 확인한다. 페이지 전체의 late source 오류는 첫 설치 전에 거절한다. scratch Top 편집과 공개된 root를 구분한다.
- NTX1a의 4→2와 NTX1b의 다중 packet 완료를 별개로 기록한다. 첫 fixture 통과만으로 전체 streaming scope를 닫지 않는다.

**범위 제한:** 기존 [공개 child-first batch](../../../../crates/quanta-index-core/src/index/ports.rs#L193)와 [같은 배치 child 인증](../../../../crates/quanta-index-catalog/src/native/authority/managed_install.rs#L773)을 사용한다. Core/Storage/Catalog 계약 변경이 정말 필요하면 정확한 결손을 별도 인수한다. 전체 DAG 사전 보유, 페이지 간 무제한 보류, 숨은 재계산이나 두 번째 설치 IR은 기본안이 아니다.

## 4. NTX2A/NTX2B — 누적 Work와 용어 중심 생산자

**변경 함수:** [prepare_inventory](../../../../crates/quanta-index-lexical/src/native/text.rs#L10190), [ColdTokens](../../../../crates/quanta-index-lexical/src/native/text/cold_tokens.rs#L337), [기존 grouped output 소비자](../../../../crates/quanta-index-lexical/src/native/text/occurrence_pack.rs#L211). 기존 Text 생산자 writer가 맡고 NTX1과 `text.rs`의 서로 다른 함수 hunk를 구분한다.

### NTX2A — 제거할 실제 작업 확정

- 기존 gated 계측 안에서 census/normalize, fill/sort, block shape/pack, authenticated alias walk, posting push/flush의 배타적 Work와 token/term/position/출력 수를 기록한다. 새 검증 순회나 요금 감면을 계측에 넣지 않는다.
- Off/Record의 출력·원 Work·거절 동일성과 malformed/partial capture 거절을 먼저 확인한다. 상위 Text·prepare와 하위 lane을 중복 합산하지 않는다.
- 원본 전체 분포와 실패 prefix를 구분한다. 632개 owner 완료를 1,421파일로 선형 외삽하지 않는다. 증명 가능한 필수 하한과 추정치를 구분한다.
- `P`는 sensitive/folded를 포함한 case occurrence 수, `U`는 고유 canonical term 수다. `Kall`은 occurrence key bytes 합, `Kuniq`는 고유 key bytes 합이다. 현재 arena의 핵심 backing은 `Kall + 12P`다. caller/출력/SQL memory는 별도 peak에 포함한다.
- 같은 cohort의 owner별 `P/U/Kall/Kuniq`, one-block borrowed alias / multi-block direct / SQL fallback 분기, Original live peak와 spill 횟수를 함께 계측한다. aggregate prepare Work만으로 이 분포를 복원했다고 하지 않는다.
- F가 인계한 기존 제한 분석은 packet term dictionary의 한 순회 raw key 절감 최대 약 2.05MB와 실패 prefix 원 edit charge 41.74MB 이상이다. 이는 다른 후보/cohort의 packet 표현 분석이며 owner-local token pool의 절감량과 혼동하지 않는다. 이번 배타적 계측에서 관련 source·단위를 대조하고 필수 logical edit 요금은 유지한다.

### NTX2A typed counter 인수 계약

Root가 Core `DiagnosticCounterV1`의 아래 key 추가를 소유한다. Text는 집계·분기 판정·emit을 소유한다. 단위는 모두 `u64`이며 성공한 cold owner의 `NativeTextInventoryPrepare` 완료 span에 한 번 기록한다. warm owner는 제외한다.

| exact key | 의미 |
| --- | --- |
| `TextCaseOccurrences` | P: indexed lexical run의 Sensitive/Folded 각각을 세는 occurrence 수 |
| `TextUniqueCanonicalTerms` | U: case prefix를 포함한 canonical stored key의 owner 내 고유 개수 |
| `TextOccurrenceKeyBytes` | Kall: P개 encoded case-prefixed UTF-8 key 길이의 합 |
| `TextUniqueKeyBytes` | Kuniq: 각 고유 stored key 길이를 한 번 합한 값 |
| `TextColdAliasOwners` | 실제 완료 출력이 one-block borrowed alias인 owner 수 |
| `TextColdDirectOwners` | 실제 완료 출력이 arena ordered/multi-block direct인 owner 수 |
| `TextColdSqlOwners` | 실제 완료에 token SQL spool/group replay를 사용한 owner 수; arena 이후 SQL 재생도 이 분기 |
| `TextColdEmptyOwners` | indexed occurrence가 없어 출력이 빈 cold owner 수 |
| `TextShapeCompleteOwners` | 네 shape 값과 실제 분기가 모두 정확하게 관측·완료된 owner 수 |

Unicode 정규화/case folding으로 합쳐지는 중복은 P/Kall에 모두 포함하고 U/Kuniq에서 합친다. Sensitive/Folded는 prefix가 달라 별도 key다. 분기 네 개는 owner별 상호 배타적이다. 네 shape와 분기를 모두 확보하고 `prepare_inventory`가 성공했을 때만 completeness=1로 emit한다. 실패·초기 census 중단·미관측 값은 0으로 만들지 않고 tuple/completeness를 생략한다. aggregate는 completeness=1인 owner만 합산하며 분기 합과 complete 수를 대조한다. 이전 attempt·이전/현재 owner를 섞지 않는다.

Work 계측은 lane 34/35를 inclusive로 유지하고 42 canonical input/normalize, 43 census, 44 fill, 45 sort/validate, 46 block prepare/shape/pack/SQL replay, 47 token SQL spool/warm으로 분리한다. lane 34에서 child 42/43/44/45/47을 뺀 값은 admission residual이다. lane 32 inventory alias visit에서 내부 lane 33 posting flush의 같은 Work 축을 빼 배타적 walk를 계산한다. per-term span을 추가하지 않는다. Text-local grant 통계는 전체 Original live peak가 아니며 새 peak getter를 필수 API로 요구하지 않는다.

### NTX2B — 같은 타입 내부 생산자 대체

1. canonical term을 한 번 저장하는 key arena·term index와 term별 bounded position stream을 사용한다. 위치는 입력 순서로 직접 추가하고 고유 term만 canonical byte순으로 정렬한다. 전체 `{term_id,position}` 행을 다시 정렬하는 두 번째 IR을 만들지 않는다.
2. key arena·term slots/index·position nodes·정렬/emit buffer·SQL packet/string은 첫 run 전에 승인한다. 필수 occurrence 출력은 grouped shape를 확인한 뒤 첫 object/edit 효과 전에 기존 `cold_output_credit_bound` 경계에서 승인한다. 두 단계에서 동시에 살아 있는 grant를 합산한다. term마다 무제한 Vec를 만들지 않는다. hash 충돌은 전체 키 비교로 확인하고 hash iteration order를 출력에 사용하지 않는다.
3. position pool은 연결 정보와 metadata를 포함한다. `Kuniq + 4P`를 실제 메모리 상한으로 주장하지 않는다. `U≈P`인 all-unique 입력에서 slot/hash/node 비용이 기존 12P Row보다 커질 수 있다.
4. 실제 capacity가 차면 SQL 입장을 먼저 확인하고 `spool.reset_tokens`로 이전 owner 행을 비운다. 이미 집계한 모든 `(term,pos)`를 정확히 한 번 배출한 뒤 source remainder를 SQL에만 보내는 mode latch를 유지한다. 기존 `tokens(term,pos)` PK와 `ORDER BY term,pos`로 그룹을 완성한다. 첫 reset/insert 효과 뒤 원본 재생·생산자 재시작·경로 역전환은 금지한다. 중간 실패는 terminal/poison으로 처리한다.
5. canonical normalizer·case 생성·position·기존 wire·alias·seal·CAS·EOF/attempt 완료 계약을 유지한다. 기존 단일-block borrowed alias·occurrence encoder·SQL grouping·posting run merge를 재작성하지 않는다.
6. census는 fixed-grant 입장과 모든 크기/위치 검증이 대체될 때만 실제 순회와 함께 제거한다. 유지된 NFC/case encode·hash/비교·node write·spill·merge는 실재 작업대로 차지한다. 메모리 해제나 단순 interning으로 누적 Work를 환급하지 않는다.

### 독립 RED·채택 조건

- 기존 frozen gold와 canonical `(case,key,position)` stream을 비교한다. NFC·Unicode case 확장/충돌·긴 unindexable run의 위치 소비·u32 및 term별 position 경계·중복 위치 거절을 포함한다.
- 반복 용어, all-unique 식별자, chunk 경계에 걸친 같은 term, 다중 chunk, 최종 EOF, hash seed/삽입 배치 경계 변경에서 같은 canonical stream을 만든다. 동일 block/alias 조건에서는 기존 wire와 identity를 비교한다.
- 최초 메모리 미승인, spill/SQL 실패, 취소·후속 owner 변조에서 부분 root 공개 0과 fresh retry 동등성을 확인한다. encoded DATA의 실제 소유권과 persisted alias의 독립 readback을 유지한다.
- NTX2A와 같은 입력에서 key copy·정렬·hash/node·SQL 비용을 포함한 **net Work·peak memory·시간**을 비교한다. RAM 절감만으로 512MiB 통과를 주장하지 않는다. 비용 역전이나 원 게이트 미달이면 후보 실패로 기록하고 채택하지 않는다.
- 다중-block borrowed view는 별도 후속 후보다. NTX2B와 동시에 변경해 개선량을 섞지 않는다. 필수 하한이 전체 Work를 넘는 것으로 입증되면 해당 생산 경계의 별도 재설계를 요청하고 임의 cutoff·요금 삭제를 반복하지 않는다.

## 5. NTX3 — 여러 packet의 prepared run은 후속 작업

NTX1의 실제 SDK 결과에서 packet 간 confirm/reserve가 남은 병목으로 확인될 때만 연다. [내부 다중 그룹 run](../../../../crates/quanta-index-storage/src/object.rs#L609)과 원자적 confirm+next-reserve를 재사용한다. 현재 [공개 prepared 계약](../../../../crates/quanta-index-core/src/index/object_install.rs)은 childless 전용이므로 child-bearing 지원은 별도 typed 계약·writer 인수가 필요한 실제 작업이다.

기본 NTX1에 새 공개 port를 강제하지 않는다. prepared-run의 COMMIT 4→3과 NTX1의 단일 packet fixture 4→2는 다른 실험이며, 어느 것도 전체 SDK 개선량이 아니다. 여러 group의 동시 Original 준비가 승인되지 않으면 적용하지 않는다. 파일 I/O를 SQL transaction 안으로 옮기지 않는다.

## 6. 실행 순서·단일 작성자·인계

| 순서 | 독립 진행과 합류 |
| --- | --- |
| 0 | F가 NTX0 source/binary·입력 기준선을 고정한다. Text는 NTX1a의 oracle/소스 검토와 NTX2A 계측 설계를 독립 진행할 수 있다. |
| 1 | NTX1a 현재 창 batch 통합과 NTX2A 생산자 계측은 서로 다른 함수 hunk에서 병렬 진행한다. 같은 `text.rs` parent/hunk는 기존 원 writer 한 명이 합친다. |
| 2 | NTX1a의 실제 behavioral RED→구현→owning 회귀는 재사용한다. NTX2B를 채택하면 NTX2A의 입장/비용 근거→gold/adverse RED→생산자 대체→owning 회귀로 진행한다. NTX1b 확대는 최초 결과와 후속 hunk 인수 뒤다. 미실행 selector를 RED/GREEN으로 부르지 않는다. |
| 3 | 각 source-ready마다 F가 변경된 범위만 기존 warm lane/실행권에서 검증하고 같은 후보 SDK에 인수한다. 두 변경의 효과를 구분할 기준선을 보존한다. |
| 4 | 채택한 구현과 기능 오너의 결과를 같은 최종 후보에 합쳐 NTX4를 실행한다. NTX3는 선행 근거가 생긴 경우만 별도 인수한다. |

새 채팅·writer·검증 큐를 만들지 않는다. 기존 작성권을 파일 전체로 확대하지 않으며, 착수 시 HEAD·dirty·활성 hunk와 source-ready 결속을 다시 대조한다. 인계에는 함수/타입·생산자/호출자/소비자·관련 source SHA·정확한 selector·명령/결과·scope/남은 입력을 기록한다. F는 공용 조립·원장·최종 후보와 실행 큐를 담당한다.

**현재 인수:** 이 기존 Text 채팅이 NTX1a의 `dense_posting::write::install`과 전용 회귀를 단독 작성한다. etcd는 실제 비용 계측 후 필요한 `ColdTokens` hunk를 이어서 인수한다. D의 명시 반환 후 Root가 기존 `test-workspace-lane`/`cargow`/mutex의 실행권을 Text에 loan했다. jobs2·resource wait10을 유지하며 Root/D Rust0·Text executor1로 지정 owning→release→원SDK를 직렬 실행한다. 새 lane/controller/SDK runner 구현은 없다. 테스트·source-ready·실제 selector와 결과는 아래 표에서 인수한다.

### proof_requests / proof_results

| 요청 | 실제 source/selector | 상태 | 결과 인수 |
| --- | --- | --- | --- |
| NTX1a-R1 | 기존 `dense_install_batches::native_text_dense_window_leaf_directory_share_two_catalog_commits_and_reopen` — Kernel `native_lifecycle` | RED VERIFIED — run `497063b3`, 4 != 2; 아래 fallback 회귀로 분리 | F canonical Rust 큐 |
| NTX1a-F1 | `dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens` — Kernel `native_lifecycle` | VERIFIED — run `9cd669fc`, actual PASS0.670s; source `a0443fd5` | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX1a-G1 | `native::dense_install_tests::dense_text_real_original_credit_coalesces_and_reopens` — Runtime lib/private fixture | VERIFIED — run `8ee85ee8`, actual PASS 2.420s; 2 vs 4 COMMIT·artifact/TF/positions/reopen | Root가 인수한 D 단일 실행권 |
| NTX1a-F2 | `native::dense_install_tests::dense_text_post_confirm_error_preserves_current_and_resumes_original_operation` | VERIFIED — 같은 run actual PASS 1.380s; Unknown·old current·resume 원 작업·9owner/reopen | Root가 인수한 D 단일 실행권 |
| NTX1a-A1 | `native::dense_install_tests::dense_text_real_original_credit_miss_falls_back_without_denial` | VERIFIED — 같은 run actual PASS 2.222s; 실제 8MiB credit miss·fallback·first denial 없음 | Root가 인수한 D 단일 실행권 |
| NTX1a-B1 | Kernel `physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically` + Storage `object::tests::batch_barrier_failure_leaves_reserved_objects_unconfirmed` / `object::managed_trace_tests::managed_reserve_confirm_trace_preserves_off_cost_and_closes_error` | VERIFIED — Storage run `a72d53a5` actual2/2PASS·208skip·exit0·0.437s; Kernel run `9cd669fc-d775-41bc-8ff1-4a2951e390e5` actual2/2PASS·83skip·exit0·0.853s includes F1 | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX1a-R2 | `native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold` — Runtime `native_index_uds` 기존 rank/view/reopen oracle | VERIFIED — run `6741d8d5-3b8d-4c9f-afe6-7b903144a703`, actual1/1PASS·74skip·exit0·3.067s; compile1m19s | Root가 명시 loan한 기존 단일 Q 큐 |
| NTX2A-R1 | `native::text::tests::inventory_trace_tests::inventory_trace_stages_preserve_work_and_close_under_lexical_parent` | FAILED — `01cb221f` 1 body/FAIL 1; cold 미선택 oracle 오류로 제품 RED 판정 철회 | F canonical Rust 큐 |
| NTX2A-WG | `native::text::tests::inventory_trace_tests::` — existing stage + 실제 cold success + first denial 동등성 3건 | VERIFIED — nextest `54c7b404-8a46-4cae-9818-99148f0e6873`, 실제 Work 3 PASS | F canonical Rust 큐 |
| NTX2A-SR | `native::text::tests::inventory_shape_tests::` — SQL case-key gold + empty 2건 | RED VERIFIED — 같은 run에서 shape 2 FAIL; empty None != Some(0), SQL None != Some(6) | F canonical Rust 큐 |
| NTX2A-G7 | `inventory_trace_tests::` 3 + `inventory_shape_tests::` 4 | VERIFIED — nextest `0a7b720b-190a-441c-a3b1-bbefd2e7032c`, 7/7 PASS·529 skip·exit0·0.126s; shape test `75ac4cae` | F canonical Rust 큐 |
| NTX2A-PY | diagnostic/timeline 소비자 2파일·회귀 2파일 | VERIFIED — locked uv pytest 621 PASS, Ruff/diff-check PASS | Text 독립 Python 실행 |
| NTX-owning | `native::text::` / `native::text_spool::` / `native::literal` — Lexical lib no-default-features | VERIFIED — run `69eabb3a-471a-41b1-9930-6ce3e75d0aa0`, actual367/367PASS·169skip·exit0·14.532s; compile38.09s | Root가 명시 loan한 기존 단일 Q 큐 |

요청 표의 자리표시자는 실행 가능한 selector가 아니다. 본문이 없는 요청을 제출·통과 처리하지 않는다. 실행 결과에는 명령·source·본문 수·실제 실패/통과와 source-ready 후 변경 여부를 기록한다.

**NTX1a-R1 최초 RED source (이력):** [새 회귀](../../../../crates/quanta-index-kernel/tests/native_lifecycle/dense_install_batches.rs)의 SHA-256은 `1bc2aaf7c5227dcf482572444b26c6a863d9ee4b6e49b2fd30b880deb64c4748`다. Kernel `native_lifecycle.rs`는 전용 module mount, `UpdateCostSpyState`의 관측 sample, 실제 `install_batch` 전후 opt-in SQL profile hook만 추가했다. 세 term×9 owners의 dense 복수 draft에서 실제 SQLite COMMIT 합 2회·한 packet을 요구하고 양 case의 owner/TF/positions·동일 root의 reopen을 독립 고정값으로 검증한다. 첫 Root 실행은 컴파일 중 exit 137로 끝났으며 본문 0건으로 행동 RED가 아니다. Root 재실행 nextest `497063b3-0ce8-4439-bc2b-12e534158803`은 본문 1/FAIL 1/exit 100, 84 skip이었다. 관측값은 leaf 6개 batch COMMIT 2 + directory 6개 batch COMMIT 2 = 4, 기대값 2로 실패했다. 앞선 owner/TF/position과 reopen 단언은 통과했다. 이 행동 RED 뒤 production 구현을 시작했다.

**fixture 보완:** 위 Kernel fixture의 `open_managed_with_scratch`와 미연결 `UpdateCostSpy` bound는 `None`이며 WorkBudget도 scoped Original credit이 없다. strict admission 경로의 GREEN 증명으로 쓸 수 없다. 해당 fixture는 미승인 fallback의 4 COMMIT 회귀로 명시적으로 분리하고, 별도 Runtime private fixture에서 production `open_managed_with_io` + 실행 reservation의 Original credit을 재사용해 2 COMMIT을 검증한다. 가짜 backend bound나 승인 우회로 기대값을 맞추지 않는다. 첫 통합 구현은 prior 없는 cold whole window만 선택하며 warm/local은 기존 공동 leaf batch 경로를 유지한다.

**현재 production/source-ready:** `dense_posting/write.rs` SHA `13045e24358df58c6a20c1d963b2f26737e921db214ce827b4e5ea50740dca47`, `wire.rs` SHA `9c27d9b3ce28e5523917a5fff55cda6ac230f971834f0ec885fede222fd30278`. 공통 batch backing은 Original parent에서 먼저 승인해 양 경로에서 유지하고, cold whole window의 payload/children/임시 fence·ref/identity·backend 준비를 원 scope credit으로 합산 승인한다. 모든 설치 receipt 검증 뒤 draft ref/output을 확정한다. 정적 적대 검토·rustfmt·diff-check와 아래 Runtime 3건이 VERIFIED다. 원본 SDK 수용은 NOT_RUN이다.

**NTX1a-G1:** [실제 Runtime private fixture](../../../../crates/quanta-index-searchd-runtime/src/native/dense_install_tests.rs)는 `ManagedObjectIo`/`PublicationFileCapacity`/`NativeResourceLedger`의 실제 Core reservation을 쓴다. 요청당 64MiB IO·memory scope에서 실제 Catalog SQL profiler로 fast 1batch/2COMMIT 대 bound-declined fallback 2batch/4COMMIT, fixed owner/TF/positions·reopen·동일 artifact를 검증했다. run `8ee85ee8` actual PASS이며 SDK60 제품 수용과 별개다.

**Runtime 후속 입장·실패 회귀:** 현재 private fixture SHA는 `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`다. 실제 combined install의 confirm/2 COMMIT 뒤 one-shot Storage 오류를 주입해 Kernel의 `Unknown`, dense 1call·공개 current 불변·빈 조회·reopen을 확인하고 canonical `resume(op2)`로 원 immutable request의 full publish·9owner membership·다시 reopen을 검증했다. 별도 8MiB Core Original 상한에서는 그대로 forward한 backend bound가 16MiB 이상이므로 optional coalesced credit이 거절된다. 원래 mandatory 경로가 2batch/4COMMIT으로 성공하고 first denial 없이 64MiB fast와 artifact·조회·reopen이 같다. 세 본문은 run `8ee85ee8` VERIFIED다. 이전 fresh-op3 fixture 실패 원인은 아래 이력에 보존한다.

**NTX2A Work source-ready:** `text.rs` shared SHA `2d3b5a90f142d2962db7de4c381fa3a98fef59d36e1bc4b4167e2217a027c24a`, `tests/inventory_trace_tests.rs` SHA `e6b39c6314b5298537885807ff1687ce89442b80878e36bf56d39754d146bcbd`. 기존 `inventory_trace_stages_preserve_work_and_close_under_lexical_parent`와 신규 `inventory_trace_work_refusal_preserves_first_denied_charge`를 Root GREEN 요청했다. 후자는 Off/Record의 첫 Bytes 거절·accepted Work·used/attempted/limit/caller를 비교한다. formatter·diff-check 이후 Root 본문은 2건 모두 실패했다. nextest `04bbd234-4bb6-41b9-ae7b-6a9b3a16a0e0`에서 stage test는 census 0 != 1, refusal test는 lane43 시작 없음으로 실패했다. helper `process_change`는 `pack_owners=false`여서 census 경로를 선택하지 않는다. 이 oracle 오류 때문에 최초 `01cb221f` 실패를 생산 코드 결함의 RED로 확정한 판정을 철회한다. 기존 noncold oracle를 복원하고 실제 `prepare_inventory(pack_owners=true, empty prior, MemoryScope)` helper를 추가했다. 현재 test SHA `10f4445ba64ffddf5327123044a2b7fb91208bdf128230b91eadcc5782ae2c05`의 새 `inventory_cold_prepare_work_lanes_preserve_off_record`와 원 stage·거절 회귀까지 3건을 Root에 재요청했다. census 순회 추가 없이 test 경계만 수정했고 재실행 본문 NOT_RUN이다.

**NTX2A shape test-only (RED 이력):** [고정 SQL/empty gold](../../../../crates/quanta-index-lexical/src/native/text/tests/inventory_shape_tests.rs)는 실제 `prepare_inventory` cold 분기에서 `Alpha alpha β`의 P6/U5/Kall30/Kuniq24·SQL1/complete1과 빈 owner의 정확한 0·empty1/complete1을 요구한다. Off/Record 결과·Work를 비교하며 RED 실행 전에는 Text emit이 없었다. `text.rs`에는 module mount만 추가해 당시 shared SHA는 `21ecc09f270db2ed6601fca9fa8f6931d576819aedb1ce479c5525c9253c2c4e`였다. 이전 Work source-ready SHA와 구분한다. Root에 실제 RED를 요청했고, 그 결과 전 emit production은 수정하지 않는다.

**재실행 전 compiler 수리:** Work3+shape2의 Root 재검증은 test의 `BTreeMap::insert` 반환값 미사용 5개로 compiler exit101·본문0이었다. 행동 RED/GREEN으로 세지 않는다. 삽입마다 중복 없음 단언으로 반환을 소비하고 formatter/diff-check를 통과했다. 현재 Work test SHA는 `54ead1d1d996ae6f7941f24aa5432a765f6dbf297450da3dc6f7b32dffdd4ab9`이며 같은 5scope를 다시 요청했다. Runtime positive는 Root의 Kernel typed History dispatch 접합이 끝나기 전 NOT_RUN이다.

**실제 재실행 결과:** nextest `54c7b404-8a46-4cae-9818-99148f0e6873`은 5 body / 3 PASS / 2 FAIL / 529 skip / exit100, compile19.61s·본문0.086s였다. 수정한 실제 cold Work 3건은 모두 통과했다. shape 2건은 emit 미구현의 counter 부재로 실패했으며 이것을 유효한 구현 전 RED로 인수했다. 이 결과 뒤에만 emit production을 수정한다. borrowed one-block Alias, admitted ordered/multi-block Direct, SQL은 성공한 실제 return site에서 구분한다. 출력의 `alias: Some`만으로 생산 경로를 추론하지 않는다.

**emit production/source-ready:** 기존 borrowed alias seal loop의 O(1) `vector_len`과 기존 grouped consume에서 P/U/Kall/Kuniq를 집계한다. 추가 토큰/행 순회나 Work 요금 변경은 없다. 성공한 실제 return site의 Alias/Direct/SQL/Empty를 붙이고 성공한 prepare 뒤 lane48에서 네 값·선택 분기 1·complete 1만 발행한다. tentative alias 실패 시 관측값을 버리고 실제 fallback 값으로 완성한다. overflow/trace 오류는 제품 결과를 바꾸지 않는다. `prepare_owner_blocks` 한 entrypoint에 observer 인자를 추가하고 생산 caller·기존 네 test caller를 함께 변경했다. 현재 SHA는 `text.rs` `ee2c6944ff0c50b657011373e688512089880cbc83cdc03bc126a689b15161e6`, `occurrence_pack.rs` `fb2eba49883cc34f31331634f87ccab557d16ee7c8b34b8db39d808562769634`다. formatter/diff-check 통과, 최초 5scope 뒤 분기 회귀 2개를 더한 현재 7scope는 아래 Root 실제 실행에서 모두 통과했다.

**추가 분기 회귀 source-ready:** 실제 SQL/empty gold 외 `cold_borrowed_alias_shape_matches_fixed_group_stream`와 `cold_sql_inline_alias_reports_sql_branch_and_fixed_group_stream`를 추가했다. persisted 0/1 prefix·UTF-8의 literal 11-key/position gold와 실제 `CheckedBlock` 출력을 대조하고 P12/U11/Kall70/Kuniq64·Off/Record Work·SQL table 유무를 검증한다. SQL은 기존 canonical arena·alias metadata·first key·encoded output minus one 공식으로 선택하며 threshold 탐색을 하지 않는다. Direct가 Directory alias를 반환하는 경우는 실제 backend bound·Original credit을 제공하는 Runtime fixture/SDK Record로 확인한다. 기존 `install_envelope.rs`의 514-key·credit1 fixture는 backend bound가 없어 SQL replay를 확인하는 회귀다. 이것을 Direct 양성으로 승격하거나 fake zero-bound provider로 제품 입장 증거를 만들지 않는다. Alias·SQL·Empty 분기는 Root 7scope에서 통과했다. Direct 분기별 계측 실행은 아직 NOT_RUN이다.

**계측 소비자 인수·VERIFIED:** Root가 인계한 Text 전용 consumer hunk에서 raw lane48/성공 stage/v5/부분 four-tuple/분기 합/completeness/u64 overflow·완료된 empty의 0·비어 있지 않은 owner의 0U 모순을 거절한다. aggregate/cached group도 검증한다. 완료된 v5 lane48에는 tuple이 반드시 있어야 하고 complete 수는 성공한 owner-metric span 수와 같아야 한다. P의 짝수·비어 있지 않은 N개 owner의 P/U 최소 2N·Kall 최소 2P·Kuniq 최소 2U를 canonical 두 case 생성 계약에 대조한다. N은 Record span 수이며 Source 파일 수의 인증으로 승격하지 않는다. `causal_cost_timeline.py`는 Root의 명시 9-key set에 대해 같은 span의 raw duplicate를 거절하며 기존 generic Work 누적과 occurrence-diff 동작을 유지한다. 실제 TDD 실패를 관측한 뒤 `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_causal_cost_diagnostic.py tools/ci/tests/test_causal_cost_timeline.py`를 실행해 **621 passed / 5.80s / exit 0**를 확인했다. 4파일 Ruff check·diff-check는 통과했다. 전체 format-check에는 작업 밖 기존 dirty hunk 차이가 남아 해당 hunk를 변경하지 않았다. 이 결과는 parser 회귀이며 실제 SDK 분포·게시·성능 증명이 아니다. 전체 Original peak API도 새 필수 입력으로 요구하지 않는다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-kernel --test native_lifecycle --no-tests fail \
  -E 'test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens)' \
  --test-threads 1 --failure-output immediate --success-output final
```

공통 영향 회귀의 기존 명령은 다음과 같다. 실제 실행은 F의 현재 canonical queue에서 변경 scope를 확인한 뒤 인수한다.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-lexical --lib --no-default-features --no-tests fail \
  --no-fail-fast --status-level fail --final-status-level fail \
  -E 'test(native::text::) | test(native::text_spool::) | test(native::literal)' \
  --test-threads 1
```

새 fixture의 정확한 selector는 해당 테스트를 실제 추가한 writer가 source-ready에 적는다. 존재하지 않는 테스트 이름이나 컴파일 실패를 행동 실패로 발급하지 않는다.

## 7. NTX4 — 제품 수용과 종료 조건

- 같은 source/binary/입력의 Caddy·etcd 각각 quiet Off 2회+Record 1회에서 full ACK·검색·재오픈·source digest 일치를 확인한다. 파일 30,000·Work 536,870,912B·읽기/쓰기/변경 각 1,000,000·SDK I/O 및 submit→ACK 60초를 유지한다.
- Record는 두 epoch의 실제 publish/query/reopen과 완전성 검증을 통과해야 한다. drop/unwritten/open request·span, partial/malformed 결과를 성공으로 취급하지 않는다. quiet 사전 검사 실패는 성능 성공이나 제품 실패로 바꾸지 않는다.
- same-view 조회·이전 root·delta/delete/no-op·실패 후 retry, 표준 Large의 두 OS 재시작과 해당 영향 회귀를 검증한다. 설치기 변경은 객체/root identity, 생산자 변경은 canonical groups·wire·조회 의미를 각각 독립 대조한다.
- Work·RSS·scratch·물리 파일/I/O·COMMIT·xSync를 분리한다. 중첩된 stage/counter는 합산하지 않는다. 동등 기능의 baseline/candidate 비교와 최종 출하 기준은 기존 DOD를 그대로 적용한다.
- 모든 필수 범위에 명령·관측 결과·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE과 scope를 남긴다. 문서·컴파일·집중 회귀만으로 SDK60·전체 Native·SOTA 성능·설치 출하를 완료 처리하지 않는다.

### 현재 원 SDK 진단과 다음 수리 경계

후보 `/private/tmp/qidx-sdk60-ntx.zOObrM`은 daemon `761bc65cd3e9ad926592adb2208ca5143ab19f59a1569792546199a4c55ed972`, 기존 `native_corpus` 재빌드 `71d747aa25d650929753c5fe1ee6568cca8eff2908e8378c70968ce0b30b5783`이다. 빌드·Record 전후 2,785개 입력과 HEAD가 동일했다. 첫 Off는 host load60.330078>8에서 본문0으로 BLOCKED였다. 같은 원 caps의 Record는 Caddy46.27s·etcd19.15s에 각각 ResourceLimited, ACK0·query/reopen 미도달로 FAILED다. 두 raw trace는 schema5·complete/normal, drop/unwritten/open-span/producer-lease0·health[]·open request0이다. 이 실패 후보의 Off/Large를 반복하지 않는다.

| 원본 | 최초 거절 | 실제 완료 metric prefix |
| --- | --- | --- |
| Caddy590 | reads999984+23>1000000, managed_install.rs108; bytes479122433·W547659·M687065·memory denial 없음 | 589 valid tuples: P799870/U207909/Kall5081067/Kuniq1588399; Alias384/Direct204/SQL0/Empty1 |
| etcd1421 | bytes536870674+1461>536870912, object.rs1914; R480232·W452360·M626787·memory denial 없음 | 704 valid tuples: P1004394/U335766/Kall8120086/Kuniq4213880; Alias516/Direct188/SQL0/Empty0 |

Metric 수는 관측된 성공 cold owner이며 전체 파일 수의 수용 증명이 아니다. etcd lane32의 배타 Work는 실제 parent에 포함된 lane33만 뺀 **37604852B**다. global33을 빼 얻은30975194B는 폐기한다. Text297685874B 외에는 Literal186751136·RepoMap32020144·SourceApply15182451 등이 포함된다. 부모·자식은 중복 합산하지 않는다. SQL spill0과 key backing 차이3906206B만으로 NTX2B나 전체512MiB 통과를 채택·예측하지 않는다.

**Caddy 구현 — bounded shared Small2..8 posting block (Root exact loan 후 구현, 제품 수용 NOT_RUN):** wrapper 객체는 현재도 페이지 단위 `put_batch`와 paired/confirmed dedup을 사용한다. 추가 배치 연결이 아니라 term별 객체 수를 줄이는 새 persisted codec/reader 경계다. 기존 public object/map API와 `Posting`/`SmallMap` 메모리 모델은 재사용한다. `entry_map::block`의 bounded child-slot 검증은 참고하되 `(term,owner)` 결합 키는1024B 한도를 넘을 수 있으므로 wire를 그대로 쓰지 않는다.

- Writer는 기존 Text 원 writer 한 명. private `small_posting_block` codec와 `text.rs::apply_postings`의 wrapper 준비·Top 편집, `read_posting` 및 네 production caller를 exact hunk로 인수한다. Catalog/Storage/Core/공개 port·quota는 수정하지 않는다.
- MapValue의 기존32B DATA/5child 한도를 유지하고, 새 inline tag+slot+DF+child1이 shared object를 참조한다. block에는 각 slot의 **전체 canonical term**과 DF2..8·정렬된 owner32→occurrence child-slot을 저장한다. 모든 child ref·slot/order/EOF·DF와 요청 term 일치를 검증한다. 같은 DF인 foreign term/slot 바꿔치기도 거절해야 한다.
- 기존 bounded block의16KiB/512행 정책을 재사용할지 codec 설계에서 고정한다. 실제 encoded payload·child dictionary·staging/decoder backing을 Original에서 효과 전에 승인한다. flush와 format 선택은 고정 recipe/실제 wire shape로 결정하고64MiB/8MiB 가용 메모리에 따라 다른 artifact를 생성하지 않는다. 부족하면 typed refusal이며 효과 뒤 representation 변경·재시도는 없다.
- 새 wire는 artifact digest가 달라지는 실제 format 확장이다. old-reader 수용이나 mixed-version boot를 주장하지 않는다. 기존 persisted Object posting의 read/identity/edge 검증은 보존하고 새 write는 Small2..8의 한 canonical codec을 사용한다. 병행 내부 IR·v2 wrapper·새 object authority는 만들지 않는다.
- 실제 Original Runtime fixture에서 여러 term의 DF2/3/8·독립 owner/TF/positions·BM25 gold와 wrapper object 감소를 먼저 RED로 실행한다. codec 독립 bytes gold, max term1024B, boundary/foreign slot/DF/child·truncation, delta/delete 후 다른 term 생존·old view/reopen/GC, 첫 memory/Work 거절과 post-confirm 복구를 검증한다. 이후 owning·새 binary·원SDK를 다시 결속한다. 객체 감소를 현재 quota PASS로 예측하지 않는다.

**etcd 경계는 추가 확인:** 현재 owned-wire payload view는 없으므로 단순 연결 작업이 아니다. 또한 `FileObjectStore::read_retained`도 IO·payload copy 없이 read/wire/decode Work를 차지한다. 따라서 payload copy만 없애1914 fee를 삭제하는 것은 원 논리 Work 계약의 수리가 아니다. 기존 attempt-local reader에서 중복 **논리 get/decode 호출 전체**를 제거할 수 있는지, 또는 canonical 표현 크기·source 준비 흐름의 구조 변경이 필요한지 Root가 exact producer/consumer 범위로 인수한 뒤 결정한다. checksum·등록·독립 seal/CAS/count/EOF 검증과 PostingRuns의 원 logical edit fee는 유지한다.

### Shared Small 이전 후보 실행과 etcd 후속 결정

- Shared Small의 실제 구현 전 Runtime RED는 4 FAIL(`289bece4`): 원 DF/TF/positions/reopen 확인 뒤 객체 3→1 및 새 policy/refusal 조건에서 실패했다. 구현 뒤 cold Runtime4는 `1abc926d` 4 PASS다.
- 독립 codec4는 `f9a97cad` 4 PASS. wire gold·두 case 슬롯·foreign term/slot/DF/ref·전체 truncation·dictionary/order/EOF·1024B term·16KiB 고정 경계를 확인했다.
- 이전 Shared Small 후보 Lexical/TextSpool/Literal은 `57e1a1cc-8509-4eaf-8f15-fa6a57904c28`, **372/372 PASS /169 skip /exit0**, 본문22.304s다. 새 Original 거절/Work 거절은 backing 해제·첫 denial·원 accepted Work를 확인한다. 이전 JSON wrapper 전제 두 assertion은 full term별 distinct inline slot·같은 block child·독립 membership gold로 교체했다.
- 이전 Shared Small 후보 Runtime owning은 `d90dad09-249e-492b-b14c-7acae01adc89`, **11/11 PASS /639 skip /exit0**, compile5m57s·본문19.117s다. Small cold/policy4·gamma-only delete→alpha/beta 기존 block 유지·old view/reopen·post-confirm Unknown→same-op resume와 기존 dense3 및 profiler helper2를 포함한다. 최초 컴파일은 unused ObjectRef 결과 2곳으로 exit101·본문0이었으며 초기 artifact와 명시 대조하도록 고쳤다.
- Kernel DF9→8→9·실제 GC/reopen 보강 owning은 기존 Q mutex 입장10초 만료/exit124·본문0으로 **BLOCKED**다. 전용 실행 레인·우회 lock·새 runner를 만들지 않았다. SDK BM25 새 policy 영향 검증은 NOT_RUN이다.
현재 실제 실행 명령(공통 prefix: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 ./scripts/cargow --lane test-workspace-lane`, 공통 nextest flags: `--locked --offline --no-tests fail --no-fail-fast --status-level fail --final-status-level fail --test-threads 1`):

| 범위 | nextest 선택 | 결과 |
| --- | --- | --- |
| Lexical | `-p quanta-index-lexical --lib --no-default-features -E 'test(native::text::) \| test(native::text_spool::) \| test(native::literal)'` | VERIFIED372/372 |
| Runtime | `-p quanta-index-searchd-runtime --lib --no-default-features -E 'test(native::dense_install_tests::)'` | VERIFIED11/11 |
| Kernel | `-p quanta-index-kernel --test native_lifecycle -E 'test(text_entry_maps::) \| test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) \| test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically)'` | BLOCKED입장 만료·본문0 |

- Shared block은 각 term 조회에서 bounded 전체 block을 검증하므로 read amplification이 생길 수 있다. 원 SDK와 delta/query 측정 전 term별 Work 감소나 종합 성능 우위를 주장하지 않는다.

etcd의 post-map child readback은 독립 검증이므로 RAM에 보관한 `CheckedBlock`을 fresh authentication으로 바꿔치기하는 안은 채택하지 않는다. 현재 256-owner map apply→diff 경계까지 frame을 보유하는 것은 Original peak도 늘린다. 읽기 전용 검토에서 남은 후보는 다음처럼 구분했다.

| 수리 | 변경 전 코드 | 구현과 검증 경계 |
| --- | --- | --- |
| 같은 Producer call의 leaf 중복 get | `directory::Producer::next_edit`가 get한 뒤 leaf count helper가 즉시 같은 ref를 다시 get | 이미 읽은 StoredObject를 같은 call의 count/parse에 전달하는 국소 수리. post-map fresh get 유지. 실제 leaf 건수·순절감 미측정. |
| Directory descriptor 생산→소비 | occurrence pack은 fence/kind/terms/ref를 descriptor seal에 반영하지만 scratch에는 first key/ref만 남김 | 기존 scratch 행의 canonical descriptor DATA 생산→소비를 구현했다. CAS/order/count/EOF·독립 source key/descriptor seal·post-map persisted get을 보존하고 첫 Producer get을 제거한다. 집중6 통과, 전체 owning 및 원 SDK는 후속이다. |

후자는 단순 연결이나 기존 기능 완료가 아니다. malformed child 발견이 map apply 중에서 post-map diff로 늦어지는 경우 provisional map 객체가 더 생길 수 있으며, 어떤 owner 완료나 published head 변경도 허용하지 않는다. 메타데이터 encode/SQL/read의 실제 Work까지 순비용으로 비교한다. 원본 etcd 전체 ACK/검색/재오픈 전에는 bytes 문제 해결로 닫지 않는다.

현재 가장 좁은 후속 구현안은 새 SQL schema나 frame cache 없이 **DirectoryAlias planned 행 한 개를 canonical descriptor DATA로 변경**하는 것이다.

1. `occurrence_pack::stage_sealed_alias/stage_leaf_alias`가 이미 구한 fence/kind/terms를 사용한다. DirectoryAlias만 `k=fence, p=None, n=Inline(kind:u8,terms:u16,child1)`를 기록한다. 일반 inventory/one-block Inline alias의 Object 경로는 각각의 의미에 맞게 유지한다.
2. `InventoryEdits`·`TextSpool.inventory_batch`는 그 exact Directory descriptor 값만 허용하고 receipt의 occurrence ref는 descriptor child에서 추출한다. generic Inline 허용·receipt 검증 생략은 금지다. SQL row count/poison/read lease/EOF 계약을 유지한다.
3. `directory::Producer`의 anchored branch는 descriptor DATA로 map edit를 만들고 첫 child get/parse를 제거한다. CAS/order/count/EOF와 ordinal 검증을 유지한다. alias root 전용 `entry_patch::MapInput`·`Edits` stamp가 같은 descriptor transcript를 검증하며 다른 root에 descriptor Inline을 허용하지 않는다.
4. `visit_cold_directory`·post-map Cursor의 persisted get/child parse·독립 key/descriptor seal·count/EOF는 유지한다. 새 내부 표현을 수용한 생산·소비 경계를 함께 변경하며 병렬 IR/업캐스터를 만들지 않는다. persisted directory value 자체는 기존 descriptor format과 같다.
5. 독립 RED는 고정 packed2/leaf fixture에서 builder child get0·post-map get1, canonical root/term/position 동일성, foreign kind/count/fence/ref/CAS/EOF 및 post-map child 손상에서 owner completion0·head 불변을 요구한다. generic/nonalias 경로와 Off/Record·첫 거절도 회귀시킨다. packed anchor당 제거된 실제 get은 read1 및 `(16 + 2×payloadLen)B`; 새 SQL/stamp 비용을 차감한 순절감은 실측한다.

이 5경계는 현재 사용자의 `$ss` 명시 승인으로 구현했다. 첫 제안의 `CheckedBlock` 보유·post-map fresh get 대체는 사용하지 않았다. 새 공개 port·SQL schema·persisted directory format 변경은 없다.

### Directory DATA 현재 실행

- source descriptor는 packed block의 마지막 funded key를 move하고 정확한 `[kind,terms:BE16]+child1`을 전달한다. physical `identified`와 logical descriptor child를 비교한 뒤 Planned 행을 쓰며, 설치 receipt 전건 확인 뒤 기존 confirmation만 사용한다. 이미 확인된 Ready 경로도 같은 값을 전달한다.
- `Edits::push_descriptor`는 complete encoded MapValue의 tag/len/data/child와 wire 길이를 original root transcript에 포함한다. 같은 child의 kind/count 변경도 stamp가 달라진다. 일반 root는 Object-only이며 descriptor Inline은 aliased Directory root에서만 허용한다.
- post-map Cursor/visitor는 변경하지 않았다. 실제 child get/parse·독립 seal·token total 검증 뒤에만 owner completion이 가능하다. generic leaf producer는 같은 call에서 이미 읽은 StoredObject를 `stored_leaf_count`로 검증해 즉시 둘째 get을 제거했다.
- 최초 builder get0 behavioral RED는 Q 입장10초 만료·exit124·본문0으로 **BLOCKED**여서 행동 RED라고 부르지 않는다. 구조상 기존 builder가 직접 get한다는 source와 보존한 회귀로 구현했으며, 첫 컴파일의 Result `?`/caller 타입 및 test key move 오류는 각각 수리했다. compiler101·본문0을 행동 실패로 세지 않는다.
- `032a817e-9b74-4329-8b50-1a16ce1b1f82`: **6 PASS /538 skip /exit0**, compile26.79s·본문0.104s. Spool descriptor Ready/Planned+foreign receipt/shape2, builder get0·post-map packed get1, independent fixed-wire/hash 및 same-child kind/count stamp1, 기존 count-only authentication/Original refusal2다.
- 명령: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=60 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-lexical --lib --no-default-features --no-tests fail --no-fail-fast --status-level fail --final-status-level fail -E 'test(native::text_spool::tests::directory_descriptors::) | test(=native::text::tests::occurrence_blocks::prepare_read_cost::cold_multiblock_alias_prepare_visits_each_descriptor_and_child_once) | test(native::text_occurrence::tests::count_only) | test(native::text::tests::descriptor_transcript::)' --test-threads 1`.
- Kernel `a4293c50`는 6 actual/5 PASS/1 FAIL/exit100이었다. 실패는 fixture의 물리 child 기대값 오류다. 모든 owner의 `[0,1]` occurrence payload가 같아 child dictionary는1이고 owner membership은8이다. 고정 header/모든 row의 child slot을1child·slot0으로 정정했으며 owner/DF/TF/positions·pin·실제 파일 GC·reopen 조건은 유지했다.
- 같은 Kernel command 재실행 `01b33230-6c96-4e84-a8fe-70d315c0d7de`: **6/6 PASS /79 skip /exit0**, compile9.10s·본문12.145s. `text_entry_maps::` 4개(DF9→8→9·pin 해제 후 block 실제 파일 제거·latest 재오픈 포함), no-backend-credit dense fallback, same-packet earlier-child atomic confirm을 검증했다. 공통 prefix는 위와 같고 `-p quanta-index-kernel --test native_lifecycle -E 'test(text_entry_maps::) | test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) | test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically)'`다.
- Runtime `352e6009-c0d8-4c5c-b660-1bb98edada53`: **12/12 PASS /639 skip /exit0**, compile2m28s·본문20.742s. 실제 ManagedObjectIo/Catalog/Original 64MiB·8MiB에서 257 distinct terms×두 case의 Directory inventory와 같은 body 두 source의 DF2·독립 첫/중간/끝 positions, artifact 동일성·reopen을 확인했다. 기존 Small/dense 전체9 + policy/profiler 범위도 통과했다. 명령은 같은 prefix·nextest flags에 `-p quanta-index-searchd-runtime --lib --no-default-features -E 'test(native::dense_install_tests::)'`다.
- 전체 영향 `b0a8245d-b8a3-449a-88dc-12328f22b450`: **407 actual /406 PASS /1 FAIL /139 skip /exit100**, compile21.20s·본문58.289s. 새 Ready 재사용 fixture가 첫 owner 뒤 `apply_postings`로 scratch를 seal한 다음 두 번째 inventory diff를 넣어 `posting scratch was sealed`로 실패했다. 실제 생산 순서인 모든 inventory windows→한 번의 final apply_postings로 fixture를 정정하며, 동일 attempt/cache·owner2 DF/positions 조건을 유지한다. 새 leaf Directory·기존 모든 나머지406은 통과했다. 정정 후 `a6b876ee`도406/407로 실패했으며 원인은 first/second owner lookup 시점이었다. `ContributionWrites::flush`는 top surface1에 DATA를 staging할 뿐 `root.owners`를 갱신하지 않으므로, 두 inventory/contribution staging 뒤 한 번 final apply_postings를 하고 나서 owner map을 조회하도록 정정했다. 기대 DF2·두 owner·전514term positions 및 같은 attempt/cache/no-extra-install은 유지했다. exact 실패1의 `2072d289-7578-4dff-9a4b-7a18595e96c9`는1/1 PASS(compile21.59s·본문0.083s·exit0)였다. 그 뒤 같은407 owning을 다시 실행한다.
- SDK BM25 exact `07526974-5acc-46d4-915a-64a7272c8fbe`: **1/1 PASS /74 skip /exit0**, compile3m47s·본문2.052s. 새 Descriptor/Small policy에서 실제 UDS pinned view·fixed BM25 gold와 reopen을 검증했다. 명령은 같은 prefix·flags에 `-p quanta-index-searchd-runtime --test native_index_uds --no-default-features -E 'test(=native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold)'`다.
- Ready fixture의 실제 staging/publication 순서 정정 후 전체 `00be1a18-ad8c-492d-a58f-4d157e8b3b7e`: **407/407 PASS /139 skip /exit0**, compile10.89s·본문15.853s. Text/TextSpool/TextOccurrence/Literal 전체 영향 범위에서 Ready reuse·leaf Directory·post-map fault·기존 회귀가 모두 통과했다. 명령은 같은 nextest flags에 `-p quanta-index-lexical --lib --no-default-features -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal)'`다. 공용 큐가60초 이내 비지 않아 이 재실행은 **운영 입장 대기만600초**로 조정했고 실제 입장은 즉시였다. SDK60/원 caps 변경은 없다.
- 새 candidate SDK는 별도 후속이다. Runtime Directory fixture는 raw bytes가 다른 공백/개행 body와 batch1로 whole-body inventory reuse를 배제하고 confirmed occurrence Ready 경로의 물리 block 설치2회를 추가 검증한다. 이 보강 뒤 Runtime12를 다시 실행한다. 집중6 PASS로 원512MiB 또는 SDK60 통과를 예측하지 않는다.

## 8. 설계 근거

- [Lucene TermsHashPerField](https://github.com/apache/lucene/blob/main/lucene/core/src/java/org/apache/lucene/index/TermsHashPerField.java): term interning, byte/int pool의 term별 stream, 고유 term 정렬.
- [SPIMI 원저자 교재](https://nlp.stanford.edu/IR-book/html/htmledition/single-pass-in-memory-indexing-1.html): 제한된 dictionary·posting 직접 추가와 block 출력·병합. Native 전체가 SPIMI이거나 외부 정렬이 없다는 뜻은 아니다.
- [Tantivy postings writer](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/postings_writer.rs): term index 정렬 후 직렬화. 기존 Native sorted run/merge를 재사용하는 근거다.
- [Lucene RAM flush 설정](https://lucene.apache.org/core/10_5_1/core/org/apache/lucene/index/IndexWriterConfig.html), [Tantivy worker](https://github.com/quickwit-oss/tantivy/blob/main/src/indexer/index_writer.rs): 메모리 기반 flush. Native의 엄격한 Original 사전 승인이나 누적 Work 한도와 동일한 계약으로 복사하지 않는다.
- [Lucene IndexWriter 10.5.2](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/IndexWriter.html): flush와 durable commit 구분. Native의 reserve/confirm·root CAS 프로토콜을 대체하는 API가 아니다.

이 레퍼런스들은 설계 패턴의 근거다. 이 패킷의 신규 구현·원본 SDK 성능 또는 다른 엔진 대비 우위가 검증됐다는 증거는 아니다.

### 2026-11-15 SDK 선행 접합 현재 입력

Kernel History/Cow/request decode 13개 PASS와 이후 SearchPlane의 History replay import/unused imports/CBOR error variant compiler101·본문0은 선행 접합의 이력이다. 아래 Text7·Runtime3은 이후 별도 실제 실행에서 통과했으며, 그 결과를 전체 SearchPlane owning이나 원본 SDK 성공으로 확대하지 않는다.

후속 공용 wire·SourceJoin owning은 D의 기존 Q 큐에서 nextest `fe47aa32-26f1-4b19-a048-84bba92222e0`, **Wire12 + SourceJoin44 = 56 PASS / 373 skip / exit0**으로 종료했다. 실제 command execution `exec-02d5588d-a73d-4879-ae68-f4b222c16d32`의 compile6.27s·Starting56·completed/exit0와 Root의 전체 본문 결과를 대조했다. 기존 `Digest64` tuple wire에 맞춘 독립 gold 수리 뒤 결과이며, 이전 `866f4440`의 gold 불일치는 종결된 이력이다. 명령은 같은 lane의 `./scripts/cargow nextest run --locked --offline --build-jobs 2 -p quanta-index-contract --lib --features quanta-native-identity-v1 -E 'test(native_cutover_evidence_v1::tests::) | test(native_cutover_source_join_v1::tests::)' --no-tests fail --no-fail-fast --test-threads 1`이다. 이 선행 owning PASS도 후속 Text 영향 회귀·release 후보·Caddy/etcd SDK60의 실행을 대신하지 않는다. 해당 다음 actual handle은 아직 미수신이며 worker Rust/SDK runner는 0이다.

Text7의 첫 실행은 새 alias calibration test의 `drop((alias, cold))` E0505로 compiler101·본문0이었다. alias를 먼저 drop한 뒤 cold arena를 drop하도록 최소 수정했다. clone·추가 allocation·production·gold 변경은 없으며 formatter/diff-check 통과 뒤 같은 7scope를 Root에 재요청했다. 이 compile 실패를 제품 RED로 세지 않는다.

SDK 원입력 read-only 재확인: 보존된 `run-original-sdk.py`가 지정한 같은 snapshot/manifest에서 Caddy 590파일·3,272,107B, etcd 1,421파일·8,666,217B의 전체 path 집합·각 파일 SHA·manifest SHA·repository commit·검색 토큰이 일치했다. Caddy manifest는 `3aae07d2535fc1f165ea9e18a597021bd1fc5d732bdbdd128055e88c78f8e937`, etcd는 `15e75db82a188dda532ac7b13ffa215e6ea7c657352c202382ced438f2478fc4`다. 입력을 수정하거나 새 manifest/runner를 만들지 않았다. 이는 SDK 입력 준비 검증이며 old `16d8d0ee` 후보를 새 NTX 후보의 수용으로 재사용할 권한이 아니다. 새 release·원 캡·같은 원입력으로 실제 SDK 게시·검색·재오픈을 실행해야 한다.

### NTX2A-G7 실제 GREEN

Root의 `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-lexical --lib --no-default-features --no-tests fail -E 'test(native::text::tests::inventory_trace_tests::) | test(native::text::tests::inventory_shape_tests::)' --test-threads 1 --no-fail-fast --failure-output immediate --success-output final`은 nextest `0a7b720b-190a-441c-a3b1-bbefd2e7032c`, **7 actual bodies / 7 PASS / 529 skip / exit0**, compile26.14s·본문0.126s로 종료·실행권 반환됐다. 현재 shape SHA `75ac4cae12911c2f30e5fa66e3778118abcc70761e7fd20216bc5b8009a217e9`, production `text.rs` `ee2c6944` / `occurrence_pack.rs` `fb2eba49`를 인수한다.

이 범위는 Work3·SQL/Empty/borrowed Alias/SQL-produced InlineAlias의 shape4다. 특히 SQL 출력에 alias가 있어도 SQL provenance를 유지한다. 고정 P/U/K·literal key/position·Off/Record 원 Work·첫 Bytes 거절·메모리 해제를 확인했다. **Direct DirectoryAlias는 이 fixture의 선택 경로가 아니며 NOT_RUN**이다. 아래 Runtime3은 이후 VERIFIED로 전환됐고 원 SDK60 수용은 NOT_RUN이다. 원본 etcd의 실제 배타적 Work·shape 분포를 얻기 전 토큰 생산자 전면 재설계나 예산 완화를 채택하지 않는다.

Runtime3의 첫 compile은 새 Catalog visitor 공용 parent export 누락으로 compiler101·본문0이었고 Root가 해당 parent를 수정했다. 다음 compile의 Text leaf 네 E0308은 concrete `Arc::clone`이 expected trait-object Arc로 역추론된 문제였다. 기존 concrete Arc를 유지하고 명시 typed local coercion으로 최소 수정했다. 새 provider/Box/wrapper·backing allocation·production·gold 변경은 없다. 현재 Runtime SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과 뒤 같은 Runtime3를 Root에 재요청했다. Root 소유 History cfgtest match 두 곳의 접합 수리는 별개다. 두 compile 실패 모두 실제 제품 RED/GREEN으로 세지 않는다.

Runtime3의 세 번째 compile도 unused-result 한 건으로 compiler101·본문0이었다. fresh retry 뒤 확인한 member artifact를 보존하고 fixture를 다시 열어 current published view·membership artifact의 동일성으로 반환값을 소비했다. clone·새 API·production·gold·한도 변경은 없다. 모든 membership 호출 반환 사용을 재대조했고 formatter/diff-check 통과 후 현재 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`로 같은 3scope를 요청했다. 기존 nextest PID32495·cargo32958·rustc37509는 terminal 이후 사라진 것을 확인했으며 observation timeout만으로 재시작하지 않았다.

### 실행권 인수

Root가 usageLimited였을 때 기존 검증 실행권을 D에 넘겼고 Runtime3 Q 실행 요청도 그 인계에 포함됐다. Source의 `compile-activity-admission.lock`과 QIndex `scripts/cargow`의 `resource-admission/build-test.lock`은 별개라는 RCA는 Root가 인수했다. Runtime3은 아래 실제 run에서 종료했다. 현재 동결 fixture는 `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`이며 production SHA는 아래 GREEN의 4파일과 같다. 후속 owning·release·SDK 실행권은 D에 있고 명시 반환 전 Text worker Rust/SDK runner는 0이다. Root/D의 대화·대기 의도는 live job이나 본문 성공의 증거가 아니다. actual PID/session/terminal을 받은 뒤만 실행 상태와 결과를 판정한다. Source supplier 종료·lock 우회·새 실행 레인을 만들지 않는다.

### Runtime 첫 실제 행동 실패와 단계 RCA

D owning nextest `bfc8e6ad-3d9f-4430-8e61-9613d3139674`은 compile0.77s 성공 뒤 실제 `dense_text_post_confirm_error_preserves_current_and_fresh_retry` 1건이 1.025s·`Error: Conflict`로 실패했고 failfast 때문에 나머지 4건은 NOT_RUN이었다. namespace가 선택한 5개는 business 3 + 공유 SQL profiler helper 2이며 source 변경이나 추가 business testcase가 아니다.

Catalog/RFC의 현재 계약상 Unknown op2는 Pending SourceEvent claim을 유지하고 동일 event의 새 op3는 Conflict를 반환한다. 같은 op2를 다시 submit하는 것도 재실행이 아니며, canonical recovery는 `kernel.resume(op2)`가 새 fence에서 원 immutable request를 재실행하는 경로다. 원 stderr만으로 실패 위치를 단정하지 않기 위해 fault fixture의 initial open/configure/request/submit/reopen/retry에 phase label만 추가했다. 현 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과다. 기대값·예산·공급자·production은 유지했다. D의 같은 Q 실행권에서 failfast 없이 현재 5개 또는 exact 실패1의 재실행을 요청했다. retry 단계임이 확인되면 fixture의 잘못된 op3 API 사용을 canonical resume으로 변경하되 full publish/9owner membership/reopen 동일성은 그대로 증명해야 한다. Conflict 자체를 성공으로 바꾸지 않는다.

복구 전제 소스 대조: native `claim_recovery`는 Pending·현재 epoch 일치를 검사하고 즉시 새 fence를 발급하며 별도 lease 만료 대기가 없다. KernelServices의 read lease는 operation recovery gate가 아니다. 이 fixture의 이전 동기 실행 반환·소유자 drop·같은 exclusive root lease 재획득 뒤 `resume(op2)`가 합법이고, 이전 fence의 늦은 쓰기는 FenceLost다. Catalog 재오픈을 OS 프로세스 재시작 증거로 승격하지 않는다. 실제 Conflict의 phase를 확인하기 전에는 생산 코드 결함이나 fixture 오류를 최종 확정하지 않는다.

### Runtime phase 확정과 canonical recovery 수리

D diagnostic exact1 nextest `f5353ad3-13cc-4128-8eec-357fe6e3be4a`은 compile2m02s PASS 후 actual body1 FAIL·1.780s·640skip·exit100으로 종료했고 stderr는 `fault retry submit: incremental precondition conflict`였다. 따라서 Unknown 원 작업의 Pending event claim을 둔 채 새 op3를 제출한 fixture의 API 사용 오류로 원인을 확정했다.

원 fixture 하나에서 새 request/event 재작성 대신 `kernel.resume(op2)`가 새 fence로 original immutable request를 재실행하도록 수정했다. 이름은 `dense_text_post_confirm_error_preserves_current_and_resumes_original_operation`로 정정했고 복제 testcase를 만들지 않았다. Unknown·dense1call/COMMIT2·old current/empty/reopen·정상 full publish·9owner membership·재개 게시 뒤 다시 reopen artifact 동일성은 모두 유지했다. Conflict를 전체 성공 기대값으로 바꾸지 않았고 엔진·버짓·한도·공급자는 변경하지 않았다. 현 SHA `67b72b8247501025273888e0008d22b348f4302c2b5ece3446b594d4b3649fd0`, formatter/diff-check 통과다. D가 인수한 같은 Root Q 큐에 `test(/^native::dense_install_tests::dense_text_/) --no-fail-fast`의 primary3 재실행을 요청했다. 이 regex는 shared SQL profiler helper2를 제외한다. 당시 수리 후 실행 대기였고, 아래 run에서 세 본문 모두 통과했다.

### NTX1a Runtime3 실제 GREEN

D의 기존 Root Q 큐에서 아래 명령은 nextest `8ee85ee8-371d-405e-98ff-aef809179e83`, compile2m03s·**actual3 / PASS3 / 638skip / body6.024s / exit0**으로 종료했다. `resource-admission/build-test.lock` 반환을 확인했다. frozen fixture `67b72b82`, production write `13045e24` / wire `9c27d9b3` / text `ee2c6944` / occurrence pack `fb2eba49`가 그대로다.

```sh
QUANTA_INDEX_BUILD_LANE=test-workspace-lane QUANTA_INDEX_RESOURCE_WAIT_SECONDS=10 \
./scripts/cargow nextest run --locked --offline --build-jobs 2 \
  -p quanta-index-searchd-runtime --lib --no-default-features \
  -E 'test(/^native::dense_install_tests::dense_text_/)' \
  --no-tests fail --no-fail-fast --test-threads 1
```

- G1 PASS2.420s: 실제 승인 Original의 whole-window leaf+directory 설치 1batch/2COMMIT 대 미지원 fallback 2batch/4COMMIT, fixed DF/TF/positions·artifact·재오픈 동일성.
- F2 PASS1.380s: confirm 뒤 오류에서 Unknown과 기존 공개 root 유지·재오픈, 원 작업 `resume(op2)`의 full publish·9owner membership·다시 재오픈.
- A1 PASS2.222s: 실제 8MiB credit에서 optional admission miss 후 기존 경로 성공, first denial 없음·fast와 같은 artifact/조회/재오픈.

이 결과는 production coalescer와 복구·입장의 집중 검증이다. 실제 Caddy·etcd SDK60 수용, 모든 source의 Direct 분기, 새 dense artifact의 실제 GC fault 조합을 뜻하지 않는다. Root에 영향 owning·기존 child-first/GC/barrier·UDS rank/reopen 후 같은 source/Cargo 후보 release와 원본 SDK를 이어 달라고 요청했다. Text worker Rust/SDK runner는 계속 0이다.


### 현재 Descriptor 후보 source capture

`/private/tmp/qidx-sdk60-directory.383jci/source-before.json`은 HEAD `ade5fadc1a084fbb81eba831861e996081d80867`의 dirty workspace build inputs 2,789개를 결속한다. 기존 실패 후보를 덮어쓰지 않았다. source freeze는 오너에게 알렸으며, before/after 비교 일치를 실제 확인한 경우에만 후보 수용에 쓴다.

보강 Runtime `cc25ca9c-ed2f-427c-8fdf-6b0096c81c7f`: **12/12 PASS /639 skip /exit0**, queue157.84s·compile28.56s·본문15.873s. Directory fixture는 서로 다른 raw 공백/개행 body, inventory batch1로 whole-body inventory reuse를 배제했다. 두 window에서 동일 occurrence block2를 한 번만 설치하고 64MiB/8MiB 모두 같은 artifact·DF2·독립 positions·reopen을 검증했다. 명령은 같은 Runtime12 selector와 `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600`이며 SDK/product caps 변경은 없다.


새 후보 Native-only `cargow tree -p quanta-index-searchd-runtime --no-default-features --locked --offline --edges normal,build --prefix none`와 Tantivy package 차단 검사는 exit0이다. 같은 Q lane/jobs2/운영 대기600에서 daemon release를 빌드 중이다. queue157.80s 뒤 입장했고, 입장 시 캡처2,789개 파일의 현재 drift0을 확인했다. `cargo-metadata.json`의 canonical target_directory에서 결과를 복사하고 source-after-daemon 비교를 통과한 경우만 기존 helper 재빌드/SDK로 진행한다. 현재 binary/pair/SDK 성공으로 판정하지 않는다.


daemon release는 compile2m50s·exit0으로 종료했고 `/private/tmp/qidx-sdk60-directory.383jci/quanta-index-searchd`에 chmod0500으로 복사했다. SHA256 `98d6ef5292b1f5745b0ef11e0fabd50462130ee58e4dd9d446760aee257661fb`·14,614,976B다. source-before/after-daemon은 HEAD와2,789개 입력이 모두 같았다. 현재 기존 `native_corpus`를 별도 rebuild 중이며 pair closure 전 SDK는 NOT_RUN이다. helper의 Legacy/Native 혼합 의존성을 daemon의 Native-only/Tantivy-free 그래프와 혼동하지 않는다.


helper release는 compile5m10s·exit0으로 종료했다. 기존 `native_corpus`의 새 SHA256은 `807af561449566de636b134a277f20a5e2a41ff8b8992855edaa0801121ea616`·2,890,976B다. `source-after-helper.json`은 HEAD와2,789개 입력이 before와 모두 동일해 pair closure를 통과했다. `pair-binding.json` 및 Off1/Off2/Record1의 동일 daemon 복사본을 준비했고 원 runner의 quiet Off1 입장을 시작했다. 이 시점 SDK 본문 수용은 아직 미판정이다.


Off1은 원 quiet preflight에서 foreign rustc1·load26.576172>8로 exit1·SDK 본문0 **BLOCKED**였다. `ALLOW_CONTENDED`를 설정하지 않았고 같은 상태의 Off2를 반복하지 않았다. pre-SDK source closure도2,789개 모두 일치했다. 원 budget의 Record를 동일 Q mutex에서 별도로 실행한다. Record 시간은 quiet 성능 수용으로 사용하지 않는다.


### Descriptor 후보 원 SDK 실제 실패와 다음 경계

새 /private/tmp/qidx-sdk60-directory.383jci의 Record는 **Caddy54.63s·etcd33.60s, 둘 다 ResourceLimited·ACK0·query/reopen 미도달 FAILED**다. source/daemon/helper/원 corpus/원 caps는 위 pair로 결속됐다. Off1은 foreign rustc1·load26.576172>8에서 본문0 BLOCKED였고 Off2·Large는 실행하지 않았다. 실패 후보를 그대로 반복하지 않는다.

| 원본 | 새 첫 거절 | 상태 |
| --- | --- | --- |
| Caddy590 | reads999729+275>1000000, managed_install.rs108; bytes499758302/W523919/M658407; memory denial 없음 | COMPLETE_DIAGNOSTIC·health[]·open request0·ACK0 |
| etcd1421 | bytes536870900+28>536870912, cold_tokens.rs398; R491784/W460564/M689731; memory denial 없음 | COMPLETE_DIAGNOSTIC·health[]·open request0·ACK0 |

post-Record의 전체 workspace 비교는 두 무관한 integration test(native_product_process/ann_regions.rs, native_index_uds/read_sessions/query/hybrid.rs) 변경 때문에 false다. selected release --bin 빌드에는 두 test target이 포함되지 않고 runtime/src·harness/src에서도 include하지 않는다. 나머지2787개 입력 및 HEAD·동결 binary digests는 동일하며 record-source-binding.json에 구분했다. 전체 workspace 불변이라고 주장하지 않는다.

Caddy의 공통 첫329 posting pages를 대조하면 lane37의 R207662/W73258/B86813378와 dense reserve293calls/4306objects가 동일하다. Small lane39는 R525787→292336(-233451), physical wrapper inputs5471→325였다. 새 실행은 이 절감으로452번째 page까지 진행했지만 다시 읽기 한도에서 거절됐다. 이를 전체 읽기756k가 비슷하다는 이유로 퇴행이라 하지 않으며, ACK0을590source 커밋으로 승격하지 않는다. 현재 Small Draft는64-term SQL page마다 flush돼 cross-page child dictionary 재사용이 없다. 다만 trace에는 인접 block child 중복·남은 총 page 수가 없어 NTX1b가 원 한도를 해결한다고 확정하지 않는다. 다음 채택에는 실제 block headroom/child overlap와 전체 term count가 필요하다.

etcd의 완료된 prepare metric은704→785 owner(Alias569/Direct216/SQL0), P1118628/U367668/Kall8859762/Kuniq4458175다. map lane40/전체 map_apply3 비용은 이전보다12162098B 줄었지만, 추가81owner 준비가11927193B를 소비했다. diff는606회/124196392B로 동일한 prefix여서 후속 backlog가 남았다. NTX2B 후보 경계는 canonical census→ColdTokens fill/sort/group이지만 Kall−Kuniq4401587B나 마지막28B 거절을 전체 Work 절감·완주 예측으로 바꾸지 않는다. 원 logical edit·seal/EOF fees는 유지한다.

### 2026-11-16 confirmed cold Dense page 재사용

기존 `PostingCasePages`는 같은 term의 두 case를 같은 페이지에 넣고 backend same-batch sharing을 사용한다. 이번 보완은 다른 term page의 동일 sorted owner/witness DATA에 한정된다. 실제65term×9owner·130case·3page fixture의 수정 전 nextest `a7960636-aeb8-454c-8510-2692d21cf0bf`는 compile 성공 뒤 actual1 FAIL/651skip/exit100이었다. 관측값은 install3·COMMIT6이며 목표는1/2다. compiler 실패는 이 행동 RED로 세지 않는다.

`Draft::cold_key`는 finished·prior 없음·nonlocal만 허용하며 version domain/PAGE_ROWS/count/모든 owner32+witness32를 SHA-256으로 결속한다. 실제 새 해시 순회의 Work도 청구한다. 메모는 기존 batch 상한256슬롯·전체 digest 비교·충돌 시 결정적 교체다. parent Original의 optional grant로 entries와 pending key backing을 생성 전 승인하며 미승인은 원 경로다. 원 stream CAS/order/count/EOF/prepass epoch를 모두 확인한 뒤만 hit을 적용한다. 새 root는 전체 install ordinal/ref 영수증을 통과한 뒤 기록하고 실패한 prepare는 메모를 버린다. 최종 Catalog parent admission과 DF/term Top 편집은 유지한다.

Full-term-bound Small block은 서로 다른 term page 사이에서 같은 root가 될 수 없다. 기존33B Object-only `confirmed_postings` 조회는 Dense Inline도 찾지 못했다. 그 미사용 API·SQL·보조 인덱스·전용 테스트를 퇴역했다. 실제 Top MapEdit 저장은 유지하고 삭제한 인덱스의 예약·쓰기만 제거했다.

새 Lexical owning nextest `5819c585-a27e-4574-9509-cb85fbe3a45e`: **408/408 PASS /139skip /exit0**, compile37.37s·본문27.193s. 공통 prefix는 `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane`, 명령은 `nextest run -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`이다. 기존407에서 obsolete SQL 전용1을 제거하고 메모 custody/fence·고정 hash gold2를 추가했다. production SHA·실제 runtime/SDK 증거는 후속 후보에서 다시 결속한다.

새 Runtime owning nextest `ccab0af4-f692-47bc-aaab-2cdc7923b24a`: **13/13 PASS /639skip /exit0**, queue347.794s·compile10m30s·본문30.413s. 같은 prefix에서 `nextest run -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`을 실행했다. 새 cross-page fixture는 64MiB에서 install3→1/COMMIT6→2, 8MiB에서는 install6→2/COMMIT12→4와 DF9·9owners·양 case TF1/positions0/32/64·artifact·fresh reopen 동일성을 검증했다. 기존 actual Original·post-confirm Unknown/resume·Directory Ready·Small 회귀도 같은13 scope에 포함된다. 시간은 contended 단위 검증이며 SDK60 수용이 아니다.

키 풀 분류: canonical cursor·연속 key/12B row arena·정렬/grouping·packed block의 같은 position vector sharing은 이미 구현됐다. 반복 occurrence의 key byte interning은 새 구현이다. 관측 prefix의 Kall−Kuniq=4,401,587B 및 가정한 row12→8 절감4,474,512B는 metadata 전 backing 추정이며 누적 Work 환급이나 전체 etcd 완주 증거가 아니다. RAW span 부모 chain 기준 read32 배타적 비용은111,996,156−74,391,304=37,604,852B다. 전역 posting33을 빼서30,975,194B라 한 추정은 사용하지 않는다.

### 후속 수용 경계와 전체 Work 소유자

Kernel/UDS 후속 요청은 동일 Q mutex의 입장600초 만료로 exit124·본문0 **BLOCKED**였다. 이를 test FAILED로 세지 않는다. 새 후보 `/private/tmp/qidx-sdk60-cold-roots.hysXvG`의 초기 캡처2,796개와 비교해 다른 오너의 harness ann/harness/lib, Runtime Cargo.toml/canonical_publication_admission, integration native_boot 변경6개를 확인했다. Native test graph 정리 중인 입력이므로 이 초기 캡처로 release/SDK의 동일 source를 주장하지 않는다. 새 binary·SDK는 **NOT_RUN**이며, 합류 뒤 입력을 다시 결속해야 한다. 실제 캡·골든·의존성 feature를 우회하지 않는다.

같은 Kernel6/UDS1 재시도는120.855s 뒤 입장했으나 새 D 합류 dependency `native_cutover_evidence_v1.rs:968-969`의 `blake3::Hasher::update` 반환값 미사용2건에서 compiler101·본문0이었다. build check는 FAILED, 테스트7은 NOT_RUN이다. 해당 오너가 반환값을 한 chained call의 `_hasher` binding으로 소비한 현재 소스를 확인했고 Text가 같은 hunk를 중복 수정하지 않았다. 동일7 scope를 재요청했다. 이 lint 접합 수리는 Text 행동 RED/GREEN이 아니다.

합류 뒤의 dependency tree 재확인에서 Runtime `--no-default-features --edges normal,build,dev`와 helper `--no-default-features --edges normal,build` 모두 Tantivy package0·exit0이었다. 기존383jci 후보 helper graph에 관한 과거 상태를 현재 후보에 복사하지 않는다. 이 결과는 그래프 검증이며 합류 compiler·테스트 본문·새 release binary·SDK 수용을 대신하지 않는다. 현재 후보의 두 tree는 외부 `native-runtime-with-dev-after-merge.txt`/`native-helper-after-merge.txt`에 있다.

etcd Record의 RAW 부모 span별 disjoint accepted Work에서 BuildPrepare는518,944,526B, 그 자식은 Text lexical_build299,881,954B·Literal186,751,136B·RepoMap32,020,144B와 작은 provider/seal이다. 관측 top-level span 합534,132,785B와 실제 accepted536,870,900B 사이2,738,115B는 미관측/바깥 비용이므로 합을 전역 총액이라고 부르지 않는다. 한도536,870,912B까지 남은12B와 거절된 추가28B를 미관측 accepted 비용에 섞지 않는다. 앞의 prepare/diff/map 부분 합만으로 전체 실패를 Text 키 풀 결손으로 단정하지 않는다.

Literal lane47의52,349,516B는 설치한 새 block384개를 각각 한 번 fresh get/decode한 뒤 독립 source seal과 대조한 비용이다. `literal_seal_refuses_changed_persisted_child_without_publication`이 이 검증을 요구한다. body collect66,553,010B도 Literal 내부 owner별 body 한 번이며 기존 bounded delta spill·압축 block encode·독립 seal이 이미 존재한다. 단순 cache나 fee 삭제로 제거하지 않는다. `read_retained`도 같은 logical read/wire/decode Work를 청구하므로 누적512MiB 해법으로 간주하지 않는다.

RepoMap `file_map::check_changes`는 인증된 next source record를 얻지만 전체 path 고유성·EOF를 마친 `stage_contributions`가 같은 source record를 다시 읽는다. 이 경계는 제한된 scratch DATA 전달을 새로 보완할 후보이며 단순 port 연결은 아니다. 전 owner record를 RAM에 보유하거나 EOF 전 쓰기를 시작하는 대안은 채택하지 않는다. scratch 쓰기/읽기까지 포함한 순절감·소유권·회귀를 확인해야 하며 현재는 정적/trace 분석뿐이고 구현/실행 **NOT_RUN**이다.

### 현재 Text 인계 상태

**state: partial.** Text 코드 보완·집중 회귀는 완료했지만 합류 후 제품 수용은 완료하지 않았다. 마지막 동일 Kernel6/UDS1 요청도 입장600초 만료·exit124·본문0 BLOCKED로 종료했다. Text worker의 live Rust/SDK는0이며 초기 SDK 후보 캡처는 수용에 사용할 수 없다. 현재 source freeze는 해제하며 Root의 기존 단일 실행 큐에서 합류 입력을 다시 캡처한다. 실패 후보의 SDK 반복, 별도 실행 레인, 예산/골든 변경은 하지 않는다.

후속 exact 명령은 공통 Q prefix와 다음 target이다. 두 binary의 selector를 구분해 Runtime의 다른 `text_entry_maps` 테스트를 우연히 포함하지 않는다.

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 \
./scripts/cargow --lane test-workspace-lane nextest run \
  -p quanta-index-kernel -p quanta-index-searchd-runtime \
  --test native_lifecycle --test native_index_uds --no-default-features --locked --offline \
  -E '(binary(native_lifecycle) & (test(text_entry_maps::) | test(=dense_install_batches::native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens) | test(=physical_gc_test::managed_batch_confirms_parent_with_earlier_provisional_child_atomically))) | (binary(native_index_uds) & test(=native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold))' \
  --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

Root의 Native test graph 합류 뒤에는 위7 scope와 Runtime13을 재결속하고 Native-only daemon·현재 helper를 같은 소스에서 release 빌드한다. 원본 Caddy590/etcd1421·SDK60·512MiB/읽기100만회·Original caps로 quiet Off와 Record를 분리한다. Source/binary·원 입력·출력·최초 거절이 결속된 새 SDK가 통과해야 NTX4/후속 Large로 진행한다. 키 풀·RepoMap 전달 경로·전체 streaming 확장은 실제 새 후보의 순 Work/입장 결과를 근거로 별도 채택하며 현재 구현 완료로 세지 않는다.

### 2026-11-16 기존 목표 재개 · 현재 코드와 남은 인수

사용자가 이 채팅의 기존 잔여 작업을 active goal로 재개했다. 철회된 I06 3주 감사 배정은 수행하지 않는다. HEAD는 `ade5fadc1a084fbb81eba831861e996081d80867`, index는 비어 있고 기존 dirty/untracked 작업은 보존한다. Dense memo·caller의 현재 SHA는 write `f1417afdcf2cf05baf98bc4aa8e20564a90893038003ea55514ca43f173e7735`, entry_patch `4ae4f704b47541377f489098d27baeb597169ef8e3bca7de23b6e8483fe2d49f`, text `ea34880194100457732e6257440e6eadaa65e42681eb69ec8f85a1788cf65458`로 이전 보완과 같다. 정적 재점검에서 source EOF/cardinality→same-attempt memo→설치 영수증→최종 child 인증 경계의 새 확정 결함은 발견하지 못했다.

공용 F 인수 기록의 Native nextest `44e83b28-438c-4b73-97f5-5a7a2c7c6bbe`는 실제107건 중106 PASS/Contract fixture1 FAIL이며 Runtime/Harness37 PASS에 dense-install13이 포함된다. Contract 수정 후 별도 `b5e041f5-97eb-417a-8c40-eb17a6efaf26`은70/70 PASS다. 이는 인수 기록의 재사용이며 전체107 PASS나 새 release 동일 후보 수용으로 합치지 않는다. Text는 408/13을 무조건 재제출하지 않는다.

이전에 막힌 Kernel6/UDS1은 공용 compiler 접합 수리와 Native 실제 본문 이후의 변경된 입력에서 위 exact selector로 재검증한다. 현재 목표 재개에 따른 직접 검증 범위는 이7건뿐이며 기존 `test-workspace-lane`·jobs2·mutex·운영 대기600초를 유지한다. 실행 중인 동일 요청은 확인되지 않았으며 관측한 이전 nextest PID93953/24708 및 자식은 이미 종료했다. 공용 parent/Cargo/lock·타인 claim·실행기는 수정하지 않는다. 이 결과와 별도로 새 source/binary 결속·원 SDK60은 여전히 NOT_RUN이다.

**보완 전 검증 공백 T-DENSE-ALL-SLOTS:** `dense_install_tests.rs::repeated_dense_request`는65term×2case×9owner를 게시하지만 `check_repeated_dense_membership`는 term000/032/064만 읽는다. case-paired64-row page의 각 case 첫 term에 해당하므로 나머지62term의 실제 조회·위치·재오픈을 직접 확인하지 않는다. 이는 기존 oracle의 범위 제한이며 생산 코드 결함이나 실제 false-green을 재현한 것으로 세지 않는다. preimage SHA는 `97f61142737669ff1fffa0d786b5da0ade35ac6d7e3e01a04ff685b606428e95`다. 같은 helper를65개 전체 term의 고정 owner/TF/position oracle로 보완한다. 실제 Store/Catalog·64MiB/8MiB·COMMIT 수·artifact·4회 open/reopen 검증과 원 예산은 유지한다. 이 cfg(test) private leaf는 현재 Kernel/UDS integration target의 non-test Runtime library에 포함되지 않는다. 새 owning 요청은 기존7의 종료 뒤 동일 큐의 exact cross-page Runtime1이며, 이전 Runtime13 결과를 이 새 oracle의 통과로 복사하지 않는다.

**Kernel6/UDS1 VERIFIED:** 위 exact binary-scoped 명령은 nextest `601f5482-1dc9-422b-95d1-74353adad91f`, discovered7/executed7/PASS7/FAIL0/filter153/exit0으로 종료했다. 기존 mutex 입장368.174s·compile1.29s·본문14.574s, 실행권 반환을 확인했다. 현재 dirty HEAD의 실제 GC·DF9→8→9·old pin/reopen·earlier provisional child·미지원 batch fallback·고정 UDS BM25 범위다. 과거 compiler101/body0 및 입장124를 삭제하거나 성공으로 바꾸지 않는다. 제품 원 SDK60은 이7개로 승격하지 않는다. 새 Runtime1 소스 SHA는 `80fedbea51c28ffd46f7288561c51427d3a604bb437cca2d3fe5bc11998cb54d`이며 다음 동일 큐의 exact selector로 확인한다.

**T-DENSE-ALL-SLOTS VERIFIED:** nextest `524bb92d-2009-4da9-9770-eec63e69291e`는 discovered1/executed1/PASS1/FAIL0/filter611/exit0, compile36.75s·본문5.441s였다. 명령은 공통 Q prefix에서 `nextest run -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(=native::dense_install_tests::dense_text_confirmed_root_reuse_crosses_posting_pages_with_real_original_credit)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`이다.65term×2case×9owner를64MiB/8MiB 및 각각 fresh reopen에서 모두 고정 DF9/TF1/위치0..64로 확인했다. 기존 install1/COMMIT2 대 fallback install2/COMMIT4·동일 artifact·first denial 없음은 유지했다. production 변경 없이 기존 oracle 공백을 보완한 결과다.

현재 재개 목표의 다음 직접 실행은 이 검증 이후 같은 warm lane/단일 mutex/jobs2의 새 source snapshot→Native-only daemon+기존 helper release→원 SDK60이다. 최초 hysXvG snapshot을 재사용하지 않고 새 외부 후보에 before/after 입력 집합과 binary digests를 결속한다. 이 목적에 맞지 않는 clean-Git benchmark closure를 dirty Main에서 우회하지 않으며, 기존 SDK의 dirty snapshot binding과 benchmark/release qualification 범위를 구분한다. 원 SDK의 파일/Work/IO/시간·Original caps·qrels/gold·trace mode는 유지한다. 이전 binary·실패 prefix·지표로 새 후보를 통과 처리하지 않는다.

**새 SDK 후보 준비:** `/private/tmp/qidx-sdk60-all-slots.zU7Bre`에 현재2798 build inputs를 캡처했다. Native daemon+helper normal/build dependency tree의 Tantivy0 검사 exit0, Caddy590파일/3,272,107B와 etcd1421파일/8,666,217B의 manifest SHA·모든 파일 해시·commit·query path/token 일치를 재확인했다. 기존 manifest/원 SDK script를 변경하지 않았다. daemon release는3m16s·exit0이며 before/after2798개+HEAD가 모두 같았다. 복사한 chmod0500 daemon은14,631,472B·SHA256 `8bf83973455773ffaacdbdcc25c2410ad6d545a63a38f134c55d807ddeaf545f`다. 현재 같은 큐에서 helper의 명시적 no-default release를 빌드 중이며 pair closure/SDK 결과는 아직 NOT_RUN이다.

**새 pair 결속:** helper no-default release는12m01s·exit0으로 종료했다. chmod0500 helper는2,873,968B·SHA256 `28c9db78ddd449bdb53f9950f054587a9f359302b295b6290a968c0e23340068`다. before/after-daemon/after-helper의2798개 입력 집합·해시와 HEAD가 모두 일치하며 복사 daemon 해시도 같다. `pair-binding.json`과 원 입력 `input-binding.json`은 새 외부 후보에 있다. 같은 daemon을 Off1/Off2/Record1에 복사·해시 대조했다. 현재 단일 mutex에서 원 `run-original-sdk.py`의 quiet Off1 입장을 시작한다. 환경의 contended override와 Work limit override는 설정되지 않았다. benchmark clean-Git closure/출하·설치 qualification과 이 dirty source SDK 후보 결속을 혼동하지 않는다.

**새 Off1 BLOCKED:** mutex 입장62.829s 뒤 원 preflight가 `host_load one_minute=26.200684 limit=8.0`로 exit1·SDK 본문0을 반환했다. contended override를 설정하지 않았고 Off2는 같은 상태에서 재제출하지 않는다. 이 결과는 새 binary의 ResourceLimited나 성능 실패가 아니다. 같은 pair/원 caps의 Record를 기존 mutex에서 진단 전용으로 실행하고 최초 거절·완료 작업량을 회수한다. Record 시간은 quiet 수용으로 사용하지 않는다.

**새 Record FAILED:** Caddy는43.93s·reads999982+27>1000000·B499650244/W524037/M658415, etcd는20.96s·bytes536870900+28>536870912·R491784/W460564/M689731에서 ResourceLimited·ACK0이었다. query/reopen에 도달하지 않았다. 두 진단 모두 COMPLETE_DIAGNOSTIC·health[]·open request0이며 parent exit1/실행권 반환을 확인했다. Source before/after-record2798개+HEAD 및 두 binary digests가 모두 같았다. 이 실패 후보를 그대로 반복하거나 Large로 우회하지 않는다.

Caddy는 이전452→새454 posting page까지 진행했다. lane37 누적 reads308687→307081과 bytes124723818→132989366, lane39 bytes58084579→51497767이다. 진행 prefix가 달라 이 누적치만으로 순절감률을 만들지 않는다. 같은 first329 page 및 실제 object calls를 대조해야 한다. etcd의 prepare/lexical Work는 이전과 정확히 같아 이 Dense·Small lookup 보완을512MiB 해결로 세지 않는다.

**다음 확정 caller 결손 T-SMALL-PAGE-FLUSH:** `apply_postings`의 Small Draft는 실제16KiB/512row 한계와 무관하게 매64-term SQL page 끝에서 flush한다. case-paired page는 각 case의 순서만 전역 단조이므로 하나의 Draft를 그대로 이어 붙일 수 없다. 기존 canonical Small codec·DF2..8·full-term/child/slot 인증을 재사용하고 case별 제한된 Draft+pending locator로 실제 byte/row 경계에서만 확정하는 caller 보완을 검증한다. 전체 NTX1b queue·prepared API·새 Core/Storage/Catalog 계약으로 확대하지 않는다. source EOF는 page별 기존 검증을 유지하고 전체 Top 적용/공개는 마지막까지 유보한다. 메모리/오류별 다른 packing fallback을 추가하지 않는다.

보완 전 producer/consumer는 `text.rs::apply_postings`의 stage38→page-end `stage_small_posting_block`→stage39 put_batch/locators/top_batch다. wire owner는 `small_posting_block::Draft::push/finish`이고 limit은16384B/512rows다. Current preimage text SHA `ea34880194100457732e6257440e6eadaa65e42681eb69ec8f85a1788cf65458`, codec current policy는recipe-fixed-v1이다. 새 grouping의 결정성은 명시적 layout-policy bump와 fixed hash gold로 결속한다. 동일 digest/다른 wire 허용·후반 install 실패의 성공 처리·post-map persisted 검증 생략은 금지한다.

독립 실제 RED 요청은 Runtime `native::dense_install_tests::small_text_packs_across_posting_pages_with_real_original_credit`다.129lower-case terms×2case×2owners는5 SQL pages지만 각 case의129term/258row block은 실제 byte/row bound 안에 들어가므로 고유 QITXSP01 입력2개를 요구한다. 기존5개가 실패해야 한다. 실제 stage_sources/Kernel submit·모든 term의 DF2/TF1/위치·64MiB/8MiB·fresh reopen/artifact가 동일해야 하며 원 budget은 유지한다. fixture SHA `874a8e8489e7267529a697fb08c4a197e60385263539aebc4957d5dc9cf7bf84`다. Storage는 같은 batch의 동일 object를 Catalog 전에 이미 dedup하므로 caller Dense dedup만으로 Caddy read quota를 해결한다고 주장하지 않으며 이번 보완과 혼동하지 않는다.

### T-SMALL-PAGE-FLUSH implementation and current proof

- Behavioral RED: nextest `9101c8ae-885c-44a1-bcac-ba09fb0d7726`, executed1/FAIL1/filtered612/exit100. Real Store/Catalog returned5 QITXSP01 inputs where the fixed129-term/2-owner/2-case oracle requires2. Membership checks completed before the block-count failure.
- The canonical caller now retains two bounded case-stream drafts across SQL pages. It flushes at the existing16384-byte/512-row envelope, validates the exact install receipt before creating locators, and applies the final Top graph only after all drafts and scratch Top batches complete. EOF tails merge only if the same wire envelope fits; the existing few-term one-block oracle is retained. Child dictionary indices and pending locator slots are remapped in canonical order.
- Original grants cover draft/pending/key/Top backing until its actual last use. The fixed tail-remapping table is admitted before allocation; copy and slot-rewrite Work remains charged. Failed memory/Work admission is terminal and does not create a smaller fallback layout.
- The recipe layout stamp now includes `packing-per-case-stream;merge-fitting-tails;recipe-fixed-v2`. Independent SHA-256 vector is `0b22e8dac28bbf924e6475408c8da973caf32ddcb32bd0df85e6a811b9c0afc5`; the previous page-layout policy remains rejected by producer and reader.
- The first Runtime14 attempt failed compilation (unused EncodedBlock memory field, compiler101/body0); the field now explicitly owns the grant as `_memory`. The subsequent a9c34819 run terminal output was not recoverable after session closure, so no pass/fail result is claimed from it. It is superseded by the current changed-source request.
- Narrow proof VERIFIED: nextest `d768216e-0113-4084-a922-0ceb7f3f4622`, executed7/PASS7/filtered542/exit0, compile30.94s/body0.063s. Command: common Q prefix + `nextest run -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'test(native::text::small_posting_block::) | test(=native::text::tests::small_tail_join_admission_failure_preserves_both_drafts_and_original_leases) | test(=native::text::tests::cold_case_pair_shares_block_with_distinct_bound_slots)'` + the existing single-thread/no-tests-fail suffix. It covers independent wire/decode, distinct child remapping, exact16384 bytes versus16385-byte decline without mutation, and Original/Work first-denial custody.
- Runtime private tests now require fixed129->2 and300->3 blocks at64/8MiB, every term/case/owner/DF/TF/position, artifact equality and reopen. The second-block post-confirm fault requires Unknown(original operation), unchanged published view, all term memberships absent before/after reopen, then resume of that same operation and full membership/reopen equivalence. These added bodies remain NOT_RUN until a valid owning request terminates.

**Integration compile FAILED / bodies NOT_RUN:** the combined Lexical+Runtime request exited101 before nextest discovery due five current RepoMap `file_map.rs` errors: unused `SourceChange` import(:7), missing `PathKey::as_slice` at589/627/631, and borrowed-after-move `plan.input` at437. Admission was immediate, held114.777s; no behavioral failure or passing test is inferred. Observed RepoMap SHA `42649899767df60fb8aebd4b50b1525b15178f6131d291e132e68bac418c8bf1`. This foreign writer hunk is preserved. Runtime/Kernel/new release/SDK require its source-ready correction. The independent Lexical410 scope is requested on the same existing lane; it cannot replace the blocked integration scope.

**Lexical owning VERIFIED:** nextest `5a7d37c7-4f77-4fc1-bc87-08aa0cbe2f41` executed410/PASS410/filtered139/exit0 (body14.047s). Final review then funded the new Top key-lease array/metadata with its own lifetime grant; repeat `5d5e7a75-181d-4aeb-8045-8fbaa68eab48` executed410/PASS410/filtered139/exit0 (compile5.87s/body13.634s). Command is the previous Lexical408 owning selector unchanged. Final Text SHA `cb82c74e3965a6bdc225725737089f6435e153a7ad8f0c30d835ffe970bb7756`; codec SHA `a2c508399459a25faa49e6d8d1fb3591b63084eff3bbd9c39b7253aefa3103d7`; Runtime private leaf SHA `9272b65e9a267d0152bfcf8a925efacd06c59df6effae712afff0e26541da00f`. Formatting and tracked diff whitespace checks exit0. Runtime/SDK status is not inferred from these410.

RepoMap's independent owner subsequently corrected the five observed compile sites (new SHA `f0e4ab3574f849b686ef1866c212afc51d44ca09c3b1b98304ae42c8d399c0e6`). Text did not edit that file. The same previously failed combined command is requested again against this changed dependency input.

The changed-dependency retry also exited101/body0: RepoMap FundedPayload.memory unused at file_map.rs:128 and scratch.rs:154 db.execute usize result unused. This is a new compile boundary after the previous five sites were corrected, not a Text test failure. The shared source is still changing; no unchanged retry or release build is started. Runtime15/Kernel7/new SDK remain NOT_RUN for this Small candidate.

**Blocked audit, third consecutive goal turn:** independent Text code and Lexical410 remain unchanged. RepoMap inputs changed to file-map SHA `4270661c99477c8436ded23780a24e210921e938392f421cf9e2a59d6af4a0d0` and scratch SHA `13ceefdb29550f248234a7d8ff531ec77db1f9bcf2533433d871bcc0842e5b3d`, but the two failing constructs remain: `FundedPayload.memory` has no read, and `InventoryScratch` still discards the `db.execute` usize result without a binding. No Rust/SDK process is live. A hash change alone does not justify repeating the same unresolved failure. These are the existing independent RepoMap writer's hunks; Text does not overwrite them. Goal status is BLOCKED pending source-ready dependency correction. Resume with the same combined owning command, then affected Kernel6/UDS1, fresh release pair binding and original SDK. No full-product completion is claimed.

### Historical compact-cursor closeout verification, 2026-11-16

The preceding blocked audit is historical: both compiler sites are now corrected by the existing RepoMap owner. The current user's resumed request was evaluated against live source. Small/Dense producer, receipt validation, reader and final publication are connected; no additional Text correctness defect was found in the focused static review. No new production patch, engine or port was invented for this turn.

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal) | test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

VERIFIED: run `c2c3d5e8-b512-4a59-a982-ca9036ed978f`, 425 executed/425 PASS/738 filtered/exit0, admission317.310s/compile2m25s/body68.860s. This is Lexical410 and Runtime15 on dirty HEAD `ade5fadc1a084fbb81eba831861e996081d80867`, with the three Text-owned source hashes in the current section unchanged before/after. Runtime includes 129/300-term fixed block counts, 64/8MiB Original scopes, complete membership/position oracles, first- and second-block post-confirm failure with same-operation resume, Dense cross-page reuse, and fresh reopen. Formatting and scoped whitespace checks exited0.

F's current integration run `16c936f8-7f9e-4b90-9762-4769cc84ee3a` is consumed as10/10 PASS across4 binaries/158 filtered/exit0, not rewritten as the older exact7 selection. Its actual selectors cover FileMap/Source transitions, Small old pins/reopen, occurrence paths, fixed UDS BM25 and the process Text policy contract. Native Runtime+Harness normal/build/dev tree has zero Tantivy packages; full workspace retirement remains F's scope.

The already-running combined no-default release under F's existing queue was reused rather than duplicated. Its first release session80090 exited0 after456s compilation; the final same-command cache validation session77274 exited0 in1.46s. The first release PID71229 is terminal. Before admission, three ANN-owned inputs changed; the final binding therefore uses `source-before-cache-check.json` and `source-after-build.json`, not the earlier snapshot. Those2799 inputs and HEAD also match `source-after-sdk.json`. The Native tree result has zero Tantivy packages. Daemon SHA256 is `652ffa299d5747c511c0d38cad8cfdaae419e2623dfe8ad1110ce1271a50352c`, helper is `a3eecb0abadbf7b3e747d1566b0e84c3468fcc86daa6dc5b6c4548b4d82f1cc4`; both local files were independently rehashed. This proves dirty-source candidate binding, not signed installed qualification or an independent ANN handoff.

**Historical compact-cursor SDK result: FAILED.** F's existing resource-admitted Record controller PID9037 terminated after both corpora. Caddy590/3272107B and etcd1421/8666217B both refused ResourceLimited/FULL ACK0, so query/reopen was not reached. Exact denials are in the current section. Both saved traces have matching hashes, COMPLETE_DIAGNOSTIC, health[] and open_requests0. `sdk-results-summary.json` records original file30000/Work536870912B/R-W-M1000000/SDK60, unchanged source, diagnostic Record times32.56s/21.67s, and current Off BLOCKED at load31.640137>8/body0. Diagnostic times are not quiet performance acceptance.

**Remaining work classification:** Small/Directory/Dense code, their production wiring and the current owning regression are VERIFIED at their stated scope. Product SDK resource acceptance is FAILED; quiet SDK60 is BLOCKED, not passed. The next Text code change requires a measured, actual work elimination with the existing logical-edit fees and EOF/CAS/independent seal/readback intact. The current etcd refusal at packed_runs.rs213 is the original logical-edit admission, not a packet-copy operation. Latest complete trace parent-chain analysis gives posting-edit lane33 exclusive91263484B and inventory prepare lane46 exclusive57615227B across861 completed owners. Direct239 failed alias-probe work has only a loose upper bound7451156B, not a demonstrated saving; block_shape itself charges zero bytes. This evidence does not justify a fee discount or an unconditional new producer/prepared API. No unchanged SDK retry or speculative production patch was submitted.

### T-PREPASS-COUNT-DATA: authenticated cardinality DATA reuse

Current user explicitly authorizes code remediation and original Caddy/etcd SDK rerun with512MiB and all existing fees unchanged. The first selected real duplication is packed_term_page discarding authenticated added/removed values, followed by packed_cardinalities repeating the same packet SQL query, payload copy and parse. The per-term logical read remains one. Full PostingCursor CAS/count/EOF/prepass validation remains mandatory. Parent owns text_spool.rs/packed_runs.rs/posting_case_pages.rs; the private cardinality test leaf has one delegated test writer. No Core/Storage fee or cap hunk is taken.

The behavioral RED request is packed_cardinality_reuses_authenticated_term_page_without_packet_reread:129 input terms, first64 keys, cardinality reads64/packet-copy bytes0, then64 fixed edits and EOF/proof validation. The request uses the original canonical lane/mutex/jobs2; production has not been edited before this RED.

Planned typed provenance: bounded Original-funded per-page counts bind to a private per-spool Rc source token, key-page digest and sealed source. Different spools and unexpected SQL epoch changes are rejected. Authorized Top writes may advance the source's expected total-change stamp only after a successful write, with a pre-write epoch check; cached cardinalities are still DATA, not posting-map authority. Derived case pages carry counts and reorder them with keys. Manually constructed pages retain the original SQL validation; a mismatched authenticated page must never silently downgrade.

T-PREPASS-COUNT-DATA actual behavioral RED: nextest d233bd2f-7e7f-4a30-93c1-127b2b85e068 executed1/FAIL1/filtered549/exit100, admission208.815s/compile1m25s/body0.030s, fixed reads65!=64. The new count integrity seal includes keys+added+removed+case flag. Its actual new hash input is charged (fixed64-term input2157B), so the previous speculative bytes0 assertion was replaced with the independently framed hash-cost gold; logical reads64 remained unchanged. Narrow run987c26d0-bf82-4349-b835-8fffa71df822 then passed3/3. The five provenance/corruption/case-pair tests were added afterward; an initial E0716 in a test expected-key temporary was compiler101/body0, fixed with a named lifetime binding. Combined run c7e5e7b1-a374-44a3-84cb-d62644e01f4b executed10: count-cache8 PASS, census-skip newbehavior1 FAIL, exact-census fallback1 PASS; filtered547/exit100.

T-CENSUS-SINGLE-FILL behavioral RED is the actual prepare_inventory caller: sufficient Original with fixed ASCII/Unicode group gold still emits census span1/110B instead of0. Its low-admission fixture retains one exact census with independent3083B gold and SQL fallback. The selected remediation is a checked all-Unicode length bound, not a corpus cutoff: indexed case rows P<=2ceil(N/2), case key bytes K<=13N+P; row backing12P. Pinned Rust1.92 to_lower returns [char;3], each UTF8 scalar<=4B, and the canonical cursor emits nonempty runs separated by at least one delimiter byte. The bound is used only after actual Original admission for both packet and arena; a miss drops the unused larger packet then uses the original exact census. The same cursor/fill/sort/output and key/position caps remain; no rate or limit is changed. Primary bound source: https://raw.githubusercontent.com/rust-lang/rust/1.92.0/library/core/src/unicode/unicode_data.rs (to_lower signature).

Superseded initial broad run8f589b09-6af6-4ab2-b37e-e559d18bea67 executed437:429 PASS/8 FAIL/738skip/exit100 after389.198s admission/13m46s compile/125.902s bodies. Runtime15 all passed; eight Lexical failures expose integration seams: two old diagnostics fixtures still required the now-skipped census span; spool-scoped provenance was incorrectly funded by the reader and outlived that reader; empty EOF pages charged an unnecessary45B hash on each repeat; early epoch corruption changed two existing error-class oracles.

T-PREPASS-FINAL-1 supersedes the preceding source-ready: fund the source token from the first validated producer append before packet effects, record its immutable-source stamp after seal, skip metadata allocation/hash for empty pages, and classify changed sealed scratch as CorruptObject while preserving the independent DensePrepassProof epoch contract. Existing cap/EOF/refund/tamper tests are preserved; diagnostics expectations follow the real omitted span, with fallback3083B still independently tested. The optimized map-census build captured the preceding revision (all2800 inputs matched its own before/after); it must not be promoted to this correction without fresh source/binary binding. No rates/caps/golds were loosened.

### T-PREPASS-FINAL-1 regression and bound candidate

VERIFIED: final owning command is the existing `test-workspace-lane` combined Lexical/Runtime lib rail:

```sh
env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-runtime --lib --no-default-features --locked --offline -E 'test(native::text::) | test(native::text_spool::) | test(native::text_occurrence::) | test(native::literal) | test(native::dense_install_tests::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail
```

Run `4f9e0f40-8f95-4ad6-a31f-21cd66e43852`:437 executed/437 PASS/738 filtered/exit0, compile5m12s/body110.156s. The preceding focused repair `3abb4149-8b3c-4bf1-aac9-d37c4635a83b` passed19/19, and new-feature run `7b77ced8-474e-4231-94ef-a18328c5ee36` passed18/18. Scoped formatting and whitespace checks exit0. Final source hashes: text.rs `be6fa449c6817250c8279f69e662e09f6819329f6f66f24112891326b98df0e4`, cold_tokens.rs `46fb4292d58ccb02c8d771f0d087b403a7e61734c5c358313108806edfb4ea38`, packed_runs.rs `1696cba0ecdd4c4b51c79f51013b6bad286968f83e3d52ebf29023a2fa9c6a4e`, posting_case_pages.rs `3fdd08d8b2f50ebb30762411a14ec134a23d56cb315c2921cd6b13f5a3ac5cc9`.

Measured intermediate original SDK DATA (compact-cursor versus map-census, not final qualification): Caddy census589 spans/8,353,147B→0; etcd861 spans/15,357,748B→0. Completed-owner count, nine aggregate inventory-shape counters and fill/prepare byte totals agree, with no new SQL fallback. Caddy successful shapes589=Alias384+Direct204+Empty1; etcd861=Alias622+Direct239. The native_text_inventory_prepare/34 inclusive bytes decrease exactly by the eliminated native_text_inventory_prepare/43 census bytes. The separate build_prepare/43 is RepoMap verify_and_seal and is unchanged. This establishes actual removed traversal; it does not certify per-owner identity or final full ACK. Caddy intermediate deadline cancellation and etcd refusal remain failed SDK bodies. The cardinality sidecar additionally avoids the packet reread while preserving logical reads and charging its new integrity seal; the original append logical edit fee is unchanged.

### Original final-copy SDK and next owning boundary

FAILED: existing original `run-original-sdk.py` ran both corpora with immutable daemon `a00a3d738876a3867e266835781226adc9bfbcb1e67bdfa7a34363f8712732f8` and helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`, unchanged Work536870912B/R-W-M1000000/files30000/SDK60. Caddy usage R828013/W479737/B536870700/M639214 +298B map bootstrap-frame denial. Etcd usage R317834/W479594/B536860274/M699544 +17847B original posting logical-edit denial. Both trace ingests COMPLETE_DIAGNOSTIC, health[], open_requests0 and exact trace hashes; full publication failed, query/reopen NOT_RUN. Off is BLOCKED at27.909668>8.0/body0. Candidate paths include `pair-binding.json`, `input-binding.json`, `sdk-results-summary.json` and complete per-corpus traces outside checkout.

Read-only producer→append→packet review found no justified fee-removal patch: `PostingTermKey::copy` is a real memory copy but WorkBytes0; append's first loop preserves logical edit admission before effects; owner/ref packet dictionaries and ordered packet fastpath already exist. Unordered packet recopy is a real possible cost, but current trace has no unordered count/bytes and does not establish a net saving. Storage map bootstrap and earlier Source/FileMap work remain the existing owners' boundaries. Do not introduce a new engine/fee discount or rerun an unchanged failed SDK candidate.

### T-PAIRED-EOF-EPOCH

The independently added regression initially hit compiler101/body0 due to three unused atomic fetch results. Only explicit result bindings were added to that test leaf. Actual behavioral RED `57b54883-d986-42b3-991f-5868b3dcf975` then executed2:1 PASS/1 FAIL/561 filtered/exit100, compile50.34s/body0.025s. `paired_eof_rejects_stale_sql_even_when_packet_payload_is_unchanged` accepted empty EOF after a same-payload SQL UPDATE; the valid Top-write/no-Work/no-allocation oracle passed. Root cause: both CaseCursor eof flags bypass refill, and PostingCasePages::next checked poison but not the sealed source epoch. The minimal owning fix calls existing `check_page_source` before output allocation on every next; it charges no new Work and allocates nothing. No digest/CAS/EOF validation is removed. VERIFIED: full TextSpool owning regression `72852bce-92a0-4a68-87f3-c9c04d60320c` executed86/PASS86/477 filtered/exit0, compile31.03s/body13.325s. Both new EOF oracles pass; scoped formatting/whitespace checks exit0. This is a narrow final-fix result, not a fresh combined439 pass or original SDK qualification.

**Concrete remaining SDK boundary:** final-copy Caddy FULL reaches the155th MapApply batch:26,893 inline values/26,893 child refs/1,549,252 input wire bytes. This failed MapApply consumes60,689,805B while previous154 successful map applies total5,854,066B. Terminal at existing charge_bootstrap_frame. Review sorted bootstrap frame lifetime/copy before proposing a Storage hunk; existing Storage owner retains this boundary. Etcd FULL is before postings: inventory prepare865 spans, diff780; original logical edit admission refuses at packed_runs.rs318. Text census saving is real, but no trace-based net saving from a new packed-writer algorithm has been established. Inclusive/overlapping child Work is not summed or subtracted into fictitious exclusive costs. Exact selector names matter: native_text_inventory_prepare/43 census is removed; build_prepare/43 RepoMap verify_and_seal is unchanged.

### Final EOF release dependency, source-ready required

The same existing no-default optimized daemon+helper command was attempted after the EOF guard, candidate `/private/tmp/qidx-sdk60-text-copy-ym3jb7a9/eof-guard`. Build FAILED compiler101/body0 after24.756s, not a Native SDK body. During the interval the existing Storage owner changed map.rs/diff.rs/node_wire.rs/original_lookup.rs/value_codec.rs and added map/retained_value.rs. Its pub(super) RetainedMapValue::Many exposes private ManyChildren (`retained_value.rs:25`, definition:11), rejected by deny(private_interfaces). Text does not overwrite that owner's production hunk or loosen the warning gate. The new snapshot contains2802 inputs; preserve failed release log/result outside checkout. Final EOF SDK is BLOCKED by this source dependency; once corrected, recapture the exact admitted source and run the same release/SDK chain. Do not repeat this unchanged compiler failure or promote the earlier paired-cursor binary to the guard revision. Current goal remains active; the SDK resource failures are unresolved.

The Storage owner corrected ManyChildren to pub(super); Text did not edit that production file. The specific compiler cause changed, so release r2 was admitted with a fresh2802-input before snapshot and the same canonical command. No production input drift was observed at the subsequent read-only check. This supersedes the preceding source-ready blocker without deleting the failed attempt; final SDK remains NOT_RUN until the new pair is bound.

### Final guard pair bound; original SDK in flight

VERIFIED build scope only: release r2 exit0/114.572s, before/after2802-input set/hash and HEAD equal, no compiler-error bypass. New immutable daemon `/private/tmp/qidx-sdk60-text-copy-ym3jb7a9/eof-guard/quanta-index-searchd`:14,648,832B/SHA256 `c394552525b0051e3aa3de3c3a492f882ef1f9147e0e0628419f935d399a1f1b`; helper2,890,928B/SHA256 `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. This pair includes the existing Storage owner's compact retained-value work in addition to Text EOF guard. Do not attribute a new SDK prefix delta entirely to Text. Static review found no logical tariff/cap deletion: bootstrap frame/backing still charges actual size and live copied bytes, multi-child Arc has Original backing, public full-width materialization is charged. Storage runtime/unit qualification is not established by this static review.

Original Off preflight on the new pair is BLOCKED at load27.558594>8/body0/exit1, no override. Original Record was then admitted and runs both corpora using the same unchanged runner/helper/inputs/caps; results pending. Build compilation is not product SDK acceptance.

### Final guard original SDK terminal result

FAILED: original runner Record exited1 after both real bodies and returned the shared mutex. Caddy37.55s terminal R874650/W488784/B536848780/M652038 plus38312B at Catalog managed_install.rs108. It completed all155 MapApply calls; the previous copy candidate's155th map apply had failed. Etcd20.87s terminal R317894/W485357/B536770775/M699544 plus139551B at packed_runs.rs835;790 owner diffs completed versus779 in the preceding candidate. Both FULL ACK0; query/reopen NOT_RUN. Both diagnostic ingests COMPLETE_DIAGNOSTIC/health[]/open_requests0, with exact saved trace SHA verified. Immutable binaries and diagnostic input hashes stayed fixed. Original input manifests/all files/commit/query tokens were rechecked after execution.

Correction: etcd835 is sort_work admission before sort and measured_copy864, not an executed second-copy charge. Removing only the second copy leaves that refused charge. No packet-level counters establish how much prior unordered recopy could be saved. Offset-index migration would require writer/reader/seek/merge/digest/fence/bijection validation changes, Original-funded bounds and measured index/SQL/read amplification. It remains conditional, not adopted or mandatory scope. No fee/cap discount or unverified packet codec was written.

The final SDK after snapshot has2802 inputs but seven Storage-owned paths differ from source-before-build-r2; `sdk-results-summary.json` explicitly records source_inputs_unchanged=false and every path. Exact source before/after build matched, so immutable candidate execution remains observable, but it is not current-source acceptance. Those owner edits are preserved; Text does not overwrite or repeatedly rebuild an actively changing dependency. Current Text source/tests remain ready at their stated scopes; full-product512MiB/quiet60 acceptance remains failed/blocked, not complete.

### Goal continuation: current owning rail and packet cost observation

The preceding goal turn made code and execution progress, not completion. Current HEAD remains ade5fadc1a084fbb81eba831861e996081d80867. No earlier Rust/SDK process remained live at continuation preflight; Storage production files then matched the prior SDK-after snapshot. The next combined current Storage+Text+Runtime lib rail was actually admitted, but exited compiler101/body0 after236.938s at new Storage cfg(test) retained_value_cas.rs34: Arc::clone type coercion. A test-only self.0.clone() correction preserved all grant/CAS assertions. Storage-only lib then exited compiler101/body0 after40.169s at four unused atomic fetch results in that same test; explicit return bindings and file-local formatting fixed them. No Storage production hunk, cap, warning gate or oracle was changed by Text.

T-PACKET-COST-OBSERVATION is selected before any offset-index codec: current traces cannot separate completed sort tariff from full row-body recopy. Existing DiagnosticContext/WorkBudget APIs can report accepted Work in NativeTextPostingEdit lanes36(sort),37(actual recopy),38(unordered packet/SQL outcome) without Core/schema edits. Off must run the original operation directly, and Record must preserve Work/result/context. Existing 5-row fixed fixture independently requires365B sort and50B copy, with identical CAS/order/EOF and no new spans for the existing ordered fastpath. The new test-only oracle was added to ordered_packets.rs; its first attempt was compiler101/body0 from one unused DiagnosticSpanIdV1 import, removed without changing the gold. The exact selector RED is currently admitted/queued on the existing lane/mutex; no production observer has been implemented before the behavioral RED.

No existing counter means internal packet row count; SqliteRows and PostingMutations must not be repurposed. Body-copy observation is useful actual work evidence but alone does not qualify index/read/SQL net savings or product acceptance. A new private packet format remains conditional; existing logical sort charge is preserved.

### Packet observation RED and current Storage/packet GREEN

The first exact selector omitted the posting_packed parent module and run5896668d-a948-4ed7-b044-1e40c9b4abc0 executed0/filtered564/exit4; this was NOT_RUN, not a behavioral RED or pass. The corrected unique selector `test(unordered_packet_observation_separates_sort_and_real_row_copy)` actually executed1/FAIL1/563 filtered/exit100 in run560e24e4-decb-40be-ad02-b5f8d10c9b6a (compile0.59s/body0.063s), expected three packet phases versus actual0.

Parent then added lexical-only observe_packet_work and factored the existing unordered body into finish_unordered. The original sort_work charge, strict sorting/duplicate checks,128KiB memory grant, measured_copy, dictionaries, digest, SQL insert and reset order are preserved. Diagnostic Off calls the original closure directly; failed span admission records trace health and still executes the original operation. Scoped contexts restore on result/err. NativeTextPostingEdit/36 is accepted sort Work; /37 is actual body recopy; /38 is the entire unordered finalization and SQL outcome. /38 includes /36 and /37 and must not be added to them or called SQL-only cost. No Core counter, wire, tariff, cap or published policy was changed.

VERIFIED owning command: `env CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-storage -p quanta-index-lexical --lib --no-default-features --locked --offline -E 'package(quanta-index-storage) | test(native::text_spool::tests::posting_packed::)' --test-threads 1 --no-tests fail --no-fail-fast --status-level fail --final-status-level fail`. Run961e4db1-2446-4dd7-a7d4-abeba87ba925 executed240/PASS240/542 filtered/exit0, compile1m47s/body30.609s. This includes current Storage lib and new observer365B/50B independent gold, ordered fastpath absence and identical Off/Record Work+CAS/order/EOF. Actual corpus packet-copy measurement remains NOT_RUN until a new bound SDK pair; no claim that observability removed more bytes or fixed SDK acceptance.

### Object-only FULL row-count inference (not a new counter)

Source inspection establishes the original native_corpus FULL request precondition: `scale/native_index/corpus.rs` creates a new workspace, reconfigures only profile on genesis, then stages every SourceAdvance owner with expected=None/next=Some(source_ref). Text push_change checks absence of the prior contribution against admitted prior source, and PostingEdits::push accepts only Option<ObjectRef>, rejecting unchanged values. Thus reached FULL posting rows are one-sided None→Object, not arbitrary inline or warm updates.

For those completed unordered initial packets only, existing formulas give sort Work per row=71+term.len and compact copied body=8+term.len, so `(native_text_posting_edit/36 WorkBytes - /37 WorkBytes)/63` derives the row count. Require exact divisibility, both child spans OK, parent packet/38 OK, and ancestry under that fresh FULL request. The fixed5-row oracle gives(365-50)/63=5 independently. This is a source-bound derived count, not a directly emitted counter and not authority to skip validation. Warm Object→Object updates have difference94; mixed typed rows only bound ceil(D/94)<=N<=floor(D/63). Inline test inputs invalidate this object-only formula. The original FULL source preconditions must be rechecked with runner/helper/source binding.

The added write-refusal observer oracle keeps accepted bytes575=365sort+50copy+160dictionary, exact Writes denial, unchanged SQL epoch, same Off/Record usage, restored parent context and successful fixed cursor/EOF retry. Current full Lexical/Runtime lib rail is live; this new test is not yet declared passed. Storage all+packet240 pass is separate scope and is not upgraded to current full441 or SDK success.

### Current full Text/Runtime regression and new bound observer pair

VERIFIED: current combined Lexical/Runtime owning selector executed441/PASS441/738 filtered/exit0 in runa3b61279-86d0-45de-a13e-6f99f45c5f12, compile4m57s/body132.252s. It includes the new SQL-write-refusal/retry observation oracle: accepted575B, exact ResourceLimited/Writes, unchanged SQL epoch, restored diagnostic parent, ordered/CAS/EOF retry, equal Off/Record producer/denied/final Work. This replaces the earlier separate437+86 current-source description for this Text/Runtime scope. Storage all+packet240 pass remains separately scoped and is not added to441 because packet tests overlap.

VERIFIED build scope: `/private/tmp/qidx-sdk60-packet-work-r4dxzzgz` no-default optimized daemon/helper build exit0/201.241s (compiler3m19s), exact before/after2803-input set/hash and HEAD equal. Immutable daemon14,648,832B/SHA256 `9e40b0045b5b593fc66d11e5f12fcdc6d3bb02715fd87025b9f8aa4e62afbb4c`; immutable helper2,890,928B/SHA256 `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. Original driver/corpus/gold/SDK60/Work512MiB/R-W-M1m/file30k and Original scopes remain unchanged. Off preflight BLOCKED/body0 at host load32.425781>8 and14 foreign Rust processes, no override. The existing runner Record is now admitted for both original corpora; body/copy measurements are NOT_RUN until its actual terminal output is recovered.

### Packet observer original SDK terminal and measured format decision

Candidate `/private/tmp/qidx-sdk60-packet-work-r4dxzzgz` Record runner exit1 and mutex returned. Both real original bodies failed `REQUEST_DEADLINE_EXCEEDED` at native_corpus_publication_status (Caddy67.58s process, etcd79.10s process, each SDK60 unchanged). Full ACK0/query/reopen NOT_RUN. Caddy observed usage386,125,553B and etcd496,323,772B, both daemon first_denial cancelled at Storage object.rs865. These are deadline failures during contended Record execution. Off was preflight BLOCKED/body0. Exact source2803/HEAD, both binaries, diagnostic inputs and original full corpus file hashes/commit/query tokens match through SDK.

Caddy raw footer invalid:229,039 attempted,228,666 accepted/written,373 dropped,4 open. Missing begin for span9501 triggers strict parser error at thread_cpu_ns seq50985. Preserve trace as failed diagnostic, do not adjust schema or salvage savings from orphan events. Etcd canonical ingest COMPLETE_DIAGNOSTIC, health[], open0, trace digest6d6855b796d65a9e76b19486c7e257050c9b14bf2c6d1a701aee3594fa32d8e0. Full request span4553 contains117 completed unordered packet/38 parents, each with checked successful sort/36 and recopy/37 children. Sums: sort16,365,685B, body recopy3,304,273B, inclusive packet27,188,860B. The original FULL object-only source and exact per-packet identity `(T−B)/63` derive207,324 rows. Existing insertion/dictionary charges recover57,440B dictionaries; checks pass for32B units and128KiB packet bound. Hypothetical u32 offset index write/copies/first read cost16*N, giving net −12,911B before added readback validation, fragmentation, or later reads.102/117 packets are already negative. Keep current packet wire. Frame these as the completed observed prefix, not extrapolated full-corpus savings or a bytes refusal.

### Borrowed term packet read

The existing term metadata consumer copies the SQLite payload into a temporary Vec, scans it once for order/count/EOF, rescans it for first/last fences, then decodes selected terms and copies every selected term into a cursor scratch key. A 64-term packet of nine-byte keys contains 1,770B, and the full page copies another576B solely to advance a cursor that returns at the page boundary. All 64 returned owned keys and their count integrity hash remain necessary. The scoped reader will borrow the SQLite row payload, verify both fences during its original order scan, and keep the packet inside one callback. SQL EOF is checked before returning tentative owned data. Cursor bytes are copied only when needed for a subsequent packet query. A manually assembled cardinality page can resume from the authenticated expected key it just matched. Preserve SQL read1/4096B per lookup, logical cardinality reads and Original allocations for the returned keys/sidecar.

The two fixed regression selectors are `sealed_term_page_read_charges_only_packet_lookup_and_count_integrity` and `manual_cardinality_page_across_packet_fence_charges_two_lookups`. Expected old versus intended deltas are B8599→6253/R1 for the sealed 64-term page and B11750→8192/R4 for the adjacent manual 063/064 lookup. The pre-change owning RED request is admitted through the existing execution queue and is waiting behind a live Native process build. These are source-derived golds until the actual body runs. Directory alias also has a separately observed owned page key copy before posting staging. Current traces only bound that duplicate by the 4,713,524B sum of all unique inventory keys in the observed etcd prefix. Its borrow API and before-EOF callback effects remain separate scope.

Actual term metadata RED `167ed586-2a06-4dba-a444-8f0055679451`:2 executed/2 FAIL/565 filtered/exit100 after441.935s admission and46.73s compile. Observed exact8599 vs6253 and11750 vs8192, with expected read checks preceding byte failures. The new borrowed TermPacket validates both SQL fences in the order/count/trailing-byte scan, then consumes within a private HRTB SQLite callback. SQL footer is checked before returning owned page/deltas. Actual BLOB backing and redundant cursor copy are removed. Original SQL and logical cardinality charges, source stamp, key/delta integrity and independent posting proof remain. TextSpool run `99268e09-8a30-4051-b8fa-eb59f15c691c` executed90/PASS90/477 filtered/exit0, admission90.858s/compile49.30s/body19.864s. Verified physical-copy reduction2346B/3558B on fixed fixtures with same read counts. This does not claim original corpus completion.

### Borrowed Directory alias descriptor rows

The existing canonical 514-term Directory/reopen fixture measures page-cursor and alias visitor Work independently. Both perform two physical child gets; 514 fixed five-byte keys and two five-byte fences require independent seal tariffs `514*(5+8)+2*(5+43)=6778`. The reference owned MapPage cursor copies2570B of keys. The new alias path borrows the authenticated same keys, so expected alias minus page Work is4208B. Actual RED `a9ae158c-5a7d-4ea8-ad5e-e179ae80907f` executed1/FAIL1/566 filtered/exit100, compile39.89s/body0.052s, old6778 !=4208 after both read-count assertions passed.

Production Directory Cursor now lends each fully checked descriptor and same-block key/count to the cold alias visitor without reserving/allocating/copying a temporary public MapPage. Its existing advance still reads and validates descriptor order, packed child, interval and fence, or leaf count. The alias visitor keeps first-child comparison, per-descriptor and per-key seals, term/anchor bounds, EOF and commit ordering. Generic owned MapPage and its paid copies remain for callers that require ownership. Cursor cannot advance while the borrowed descriptor is live. The cold caller stages only disposable posting edits before final seal; its existing failed-window invalidation remains. The current combined Lexical/Runtime rail is live. Original SDK for this new production revision remains NOT_RUN.

### Current combined dependency status

The combined Lexical/Runtime request exited compiler101/body0 after455.515s holding the shared admission. Runtime test `publication_sqlite_vfs/tests/read_only.rs:847` imports CommitSha from the wrong module (E0432). Production `native_recovery.rs:332` omits the new LegacyHistoryBootstrapRequired refusal variant (E0004). Existing NC-D boot packet assigns the recovery receiver to D; its current owning Runtime request is live. Text does not take over the typed Recovery wire mapping or treat compilation as a test body. Current Text files match the captured borrowed-term and borrowed-directory hashes. Lexical independent scoped owning nextest is submitted on the existing shared lane, ahead of source-bound final release and original SDK reexecution when the production dependency compiles.

The owning Lexical-only request terminated admission124/body0 after600.003s. The admitted History/Runtime request remains live (PID20089 and actual Cargo child20193); do not count a queue receipt as a test. The production exhaustive refusal arm is now present in native_recovery.rs with explicit LEGACY_HISTORY_BOOTSTRAP_REQUIRED code. The test-only CommitSha root import was still unchanged at this observation.

Current existing canonical release candidate `/private/tmp/qidx-sdk60-scratch-projection-r3-jmmd981i` is owned by the shared integration queue (PID30248 child of build-bound.py PID30164). Its source-before snapshot binds2803 inputs and exact current packed_runs.rs/directory.rs/occurrence.rs plus current native_recovery.rs. Reuse this live request by inspecting terminal exit/artifacts and source-before/after binding. Do not launch duplicate optimized build or claim SDK based on this queued artifact. Restart Lexical-only verification after a genuine execution admission transition. Whole current Text/Runtime and original-sdk acceptance remain open.

The existing scratch-projection r3 release reached terminal admission124 after600.647s with zero compiler artifact events and no binaries. Its build-result lists7 source paths changed while queued and diagnostic input drift. This is BLOCKED admission with SDK NOT_RUN. Both specific Runtime source defects have now been corrected by their existing writers: explicit LEGACY_HISTORY_BOOTSTRAP_REQUIRED terminal arm and `quanta_index_contract::lex::CommitSha` import. Current combined owning nextest session76029 is submitted after those changed inputs. Its execution-lock wait is1200s to allow the already-live product/history jobs to return the same shared mutex; SDK60, Work caps, fees, jobs2, selectors and Original contracts retain their original values. No concurrent paired release is live at this capture.

The shared integration queue submitted replacement candidate `/private/tmp/qidx-sdk60-projection-admitted-vtxkvcin` (live resource controller PID74730). Its build script captures source only after canonical admission rather than before waiting. At observation the root has only the script and no approved candidate binaries. Retain this existing release request and inspect its terminal plus actual Text hashes, before/after and SDK custody. Current Text combined request76029 remains live behind admitted Q ownership; no duplicate pair or SDK has been started by Text.

The admitted optimized build `/private/tmp/qidx-sdk60-projection-admitted-vtxkvcin` finished compiler exit0/191.560s with both pinned binaries. Before/after2803 workspace inputs differ only in `crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs`, a test module outside either release target. The recorded `release-target-binding.json` verifies the release target dependency set and hashes; whole-worktree build snapshots differ. Daemon SHA256 `02f923cf5ef991d86784f162682203f92cce7c7810dbba6acc7e4925cc61346c`, helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`; rehashed copied artifacts match. Both Text production hunks and their test leaves match the actual admitted source-before. Original driver SHA is unchanged; Caddy590/3272107B and etcd1421/8666217B source manifests/all file hashes/commits/query tokens rechecked. Existing shared original Off→Record SDK controller PID53466 has been submitted on the same mutex with1800s admission wait. At this observation it is live and waiting. No SDK test result yet. Text combined session76029 remains live in the same queue.

The corrected-input Text+Runtime combined session76029 reached terminal admission124/body0 after1200.012s. The independent release ran and matched both production target dependency sets, but no current combined regression body has executed. The existing original SDK controller53466 is still live at this observation; monitor it rather than duplicate SDK or repeatedly submit the same owning tests behind the same active lock holder. Resume the Text+Runtime owning selector when the queue has an actual execution transition.

### Bound borrowed Text / scratch projection SDK terminal

The original shared SDK controller terminated after both actual Record bodies. `sdk-exit-codes.json` is off1/record1. No Off output roots and no corpus full result/reopen outputs exist. Both traces pass canonical diagnostic ingestion with health[] and open requests0. Caddy first denial bytes536848332+58652/536870912 at Catalog managed_install.rs108 (R878980/W497088/M660342). Etcd first denial bytes536771969+138267/536870912 at packed_runs.rs895 (R317984/W494194/M699544). The latter is the original sort_work admission before sorting and before a new body copy. Both BuildPrepare ResourceLimited / FULL ACK0. The preflight failure is separate from SDK Record byte refusal. No timing/cap overrides or new rates were used.

Whole workspace before/after Release changed only ANN integration test ann_quality.rs; before/after SDK changed only History integration test history_native.rs. These are outside both selected release target dependency graphs. Verified binding is limited to the optimized daemon/helper plus original SDK inputs/driver, not a frozen full-worktree test suite. Code/provenance and first refusal come from the actual release-pair artifacts, not the prior contended deadline sample. Existing current Lexical owning request64152 was submitted after the production release+SDK moved into actual execution; this verifies the isolated Text/Directory scope first. Runtime15 and complete NTX4 product acceptance remain outstanding.

R4 Caddy compared with the prior eof-guard refusal: owner prepares593 and diffs590, all155 MapApply/3 completed with the same input shape. Inventories/diff prep Work falls1,107,087B and postings/35 prep falls1,752,865B. The same final256-input reservation reaches2,859,504B more accepted Work before refusal, so BuildPrepare inclusive total differs only448B. Both are partial reservation prefixes; per-object payload/edge counts are not exposed and do not establish a fee or extra-work regression. R4 Caddy completed114 unordered posting packets and actually recopied3,162,301B. A u32 index costs at least16B/row through initial creation/SQL write/first read, net −69,827B for202,008 rows.

R4 etcd compared with the prior packet-work deadline prefix: same861 completed owner shapes (alias622/direct239). For the first606 completed inventory diff spans in producer order, observed Work fell from124,366,408B to121,343,195B (−3,023,213B). The complete terminal prefix has803 completed owner diffs plus one error. First117 completed packet Work is unchanged; R4 completes166 (294,152 inferred rows, physical body recopy4,664,794B). The167th packet is refused before sorting/recopy. u32 index net through initial first read −41,638B on those166; u16 absolute byte offsets do not represent all128KiB packets and apparent savings omit reader validation/repeated reads/fragmentation. The observed higher progress is not a full corpus Work total or proof that one hunk caused every measured delta. Keep the current packet format and original tariffs.

### Paired case cursor duplicate digest

Source review isolated an actual local rehash in `CaseCursor::fill`: the freshly constructed `packed_term_page` already computes the count seal, then `fetched.check_counts` hashes the identical private page before any consumer or mutation. Candidate change removes only that immediate verification pass. Preserve the sealed-source epoch checks, key/surface ordering, original producer seal, Rc source equality on `push_from`, result paired seal, cardinality-side integrity check, and dense original-stream proof. A fixed two-case oracle was added to `paired_eof_epoch.rs`, selector `paired_page_reuses_untouched_case_count_seals`, test source SHA256 `d40726aea041a5cac663beeb76ee6e4470ac238a3aa70e7d1f823f6bb5d2c1e2`. It seals the producer before snapshot, requires R5/B20746 from five SQL calls, two 74B producer seals,103B paired result seal and three5B cursor copies, plus fixed keys and DF1. Old expected B20894 is source-derived until executed. The live Lexical session64152 was still queued and its Rust body had not started before the test was frozen. No production digest change has been made yet.

Actual case digest RED and Directory owning evidence: nextest `6133ff28-3a4e-44b4-9ef8-4cf0c1b9873f` admitted after715.024s and executed429 across Lexical lib, 428 PASS/1 FAIL/139 filtered/exit100, compile34.02s/body22.340s. Sole failure `paired_page_reuses_untouched_case_count_seals` shows20894 !=20746 with reads5 already matched. It isolates exactly148B (two unchanged74B digest passes). Directory's borrowed alias/reopen fixture and term BLOB fixtures were within the passed428. This is not a429-pass claim.

Applied the one-hunk `CaseCursor::fill` production change: reuse the freshly sealed cursor-owned page, retain source/epoch/case ordering and `push_from` provenance checks, output seal and ordinary cardinality validation. No original logical charge is changed. The prior R4 original SDK failed before this edit and does not cover the new revision. New Lexical owning session8551 submitted the same exact429 scope on the shared mutex after RED, production source frozen. Runtime owning scope and source-bound original SDK for the final revision remain outstanding.

### Final case-seal release request

The post-fix Lexical owning request8551 is still live. No optimized daemon/helper build or original SDK request was live at the submit check, so the changed production revision was submitted once using the existing admitted build-bound producer from the previous candidate. External root `/private/tmp/qidx-sdk60-case-seal-n24abmut`, session94796, same canonical mutex, wait1800s. It takes the source snapshot inside admission and pins both compiler-emitted artifacts before release. The expected case-source SHA256 is `5e9302f129213d94901f62c554b6ba3a35f35aee42f57db97ae7a5b4e693f8b4`. The source-bound optimized build and its original SDK are NOT_RUN until their actual execution evidence. No new lane, build mechanism, fee, Work cap or SDK timeout is introduced.

The case-seal release request94796 terminal exit0 built pinned no-default Native daemon/helper in248.175s. Canonical admission168.830s, held249.979s. All2803 workspace inputs and HEAD match before/after build, as do diagnostic inputs; source includes the exact case hunk. Daemon SHA256 `038ad043d29d62574d3578fd818eb358ebc4936b94ce3ed260f382fd2d553ea0`, helper `3b1a819a27b50931af0db2f17fd2e0fddf30cd475ae7968979cd7521b67d06b7`. Existing original runner with the fixed source manifests/input binding was submitted as session5997 after checking no duplicate live original controller. It reuses the admitted Off→Record wrapper on the same mutex. This candidate includes separately owned Literal and recovery changes, so any SDK delta cannot be attributed solely to the Text rehash hunk. Tests8551 and SDK5997 remain live. Actual bodies are pending.

Catalog denial follow-up (read-only): `reserve_attempt` calls the canonical registration check during child preflight (`managed_install.rs:791`) and again before mutation (`:807-814`), with a full children clone in the canonical helper. This is an actual memory copy repeated within the Catalog-owned transaction. It happens after `charge_object` and does not shrink the existing admitted payload/edge tariff. `encoded_object_len` writes to a length sink, so there is no transient JSON Vec to remove. Same-transaction child proof reuse is already wired. That copy is a separate CPU/RSS opportunity for the existing Catalog owner, with no supported claim it fixes the 512MiB byte refusal. No new Text or Catalog production patch follows from this finding.

The post-fix Lexical request8551 reached terminal admission124/body0 at1800.018s. It adds no case-seal GREEN or429 PASS evidence. Original SDK session5997 remains live, controller PID1513; at this observation a Kernel/Runtime integration command owns the canonical mutex with actual Cargo/nextest descendants. Preserve the pending SDK and avoid re-submitting the Lexical body while the same contended admission state remains. Before retesting, verify a real admission transition and the current Text source. Candidate SDK-before snapshot and build-after match all2803 inputs, and an independent workspace check during the wait found no drift. Current completion still requires actual post-fix unit/integration bodies and original corpus acceptance.

### Case-seal release original SDK terminal

Original SDK session5997 exit1 after actual admission989.240s and held117.219s. Off preflight blocked by host load18.807129>8 and6 foreign Rust processes; zero Off bodies. Record executed both original files and captured typed Work refusals under file30k, bytes536870912, R/W/M1m and SDK60. Caddy process68.08s, Work R881148/W500492/B536860391/M663746, attempted52768B at Catalog managed_install.rs108. Etcd process26.01s, Work R317984/W494194/B536771969/M699544, attempted138267B at packed_runs.rs895. Both FULL ACK0 and search/reopen NOT_RUN. Process wall times are not quiet SDK timings. Etcd usage and denial match R4 exactly, consistent with the last case rehash fix occurring after the first refusal. Caddy capture footer invalid, attempted271909, accepted/written267114, dropped4795, open2; strict parser reports owner-span loss. No whole-prefix Caddy Work delta is attributed from the invalid trace. Etcd COMPLETE_DIAGNOSTIC, health[], open0, trace digest rechecked.

Canonical workspace snapshots before/after build and before/after SDK all match exactly2803 files and HEAD. Diagnostic inputs, both immutable binary hashes, both original manifest/commit/query/file bindings were rechecked. The combined Lexical+Runtime no-default owning command (same Text/TextSpool/Occurrence/Literal plus Runtime dense-install selectors) entered the common mutex immediately, session32393. It ran445 tests in92.862s with444 PASS, one FAIL,739 skips, and exit100. The only failure was F's `native::literal_blocks::tests::shared_owner_prefix_wire_preserves_full_logical_keys`, so the paired count seal, Directory alias, term packet and dense-install tests in this selection passed. The terminal summary preserves the failing name but no assertion text. With the mutex free after the separate F release attempt, the same-package exact Literal selector was admitted in session58481 to obtain the failure or confirm the current hunk. The F build result at `/private/tmp/qidx-sdk60-literal-v4-0ni8ypt2/build-result.json` is exit101, no pinned binary, SDK60 NOT_RUN. It supplies no new SDK result.

F then recorded the current Literal V4 nextest `8768d533-71d4-4d3f-873d-4e7236b7adbe` at41/41 PASS including the exact failing selector. The obsolete duplicate diagnostic58481 was stopped during dependency compilation with exit143/body0 to release the mutex to F's existing release retry. F's retry includes the Catalog SQLite INTEGER read fix and is bound at `/private/tmp/qidx-sdk60-literal-v4-ci2rwa01`. It reuses the existing build + original SDK custody and preserves original Work caps. No new optimized build or SDK runner is started by Text while that controller is live.

The integrated Literal V4 retry compiled in102.128s with Cargo exit0 and both pinned artifacts. The candidate controller stopped before SDK because five workspace inputs changed during the build. The daemon's release dep-info includes four changed production Kernel paths (`lib.rs`, `boot.rs`, `update.rs`, `update/history.rs`) and excludes the fifth test-only Runtime file. The V4 input hashes match, but this is material daemon source drift. The candidate has SDK60 NOT_RUN and R5 remains the latest executed original SDK. F's Literal persisted-seal rail is admitted on the common mutex.

The original driver subsequently executed Record only for both corpora on the immutable compiler artifacts in `/private/tmp/qidx-sdk60-literal-v4-ci2rwa01`, daemon `b33b3a2c36cc93d114d952530fdc6814c9b702a495e924235b97bdef20f85a8e`, unchanged helper. The command unset Work and contended-timing overrides. Both traces are COMPLETE_DIAGNOSTIC, health[], open requests0, and their computed digests match the ingests. Caddy reached the same155th Text map batch and input census as R4, then refused B536829657+48028 at Catalog managed_install.rs108 with R916370/W523129/M686431. Etcd advanced from803 to862 completed inventory diffs and reached a fifth inventory map batch, then refused B536870908+68 at scratch edit_page.rs112 with R333371/W534085/M868115. Both FULL ACK0, query/reopen and Off NOT_RUN. This diagnostic does not resolve the build-time source mismatch.

The complete Literal preparation stage fell from51,151,663 to42,026,185B for Caddy (R4), and186,711,399 to143,008,445B for etcd (R5). Fresh persisted child verification fell from92 to53 children and12,001,971 to8,201,871B for Caddy, and384 to198 children and52,349,516 to34,119,623B for etcd. Child verification and other inner stages are already in the preparation totals. Caddy consumed about10.16MB more in the same failing MapApply/5 prefix, consistent with progress after the Literal saving. Etcd's four completed global inventory checks retain the same35,170 reads/9,137,041B, while the fifth batch stops before a completed global check. The minimum68B refusal is before construction of a retained edit, not an executed68B copy. Keep the existing Work charges and compare producer readback, local per-root construction and global input custody before changing a cursor or storage contract.

`InventoryCursor::next_edit` reads one bounded SQL page and retains `after` by `Arc` clone. The root-specific plan and global cursor in `entry_patch::apply_inner` do decode the same frozen rows separately. Their stamp comparison, global order/CAS and EOF checks, and fresh installed-directory readback are required. Eliminating either scan requires an independent replacement for its proof of the original ordered stream. First freeze a fixed Work RED plus wrong-root, reordered, malformed, and stale-epoch refusals. Keep slot, anchor, root, value, and installed-object custody; dropping the global scan or its charge with no equivalent proof is not a candidate. Separately, compacting per-row Arc/Vec into owned page backing belongs to the shared Core/SQLite contract, and copying an already canonical row wire could save decode/encode CPU while retaining the same owned bytes and Work admission. The current trace does not establish a net Work gain for that separate change.

### Frozen compact inventory verification

Preimages `entry_patch.rs` a0d3d481 / `entry_map/directory.rs` b0eff5e8 were rechecked before editing their Text-owned digest/producer functions. A descriptor seal now authenticates the existing `OrdinalMapEdit::next_wire` instead of re-encoding the identical `MapValue`. `RootInput::next` no longer decodes object values immediately after `Edits::push` has already checked and sealed them. Directory producer CAS/presence tests use the canonical cursor's optional wire slices before the same descriptor/object checks. The transcript, serialization, Work charges, per-root/overall scans, stale-epoch and persisted checks remain intact. Canonical no-default Lexical `test(native::text::)` nextest `d025081b-ed8f-4ed7-ae77-41b477b9bba3` ran268/268,301 skips, exit0, compile90s/actual bodies5.978s. Source hashes `entry_patch.rs` 46b46d82 / `directory.rs` 83c07e9b. The original SDK has not been rerun for these two transparent changes.

The R6 raw trace parent chains show zero descendant spans from each of the four completed global inventory verification scopes on each corpus. Those scopes are compact verification candidates, with exactly15,089R/3,940,716B on Caddy and35,170R/9,137,041B on etcd. Those totals include untouched digest/admission/readback and are not estimates of removable work. The next change retains the independent ordered global SQLite read, but uses borrowed row values for digest, CAS/order/count/EOF and fresh persisted readback, with one bounded previous-key copy. Compact verification does not emit a retained edit. Tree application and unbound generic callers retain the existing owned cursor. The fixed two-descriptor production fixture already checks canonical persisted child reads and reopen; its new isolated existing phase oracle expects2 reads and4356B (4096 query + two existing125B descriptor seals + two5B previous-key copies). The focus command is canonical `test-workspace-lane` no-default Lexical lib, exact `cold_multiblock_alias_prepare_visits_each_descriptor_and_child_once`, session77907. The assertion is pending actual behavioral RED. The borrowed compact production change is pending until that result.

Actual RED `8cb09b5c-1ef5-4b88-9fa3-0cce436a810d` executed the exact production-batch fixture, exit100 with one failure and568 skips. The observed lane41 Work was2R/5072B versus independent2R/4356B; the 716B delta isolates512B of page slots and214B of owned rows, net of the two5B order-key copies required by the new path. The test held the original fee and exact output/child readback checks. The new production implementation selects borrowed rows only on the frozen cold-inventory caller with a fully compact plan and an admitted one-key order buffer. It retains the exact 64-row SQL queries and fees, extra EOF query at an exact page boundary, original per-root stamp, key and ordinal checks, CAS, ready/frozen epoch, and independent installed block/directory readback. The unchanged owned cursor handles Tree and generic callers or unavailable buffer memory. Lexical Text+TextSpool no-default nextest session3781 is the current post-fix request. Added targeted 64/65-row Work, foreign-root and stale-epoch checks; no post-fix Rust or SDK result is claimed before the actual bodies.

Current post-fix Nextest `d3fe5f70-55c2-44d4-b5d7-67e2d510d9f3`, canonical no-default Lexical `test(native::text::) | test(native::text_spool::)`, 360/360 executed PASS,210 skips, exit0, compile19.18s and bodies14.808s. This includes the unchanged4356B/2R oracle, 64/65 row boundary check, foreign root and epoch refusal. Production source SHA256 `text.rs` e5e842ed / `text/entry_patch.rs` 05546b1d / `text_spool.rs` 1f2a641b. No changed fee, cap, SDK or dependency was introduced. Text is source-ready for the next integrated pair. The separate F Catalog `managed_install_trace_tests::` nextest is actually admitted and its prepared controller `/private/tmp/qidx-sdk60-managed-proof-93kedcbg` has no live build yet. Reuse that one canonical controller if it captures these Text paths, and preserve the original 512MiB/SDK60 Caddy·etcd execution and exact build/input custody. No new SDK body for the borrowed verifier has run yet.

F's Catalog proof nextest `b5dff10a-5345-4ce3-ad69-70fb22124ec2` passed11/11. The existing integrated controller process72343 is live on the canonical queue, with both build and original SDK in the same request. Its source binding pins all four current Text paths, Catalog managed install and both Literal files. All seven input hashes were independently matched. The controller captures admitted whole-source before/after, pins daemon and helper, fences the same source before SDK, and runs the unchanged original Off→Record driver. Do not submit another candidate while this handle is live.

That controller compiled both targets with Cargo exit0 in304.302s, but stopped before SDK because the Runtime integration fixture `native_index_uds/read_sessions/query/history.rs` changed. The one changed test is absent from both exact release dep-info files. Both pinned binary hashes and all seven production bindings independently match. After the controller terminated and no duplicate build/SDK controller was live, the same controller and binding files were reused in `/private/tmp/qidx-sdk60-frozen-compact-rebind-frlwdl2i`, session27378, on the same mutex and build lane. The warm rebind preserves the full source gate rather than overriding it. The current candidate SDK remains pending actual execution.

F's existing `resume-sdk.py` ran the actual original SDK under the same mutex. The redundant warm rebind session27378 was cancelled before admission (exit143, body0). The resume verified both immutable binaries and pinned release dep-info files, diagnostic tools, HEAD and complete retained input snapshots. All2803 inputs match from build-after through SDK-before/after; the one build-time test is absent from both release dep-info files. Daemon SHA256 `7550029b4bf8cd3b8f5aeb76ac37f7ae409f41aa296635fd2d8f89f629082bd8`; native_corpus `8862af2296a29bc2010de58cc313c39ad55751481759f89d6c525db6a353f666`. `controller-result.json` reports SDK exit1, release-target binding true. Off exit1 before any corpus bodies. Original caps were file30k, bytes536870912, R/W/M1m and SDK60.

Both actual original Record bodies returned `ResourceLimited`, FULL ACK0 and query/reopen NOT_RUN. Caddy at Catalog `managed_install.rs:108` used R930368/W524351/B536856475/M687653 and requested60612B. Etcd at Storage `map/scratch.rs:115` used R345282/W535089/B536863078/M869040 and requested8192B; refused before allocating the next directory's scratch. Both strict replay files prove `COMPLETE_DIAGNOSTIC`, health[], open0 and exact trace digests. The preceding Caddy and etcd windows each retain their four exact read counts, with lane41 Work 3,940,716→2,351,650B and9,137,041→5,437,582B respectively. Every individual window reduced the expected same-read allocation cost. These are matching-phase measurements, not full corpus Work totals. Etcd completed the same862 diffs and 210 instead of180 directory map applies; the211th `/40` failed as `map_apply/0` at scratch open with zero accepted Work. The failed fifth inventory window had no `/41` completion. Reuse of the existing multi-root map producer across the directory builds is the measured next seam if it can preserve each original stamp, packing ordinal, persisted readback and all-root completion. The old per-slot scratch allocations are real work; no fee removal or SDK-pass estimate follows from the first 8192B refusal.

### Cold directory multi-root staging

Fixed production-frozen fixture `frozen_cold_directories_stage_together_and_reopen_by_original_slot` prepares two distinct514-term directories at slots0 and2 and an empty slot1, independently reopens both, checks disjoint keys, then requires one map stream. Exact nextest `d1c29436-3b80-451c-968a-2971106efffb` ran1 body, exit100/570 skips, with only actual2 versus expected1 map calls. The storage path uses one Scratch open per empty-stream application. No charge constant was changed.

The frozen Text caller collects its directory requests and sends them in slot order to one existing `AttemptContext::apply_stream` with empty bases and Nested packing for each dense map ordinal. Only one source RootCursor and one ProducerState are active. The outgoing ordinal is assigned during the original SQL row allocation, and anchored edits cross the Storage boundary without an ordinal recopy. Each root independently verifies ordering/CAS, row count, descriptors and EOF before completing its digest. Dense output roots and digests are mapped to original slots; the complete independent global stamp and persisted directory/child readbacks remain. Unbound and warm input paths retain their existing behavior. No new Storage/Core/Catalog private writer or port was introduced. Stage `/40` represents the directory batch on this production route, and Storage root build counters represent the number of directories. Because directory nodes remain provisional, the late second-root fixture checks that a count mismatch cannot return any staged map and leaves no result in the fixture store.

Canonical combined no-default Lexical+Storage nextest `f504f2e5-72ef-42c2-9f11-8b76ede414c6` passed369/369,421 skips, exit0, compile35.86s/body18.266s, selectors `test(native::text::) | test(native::text_spool::) | test(packed_subtrees::)`. Text hashes `text_spool.rs` abfa1dfc / `text/entry_patch.rs` 0772e1cb / `text/entry_map/directory.rs` 653d14c7 / fixture a0caad3c. Same existing admitted build and unchanged original SDK group submitted in `/private/tmp/qidx-sdk60-directory-batch-l8x2gr6u`, session17750, after verifying no competing build/SDK controller was live. New original SDK bodies are pending.

Session17750 admitted after53.109s, optimized build116.559s/exit0, immutable daemon `d747a40b098f14eb6c685843012914a6f4eed949710f9876fc7af0c86e7ac664`, helper `8862af2296a29bc2010de58cc313c39ad55751481759f89d6c525db6a353f666`. All four whole-workspace snapshots (2803 files plus HEAD), diagnostic inputs, and both binary hashes match. Off preflight blocked at load46.905273>8 and3 foreign Rust processes, zero bodies. Both original Record bodies ran at file30k / Work536870912B / R-W-M1m / SDK60 and returned ResourceLimited. Caddy diagnostic wall40.44s, R935743/W533073/B536853187/M696677 plus52768B at Catalog managed_install.rs108. Etcd wall15.58s, R362289/W538086/B536870399/M869734 plus5752B at Storage object.rs1840. Controller exit1, lock released, both trace digests independently verified against COMPLETE_DIAGNOSTIC with health[] and open0. Both FULL ACK0, query/reopen NOT_RUN.

Caddy built the same152 directories with root counts18/25/101/8 in4 map applications (previous152). Inclusive directory stage Work fell6,293,336→3,189,396B (3,103,940B), and all590 diffs retained exact Work. Etcd built the same first159 directories (28/28/37/66) in4 applications (previous159); their inclusive directory stage fell8,301,306→5,009,203B (3,292,103B). The fifth window completes55 more directories and its global stamp; successful inventory diffs advance862→869. R8 also contains independent Storage map/resident/scalar_pair changes. These are matched stage comparisons, not whole corpus net cost or a controlled isolation of every individual source hunk.

The new etcd terminal is in `NativeTextInventoryDiff/2 -> NativeTextInventoryRead/32`, before any posting write for that owner, with accepted R2/B11500. Source `visit_cold_inline` reuses the authenticated packed block for its first count via `read_retained`; the adapter already shares the Arc and performs no second physical payload read/copy. `object.rs:1840` is the existing logical wire admission. Do not remove it to treat the refused5752B as a physical duplicate. Caddy still needs the remainder of final Catalog installation. Product NTX4 remains open.

### Borrowed bounded inventory plans

R8 previously removed owned SQL rows only from independent global compact verification. Cold, empty-base bounded planning still used each `RootCursor` to allocate a64-slot page plus a retained `OrdinalMapEdit` for each row, even though `resolve_bounded` only needs its key and values to build an admitted layout. In the fixed two-key production-frozen `SmallMap` fixture the expected Work is R4/8504B: each of planning and global verification retains one4096B query, two76B edit digests, and two2B order-key copies. Actual RED `611800c2-c38d-4aaa-869a-4f89198142e6` executed1 body,571 skips, exit100 with9210 !=8504 and unchanged4 reads/output. The706B is the old512B slot page plus two99B owned rows minus the4B order-key copies. No first-count logical read is changed.

Root-specific and global SQL scans now share the existing frozen borrowed visitor and keep their original query bounds, row/page/EOF admissions and independent scans. A single granted previous-key backing services all bounded cold plans and is released before directory construction. The bounded draft shares the same Small/Block/Tree construction policy with ordinary planning. Borrowed rows retain canonical stamp, object-value decoding, empty-prior CAS and final count. Warm, generic and memory fallback routes keep the owned input; Tree materialization still receives the owned global cursor. The fixture also confirms a claimed existing value on an empty base is refused before map application.

Nextest `386d7837-f68f-482e-90b3-e4855a60a673`, no-default Lexical `test(native::text::) | test(native::text_spool::)`, verified362/362,210 skips, exit0, compile29.54s/body16.902s. The fixture's Work8504B and R4, mapped output refs, and wrong expected Conflict all passed. Production hashes `text_spool.rs` 02c22cdb and `entry_patch.rs` 1063c426.

R9 release builds in `/private/tmp/qidx-sdk60-root-plan-borrow-j3kuiahp` (session38198,104.027s) and `/private/tmp/qidx-sdk60-root-plan-rebind-byisbpey` (session1731,109.538s) compiled with exit0. Both controllers stopped with exit2 after detecting a change to the release dependency `crates/quanta-index-storage/src/map.rs` between input captures. Original corpus SDK body count0, Off/Record NOT_RUN. The R8 Record remained the latest executed original at this point. The prepared `resident-cold-08myshdn` request supplied no build or SDK results.

### Compact cold token rows

R8 etcd Record completed1,117 cold token fill stages. They contain1,581,486 case rows, each with a12B pair of offsets plus position. The fill Work calculation includes the actual row bytes; at 8B per row the same inputs account for6,325,944 fewer bytes. This arithmetic describes that recorded prefix and remains separate from the new SDK measurement. Retained alias reads and token/key logical fees remain as before.

Each ephemeral row now uses two32-bit words with a28-bit key start, 10-bit key length minus one and 24-bit position. The 16MiB normalized input caps worst-case key backing below 2^28, the term-key cap is1,024B, and every token run advances through at least one input byte. The actual arena grant and fill Work follow `size_of::<Row>()`. Exact casing, Unicode normalization, position order, SQL fallback and packed occurrence wire retain their existing contracts. The fixed “A a İ” body reproduced the old B88 append delta against independent B64 before the patch (RED `f85c3578-8587-4827-bf3d-2ea9910c804d`, one failed,571 filtered, exit100).

The first compact-row test run (`32b72828-46fb-4a0e-8974-3cbe5b9db82f`) executed363 bodies:359 passed,4 failed. Three fixed physical-size expectations still described12B rows. The fourth needed a smaller amount of free test memory to preserve real optional denial and exercise SQL fallback. The corrected fixed exact arena expectations are22,000B ASCII,84,000B wide keys,173B Unicode, and19,968B with19,000B free for optional denial. Logical group, position, packed wire and fallback assertions still apply. A test-only integer type mismatch in the first correction was fixed before the full rerun.

An independently admitted frozen checkout `/private/tmp/qidx-sdk60-frozen-install-j3nf1gxu` included the same Text production and a pre-update Runtime restore-forward input. Its 2,803-input capture was stable, but the optimized build finished exit101 after582.913s and emitted no executable pair. The stored build artifact stream does not contain the error detail. Original SDK body count0, Off/Record NOT_RUN.

Canonical no-default Lexical `test(native::text::) | test(native::text_spool::)` nextest `c8f633d1-d490-4720-80dd-f56fba993d45` VERIFIED363/363,210 skipped, exit0, compile56.47s/body146.625s. Compact bounds, exact ASCII/Unicode, wide key offset, logical-equivalent packed wire, optional denial before the SQL fallback, and memory refund passed. `cold_tokens.rs` b5501bda and fixed parity fixture `token_spool_batch.rs` 3b7edb09.

R10 `/private/tmp/qidx-sdk60-compact-rows.zGLJV5`, session74904, optimized daemon+helper build exit0 in292.027s. The 2,803 workspace inputs and HEAD match across all four build and SDK captures, and the diagnostic inputs and pinned binaries match. Daemon `0919db4a30ffe7a105f16a20aa62de9c52d981c904483c4b0b49fa2b39a19eee`, helper `5147e069d390029c5d6a03344d82bcb4e4ba6092f989d8815df901db44182278`. The input binding is identical to R8. The driver retains SDK60, file limit30,000, Work536,870,912B and R/W/M1,000,000 each. Quiet Off preflight BLOCKED with body0 at load32.440918>8 and two foreign Rust processes. Both original Record bodies ran and returned exit1. Both first-process traces are COMPLETE_DIAGNOSTIC with no health reports, zero open requests, and a matching SHA-256.

| Original Record | Actual wall | Accepted R/W/B/M | First refused charge | Full ACK · query · reopen |
| --- | --- | --- | --- | --- |
| Caddy590 | 43.23s | 956713 / 543365 / 536141651 / 706969 | 752549B in Storage prepared batch write `object.rs:807` | 0 · NOT_RUN · NOT_RUN |
| Etcd1421 | 18.65s | 362622 / 568439 / 536869310 / 869734 | 17613B in posting edit logical admission `packed_runs.rs:365` | 0 · NOT_RUN · NOT_RUN |

R10 includes independently owned Storage map/resident/scalar and Runtime/Contract changes since R8. Corpus-wide deltas and the later failure locations therefore describe the whole candidate. The lane44 Text fill comparison matched normalized input charge, case shape, owner order and branch across all589 Caddy fills and all1,117 etcd fills. Each owner saved exactly4B per case row. Caddy799,870 rows saved3,199,480B, etcd1,581,486 rows saved6,325,944B. Small root planning lane40 had the same reads/writes in all four Caddy windows and five etcd windows and used12,782B and23,464B fewer respectively. Independent global stamp verification lane41 retained exact reads and bytes across those windows. R10 completed all590 Caddy and944 etcd inventory diffs.

Etcd's terminal64-row flush failed inside the original posting logical write admission, before page-source creation, packet encoding, sort, second row copy or SQL. Earlier lane37 contains197 completed physical reorder copies totaling5,502,583B. An alternative packet representation requires measured net savings after offset and reader costs, separate from the terminal canonical fee. Caddy reached final `Text::apply_postings` and consumed33,025 inline plus18 object top-map rows before the reachable-node install. The terminal Storage admission is actual new batch payload plus256B per object. The 23,288B and16,011B overages describe the first refused charges only. Subsequent work required for FULL ACK, search and reopen remains unqualified.

### Authenticated posting packet boundaries

The packed cursor authenticates in-packet order once and keeps the boundary
witness through packet turnover and retry. The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md) owns this
contract. R11 Caddy and etcd Records refused before ACK at their original
caps. R11 Large passed its two-restart body in
`ef7d80cc-502d-4510-a803-9640e018463c`. Exact binaries, charges, source
capture and result tables remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### Resident packets across cold term windows

The two-slot resident packet cache reuses authenticated payloads across
term windows under its Original grant, source identity and accepted epoch.
R12's two Record requests still failed before ACK. Its selected test-only
source drift and original metrics remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).
The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md) owns cache lifetime and retry.

### Parallel closeout against NTX4

The sealed-writer grant release and existing group-local child validation
are implemented. The R12 owner table is historical and retained in the
[exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction). Current Caddy Record/Large and etcd outcome follow
[the active product boundary](#admission-decision-and-remaining-product-verification).

### Same-wire late materialization for phase-0 packets

The isolated late-emission draft measured a proposed Work change on five
rows. The later production cold term writer is recorded in the [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md).
The draft's exact fixture and proof limits remain in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### Posting cursor retry after retained-memory refusal

The retained cursor retries the refused row and checks stale-source
identity. The owning no-default Lexical run
`9d6e5066-bc00-49f1-b54d-5424a2fd89cf` passed 365/365. The subsequent
child-census candidate supplied diagnostic Caddy and etcd failures with
release-input drift. Their exact charges, metrics and build identities are
in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### NTX4 structural remediation and acceptance

- **Caddy, Storage done at owner scope.** Its guarded full-reference validation reused 22,696 results and completed all three physical groups in `/private/tmp/qidx-sdk60-guarded-children-retry.AVf3sU`. Storage nextest `30865238-1d41-4b81-a88f-5c998b63bf22` passed 223/223. The captured Storage inputs match live source. The original Caddy Record now refuses 5 reads at an already consumed 1,000,000 in Catalog confirmation. An unrelated production History input changed during the release build. F owns confirmation-time custody and the corresponding fixed fixture. Use the F packet for that implementation and evidence. [F integration](NC-F-integration.md)
- **Etcd, phase-0 run construction connected.** The empty-prior cold entry, including empty-profile Patch, uses the bounded writer described in the [posting custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody). The prior 5,502,583B reorder-copy saving was below the 6,309,495B needed for one read per completed phase-0 packet and its row bytes. The older same-wire late-emission draft remains an experiment. [Lucene term streams](https://raw.githubusercontent.com/apache/lucene/main/lucene/core/src/java/org/apache/lucene/index/TermsHashPerField.java)
- **Owner proof.** The cold fixture must cross both producer batch and phase-0 packet boundaries, compare complete `(term, owner, expected, next)` output against a sorted independent fixture, and exercise refusal and duplicate checks. Track unique terms, output rows, physical packets, sort/repack Work, retained peak and merge passes. Charge logical edits and real staging or merge work. Count compiler success separately from executed bodies.
- **Product proof.** On one stable source-bound daemon/helper pair with the original manifests and caps, Caddy590 and etcd1421 each require a healthy Record, two quiet Off successes, FULL ACK, query, and reopen. Run the same-binary 4,096-file restart check. NTX4 remains open until both corpus paths pass. Public SDK, IPC and durable index formats remain the same.

The first production-connected cold builder passed 366/366 Text and TextSpool cases (`5ad8dd70-4f05-477e-acfa-b90742ed0f8f`, exit0). Its audit exposed two regressions outside the first fixture: a larger mandatory Original grant and extra work for already ordered packets. The revised builder uses the original grant, preserves and reuses a checked wire prefix, and interns the suffix only after the first real ordering inversion. It merges that ordered prefix with sorted suffix terms once at flush. The run merge now borrows heap-head keys from checked packets and retains only one prior term with the existing key representation. The ordered fixture compares wire bytes, complete Work and output under the original small grant. The revised owning result and its cohort are recorded in [the release and Record follow-up](#same-source-release-and-original-record-result).

Catalog's current confirmation already uses a transaction-local full-reference `ChildWindow`. Its preflight proof is consumed by registration; per-parent lifecycle transitions remain separate. The Caddy refusal at `require_child_charged` has no per-reference evidence of an additional same-transaction query. F's separate namespace candidate `/private/tmp/qidx-sdk60-namespace-fusion.Lh6Rg2` has a Caddy-only original Record controller. Reuse its compiler pair for etcd only if both build captures contain the complete Text change, then preserve the source-custody classification of the pair.

F proved the separate Catalog namespace lookup fusion with 387/387 Catalog cases. Its first joint release attempt stopped at compiler exit101 before either SDK body because the hybrid writer assigned a last fence that the immediately following suffix always overwrites. The live Text file `cold_run.rs` `f48be457` removes the assignment. The new prefix/suffix fixture also checks exact order after a refused SQL write, unchanged SQL epoch, and duplicate rejection across that boundary. The corrected Caddy controller `/private/tmp/qidx-sdk60-namespace-text-repair.DpImEH` is live in shared build admission. The same-pair etcd follow-up `/private/tmp/qidx-sdk60-term-runs-etcd-bla20_ul` waits for the Caddy controller and verifies the four current production Text hashes in both build captures. The immutable binaries and the resulting SDK bodies are recorded [below](#same-source-release-and-original-record-result).

#### Original-input and product qualification checks

The pinned manifests and every listed source hash match for Caddy590 (3,272,107B) and Etcd1421 (8,666,217B). Each query path belongs to its checked manifest, and each repository commit matches its binding. The current darwin timing preflight (`env -u QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS python3 tools/ci/timing/check_host_contention.py --expected-os darwin`) exited1 with three foreign Rust processes and one-minute load 48.866699 over the ceiling 8.0. Original clean Off bodies remain `NOT_RUN` at this observation.

The cold term index now uses a fixed per-key fingerprint for a pinned Rust build (`cold_run.rs` `83f329bb`). This keeps the charged exact-key collision chain independent of the randomized `HashMap` placement. Output order still comes from the checked full term and owner keys. This closes a static determinism gap; no corpus collision was observed. The change and the updated etcd binding landed while the Caddy controller was still awaiting admission and before source capture.

The revised Lexical nextest attempt exited124 after 1,800 seconds of shared build-lock wait. It never started Cargo or any test body. The identical selector was submitted again on the current source with a longer admission wait. Caddy remains a single corrected release job, and the etcd watcher takes that exact pair after the Caddy Record terminates. Neither waiting handle is a verification result.

1. Reuse the one admitted daemon/helper build and compare both capture files with the production SHA set. Confirm the daemon compiler dep-info contains the two posting implementation files. If the full capture changes only outside the two release dependency closures, prove that scope from both depfiles. A changed release dependency prevents exact-source acceptance. Rebuild once against the stable input set, using the existing checked overlay only if concurrent production changes require it.
2. Require both original Record runs to produce FULL ACK, search and reopen. Replay each successful output with `python -m tools.benchmark.retrieval.causal_cost_diagnostic native-corpus-record --run-dir <output-root>`, which requires independent first/reopen trace identities and a successful lexical readback in both epochs. On refusal, preserve axis, accepted counters and failed stage from the complete diagnostic. For the Text calculation, `native_text_posting_edit/38` includes its child sort lane (`/39` on hybrid, `/36` on legacy), so do not add those child Work deltas again.
3. Run two quiet Off rounds per corpus with the original helper, manifest and query on that exact daemon, using fresh output roots. Preserve SDK60 and the original file/Work caps. For Large, pin `QIDX_NATIVE_LARGE_RELEASE_BIN` and `QIDX_NATIVE_LARGE_RELEASE_SHA256` to that daemon and run `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --test native_product_process --no-tests fail -E 'test(=large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks)' --test-threads 1`. The 4,096-file, two-restart fixture uses its own larger Work and deadline envelope. Its body and the original SDK60 bodies close distinct requirements.

#### Same-source release and original Record result

The namespace/Text repair build at `/private/tmp/qidx-sdk60-namespace-text-repair.DpImEH` finished in 298.275s, exit0, with 2,807 equal build inputs, equal HEAD and equal diagnostic tools. Daemon `91046e6be599…` and helper `049845718770…` were pinned and rehashed. Both release depfiles include `packed_runs/cold_run.rs` and the daemon also includes the modified Catalog reclaim path. `/private/tmp/qidx-sdk60-term-runs-etcd-bla20_ul` verified the same binaries, unchanged SDK source captures and zero release drift. Both Record traces report `COMPLETE_DIAGNOSTIC` with zero open requests and empty health. Both SDK bodies exited1 before FULL ACK, search or reopen. Clean Off and Large for this daemon are `NOT_RUN`.

| Original Record | First refused Work | Completed prefix and current owning boundary |
| --- | --- | --- |
| Caddy590 | Reads 1,000,000 + 1 at Storage `object.rs:1493` | All five Catalog advance spans passed. The new fourth physical group planned 187 objects and 6,179 external edges over 2,047 full refs. It reused 3,925 guarded child checks before the denial. The first three groups still reused 22,696 in total. F owns Storage follow-up. Measure exact child-ref overlap across groups and the failed validation prefix before widening witness scope. |
| Etcd1421 | Bytes 536,856,479 + 27,703 at Text `packed_runs.rs:36` | 1,045 inventory diffs and 221 packet flushes passed, versus 945 and 197 on the previous candidate. The failed packet's unique-term sort succeeded with 9,149B. Its output-body copy was the next charge, so savings must occur before this allocation and remain sufficient through final merge and posting application. The completed hybrid packet spans replace the old full-row sort/reorder spans. Aggregate costs cover unequal prefixes and cannot be subtracted as a matched saving. |

The first revised Text nextest compilation reached Rust and exited101 because an internal packet test still called the removed owned-row `Packet::next`. Its two calls now exercise the same assertions through `Packet::next_row`. No body ran in that failed attempt. Behavioral RED `428a8d6a-58fb-41b7-aa6a-ec4379d9d70e` executed one body and observed 392B against the 386B independent golden, exit100. The fix compares the unchanged prefix once per suffix term and preserves the full owner check on equal terms. Current-source no-default Lexical nextest `fc03272b-6696-4374-aa2b-f7215f8a7867` passed 369/369 with 210 skipped and exit0. This includes exact Work, unchanged SQL epoch on refused write, funded retry, duplicate key rejection and complete term/owner output. The complete Record traces remain tied to the earlier binaries.

The next larger Text cost to evaluate is materializing phase-0 packets through SQLite. At the exact Etcd1421 refusal, 221 successful disordered packets had already written one SQL row each, and no phase-0 merge read had run. For a packet payload of `p` bytes, omitting that actual SQL insert avoids `2p+6,208B` and one write before the refusal. The existing trace puts an upper bound of 23,006,301B on those writes because `lane38−lane39` still includes payload production, dictionaries and merge comparisons. Future eliminated point reads and EOF probes count separately. Any optional Original-backed run experiment must prove retained headroom, immutable replay after pre-effect refusal, accounted spill to SQLite, final SQL seek for a single phase-0 run, and mixed resident/spilled full tuple and EOF parity under the same caps. Measure the resulting prefix and FULL against the existing original input before adoption.

#### Structural RCA and compatibility decision

The grouped-prefix original Etcd1421 Record at `/private/tmp/qidx-sdk60-prefix-group-p4cz3qql/record` has a healthy complete diagnostic, matching released binaries and exit1. Six workspace source paths changed across build capture, so use this run for diagnosis. The first 221 successful lane38 spans, aligned by packet ordinal, used 561,441B less than the paired baseline. Lane39 Work was identical at those ordinals. Packet222 completed and the next refusal was `object/read_io.rs:168`, at 536,865,220 accepted bytes plus 5,987B. Inventory diff advanced from 1,045 to 1,049 completed owners. The failed leaf is inventory-read lane32 under lexical build, which runs both the authenticated cold-inventory visitor and its nested posting flushes. Charge their costs by exclusive path or matched prefix before treating the lane total as an object-read saving. FULL ACK, search, reopen and phase-0 merge were not reached.

| Owner | Existing implementation | Structural change and acceptance |
| --- | --- | --- |
| Storage / F · Caddy590 | The admitted run holds the union of destination gates until its last group finishes. Physical validation keeps a bounded exact full-reference table per group. Catalog custody has its own reservations, confirmations and lifecycle checks. The refused-prefix overlap observer and its fixture are wired. Confirm their owning run and the actual Caddy census before deciding on cross-group reuse. | Measure previously validated, still-gated full refs in subsequent groups and in the actual failed prefix. If the overlap justifies the table, carry successful path validation through one admitted run under its existing guards. Preserve lazy first-use validation, per-edge local dependency checks and reserve/advance/barrier/confirm order. Prove same-ref reuse, different refs sharing one gate, ungated/capacity fallback, missing/symlink child, concurrent reclaim, and refusal without publication. Compare a full table against the existing per-group baseline so early entries cannot silently erase later in-group savings. |
| Lexical · Etcd1421 | Cold runs already group suffix postings by term and merge the checked ordered prefix. Phase-0 packet emission still encodes and inserts a scratch SQL BLOB, later reopened by the merge reader. Installed inventory readback independently checks root identity, keys and counts. | First encode repeated phase-0 terms once per ordered chain without adding live packet memory. Measure actual packet emission, SQL insert, SQL reload and EOF work separately. Keep a bounded, immutable phase-0 run source with SQL spill as a separately gated candidate. Preserve final SQL seek/EOF and the single-run path. Trace exact occurrence refs across the prior prepare and failing visitor before proposing any independent readback reuse. |

Keep the existing group-local table. The initial cross-group candidate adds one optional bounded table for successful child path checks of prior confirmed outputs from the same install run. Membership uses the whole `ObjectRef`, and reuse requires the still-held governing gate. Allocation or capacity misses continue through the current group-local path. The namespace lease and gates serialize cooperating writers and reclaim. Reuse also requires the same managed namespace binding and the current contract for detecting out-of-band path changes. Eager validation of every edge would check later references before earlier groups have installed their children and shift the point at which effects or errors occur.

The resident Text prototype retains phase-0 bytes only when the Original grant can preserve the mandatory producer and merge envelopes. A resident packet can replace its merge reader's SQL buffer. Admission must account for the actual live backing, including the direct-merge credit, before counting it as a replacement. Prove a tight-scope case where the current spill path succeeds and the candidate either merges or falls back with the same output. Reject a SQL shadow row or any missing `(phase, run, page)` identity before merge. The term encoding below can be evaluated with the current scratch path and does not depend on resident admission. Any later residency change needs measured net Work and exact output equivalence.

The proposed changes are private to one install run or one Lexical attempt. SDK and IPC shapes, catalog state transitions, published object encoding, object identities, and on-disk reopen/restart semantics require no version bump or migration. Keep `posting_packet` final seek/fences and change-count observation, inventory `planned` → `ready`, confirmed occurrence receipts and top validation in their existing SQL authority. Replacing the full scratch database would expand the in-flight contract beyond this RCA. Record analyzers enumerate known counter keys. Ship new diagnostic counters with their matching reader/schema and check old-reader behavior before declaring trace compatibility.

The Caddy design uses exact batch reuse under the authority that guarantees namespace stability. RocksDB groups common immutable-block access within `MultiGet`, and Bazel CAS `FindMissingBlobs` explicitly couples batch presence checks with lifetime protection. Neither source supplies the Quanta reclaim or install guarantee. [RocksDB MultiGet](https://github.com/meta/rocksdb/wiki/MultiGet-Performance) and [Bazel CAS protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto).

The Text design follows term-first stream production and budgeted run spill. Lucene sorts each field's unique terms at flush and streams postings, Tantivy enumerates and sorts its term index before serialization, and PostgreSQL sizes its balanced external merge and read buffers under a single sort budget. Keep the established term chains and copy only the bytes each downstream consumer requires. [Lucene postings flush](https://github.com/apache/lucene/blob/main/lucene/core/src/java/org/apache/lucene/index/FreqProxTermsWriter.java) · [Tantivy postings serializer](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/postings_writer.rs) · [PostgreSQL tuplesort](https://github.com/postgres/postgres/blob/master/src/backend/utils/sort/tuplesort.c).

Run both original SDK60 Records on one stable released pair after the owner-level changes. Require complete healthy traces, FULL ACK, search and reopen for each, two clean Off rounds each, and the same-daemon 4,096-file two-restart case. The 512 MiB cumulative Work limit, 1,000,000 R/W/M limits, 60-second deadline and fees stay fixed. The phase-0 resident prototype has no production merge or executed proof yet.

#### Cold phase-0 term encoding

Native allocation admission checks live Temporary and Retained bytes together, including outstanding credit (`native_resources.rs:1322`). Holding optional packet data during inventory production could consume the next mandatory allocation's headroom. The resident prototype stays isolated pending a bounded producer envelope. Its directory patch scans and verifies the exact SQLite complement of resident run IDs, but a tight mixed-lifetime and merge-time mutation proof is still required.

The [cold posting ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody) records the mounted transient term encoding. Owning compilation, the fixed merge fixture and same-pair SDK execution still need their results.

The owning selector covers Text and TextSpool. Its new fixed 3,544-row oracle checks two input runs, 572 repeated terms, raw packet width 25,050B → 21,618B for run one, an independent lane38 Work golden of 102,206B, every merged row and EOF, and Off/Record Work parity. A separate fixture rejects malformed references even when their scratch digest is recomputed. After a green selector, reuse F's pending Caddy census release for original etcd only if both compiler captures include this exact implementation. SDK qualification still requires the outcomes above.

Owning Lexical proof **VERIFIED**. The first selector stopped during test compilation on a temporary digest borrow, with zero bodies. After binding that digest in the fixture, `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E 'test(native::text::) | test(native::text_spool::)'` exited0 in run `9626b601-4703-4dbf-9818-00284a86bdf2`. Passed371/371, skipped210, compile1m17s, bodies18.907s. The run includes the two new encoding oracles and the existing 131,073-term and etcd-size posting fixtures. The original etcd driver is staged at `/private/tmp/qidx-sdk60-term-backref-k_xrrs0s` with exact production Text hashes. It requires the identical daemon/helper produced by the census release and the unchanged original corpus binding.

Raw span ancestry narrows the separate inventory investigation. Of 155,282,012B and 431,730 writes in the grouped-prefix Etcd1421 lane32 diagnostic, 105,239,440B and every write were in its 3,371 nested lane33 posting flushes. Residual visitor Work was 50,042,572B and 4,177 reads. Lane32 mixes inline and directory aliases and does not break out the actual object routines. Storage's retained read clones the immutable `Arc`, so there is no second payload copy there. Producer authentication and post-apply readback retain separate validation points. Before changing that path, measure exact object-route reads and complete output under failed or mismatched readback.

The same trace's 142,994,133B build_prepare/40 is the inclusive Native Literal prepare. Its source/gram collection and directory/installation spans are nested, including the 198 single installed-block readbacks. Kernel already passes one bounded `AttemptObjectReads` through both member builders. A full-reference hit reuses immutable backing with the existing logical read admission. The trace does not attribute those hits to each member or source ref. A new cross-member stream would need to preserve independently complete Literal raw-byte trigrams and Lexical UTF-8/tokenization, so this is outside the current packet modification.

Two earlier shared releases exited 101 at History validation-only calls, with
zero SDK bodies. The repaired source retains both validations. The exact
compiler runs and paired supervision are in the
[recovery index](../../../ARCHIVE-INDEX.md#2026-11-17-active-ticket-proof-refresh).

#### Original Caddy census with grouped Text encoding

The compiler exited0 after660.367s. All nine census bindings and both binary digests match. Three release inputs changed across the 2,812-file capture (`native_history/build.rs`, `native_history.rs`, and `native_boot.rs`). This pair supports bound Text/Storage diagnosis and does not qualify as a whole-source release. Original Caddy590 Record exited1 with 1,000,000 reads, 618,916 writes, 494,294,051B, and 781,821 mutations accepted. Its next charge was one read at Storage `object.rs:1518`. Strict ingest passed with 244,058 written events, no drops or open spans, and empty health. FULL ACK, query and reopen were not reached.

Four successive physical groups under the failing map apply measured planned guarded full-ref overlap of 0 / 273 / 306 / 350 and completed repeat path checks of 0 / 273 / 306 / 331. Total completed overlap checks in the captured request are **910**, including **331** in the refused group before the denied read. The already reused group-local checks were 8,199 / 7,229 / 7,268 / 3,925. The cross-group counts exclude those local-cache hits, refs first installed as local outputs, and the denied edge. They establish a concrete candidate for Storage reuse and do not establish sufficient remaining Work for FULL. Keep the group-local cache and confirm the managed-namespace validity across group boundaries before adopting cross-group reuse.

F's paired SQL/Work evidence bounds later confirmation at **10,104 reads before dynamic child authentication** for the final group alone. That is greater than all 910 measured repeats. F owns transaction-local namespace authentication reuse, with reauthentication after writes, exact SQL/Work delta, and mutation rollback. Keep the Storage memo as a follow-up to the actual residual. See [F integration](NC-F-integration.md).

#### Original etcd result on the same daemon and helper

The unchanged 1,421-file, 8,666,217B original input used the Caddy daemon and helper with matching SHA-256 digests. The 2,812-file SDK pre/post captures were equal. The EtcD Record exited1 in34.80s wall. The first refused charge is the existing logical posting admission at `packed_runs.rs:401`, with R354,434 / W619,540 / B536,869,801 / M869,734 accepted and 17,524 more bytes requested. The raw footer is complete with 200,304 events, no drops and no open spans. The initial system Python3.9 analyzer could not run the current `zip(strict=True)` validator. Reingesting the existing trace with workspace Python3.12 exited0, reporting `COMPLETE_DIAGNOSTIC`, zero open requests and empty health. No second SDK body was needed.

Against the grouped-prefix Record, all 222 common lane39 sort costs are equal. The 222 ordinal-matched, completed lane38 packet spans used **2,213,358B less** in the new candidate (210 lower, 12 equal, 0 higher). Every saving is a multiple of3, matching the encoded-byte removal and the two actual payload widths in the existing INSERT charge. Original inventory diff completed 1,063 owners versus 1,049, and 226 versus 222 disordered packets. No final merge, FULL ACK, query or reopen was reached. This is pinned-binary diagnosis with the same three History release-input changes as Caddy.

Keep the existing logical posting admission, fee schedule, limits and phase0 integrity. The priority for the next Text change is a measured reduction of actual write/read work before sealing. Compact owner/reference cells can remove 2–4B per eligible row but the previous 222-packet inventory bounds pre-merge savings at 2,349,672B to 4,699,344B even before accounting for prefix re-encoding. Do not treat this upper bound as a remaining budget or as proof of FULL. Evaluate a bounded spill/retention path and the original allocation envelope before adopting another codec variant. The release and SDK diagnostics above leave both original Records FAILED. Clean Off preflight refused at 78.309 one-minute load against8.0 (22 foreign Rust processes). The same-daemon Large and clean Off bodies are NOT_RUN.

For the new 226 completed disordered packets, lane38 minus lane39 reaches20,703,017B. The existing emission charges `P−50` for body plus dictionary bytes and the INSERT charges `2P+6,208`, leaving a nonnegative comparison charge. Thus total payload is at most **6,437,103B**, and the INSERT components at most **14,277,214B** before this refusal. A candidate retaining those packets would need room for that observed payload shape, metadata, and later packets while preserving mandatory allocations and separately metered fallback. These numbers are upper bounds, not measured savings. A positional file spill cannot be connected through the existing `ScratchIoBinding`: the binding covers SQLite VFS scope and namespace custody. A new managed file port would need reservation, exact I/O receipts, cancellation and partial-write cleanup.

The source already compares adjacent term keys in the mandatory `ColdTokens::sort_and_validate` pass. Reuse that exact group count before the single-block alias probe. Once there are over 512 distinct case-prefixed keys, the probe cannot succeed, so skip its actual key/position inspection and vector lookup. The original direct or SQL fallback still receives the fully validated arena. The current diagnostic observed 205 direct owners above that count. Their exact probe cost was not independently metered. This candidate requires the 512/514 boundary oracle and the existing owner/fallback suite before the next joint release.

The count gate and its fixed boundary oracle are applied. At 514 terms the oracle verifies zero alias-probe Work and zero new memory grants, two direct output blocks and every ordered key/position. At 512 terms it verifies an eligible one-block alias, the original 4,608B inspection charge and memory refund. The owning no-default Text and TextSpool selector `5af239de-a369-44be-8de6-d4b7cab44e76` passed372/372, skipped210, exit0. Compile took14.20s and bodies12.921s. The next paired etcd driver at `/private/tmp/qidx-sdk60-alias-count-fdlr4ij6` binds all five Text build inputs including `cold_tokens.rs`. Invoke it with the workspace Python and the one F-owned release containing both owner candidates after their owning proofs. The original SDK bodies are NOT_RUN for the count gate.

#### Next measured Text boundaries

The registered attempt's shared object window intentionally preserves logical read and decode charges. The source's term and position groups cannot replace the independent installed-object readback, which proves persisted references, counts, descriptor boundaries, both seals and EOF before publication. Preserve those checks.

The 953 lane32 owners from the pinned backreference Record consumed50,508,862B after excluding nested posting edits. The 202 spans with more than two reads represent34,064,564B of that exclusive total and are the confirmed multi-read tail. A cold Directory cursor currently starts a fresh map scan at each descriptor. A retained bounded descriptor page could eliminate repeated root/path traversal. Its potential map work is a subset of those exclusive totals. It needs an Original-funded page/retained key grant at the backend admission boundary, intact child verification, and boundary/EOF/output and failure-oracle tests. A scan8 failure after charged reads cannot be retried as if the scan had never happened. Changing the page-prefetch order also changes the observable error prefix. No unconditional Text-only fallback was adopted.

The original Etcd manifest and all 1,421 file hashes were independently checked. On the 1,392 entirely ASCII files, the exact token character/length rules find 927,696 indexable runs. 626,163 repeat the same raw term within an owner and span3,069,185 raw bytes. The two case keys for these repetitions would occupy7,390,696B if every such source used the cold arena. This is an input-shape bound across the whole ASCII subset, with no claim about the executed prefix or net Work. A sorted-source interner must preserve NFC and both case forms, the exact-census assertion, case-sensitive positions and replay fallback. The current arena grant covers capacity before filling; a shorter used key buffer alone does not return that grant. Count new hash/compare, row movement and temporary Original costs before choosing a design.

An isolated candidate oracle at `/private/tmp/qidx-map-scan-oracle.sJyhXo/descriptor-scan-work.patch` applies cleanly to the current Storage source in a dry run. The fixed 32-descriptor trie compares actual scalar scan1 and page scan8 Work, complete ordered output, cancellation, and admission/refund. No production code is changed by the oracle. Compile and body are NOT_RUN. Use the measured pair before committing to the backend admission seam. The existing paired etcd watcher is bound to the prepared F release `/private/tmp/qidx-sdk60-caddy-catalog-bound.81rgVy` and remains in intake until F's release controller is active.

#### Combined Catalog and exact alias-count candidate

F's Catalog lib selector `b3cbd4fb-9a03-49b5-8e5a-a88122855c92` passed390/390, exit0. The exact fixture observed one namespace SELECT in each of reserve and confirm (54 and37 reads) and proved reauthentication, refusal, rollback and reopen after a queue trigger changes the binding. Catalog and Text owning checks now cover this candidate. The single shared optimized release `/private/tmp/qidx-sdk60-caddy-catalog-bound.81rgVy` acquired the build-test lock and started compiling. Its prebuild capture matches all nine census-bound inputs and all five Text inputs, including the 512-term alias gate. The live etcd watcher observed that exact controller and will reuse both binary hashes after the Caddy Record. Fresh original Record, Off and Large results remain pending.

#### Original SDK results and structural decision

The candidate above compiled in271.720s, exit0. All 2,812 build inputs were identical across build and both SDK captures. Both original SDK runs rehashed the same daemon `f87b6b3d239a…` and helper `eb9ffec3e4d5…`, with no Work or request-limit override. Strict ingest reported `COMPLETE_DIAGNOSTIC` with empty capture health and zero open requests for both first-process traces.

| Original SDK Record | Outcome | Evidence |
| --- | --- | --- |
| Caddy590, 3,272,107B | VERIFIED | FULL publication in50.457s from submit, exact 590-file inventory ACK, FileMap/literal/text search and independent reopen search. SDK and both-trace verifier exited0. The successful BuildPrepare span used951,711 reads, 621,676 writes, 492,696,487B and786,109 mutations. |
| Etcd1421, 8,666,217B | FAILED | `BuildPrepare` refused17,445B after536,855,013B, with R348,315 / W623,254 / M869,734. The first denial is existing logical posting admission at `packed_runs.rs:401`. Completed1,072 inventory diffs and228 disordered phase-0 packet writes. The final posting merge, FULL ACK, search and reopen were not reached. |

Comparing the 1,117 ordinal-matched inventory preparation spans at `native_text_inventory_prepare/46` with the previous backreference Record shows1,370,449 fewer bytes, across207 lower and910 equal pairs. Reads, writes, mutations and the parent token-stage Work match. Completed inventory diff advanced from1,063 to1,072. The required logical debit is where the exhausted budget becomes visible. Upstream packing, re-encoding, SQLite scratch and repeated map scans all consume the same budget. The matched preparation savings and the one next debit do not measure the budget required for the remaining349 inventory owners and downstream apply.

1. **Catalog lifecycle, implemented.** Keep transaction-local binding, write-triggered reauthentication and per-parent lifecycle checks. The paired original passes this public Caddy Record path. The earlier910 measured repeat cross-group child probes did not establish enough savings to close the older terminal, so further cross-group memoization is outside the current repair.
2. **Directory traversal, measure first.** `Cursor::advance` performs `scan(limit=1)` for each descriptor. The backend `scan_with` rebuilds its path and stack and reopens the same root on each call. The current EtcD result has204 completed lane32 spans with more than two reads, R2,724 and34,167,943B after subtracting nested posting lane33. That includes child loads and seal checks. Use the fixed scalar/paged Storage oracle to measure the actual repeated trie work, then compare an attempt-bound root/continuation cursor against it. Keep bounded grants, exact ordered fences, independently loaded child identity/counts, seals and EOF. Prefer incremental continuation when eager pages would move future validation ahead of the current child. Optional page admission can choose scalar only before additional Work or input effects. After a selected path has started, propagate refusal/cancellation/corruption and poison the attempt.
3. **Phase-0 posting, structural prototype.** Reuse the existing sorted run merge and publication path. Prototype a generic one-head owner cursor inside prepared inventory, then emit a bounded sorted multi-page phase-0 run into the existing merge. A selector requiring all alias owners would skip all five observed mixed batches on the preceding Record. The generic path must cover inline aliases, directory aliases and ordinary owners, preserve the logical per-edit fee and exact `(term, owner)` order, retain grouped term backreferences, and fund all simultaneously live cursor heads through Original. Compare actual packet copy, SQL I/O, heap Work and peak grants against the reference. Keep sorted runs disposable until the last input seal and posting completion. Use neither full-attempt cursor retention nor a new spill port without measured net benefit and explicit funding.

An internal cursor and paired private `QITXPR02/03` writer/reader change need no SDK, IPC, or durable-format migration. Restart reclaims old scratch as orphaned allocation. Preserve the durable posting formats, semantic output policy, expected-view CAS and one atomic FULL ACK. Changing the persistent dense descriptor/page would change output identity and require old-view/reopen compatibility work. Splitting externally visible ingestion would change the source and ACK contract.

Reference implementations for the chosen boundaries are [LevelDB's same-block iterator reuse](https://github.com/google/leveldb/blob/main/table/two_level_iterator.cc), [SPIMI's bounded runs and merge](https://nlp.stanford.edu/IR-book/html/htmledition/single-pass-in-memory-indexing-1.html), [Lucene's flush and commit boundaries](https://lucene.apache.org/core/10_3_2/core/org/apache/lucene/index/IndexWriter.html), and [RocksDB's recoverable manifest groups](https://github.com/facebook/rocksdb/wiki/MANIFEST). These support the design, not a Work forecast. Lucene's RAM threshold is a flush trigger, while this request's 512MiB Work is cumulative and unchanged.

Clean Off preflight refused with one-minute load54.816406 against8.0 and11 foreign Rust processes. Clean Off bodies and the 4,096-file two-restart body for this daemon are NOT_RUN. The next product candidate must pass both original healthy Records and both quiet Off rounds per corpus plus the same-daemon Large check.

#### Executed Storage traversal oracle and mutation ceiling audit

The frozen oracle at `/private/tmp/qidx-descriptor-oracle-run.yY3zYDIs/snapshot` checked 3,252 copied paths against Main, including the Storage `map.rs` preimage `0e20ad2a…`. Only the snapshot received the oracle. `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-storage --lib --no-tests fail -E 'test(=map::tests::descriptor_shaped_scan_pages_measure_exact_work_without_changing_rows)' --no-capture` passed one body, exit0, run `60e838fc-9d06-44aa-9bc0-70e247b61447`. Scalar pages used66 reads and85,925B. Eight-row pages used10 reads and26,019B for the identical32 ordered `(key, value)` rows. Both had zero writes/mutations. Cancellation recorded zero Work. A refused eight-row retained capacity recorded a sticky memory denial, so scalar fallback must be selected before an optional attempt records that denial. These savings describe the fixed trie.

The isolated DFS continuation is **VERIFIED** at the Storage library scope. The first shared attempt ended before admission. The isolated compile then reached Rust and stopped at E0382 with zero bodies. The corrected snapshot reborrows the mutable reader for each loop iteration. Nextest `fb3d316d-be69-4d95-b50e-6a531082a768` passed the expanded fixed fixture, exit0. Continuing one traversal and returning one row at a time used3 reads and17,664B. This saves63 reads and68,261B versus scalar scan1 and7 reads and8,355B versus scan8. Exact rows and `has_more`, zero-Work EOF, resumed fence, cancellation, sticky failure and fund release passed. The full no-default Storage library run `a02e0be0-baa8-4a92-9ea5-ace7365f9542` passed226/226 with1 skipped, exit0. Both executed in the warm copied checkout with cooperative admission disabled for the isolated crate. The verified patch at `/private/tmp/qidx-scan-continuation-prototype/scan-continuation-verified.patch` has SHA-256 `9494318bd172b5c4e36cb392799acd5955c48905a3a341bbd5fad6434f83a647` and applies cleanly in a dry check. Main runtime sources are untouched.

Production adoption requires a same-budget synchronous scan visit that preserves page1 lookahead order, passes through `OwnerScope::read_node`, reuses existing descriptor and child verification and keeps the owner attempt valid. It needs a pre-effect Original bound for live DFS frames, returned page, child decode and seal state combined. The cold output producer credit does not cover this. It also needs a directory-heavy end-to-end target showing net savings under the measured Original envelope. The phase-0 grouped multipage consumer remains staged outside Main. Its SQL roundtrip fixture covers page1 and rejects grouped bytes as final format. An actual bounded multipage writer is necessary to affect SDK Work.

The etcd Record already charged790,743 token mutations over1,117 complete source preparations and869,734 mutations total before its byte refusal. The direct and SQL paths each admit one mutation per indexed run. At the time of this Record, refusal of both direct output and funded arena replay could cause the SQL fallback to reparse and bill its actual second traversal. The captured prefix has no SQL output branch. The fixed direct-fill test retains exactly one mutation per indexed run even when the old packet allocation disappears. Phase-0 posting lanes33/38/39 add zero mutations. Directory traversal also adds zero mutations in the executed oracle. The existing generated-Large harness enforces a token-mutation floor before its own daemon execution and uses a separate profile when that floor exceeds the fixed limit.

The canonical census of the original 1,421-file etcd manifest is **VERIFIED**. The shared command ended before admission with exit143. A dedicated cache lane then ran the same audit once through `./scripts/cargow --lane token-census-lane run --locked --offline -p quanta-index-lq-text-normalizer --example original_token_census` in the copied source, with `QUANTA_INDEX_RESOURCE_ADMISSION=0` for this standalone census. It compiled in3.94s and exited0. The audit uses the production `TokenCursor::try_new_bounded` NFC and `is_indexable` path. All 1,421 file hashes and the exact manifest were checked before and after. All 35 captured normalizer, vendored NFC, lockfile and toolchain inputs match the released daemon.

The complete corpus contains **1,009,664 indexed token runs**, including927,696 in1,392 ASCII files. Token admission alone exceeds the fixed1,000,000 mutation limit by **9,664**, before any source, Literal, Catalog or Storage work. The original SDK submits one `SourceTransition` and uses `FIXED_FOREGROUND_WORK_LIMITS` directly. Additional byte savings, faster merge and eliminating replay cannot make these two fixed conditions compatible. The observed original SDK failure is still the earlier byte refusal. The mutation result is an independent lower-bound proof, without a separate SDK mutation-refusal run. `1,009,664` is a floor, and total mutation funding needed for a complete build is still unmeasured.

The read-only fallback audit confirmed a separate duplication case. If the direct output grant and SQL replay credit were both refused, the former path could tokenize the already validated source again. The production disposition and the 257-token no-credit oracle are recorded in [Directory scan continuation and SQL replay](#directory-scan-continuation-and-sql-replay). The exact-input admission conflict applies independently of this duplicate traversal.

The first no-default Large run on daemon `f87b6b3d239a…` reached the shared `native_product_process` test driver and exited101 with zero bodies. The compiler rejected two unused publication-handoff fields in `tests/native_product_process/native_boot.rs` because their only consumer is a `legacy-tantivy` test. The fixture now gates both fields, their computation and initialization on that feature. The default-off Native cross-root refusal test is unchanged. The exact Large selector acquired the build/test lease again with the same pinned daemon after the failed command terminated. The corrected driver compiled in2m13s, and nextest `8aefe725-648f-4fcb-8a4b-7e8aeacfd510` started the one selected body with56 skipped. Process inspection observed the exact pinned daemon serving `native-config.json`. The body **VERIFIED** in143.167s, exit0 (1 passed, 56 skipped). It published 4,096 files and compared source bodies, owner identities, repository and global ranks across two daemon restarts. Current source capture finds only this fixture change and the previously reviewed History `cfg(test)` change. Both release dependency files exclude the changed `native_boot.rs` test fixture. The post-body 2,812-file capture preserved that same two-path difference and HEAD. The daemon and helper digests still match their source-bound SDK runs.

F's existing paired Caddy Off qualification `/private/tmp/qidx-sdk60-caddy-catalog-off.mwOL3o` owns both rounds for the released pair. The duplicate waiter was stopped before any SDK body. The retained 20-minute quiet-host window terminated with **0/2 rounds started**. The last gate in that window was load24.569336 against8.0 and11 foreign Rust processes. Both bodies remain NOT_RUN. The post-Large canonical preflight refused at load35.196777 and19 foreign Rust processes. When admission is available, bind the retry to the same original input and binaries and account for the exact test-only fixture change alongside the previously reviewed History test change. The full product criterion still requires the original etcd Record and both etcd Off rounds on the same accepted pair.

#### Admission decision and remaining product verification

Reuse the bound Caddy Record and same-daemon Large PASS for their recorded
candidate. The post-reboot Caddy quiet Off window also ended at **0/2**.
The pinned daemon and helper hashes match the original Record.
[F's admission record](NC-F-integration.md#f-로컬-합류--2026-11-16) owns
the retry and host outcome. The original etcd completion
condition and current fixed per-operation funding are incompatible. Current production keeps all fees, the 512MiB byte limit, each1,000,000 count limit and the60s deadline. There is no accepted SDK, IPC, durable-format or atomic FULL change in this packet. Repeating the fixed etcd attempt cannot establish acceptance. A weaker target cannot close NTX4.

If a separate diagnostic profile is explicitly requested, reuse the existing harness `NativeForegroundWorkload::Explicit` and `native_config` boundary. The original `native_corpus` directly selects fixed Work and has no environment or CLI limits override. Keep the exact 1,421-file manifest, one FULL source transition, pinned daemon, result identities and reopen checks. Record the non-production profile separately and capture full R/W/B/M, deadline and resource demand through final posting apply and release. The token floor is already1,009,664. Keeping the observed78,991 non-token mutations alone would yield1,088,655 before unobserved work. Neither number is a funded success target. Select any future supported envelope only after full observed demand and the needed capacity margin are reviewed. An isolated explicit success would inform that decision and would leave the original fixed-profile result as FAILED.

#### Directory cursor allocation repair

The [Directory custody contract](../../../adr/NOV-07-001-native-index-execution-and-custody.md#cold-text-installation-and-packet-custody)
is implemented. Borrowing the admitted fence removes a temporary lease and a
key copy per descriptor transition. The owning tight-memory oracle and the
remaining Native SDK verification are recorded below.

On dirty HEAD `ade5fadc1a084fbb81eba831861e996081d80867`, the first no-default Lexical compile stopped at two existing fixture errors. The planner test now imports `LqPredicateArg`, and the SourceRecord fixture clones its prior record before constructing the invalid variant. The corrected fixture build then ran the independent tight-memory case on the old Directory production path. Nextest `fc44b627-a895-48c1-9c7c-ed6a17faaa65` returned `ResourceLimited` with one executed failure and exit100. The fixture independently funds one exact leaf parse and checks the two output rows, EOF, two child reads and grant release.

After the repair, `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-lexical --lib --no-tests fail -E 'test(native::text::tests::occurrence_blocks::) | test(native::text::tests::shared_entry_maps::) | test(planner::tests::) | test(changed_body_enters_token_build_and_bad_reuse_metadata_refuses)'` passed74/74, skipped509, exit0 (`ed9051a9-f98e-429c-bee7-be904997ec55`). Coverage includes cold multiblock readback, malformed intervals, late child refusal, retry and funded memory release. The Directory source passes its format check. Native SDK for the modified production source is NOT_RUN. The cached pair for Caddy Clean Off remains the previously source-bound release.

The cached-pair Caddy Clean Off controller at `~/Library/Caches/quanta-index/native-sdk-caddy/off-20261117.7LqaiU` verified both original executable hashes and the original manifest. Its admission window ended at0/2 without starting an SDK body. The direct follow-up timing check exited1 at load76.753418 against8.0 with19 foreign Rust processes. Clean Off remains blocked. The earlier Caddy Record and Large results remain scoped to the pinned release.

#### Directory scan continuation and SQL replay

The [custody ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#sequential-cold-alias-scan-and-sql-replay)
records the mounted forward Directory scan and borrowed SQL rows. Storage
continues from the last delivered fence after either callback memory pressure
or excess DFS capacity from an earlier, deeper path. SQL fallback consumes
the validated arena in 64-row batches. It retains the existing row bindings,
Scratch admission and fees without a second Rust token backing. The patterns
are consistent with [Lucene `TermsEnum`](https://lucene.apache.org/core/10_5_2/core/org/apache/lucene/index/TermsEnum.html)
and [SQLite parameter binding](https://www.sqlite.org/c3ref/bind_blob.html).

The complete Core, Storage and Lexical no-default lib rail is **VERIFIED**.
The fixed 3,252-path source cohort at
`/private/tmp/qidx-ss-cold-proof.47fQi2Xz/snapshot` has the same dirty HEAD
`ade5fadc1a084fbb81eba831861e996081d80867`. The 16 edited Rust paths matched
Main at capture. The final check found 15 still identical and a later
test-only addition/assertion edit in `native/text.rs` from the concurrent
repository-path lane. The production hunks in that file match the cohort.
The copied checkout and isolated build cache avoid the occupied shared
build slot. This run used `QUANTA_INDEX_RESOURCE_ADMISSION=0` and
`QUANTA_INDEX_TARGET_GC=0` only for isolated test execution, without changing
product limits or making a quiet timing claim.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-core -p quanta-index-storage \
  -p quanta-index-lexical --lib --no-tests fail --no-fail-fast \
  --status-level fail --final-status-level fail
```

After forcing compiler freshness for all edited lib paths, nextest
`8e9d485d-bee4-4280-9799-f6d8e86e2f2e` exited 0 with
**1,172/1,172 passed**, two existing skips, in 17.009s. The new regression
checks cover actual FileObjectStore and ScopedReads traversal, exact scalar
Work after optional state eviction, deep-to-shallow capacity release,
decoded-child release before the next map read, cancellation and sticky
genuine refusal. The SQL fixture exhausts all 257 token mutation admissions
before fallback, funds only the live input backing, and compares independent
positions and packed output. The SQL writer fixture also checks the fixed
two-row write and 132-byte charge with zero extra Rust memory grants.
An earlier test expected refusal at the removed SQL string allocation. It
now verifies successful insertion and those unchanged fees and rows.

The actual Native UDS composition rail is also **VERIFIED**.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  --no-default-features -p quanta-index-searchd-runtime \
  --test native_index_uds --no-tests fail --no-fail-fast \
  --status-level fail --final-status-level fail -E 'test(text_entry_maps::)'
```

Nextest `cd228003-4186-42a7-8dcf-b4b0c7f67fa7` exited 0 with
**4/4 passed**, 72 outside-selector skips, in 5.784s. The new fixture publishes
257 distinct ASCII words through `SourceAdvance`, crosses the 512-term packed
envelope, and verifies first/middle/last query owners and identical ranked
hits after service reopen. The other selected fixtures cover DSL published
files, DELTA/delete/noop/pinned old views, and old-layout refusal. The initial
copied test mtime left the new fixture absent from the cached binary. An
explicit source timestamp refresh, compiler rebuild and selector listing
resolved that problem before this four-body run. All edited lib paths were
also forced fresh for the final 1,172-body result above.

The current-source original SDK run is **NOT_RUN**. The canonical quiet
preflight exited 1 at one-minute load 56.19043 against 8.0 with 34 Rust
processes classified as foreign by the gate. Earlier Caddy Record and Large
evidence remains bound to its original release. The original etcd input
requires 1,009,664 token mutations before other Work under the unchanged
1,000,000-mutation ceiling. SDK, IPC, durable format and fee schedules are
unchanged.
