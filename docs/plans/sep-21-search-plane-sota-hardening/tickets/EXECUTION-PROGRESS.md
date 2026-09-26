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

## 2026-09-26 activation-event integrity continuation

- Scope: this catalog work began from `main` at `af640562`; unrelated retrieval work advanced it to `619292ca` without touching these owner files. Concurrent retrieval/benchmark and planning-document changes are outside this ownership. The goal remains broader than this catalog slice.
- Confirmed `FAILED` before repair: `invalidate_repomap_activation(repo, revision, generation=2)` accepted a sealed generation 2 while generation 1 was the active head, appended an invalidation event, deactivated generation 1's row and marked generation 2 invalidated. The owner test observed the wrong success. The catalog now checks the requested generation against the active head and verifies its candidate state/commitment before any event allocation or row mutation; a stale target is a typed CAS refusal with zero allocator delta.
- Confirmed `FAILED` before repair: self-digested `Activation` and `RepoMapInvalidation` ledger events with the wrong logical identity or commitment passed catalog reopen because domain pairing checked only sequence. The owner test constructs real ledger and activation/candidate rows for both event kinds and both substitutions. Reopen now uses one canonical activation-row decoder and checks event identity/commitment plus the candidate's state/commitment.
- Confirmed `FAILED` before repair: substituting the original `activation_sequence` in a structurally valid inactive activation row left its previous `row_sha256` accepted, because the digest omitted that field. The activation row preimage now includes both activation and terminal sequences under a new digest domain. SQLite and the canonical decoder enforce active/inactive reason and sequence ordering, and open refuses an installed activation schema without those checks before allocator seed.
- Focused local proof on the final dirty overlay: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 16 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` passed 1/1. Relevant source SHA-256s: `candidate.rs=7b8b9bf056a3a51c7af06326ca946fa0eba6a957b24d6270785095de637f4f4e`, `sequence.rs=f99eaf124ef724e6481a38b05e3e38f62db8a8bd5cc6bcad1772b3d557625386`. These are narrow owner/process checks, not clean-HEAD qualification.
- Breaking persistence boundary: an existing activation table lacking the new constraint is refused, and old activation-row digests do not match the new domain. No automatic migration or deployed-root conversion was implemented or claimed. Inventory and back up retained state before selecting a rebuild/migration path.
- `NOT_RUN`: clean-HEAD full `just verify`, cross-repo/release proof, production-state migration, deployment, activation and rollback. The focused passes above do not close the overall P0–P2 goal.

## 2026-09-26 active-head cardinality continuation

- Base: `main` at `9af01c75f39bd6517f66ae526df34d54db15c361`. During shared-main integration, the concurrent `8b612c8d` commit captured the catalog code alongside retrieval/benchmark work; the progress note landed in `d67daae5`. The catalog source hashes below match the committed tip, but the two commits do not have independent ownership boundaries.
- Confirmed `FAILED` before repair: two active epochs for the same repo/revision, each with a self-digested activation row, candidate row and ledger event, reopened successfully. A variant with the older epoch active and the latest epoch invalidated also has individually valid event/domain pairs but violates the one-head authority invariant. The hostile owner test first failed on the dual-active fixture.
- Repair: the catalog's existing integrity pass now rejects multiple active rows or an active nonlatest epoch per repo/revision at reopen. Targeted activation reads also reject an active row in any other epoch, so a post-open direct SQLite mutation cannot make a query silently select only the latest row. The normal writer remains serialized by the catalog transaction; no second active-head authority or separate projection was introduced. No new schema/index migration was added in this slice.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 17 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33, zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; focused reboot quarantine process test passed 1/1 (136 filtered). Source SHA-256s: `candidate.rs=bdca42c2dd86eaa4288081afdd7bc61ed12bdbf5aa427278c1187fa7299ab351`, `sequence.rs=0c31fe0c28066c6abd41507931ed437a3b96d935236a3d7ba536f04d0e769dcd`.
- Post-integration rerun on `d67daae5`: `just rust-test-candidate-activation-owner` passed 33/33, zero skipped, and the full catalog package command above passed 17+6+18+18 tests. This is a committed-tip local source check; it is not the full frozen qualification gate.
- `NOT_RUN`: clean-HEAD full `just verify`, operational migration of retained roots, release/deployment/activation/rollback. This local proof is not broad P0–P2 closure.

## 2026-09-26 ledger/domain bidirectional integrity continuation

- Scope: catalog sequence and RepoMap domain authority, starting from clean `main` at `da3e5d91`. During shared-main work, `f1d041ff` captured the candidate repair and an earlier sequence version alongside unrelated retrieval edits; the remaining sequence consolidation and this note are separate. Retrieval files are outside this ownership.
- Confirmed `FAILED` before repair: deleting a middle `Activation` event while a later event remained left the allocator maximum unchanged and allowed a self-digested active activation row to reopen. An activation row borrowing a `CandidateSeal` sequence also reopened; candidate and quarantine rows borrowing a `Rollback` sequence did likewise. Owner tests observed each wrong success against real SQLite before the corresponding repair.
- Repair: the append-only event scan now requires sequences 1..N without gaps. RepoMap activation, candidate and quarantine rows have indexed reverse kind/sequence checks; the existing forward event scan remains the one place that checks row digest, identity, payload and state. Direct activation reads additionally verify the exact self-digested activation/invalidation event references and candidate state. The forward and targeted-reference paths share one canonical event decoder instead of duplicating hash rules. A separate no-domain `Rollback` deletion fixture proves the gap check independently of reverse pairing.
- Narrow local proof on the final source overlay: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 22 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; focused reboot quarantine process test passed 1/1 (136 filtered) before the final decoder-only/test change. Relevant source SHA-256s: `candidate.rs=3aa2ab0750e9b70e9541fc2e6f4fdc69825c8265513d0e826cad5786326da7f0`, `sequence.rs=23760a87787e52208f5516ab99b5281b26129157ad7ac1fef8a4318d18468e97`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- `NOT_RUN`: clean-HEAD full `just verify`, release proof, retained-root restore/migration, deployment, activation and rollback. The owner and focused process checks do not close the broader P0–P2 goal.

## 2026-09-26 operation-journal event-custody continuation

- Scope: `main` at `f9f4ec44175cdb6a94b9e3bd1f8e8a21b9e3be8f` plus this catalog-only overlay. Concurrent Sep-23/Sep-26 retrieval edits are outside this ownership and were preserved.
- Confirmed `FAILED` before repair: after a real committed operation, replacing only its ledger event with a correctly self-digested `Rollback` event let the terminal journal row and catalog reopen successfully. The existing forward pass skipped `Rollback`, and no journal-row-to-ledger check existed. The owner test first observed the wrong success. The repaired test also exercises separately self-digested wrong identity and payload, both after an in-process replay attempt and after reopen.
- Repair: terminal journal rows now require an exact current event kind/sequence at reopen; the canonical journal reader verifies its own row digest, receipt-bytes digest, expected event kind, key identity and state-specific payload against the self-digested ledger event before returning a replay. The ledger forward pass uses that reader for surviving terminal rows and retains explicit invalidation attribution for legitimately removed historical rows. Prepared/claimed/applying recovery and generation-GC semantics remain distinct; no second journal authority was introduced.
- Existing corruption tests now expect refusal at the earlier catalog-open boundary. The former one-byte foreign-version fixture was not a coherent persisted future-format receipt because its receipt and event digests were left unchanged; it is now named as a tamper test. A separate pure decoder test checks typed foreign-tag refusal. A fully re-digested foreign-version persisted fixture remains `NOT_RUN` and must not be inferred from these two tests.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 24 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36, zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Source SHA-256s: `idempotency.rs=77bf300d3b8b987bbf2c38cf08de07d3e9fa69da6d9baeb8e0953df4186a8ed1`, `sequence.rs=27545c76a6b6301db1fa0803d5adabe9f77b1e4c37d681828bc10b2b7f5d07c7`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- `NOT_RUN`: clean-HEAD full `just verify`, fully coherent foreign-version persisted fixture, release proof, retained-root restore/migration, deployment, activation and rollback. This owner/process check is not overall P0–P2 closure.
