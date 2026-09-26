# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

최신 보정: [CURRENT-AUDIT](CURRENT-AUDIT.md)의 `79bb8d23` 공유 dirty local에서 이전 fmt/scoped Clippy 실패를 수선해 두 명령은 exit 0, contract/IPC/SDK/search-plane lib 및 LXE 통합 테스트도 local 통과했다. 이전 문단의 `FAILED`는 당시 소스 스냅샷의 역사적 결과다. **남은 기능/자격**은 이 표 그대로다: RBR-01 on/off overhead, RBR-10 ingest stage 공개 연결, RBR-12 T15/T16 독립 raw 양성 proof, RBR-06 fixed matrix와 frozen-source 재자격, 외부 gold/admission/quiet-host final pair. 조건부 RBR-08/09/10 정책은 개발 실측 전 변경하지 않는다.

2026-09-26 `f16bad93` 기반 dirty overlay 재감사. RBR-04 strict-full 실패/성공 coverage, RBR-06 indexed-span record/merge/evaluator, RBR-12 admission v2 cross-suite custody는 코드에 반영했다. T15/T16의 summary-only false positive는 닫았으나 raw 양성 proof는 아직 없다. 이 단계의 Python 전체 **274 passed/32 subtests**, Rust lib 82/chunking 25, live SDK 16, Python/Rust/SDK inventory는 lexical stage 추가 전 공유 dirty source의 역사적 local diagnostic이다. clean-source receipt와 **qualified final pair**는 자격 판정이 없다. 티켓별 계약과 증거 한계는 [CURRENT-AUDIT.md](CURRENT-AUDIT.md), [RBR-00~12](INDEX.md), [TEST-PLAN.md](TEST-PLAN.md)을 따른다. 아래 상태는 **남은 작업**이다.

후속 dirty 변경: RBR-01 lexical/semantic/hybrid server stage DTO→SDK→diagnostic v4 및 replay guard를 코드에 추가했다. lexical 추가 후 contract IPC 55, search-plane 413, retrieval 83, searchctl 42 local tests와 실제 SDK 통합 16/16이 통과했다. raw `/private/tmp/rbr01-lexical-final.B3GlSv/`의 3-route replay와 6종 변조 거부는 [RBR-01](RBR-01-diagnostics.md)에 묶었다. Python 전체 두 실행은 각각 273 passed/1 failed·32 subtests이며 동시 canonical receipt schema 확장에 의한 한 필드 drift를 단계마다 수정하고 집중 테스트를 통과시켰다. 최신 schema bytes로 전체 재실행은 `NOT_RUN`. ingest stage와 on/off overhead, clean receipt는 미완료다.

추가 local 재검증: diagnostic v4의 stage↔engine/strategy 모순 거부를 보강했고 보존된 실제 3-route sidecar에서 9개 필드 변조를 거부했다. Python 전체 **275 passed/32 subtests**(exit 0)이나 실행 중 HEAD·test bytes가 이동해 source-stable receipt가 아니다. 현 bytes의 관련 6 tests, required inventory 275/275, Ruff는 통과했다. Rust search-plane 414/retrieval 83과 semantic 내부 ingest report 4는 local 통과. Clippy/fmt 전체는 별도 control/embed/semantic 파일 문제로 `FAILED`. 현재 명세와 코드 판정은 [CURRENT-AUDIT](CURRENT-AUDIT.md)를 우선한다.

| ID | 우선순위 | 남은 코드/결정 | 현행 증거·종결 게이트 |
| --- | --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | P1 proof | 최종 source freeze 후 Python/Rust/SDK exact collection 및 계약 closure 재자격 | Python exact collection과 Rust collection local 통과; clean-source contract/SDK receipt `NOT_RUN` |
| [RBR-01](RBR-01-diagnostics.md) | P1 코드+proof | ingest stage, diagnostic on/off overhead 및 frozen-source receipt | 세 query route stage·실제 SDK 16/16·raw 3-route replay local 확인; overhead·clean receipt `NOT_RUN` |
| [RBR-02](RBR-02-query-policy.md) | P1 proof | 정책/identity/negative fixture 최종 revision 재실행 | current-source SDK/receipt `NOT_RUN`; native 제품 정책은 변경하지 않음 |
| [RBR-03](RBR-03-semble-profiles.md) | P1 proof | pinned reference capture·phase event·raw hash 최종 revision 재발급 | 새 dirty lexical latency 코드 5 tests passed(focused local); GIN/ripgrep v4 영수증 digest 일치·탐색 `PAIR_VALID=pass`는 현 clean pinned/qualified pair proof 아님 |
| [RBR-04](RBR-04-symbol-producer.md) | P1 proof | strict-full coverage/5언어 hand oracle·combined replace/reopen을 고정 revision에서 재실행 | failure path/SHA typed abort와 success-side 파일별 coverage→corpus replay 구현; Rust lib 82/live SDK 16 local, clean receipt `NOT_RUN` |
| [RBR-05](RBR-05-symbol-route-proof.md) | P1 proof | live symbol route, forged/stale/no-answer/timeout, span identity를 RBR-06 출력과 연동 | 현 SDK receipt `NOT_RUN` |
| [RBR-06](RBR-06-span-chunking.md) | P1 실험+proof | indexed-span 보조 지표의 live producer→merge→report/SDK 재증명과 fixed matrix | span 계약·rank-only 지표·context 계산 구현, Rust lib 82/chunking 25 및 Python 집중 fixture local 통과; 외부 matrix `NOT_RUN` |
| [RBR-07](RBR-07-semantic-parity.md) | P1 코드+proof+실험 | pinned asset/256차원 parity 및 외부 per-query exact-vs-served delta; T15 raw vector·source/model receipt binding | 현 Rust/semantic/asset receipt와 외부 분해 `NOT_RUN`; summary-only 승인 차단, 양성 T15 raw protocol `NOT_RUN` |
| [RBR-08](RBR-08-symbol-ranking.md) | P2 조건부 | 오순위 진입 probe 후 한 후보 또는 유지 결정 | probe/ranker/효과 `NOT_RUN`; 무근거 기본값 변경 금지 |
| [RBR-09](RBR-09-query-performance.md) | P2 조건부 | RBR-01 후 fetch 100/25/50 × k/filter 및 ANN guard·quiet-host p95 | stage/matrix/효과 `NOT_RUN`; floor 100 유지 |
| [RBR-10](RBR-10-ingest-performance.md) | P1 계측, P2 조건부 최적화 | 내부 `build_stream_reported`의 공개 caller 연결; fresh/delta 원본·row-set·fault/restart; 비용 확인 시 한 최적화 후보 | 내부 report 존재만 확인; 공개 stage/비용/효과 `NOT_RUN` |
| [RBR-11](RBR-11-resource-accounting.md) | P1 proof, P2 한계 | 지원 플랫폼 owner proof와 clean contract/resource replay; ps PID 재사용 identity 미보장 한계 명시 | 최신 Python 전체 273 passed/1 schema drift fail·32 subtests; resource child fixture 오류 수정·집중 재통과, platform/clean receipt `NOT_RUN` |
| [RBR-12](RBR-12-evaluation-closeout.md) | P1 코드+통합 | T15/T16 raw vector/row-set producer·validator·execution receipt; custody의 live frozen proof; 단일 최종 조합 | admission v2 dev/holdout custody 및 재바인딩 부정 fixture local 통과, summary-only false positive 차단; qualified final `PAIR_VALID`/quality/perf 및 양성 T15/T16 `NOT_RUN` |

## 종결 순서와 판정 경계

1. 현 코드의 RBR-04/06/12 소유 경로를 통합 검증하고, RBR-01/10 stage 공개, RBR-12 T15/T16 raw 양성 proof를 producer→schema→validator→negative fixture까지 구현한다. 기존 v3 suite나 primary NDCG 의미를 소급 변경하지 않는다.
2. RBR-00 exact inventory·clean-source contract/SDK receipt와 RBR-02/03/04/05/07/11 owning rails를 **같은 revision**에서 발급한다. 국소 테스트 통과와 변경 전 receipt는 이 게이트를 대체하지 않는다.
3. RBR-08/09/10은 development raw 실험 후 변경 하나 또는 유지 결정을 기록한다. 효과가 불확실하면 기본값을 유지한다.
4. 외부 `frozen-v4/corpus-set.json` 후보(10 repo/1,480파일, 현 manifest SHA 재확인)는 이미 있으나 자체 상태가 `candidate_not_admitted_no_gold_no_pair`다. 독립 gold/admission·holdout split·고정 model/spec·quiet host가 갖춰질 때만 RBR-12 한 조합의 final holdout pair/replay 및 품질·성능 자격을 판정한다. 이는 수동 승인 작업 티켓이 아니라 qualified claim의 필수 입력이다. 없으면 세 final claim은 `NOT_RUN`이다.

2026-09-26 이전 단계별 `OPEN`/`DONE` 스냅샷과 수행 기록은 Git 역사 및 [AUDIT.md](AUDIT.md)에 남는다. 이 표가 현 소스의 우선 상태다.
