# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

현재 source: `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e` + 공유 dirty. [CURRENT-AUDIT](CURRENT-AUDIT.md)와 [IMPLEMENTATION-WAVE](IMPLEMENTATION-WAVE.md)의 명령·원본·검증 경계를 따른다. 수정 전 parity fail-open, 항상 켜진 query stage clock, 공개되지 않은 ingest stage report는 **현재 미구현 목록에서 제거**했다. 아래는 남은 실제 작업이다. `VERIFIED` local 결과를 clean-source proof나 검색 우위로 승격하지 않는다.

| ID | 남은 작업 | 현재 경계 / 종료 조건 |
| --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | 최종 source/document freeze 후 Python/Rust/SDK exact collection, terminal execution, source-bound contract/SDK receipts | 현재 Python authority는 283개. 실제 수집·실행의 exact equality와 raw terminal을 함께 확인해야 한다. 공유 dirty local 실행은 허용하되 clean closure는 `NOT_RUN` |
| [RBR-01](RBR-01-diagnostics.md) | final-binary actual daemon on/off·diagnostic5/protocol3 replay 및 동일 workload 비용 측정 | 첫 SDK17/17·actual sidecar 정상+5변조 거부는 local 관측; 최종 재캡처 대기. `SPEC_OPTIONAL` selector 누락으로 disabled spec을 거부하던 실제 실행 결함도 수정했다. enabled/disabled/schema·invalid9 회귀 포함. roomy budget 결과 동등성; tight deadline·sidecar 비용 별도 |
| [RBR-02](RBR-02-query-policy.md) | 정책/identity/negative fixture의 최종 revision 재자격 | 정책 코드는 구현돼 있다. native 제품 정책은 변경하지 않음; clean receipt `NOT_RUN` |
| [RBR-03](RBR-03-semble-profiles.md) | 네 profile의 pinned reference capture·phase event·raw hash 재발급 | profile/phase validator 구현. 과거 탐색 pair를 현 source 자격으로 쓰지 않음 |
| [RBR-04](RBR-04-symbol-producer.md) | 5언어 hand oracle·strict-full coverage·combined replace/reopen의 고정-source proof | 파일 path/SHA/text 및 coverage replay 구현. 현재 owning unit/live SDK와 clean receipt를 분리 |
| [RBR-05](RBR-05-symbol-route-proof.md) | 실제 symbol route·forged/stale/no-answer/timeout·span binding 재자격 | 공통 route/record producer 구현; final live SDK raw terminal 필요 |
| [RBR-06](RBR-06-span-chunking.md) | 외부 development의 strict vs line-aligned 1024·same-overlap/whole-file matrix | indexed/SDK/scored span 및 rank/context 지표 구현. primary density NDCG는 변경하지 않음; 외부 matrix `NOT_RUN` |
| [RBR-07](RBR-07-semantic-parity.md) | 외부 동일 vectors/filters/k의 exact-vs-served CLI 구현·원인 분해, claim-specific T15 proof 연결 | strict schema2 fixture·9×256 capture·manual serde·정상/invalid 거부 local 검증. tolerance 0.002/0.005 유지. T15 exporter는 구현됨; `vector_index.rs::exact_top_k`는 cfg(test) synthetic helper여서 외부 분해 도구는 아직 없음 |
| [RBR-08](RBR-08-symbol-ranking.md) | 기존 symbol route 반환 순위 probe → 재현되면 bounded 후보-stage trace/한 후보, 아니면 관측 범위의 유지 결정 | reachable ranker 결함 미확정. 현재 diagnostics는 `returned_window_only`; 미반환 정답이 후보에 진입했는지/어디서 탈락했는지는 미관측. probe `NOT_RUN`; ranker 교체 금지 |
| [RBR-09](RBR-09-query-performance.md) | bounded experimental floor25/50 selector 구현, 100/25/50 × k/filter·ANN guard·quiet-host p95 | core `MIN_INTERNAL_FETCH_K=100` 상수이며 현재 config/CLI에 selector 없음. top-k 변경을 floor 실험으로 대체하지 않음. production floor100 유지. observation on/off·overhead replay 구현과 실제 frontier/성능 자격 분리 |
| [RBR-10](RBR-10-ingest-performance.md) | fresh 비용·T16 owner delta/full rows → 필요한 경우 daemon delta/fault/restart rail·한 최적화 후보 | public transient outcome 연결·manual serde policy 보완. 일반 runner는 fresh root/단일 publish이며 delta CLI 없음. T16은 precomputed vectors의 sealed owner-state·5mutation, provider embedding/activation/daemon fault·restart 증명이 아님. durable receipt에 elapsed 비혼입·activation null 유지 |
| [RBR-11](RBR-11-resource-accounting.md) | 지원 플랫폼 owner proof 및 clean resource replay | ps PID-start identity 미보장 한계 유지. macOS live fixture는 child reaping·실제 PID/RSS를 검증하고 cleanup EPERM을 성공으로 바꾸지 않음 |
| [RBR-12](RBR-12-evaluation-closeout.md) | final-source T15/T16 실제 양성/변조 거부·terminal custody, frozen admission/live replay·단일 final pair | exporters·typed operation/full-row consumer 구현. 독립 감사의 no-op/terminal/scalar/sentinel 거부 회수. 실제 T16 raw5/5 local 관측은 semicolon-only lint 수정 전 binary이며 최종-source 회수 별도. claim=false `NOT_APPLICABLE`; self-reported build/binary hash는 signed attestation 아님 |

## 직렬 종료 순서

1. 현재 raw 실행의 실패를 RCA하고 소유 코드/fixture만 수정한다. producer→IPC/SDK→runner→diagnostic→Python replay와 실제 daemon을 확인한다. API/module/fuzz/daemon escalation gates 및 exact inventory를 최종 bytes에서 회수한다.
2. 새 T15/T16 구현은 independent raw oracle, model/dependency/config/source/binary/environment, exact command 및 terminal custody를 묶어 양성/음성 모두 확인한다. exporter의 자기 보고와 임의 `pass`/count는 단독 증거가 아니다. Semble native `max_length=512`와 controlled `None`는 다른 정책이다.
3. development에서 RBR-06/07/08/09/10의 유한 원인 matrix를 실행한다. 단일 후보 또는 근거 있는 유지 결정을 고정한다. 외부 gold가 없는 개발 진단과 qualified quality를 구분한다.
4. 문서도 source closure 입력이므로 최종 문서 갱신 뒤 고정-source proof를 새로 발급한다. frozen corpus/model/spec·admission/독립 gold·holdout custody·quiet host가 충족된 때 한 final holdout pair/replay를 판정한다. 수동 승인·심사자·host 확보는 engineering 작업 티켓에 넣지 않는다.

외부 corpus-set `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-v4/corpus-set.json`은 SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33`, 10 repo/1,480파일의 `candidate_not_admitted_no_gold_no_pair` 후보다. manifest 확인은 현재 checkout/독립 gold/실제 pair 검증이 아니다. final `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`는 **NOT_RUN**이다.

수정 전 단계별 상태는 Git 역사와 [CURRENT-AUDIT](CURRENT-AUDIT.md)의 역사 절에 남는다. 현재 미구현 목록으로 재사용하지 않는다.
