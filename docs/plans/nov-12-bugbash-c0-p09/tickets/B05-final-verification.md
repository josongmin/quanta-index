# B05 · 최종 검증과 실행 인계

## 현재 상태

- **Source guard FAILED**(exit 1). 11월 17일 고정 guard와 명세의 SHA를 coordinator resume spec과 대조해 재실행했다. 현행 common owner `6e7710fc`에서 승인 조합 `220b2687`과 다르다고 거절됐다. Native caller `45340aa8`의 import와 두 verified-switch 호출은 연결돼 있다. 정확한 조합 재검토와 worker 공급이 남았다. P09는 **NOT_RUN 0/1**이다.
- C0: **VERIFIED 1/1**, 원본 run `20261112T094353.543064Z-b94847376727`, exit 0, failed/ignored 0. 원본 verifier를 재확인했다. [원본 QBC receipt](</Users/songmin/Library/Caches/semantica-codegraph-v2-target/quanta-build-cli/_state/execution-roots/semantica-codegraph-v2-4935153f7ddd/lanes/qidx-c0-r11-reboot-20261109-snapshot-artifact/verification-results/20261112T094353.543064Z-b94847376727/receipt.json>).
- 고정 Native feature-on 증거: F의 기존 공통 owner 이동을 유지하고 Native parent에 `selected_definition_v222` import 1줄을 복구했다. verified switch import·두 호출과 하위 pending-virtual 알고리즘은 변경하지 않았다. 전후 boundary 정책 5/5, 고정 source `10479e54…+ovl-e4627ac6…+ext-576cfabb…`의 Native feature-on QBC lib check가 **VERIFIED**, exit 0이다. 원본 run `20261115T074359.987701Z-e2fe99a7234b`의 command·cwd·source·fresh PTA compiler artifact를 확인했다. 공식 no-default worker는 아직 입장 전이며 **NOT_RUN**이다. [Native 원본 결과](/Users/songmin/.codex/task-evidence/qidx-nov12-bugbash-qiisv3su/two-code-fixed-cohort-20261115T073611Z/native-owner-result.json).
- 기존 source guard 회귀 **39/39 PASS**는 검토된 common owner `220b2687…`와 Native caller `5e5a8508…` 범위다. [승인 조합 계약](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#reviewed-native-worker-source-pairs)
- 11월 15일 입장은 exit75에서 끝났고 worker 공급 journal·빌드·P09 입력은 생성되지 않았다. PID·용량·C34와 claim 관측은 [당시 원 상태](/Users/songmin/.codex/task-evidence/qidx-nov12-bugbash-qiisv3su/admission-current-closeout-20261113/sequential-closeout.json)에 보존한다.

## Current resume contract

The current state above owns the Native/no-default/guard verdict and P09's
missing current inputs/body. Completed C0 source-scoped 1/1 is reused; the exact
C0 rail below preserves its acceptance contract, not a request to rerun it.
Nov-12–15 waiter PIDs, C34 reservations, VM memory/disk/swap samples, service-stop
and preflight histories are retired as current operating instructions.
They do not establish present host capacity, a live lease or a new failure.

| Existing request | Remaining contract |
| --- | --- |
| F-SUPPLY-CURRENT-01 | Consume the actual current registered worker lane/product recipe and paired searchd result. No unregistered lane, duplicate supplier or incompatible seed; seed absence alone is not a product failure. |
| F-ADMISSION-01 | Recheck original canonical reservation, exclusive capacity and host/guest resource conditions at admission. Old PID/load/disk samples do not authorize killing services, bypassing a lease or starting a competing runner. |
| F-INPUT-CURRENT-01 | Final attempt-input must bind the actual source, lockfile, recipe and toolchain, worker/searchd SHA-256, the four required SDK environment inputs, original supplier receipts and the public packet. Preparation JSON is not execution input; do not fill missing hashes or promote old V83 artifacts. |
| F-PREFLIGHT-CURRENT-01 | Preserve exact source guard and Linux preflight after the already implemented owner-mount repair. Local resolver/Python guards are not production worker/installed success. |
| F-CLOSEOUT-01 | Reuse C0's valid original witness; current P09 still requires selected=executed=passed1, same input/binary/receipt and same-daemon container/claim cleanup under the unchanged exit conditions below. |

The existing single coordinator and [completed admission/claim contracts](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md)
are reused. Exact historical commands, field/guard changes, hashes, observed
failures and artifacts remain in the [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-full-document-and-ticket-reconciliation).
No new runner, current resource observation or runtime proof is supplied by this
compaction. Original verification, observer and exit criteria below are unchanged.

## 1. 변경부 검증

각 파일 소유자가 기존 회귀에 해당 티켓의 장애 주입을 추가하고 F가 통합본을 실행한다. 아래는 현재 파일 기준 검증 명령이다. 이동한 경우 전환 기록에 새 경로를 남긴다.

```sh
set -eu
cd "$QIDX_RUN"
python3.12 -B test-c0-p09-coordinator.py
python3.12 -B test_consumer_admission_v1.py
python3.12 -B test-delta-host-admission.py
python3.12 -B test-delta-searchd-identity.py
bash -n delta-container.sh
cd "$SEMANTICA_ROOT/tools/quanta-build-cli"
python3.12 -B -m unittest test_compile_activity_host_lock test_execution_resource_admission_v1
```

회귀의 종료값, 실행 수, 확인한 실패 조건을 남긴다. V83 기존 설치 입력 검증과 새 코호트 설치 입력 검증의 귀속을 각각 확인한다.

통합 회귀는 기존 대기기·P09 실행기·셸 호출을 한 번 연결한다. 대기기 fixture는 A, 자원 관측 fixture는 B, QBC claim fixture는 C, 컨테이너·guest guard fixture는 D가 자기 테스트 파일에 작성한다. F는 인수한 회귀를 실행하고 추가 fixture 요구를 해당 작성자에게 돌려준다. 가짜 Docker와 임시 QBC coordination root를 사용해 claim 발행 성공 전 설치용 `create` 호출이 0건인지 확인한다. 생성·시작 응답 유실 뒤 다른 데몬의 빈 조회가 claim을 해제하지 않는지, 같은 데몬의 실제 종료·제거가 확인된 뒤에만 대기기가 terminal을 소비하는지 확인한다. 동일 본문 시작은 1건이다.

## 2. 실제 본문 실행

| 대상 | 실행 경로 | 통과 조건 |
| --- | --- | --- |
| C0 Host publish-panic | macOS 기존 Semantica QBC `owner resolve` → `owner run`. `quanta-runtime --lib`, `feature-isolation:quanta-runtime.no-default.502ba7d77f57`, exact `retrieval::voyage_adapter::native_product_host_v3::tests_v3::initialized_host_publish_panic_drains_queued_pending_and_refunds_v3`. | 현재 시도의 QBC 불변 영수증에 고정 소스와 해당 witness 1건, exit 0, 실패·무시 0이 결속된다. 기존 본문의 panic·대기 요청 정리·환급 assertion을 모두 통과한다. |
| P09 현행 설치 DELTA | [B04](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b04-attempt-inputs-and-cli)의 고정 입력으로 Linux/arm64 설치 실행기와 기존 등록 suite `ifx150-native-checked-delta-second-terminal-product-owner`를 사용한다. Exact `native_checked_delta_from_real_full_ack_publishes_second_v5_terminal_v3`. | selected = executed = passed = 1, exit 0, 실패·무시 0. 현재 시도의 suite와 QBC 원본 영수증의 digest·run ID·source·witness가 일치한다. 실제 사용한 searchd·SQL worker SHA-256이 선택 입력과 일치하고, FULL ACK→DELTA와 거절 이후 송신 관측 assertion을 통과한다. 해당 컨테이너와 claim 정리까지 확인한다. |

C0의 고정 입력 1/1 영수증을 재사용한다. P09는 `--attempt-input`으로 preflight와 `--validate-inputs`를 먼저 확인하고 같은 파일을 통합 대기기에 전달한다. [C0 Rust exact](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/native_product_host_v3/tests_v3.rs:1001) · [기존 suite 등록](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml:11383) · [설치 Rust exact](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/tests/ifx150_restart_recovery_product_owner.rs:19)

실행 직전에 C34와 기존 호스트 작업의 실제 상태를 확인하고 canonical 입장을 사용한다. Native F1/F2/F3와 Java PTA의 공유 호스트·VM 예약도 [공유 실행 경계](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#shared-execution-and-handoff)에 따라 대조한다. 현재 호스트 디스크 바닥은 50 GiB와 총 용량 2.75%를 모두 적용한다. P09 게스트는 가용 메모리 25 GiB, 스왑 여유 512 MiB 및 기존 경쟁 작업 조건을 적용한다. 기존 대기기·시도·컨테이너를 조회해 같은 작업의 동시 실행을 방지한다. [실행 인계](/Users/songmin/.codex/task-evidence/qidx-c0-reboot-recovery.q2kIxd/p09-linux-run/sole-supplier-handoff.json)

P09는 [B01 coordinator](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b01-c0-and-coordinator)와 [B02 claim](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b02-qbc-external-claim) 계약을 적용한다. [B03 복구](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b03-p09-execution-and-recovery)와 [B04 입력](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b04-attempt-inputs-and-cli)을 기존 대기기에서 검증한 뒤 같은 P09 본문을 실행한다. 원 C0 exact는 위 고정 영수증을 재사용한다.

<a id="p09-observer-acceptance"></a>
### P09 송신 관측 수용

같은 Semantica Runtime·SDK와 Q IPC `native-send-observer-v1` 입력의 기존 exact에서
G1 정상 DELTA 송신 양성, G2 dispatch 기준점, 각 거절 이후 송신 시도 증가 0과 종료 처리를 확인한다.
계수는 클라이언트 요청 프레임·진단 preface만 대상으로 하며, 서버 bounded terminal 응답은 기존 raw UDS 회귀로 확인한다.
이 조건은 위 설치 DELTA 한 본문의 기존 수용이며 추가 실행 큐나 별도 티켓이 아니다.
로컬 계약은 [Native ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#consolidated-implementation-boundaries),
종료 응답은 [Query ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#retained-profile-and-terminal-reply-details)이 소유한다.

## 3. 종료 판정

- C0는 위 원본 영수증의 **VERIFIED 1/1**을 재사용한다. 현행 P09 설치 본문은 **0/1, NOT_RUN**이다. 입장하지 못하면 차단 원인과 시도를 기록하고 본문 상태를 유지한다. 이전 V83 시도 07의 QBC exit 143·본문 0은 V83 실패 이력에 남긴다.
- V83이 별도 실행에서 통과하면 해당 고정본 결과로 기록한다. 현행 수용은 위 P09의 새로운 고정 입력과 실제 본문으로 판정한다. 관련 입력이 유지된 검증 산출물은 재사용한다.
- 실패 시 같은 시도의 도구 종료, 컴파일·링크, 본문 실패를 구분한다. 링크 제한에 다시 도달하면 링크 단계와 Cargo 재사용 관측을 확인하고 원인 또는 관련 입력 변경을 남긴 뒤 재시도한다.
- F가 이 B05의 현재 상태에 실제 명령, 고정 입력, 종료 코드, 실행 수와 영수증을 연결한다. [제품 원장](../../oct-22-finalizing/README.md)의 해당 에픽 작성자가 같은 결과를 참조한다. Native 전환 담당은 입력이 일치하는 P09 영수증을 DELTA 부분 증거로 소비하고 추가 view·delete/no-op·재시작 단언을 확인한다. 통과 범위는 실행된 exact 본문에 귀속한다.

## 4. 감사 항목별 종료 확인

The eight original review items use the
[coordinator](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b01-c0-and-coordinator),
[external claim](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b02-qbc-external-claim),
[installed recovery](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b03-p09-execution-and-recovery)
and [attempt input](../../../adr/NOV-12-002-execution-admission-and-terminal-recovery.md#b04-attempt-inputs-and-cli)
contracts. Their original mapping remains in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).
The required final checks follow below.

**레인 F가 아래 네 항목을 모두 확인하면 버그배시를 종료한다.**

- [ ] 감사 1–8번의 재현·수정 후 회귀와 통합 회귀에 실행 결과가 있다.
- [ ] 활성 대기기·자식·셸이 같은 검증 버전이다. 이 작업의 미회수 child·컨테이너·claim이 없다.
- [ ] C0 exact와 현행 P09 설치 DELTA가 각각 1/1 통과했고, 해당 시도·소스·바이너리의 영수증이 원장에 연결됐다. V83 영수증은 해당 고정본 이력에 남긴다.
- [ ] 종료 시점 입력을 다시 대조했다. 관련 소스·실행 도구·빌드 입력이 바뀌었으면 영향받은 증거를 갱신했고, 그대로인 증거는 이어받았다.

입장 거절이나 재검증 미실행이 남아 있으면 해당 단계와 재개 조건을 기록한다. F는 조건이 회복되면 같은 작업을 이어서 위 목록을 닫는다.

## Retired section links

These historical section IDs recover the [exact preimage](../../../ARCHIVE-INDEX.md#2026-11-16-full-document-and-ticket-reconciliation).
Current work follows the first packet/status and unchanged acceptance, not old
queue/PID/patch/freeze instructions.

<a id="11월-12일-후속-재개"></a><!-- 11월 12일 후속 재개 -->
<a id="독립-실행-채팅의-인수-범위"></a><!-- 독립 실행 채팅의 인수 범위 -->
<a id="최초-실행-인수-상태-이력"></a><!-- 최초 실행 인수 상태 (이력) -->
<a id="f--현행-고정본-및-실제-입장-결과"></a><!-- F · 현행 고정본 및 실제 입장 결과 -->
<a id="f--실제-linux-preflight의-git-owner-경계-수리"></a><!-- F · 실제 Linux preflight의 Git owner 경계 수리 -->
<a id="dependency_requests--정확한-재개-조건"></a><!-- dependency_requests · 정확한 재개 조건 -->
