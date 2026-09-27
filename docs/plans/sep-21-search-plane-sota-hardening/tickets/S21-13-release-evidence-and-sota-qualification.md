# S21-13 — Remaining release evidence and qualification

Status: `ACTIVE — infrastructure exists; final qualification remains open`.
Completed proof parsing/custody/selection decisions are in
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
Trusted host, authentic producer, complete recipe and final source-bound execution
remain [R0/R6-owned](FINAL-RESIDUAL-EXECUTION-PLAN.md). Historical P12A
checks and numerical counts are recoverable in Git; they are not issued receipts.

Depends on: phase A depends on S21-00; phase B depends on S21-01 through S21-12

## Goal

모든 구조 변경을 동일 final source에서 검증하고, artifact absence/staleness가 green이 될 수 없는 blocking
release proof graph를 만든다.

## Proof manifest

각 proof node는 다음을 필수로 가진다.

- proof ID and required evidence class `S/U/A/D/P/F/Q/X`
- exact 40-char HEAD, dirty digest, branch/upstream/merge-base
- command/profile/target/filter and selected/executed/passed/failed/ignored counts
- toolchain/features/OS/arch/host CPU-memory
- daemon binary SHA-256 and state-root format
- fixture/corpus/config/model/provider digest
- started/ended timestamps and raw log/artifact paths
- terminal status; missing field/failed prerequisite는 success 금지

## Mandatory proof families

Canonical family IDs and semantics are owned by `tools/ci/proof-authority.toml`: `S/U/A/D/P/F/Q/X`. Coverage such as
migration, provider and cross-repo cutover is represented by registered proof nodes in those families, not by a second
human-maintained family enum. Final qualification requires every registered release proof and its dependency DAG.

## Threshold freeze

S21-00에서 baseline과 target을 같은 corpus/host/profile로 고정한다. 구현 후 유리한 metric만 선택하지
않는다. 최소한 다음을 분리 측정한다.

- authority correctness: collision, mixed generation, replay, recovery failures = 0
- completeness: exact/partial/capped truth and ANN recall
- relevance: nDCG/MRR/Recall@K on held-out judgments
- latency: p50/p95/p99 plus offered/accepted/completed QPS and error rate
- resources: peak RSS, FD/thread/request count, disk/WAL, queue depth, drain duration
- operations: recovery time, restore verification time, GC progress and reclaim
- provider: request count, residual tasks, tokens/usage/cost, model identity

## Final aggregate

Phase B는 독립적인 P12A infrastructure proof와 최종 aggregate qualification으로 나뉜다.
P12A는 Quanta exact-source와 원시 Python 실행 증거로 발급할 수 있다. Aggregate는
P11 exact-pair proof, P12A proof, 전체 final-source proof 및 운영 동작을
검증한 뒤에만 최종 verdict를 발급한다.

Code/process proofs retain their registered Linux host-profile requirement but
may run on distinct host instances. Deployment, activation and rollback must
share one exact operational host identity, and the aggregate binds to that host.
The manual CI checkout under `.proof-pairs/` is excluded from Quanta's primary
source dirty state; its Semantica source and lockfile are checked separately.

- final clean source에서 동일 release daemon binary를 모든 process/cross-repo proof에 사용한다.
- mandatory family 전부와 deployment/activation/rollback evidence를 aggregate하고 누락/실패/skipped/stale를
  success로 계산하지 않는다.
- verdict를 `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`으로 분리한다.
- registered `AggregateQualificationReceiptV2` schema, registry-derived writer, validator and canonical final Just
  recipe가 dependency manifest validation → four-verdict calculation → atomic aggregate artifact publication →
  current-source release-gate validation을 수행한다. Aggregate 자체가 최종 릴리즈 receipt다.
- aggregate receipt는 exact source pair, attested binary, operational host와 모든 mandatory artifact digest를 검증한다.
  과거 handoff 체인은 별도 역사적 감사 도구로 검증하며 릴리즈 자격의 필수 입력이 아니다.

## Owner files

- `tools/ci/test-authority.toml`
- `tools/ci/lint/check-test-authority.py`
- `tools/ci/lint/check-bench-artifacts.py`
- verification receipt writer/schema
- aggregate qualification receipt schema/writer/validator and canonical producer recipe
- `.github/workflows/ci.yml`
- `.github/workflows/correctness.yml`
- `Justfile`
- `tools/benchmark/`
- purpose validation checklist and final closeout reports
- `tools/ci/proof-authority.toml` (new canonical registry; exact name is S21-00 decision)

## Canonical verification escalation

- contract/SDK: `just rust-public-api`
- wire/decoder: `just rust-fuzz-smoke`
- crate/module boundaries: `just rust-hexagonal` and `just rust-cargo-modules`
- owner loop: `just rust-profile test-fast` plus owning integration targets
- storage/semantic: `just rust-profile test-integration-semantic`
- activation/pin/state root: `just rust-profile test-daemon`
- exhaustive runtime: `just rust-profile test-daemon-all`
- quality: `just rust-verify-quality-all` plus strict artifact validation
- full closeout: `just rust-profile verify-rust` plus external/migration/provider rails

실제 구현 시 profile catalog가 새 target을 포함하는지 먼저 검증한다. profile 이름만으로 coverage를
추정하지 않는다.

## Adversarial closeout matrix

- artifact missing, stale HEAD, dirty mismatch, wrong binary
- selected/executed 0, ignored-only, early-return fixture
- duplicate proof ID, missing required family, report-only job
- composite test scope가 child scope의 더 엄격한 thread cap을 넓힘
- fake/hash provider substituted for required real-provider proof
- in-process harness substituted for process crash proof
- macOS result substituted for Linux production performance/credential proof
- restored state without verified manifest
- external producer checkout mismatch

## Acceptance

- mandatory artifact absence is failure
- all proof nodes refer to one final clean source pair or explicitly scoped delta proof
- no failed/skipped/missing node can produce `PURPOSE_GREEN` or `PRODUCTION_READY`
- independent quality oracle does not derive expected output from SUT output
- checklist mandatory P0/P1 rows are all `PASS`; runtime/external rows are not static-pass
- final report separates code completion, test proof, deployment, and activation
- final recipe가 새 aggregate receipt를 생성하고 현재 source binding과 dependency digest를 재검증한다.

## Final release gate

`PRODUCTION_READY`는 다음을 모두 만족할 때만 허용한다.

1. M0-M4 merge units complete
2. legacy live paths removed
3. full proof manifest validates
4. migration and rollback drill passes
5. cross-repo terminal receipt passes
6. Linux process/performance proof passes
7. required real-provider relevance/egress proof passes
8. unresolved P0/P1, `NOT_RUN`, `BLOCKED` = 0

추가 mandatory proof inventory:

- 등록된 J7Q/live owner target 전수(실제 collection과 현재 authority 기준)
- ANN recall/relevance와 latency/resource를 분리한 fixed-corpus proof
- activation/read-view/GC concurrency 및 state-root two-process lease proof
- provider spy cancellation/egress matrix와 opt-in real-provider budgeted proof
- migration/backup/restore/rollback 및 cross-repo producer integration
- self-hosted production-like Linux runner의 pinned host identity; GitHub label 문자열만으로 host proof를 대체하지 않음

## No patch-on-patch rule

artifact schema 추가만으로 닫지 않는다. validator, blocking workflow, test authority, external rail,
final verdict computation을 하나의 proof graph로 연결한다.
