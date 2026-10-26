# E07 — 총비용·성능·관측·운영

상태는 [SSOT E07](README.md#e07). 원 요구 R13/R39, 종전 W2-09/10과 각 기능의 성능/운영 결과를 소비한다. PHYSICAL/harness와 기존 SEARCH 원4K의 단일 실행 큐를 유지한다.

## Fixed profiles and oracles

- 기존 P00–P09 trace/recorder/exporter·제품 ReadSession/renew/release/settlement를 재사용한다. 실패·timeout·reopen stop/join/start에서도 결과가 남고 observer 비용/누락을 분리해야 한다.
- Large=4096파일(seed42,16×256,평균120행), XL=32768파일의 독립 functional ACK/search/reopen profile을 유지한다. debug 결과와 bench/release throughput은 구분한다.
- 기존 Large 운영 profile: Linux aarch64 전용2CPU/6GiB·local SSD, 같은 bench binary/caps·논리 work와 실제 RSS/공간을 분리한다. host/storage 경합은 qualified timing이 아니다.
- 원960초 배치:60초 warmup+900초 open-loop50QPS, 동시4/대기64·초과도 실패,1초마다≤16KiB 파일 replace/delete/restore. ≥10,000 attempts/각 query class≥1,000,old-view witness와 실패/timeout·scheduled queue wait 포함.
- 실제 ReadSession pin600초,300000ms lease·120초 renewal/gap≤180초. 마지막 borrower 뒤 release·60초 이내 settlement. 기존 full/mixed의 cap·retention 선택은 원4K 요청 예산을 변경하지 않는다.
- 정식 Off/Record는 같은 binary/input/caps의 직렬 교대 Large≥3쌍·각 고정 phase paired ratio 중앙값 증가≤5%·drop0. noise/partial capture로 PASS를 만들지 않는다.

### Existing execution and adjudication path

The existing `tools.benchmark.retrieval.causal_cost_capture` module is the one
capture front door. It invokes the prebuilt bench `scale_matrix` and bound
Native daemon, validates source revision/binary SHA/epoch, and revalidates
product summary/manifest plus Record parent and both first/reopen child traces.
Do not create another runner or treat diagnostic artifacts as qualified timing.
The capture front door requires a clean bound source; current dirty-source
focused proof is not automatically that formal source qualification.

- Large and XL functional gates select `--backend index-native --tier large` or
  `--tier xlarge`, seed42, explicit frozen four-axis work limits and fresh
  external output. XL's four-axis qualification values must be frozen before
  execution; a necessary mutation floor is not a sufficient whole profile.
- Formal Large observer comparison alternates Off/Record at least three pairs
  on the same prebuilt binaries/input/caps/host/cache. The existing validator
  checks each capture and product result; the allowed final adjudication uses
  the fixed phase walls in `artifact/summary.json`. No existing automatic
  three-pair phase median/noise verdict is claimed. Preserve the <=5%/loss0 gate.
  One observed managed build supplies the same copied bench/daemon files to all
  six arms; repeating `--managed-build` per arm does not guarantee that contract.
  Reuse exact paths/SHA/epochs through the existing prebuilt inputs and fresh
  external arm outputs. Prebuilt `build_binding=UNPROVEN`/PARTIAL_DIAGNOSTIC
  remains unchanged. Separately review transitive source custody from the
  original observed-build receipt through every arm's source/epochs/raw/product
  validation, then adjudicate fixed-phase ratios/noise/loss. A caller receipt
  never grants OBSERVED_LOCAL_BUILD, and arm status is not renamed by this manual
  adjudication. No second runner or six-arm orchestration is required here.
- The existing `--native-mixed-load` path is Large42/Off with explicit work
  limits and sufficient overall max-seconds. It selects the fixed60+900s,
  deterministic50QPS,4 workers/64 queue, scheduled-to-reply deadlines and
  1Hz publisher. The actual ReadSession holds600s, renews its300s lease every
  120s and validates last-release settlement within60s. The original query and
  publisher thresholds are checked before a valid product artifact is emitted.
- NativeSampler's Linux process100ms/disk500ms observations retain PID birth,
  unique-inode allocated state/scratch accounting and missing/error counts.
  Python revalidates the typed method, epoch and availability and rejects
  sampled ceiling violations. Under-cap sampled values or incomplete sampling
  never establish the continuous high-water SLO. Whole-lifetime resources,
  physical faults/churn and the Linux aarch642CPU/6GiB/local-SSD profile remain
  explicit final qualification inputs.

Read-only environment inventory on2026-10-25 finds a reusable Linux arm64
Rust1.92.0 toolchain image `qidx-ann-toolchain:20261018-root`, ID
`sha256:61cfd13d3b65f009cc5ea9771c240a932f634c7893b9816dba4f48b6f6dc331c`.
Its old source/registry mount labels establish neither current source nor a
current compiler cache/binary. The current Docker Desktop Linux aarch64 engine
supports CPU/memory limits and cgroup v2 but runs49 containers. No container is
started or changed for this audit. The engine is not a proved dedicated quiet
2CPU/6GiB/local-SSD qualification host. Clean source plus bound current Linux
bench/daemon SHA/epoch, actual local-SSD authority, delegated cgroup and host
ownership remain required inputs; existing foreign E02 containers are not an
E07 allocation. Do not start a competing heavy build beside E04's live original.

This source audit confirms existing producer-to-validator wiring; it runs no
Large/XL/mixed body and closes no operating or overhead gate. Continue through
this same E07 queue after E04's original4K result and required source/host freeze.

## Capture capacity before a full Record run

현재 기본 recorder는 queue4096건·record4096bytes·output64MiB이고, canonical offline timeline parser는128MiB를 상한으로 둔다. source96578416…의128-owner complete capture는46342events/9269407bytes였다. 이를 단순32배 하면 약283MiB이므로 원4K Record 전체가 두 상한을 넘을 가능성이 있다. 이는 밀도 환산이며 실제4K 크기나 실패를 측정한 결과는 아니다.

상한 초과는 recorder의 unwritten/partial로 남고, 기존 `causal_cost_capture.py`는 complete가 아닌 timeline을 거절한다. daemon/product test의 성공 종료만으로 trace complete를 주장하지 않는다. 정식 Record 실행 전에 writer와 offline parser의 동일 capture 범위를 함께 정하고 실제 footer·written/unwritten·drop·lease/open-span을 확인한다. IPC readiness 수리 후 source8b63…의128-owner 진단은232378events/약43MiB로 늘었고, polling 호출도614→17584로 달라졌다. no-progress pacing 후의 실제 밀도와 동일 workload를 다시 고정해야 하며, 이 서로 다른 진단을 paired overhead로 비교하지 않는다. 파일 상한만 키워서 전체 RSS/관측 overhead가 검증됐다고 취급하지 않는다. 제품 요청·worker·품질 예산은 변경하지 않는다.

이전 cf6a3314…는 pacing과 Catalog wait/hold를 함께 사용한128-owner capture가107831events/21108098bytes로 COMPLETE이며, 상태조회3912회와 정확128/128 완료를 확인했다. 별도 region 초기/reopen capture2개도 COMPLETE다. [직접 비용과 검증 경계](VALIDATION.md#e04-e06-catalog-control-closure-20261024)를 따른다. 이128 밀도를 단순32배 하면 약644MiB로 두 상한을 넘는다. 환산은 실제4K capture 크기·누락·RSS 측정이 아니며, 현재 작은 capture의 complete 판정을 전체 Record 수용으로 올리지 않는다. Worker의 Catalog hold32.017s/operation41.851s와 wait0.230s는 잠금 안의 durable 비용을 지목하지만, parent-sync 정산 계약을 생략하거나 다른 source 진단과 paired speedup을 계산할 근거가 아니다.

청크 배치 수리 후 d07ba483…의 실제128 capture도 COMPLETE/유실0이며,
103963events/20353421bytes,정확128/128·상태조회3680회를 확인했다. 설치쌍은
이전128 진단657→현재649,worker 관리 COMMIT1314→1298이다. 이는 작업 수의
감소이며 경합·관측 workload가 달라 paired latency/overhead 수용이 아니다.
현재 밀도의 단순32배는 약621MiB로 여전히 writer/parser 상한을 넘는다.
현재 worker Catalog hold32.093s/operation42.181s,wait0.0648s와 전체 trace3개의
검증 경계는 [최신 원장](VALIDATION.md#e04-original4k-and-bounded-installs-20261024)에 있다.
clock 동일 시각 생략의 실제 빈도·효과는 미측정이다. 원4K를 다시 통과했거나
정식 Off/Record·전체 process RSS/공간을 수용한 것으로 표시하지 않는다.

후속 분류 계측 df419812…의 실제128은 v4 COMPLETE/유실0,101392events/
19858532bytes이다. 단순32배는 약606MiB로 writer/parser 상한 문제는 그대로다.
Worker32.829s 중 Catalog hold24.371s/wait0.018s,overwrite parent5.184s를
직접 구분했다. Parent 전량 생략의 이상적 산술도 약1.188배이며,내구성 동등성·
실제 credit 적중률·원4K 수용을 입증하지 않는다. Host load32.274/외부 Rust2로
이번 실행도 경합 진단이다. [현재 분류·검증 원장](VALIDATION.md#e04-native-sync-mutation-classes-20261024)을
사용하고 다른 source의 wall 차이를 speedup 또는 observer overhead로 계산하지 않는다.

후속 결합74f80886…은 daemon3개를 포함한 영향82/82가 통과했다.128 capture와
region 초기/reopen capture3개 모두 v4 COMPLETE/유실0이다.128은117199events/
22881188bytes,설치그룹521·관리 COMMIT1042로 df419812의649/1298보다 줄었다.
다만 polling은4848회,host는외부Rust9/load21.296387로 달라졌고,worker wall은
61.034s/Catalog hold45.941s/wait0.251s다. 작업 수 감소만 확인하며 속도·overhead
비교로 승격하지 않는다.128 밀도의 단순32배는 약698MiB로 writer/parser 상한
문제가 남는다. [현재 구현·계측](VALIDATION.md#e04-prepared-delta-closeout-20261024)을
따르며 원4K·정식 paired Off/Record·연속 RSS/공간·운영 DoD는 별도 수용한다.

Readiness observer도 workload로 고정한다.74f80886 원4K Off 실패는106944회의
개별 WorkStatus RPC와233.062s client RPC wall/1134.029s loop를 관측했다.
약94reads/s이며 각 요청은 worker와 같은 Catalog mutex를 사용한다. 이 wall은
CPU·mutex 점유 또는 worker 지연 비율이 아니다. WorkStatus는 read-only이고
COMMIT을 발급하지 않는다. 공개 Native UDS에는 별도 aggregate 완료 알림이
없으며 ReadView는 pin 트랜잭션을 포함한다. view sequence나 ReadyWork의 빈
목록을4096개 exact completion receipt 대신 쓰지 않는다. Observer cadence를
바꾸는 진단은 원 절대 deadline·전수 receipt·final coverage oracle를 유지하고
같은 workload/profile로 다시 비교해야 한다. 현재 증거만으로 polling이 전체
처리량 실패의 원인이라고 확정하거나 낮춘 빈도를 성능 수리로 세지 않는다.

Full Record 준비의 이전 연결·메모리 공백도 이 에픽에서 처리한다. Recorder API의
`DiagnosticCaptureLimitsV1.max_output_bytes`와 달리 실제 scale harness/daemon
CLI에는 별도 용량 전달 인자가 없었고, 자식 capture/profile 소비자도64MiB 고정
검사였다. 이전 offline timeline parser는
128MiB 제한 안에서도 raw bytes·split lines·decoded records/events·최종 span/
observation 결과를 중복 보유한다. Writer의 seen/counter_totals도 출력량에 따라
커진다.128 밀도 환산을 실제4K 실패로 단정하지 않되,상한만 올려서 준비 완료로
표시해서는 안 된다.

이 빈칸은 별도 티켓 없이 이 E07의 기존 full Record/RSS 조건으로 처리한다.
기존 JSONL·동일 검증기·complete/footer/sequence/digest/drop/unwritten/open-span
거절을 유지하고,run별 명시적 용량을 harness→daemon→자식 검증 경로에 일치시킨다.
같은 parser를 행 단위 검증·집계로 바꿔 전체 raw/record 복제를 제거하고,대형
span 결과와 writer 상태 메모리를 원 aggregate RSS/physical profile에서 계측한다.
새 포맷·중복 parser·무한 상한·잘린 capture의 성공 처리는 금지한다.

2026-10-24 구현에서 `--trace-max-output-bytes`를 추가해 용량을 전달한다.
Harness/daemon의 기본64MiB는 유지하며 명시값6144bytes–1GiB를 parent recorder,
첫 실행/reopen child, drain/artifact/Python profile 재검증에 전달한다. Python
capture의 명시 override는 Native Record에서만 허용하며 offline export도 명시
상한을 받는다. 기본 offline parser128MiB와 기본 child64MiB는 유지한다.
원4K/128 진단 helper는 `QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES` opt-in을 같은
daemon 인자로 전달한다. 제품 요청·worker·quality 예산과 별개인 관측 출력 예산이다.

동일 parser의 `parse_stream`이 bounded binary line과 증분 SHA-256으로 집계하고
`parse(bytes)`도 이 경로를 사용한다. 소비자는 커밋된 RawFile을 사용하며 export는
원본을 다시 검증한다. 전체 raw/decoded record 복제는 제거했지만 최종 span/metadata,
writer의 seen/counter_totals와 viewer 출력은 여전히 데이터량에 따라 커진다.
실제 v4 recorder를 v2 전용 child validator가 거절하던 공백도 지원2–4와 동일
header/footer 버전 검사로 보완했다. [Source별 영향 검증](VALIDATION.md#e07-bounded-record-stream-replay-20261025)은 Rust95/Python658 통과·기존2skip·실제128 COMPLETE/유실0이다. Schema 분기 선택은 기존 전체 parser 결과 SHA를 유지하며 동일19.7MB 재생 CPU39.938→8.139s를 확인했다. 이는 단일 offline 진단이며 전체 peak RSS·정식 Off/Record 수용은 NOT_RUN이다.

이전 원4K Record는 cbba에서 Source30s deadline으로 FAILED0/1이다. 캡처2.56MB COMPLETE/유실0은 실패 관측 증거다. SourceSubmit30.074s 안의 Catalog hold15.767s와 managed COMMIT5.756s는 포함 관계이며, staging을 포함한 전체 reserve/confirm40.8s를 이30s에 더하지 않는다. 후속 scratch mark의 bounded64-row SQL은 f0dca8d6에서 storage strict·map77/77·Kernel source-batch1/1을 통과했다. 별도 Source-only 심볼 표본은 VFS의 매-write root/lock 검사와 SQLite spill/checkpoint를 확인했지만 검사 캐시는 쓰기 전 거절 계약 때문에 제외했다.

기존 AfterWrite 콜백의 scope별 calls·요청bytes·오류·WAL/main/other 집계는062fe720에서 strict·Rust82/82·Python678 pass/기존2skip, 역사128 전체 결과 SHA 동일을 확인했다. 해당 source의 Source-only는 원30s에서 FAILED0/1·provider0이다. SourceSubmit 내부 SQLite xWrite111775회·요청241854244B·하위쓰기실패0을 staging/cleanup과 분리했다. other를 subjournal로 단정하지 않으며, 캐시 통계·실제 기록bytes·전체 CPU 측정으로 취급하지 않는다.

단일 provisional INSERT/Transaction 계약 수리는c39a67e9에서 strict·Rust106/106을 통과했다. 고정256회 provisional INSERT gold·늦은 실패 rollback·재오픈을 유지한다. 동일 source Source-only4096은 Published22.384s/1회 통과했고 원 full4K는 Source30.001s/1회 실패했다. 후자 preflight 외부 Rust30/load23.574에서 authorized 경합 진단으로 실행했으며 host 인과 기여도와 개선율은 미입증이다. 원키 Pending/provider0·3.14MB COMPLETE/유실0을 기록했고 Q64 이후는 NOT_RUN이다.

같은-Tx canonical fence counter UPDATE256→1은 ae051807에서 Catalog strict·74/74 통과했다. 단일 scope의 batch_limit1/max_in_flight1/provider concurrency1을 유지하며 max_dispatches2는 동시성 아닌 재시도 한도다. 원4K 단일 진단은 Source18.810s Published 뒤 materialization의 ResourceLimited에서 FAILED0/1이다. 당시 watchdog 약285s 잔여, snapshot2041/4096 completed·provider 중복1이며 Q64 이후는 NOT_RUN이다.616881711B/3102554events 캡처의 canonical COMPLETE/유실0을 확인했고 하위 거절/renewal RCA를 진행한다. 원인 메시지를 잃던 read/write admission과 최종 coverage-save 경계를 보완한7b56a113은 strict·budget/native-I/O23/23·Kernel8/8이 source drift0으로 VERIFIED다. 후속332c3ec8(코드6·기준파일1)는 설치 재시도에서만 원 authority의 retired-create 복구와 sync 후 경로/FD 재검증을 추가했다. strict·계층/모듈 경계·집중 회귀97/97은 VERIFIED이며 Kernel 접합8/8도 VERIFIED다. 원 fixture 기본 Off4K는 Source30.002s에서 FAILED0/1·Pending/provider0이다. BuildPrepare 취소이며 수치 예산 초과와 external refusal은 없다. 당시 host 외부 Rust2/load20.75여서 tracing/host 인과·정식 성능은 미입증이다. 동일 실패를 재시도하지 않는다. 후속255493d6의 parser terminal 중복 보관 제거는 Ruff·Python684 pass/기존2skip을 통과했고 실제588MiB 구·신 전체 결과 해시가 일치하고 parser peak RSS1549893632→1277411328B(약260MiB 감소)를 관측했다. 단일 직렬 쌍이며 제품 전체 RSS·runtime overhead 수용이 아니다. 실제4K 원인과의 결속은 아직 없다. guard·한도·발급 순서·실제 발급 수만큼의 최종 counter·원 COMMIT 경계를 유지한다. 정식 quiet/paired·전체 RSS/공간/fault/운영 수용은 별도로 남는다.

The existing c39 successful Source-only capture has a further canonical,
same-run phase bound: complete pre-postings inventory1.157187708s,
postings6.636377834s, contained Catalog hold4.960253460s/18 and
COMMIT2.313156499s/18. Inclusive regions overlap and cannot be summed.
All token scratch INSERTs are inside the inventory upper bound; this does not
measure their individual cost or current source/host timing. Initial Source's
old terms root is empty throughout postings, excluding map lookup_batch as a
read-saving repair for this fixture. No candidate is applied without a
matching measured cost. The255 source's R2 Source-only diagnostic proves
both stack collection and independent1s OS CPU/disk-I/O windows inside the
canonical lexical/postings stages. Source still FAILS at30.002143209s;
lower-VFS Source calls192662/requested421821248bytes/failures0 describe
reached writes, not allocation or completed device bytes. The postings1.0006s
window records daemon CPU749.349166ms (176.076625ms user/573.272541ms system)
and OS-accounted writes19996672bytes after Mach timebase correction;
stacks show final map install→Catalog confirm→pager spill/subjournal/WAL→VFS.
Whole Source Catalog hold16.246s and COMMIT6.838s overlap and are not additive.
R1 coupled rusage to11s symbol processing and could not prove stage containment;
its cost attribution is rejected. The corrected observer separates collection,
postprocessing and bounded rusage, rejecting negative deltas and late windows.
Both runs retain original product limits. This is diagnosis underforeignRust16/
load35.15625, not original4K acceptance, phase-wide CPU share, unchanged-machine-
code comparison or Off/Record qualification. No further identical run is assigned.

The earlier17.98438ms CPU value was a unit error: raw Mach ticks were labelled
as nanoseconds. This host's mach_timebase_info is125/3; an independent1s CPU
calibration agrees after conversion. Original R1/R2 raw values stay unchanged,
with corrected interpretation in VALIDATION; no product rerun is required.
This changes CPU interpretation, not trace/stack containment or disk-byte units.

The subsequent26d99ebf cut removes charge_object's discarded JSON Vec solely
for fee estimation, using the existing canonical serializer with a counting
sink. The exact fee and error/checkpoint order remain. Catalog strict and
hexagonal pass; all Catalog library tests252/252,0 skipped pass with source
drift0. Other metadata/intent/digest allocations are unchanged. This is a
scoped allocation repair, not a measured Source30s improvement or product closeout.

The next39e9a830 cut changes the measured lane8 protection write shape: each
existing64-row preparation packet uses cached single-row INSERT OR IGNORE inside
the same outer transaction. All packet key/digest preparation precedes SQL and
all packet protection precedes reclaim transitions. Existing256-object install
groups, precharged WorkUsage, immediate FKs, replay/rollback, CAS, barriers and
COMMIT stay in place. This targets SQLite's multi-write statement journal;
it does not change cache/spill policy or remove durability checks. Strict and
hexagonal plus Catalog252/252 pass at this cut. Runtime VFS is FAILED42/43:
the new discriminator leaves a pending WAL mutation after raw connection close;
its write comparison is not reached. Kernel and original4K are NOT_RUN.
A focused9d0a6eb0 diagnostic preserves the check, identifies only the pending
WAL path with pins0/close_unwitnessed=false and observes packet writes98/
262400bytes. That probe does not reach the scalar arm. The subsequent86d2f5bb
cut changes only the existing Runtime test to use the original Catalog owner
through reopened verification, with WAL/FULL/fullfsync/FK readback and checked
close. Strict/hexagonal, Runtime VFS43/43 and Kernel8/8 pass with2106-input
drift0. The unchanged Catalog252 proof remains bound to39e9a830. The fixed
64-target insertion scope observes packet[98calls,262400bytes] versus scalar
[0calls,0bytes], failures0; COMMIT is outside this scope. Exact reopened rows,
digests/FKs and terminal quiescence pass. This is mechanism evidence, not zero
transaction writes or a measured product speedup. One original default-bench
Off4K at86d2f5bb is FAILED0/1: SourceAdvance is Published in25.455199041s within
30s, but initial materialization reaches3119/4096 at the unchanged1200s watchdog.
Readiness lasts1144.521s; the later non-atomic snapshot is3140 Completed/
956 Pending/0 Blocked and provider3146 unique/0 duplicates. Daemon exits0 and
all2106 hashes remain fixed. Q64/reopen/delta/path/overlay are NOT_RUN.
Normal preflight reportsforeignRust2/load14.39502; this is authorized contended
diagnosis, not a timing comparison. No unchanged full4K retry is assigned.
Readiness123712 RPCs accumulate125.008s wall overlapping product execution;
that is not proof polling caused the roughly31% throughput shortfall.

The followinga5e4e974 cut removes only prepared_work's clock-row UPDATE by
using the existing advisory observed_catalog_now_ms. BEGIN/COMMIT, authenticated
floor/live claim/artifact checks and stage/final durable revalidation remain.
A new zero-SQL-write/restart/clock-corruption/expiry oracle and existing Kernel
Off/Record result/work parity pass in the serialized validation batch. Kernel's
existing prepared/stage API envelopes now use distinct SqliteWork lanes23/24
instead of conflated lane14; other Catalog envelopes keep14. Off gains no clock
or event. Strict/hexagonal, Catalog253/253 and Kernel8/8 pass with2106-input
drift0. One existing128-owner Record diagnostic also passes29.44s,128 unique
provider rows/no duplicates. Its22.6MB schema4 trace is COMPLETE/loss0, with
128 successful worker roots and balanced prepared/stage128/128 API envelopes.
All128 prepared calls perform0 native writes, confirming the lookup repair.
Current worker inclusive total23.869s contains Catalog hold16.310s and managed
COMMIT9.329s, which overlap; stage128 costs0.798s inclusive. This identifies
remaining transaction/durability cost without attributing the whole4K failure
or proving observer overhead/speedup. A fresh normal preflight now passes with
foreignRust0/load7.953125<=8. Root starts one original default-Off4K at this new
cut without a contention override. SourceAdvance is Published16.8077325s within
30s. All4096 exact completion receipts are observed in1143.915s; readiness uses
128541 RPCs/102.576s accumulated wall overlapping worker execution. The body is
FAILED0/1(1211.53s): the shared1200s deadline expires during initial Q64 seed48,
with an11ms remaining IPC read budget. Provider4096 unique/4096 total/0 duplicate
rows and48 unique/49 total queries are observed; daemon exits0 and source drift0.
Materialization completes but full initial quality, reopen/delta/path/overlay do
not complete. This clean start does not qualify paired overhead, matched speedup
or whole-run host isolation. No unchanged full4K retry is assigned. A bounded
4K-source/exact128-completion Record prefix is the next diagnosis, to distinguish
large-map transaction cost from the small128-source working set. It retains all
request/owned/product limits and normal shutdown; partial receipts are never
full4K acceptance.
The recoverable typed VFS detail gap now has an opt-in callback-local repair
and owning proof below. Historical final-error attribution is still UNKNOWN.

The prefix fixture at471ae8d6 changes only existing integration diagnosis source
relative toa5e4. Runtime owning strict is VERIFIED2.929s; one clean-start Record
body is VERIFIED1/1,90.13s(body)/106.874s(command),2106-input drift0. Source4096
is Published16.175444875s and exactly129 Completed receipts are observed in
50.143s(5312 status reads/4.128s accumulated RPC wall). Normal confirmed shutdown
observes200 provider rows/0 duplicates. Canonical parsing of45,677,115B reports
schema4 COMPLETE,232745 events/80941 spans, health/orphan/truncation/loss0. It
contains200 worker roots:199ok and1aborted during intentional shutdown; this is
not200 completed public receipts. Inclusive worker46.834s contains Catalog hold
32.996s and managed COMMIT20.630s; WAL sync15.307s contains parent sync3.614s.
These are overlapping intervals, not additive or matched speedup/CPU shares.
All200 prepared lookups still have0 native writes. This is an early4K working-set
diagnosis, not full4K quality/steady state/observer overhead qualification.
A proposed observational validate_owned_work clock change is rejected: removing
its durable high-water update could accept a subsequent time below an earlier
validation sample. The existing Immediate tx/clock/lease contract stays. The
narrow safe code lane reuses one authenticated operation read in the same tx;
a separate opt-in VFS callback observation will preserve typed refusal detail
without inferring the eventual SQL/worker failure's cause. Both code lanes now have focused validation below; they do not close throughput
acceptance.

The first86f12734 attempt
failed strict on12 new test-only lint findings; no test body ran, and those
findings were repaired before39e9a830.

At092f5788, the Catalog private validator reuses the status from its canonical
same-transaction operation read. BEGIN IMMEDIATE, durable clock persistence,
retained-body/terminal/live-claim validation and error precedence remain. Its
Unknown/Pending/Published operation SELECT count is exactly1; clock corruption,
regression, wrong body and lease expiry remain refusal oracles. Catalog254/254
is VERIFIED at806f4bec with identical Catalog production/tests in092f; no new
Catalog execution is invented at092f.

The Runtime repair samples only the dispatched callback's own typed budget
refusal and matching returned Capacity, up to8 process-wide events. It moves
the owned CoreError into a nested/thread-local frame and emits after Adapter
and Router locks release. Detail is truncated on a UTF-8 boundary at256 bytes;
direct Capacity lacks attributable CoreError and is explicitly unknown. Startup
registration reads the existing opt-in env once. Off adds one TLS read per
callback but no configuration read, clock, heap allocation or output; zero total
performance overhead is not claimed. A disabled inner frame masks an enabled
outer frame; unwind, concurrent8-slot admission and failed output retain the
original result. Successful product-fixture shutdown now relays these bounded
records before deleting daemon stderr. Logs are callback observations, not a
terminal SQL/worker cause or complete event inventory.

Current092f(2107 inputs, one added private Rust module) passes Catalog+Runtime
all-targets strict9.360s, hexagonal0.696s, VFS49/49(80.665s command), Kernel8/8
(26.586s), and diagnostic relay2/2(47.763s). Source drift0. VFS first failed48/49
at806f/c7 because its new oracle incorrectly required BeforeWrite: one held byte
funds the empty first DB, then the journal creation fails at BeforeOpen. The
original one-byte allowance and exact SQLite capacity classification stay;
the fixed expected callback/detail follow that independent producer contract.
The mode is contended focused functional validation, not timing acceptance.
No original full4K rerun is performed. The remaining throughput gate is the
last a5e4 original failure, not this focused batch. The prefix shows basic
workers pay4 managed object groups/8 reserve-confirm COMMITs plus7 other WAL
syncs; dependencies and parent-directory barriers remain. Removing any of them
without an atomic contract/authority proof is not an accepted repair.
[Commands, failed attempts and proof scope](VALIDATION.md#e07-bounded-record-stream-replay-20261025).

The existing resource ledger's Retained lifetime can fund a connection-owned
cache without a new authority. Its lease must survive through actual checked
close, including close failure, with a proven complete memory bound; advisory
SQLite cache_size is not such a proof. No unaccounted cache increase or spill/
guard/fsync suppression is selected as a repair.

The next80e62276 cut is focused VERIFIED: same-tx immutable key encoding,
per-call bounded namespace SQL2→1, and authenticated ANN delta scan-reference
reuse. Strict/hexagonal, Catalog255/255, Semantic234/234, Kernel8/8 and the public
filtered-exact daemon gold1/1 pass. Its single partial4K Record diagnosis also
passes1/1 with131 exact Completed/50.119s and45.9MB COMPLETE/loss0 capture.
The owned Source and worker one-second windows measure user/system CPU and
physical I/O with the actual125/3 Mach timebase. System CPU exceeds user CPU in
both windows, but the short contended samples do not establish a whole-stage
cause, matched speedup or observer overhead. Whole4K is NOT_RUN at this source;
the last original failure remains Q64 seed48 ata5e4. Keep all original limits,
barriers and quality gates. [Implementation, OS observations and limits](VALIDATION.md#e07-catalog-ann-cost-cut-20261025).

### Selected DATA and phase attribution closeout — 2026-10-25

The next source edits reuse an already-held artifact without an additional
cache allocation, and emit real phase ancestry for managed child work. The
Catalog owned-ready normal path removes a redundant same-transaction row read
through the original full validator, preserving corrupt cross-key refusal order.
The focused library/strict/lifecycle/actual-daemon batch now passes across the
explicit ba31b8a5→67741542 fixture-only correction. The same-source partial4K
capture passes1/1 with128 exact Completed/51.711s,
healthy COMPLETE/loss0 and emitted four-phase managed ancestry. The original4K
normal admission is BLOCKED(foreign Rust5/load29.469727>8), body NOT_RUN.
Throughput improvement and Off/Record overhead are not established.
Read-only audits find no further proved small COMMIT/barrier cut or owner-wise
full-corpus scan to remove. Reserve/physical durability/confirm, prepared-work
replay checkpoints and bounded child-first installation remain mandatory.

E04 owns one original4K queue after decisive source/diagnostic evidence. E07
consumes that result, then requires quiet Off/Record≥3 pairs, Large/XL, original
960s mixed, total RSS/allocated space and fault/churn acceptance. These are still
separate open gates. [Implementation and verification scope](VALIDATION.md#e07-selected-artifact-phase-parenting-20261025).


### Sync observer P2 and actual fault follow-up — 2026-10-25

A generic clock/sink reentry defect is repaired in the original VFS observer.
No callback or span drop runs under the TLS borrow; reinstallation checks owner,
exact capture session and the existing write-span identity. Independent nested/
pending/colliding-session regressions pass. At314666fa Runtime strict, VFS48,
short-write/last-pin2 and UDS fixed-cap1 are VERIFIED. The original actual ANN
seal ENOSPC1 also passes on a64MiB isolated HFS+ filesystem with cancellation,
old/current view preservation, same-operation recovery and confirmed detach.
That is one fault oracle, not the complete operating matrix. Linux FUSE fsync/
close bodies were BLOCKED at that checkpoint. The2026-10-26 local Linux replay
now executes original-FD faults and managed ANN fault/recovery cases;
[the current source-bound result](VALIDATION.md#local-fuse-fault-product-20261026)
supersedes the old environment blocker without closing the operating matrix.

Actual Cargo-built symbolized daemon/test attribution is VERIFIED against the
measured daemon UUID. The partial4K body passes1/1: Source17.847s Published,
128 exact Completed/55.578s, COMPLETE/health/loss0. Actual worker frames include
default automatic checkpoint, native SQLite sync and separate parent/object
sync; they do not quantify whole-run cost or justify dropping barriers.
This separate diagnostic profile cannot supply default timing/Off-Record
acceptance. Cargo restores default artifacts and their independent filtered
gold passes1/1. A later original4K normal-admitted Off run is FAILED0/1 at
initial materialization:4020/4096 exact receipts in1162.509s, no Q64,provider
4034unique/0duplicates. Source17.246s succeeds. The post-failure snapshot4031
completed/64pending/1claimed is non-atomic; no budget/quality gate is relaxed.
E04 remains the single run owner, and new source-backed RCA precedes a retry.
[Commands and current scope](VALIDATION.md#e07-sync-observer-reentry-20261025).

### Prepared-work indexed-write cut — 2026-10-25

After314's normal-admitted Off full4K failure,5510c600 changes only Catalog's
stage_work writer to payload/digest-only UPDATE. All existing claim, row bound,
clock, root protection and durable COMMIT checks remain. The pinned engine's VM
write cursors decrease5→1 and secondary IdxDelete4→0. Production behavior is
focused VERIFIED: repaired Catalog1 with unchanged256 reused at ec9b, strict,
Kernel73/73 plus existing skip1, and actual daemon filtered gold1/1.
The same5510 Record prefix passes1/1:Source19.850s,128 public completions54.269s,
COMPLETE/health/loss0. Its exact stage_work scopes record191 calls/4770 xWrite
callbacks/12477088 requested bytes. These are not physical bytes or paired
throughput; original4K and all operating gates remain open.
Following two admission blocks, whole4K Record RCA runs on5510 after normal
admission(foreign Rust0/load7.351562<8). Source4096 publishes18.232688792s, but
the body is FAILED0/1:3556/4096 public completions at1160.546s before Q64.
Provider3576unique/0duplicates/0queries. The closed trace is COMPLETE/healthy/loss0 after one canonical parse; no duplicate product run or unmeasured redesign is assigned. No matched Off/Record overhead or speedup claim is made.
Whole-capture native+post union499.929s and managed COMMIT482.249s overlap
by302.623s; parent sync125.133s is inside post. Stage_work totals30.486s.
Sourceb62c47c9 repairs Store→Catalog managed-span parenting, preserving original
request/Off/authority/barriers. Strict, owning1 and actual128 diagnosis1 pass;
all1094 actual managed calls expose one child COMMIT/lock/BEGIN/VFS scope.
The complete128 capture is22.96MB/health/loss0, distinct from full4K acceptance.
[Current implementation, actual costs and limits](VALIDATION.md#e07-stage-work-payload-only-20261025).

### Writer hot-path repair and bounded paired diagnosis — 2026-10-25

Source31aec3e7 reuses the event encoding Vec and replaces per-event dynamic
stage/counter name formatting with the same79/71 static wire names. Strict,
recorder22/22(existing20+fixed gold2), actual128 diagnosis1 and three serial
Off/Record128 pairs6/6 pass. The first functional diagnosis is contended; the
six paired starts are normal-admitted with foreign Rust0. Record3 captures are
COMPLETE/health/loss0. Median paired body/readiness increases are1.142%/1.187%;
driver-tree CPU increases4.177%. These are bounded observed ratios with visible
variance, not Large's formal per-phase gate or aggregate resources. Original
Large overhead remains NOT_RUN. The current-source original4K Off is now FAILED0/1 at incremental Q64 seed43
under the shared1200s deadline, after all4096 public completions, initialQ64,
cold-reopenQ64 and one-owner delta completion pass. No full4K acceptance follows.
SEARCH's subsequent52296745 query DATA-reuse batch is focused VERIFIED132/132
plus strict, preserving original charges/validation/pin lifetime. Its first original4K admission is BLOCKED at foreign Rust2. After those processes terminate, normal admission(foreign0/load5.656738)
starts one current-source body, now FAILED0/1(1210.51s). All4096 materializations
complete1163.968s; selected source inventory consumes the remaining shared1200s
before Q64. Source15.487s succeeds, corpus provider duplicates0, daemon exit0,
source/binary drift0. Query improvement is not measured by this failure. E07
consumes SEARCH's same deferred owner-install code batch without a second
queue. Core/Storage/Kernel scope, stage-aware original readers, early write
flush and explicit token completion are written; sourcea646 strict and
focused139/139 including actual UDS192 pass. Original4K normal admission is
initially BLOCKED by foreign Rust2/load13.039>8. Fresh normal admission at
2026-10-25T05:57:27.500813Z(foreign0/load6.456055) starts one original body,
now FAILED0/1(body984.14s) at unsaved-overlay Native Keyword lowering. Source
14.672099875s and4096ready881.921s, initial/cold/incrementalQ64 plus path pass.
This is not a watchdog failure or a paired speedup. The same SEARCH queue
implements canonical Keyword token-sequence execution/admission; current4322046b
strict and focused157/157, including actual UDS overlay/short-write/last-pin/
fixed-cap pressure/filtered gold/isolated-volume ENOSPC, pass. The one supported
contention-override original4K is now FAILED0/1(body1209.84s) at shared1200s in
the overlay sweep: Source15.334886083s,4096ready1037.647s, initial/cold/deltaQ64
and selected path checks pass. The old Keyword InvalidInput no longer appears.
After the previous foreign build ends, fresh normal admission is BLOCKED/body0
by new foreign4/load9.756348>8. No Root Rust/SUT remains live; no fixture or
product budget is changed. No unchanged contended full run or installation/publication protocol fusion is
assigned. Aggregate RSS and formal operating/
full-profile observer gates remain open.
[Current query code and proof boundaries](VALIDATION.md#e04-query-scoped-data-20261025).
[Current code, paired results and limits](VALIDATION.md#e07-trace-writer-allocation-20261025).

The final static overlay audit confirms that a two-term phrase currently
validates the union of both posting lists. A common4096/rare1 case therefore
advances about4097 postings, but the exact late-corruption and declared-DF
oracles require EOF validation. No authenticated skip/rank certificate exists;
a rare-driven point-lookup path that ignores the tail would weaken the contract.
This source complexity is not a measured critical-path contribution. The
failure's provider snapshot is cumulative since reopen: relative to selected
path/delta, overlay adds29 row and29 query embeddings, not30 overlay rows or
two overlay row kinds. Request-local Fast results are reused; expected_corpus
is identity checking, not cross-request prepared-vector authority. No proven
narrow missed reuse or safe I/O cut is identified under the current contracts.
Keep attribution UNKNOWN until a valid source/host-bound phase measurement;
do not add a second engine/cache or call either observation the measured cause.

### Current query phase observability — 2026-10-25

The existing recorder lacked actual Snapshot/Fast/Deep, lexical and query-provider
envelopes. Source7a0828f7 adds six closed-vocabulary stages and five aggregate
posting/phrase counters on the original paths, without per-posting spans. Kernel
rebinds diagnostic context on the same WorkBudget and restores it on every exit.
Off uses the original call path without a query clock or budget clone. Provider
spans measure invocation envelopes, not pure external I/O; descriptor/checkpoint/
validation retain their original order. A proved P2 in the new diagnostic code,
provider cancellation recorded as Error, is repaired with typed/cancel-state
classification and Off/Record result/WorkUsage regression. Current strict/Rust274 and Python702/2existing skips are VERIFIED. The single
original4K Record fails at Source30s before query; the initial capture is complete/
healthy/loss0. Formal Large overhead stays NOT_RUN.

Snapshot/Fast/Deep are siblings of Backend under the request root. Lexical is
inside Fast and query/overlay provider envelopes inside Deep. Never add these
nested/overlapping durations. First/middle/last ordinary materialization,
managed COMMIT/sync and periodic seal envelopes can distinguish the next source
cut. Static duplicate immutable-base CAS prechecks and map DATA decoding are
candidates only: existing traces cannot assign their physical read contribution,
and removing prechecks may change error priority. Keep them unpatched until
measurement or an independent preserving contract justifies the change.

The stopped normal432 body has no final verdict. The next original4K Record uses
the same fixture and shared1200s, two original epochs and each1GiB cap. Each epoch
must close before one canonical parse confirms complete/loss0/health0/parent
integrity. A failed body can supply a valid diagnostic capture, never full4K
acceptance or formal Off/Record overhead. Existing Linux resource/source binding,
Large/XL/mixed and operating gates retain their original requirements.
[Current commands and results](VALIDATION.md#e07-query-phase-observability-20261025).

The small actual product daemon now independently verifies both durable and
unsaved-overlay query ancestry, fixed owner/path/revision/cosine and Off/Record
results/scores after reopen. Its bounded trace checks both IPC request roots,
all six new stages and complete footer/loss0; final8387 strict and1/1 pass.
The original product source is unchanged from7a. The same source's normal-only
original4K admission is BLOCKED/body0, so query critical-path proportions and
original Large observer overhead stay unqualified. A suspected default Kernel
context suppression is falsified by the existing context.map semantics and
independent inherited-context oracles; no redundant context API/patch is added.
[Final evidence and remaining gates](VALIDATION.md#e07-actual-uds-query-trace-20261025).
The third consecutive goal-turn admission remains BLOCKED at09:30Z,foreign
Rust3/load32.417480>8,body NOT_RUN. This records local admission blocked by
foreign work at that checkpoint, not a requirement to acquire an external host.
Resume the same original4K queue after normal admission; local code/function
work continues independently. Formal Linux/source inputs and every original
operating/observer gate remain.


### Independent code work while performance admission is blocked — 2026-10-25

The prior full-goal blocked wording was too broad for source work. The latest
user direction resumes independent code remediation before batched runtime.
Source1624aff0 adds bounded query Err observation under the existing
QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1 flag. It captures only request/connection
IDs, successful Fast/Deep/terminal delivery flags, fixed error class and first
local WorkBudgetDenial. The process-local prefix is at most8 records; dispatcher
Ok results do not read the flag, format or read a diagnostic clock. An Err after
an already delivered terminal is still observed. Full WorkBudget
Debug and provider/query content are excluded. None means no observed local
budget denial, not proof of a scheduler/provider cause. Existing replies,
terminal-emitted/no-second-frame behavior and provider custody stay unchanged.

Root connects the existing ANN successful-run relay and actual daemon valid-
syntax/unsupported-selector refusal witness; independent footer/phase/result
checks remain. The actual run exposed an earlier successful query's postterminal
Cancelled record, which the first witness confused with its explicit refusal.
Only terminal-emitted Cancelled is capped at2 of the total8 records so it cannot
consume every later error slot; a deterministic100-repeat regression preserves
6 ordinary slots. Preterminal cancellation and postterminal Storage keep the
original sampling path. The product witness distinguishes these cases and
retains typed refusal and all original trace/result oracles. Final eed03da5
Runtime all-target strict,diagnostic10/10,actual effort UDS42/42 and product3/3
are VERIFIED,source drift0. The3 include the actual daemon Off/Record result/
score/phase/ancestry/footer/loss/privacy witness and two existing relay tests.
NativeText143/143 retains its unchanged earlier1624 lexical scope; these are
not a single-source198-test result. No formal timing SLO is promoted.
The Linux audit confirms kernel memory.peak is already wired to execution.json;
it is cgroup-accounted memory, not a proved aggregate-RSS upper bound. Do not
create a duplicate sampler or relabel sampling as continuous peak proof.
[Code-first evidence](VALIDATION.md#e07-code-first-remaining-20261025).

## Resource acceptance instrumentation boundary — 2026-10-26

User direction is to maximize local completion without requiring an external
host for code or functional checks. The existing Docker Desktop Linux aarch64
kernel supports genuine cgroup v2 delegation inside a new owned private cgroup
namespace. Current same-source Python impact checks are Linux817passed/5skipped
and Darwin807passed/15skipped after the observation-deadline correction below.
These are functional checks, not the original
quiet2CPU/6GiB/local-SSD performance profile. No foreign container is modified.
Landlock enforcement is unavailable on this local kernel (ENOSYS even without
the Docker seccomp filter); its three native bodies stay BLOCKED/NOT_RUN.

The existing qualified Linux owner reads bounded smaps_rollup Rss per process.
Actual local execution exposed normal exit_mm/ESRCH before state becomes Z;
the owner now corroborates PID/start/cgroup and kernel exit state, preserving
normal cleanup and refusal of unknown live data. A leader can exit while worker
threads keep its mm resident. The same owner enumerates cgroup.threads and reads
one live representative with before/after TID start,Tgid,cgroup and original
TGID identity checks. RSS and CPU evidence remain keyed by TGID, summed once per
process. A successful RSS read is retained through a subsequent Z transition
after identity/membership revalidation. One metadata validator has no state-drop
boolean. No second collector,process issuer or approximate-RSS fallback exists.

Actual opt-in tests cover root+child normal exit, timeout while root waits,
root-first exit with a surviving child,and pthread_exit followed by a worker
allocating/touching64MiB only after the leader is Z. They require an explicit
delegated cgroup,check positive RSS,subgroup removal and parent preservation.
Permissions,malformed data,PID/TID reuse,Tgid changes and migration still refuse.
Point samples are not atomic family snapshots or continuous peaks; shared pages
may repeat and short peaks can be missed. Diagnostic process-group stat RSS and
the inner Native sampler retain their declared diagnostic methods. Smaps observer
cost itself still needs performance measurement. See the
[Linux RSS contract](https://man7.org/linux/man-pages/man5/proc_pid_statm.5.html)
and [thread proc interface](https://man7.org/linux/man-pages/man5/proc_pid_task.5.html).

Both existing Linux owner loops now check the time after sample and completion
observation before accepting an empty/dead family. A final slow sample or
completion read at/after the original deadline cannot return timed_out=false.
The checked observation timestamp is also the elapsed_ms endpoint. Qualified
empty+late avoids an unnecessary cgroup.kill while retaining escape cleanup,
wait-empty, final accounting, root wait and subgroup removal. Original request,
absolute deadline and separate cleanup grace remain unchanged. Twelve fixed
clock cases cover both owners, a tighter absolute deadline, before/exact/after
expiry and a slow final sample. No real sleep or fake-PID signalling is used.

ProcessResult.rss_method binds the existing producer to both resource consumers.
Persisted generic resource schema3 requires the backend-matching method and
rejects methodless schema2. The Native capture product validator and timeline
retain the outer observation and refuse mixed sampled aggregate RSS above2GiB.
Ordinary non-mixed profiles do not inherit that cap. Malformed backend,escaped
identities and storage kinds produce typed failure; false/0.0 cannot represent
integer exit0. Capture fixtures now produce final file dev/inode after copying
across filesystems; consumer raw-epoch checks remain strict. The diagnostic
cleanup fixture waits for its child before the original0.3s timeout measurement.

The producer still emits continuous_rss_peak=false and allocated_disk_peak=null.
Kernel cgroup memory.peak is charged memory, not summed RSS; RUSAGE_CHILDREN
ru_maxrss is the largest child, not the process-tree sum. Neither substitutes
for the lifetime DoD. Native inner observation excludes startup and final drain;
the outer cgroup owner spans harness/descendants through cleanup but remains
sampled. Shape validation does not authenticate arbitrary JSON as host proof.

Native child TMPDIR stays private below the capture root and the locked socket
shape is checked before output/build. This is placement, not write confinement
or all-root quota. Existing legacy/Native logical byte/file maintenance reserves
are reused and are distinct from physical resource measurements.

Remaining resource acceptance under existing E07/PHYSICAL ownership:

The original contract requires full-lifetime coverage and the fixed resource
ceilings. It permits existing admission/OS high-water evidence; it does not
require an exact instantaneous time series, a new sampler or a per-FD ledger.
A sound conservative upper bound with complete custody may suffice. These are
unresolved authority/evidence inputs, not three demonstrated missing code
bodies. Select and verify the authority before assigning implementation work;
absence of final proof alone is not an observed product defect.

1. Whole-lifetime summed RSS: a compatible authoritative method and external
   migration prevention are required. Faster sampling,VmHWM,RUSAGE_CHILDREN or
   relabelled cgroup memory.peak are insufficient. pids.max times inherited
   RLIMIT_AS is a conservative enforcement alternative only with exclusive
   controls and privilege closure; it counts threads and can overrestrict this
   workload,so it is not selected or silently imposed as the product profile.
   pids.peak times wait4.ru_maxrss is also rejected: this kernel's pids watermark
   can miss a race, auto-reaped children can omit wait4 evidence, and ru_maxrss
   uses approximate kernel RSS counters. Multiplication does not close those
   missing upper-bound premises.
2. All writable roots: bind scratch,state,temporary,logs/traces,WAL,retired/pinned
   objects and relevant open FDs to actual allocated-block authority/enforcement.
   A bounded dedicated filesystem/quota plus write confinement is a candidate;
   it must cover all roots and deleted-open files. A4GiB cap on the whole fixture
   is stricter than scratch-only and cannot silently replace that contract.
3. Once a feasible authority exists, check whether its output needs a concrete
   connection to the existing capture→profile verdict with source,binaries,
   config and first/reopen epochs. Implement only a demonstrated missing seam.
   Shape-only validators or caller proof booleans cannot supply that authority.
   Current false/null markers remain accurate; incomplete proof stays
   non-qualified.

Local code/function checks do not wait for an external host. The same-source
Linux817/5 and Darwin807/15 results above close the repaired seams;
source-identical repetitions add no proof. Continue independent code/fault
checks only when a specific uncovered body or defect justifies them. The next
original4K performance body uses the existing normal admission, whose latest
observation is foreign Rust3/load25.371094>8: BLOCKED,body NOT_RUN. No unchanged
contended4K rerun is assigned. There is no further confirmed code hunk in the
audited E04/diagnostic seams at this checkpoint. This is not a whole-product
or whole-repository completion claim.

Resource concerns1/2 remain distinct; freeze a feasible authority before any
parallel source changes and serialize shared integration. No speculative second
sampler, allocator or mount orchestrator is assigned.
E01/E02/E05 supplier work and E06 cutover retain their owners. Original Large/XL,
paired observer,mixed/fault/churn and release DoD are unchanged.
[Current source,failures and final local evidence](VALIDATION.md#local-linux-rss-lifecycle-20261026).
[Authority audit and local continuation boundary](VALIDATION.md#local-resource-authority-audit-20261026).
[Observation-deadline repair](VALIDATION.md#local-observation-deadline-20261026).

## Local managed-publication fault completion — 2026-10-26

Local Linux FUSE is usable on this Mac through a new owned container; no external
host is required for these functional bodies. The pinned passthrough backend
uses cache=auto for SQLite WAL/mmap, preserves exact original-FD fsync/flush EIO,
and forwards atomic backing renameat2/NOREPLACE without overwrite fallback.
Only the owned external backend and test environment change. FD fixtures use
TMPDIR=mount; managed ANN fixtures explicitly use tempdir_in(mount) with normal
/tmp. All cases are serialized under existing resource admission.

At source6121811e/ELFd5a55299 all five selected bodies finish: original-FD2 and
managed ANN fsync,checked-close,actual ENOSPC. The combined ANN command reaches
its600s runner timeout after close/fsync finish; that batch remains FAILED.
Only unfinished ENOSPC is rerun with the same source/ELF/input and600s guard;
its full summary is VERIFIED1/1 in88.65s. No green aggregate command is invented.
The close oracle authenticates the held A pin's original session/fence/expiry/
view and all residual child A/B custody. Actual post-exit recovery retires1 dead
session and2pins to0; original Pending131,same-key replay and fixed source/
owner/score/coverage/reopen gold pass. An exact aggregate pin_count==1 expectation
would incorrectly reject the failed materialization's additional retained pin.

These local fault bodies do not close the entire operating matrix or qualify
performance/resource peaks. The final normal original4K preflight is BLOCKED
at2026-10-26T02:29:31Z,foreign Rust3/load25.371094>8; body NOT_RUN. Preserve all
original request/worker/quality limits. Full-lifetime resource authority and
formal paired/Large/XL/mixed/churn/release remain separate. Existing source/
static evidence does not establish another specific production code defect in
the audited E04/observation seam.
[Exact source,commands,failed cohorts and final functional results](VALIDATION.md#local-fuse-fault-product-20261026).


## User-requested original4K RCA — 2026-10-26

The user requests an actual run after the normal-admission block. The existing
supported contended diagnostic mode runs source38f70c96 after canonical current
bench/all-feature compilation. Original request/work/quality limits remain.
Actual body is FAILED0/1,1073.76s: Source4096 Published25.423311s,then owned
materialization Blocked(ResourceLimited),2097/4096 observed completion bindings.
The two bounded refusal records show the shared original10s grant deadline,not
1Mreads/512MiB exhaustion. Initial trace is VERIFIED COMPLETE,578405827B,
2912311events,loss/open/leases0; query/reopen/later phases remain NOT_RUN.

Worker active intervals sum934.832s; inside those workers Catalog hold union
sums704.649s and managed-or-SQLite-sync union673.729s. These overlap and must not
be added. Semantic build/seal spans separately total1.510s/2.266s. This points
to durable metadata work as the measured dominant region,without proving CPU,
physical I/O,observer overhead or host causality. Two error workers spend
10.317s/10.922s,including long COMMIT/sync boundaries. Current bounded stderr
operation keys cannot exactly join those qtrace worker roots or public owner718.

The original investigation now binds the existing worker DiagnosticContext/trace ref
and original operation/grant in the same bounded refusal path. Final affected
regressions21/21 and Runtime library strict Clippy are VERIFIED; all-target
strict retains separate Core/risk-suite failures. The connection can identify
the exact worker subtree when a selected failure is emitted; a zero-refusal
cohort cannot supply a nonzero product join or justify a guessed performance patch.
[Connection implementation and narrow verification](VALIDATION.md#e07-owned-grant-trace-correlation-20261026). Preserve fsync/CAS/
reservation/pin authorities and all10s/30s/work/quality limits. No unchanged
contended rerun,second observer,duplicate engine or speculative collector is
assigned. Resource-authority selection and formal paired/Large/XL/mixed/churn/
release remain original gates.
[Current body,capture,measured costs and inference limits](VALIDATION.md#e07-user-requested-original4k-20261026).

The subsequent source1182969a Record run fails the whole watchdog with3302/4096
exact completion bindings observed; Source18.157681s passes. No owned refusal
is emitted, so its product failure-join coverage is NOT_APPLICABLE, while the
actual worker child regression21/21 proves the new connection. Its complete
847MB capture shows worker-active1091.409s,Cataloghold818.392s and worker
lockwait1.853s. This is throughput failure, not another observed10s grant
denial. A same-source/binary Off control retains every original product/quality
limit and is diagnostic only on the contended host. Static pacing inspection
does not justify changing500ms/64-owner polling or weakening the exact bitmap.
That Off control also expires at the shared watchdog with2953/4096 completion
bindings observed; Source27.771361s passes. Both modes fail without an observed
owned-work refusal; causal overhead and formal <=5% acceptance remain unproved.
[Current Record result and Off control](VALIDATION.md#e07-owned-grant-trace-correlation-20261026).

## DoD

- [ ] E01–06의 실제 제품 성공 뒤 Large/XL ACK/current·old search/reopen·같은 operation 복구를 독립 gold로 수용.
- [ ] warm lexical 예정 발행→decoded top20 p95≤100ms/p99≤500ms, one-file staging→ACK→검색 freshness p95≤1s/p99≤5s를 원 운영 profile에서 수용.
- [ ] aggregate 모든 process RSS≤2GiB·allocated scratch≤4GiB·physical≤64GiB, 실제 high-water·orphan/retired/WAL/FD/pin과 누락 없는 비용 회계. sampled maximum은 continuous peak가 아님.
- [ ] pin 해제 후60초 안 reclaimable debt0·shutdown grace5초·지속 churn/debt/drain. 보호 중 객체는 공간 회계에 포함.
- [ ] ENOSPC/short-write/fsync/checked-close·cancel/panic/process-death·long pin·space pressure에서 accepted publication 유실0, old-view 정확성·bounded cleanup.
- [ ] initial build·fixed delta/no-op·N/H/M·fanout/history·공통 term·1/100/1000-file batch·branch/watcher/model/policy 변경을 독립 축으로 계수. 숨은 전수 scan과 maintenance debt를 포함.
- [ ] [RFC 12.3/12.5절](../../rfc/oct-10-native-incremental-vnext.md#123-latency-stability-and-qualification-envelope)의 원 native/IDE profile을 실행 전에 고정하고 latency/quality/freshness·served rate·전체 비용을 함께 E08에 인계.

기존 사용자 유예(continuous RSS/전체 roots allocated peak·delegated Linux 등)는 실행 순서 유예이며 최종 DoD 면제가 아니다. 입력 부재는 BLOCKED/미실행은 NOT_RUN으로 남긴다. 기존 [세부 운영 사양](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-09-cost-observability.md)의 cap/clock/capture/오류/settlement 계약을 보존하며 결과를 본 뒤 같은 실행의 합격선을 바꾸지 않는다.
