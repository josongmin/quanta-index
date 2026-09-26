# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

최신 관측 source: `6e266b99ab03cd585aa0a3e2f8b3bf890cd04169` + 공유 dirty. [CURRENT-AUDIT](CURRENT-AUDIT.md)와 [IMPLEMENTATION-WAVE](IMPLEMENTATION-WAVE.md)의 명령·원본·검증 경계를 따른다. parity fail-open·항상 켜진 stage clock·공개 ingest report 부재·symbol literal false-exhaustion·strict UTF-8 byte 누락·JSON finite scalar와31 process-preflight 등록은 현재 미구현 목록에서 제거했다. 아래는 실제 잔여다. local 증거를 clean-source proof나 검색 우위로 승격하지 않는다.

| ID | 남은 작업 | 현재 경계 / 종료 조건 |
| --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | 최종 source/document freeze 후 Python319 전체·source-bound contract/SDK receipts | authority319 actual exact collector·새31 focused/consumer 거부증명 및 Rust108/SDK18 terminal은 회수. 이전288 전체+31 focused를319 전체로 합성하지 않음. clean closure NOT_RUN |
| [RBR-01](RBR-01-diagnostics.md) | 동일 workload on/off·sidecar 비용 측정 및 final-source custody | SDK18/18 actual on/off·floor9조건과 diagnostic6/protocol4·3route/10mutants replay 회수. roomy budget 동등성은 검증했으나 tight deadline·실제 overhead 비용은 별도 NOT_RUN |
| [RBR-02](RBR-02-query-policy.md) | 정책/identity/negative fixture의 최종 revision 재자격 | 정책 코드는 구현돼 있다. native 제품 정책은 변경하지 않음; clean receipt `NOT_RUN` |
| [RBR-03](RBR-03-semble-profiles.md) | 네 profile의 pinned reference capture·phase event·raw hash 재발급 | profile/phase validator 구현. 과거 탐색 pair를 현 source 자격으로 쓰지 않음 |
| [RBR-04](RBR-04-symbol-producer.md) | 5언어 hand oracle·strict-full coverage·combined replace/reopen의 고정-source proof | 파일 path/SHA/text 및 coverage replay 구현. 현재 owning unit/live SDK와 clean receipt를 분리 |
| [RBR-05](RBR-05-symbol-route-proof.md) | 실제 symbol route·forged/stale/no-answer/timeout·span binding 재자격 | 공통 route/record producer 구현; final live SDK raw terminal 필요 |
| [RBR-06](RBR-06-span-chunking.md) | 외부1024 same-declared-overlap/whole-file matrix와 final-source custody | UTF-8 before1failed 후 수정·chunking25·union9/tiny-budget4·scoped Clippy/fmt 및 current Rust108 local 통과. SDK 이전 binary를 이 수정 증거로 쓰지 않음. span/보조지표 구현·primary density NDCG 불변. 외부 matrix NOT_RUN |
| [RBR-07](RBR-07-semantic-parity.md) | 외부 corpus/query/filter/churn matrix 확장과 source-qualified custody | CLI unit4·Clippy/fmt·synthetic512/14pre-write 거부·overwrite 거부 terminal 회수. 실제 pinned-model406×256/4query exact/served10/10/8/0·full semantic-row/source/vector·mutant10 local VERIFIED. membership raw-only. 전체 ANN/품질/perf·churn 및 matrix breadth NOT_RUN. parity tolerance 유지 |
| [RBR-08](RBR-08-symbol-ranking.md) | 실제 phrase/raw/regex 지원의 canonical authority migration 결정·조건부 rank 실험 | typed refusal 구현·lib121/smoke37/192조합 및 installed SDK18 native symbol positive/literal refusal local VERIFIED. canonical text authority는 schema/ingest/lifecycle/cursor migration을 동반하는 별도 기능 확장 NOT_RUN. ranker 결함 미확정; 근거 없는 boost 금지 |
| [RBR-09](RBR-09-query-performance.md) | 외부 k/filter·ANN guard·quiet-host 비용-품질 frontier | selector/core/daemon/runner/spec 및 diagnostic6/lock4 floor SHA·actual probe 구현. installed floor25/50/100×k1/10/100·SDK18 terminal 회수; default100 유지. 외부 frontier NOT_RUN |
| [RBR-10](RBR-10-ingest-performance.md) | fresh/replace/delete 비용의 별도 delta 시나리오 측정 → 근거가 있으면 최적화1개; fault/restart proof는 별도 | runner fresh boot+ReplaceGeneration+CAS expectedNone는 의도된 fresh-only 계약이며 delta 제품 결함이 아님. T16 sealed owner-state5mutation/full rows correctness는 회수했지만 provider embedding/activation/daemon fault·restart/latency proof가 아님. publish+activate 총 wall/receipt/ACK/public transient은 이미 연결됨. activation_ns=None은 별도 control request라는 명시적 granularity이며 unknown→zero가 아님. durable receipt 불변 |
| [RBR-11](RBR-11-resource-accounting.md) | 지원 플랫폼 owner proof 및 clean resource replay | ps PID-start identity 미보장 한계 유지. macOS live fixture는 child reaping·실제 PID/RSS를 검증하고 cleanup EPERM을 성공으로 바꾸지 않음 |
| [RBR-12](RBR-12-evaluation-closeout.md) | final-source custody·단일 final pair | 이전4fix는 b24d1148·immutable owner283+32 subtests local VERIFIED. 후속 deletion-counter/huge-number/object-shape3fix 및 인접 finite scalar guard는 코드 반영·focused/actual9/5 replay 통과. owner frozen288 전체287pass/1fail은 nested just dry-run oracle 오류로 기존 test를 보강해 재실행 중. 독립 current319 전체도 별도 실행; terminal 전 NOT_RUN. prior/focused 결과를 합성하지 않음. claim=false NOT_APPLICABLE; self-report는 signed attestation 아님 |

## 직렬 종료 순서

1. 현재 raw 실행의 실패를 RCA하고 소유 코드/fixture만 수정한다. producer→IPC/SDK→runner→diagnostic→Python replay와 실제 daemon을 확인한다. API/module/fuzz/daemon escalation gates 및 exact inventory를 최종 bytes에서 회수한다.
2. 새 T15/T16 구현은 independent raw oracle, model/dependency/config/source/binary/environment, exact command 및 terminal custody를 묶어 양성/음성 모두 확인한다. exporter의 자기 보고와 임의 `pass`/count는 단독 증거가 아니다. Semble native `max_length=512`와 controlled `None`는 다른 정책이다.
3. development에서 RBR-06/07/08/09/10의 유한 원인 matrix를 실행한다. 단일 후보 또는 근거 있는 유지 결정을 고정한다. 외부 gold가 없는 개발 진단과 qualified quality를 구분한다.
4. 문서도 source closure 입력이므로 최종 문서 갱신 뒤 고정-source proof를 새로 발급한다. frozen corpus/model/spec·admission/독립 gold·holdout custody·quiet host가 충족된 때 한 final holdout pair/replay를 판정한다. 수동 승인·심사자·host 확보는 engineering 작업 티켓에 넣지 않는다.

외부 corpus-set `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-v4/corpus-set.json`은 SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33`, 10 repo/1,480파일의 `candidate_not_admitted_no_gold_no_pair` 후보다. manifest 확인은 현재 checkout/독립 gold/실제 pair 검증이 아니다. final `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`는 **NOT_RUN**이다.

수정 전 단계별 상태는 Git 역사와 [CURRENT-AUDIT](CURRENT-AUDIT.md)의 역사 절에 남는다. 현재 미구현 목록으로 재사용하지 않는다.
