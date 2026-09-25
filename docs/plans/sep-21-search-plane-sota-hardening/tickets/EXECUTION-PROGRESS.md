# SEP-21 execution evidence

This file indexes live evidence. The former dated execution ledger is available
in Git history; its dirty-checkout observations and test counts are not current
qualification.

2026-09-24 static audit at Quanta `28c20fabfdc9d57b0d7d94794d59bcf78ea7cd14`
found proof-result/host trust, resolved cross-repo Cargo dependency-root,
P09 backend-health/diagnostic, and P11 operational-action authority gaps.
The [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) and
[action list](ACTION-LIST.md) now sequence the repairs. This document update
issued no proof manifest and did not run Rust or release qualification.

## Source of truth

- [proof-authority.toml](../../../../tools/ci/proof-authority.toml) declares
  proof IDs, dependencies, execution state, host, and artifact paths.
- [check-proof-authority.py](../../../../tools/ci/lint/check-proof-authority.py)
  validates the registry, individual receipts, and the final aggregate.
- [Justfile](../../../../Justfile) owns the commands. PR CI issues and checks a
  fresh P00 receipt; the full release gate runs only with an explicit proof
  bundle and paired repository revision.
- [state-cutover-runbook.md](../../../operator/state-cutover-runbook.md) describes
  the supported current-format backup/restore/verify workflow. Legacy
  `migrate-state` is retired.

## Read the current result

From a frozen checkout, record `git rev-parse HEAD`,
`git status --porcelain=v1`, branch/upstream, and the paired Semantica
revision before using any receipt. Run:

```sh
just proof-authority-lint
python3 tools/ci/lint/check-proof-authority.py --require-all --bind-source \
  --paired-checkout "github:josongmin/semantica-codegraph-v2=$SEMANTICA_CHECKOUT"
```

The first command is registry-only; zero manifests validated is not execution
proof. The second is the final release check and requires every current-source
manifest and the P12 aggregate. An old `passed` receipt remains historical
evidence, even when its archive is intact. A staged node is blocked until its
real authority is implemented and registered; adding a JSON file cannot
promote it.

Classify owner tests, exact-pair proof, Linux process proof, deployment,
activation, and rollback separately. A missing or stale receipt is not a failed
test. The validator's raw findings, exact command, source pair, and artifact
digests are the report; do not copy a dated count from this document into a
new closure claim.

## 2026-09-26 local quarantine-recovery check

- `FAILED` before repair: `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` reopened a catalog with a sealed-only candidate quarantine and returned `CATALOG_ROW_CORRUPT`: event 7 had no activation-invalidation domain pair.
- RCA: `quarantine_repomap_candidate` appended `RepoMapInvalidation`, while replay paired that kind only to an inactive activation row. The sealed candidate has no activation row. Commit `b4e21b50` separates `RepoMapCandidateQuarantine` and stores its exact sequence on the candidate row without overwriting the seal sequence; the new event is checked for a candidate pair at reopen. Prior event-kind schemas are refused, not silently migrated.
- `VERIFIED` narrowly: the focused E2E above passed 1/1 after repair. Catalog unit coverage passed the sealed-candidate quarantine/replay/reopen path and rejected an orphan candidate-quarantine event. The later `just verify` run passed workspace Rust tests (including runtime extended 64/64 and risk 137/137), rustdoc, policy checks and Semgrep, but is **not** a final-source qualification: `main` moved from `8083abc2` to `b4e21b50` and then `1ef1ec60` during the run.
- `FAILED` final command: `just verify` exited 1 at `python-format-check` because concurrent retrieval work left `tools/benchmark/retrieval/query_plan.py`, `tools/ci/tests/test_retrieval_benchmark.py`, and `tools/ci/tests/test_write_verification_receipt.py` unformatted. This was a moving, dirty shared checkout, not a clean-HEAD receipt. Those files were not modified by this quarantine repair.
- At `1ef1ec60236bb53176d72578830a7241547acb81`, `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` and the focused daemon E2E above passed. The predecessor-schema refusal fixture was corrected from the older 1..=9 set to the immediate predecessor 1..=10 set; catalog tests were rerun and passed again (4 unit, 6 auxiliary, 18 idempotency, 18 operation-journal). Retrieval files and this progress note were dirty, so these remain narrow local checks, not clean-source qualification.
- `NOT_RUN`: clean, frozen-HEAD full verification, release proof, deployment, activation and rollback. Rerun `just verify` only after the concurrent writer freezes and formats its own files; bind the resulting receipt to that exact clean source.

## 2026-09-26 session hardening of candidate-event integrity

- Scope: local `main` at `bf51f55f` plus this task's catalog edits. Concurrent dirty retrieval-benchmark files are outside this repair and were preserved.
- Confirmed `FAILED` before repair: a correctly self-digested `RepoMapCandidateQuarantine` event with the wrong logical identity passed catalog reopen when its sequence matched a quarantined candidate. A quarantine sequence earlier than its seal sequence also passed the candidate table. An installed candidate table lacking the new ordering constraint was accepted by `CREATE IF NOT EXISTS`. These three cases were observed RED in owner-local tests against real SQLite. The same sequence-only lookup also omitted commitment binding; the final test exercises both substitutions.
- Repair: the catalog now verifies both candidate event kinds through the canonical candidate row decoder, binding sequence, row digest, logical identity and commitment; the table enforces quarantine-after-seal ordering; open refuses an incompatible installed candidate schema before allocator seed. These checks add no second serving authority or consumer fallback.
- Narrow local proof on that dirty source: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 8 unit, 6 auxiliary, 18 idempotency and 18 journal tests; package all-target Clippy passed; `just rust-test-candidate-activation-owner` passed 30/30 with 0 skipped; the focused `e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot` daemon process test passed 1/1. The three observed RED cases pass after repair; the added seal-identity and commitment variants also pass.
- `NOT_RUN`: clean-HEAD full `just verify` and release/operational proof for this new source. These narrow checks do not supersede the earlier failed full gate or qualify the concurrent retrieval changes.

## 2026-09-26 session hardening of quarantine discard and replay

- Scope: catalog quarantine record/discard authority, RepoMap payload projection and recovery, and the daemon quarantine route. The review started at `8d9b9f37`; unrelated retrieval work advanced `main` to `c6f0405a` and left other files dirty while these changes were made. The commands below therefore prove only the named owner/boundary behavior on a local overlay, not a clean-HEAD repository qualification.
- Confirmed `FAILED` before repair: a self-digested tombstone with `discarded=1, discard_sequence=NULL` returned the record sequence as a successful discard retry. The catalog now requires the discard state/sequence relation in the SQLite schema and canonical row decoder, validates the complete existing row before mutation/replay, and refuses a predecessor quarantine schema before allocator seed. The owner test first failed on the old fallback, then passed against the repaired owner.
- Confirmed `FAILED` before repair: a catalog tombstone committed before payload unlink left a file that a retry called `Absent`; two incidents with the same content-addressed payload lost the still-live projection when the first was discarded. Real-file owner tests failed on both cases. The RepoMap store now finishes pending reclaim on retry and boot, consults catalog live references before unlink, and serializes a store's discard/reclaim boundary. A repeated already-tombstoned incident with another live reference returns `Absent`, not a fabricated new discard.
- Confirmed `FAILED` before repair: a correctly self-digested `QuarantineRecord` ledger event with an incorrect incident identity reopened because integrity pairing checked only sequence. The catalog now pairs both quarantine event kinds through the canonical self-digested incident decoder and checks event identity and payload digest. The owner test covers wrong record identity and wrong discard payload.
- Narrow local proof after these repairs: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 12 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with 0 skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` passed 1/1. These are a local dirty-overlay proof of the named paths, not clean-HEAD qualification. Relevant code/test SHA-256s at closeout: `candidate.rs=a1ae23a5f549e3c2a21f5ceff9fcea225ffd03d3c204d04a436d661fd624341f`, `sequence.rs=d7c91a93e49013ac2846f8863b6e59501848fd9b6165bba4917216ba6bd09dc8`, `store.rs=2f426c392d19e54a21f651e80b056852715224e63a7d98654279bf964c061034`, `candidate_activation_owner_v1.rs=d55988e98527e0bb2f56dc72a1db84fffa20fc8635dbaab3c3ccdbd5e6abd69e`. Any relevant source change invalidates this receipt.
- Breaking persistence boundary: this build refuses existing quarantine tables lacking the new checks; there is no silent in-place migration. Before activation on any retained state root, inventory exact schema/data, back up, and select a supported rebuild/migration path. No production root was inspected or mutated by this task.
- `NOT_RUN`: frozen clean-HEAD full `just verify`, release proof, deployment, activation, rollback and production-state migration. A focused owner or daemon pass does not close those claims.
