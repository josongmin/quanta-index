# Final DoD — 원래 합의한 전체 범위

상태/진척은 [8개 에픽 SSOT](README.md#epics)만 갱신한다. 이 문서는 원래 [RFC R01–R40](../../rfc/oct-10-native-incremental-vnext.md#23-fixed-full-product-implementation-and-acceptance-ledger)와 수용 계약을 고정한 추적표다. 실행 순서는 [W0–W6](WAVES.md#wave-map)이며, 에픽/웨이브 재편은 scope 축소·새 기능 분모·실행 성공이 아니다.

<a id="final-dod"></a>
## Final DoD

아래 조건을 모두 충족해야 **원래 하기로 한 Quanta Index 전체 범위 완료**다.

- [ ] E01–E08의 모든 필수 DoD와 아래 R01–R40이 actual producer/caller/consumer에서 충족되고, 각 주 에픽에 source-bound independent oracle가 있다.
- [ ] 실제 FULL/delta/delete/rename/recreate/no-op/Resync·complete affected-domain·required side output·RepoMap/lexical/semantic/public SDK·동일 operation 복구가 동작한다.
- [ ] single Catalog CAS·canonical wire·원 Original/예산·거절 before-birth/write0·partial/typed first+secondary cause·highest Source/backing-last·최종 drop 환급을 지킨다.
- [ ] first-FULL3/Facts2의 5 durable cut·stale/foreign/CAS·cancel/unwind/panic·old pins·restart·ENOSPC/short-write/fsync/checked-close·bounded cleanup을 실제 owner/process에서 수용한다.
- [ ] 원4K·Large/XL·초기 구축/증분/N-H-M/history/fanout·mixed/Off-Record·운영/총자원·유지보수와 RFC의 고정 성능/품질 profile을 충족한다. 작은 회귀·경합 진단·success-only latency로 대체하지 않는다.
- [ ] 실제 client/Runtime/Index의 필수 IDE workflow, P2I01–08·UX01–12와 적용되는 AUTO01–10, 실제 IntelliJ 동일 capability/품질/환경 비교가 충족된다.
- [ ] 통합 A/pre-switch→E06 default 전환→통합 B/post-switch·current full-suite/strict·공개 API/module·exact-source hosted CI/release를 완료한다.
- [ ] 설치 producer/consumer pair·worker/provider provenance·실제 운영 target/action/독립 observer의 필수 입력과 판정이 닫힌다. CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN을 혼합하지 않는다.
- [ ] 필수 FAILED/BLOCKED/NOT_RUN·누락된 입력/시나리오·충돌하는 owner0이며 SSOT/검증 요약/사용 문서가 최종 source와 일치한다.

### Scope and evidence rules

1. 원래 필수 기능을 새로 unsupported/N/A로 선언해서 닫지 않는다. 조건부 AUTO model evaluator/live reload/production shadow 등 원 계약이 허용한 absent capability의 하위 사례만 근거 있는 NOT_APPLICABLE이다. baseline admission/identity/invalid proposal·test-only 정책 교체는 계속 필수다. AUTO10 tuning 개선 주장은 실제 비교가 있어야 한다.
2. 기존 사용자 유예와 외부 환경/권한 부재는 실행 시점 유예 또는 BLOCKED다. 이 최종 종료 판정의 면제가 아니다. 영구 scope 변경은 사용자의 명시 변경이 있어야 한다.
3. source 적용·정적·컴파일·실행 본문·제품·성능·운영·hosted CI/출시를 각각 판정한다. VERIFIED는 명시 source/input/profile/scope에만 귀속한다. skipped/ignored/zero-body/partial/inconclusive는 성공이 아니다.
4. 기존 A semantic compatibility·D RepoMap lifetime·C5 generation reservation의 VERIFIED 범위와 완료된 bank/engine은 재사용한다. 관련 source/입력 변경 영향만 좁게 재검증하고 전체 미수용을 이유로 재구현하지 않는다.
5. Semantica의 전체 PTA/DFG/Type 캠페인을 Index 작업으로 복제하지 않는다. Index가 실제 소비하는 source/generation/affected-output/lower failure 계약은 기존 canonical owner에게 있고 그 결과의 접합/수용은 필수다.
6. profile/hardware/backend/provider·sample 수·품질 gold·latency/자원 상한은 측정 전에 고정한다. 결과를 보고 예산/시나리오/분모/합격선을 바꾸지 않는다. unset profile·baseline/client workflow 부재는 해당 성능/IntelliJ claim을 닫지 못한다.

<a id="requirements"></a>
## R01–R40 — 각 요구의 주 에픽 하나

아래 원문 요구사항은 RFC 23절의 고정 inventory다. 다른 에픽은 같은 결과를 소비하며 oracle/구현을 복제하지 않는다. 현재 완료 상태를 이 표에 따로 기록하지 않는다.

| ID | 원 요구사항 / exit condition | 주 에픽 |
| --- | --- | --- |
| R01 | One canonical paged source/projection/request IR; complete EOF/count/digest rejection | [E02](E02-publication-recovery.md) |
| R02 | Typed extensible adapters; multiple instances and profile capability refusal | [E02](E02-publication-recovery.md) |
| R03 | Bounded native filesystem capture and child-first ingress batches beyond old direct-child limit | [E01](E01-producer-generation.md) |
| R04 | Owner/path/frontier atomic transitions, cross-page rename cycles and original-base CAS | [E01](E01-producer-generation.md) |
| R05 | Persistent COW maps, streamed complete validation, bounded scratch construction and root-only publication | [E02](E02-publication-recovery.md) |
| R06 | Reserved-before-write object custody and bounded durable batch barriers on supported OS | [E02](E02-publication-recovery.md) |
| R07 | Sole Catalog CAS publication of roots, event bindings, work and original terminal receipt | [E02](E02-publication-recovery.md) |
| R08 | Replay, ACK-loss reconciliation, resume and immutable old-view/reopen correctness | [E02](E02-publication-recovery.md) |
| R09 | Session-fenced renewable read custody; real guard validation, process-death recovery and safe expiry | [E02](E02-publication-recovery.md) |
| R10 | Retention floors, terminal custody release, bounded DAG reclaim and physical incarnation/adoption fencing | [E02](E02-publication-recovery.md) |
| R11 | Shared live RAM/disk/WAL/provider reservations, draining charges and enforced aggregate limits | [E02](E02-publication-recovery.md) |
| R12 | Service-owned scratch, crash cleanup under custody and enforcement including secondary spill | [E02](E02-publication-recovery.md) |
| R13 | Existing waterfall P00-P09 integration, failure-surviving reopen stages and observer-cost isolation | [E07](E07-performance-operations.md) |
| R14 | Native incremental byte-literal indexing and exact current/old-view query | [E03](E03-lexical-construction.md) |
| R15 | Required token/BM25/regex domain parity with independent ranking/query gold | [E03](E03-lexical-construction.md) |
| R16 | Bounded cold bulk-build, reopen and catch-up; initial-index algorithm/parallel preparation experiments | [E03](E03-lexical-construction.md) |
| R17 | Sustainable lexical churn/maintenance and bounded write amplification/debt | [E03](E03-lexical-construction.md) |
| R18 | Semantic desired/covered/missing ownership, certified projection delta, stale job/delete fencing | [E04](E04-semantic-ann.md) |
| R19 | Model/recipe/input provenance and durable unchanged-owner/ABA artifact reuse | [E04](E04-semantic-ann.md) |
| R20 | Bounded small-scope exact vector materialization/search with exact coverage and cosine gold | [E04](E04-semantic-ann.md) |
| R21 | Concrete chunking/ANN feasibility and incremental searchable membership | [E04](E04-semantic-ann.md) |
| R22 | Sustained ANN recall/query/update/filter/history retention on pinned roots | [E04](E04-semantic-ann.md) |
| R23 | Native file inventory/path delta with one Catalog view and exact pagination | [E01](E01-producer-generation.md) |
| R24 | Provenance graph RepoMap delta and full query parity | [E01](E01-producer-generation.md) |
| R25 | Editor overlay/mask/isolation without durable publication per keystroke | [E05](E05-ide-auto.md) |
| R26 | Save ACK and session-frontier handoff with exact source/action identity | [E05](E05-ide-auto.md) |
| R27 | Required Stub index and language capability refusal | [E05](E05-ide-auto.md) |
| R28 | Progressive query, exact coordinates/AST/action fencing and fixed intent budget | [E05](E05-ide-auto.md) |
| R29 | Replaceable bounded Auto policy, estimator/governor separation, stale/unknown signal semantics | [E05](E05-ide-auto.md) |
| R30 | Live same-intent Fast/Deep execution, scheduler permits and dispatch/completion rechecks | [E05](E05-ide-auto.md) |
| R31 | Coherent live worker queue/active/lag snapshots, TTL, cancellation and shutdown cleanup | [E06](E06-sdk-service-cutover.md) |
| R32 | Owned background work pump, fair Core/Enrichment/Maintenance service and provider-bounded execution | [E06](E06-sdk-service-cutover.md) |
| R33 | Real opt-in native source/literal/path UDS, exact selection and SDK reply reconciliation | [E06](E06-sdk-service-cutover.md) |
| R34 | Configured semantic provider/query transport with correct coverage/freshness | [E06](E06-sdk-service-cutover.md) |
| R35 | Default daemon/CLI/launcher product routing and service shutdown/reopen | [E06](E06-sdk-service-cutover.md) |
| R36 | Backup/restore, new-format cutover and retirement of competing legacy lifetime/head paths | [E06](E06-sdk-service-cutover.md) |
| R37 | Real Semantica complete affected-domain generation, durable outbox and required side outputs | [E01](E01-producer-generation.md) |
| R38 | P2I01-P2I08 through real producer/consumer calls and independent semantic outputs | [E01](E01-producer-generation.md) |
| R39 | Full fault/operational matrix plus fixed-delta N/history/fanout, initial-build and mixed-load cost gates | [E07](E07-performance-operations.md) |
| R40 | Full suite/new CI selection/release and UX01-UX12/AUTO01-AUTO10 matched IntelliJ acceptance | [E08](E08-integration-release.md) |

## Original supplementary oracles

| 원 수용 계약 | 기능/코드 owner | 최종 수용 |
| --- | --- | --- |
| P2I01–P2I08: dependency/affected closure·side output·partial/rebuild·old-view/bounds | E01/E02와 실제 lower canonical owner | E08이 같은 결과 소비 |
| UX01–UX12: edit/cancel/revision·Fast/Deep·fairness/readiness·coordinates/action·cold/reopen | E05; E01/E02/E06/E07의 실제 결과 사용 | E08의 실제 client/IntelliJ와 결합 |
| AUTO01–AUTO10: 두 정책 conformance·invalid/stale/unknown·ledger/progress·bounded evaluator/config/shadow/실험 | E05; RFC 13.7절 조건부 범위 유지 | E08이 필수/근거 있는 N/A를 대조 |
| 초기 indexing/증분 SLO·quality·work bounds | E03/E04/E07, RFC 12.1–12.5절 | E08에서 동일 profile로 AND 판정 |
| p03–p11 installed/provider/supervisor/readiness/migration/pair/실제 운영 action | E02/E06와 기존 proof registry/recipe owner | E08에서 source/binary/input/observer 결속 |
| A/D/C5 및 legacy LQ5/semantic/test-platform의 잔여 수용 | 기존 owner의 source별 영수증 재사용 | E08에서 빠짐 없이 대조 |

### Existing profile separation

- ANN 원4K:4096×64/Q64/K10/recall≥.95·원1M reads/512MiB·SDK30s/worker10s. watchdog/새 status 관측은 원 요청을 연장하지 않는다.
- Large/XL 기능과 Linux2CPU/6GiB Large 운영·원960초 mixed·Off/Record≥3쌍은 E07의 서로 다른 gate다.
- native/IDE RFC profile의 100K files/≤1GiB·8 performance cores/≥32GiB host·Index RSS8GiB 등은 위 운영 profile과 섞지 않는다. 원 RFC 12.3/12.5절 absolute gates와 actual IntelliJ 비교를 함께 수용한다.
- profile별 latency/quality/served rate·actual RSS/allocated disk·queue/retry/provider/maintenance/observer 비용을 모두 포함한다. 이 문서의 목표치는 관측 성능이나 보장이 아니다.

## Previous work mapping

| 이전 ID | 현재 주 에픽 |
| --- | --- |
| W2-01 | [E02](E02-publication-recovery.md) |
| W2-02 | [E02](E02-publication-recovery.md) |
| W2-03 | [E02](E02-publication-recovery.md) |
| W2-04 | [E02](E02-publication-recovery.md) |
| W2-05 | [E01](E01-producer-generation.md); 게시/terminal·Wire는 E02 |
| W2-06 | [E01](E01-producer-generation.md) |
| W2-07 | [E02](E02-publication-recovery.md) |
| W2-08 | [E06](E06-sdk-service-cutover.md) |
| W2-09 | [E07](E07-performance-operations.md) |
| W2-10 | [E07](E07-performance-operations.md) |
| W2-11 | [E03](E03-lexical-construction.md) |
| W2-12 | [E01](E01-producer-generation.md) |
| W2-13 | [E04](E04-semantic-ann.md) |
| W2-14 | [E01](E01-producer-generation.md) |
| W2-15 | [E05](E05-ide-auto.md) |
| W2-16 | [E05](E05-ide-auto.md) |
| W2-17 | [E05](E05-ide-auto.md) |
| W2-18 | [E06](E06-sdk-service-cutover.md) |
| W2-19 | [E08](E08-integration-release.md) |

이전 고정 계약/완료/실패/미실행은 [마이그레이션 보존 색인](../../ARCHIVE-INDEX.md#oct-22-finalizing-migration)에 있다. 과거 상태나 오래된 다음 명령은 현재 작업을 새로 발급하지 않는다.
