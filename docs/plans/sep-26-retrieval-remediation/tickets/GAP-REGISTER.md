# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

현 코드 재감사 기준 HEAD `6a142add69b4ff73b716de5ec2c0514f3710f0bd`; 감사 시작 시 기존 dirty는 retrieval source closure에 포함된 sep-23 비교 문서 1개였다. 재감사 중 해당 문서는 `da3e5d91`로 커밋되고 catalog의 다른 writer 편집이 생겼다. `6a142add..da3e5d91`에 RBR 구현 변경은 없지만 source-closure 문서가 바뀌었고, 이번 갱신으로 sep-26 티켓도 dirty가 되었다. 아래 잔여 공백은 `6a142add` 코드에서 다시 확인한 것이며 새 HEAD의 receipt가 아니다. 티켓별 계약과 증거 한계는 [CURRENT-AUDIT.md](CURRENT-AUDIT.md), [RBR-00~12](INDEX.md), [TEST-PLAN.md](TEST-PLAN.md)을 따른다. 코드 관측을 `VERIFIED`나 qualified 비교로 승격하지 않는다. 아래 상태는 **남은 작업**의 상태다.

| ID | 우선순위 | 남은 코드/결정 | 현행 증거·종결 게이트 |
| --- | --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | P1 proof | 현 HEAD Python 272/272 exact; Rust/SDK exact collection 및 최종 계약 closure 재자격 | 현 HEAD 선택 Python 11+4 passed; 전체 Python과 clean-source contract/SDK receipt `NOT_RUN` |
| [RBR-01](RBR-01-diagnostics.md) | P1 코드 | 서버 query stage duration/calls·source binding, RBR-10 ingest stage 공개, diagnostic on/off overhead | current-source SDK/daemon roundtrip·overhead `NOT_RUN` |
| [RBR-02](RBR-02-query-policy.md) | P1 proof | 정책/identity/negative fixture 최종 revision 재실행 | current-source SDK/receipt `NOT_RUN`; native 제품 정책은 변경하지 않음 |
| [RBR-03](RBR-03-semble-profiles.md) | P1 proof | pinned reference capture·phase event·raw hash 최종 revision 재발급 | 현 HEAD bare-symbol lexical-only 도구 4 tests passed, 실행 기록은 dirty 문서 진술; clean pinned/qualified pair `NOT_RUN` |
| [RBR-04](RBR-04-symbol-producer.md) | P1 코드+proof | unsupported admitted 파일의 path+SHA/skip reason·partial/full capability admission을 묶고, 5언어 hand oracle·combined replace/reopen을 한 revision에서 재실행 | 현재 unsupported 파일은 count만 남기고 skip해 계약 불일치; 현 storage/SDK receipt `NOT_RUN` |
| [RBR-05](RBR-05-symbol-route-proof.md) | P1 proof | live symbol route, forged/stale/no-answer/timeout, span identity를 RBR-06 출력과 연동 | 현 SDK receipt `NOT_RUN` |
| [RBR-06](RBR-06-span-chunking.md) | P1 코드+실험 | indexed/SDK/scored span 분리; rank-only Hit@1/MRR·exact-index-span Recall@10·context; fixed matrix | dirty chunking 25 passed는 부분 진단; end-to-end 지표/외부 matrix `NOT_RUN` |
| [RBR-07](RBR-07-semantic-parity.md) | P1 코드+proof+실험 | pinned asset/256차원 parity 및 외부 per-query exact-vs-served delta; T15 raw vector·source/model receipt binding | 현 Rust/semantic/asset receipt와 외부 분해 `NOT_RUN`; 조건부 T15 summary-only verdict `FAILED` |
| [RBR-08](RBR-08-symbol-ranking.md) | P2 조건부 | 오순위 진입 probe 후 한 후보 또는 유지 결정 | probe/ranker/효과 `NOT_RUN`; 무근거 기본값 변경 금지 |
| [RBR-09](RBR-09-query-performance.md) | P2 조건부 | RBR-01 후 fetch 100/25/50 × k/filter 및 ANN guard·quiet-host p95 | stage/matrix/효과 `NOT_RUN`; floor 100 유지 |
| [RBR-10](RBR-10-ingest-performance.md) | P1 계측, P2 조건부 최적화 | 내부 `build_stream_reported`의 공개 caller 연결; fresh/delta 원본·row-set·fault/restart; 비용 확인 시 한 최적화 후보 | 내부 report 존재만 확인; 공개 stage/비용/효과 `NOT_RUN` |
| [RBR-11](RBR-11-resource-accounting.md) | P1 proof, P2 한계 | 지원 플랫폼 owner proof와 clean contract/resource replay; ps PID 재사용 identity 미보장 한계 명시 | 현 HEAD 선택 process-tree rail 통과, inventory 272/272; 전체/clean receipt `NOT_RUN` |
| [RBR-12](RBR-12-evaluation-closeout.md) | P1 코드+통합 | cross-suite dev/holdout file·definition·family custody validator, frozen digest binding, T15/T16 raw proof, 단일 최종 조합 | 현 HEAD 단일-suite split 4 tests passed; cross-suite `NOT_RUN`, 조건부 T15/T16 summary-only verdict `FAILED`; final `PAIR_VALID`/`QUALITY_DELTA`/`PERF_QUALIFIED` `NOT_RUN` |

## 종결 순서와 판정 경계

1. source/dirty ownership을 고정한 뒤 RBR-04 coverage authority, RBR-06 span 계측, RBR-01/10 stage 연결, RBR-12 cross-suite custody와 T15/T16 raw proof를 producer→schema→validator→negative fixture까지 구현한다. 기존 v3 suite나 primary NDCG 의미를 소급 변경하지 않는다.
2. RBR-00 exact inventory·clean-source contract/SDK receipt와 RBR-02/03/04/05/07/11 owning rails를 **같은 revision**에서 발급한다. 국소 테스트 통과와 변경 전 receipt는 이 게이트를 대체하지 않는다.
3. RBR-08/09/10은 development raw 실험 후 변경 하나 또는 유지 결정을 기록한다. 효과가 불확실하면 기본값을 유지한다.
4. 외부 독립 gold/admission·고정 corpus/model/spec·quiet host가 갖춰질 때만 RBR-12 한 조합의 final holdout pair/replay 및 품질·성능 자격을 판정한다. 이는 수동 승인 작업 티켓이 아니라 qualified claim의 필수 입력이다. 없으면 세 final claim은 `NOT_RUN`이다.

2026-09-26 이전 단계별 `OPEN`/`DONE` 스냅샷과 수행 기록은 Git 역사 및 [AUDIT.md](AUDIT.md)에 남는다. 이 표가 현 소스의 우선 상태다.
