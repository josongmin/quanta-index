# Quanta Index closeout 최종 구현안

설계: `FINAL` · A/D/C5 원 수용: 10-10 `VERIFIED` · 10-11 추가 수리: 영향 검증 `VERIFIED` · 배포 런처/Large: `VERIFIED` · XL product/lifecycle: `VERIFIED` · 최소 준비 worker/정리 코드: `VERIFIED` · 성능 수용: `BLOCKED` · 설계 확정일: 2026-10-10.
설계 감사 소스 기준: `e073465e534f3529d47a43f84ceb1f19992953f1`.

## 최신 코드 마무리 상태 (2026-10-11)

- target-scoped compaction 준비 worker를 추가했다. 기존 예약·base pin·adoption lease를
  유지하고 File old debt 1개와 Native old debt 최대 2개만 읽기/CPU로 준비한다. writer
  mutex 전 join, 취소 시 cancel 후 join, 준비 packet과 worker를 합쳐 process당 1개로 제한한다.
- 준비한 descriptor/current/source/membership reference를 채택 전에 다시 확인한다.
  drift 또는 준비 예산 부족은 기존 foreground 도출을 사용한다. Native body/NFC 준비와
  foreground posting build·physical ID 부여를 분리하고 독립 seal 검증을 유지한다.
- 실제 no-op source event도 예약된 target에서 Native old debt를 처리한다. 새 logical ID를
  발급하지 않으며 이전 sealed base와 held read view를 보존한다. 기존 Chunk clear 거절을 유지한다.
- harness 소유의 driver-stopped TempDir만 canonical object shard 최대 4개 worker로
  정리한다. 모든 worker를 join하고 남은 root를 close하며 삭제 오류를 반환한다. 링크 또는
  비정상 layout은 기존 close를 사용하고 기존 전체 deadline과 RuntimeStop span을 유지한다.
- strict lexical/harness Clippy와 회귀 36개, 6개 바이너리의 통합 테스트 113개가 모두
  `VERIFIED`다. 준비 유무의 root 동등성, 세 번의 compaction cycle, 실제 no-op 게시,
  취소·stale·base 보존·SIGKILL·I/O 거절·정리 오류·small-scale trace를 포함한다.
  명령과 범위는 [VALIDATION.md](VALIDATION.md)의 마지막 절을 따른다. 전체 workspace
  또는 LQ5 전체 운영 수용은 별도 범위다.
- 새 source `07f0430…`의 XL은 기존 900초 상한에서 777.167초, producer exit 0으로
  product/lifecycle `VERIFIED`다. 10개 phase·재오픈 후 검색·owned-root 정리·artifact·footer가
  완료되고 Health/loss/open spans는 0이다. 기존 timeout 기록은 보존한다. contended host의
  진단 결과이므로 정리 최적화의 인과적 배속이나 정식 성능 수용으로 해석하지 않는다.
  legacy aggregate parity와 build/source binding이 부족해 capture CLI는 정상적인
  `PARTIAL_DIAGNOSTIC`/exit 2다.

이번 범위는 A/D의 직접 수리와 C5의 최소 영속 예약 공급이다.
이 문서가 구현·소유권의 기준이며, 근거는 [AUDIT.md](AUDIT.md), 합격 조건은
[VALIDATION.md](VALIDATION.md)에 둔다.

후속 lexical 구조 개선은 [LEXICAL-QUERY-PLAN.md](LEXICAL-QUERY-PLAN.md)에 정리했다.
브레이킹 저장 포맷·검증 시점·검색·증분 갱신의 최종 실행 계획이며,
기존 A/D/C5 완료 조건과 구분한다. paged 저장·검색, decoded-page cache, File 독립
proof 및 observer 변경에 후속 Native body·map V2·File segment 교집합·요청 metadata cache를
통합했다. 10-10의 Large/XL 및 전체 배치 수치는 당시 소스의 이력이다. 10-11 통합 이후
전체 배치 실패 두 건과 수리 후 영향 검증은 아래 절에 분리했다. 현재 source의 배포/scale
결과와 남은 LQ4/LQ5 운영 수용은 [VALIDATION.md](VALIDATION.md)의 마지막 절을 따른다.

## 2026-10-11 정적 원인 확인 후 수리

- retention/GC의 반복 SQL 준비를 측정 호출·bounded GC step 안의 statement 재사용으로
  바꿨다. 객체 인증·중복 digest의 kind/size 확인·완전한 mark·epoch fence는 유지한다.
  dependency/feature 변경 없이 기존 `rusqlite::Statement`를 사용한다.
- adoption group은 기존 object codec의 128참조 한도에 맞췄다. 5,351바이트 manifest를
  먼저 예약해 기존 2MiB 총량을 유지하며 writer와 pending-head recovery가 같은 한도를 쓴다.
  durable journal/barrier 순서와 GC sweep의 64객체 한도는 유지한다.
- 정적 caller/codec 대조 뒤 lexical library·tests strict Clippy와 store/GC/publication fault
  회귀를 한 배치로 실행했다. **46 실행·통과, 349 skipped, 159.110초**다.
  128참조의 빈/부분/완전 journal 복구, 고정 262바이트 공유 graph, typed corruption,
  실제 SIGKILL 복구를 포함한다.
- 10-10의 Large/XL은 이 수리 이전 소스의 기록이다. 수리 후 Large/XL·새 daemon SDK·전체
  workspace 배치는 `NOT_RUN`이며, 성능 개선이나 XL 마감을 주장하지 않는다.
  전수 closure/physical 조사와 eager compaction은 아직 남는다. 이 46개 검증은 waterfall
  통합 이전 결과다. 이후 통합과 검증 상태는 다음 절에 별도로 기록한다.

## 2026-10-11 최종 통합과 검증 상태

- 별도 waterfall worktree의 고정 source snapshot에서 141개 파일의 source/test 변경을
  현재 checkout에 통합했다. 다른 pre-existing dirty hunk는 보존했고, root의
  statement 재사용과 128참조 adoption/recovery 수리는 3-way 대조 후 유지했다.
  이 통합 이후의 증거는 앞선 두 checkout의 결과와 별도로 기록한다.
- 통합 정적 검사에서 새 module cycle, 금지 serde derive, contract crate의
  transport callback port를 확인해 수리했다. read/hash/decode/SQL totals는 하위
  `causal_profile` owner로 옮겼고, SDK callback port는 SDK가 소유한다. core는
  concrete attempt lease를 제공한다. 진단 Off에는 객체별 타이머를 만들지 않는다.
- 실제 CLI backup/restore 후 두 개의 fresh daemon process에서 Native/CodeSearch의
  generation·고정 candidate·source SHA·ranked 결과를 대조하는 회귀를 추가했고,
  전체 배치에서 **통과, 4.385초**였다. 4,096-source binary restart도 두 target 모두 통과했다.
- Python capture/profile/timeline 회귀 **226개 통과**. 최종 정적 감사에서 모든 Health가
  `complete` footer와 모순된다는 검증을 수리하고 schema의 11개 코드 회귀를 추가했다.
  최초 workspace strict Clippy의
  decoder/fixture lint 실패와 전 target 수집의 scan-experiment 인자 누락을 수리했다.
  후속 수집의 313개 lint는 파일 소유권을 나눠 정적으로 수리했다. strict Clippy의
  최종 재검사 결과와 실행 source는 아래에 별도로 기록한다.
- 실제 trace `Invalid`의 producer 원인도 수리했다. harness query는 capture preface를
  협상한 동일 연결에서만 attempt token을 보낸다. 기존 plain fallback/deadline을 유지하고,
  작은 scale 회귀에서 `Complete`·health 이벤트 없음·client/server link를 검사한다.
- 공유 트리에 별도 index/kernel/storage 작업이 추가돼 전체 fmt·API/module snapshot
  범위가 달라졌다. 해당 source와 baseline은 덮어쓰지 않는다. 이번 변경만 통합한
  `/tmp/qi-oct11-closeout-qrsk8un_`의 `733082fa318c58d069e5182721fc1a19f14ba04c`에서
  format/no-allow/module-cycle/derive/hexagonal/wire/test-authority/error-shape가 통과했다.
  API/module baseline은 이번 diagnostic 변경만 검토해 갱신했다. 후속 고정 source의
  API/module guard는 통과했다. 진단 오류→Option 변환·테스트 constructor·마지막 binary
  target까지 수리한 고정 `c45eacb`에서 workspace/all-targets/all-features strict Clippy가
  통과했다. 단일 전체 workspace nextest 배치는 **4604 passed / 2 failed / 31 skipped**,
  3026.782초, exit 100으로 종료됐다. 두 실패는 parentless capture context를 넘긴
  semantic unit fixture다. 명시적 Operation root/child와 Health 없음 assertion으로 수리했고,
  기존 counter·row/membership 기대값은 유지했다. 후속 `ce22909`의 semantic strict Clippy와
  lib nextest **90/90개가 통과**했다. 같은 revision의 fresh daemon build와 실제 daemon
  SDK L2 **6/6개가 통과**했다. 원 전체 실패와 후속 검증을 구분한다.
- 마지막 정적 감사에서 IPC accept/spawn 오류가 기존 connection worker를 join하지 않고
  반환하는 경로와 runtime boot가 trace span 생성 오류 때문에 실패하는 경로를 수리했다.
  IPC는 shutdown→기존 worker join→socket 정리→원 오류 반환 순서를 유지한다.
  runtime은 typed Health를 기록하고 실제 build 결과를 반환한다. worker 종료 순서와
  trace parent 누락 시 실제 boot 성공·Invalid capture를 확인하는 회귀를 추가했다.
  최종 고정 source는 `1efa4dd443c37d814a827db9aef523aff96e6345`다. 이 마지막 세 파일의
  rustfmt·diff 검사는 통과했지만 생산 Rust 코드가 바뀌었으므로 앞선 Clippy/SDK 결과를
  최종 revision의 결과로 승계하지 않는다.
- 마지막 확인 당시 Battery Power 1%·예상 잔여 1분이었다. workspace doctest는
  resource-admission 대기 중 해당 실행만 종료했고 Cargo는 시작되지 않았다(`NOT_RUN`).
  최종 IPC/runtime 영향 Clippy·회귀, fresh daemon/SDK, doctest 및 새 source Large/XL은
  AC 전원이 없어 `BLOCKED`다. 이전 절전 중단 이력 때문에 대형 실행을 시작하지 않았다.
  공유 checkout의 다른 작업과 공용 lock은 중단하거나 우회하지 않았다.
- 운영 수용 정적 감사에서 LQ5의 sustained churn·long pin·space pressure, 측정에 근거한
  merge/admission 정책과 §9의 durable launcher 전환·crash cut은 별도 미완료 범위로 확인했다.
  두-source compaction 회귀와 두 fresh process 복원을 대규모 운영/제품 전환 수용으로 승격하지 않는다.
- 별도 담당 채팅의 P09는 별도 source에서 진행한다. 그 Off/Record·Large/XL 결과는
  현재 통합 source의 결과로 승계하지 않으며, 중복된 동일 소스 벤치는 실행하지 않는다.

## 2026-10-11 AC 연결 후 후속 검증

- 앞 절의 AC 부재는 당시 중단 사유다. 이후 AC 연결을 확인해 공용 admission 아래
  후속 검증을 재개했다. 다른 작업의 lock을 중단하거나 우회하지 않는다.
- IPC worker를 RAII owner로 묶어 오류 반환뿐 아니라 unwind에서도 shutdown과 join을
  수행하게 했다. `43046bb`의 IPC/runtime strict Clippy와 IPC unit **102개가 통과**했다.
  새 runtime boot 회귀는 필수 retention policy를 빠뜨린 fixture로 실패했다. 제품의 거절은
  유지하고 fixture에 명시적 policy를 추가한 `bdcc708339f431874922eecf8d2f87de18babff4`에서
  runtime strict Clippy, parent negative **2/2**, runtime 영향 **72/72**, 실제 daemon SDK
  **6/6**, workspace doctest **4개가 통과**했다. unchanged 전체 4,606개를 반복하지 않았다.
- 후속 Cargo 빌드가 같은 cache 경로의 daemon 파일을 바꿔 최초 binary SHA binding이
  성립하지 않았다. 배포 테스트는 실행 전 SHA 검사에서 거절됐다(`NOT_RUN`). canonical
  build 후 `/private/tmp/qi-oct11-final-deployment-searchd`로 고정 복사했고, SHA256은
  `efcf231c8f3f7a5079f0e390b95625ee8807a9920a631e90d19f61931c4bdae7`이다.
  이 불변 파일로 SDK **6/6**, 배포 회귀 **21/21**을 검증했다. 앞선 SDK 6개는 source 영향
  증거이며 최초 cache SHA에 묶인 증거로 표시하지 않는다.
- §9의 `tools/deployment/launcher.py`는 binary SHA/config/root를 하나의 private selection에
  file fsync→rename→directory fsync로 게시한다. 동일 deployment directory의 lock을
  child에 상속해 serve/drain 및 launcher SIGKILL 뒤에도 전환을 막는다. 운영 신호는
  한 번씩 전달하고 130초 후 자기 child group만 종료한다. `install`/`verify-bootstrap`의
  pristine artifact 검증과 일반 `serve`의 mutable-root boot 검증을 분리했다.
- 이 신규 도구·schema·독립 회귀와 wire inventory는
  `dd0f386ea2418930e529725e74797ffab35e42f3`에 포함됐다. Rust source/Cargo.lock은
  `bdcc708`과 byte-equal이다. Python lint/format, schema/allowlist 대조와 wire guard는
  통과했다. 실제 신호/SIGKILL·fresh daemon 전환·mutable-root 재시작 회귀는
  **21 passed / 0 skipped, 9.67초**다. 현재 운영 endpoint/외부 producer의 실제 전환은 수행하지 않았다.
- 추가 정적 수리의 write set은 search-plane의 `search_corpus_retention.rs`,
  `readiness/search_corpus_history.rs::restore_search_corpus_history_into`와 기존 history
  test 모듈이다. 전체 관측 세대가 count/byte 한도 내인 정상 restore에만 단일 전체 측정값을
  재사용한다. 과한도 planner, 손상·부재 처리, active/unresolved pin 검사와 마지막 전체
  physical census는 유지한다. port/format/한도 변경과 전역 cache는 범위 밖이다.
  수리를 반영한 고정 source는 `47f904adcce2f5c3eacd9761b24e25ef6af8ec2d`다.
  strict Clippy, retention/history **47/47**, 새 고정 daemon SDK **6/6**, 실제 런처 전환·재시작
  **1/1**이 통과했다. 이전 통과 배치는 수정 없는 범위로 유지하고 release scale만 추가 검증한다.
- 같은 `47f904a` source의 release build가 통과했고, Large는 producer exit 0,
  **87.881초**, 10개 phase 성공, Complete timeline·Health/loss 0이었다. canonical profile
  parity는 성립하지 않아 부분 진단이며, source/build binding도 `UNPROVEN`으로 유지한다.
- XL은 **900.083초 timeout, FAILED**다. 10개 phase와 재오픈 후 검색은 끝났지만 제품
  artifact와 trace footer가 없다. 마지막 driver join은 약 48ms였고 마지막 trace event는
  876.082초다. 이후 `rt.stop()`의 동기식 owned TempDir 삭제가 미계측 경로로 확인됐다.
  기존 trace만으로 timeout 원인을 확정하지 않는다. 삭제를 기존 deadline 안에서 계측하고
  I/O 오류를 보고하도록 수리했다. `a833970390eb69320897a90b672c1a7dc04c4092`의 harness
  strict Clippy, cleanup/teardown/root custody/reopen **13/13**, 실제 small-scale trace
  **1/1**이 통과했다. 삭제 실패·trace admission 실패·invocation parent 연결을 포함한다.
  이 수리는 계측/오류 보고이며 성능 개선이나 XL 해결 증거가 아니다. 새 Large/XL은 `NOT_RUN`이다.
- LQ5 staged worker·측정 기반 merge 정책·지속 부하는 별도 미완료 범위다. quiet-host
  timing은 외부 Rust/load 경합 때문에 `BLOCKED`다. 앞선 전체 실패와 영향 범위별 성공을
  하나의 green 전체 배치로 합치지 않는다.

## 확정 결정

| 작업 | 구현 | 완료 기준 |
| --- | --- | --- |
| A semantic compatibility | 누락된 모델·정책 저장과 target/base 순수 preflight | 비호환 계약을 첫 window/provider 및 최종 event binding 전에 거절 |
| D RepoMap lifetime | 기존 store gate의 acquire/pin/publish/activation/GC 보호 범위 수리 | 성공한 pin의 객체 보존, 마지막 drop 뒤 reclaim, 실제 unwind 회귀 |
| C5 generation reservation | 기존 catalog의 high-water, event별 선발급, 1회 최종 body binding | 번호 비재사용, 원 예약 재시도/재시작 복구, control/SDK 연결 |

A/D는 현재 owner의 직접 수리다. C5는 새 기능이므로 catalog/control/ingress/SDK가 모두
연결된 한 묶음으로 수용한다. 실행 결과와 미실행 범위는 [VALIDATION.md](VALIDATION.md)에 기록한다.
최종 Native 6/File 18/manifest 19 생산 소스로 workspace `check`·strict Clippy와 기본
nextest 4,532개를 실행했다. 전체 배치의 GC fixture 실패는 보존하며, test oracle 수리
후의 owner 모듈·SDK L2·doctest 성공과 합산해 전체 성공 배치로 표시하지 않는다.
기본 제외 31개는 별도 범위다. 명령·소스 범위·결과는 [VALIDATION.md](VALIDATION.md)의
마지막 절을 따른다. 별도 lexical 성능 수용·외부 producer·release 완료를 뜻하지 않는다.

## 고정 불변식

| 경계 | 불변식 | 실패 시 동작 |
| --- | --- | --- |
| semantic inheritance | 같은 차원이라도 모델·revision·정책이 다르면 base 벡터를 상속하지 않음 | provider/첫 window·신규 최종 binding·target 변경 전에 거절 |
| semantic recovery | 정확히 완료된 원 target의 복구는 새 delta build와 구분 | 기존 receipt/roots/sequence로 복구, 재임베딩·GC된 base 재요구 없음 |
| RepoMap acquire/GC | 성공한 pin 등록과 해당 객체 unlink 사이에 단일 직렬화 순서가 있음 | retire가 먼저면 acquire 거절, acquire가 먼저면 마지막 drop까지 GC 보류 |
| allocation durability | 동일 root incarnation의 repo/revision 안에서 high-water가 감소하지 않음 | persist 불확정 시 성공 ACK 금지, reopen 후 원 event로 재시도 |
| event binding | target-independent source payload와 발급 번호를 최종 body에 한 번만 연결 | payload/base/revision 또는 최종 digest/key 변경은 conflict |
| activation authority | 번호 발급·publication receipt는 activation 권한이 아님 | 기존 explicit CAS 검사, conflict 후 자동 expected-head 갱신 없음 |

## A semantic compatibility

- [x] `GenerationContract`와 manifest에 기존 모델의 `policy_digest`/`view_policy_digest`를
  보존한다. writer/reader/validation을 함께 바꾼다. 옛 format은 typed rebuild refusal,
  현 format의 필수 key 누락은 decode/storage corruption이다.
- [x] semantic owner 한 곳에서 target와 delta base를 검사한다. model ID/revision/dimension/
  metric/normalization/embedding policy/view policy 및 공통 corpus policy를 비교한다.
  generation/mode/base 번호, required-corpora와 row별 render 집합 전체 equality는 요구하지 않는다.
- [x] 직접 build는 첫 `next_window()` 전에 기존 target/base guard 안에서 검사한다.
  paired ingest는 provider·최종 event binding 전과 operation lock 안에서 같은 검사를 한다.
  쓰기를 수행하는 `ensure_generation_contract`를 순수 preflight 대신 쓰지 않는다.
- [x] 기존 `embedding_model_contract_for` 투영을 공유한다. 빈 필수 identity/policy와
  base/target의 None·빈 revision으로 delta 상속하지 않는다. full replacement admission은 유지한다.
- [x] terminal replay와 complete-target finalize 복구를 유지한다. 완료된 원 작업을
  재임베딩하거나 GC된 base를 다시 요구하지 않는다. 기존 query ID/revision gate도 유지한다.

C5 선발급은 별도 요청이다. 이미 발급된 번호가 A의 publish preflight 거절로 취소되는 것은
아니다. 거절 oracle은 provider/첫 window·신규 최종 binding·target 변경 0이며,
기존 immutable Prepared journal과 이미 발급된 번호의 보존은 허용한다.

변경 표면: [semantic 계약](../../../crates/quanta-index-semantic/src/generation_contract.rs),
[manifest](../../../crates/quanta-index-semantic/src/manifest.rs),
[build](../../../crates/quanta-index-semantic/src/build.rs), Core semantic port,
[semantic_derive](../../../crates/quanta-index-search-plane/src/semantic_derive.rs),
[paired ingest](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs)와 해당 회귀.

구현 순서:

1. `GenerationContract::{from_batch,merge_batch}`와
   `SemanticManifest::{from_generation_contract,validate_against}`의 누락 필드를 함께 보완한다.
   codec의 필수 key 계약과 format stamp를 함께 변경하며 기존 `EmbeddingModelContract`를
   그대로 사용한다. 별도 V2 모델 IR이나 fallback reader를 만들지 않는다.
2. 읽기만 수행하는 target/base compatibility 검사를 semantic owner에 둔다. 새 필수 port를
   추가하면 모든 production adapter와 test double을 함께 수정한다. 기본 `Ok(())` 구현은 금지한다.
   target의 기존 계약 검사와 delta base의 sealed marker/manifest/build contract 검사를 구분하고,
   이미 저장된 target의 mode/base 불변식은 유지한다.
3. 직접 경로는 `generation_build_guards`와 directory lifecycle guard 안에서 첫 window 전에 검사한다.
   paired 경로는 provider 없는 모델 투영을 공유하여 BeforeIntent와 UnderOperationLock에 연결한다.
   preflight 결과를 이후 mutation의 영구 허가로 캐시하지 않는다. 실제 build 진입도 같은 검사를 한다.
4. current query의 모델 identity gate, exact terminal replay, finalize-only/partial recovery를
   함께 검증한 뒤 A 전체를 통합한다. full replacement는 새 generation에서 모델 변경을 허용한다.

## D RepoMap lifetime

- [x] 기존 `activation_commit_gate`로 head/registry 확인→pin 등록을 보호한다.
  GC도 같은 gate에서 retirement/pin을 재확인하고 unlink/registry 제거까지 수행한다.
- [x] publish의 최종 seal/catalog/registry 설치와 activation을 같은 lifecycle로 보호한다.
  기존 replay의 object 검증도 GC와 경합하지 않게 한다. replay 응답/보존 정책은 바꾸지 않는다.
- [x] 순수 compile, query 본문, lease Drop은 gate 밖이다. Drop은 기존 pin table만 사용한다.
  실제 call graph의 lock order와 재진입을 확인하며 새 lock/registry를 추가하지 않는다.
- [x] panic 테스트는 active view 확보, pin=1, 의도한 body 진입, 정확한 panic payload,
  unwind 후 pin=0과 후속 reclaim을 각각 확인한다.
- [x] acquire의 read→pin 경계를 barrier로 고정해 activation/GC와의 직렬화를 검사한다.
  publish/replay/activation은 같은 gate 사용과 기존 owner 회귀로 확인한다.
  sleep 반복이나 `catch_unwind().is_err()`만으로 통과시키지 않는다.

보장 범위는 기존 state-root lease 아래 한 store Arc를 공유하는 daemon이다.
독립 public `open()` 복수 handle의 보장이나 전역 lifecycle manager로 범위를 늘리지 않는다.

변경 표면: [store](../../../crates/quanta-index-repomap/src/store.rs),
필요한 [pin lease](../../../crates/quanta-index-repomap/src/pinned.rs) 연결과
[lifetime owner 테스트](../../../crates/quanta-index-repomap/tests/read_view_lifetime_owner_v1.rs).

구현 순서:

1. 기존 panic oracle을 먼저 교정한다. 현재 테스트는 G2 활성화 뒤 G1을 acquire하여
   의도한 본문 전에 panic할 수 있다. active generation acquire 성공과 pin=1을 확인한 다음
   의도한 panic을 발생시키고, 정확한 payload와 unwind 뒤 pin=0을 확인한다.
2. 기존 gate가 acquire의 head/registry 조회부터 lease 생성까지, GC의 최신 retired/pin 재검사부터
   unlink/registry 제거까지 보호하도록 한다. compile은 밖에서 수행하고 publish의 최종
   replay/pin 재검사 및 seal/catalog/registry 설치는 gate 안에서 수행한다.
3. `ingest_bundle_v2`의 exact replay/object 검증과 `commit_activation`을 같은 규칙에 연결한다.
   gate를 획득하는 entry와 이미 guard를 가진 helper를 구분해 중복 획득을 없앤다.
   gate 아래에서만 store/catalog의 짧은 guard를 사용하고, pin guard를 쥔 채 gate를 기다리지 않는다.
4. 읽기 본문과 Drop은 기존 Arc/pin table만 사용한다. read-to-pin 및 check-to-unlink 경계의
   test-only barrier로 실제 경쟁 순서를 고정한다. 정상 query 경로에 gate나 테스트 hook을 추가하지 않는다.

## C5 generation reservation

번호 예약은 activation 권한이 아니다. 기존 publish와 activation CAS 분리를 유지한다.

- [x] repository/revision과 기존 `SourcePublicationEvent`로 번호를 선발급한다.
  기존 `(repository, stream, event)` identity를 유지하며 revision/payload/expected source base
  변경 재사용은 conflict다. 응답은 원 번호와 event binding이며 publish receipt가 아니다.
- [x] 기존 repository envelope에 revision별 영속 high-water를 둔다. 현 형식에서 등록된 양 track의
  sealed/in-flight/rollback/history와 기존 event의 점유보다 높은 번호를 발급하고 persist 후 응답한다.
  명시적 generation writer도 같은 점유 admission에 참여한다. 예약된 target 탈취를 막고
  explicit writer가 high-water를 추월한 경우 다음 발급에 반영한다.
  sealed 목록은 미완성 디렉터리를 제외하므로 floor의 유일한 근거로 쓰지 않는다.
  최초 pair는 adapter 소유의 순수 empty 검사로 확인한다. 기존 미추적 점유가 있거나 empty를
  확인할 수 없으면 typed refusal한다. 기존 root의 floor를 추정해 자동 편입하지 않는다.
- [x] GC/rollback/timeout으로 번호를 재사용하지 않는다. gap은 허용한다.
  overflow/capacity/저장 불확정은 기존 typed 오류로 처리하며 누락 high-water를 0으로 읽지 않는다.
  persisted format은 한 번 변경하고 구형은 typed rebuild refusal로 처리한다.
- [x] 기존 event record에 최종 body가 없는 선발급 상태 하나만 추가한다.
  발급 번호를 포함한 최종 body를 검증한 뒤 canonical digest/journal key를 한 번 연결하고
  기존 Pending→Staged→Active 경로를 사용한다. fake digest/key나 별도 journal은 만들지 않는다.
- [x] 같은 event 재시도와 restart는 원 번호/최종 binding을 돌려준다. 다른 body/key의
  재결합은 거절한다. 기존 single-pending 및 오래된 미완료 event의 순서 제한을 유지한다.
  선발급 상태도 기존 unresolved target 보호에 포함하며 timeout으로 자동 해제하지 않는다.
  unresolved selector는 digest 없는 선발급과 digest 있는 bound target을 구분한다.
  선발급에는 generation 점유/보호를, bound에는 기존 exact digest 검사를 적용한다.
- [x] 제어된 restore 뒤 이전 예약의 재사용은 저장된 기존 root incarnation과 현재 값을
  binding 시 비교해 거절한다. 자동 rebase나 root 전체 reconciliation을 추가하지 않는다.
  이 최소안은 중단된 event의 무조건 진행이나 restore 이후 자동 stream 해제를 보장하지 않는다.
- [x] 기존 control mutation 하나를 Admin admission으로 공급하고 SDK
  `SearchCorpusNamespace`에 예약 메서드 하나를 연결한다. codec/오류/실제 exhaustive match와
  native admission을 함께 수정한다. `GenerationNamespace`에 중복 facade를 두지 않는다.

**activation CAS는 예약에 넣지 않는다.** caller가 기존 `activate_published`/
`publish_and_activate`에 명시적으로 전달한 expected head를 기존 방식으로 검사한다.
SDK가 충돌 후 expected head를 자동 갱신하지 않는다. head 변경/rollback은 번호 예약을
자동 취소하지 않으며, 예약 시 frozen CAS 때문에 생기는 별도 Superseded 상태 축도 도입하지 않는다.
동시 번호 발급은 같은 pair의 병렬 publication이나 무조건 진행을 뜻하지 않는다.
새 allocation 요청의 revision 변경 conflict와 기존 publish replay는 구분한다.
이미 bound된 event의 publish replay가 새 요청 target 대신 원 target을 복구하는 기존 계약은 유지한다.

변경 표면: Core [source publication](../../../crates/quanta-index-core/src/domains/source_publication.rs),
[activation catalog](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs)의
envelope/source-events, 필요한 기존 unresolved selector,
Contract control/split/codecs, control/ingest dispatcher와
[SDK SearchCorpusNamespace](../../../crates/quanta-index-sdk/src/lexical.rs).

구현 순서와 선형화 지점:

| 단계 | 변경 | 완료/선형화 기준 |
| --- | --- | --- |
| C1 계약 | 기존 event record를 선발급과 최종 binding을 구별하는 단일 canonical 타입으로 변경 | 선발급은 generation/event/incarnation만 보유하고 fake manifest/key를 요구하지 않음 |
| C2 발급 | 기존 envelope에 pair별 high-water와 원 event의 선발급 기록 추가 | 기존 `persist_repository`의 durable 완료 뒤 ACK. memory-only 발급 없음 |
| C3 최종 연결 | 검증된 body로 기존 `reserve_source_event(binding)`을 호출 | 원 예약 번호와 최종 manifest/batch digest/journal key를 한 번 영속 연결한 뒤 양 track 변경 |
| C4 경로 연결 | control Admin admission, codec, dispatcher/native match, SDK 메서드와 response binding | 실제 UDS 요청이 owning catalog를 호출하고 원 event/repo/revision/generation을 검증 |
| C5 복구 | 기존 journal/reconcile/activation과 unresolved selector 연결 | Allocated→bound Pending→Staged→Active, 원 event retry/restart와 explicit CAS 유지 |

`source_event_payload_sha256`는 이미 containing revision/generation/manifest/transport digest/seal을
제외한다. 이를 선발급의 source fingerprint로 사용한다. 최종 body의 batch digest와 혼동하거나
번호를 받기 위해 임시 generation/가짜 digest를 만들지 않는다. 새 allocation의 revision 비교는
이 fingerprint와 별도로 수행한다. 최종 publish에서 기존 source payload 재검증도 유지한다.

W0에서 실제 production ingress와 번호 점유의 관계를 먼저 닫는다. 현재 wire의 paired track
입구는 `PublishSearchCorpusBatch` 및 그 경로로 합류하는 staged upload다. 같은 root에 쓰는
추가 production 경로가 발견되면 첫 physical mutation 전에 동일 점유 admission에 연결한다.
seal 이후 inventory만으로 미완료 target이 없다고 추정하지 않는다.

lexical generation 디렉터리를 만드는 recency/topic/description/ownership/contributor/meta
overlay 6개 경로는 새 apply 전에 필수 read-only
`SourcePublicationCatalogPort::require_reserved_generation(pin)`을 호출한다.
현재 root incarnation의 정확한 repo/revision/generation 예약이 없으면 재시도 가능한
`NotReady`다. 이 검사는 번호를 발급하거나 journal을 bind하지 않으며, sealed target의
쓰기 금지는 adapter가 계속 검사한다. 원 terminal replay는 이 admission보다 먼저 해소한다.
overlay를 먼저 발행하는 caller는 완성된 source intent를 예약한 뒤 overlay를 발행하고,
예약한 것과 동일한 source event/payload로 최종 source를 발행한다.
harness도 source body를 고정·예약한 뒤 overlay를 전송한다.

하위호환은 제공하지 않는다. 구형 envelope/semantic format을 현재 형식으로 자동 변환하지 않는다.
high-water가 없는 pair의 최초 생성은 양 adapter에서 기존 점유가 없음을 확인한 경우에만 허용한다.
미추적 점유가 있으면 typed rebuild/refusal이며, 이 범위에 기존 root의 자동 복구·최대 번호 추정
migration을 추가하지 않는다. 새 형식의 명시적 generation publish도 persist된 점유와 high-water를
갱신한 뒤 쓰며, 이미 다른 event에 선발급한 번호는 사용할 수 없다.

counter 단조성은 동일 root incarnation 안의 보장이다. 제어된 restore는 기존 incarnation 회전을
사용하고 이전 예약의 binding을 거절한다. restore를 가로지르는 전역 단조 counter나 자동 stream
해제를 보장하지 않는다. 미완료 event는 기존 capacity/single-pending 제약을 소비하며 취소/TTL은 없다.

## Execution order

최신 실행 규칙: 계약/인터페이스 코드 → 정적 검토 → 독립 모듈 병렬 구현 → 통합 정적 검증 →
runtime 일괄 실행 순서다. 이전의 lane별 선행 runtime/개별 green 통합 순서보다 이 규칙이 우선한다.

- 담당끼리 직접 메시지·followup·재위임·새 채팅 생성은 하지 않는다. 부모가 단계 배정과 결과 취합을 한다.
- 계약 단계는 선언·타입·불변식·codec 표면을 먼저 고정한다. 모듈 단계에서 계약 변경이 필요하면
  임의로 공용 파일을 바꾸지 않고 자신의 종료 보고에 기록한다. 부모가 판단·반영한다.
- worker는 build/check/test/clippy/daemon/provider/benchmark를 실행하지 않는다.
  소유 파일의 rustfmt parser/check, rg/caller 대조, scoped diff/whitespace 검사를 수행한다.
- 모든 모듈을 통합한 뒤 부모가 compile/static checks를 실행하고, 그 다음 focused tests·daemon·SDK L2를
  한 번에 하나씩 모아 실행한다. 정적 통과를 runtime 성공으로 기록하지 않는다.

실행 배정은 A/D/C5 owner 3개 + 부모 통합 1개다. 계약 단계부터 각 담당을 실행한다.
독립 코드 작성은 병렬로 배정할 수 있고, 무거운 build/native
검증은 한 번에 하나만 실행한다. 기존 dirty 문서·benchmark·Justfile·CI 파일은 다른 작업 소유다.

W0에서 고정할 연결 계약은 다음 세 가지뿐이다.

- A: 필수 `preflight_contract(pin, semantic_contract) -> Result<(), CoreError>` port와
  provider 없는 모델 투영. target/base 검사 및 completed-target 복구 예외를 함께 전달한다.
- C5: `allocate(repo, revision, event)`의 원 번호/incarnation 응답과 검증된 최종 body의
  1회 binding. 명시적 generation의 기존 `reserve_source_event(binding)`도 같은 점유 admission 사용.
- C5 unresolved target: 선발급의 generation-only 점유와 bound의 exact identity를 표현한다.
  fake manifest digest를 넣지 않는다. wire에는 내부 catalog 타입을 노출하지 않는다.

최초 counter 생성에 필요한 양 adapter의 순수 empty/점유 검사는 W0에서 별도로 고정한다.
Core `domains/generation.rs`의 최소 read-only 경계로 제공하고 sealed scan과 구분한다.
디렉터리 이름을 catalog가 직접 읽거나 sealed-only 결과를 empty로 투영하지 않는다.
이를 소유하는 adapter 구현과 모든 test double이 연결돼야 C5 발급을 활성화한다.

| 단계 | A | D | C5 | 통합 |
| --- | --- | --- | --- | --- |
| W0 | preflight port/모델 투영 고정 | gate call graph/panic oracle 확인 | allocate/bind/selector shape 고정 | 소유권 확인, 공유 exports·wire shape 고정 |
| W1 | A1 persisted 계약 → A2 직접 preflight/adapter → A3 회귀 | D1 panic oracle → D2 acquire/GC gate → D3 publish/replay/activation 회귀 | C1 Core/selector → C2 envelope/high-water → C3 allocate/bind/restart 회귀 | I1 control DTO/codec → I2 Admin/native/SDK binding. catalog 완성을 기다리지 않음 |
| W2 | paired 연결 요구 인계 | 독립 구현 제출, 다른 lane 검토 가능 | explicit writer admission 요구 인계 | I3 공용 ingest에 A 후 C5 연결, 공용 test double/composition 수정 |
| W3 | 먼저 비는 A/D lane이 SDK facade/consumer 회귀 담당 | SDK 담당이 아니면 lifetime/lock order 검토 | 점유·restart·binding 회귀 보완 | I4 집중 테스트 → 영향 daemon → fresh-binary SDK L2 직렬 실행 |

SDK 작업은 W3까지 대기시키지 않는다. I1 wire가 고정되고 A/D 중 한 lane의 독립 구현이
제출되는 즉시 SDK 파일을 그 lane에 인계한다. 그동안 통합 담당은 I3를 진행한다.
아직 빈 lane이 없으면 통합 담당이 I2에서 SDK를 작성한다. 같은 파일의 동시 writer는 두지 않는다.

핵심 의존성은 C1→I1이며, 이후 C2/C3와 I2/SDK는 병렬이다. I3는 A2와 C3 계약을 소비한다.
D는 독립 진행한다. C5의 점유 증명이 막혀도 A/D 구현·집중 검증은 계속한다.

D의 publish compile은 gate 밖에서 하고 최종 설치 전에 gate 안에서 replay/pin을 재확인한다.
이미 gate를 잡는 `commit_activation`의 바깥 caller에서 같은 mutex를 다시 잡지 않는다.
I3는 같은 state root에 쓰는 실제 production ingress를 확인해 첫 physical mutation 전에 점유를
기록한다. 그 증명이 없는 우회 경로를 sealed 목록만으로 안전하다고 처리하지 않는다.

각 handoff는 변경 파일, 호출 서명, 거절 조건, 회귀 selector와 미실행 항목만 전달한다.
공용 파일에 대한 병렬 patch·동시 workspace format·공용 schema/API 변경은 하지 않는다.

## Ownership

아래 write set을 고정하며 착수 전 현재 dirty hunk와 writer를 확인한다.

- A: `crates/quanta-index-semantic/` 전체(`src/lib.rs` adapter impl 포함),
  Core `domains/semantic/`와 `tests/semantic_stream.rs`, search-plane `semantic_derive.rs`,
  `ingest_dispatcher/semantic.rs`와 `ingest_dispatcher/tests/semantic.rs`.
- D: `crates/quanta-index-repomap/`와 해당 테스트.
- C5: Core `domains/source_publication.rs`, search-plane `readiness/activation_catalog*`,
  `search_corpus_lifecycle/pair_lock.rs`의 unresolved-target port/type,
  필요한 `search_corpus_history.rs` selector 소비와 catalog 테스트,
  lexical `adapter_lifecycle.rs`의 순수 점유 검사와 owning 회귀.
- 통합: 공용 exports/API inventory/Contract control·split·codec, control dispatcher,
  `ingest_dispatcher/{dispatcher,search_corpus,generation_plan}.rs`, 공용 테스트
  `ingest_dispatcher/tests/{support,search_corpus,idempotency}.rs`, composition root와 필요한 Cargo 변경.
  실제 match인 SDK `binding.rs`, searchd `app/ipc_dispatcher.rs`와 native admission도 포함한다.
  SDK `lexical.rs`/`binding.rs`/전용 consumer 테스트는 wire 고정 후 빈 A/D lane에 일괄 인계할 수 있다.
  Core `domains/generation.rs`의 점유 port와 공용 `lib.rs` exports는 통합 전용이다.
  semantic `src/lib.rs`는 A의 독립 변경 제출 전까지 A 전용이다. 그 뒤 C5 점유 검사 impl을
  해당 파일의 writer와 함께 명시적으로 인계한다. C5와 A가 동시에 수정하지 않는다.
  Core/search-plane 공용 `lib.rs`는 통합 전용이다.

공유 파일은 단일 writer가 수정한다. handoff 전 preimage를 확인하고, C5는
catalog/core/control/ingress/SDK가 연결된 전체 변경으로 수용한다. 독립 A/D 검증에 외부
Semantica build를 선행시키지 않는다.

## Main 통합 단위

| 묶음 | 포함 파일/경계 | main 수용 조건 |
| --- | --- | --- |
| D | RepoMap store/pin과 lifetime/candidate owner 회귀 | 실제 pin/unwind oracle, acquire/GC barrier, publish/replay/activation gate와 owner 회귀 |
| A | semantic persisted 계약/adapter/Core port, 모델 투영, paired ingest/test double | 필드 round-trip·비호환 무부작용·동일 계약 delta/full rebuild·복구 회귀 |
| C5 | allocator/envelope/Core selector, control DTO/codec/admission, 공용 ingest, SDK/consumer | 번호 점유·영속 재시도·최종 binding·실제 UDS/restart/explicit CAS 전체 검증 |
| 통합 검증 | 영향 build/API inventory, daemon, 새 binary SDK L2 | 동일 통합 소스에서 직접 회귀 통과, 실행하지 못한 항목 명시 |

부모가 A/D/C5의 정적 검토 결과를 취합해 전체 소스를 통합하고, 공통 compile/static 검사를
완료한 뒤 runtime 검증을 직렬 실행한다. API만 노출하거나 catalog만 연결한 C5 중간 상태를
완료 결과로 기록하지 않는다. 문서 정리·benchmark 변경과 섞어 commit하지 않는다.

각 묶음 통합 전 main과 실제 diff를 대조하고, 해당 파일의 dirty 소유권을 확인한다. 이미 포함된
패치는 재적용하지 않는다. 통합 뒤에는 영향이 달라진 검사만 재실행하고 같은 통합 소스에서
daemon/SDK L2를 직렬 실행한다. 검증되지 않은 public match, test double, API inventory 갱신을
후속 TODO로 남기지 않는다. C5 전용 schema/API의 공용 파일은 끝까지 통합 담당 단일 writer다.

## 이번 범위에서 제외

- RepoMap GC 후 object-free 역사적 receipt 복구와 catalog/envelope alias 추가 hardening.
- C5 publication/activation 이중 상태 축, 자동 Superseded/cancel, rollback과 예약의 원자 갱신,
  root 전체 restore reconciliation, retention 정책 재설계.
- 다중 모델 migration, RCU/새 registry/별도 journal, Loom·새 simulator·외부 fault platform 도입.
- 장기 event-history rollover, 일반 성능 최적화, 새 자원 budget, source format/config 확대.

제외 항목을 현재 안전성이 증명된 것으로 표시하지 않는다. 기존 fail-closed/typed busy·보존
계약을 유지하며, 구체적 제품 요구나 재현된 결함이 있을 때 별도 범위로 판단한다.
정식 benchmark·release·외부 producer 수용은 이번 수정의 완료 gate에 넣지 않는다.
