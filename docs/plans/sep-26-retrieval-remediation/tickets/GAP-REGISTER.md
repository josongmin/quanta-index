# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

현재 source: `577d60b518344163145ad2f1afe1f3e7c656762e` + 공유 dirty. [CURRENT-AUDIT](CURRENT-AUDIT.md)와 [IMPLEMENTATION-WAVE](IMPLEMENTATION-WAVE.md)의 명령·원본·검증 경계를 따른다. 수정 전 parity fail-open, 항상 켜진 query stage clock, 공개되지 않은 ingest stage report, symbol literal false-exhaustion의 typed-refusal 보완은 **현재 미구현 목록에서 제거**했다. 아래는 남은 실제 작업이다. `VERIFIED` local 결과를 clean-source proof나 검색 우위로 승격하지 않는다.

| ID | 남은 작업 | 현재 경계 / 종료 조건 |
| --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | 최종 source/document freeze 후 Python/Rust/SDK exact collection, terminal execution, source-bound contract/SDK receipts | 현재 Python authority는 288개, Rust108/SDK18. 실제 수집·실행의 exact equality와 raw terminal을 함께 확인해야 한다. 공유 dirty local 실행은 허용하되 clean closure는 `NOT_RUN` |
| [RBR-01](RBR-01-diagnostics.md) | final-binary actual daemon on/off·diagnostic6/protocol4 replay 및 동일 workload 비용 측정 | 첫 SDK17/17·actual sidecar 정상+5변조 거부는 local 관측; 최종 재캡처 대기. `SPEC_OPTIONAL` selector 누락으로 disabled spec을 거부하던 실제 실행 결함도 수정했다. enabled/disabled/schema·invalid9 회귀 포함. roomy budget 결과 동등성; tight deadline·sidecar 비용 별도 |
| [RBR-02](RBR-02-query-policy.md) | 정책/identity/negative fixture의 최종 revision 재자격 | 정책 코드는 구현돼 있다. native 제품 정책은 변경하지 않음; clean receipt `NOT_RUN` |
| [RBR-03](RBR-03-semble-profiles.md) | 네 profile의 pinned reference capture·phase event·raw hash 재발급 | profile/phase validator 구현. 과거 탐색 pair를 현 source 자격으로 쓰지 않음 |
| [RBR-04](RBR-04-symbol-producer.md) | 5언어 hand oracle·strict-full coverage·combined replace/reopen의 고정-source proof | 파일 path/SHA/text 및 coverage replay 구현. 현재 owning unit/live SDK와 clean receipt를 분리 |
| [RBR-05](RBR-05-symbol-route-proof.md) | 실제 symbol route·forged/stale/no-answer/timeout·span binding 재자격 | 공통 route/record producer 구현; final live SDK raw terminal 필요 |
| [RBR-06](RBR-06-span-chunking.md) | strict UTF-8 zero-overlap byte 누락 수정·owning oracle → 외부1024 same-overlap/whole-file matrix | cap을 뒤로 snap한 end보다 다음 start가 앞서가 β/emoji2~4bytes를 빠뜨리는 reachable defect 확인. emitted end 이하의 next-start와 원문 byte-union·CRLF/EOF/tiny-budget 보강 중. indexed/SDK/scored span·rank/context 지표는 구현됨; primary density NDCG 불변. 외부 matrix NOT_RUN |
| [RBR-07](RBR-07-semantic-parity.md) | proof-only 외부 exact-vs-served CLI의 actual 양성/음성 terminal·외부 개발코퍼스 원인 분해 | 새 `ann_proof.rs`/`quanta-index-ann-proof`는 bounded manual parser·production build/open/search·독립 f64 exact scan·full row binding으로 구현 중. 실제 terminal 전 NOT_RUN. strict schema2/parity9×256은 별도 local VERIFIED; tolerance 유지 |
| [RBR-08](RBR-08-symbol-ranking.md) | final installed symbol keyword/typed-refusal 재검증; 실제 literal 지원은 별도 authority migration 결정 | 중앙 domain-aware typed refusal 구현·owning lib121/smoke37 및 192 refusal 조합 local VERIFIED. keyword positive 유지. SDK18의 실제 daemon 거부 계약은 terminal 대기. canonical symbol phrase/raw/regex 지원은 schema/ingest/lifecycle/cursor migration을 동반하는 별도 기능 확장 `NOT_RUN`. ranker 결함 미확정; 우선순위 boost 금지 |
| [RBR-09](RBR-09-query-performance.md) | fresh installed floor25/50/100 integration → 외부 k/filter·ANN guard·quiet-host frontier | typed selector/core/dispatcher/daemon/runner/spec 배선은 구현됨, default100 유지. owning plane427/config39·Clippy 및 Python protocol8 local VERIFIED. diagnostic6/lock4 floor SHA와 actual initial-fetch trace 대조. actual SDK18 terminal 대기; 외부 frontier NOT_RUN |
| [RBR-10](RBR-10-ingest-performance.md) | fresh/replace/delete 비용의 별도 delta 시나리오 측정 → 근거가 있으면 최적화1개; fault/restart proof는 별도 | runner fresh boot+ReplaceGeneration+CAS expectedNone는 의도된 fresh-only 계약이며 delta 제품 결함이 아님. T16 sealed owner-state5mutation/full rows correctness는 회수했지만 provider embedding/activation/daemon fault·restart/latency proof가 아님. publish+activate 총 wall/receipt/ACK/public transient은 이미 연결됨. activation_ns=None은 별도 control request라는 명시적 granularity이며 unknown→zero가 아님. durable receipt 불변 |
| [RBR-11](RBR-11-resource-accounting.md) | 지원 플랫폼 owner proof 및 clean resource replay | ps PID-start identity 미보장 한계 유지. macOS live fixture는 child reaping·실제 PID/RSS를 검증하고 cleanup EPERM을 성공으로 바꾸지 않음 |
| [RBR-12](RBR-12-evaluation-closeout.md) | final-source T15/T16 실제 양성/변조 거부·terminal custody, frozen admission/live replay·단일 final pair | exporters·typed operation/full-row consumer 구현. 실제 T15 vector9/9·T16 sealed8D delta5/5 및 독립1900 mutants 회수. mixed-model/dimension/append-window/empty-scope 보완은 b24d1148, immutable owner whole283+32 subtests source-stable local VERIFIED. 최신288 전체·suite-wide custody는 별도 NOT_RUN. claim=false `NOT_APPLICABLE`; self-reported build/binary hash는 signed attestation 아님 |

## 직렬 종료 순서

1. 현재 raw 실행의 실패를 RCA하고 소유 코드/fixture만 수정한다. producer→IPC/SDK→runner→diagnostic→Python replay와 실제 daemon을 확인한다. API/module/fuzz/daemon escalation gates 및 exact inventory를 최종 bytes에서 회수한다.
2. 새 T15/T16 구현은 independent raw oracle, model/dependency/config/source/binary/environment, exact command 및 terminal custody를 묶어 양성/음성 모두 확인한다. exporter의 자기 보고와 임의 `pass`/count는 단독 증거가 아니다. Semble native `max_length=512`와 controlled `None`는 다른 정책이다.
3. development에서 RBR-06/07/08/09/10의 유한 원인 matrix를 실행한다. 단일 후보 또는 근거 있는 유지 결정을 고정한다. 외부 gold가 없는 개발 진단과 qualified quality를 구분한다.
4. 문서도 source closure 입력이므로 최종 문서 갱신 뒤 고정-source proof를 새로 발급한다. frozen corpus/model/spec·admission/독립 gold·holdout custody·quiet host가 충족된 때 한 final holdout pair/replay를 판정한다. 수동 승인·심사자·host 확보는 engineering 작업 티켓에 넣지 않는다.

외부 corpus-set `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-v4/corpus-set.json`은 SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33`, 10 repo/1,480파일의 `candidate_not_admitted_no_gold_no_pair` 후보다. manifest 확인은 현재 checkout/독립 gold/실제 pair 검증이 아니다. final `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`는 **NOT_RUN**이다.

수정 전 단계별 상태는 Git 역사와 [CURRENT-AUDIT](CURRENT-AUDIT.md)의 역사 절에 남는다. 현재 미구현 목록으로 재사용하지 않는다.
