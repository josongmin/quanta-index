# P09 · Native IPC 취소 응답 계약과 송신 관측

one-shot 계약과 로컬 IPC·SDK 회귀 `VERIFIED` · 기존 설치 DELTA observer 제품 회귀 `BLOCKED`.

## 1. One-shot 종료 응답 계약

[IPC server](../../../../crates/quanta-index-ipc/src/server.rs)는 `terminal_reply_phase()`가 참인 응답에만 제한된 송신 예산을 부여한다. [Native envelope](../../../../crates/quanta-index-searchd-runtime/src/native_reply.rs)는 현재 opt-in하지 않는다. [Native SDK](../../../../crates/quanta-index-sdk/src/native_uds.rs)는 원래 요청 budget 아래 응답을 수신한다.

- Native one-shot은 응답 송수신에 원래 절대 deadline을 적용한다. 명시 취소는 `Cancelled`, 요청 시간 소진은 `ResourceLimited`로 구분한다. 현재 계약은 deadline 이후 연결된 peer에 typed terminal 송신을 보장하지 않으므로 envelope opt-in이 필요하지 않다.
- 실행 중인 durable mutation의 결과가 불확실하면 SDK는 원 operation key를 유지한다. 이후 별도 요청 budget에서 같은 key의 `status` 또는 `resume`으로 결과를 확인한다. raw peer에 deadline 이후 typed terminal을 보장할 새 계약은 해당 요청과 오류 코드를 먼저 확정한다.

## 2. 기존 DELTA observer 수용

[observer](../../../../crates/quanta-index-ipc/src/native_send_observer_v1.rs)는 이미 Semantica Runtime의 `source_bound_replay_owner_test_support_v3`를 거쳐 SDK의 `logical_io_after_refusal_v3.rs`에서 소비한다. 기존 fixture의 arm·G1 양성·G2 dispatch 기준점·거절 구간·종료 처리를 그대로 사용한다. [E02 기존 실행 레인](../../oct-22-finalizing/VALIDATION.md#e02-peer-ownership-refresh-20261106)의 `ifx150-native-checked-delta-second-terminal-product-owner`에 수용 결과를 기록한다.

hook 범위는 **클라이언트 요청 프레임과 진단 preface의 송신 시도**다. 거절 이후의 송신은 기존 DELTA observer로, 서버 one-shot 응답은 기존 raw UDS 회귀로 확인한다.

기존 selector는 현재 레지스트리에서 exact 1개로 resolve된다. 본문 하나가 정상 DELTA와 거절 시나리오를 순차 실행한다. 설치 실행에는 매칭된 Linux searchd, 적격 SQL worker image와 SHA-256이 필요하다. RQR-E/B2 C는 SQL worker 공급 경로다.

기존 RQR-E Source5 고정본의 Quanta Index `de16b193b`에는 송신 observer가 없다.
P09는 현재 Semantica DELTA observer leaf와 Quanta Index IPC observer·feature를
함께 고정한 입력에서 E02 기존 selector 본문을 실행해 판정한다.

2026-11-08 17:45 KST에 기존 B2 C 공급 프로세스는 host admission을 대기 중이다.
Linux preflight와 searchd 빌드 계획은 준비됐고, 실행 가능한 worker와 searchd
출판은 0건이다. 공급 후보 고정본의 Runtime·SDK·DELTA 테스트 및 Q IPC
observer 관련 파일 11개가 현재 소스와 일치했다. 그 고정본에서 기존 DELTA
selector가 exact 1건으로 resolve된다.

공급 레인에 연결된 A01은 TaintRows 검증을 실행한다. P09 수용은 공급된 SQL
worker·searchd의 digest와 소스·ABI를 확인한 뒤 기존 E02 DELTA selector에서
판정한다. 현재 검증 입력에는 후속 Q IPC observer 수정과 P08의 searchd 생산
코드를 함께 반영해야 한다.

2026-11-09 KST에 기존 B2 C 공급 빌드가 시작됐다. 시작된 공급본의 Q IPC
observer와 searchd Sidecar 코드를 현재 파일과 비교한 결과 두 경로 모두
변경 전 소스였다. 공급 빌드의 종료와 별개로 현재 observer를 사용하는 SDK와
현재 Sidecar가 포함된 searchd로 설치 실행 입력을 맞춰야 한다.
첫 공급 실행 `20261108T150652.770910Z-67f03a7a5816`은 private Git 의존성
fetch에서 exit 101로 종료됐다. 정확한 두 revision은 읽기 전용 게스트 마운트에
있으며, 기존 공급 실행기는 해당 Git URL만 로컬로 매핑하는 재시도를 대기한다.
이 실패 실행에서 worker artifact는 발행되지 않았다.

## 로컬 실행

2026-11-08에 `./scripts/cargow nextest run`과 `quanta-index-ipc/native-send-observer-v1`
feature로 기존 8건을 선택·실행해 8건 통과했다. Native SDK 2건은 취소와 불확실한
durable 작업의 원 operation key 보존을 확인했다. IPC 6건은 UDS admission·송신/응답
수명과 deadline 경계를 확인했다. 늦은 one-shot 응답은 raw peer에서 EOF로 끝났고
응답 바이트와 in-flight dispatch가 0이었다. 설치 DELTA 본문 실행은 0건이다.

19:43 KST에 Q IPC observer의 미무장 assertion 이후 재무장에서 `PoisonError`가
발생하는 것을 Rust로 재현했다. `snapshot_v1`과 `disarm_v1`의 상태를 락 안에서
복사하고 락 해제 후 검사하도록 수정했다. 동일 회귀는 두 assertion 실패 후
각각 재무장·계수·해제를 확인한다. `./scripts/cargow --lane shared nextest run
--locked --offline -p quanta-index-ipc --features native-send-observer-v1 --lib`의
집중 회귀는 7/7 통과했다. 연결된 Semantica SQL observer의 동일 문제는
해당 reader 담당 범위다.

독립 프로세스에서 실제 UDS 송신을 관측하는 회귀를 추가했다. 전송 전 거절은
증가 0, one-shot과 stream은 각각 증가 1, 진단 preface와 구 peer fallback은
합계 증가 2를 검증한다. IPC 집중 회귀 8건이 통합 실행 93/93에서 통과했다.
Nextest run `4eb023e4-a5af-462e-b5b0-0d24e8cf9a84`.

## 완료 조건

- [x] one-shot 계약과 기존 deadline을 확인했다. durable 작업 결과가 불확실할 때 SDK가 원 operation key를 반환하며 이후 재조회에 사용한다.
- [x] Q IPC observer는 미무장 assertion을 두 차례 처리한 뒤 다시 arm·계수·해제할 수 있다.
- [x] 실제 UDS 송신의 one-shot·stream·진단 preface hook과 전송 전 거절을 계수로 검증한다.
- [ ] 기존 DELTA 회귀에서 정상 요청 송신 양성, 각 거절 기준점 이후 송신 시도 증가 0, 종료 처리를 확인한다. Q IPC와 Semantica의 Runtime·SDK feature를 포함한 같은 실행 입력에서 결과를 남긴다.
