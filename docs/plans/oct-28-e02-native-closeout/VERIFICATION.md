# Packet verification — owning proof and immutable cohort

<!-- b-closeout-20261103 -->
**2026-11-03 현재:** B source·cold exact suite/Just consumer 및 L3/common/Chunk 인계는 source에 반영됐다. 현재 syntax7/7·영향policy7/7 및 registry/consumer/negative 검사는 통과했다. QE11 374/374·scalar4실행은 historical Source94ca 결과이며 current Source는 별도 검증한다. 현재 단일 executor는 D, accepted45와 prepared54newaccepted0은 분리한다. P60 source gate와 P07 current Native/SDK·Linux/process 수용은 남는다. [단일 현재 checkpoint](../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정/예산/DoD는 유지한다.

<!-- e02-b-current-20261030 -->
**B 현재 상태(2026-10-30):** P03 실제 Trace/Evidence/forensic→closed memo→admitted Store와 P08 Native reader/checked binder는 source에 연결됐다. 준비10파일과 provider/census도 같은 checkout에 반영됐다. 기존 P03/P08 미착수·미적용·producer 부재 문구는 과거 지시다. 새 predecessor4 scalar tests와 genuine Runtime/SDK/cross-generation/cold는 현재 수용되지 않았고 capability=false다. [유일한 현재 결과·담당별 잔여](../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정·D0–D3·예산/DoD는 유지한다.

이 문서는 실행 계약이다. 10-28 최초 계획 작성 시 Rust/build/SDK/remote 실행은 NOT_RUN이었다. 10-29 실제 배정과 후속 접합은 [실행 배정](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/LANE-ASSIGNMENTS.md)을 따른다. 실행 결과의 SSOT는 기존 VALIDATION이며 이 문서가 현재 green을 선언하지 않는다. [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md)의 D0–D3를 구분하며 실패·미실행을 VERIFIED로 바꾸지 않는다.

## 비용 순서

1. 실제 caller·현재 claim/dirty hunk·input/target/mode/예산/oracle를 고정한다. 기존 green과 입력 차이를 대조해 동일 검증 재발급을 없앤다.
2. source/static/boundary와 가장 작은 소유자 행동 검증부터 한다. compile이 의심되는 구조 합류에는 owning integration check를 먼저 쓸 수 있다.
3. 같은 registered QBC lane과 coherent source에서 관련 코드 묶음을 합류한다. check와 test는 같은 immutable execution_root/정식 resolved snapshot marker/feature class를 사용한다.
4. owner body 통과 뒤 실제 public Runtime/SDK product gate를 실행한다. external installed/provider 입력은 미리 병렬 준비한다.
5. 정상 결과 뒤 같은 source를 단순 반복하지 않는다. source/입력/feature/config가 달라졌으면 영향 rail만 재검증한다. unrelated dirty가 있다고 소유자 green을 폐기하지 않으며 green을 최신 전체 source로 승격하지도 않는다.

## Canonical front door

- S Rust: `/Users/songmin/Documents/code-new/semantica-codegraph-v2/scripts/quanta-build-cli`의 `owner resolve`→같은 typed flags의 `owner run`. root/product/recipe는 해당 QBC authority를 따른다.
- 기존 logical lane: `worktree-heavy-semantica-codegraph-v2-d0b8b5a985ea`; canonical snapshot child: `worktree-heavy-semantica-codegraph-v2-d0b8b5a985ea-snapshot-artifact`. 실제 registration/marker를 현재 owning tool에서 얻고 추정하지 않는다.
- snapshot/seed/캐시는 기존 QBC owner가 준비한다. 퇴역한 R34 root를 복원한 척하거나 파일을 복사해 새 snapshot을 만들지 않는다. immutable root에서 standalone preflight/cache 생성도 하지 않는다.
- Q Rust가 실제 영향받으면 기존 `/Users/songmin/Documents/code-new/quanta-index/scripts/cargow`/Justfile을 사용한다. 이번 계획 작성만으로 Q 전체 Rust를 실행할 이유는 없다.
- per-run disposable output은 checkout 밖에 둔다. source/입력·선택·실행·실패·0body·TIMEOUT을 보존하며 새 repo 영수증 SSOT를 만들지 않는다.

## 기존 Runtime replay의 정확한 owning 요청

현재 registry에서 다음 요청을 먼저 resolve한다. 이것은 실행되었다는 뜻이 아니다. 등록 class가 바뀌었다면 existing owner에서 current equivalent를 확인하고 registry를 우회하지 않는다.

```sh
scripts/quanta-build-cli owner resolve \
  --package quanta-runtime \
  --execution-kind test --target-kind integration \
  --target-name source_bound_replay_identity_owner_contract_test \
  --selector-mode whole-target \
  --compile-policy feature-isolation:quanta-runtime.no-default.73ab9f2c701b \
  --max-test-threads 1
```

owning `run`에는 같은 요청과 정식 `--execution-root`, snapshot child `--lane`, 기존 admission의 jobs/seed 정책을 전달한다. R34 class의 실제 features는 `original-snapshot-v3,search-plane-sdk-ingress-product-proof`였다. 새 테스트 지원을 기본 feature에 올리거나 arbitrary cfg로 우회하지 않는다.

## 패킷별 수용 rail

작성 중 위 `owner resolve`를 실행해 exit0을 확인했다. 현재 registry는 `--no-default-features --features original-snapshot-v3,search-plane-sdk-ingress-product-proof`와 동일 integration target을 반환했다. 이는 요청 해석만 VERIFIED이며 compiler/link/test body는 실행하지 않았다.

| 패킷 | 최소 owning 회귀/독립 oracle | 최종 필요한 실제 소비 |
| --- | --- | --- |
| P01 | 같은 Acked 양성4MiB + malformed/foreign/short/late Source + byte/digest reference + last drop | replay 전체 target; R34 필수76개 유지 |
| P02 | locator seek/page/cursor/retention/live receipt·same epoch concurrent reader | public Native SnapshotGC와 마지막 release 뒤 진전 |
| P03 | 기존 Trace frame/encoder·nonempty trace·required forensic artifact·거절/partial/refund | Immediate/Deferred/no-store 정책별 Runtime→ArtifactStore→memo/read |
| P04 | 실제 queue/command/ledger/worker/drop·3회/4회 refill·shutdown/unknown write | public Deferred+shutdown drain과 final owner 환급 |
| P05 | genuine E01 complete target gold·unchanged carry/dirty/delete·foreign target 음성 | 실제 supplier→installer→successor slot |
| P06 | 기존 member/final/M/restore oracle·source gate signed/unsigned negative | same operation SDK→V9/CAS/ACK→checkpoint, 5 durable cut |
| P07 | configured adapter+local HTTP wire/model/vector gold·bound/cancel/provider 오류 | 실제 selected remote provider + SDK Native artifact |
| P08 | 기존 native binder/reuse/empty-root oracle·currentness 모든 시점 | 각 필수 query/mode의 public MemoHit/SharedInFlight/cold reuse |
| P09 | 영향 QE owner whole lib와 Runtime proof, guard/boundary/locked 정합 | installed pair/provenance, CP3, 기존 E08 gate 소비 |

새 테스트의 selector/패키지 feature class는 실제 owning resolver/registry에서 확정한다. 경로로부터 임의의 테스트 module 이름이나 compile class 해시를 발명하지 않는다. QE의 이전334/334는 당시 source의 owner 결과이며 새 코드의 고정 body 수가 아니다. Python guard의 이전16/16도 마찬가지다.

## P07의 별도 owning 요청

10-30에도 다음 요청의 `owner resolve`가 exit0으로 해석됐고 request ID는 `0ebcece46ed34f33c9b51d8459630862`다. feature closure는 `--no-default-features --features search-plane-sdk-ingress-proof`이며 `embeddings-http`와 Voyage test module을 포함한다. QE lib·L1 QBC·별도 Runtime replay integration target은 이 Runtime 모듈의 검증을 대신하지 않는다.

현재 source에는 전송 전 명시적 거절, request/response codec·exterior partial/full-error DATA, child 4개의 작성 회귀 28개가 있다. 최종 정적 감사에서 생산 enum5와 테스트 패턴1을 lexical contract로 수리했다. format·diff·boundary17·changed-source·적용 가능한 honesty lints는 VERIFIED다. Semgrep targets0/shape scoped0은 NOT_APPLICABLE이다. **해당 수정 source의 compiler/본문/local admitted HTTP/remote SDK/최종 환급은 C에서 NOT_RUN**이며 원 owning 결과 없이 승격하지 않는다. [최신 source/검증 범위](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/VALIDATION.md#e02-provider-final-audit-20261030-c).

```sh
scripts/quanta-build-cli owner resolve \
  --package quanta-runtime --execution-kind test --target-kind lib \
  --selector-mode module --selector retrieval::voyage_adapter::tests \
  --compile-policy feature-isolation:quanta-runtime.no-default.9c3270892708 \
  --json
```

Root는 같은 요청의 `owner run`을 기존 검증 큐에서 직렬 실행한다. module selector는 실행 범위이며 전체 Runtime `lib(test)` compile/link 비용을 없애지 않는다. 이 요청은 codec·response·pre-transport refusal 회귀를 포함하지만, 실제 Output A의 HTTP allocation custody·local HTTP 양성·configured remote/SDK 제품 수용을 대신하지 않는다. 요청 해석만 VERIFIED이고 실제 compiler/body는 실행 결과가 나오기 전까지 NOT_RUN이다.

## CP3 scenario matrix

- public normal write/read: Immediate, Deferred, no-store의 합의된 의미, nonempty exact bindings, required trace/evidence/forensic.
- reuse: cold persisted load, SharedInFlight, I/O 전후/await 후/decoder 후 currentness, stale/foreign key/schema/root/function/host, complete-empty와 rootless miss.
- visibility/reclaim: Memo+Trace+binding same epoch, concurrent readers, pinned old view, receipt retention, page bound, last release progress.
- DELTA: actual unchanged/dirty/delete/rename/recreate/no-op complete successor, exact request/target, member/V5/Source finish/prior V9/terminal/M/ACK.
- restart: 기존 first-FULL3/Facts2의5 durable cut, unknown publication, ACK loss, repeated same-key retry, CAS conflict, cancellation/unwind/panic. 새 operation/attempt/target 생성이나 duplicate CAS로 복구하지 않는다.
- final custody: late highest Source refusal가 prior local error보다 우선, secondary cause/partial 보유, payload/decoder/retired backing/shared weak/header 뒤 funding 해제, 최종0.

## 실패 분류와 외부 입력

- FAILED: 실제 실행이 실패. 예를 들어 P01 R34 baseline의 당시 결과가 여기에 해당한다. 이를 다른 source의 현재 실패로 재사용하지 않는다.
- BLOCKED: 필요한 실제 typed supplier/installed worker/credential/provenance/정합 lock 입력이 없거나 유효하지 않음. 무관한 독립 코드까지 전역 정지시키지 않는다.
- NOT_RUN: 아직 실행하지 않음. 작성한 회귀·resolve만 한 요청·컴파일-only를 실제 behavior pass로 바꾸지 않는다.
- VERIFIED: 표시한 정확한 source/inputs/target/features/본문 범위만 통과.
- NOT_APPLICABLE: 원 계약이 허용하는 조건부 사례에만 사유와 함께 적용. 원 필수 GC/provider/DELTA/제품 gate에 입력이 없다는 이유로 N/A를 붙이지 않는다.

전체 boundary/--locked/CI는 착수 시 current로 재확인한다. 과거19 policy 실패는19개의 현재 코드 누락이 아니며, lock은 실제 변경 manifest/owner와 함께 조정한다. lock 제거/비검증 Cargo·skip hook·필수 테스트 축소로 green을 만들지 않는다.
