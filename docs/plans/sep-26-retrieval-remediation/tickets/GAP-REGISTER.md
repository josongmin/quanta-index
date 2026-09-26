# SEP-26 RBR 잔여 공백 등록부 — 2026-09-26

최신 코드 감사: `604149ed3f6033e24a834ebaa86a596f7b8ed82d` + 공유 dirty, 2026-09-26. 이 표는 **잔여 작업만** 기록한다. 이전 수행 수치는 [CURRENT-AUDIT](CURRENT-AUDIT.md)의 역사 기록이며 현 qualification이 아니다. raw 최신 root는 `/private/tmp/qi-rbr-source-audit.p3VfSl/`이다.

**확정 코드 작업:** RBR-07 parity fixture fail-open 수정, RBR-01 실제 server observation-off rail, RBR-10 internal ingest report의 public transient propagation. **조건부 코드 작업:** same-model/incremental claim을 열 때 RBR-12 T15/T16 raw producer·실행 custody; fetch matrix를 위한 RBR-09 bounded experimental variant. **미실행 실험/재자격:** RBR-06/07 matrix·원인 분해, RBR-08 진입 probe, RBR-09/10 비용·효과 및 최종 source-bound inventory/SDK/contract proof.

이미 구현된 RBR-04 strict-full coverage, RBR-06 indexed/SDK/scored span 분리, RBR-12 admission v2 cross-suite custody, 세 query route stage DTO→SDK→diagnostic v4는 **코드 재구현 목록에서 제외**한다. 단, 현 revision의 integration/qualification proof는 별도로 필요하다. 수동 승인·심사자·host 확보는 engineering 작업에 넣지 않는다.

| ID | 우선순위 | 남은 코드/결정 | 현행 증거·종결 게이트 |
| --- | --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | P1 proof | 최종 source/document freeze 후 Python/Rust/SDK exact collection 및 계약 closure 재자격 | 최신 Python 276 passed/32 subtests·276/276 exact collection local 통과; Rust/SDK current collection·clean-source contract/SDK receipt `NOT_RUN` |
| [RBR-01](RBR-01-diagnostics.md) | P1 코드+proof | 실제 server observation-off rail·config/binary binding 및 동일 workload overhead; fresh SDK receipt | 현재 query stage는 항상 계측; runner sidecar off는 server off 아님. ingest는 RBR-10 소유. overhead·현 clean/live receipt `NOT_RUN` |
| [RBR-02](RBR-02-query-policy.md) | P1 proof | 정책/identity/negative fixture 최종 revision 재실행 | current-source SDK/receipt `NOT_RUN`; native 제품 정책은 변경하지 않음 |
| [RBR-03](RBR-03-semble-profiles.md) | P1 proof | pinned reference capture·phase event·raw hash 최종 revision 재발급 | 네 profile/phase validator 구현; 과거 lexical 5 tests·탐색 pair는 역사적 local 기록. 이번 pinned recapture·clean/qualified proof `NOT_RUN` |
| [RBR-04](RBR-04-symbol-producer.md) | P1 proof | strict-full coverage/5언어 hand oracle·combined replace/reopen을 고정 revision에서 재실행 | failure path/SHA typed abort와 파일별 coverage→corpus replay 구현; Rust lib 82/live SDK 16은 과거 local 기록. 현 clean receipt `NOT_RUN` |
| [RBR-05](RBR-05-symbol-route-proof.md) | P1 proof | live symbol route, forged/stale/no-answer/timeout, span identity를 RBR-06 출력과 연동 | 현 SDK receipt `NOT_RUN` |
| [RBR-06](RBR-06-span-chunking.md) | P1 실험+proof | indexed-span 보조 지표의 live producer→merge→report/SDK 재증명과 fixed matrix | span/rank/context 구현; Rust lib 82/chunking 25는 과거 local 기록. 최신 Python 계약 local 통과; 현 live/clean·외부 matrix `NOT_RUN` |
| [RBR-07](RBR-07-semantic-parity.md) | **P0 proof-integrity**+실험 | strict typed fixture validator·asset-free omission/subset/norm/triangle 음성 tests → 실제 asset 재검증 → 외부 exact-vs-served delta | actual pinned baseline 1 passed지만 invalid fixture 7종도 통과: **거부 계약 FAILED**. 완전한 parity 증거 승인만 차단; 제품 모델/ANN 결함은 미확정. 외부 delta/clean/T15 양성 proof `NOT_RUN` |
| [RBR-08](RBR-08-symbol-ranking.md) | P2 조건부 | 오순위 진입 probe 후 한 후보 또는 유지 결정 | probe/ranker/효과 `NOT_RUN`; 무근거 기본값 변경 금지 |
| [RBR-09](RBR-09-query-performance.md) | P2 조건부 | bounded experimental floor variant 후 100/25/50 × k/filter 및 ANN guard·quiet-host p95 | query stage는 구현; matrix/효과/quiet-host 자격 `NOT_RUN`; production floor 100 유지 |
| [RBR-10](RBR-10-ingest-performance.md) | P1 계측, P2 조건부 최적화 | 내부 `build_stream_reported`의 공개 caller 연결; fresh/delta 원본·row-set·fault/restart; 비용 확인 시 한 최적화 후보 | 내부 report 4 tests는 과거 local 기록; 공개 stage/비용/효과 `NOT_RUN` |
| [RBR-11](RBR-11-resource-accounting.md) | P1 proof, P2 한계 | 지원 플랫폼 owner proof와 clean contract/resource replay; ps PID 재사용 identity 미보장 한계 명시 | 최신 Python 전체 276 passed/32 subtests·exact inventory local 통과; platform/clean receipt `NOT_RUN` |
| [RBR-12](RBR-12-evaluation-closeout.md) | P1 통합, 조건부 코드 | same-model/incremental claim을 열 경우 T15/T16 raw vector/row-set producer·validator·terminal receipt; custody의 live frozen proof; 단일 최종 조합 | admission v2 custody 구현, summary-only false positive 차단. claim=false는 `NOT_APPLICABLE`; true면 현재 양성 protocol 미구현으로 거부. qualified final `PAIR_VALID`/quality/perf `NOT_RUN` |

## 종결 순서와 판정 경계

1. 재현된 RBR-07 validator 결함을 먼저 수정한다. 이어 RBR-01 실제 off rail과 RBR-10 public transient stage를 producer→schema→SDK→replay→negative fixture까지 연결한다. T15/T16을 주장할 경우에만 RBR-12 양성 protocol을 완성한다. 기존 v3 suite/primary NDCG 의미는 소급 변경하지 않는다.
2. development에서 RBR-06 fixed chunking matrix·RBR-07 외부 exact 분해·RBR-08 오순위 probe·RBR-09 fetch frontier·RBR-10 fresh/delta 비용을 측정한다. RBR-08/09/10은 한 후보 또는 근거 있는 유지 결정; 효과가 불확실하면 기본값을 유지한다.
3. 최종 코드·계약 문서를 고정하고 RBR-00 Python/Rust/SDK exact inventory·clean-source contract/SDK receipt와 RBR-02/03/04/05/07/11 owning rails를 **같은 revision**에서 발급한다. 문서 변경도 source closure 입력이다. dirty local 실행은 허용하되 qualification으로 변환하지 않는다.
4. 외부 `frozen-v4/corpus-set.json`은 SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33` 재확인, 10 repo/1,480파일·`candidate_not_admitted_no_gold_no_pair`다. 독립 gold/admission·holdout custody·고정 model/spec·quiet host 입력이 충족될 때만 한 최종 조합의 final holdout pair/replay를 판정한다. 이는 수동 작업 티켓이 아닌 qualified claim 입력 조건이며 현재 세 final claim은 `NOT_RUN`이다.

2026-09-26 이전 단계별 `OPEN`/`DONE` 스냅샷과 수행 기록은 Git 역사 및 [AUDIT.md](AUDIT.md)에 남는다. 이 표가 현 소스의 우선 상태다.
