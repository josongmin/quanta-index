# S21-13 — Release Evidence and SOTA Qualification

Status: infrastructure implemented; final qualification blocked.

Phase A status: `done` for the M0 foundation gate. Registry-driven atomic manifest production, staged/unstaged/
Git-visible-untracked source binding, conditional non-binary semantics, exact-pair binding, blocking P00 validation
and the fail-closed aggregate release command are implemented. The P12A aggregate
schema, writer, validator, handoff-DAG checks, and final recipe are also present
in source. P12A's exact-pair receipt and the P12Q release receipt are absent;
P12Q remains staged in the registry. Code presence does not qualify the product
or the release.

Depends on: phase A depends on S21-00; phase B depends on S21-01 through S21-12

## Goal

모든 구조 변경을 동일 final source에서 검증하고, artifact absence/staleness가 green이 될 수 없는 blocking
release proof graph를 만든다.

## Root cause

- benchmark/quality artifact checker가 absence를 허용
- 일부 summary schema/boolean/source binding이 약함
- verification receipt가 full source/binary/host/feature 정보를 자체 검증하지 않음
- focused/static/in-process proof가 product closure로 승격될 수 있음
- deploy/backup/provider/cross-repo rail이 하나의 mandatory graph에 연결되지 않음

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

## Work items

1. strict proof-manifest schema and validator
2. `check-bench-artifacts --require` equivalent를 blocking workflow에 연결
3. mandatory family inventory에 ambiguity/snippet/ops/ui/integration 포함 여부를 owner 결정
4. strict boolean/schema/full HEAD/source binding
5. test-authority invariant universe에 S21 scenarios 등록
6. Justfile canonical profiles와 CI workflow 연결
7. receipt writer가 terminal success와 source metadata를 자체 검증
8. exact-source artifact store/publish policy
9. purpose checklist에 pass2/pass3 proposed rows 실제 반영
10. final closeout report와 unresolved risk ledger
11. composite local test scope는 포함된 scope 중 가장 엄격한 thread/resource cap을 적용
12. generated checklist/agent docs는 canonical source owner를 수정한 뒤 생성·lint

### Phase A — land early

- `proof-authority.toml` 또는 동등한 canonical registry에 proof ID, owner, family, command/profile,
  source-binding rule, required host, artifact schema를 선언한다.
- strict proof-manifest schema/validator, test-authority entries, CI workflow skeleton을 W0/W1에 배치한다.
- schema는 full 40-char SHA, dirty digest, selected/executed/passed/failed/ignored counts, exact binary SHA,
  features/toolchain/OS/arch/host, timestamps와 artifact digest를 mandatory로 한다.

Implemented owners:

- `tools/ci/proof-authority.toml`
- `tools/ci/proof-manifest.schema.json`
- `tools/ci/lint/check-proof-authority.py`
- `.pre-commit-config.yaml` `proof-authority` hook
- `.github/workflows/ci.yml` blocking proof-authority step

Closed Phase A owners:

- source digest semantics include staged, unstaged and Git-visible untracked bytes while excluding proof output
- `tools/ci/write-proof-manifest.py` atomically publishes registry-derived terminal receipts
- `binary_binding=none`, upstream-less/detached worktrees and non-passing terminal receipts are explicit
- PR CI blocks on the current P00 receipt; the explicit release gate uses `--require-all --bind-source`
- exact-pair release receipts bind normalized Semantica repository identity, Git state and `Cargo.lock`

Per-lane requirement, not Phase A closure: each P01-P11 owner and P12A infrastructure owner must register any new concrete `test-authority`
target before claiming its proof. Empty future target lists are not evidence and are not promoted by the M0 receipt.

### Phase B — final aggregate

Phase B는 `P12A → P12Q` 두 직렬 proof lane이다. P12A infrastructure는 구현돼 있지만
현재 exact-pair manifest가 없다. P11 source-pair dependency와 P12A handoff를
검증한 뒤에만 P12A receipt를 발급한다. P12Q는 전체 final-source proof를
실행·수집하고 terminal verdict/manifest를 발급한다.

- final clean source에서 동일 release daemon binary를 모든 process/cross-repo proof에 사용한다.
- mandatory family 전부와 deployment/activation/rollback evidence를 aggregate하고 누락/실패/skipped/stale를
  success로 계산하지 않는다.
- verdict를 `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`으로 분리한다.
- registered `AggregateQualificationReceiptV1` schema, registry-derived writer, validator and canonical final Just
  recipe가 dependency manifest validation → four-verdict calculation → atomic aggregate artifact publication → P12
  proof-manifest issuance를 수행한다. 기존 dependency manifests를 검사만 하고 새 aggregate artifact를 만들지 않는
  command는 P12 producer가 아니며 final proof를 발행할 수 없다.
- aggregate receipt는 P11 immediate handoff뿐 아니라 P00→P11 transitive handoff chain, P02A/P02B/P02I fork/join,
  exact source pair, attested binary와 모든 mandatory artifact digest를 검증한다.

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
- final recipe가 새 aggregate receipt와 P12 manifest를 실제 생성하고 둘의 digest/source binding을 재검증한다.

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

- 등록된 J7Q/live owner target 전수(현재 inventory 기준 7개는 구현 착수 시 재산정)
- ANN recall/relevance와 latency/resource를 분리한 fixed-corpus proof
- activation/read-view/GC concurrency 및 state-root two-process lease proof
- provider spy cancellation/egress matrix와 opt-in real-provider budgeted proof
- migration/backup/restore/rollback 및 cross-repo producer integration
- self-hosted production-like Linux runner의 pinned host identity; GitHub label 문자열만으로 host proof를 대체하지 않음

## No patch-on-patch rule

artifact schema 추가만으로 닫지 않는다. validator, blocking workflow, test authority, external rail,
final verdict computation을 하나의 proof graph로 연결한다.
