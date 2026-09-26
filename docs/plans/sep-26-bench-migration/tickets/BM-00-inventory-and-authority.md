# BM-00 — Inventory and authority freeze

Status: `PLAN / NOT_RUN`. Priority: P0. Depends on: none. Common gates: [TEST-PLAN](TEST-PLAN.md).
Implementation/verification/qualification verdicts for this ticket are recorded in `git show eff53181:docs/plans/sep-26-bench-migration/tickets/CLOSEOUT.md` and, where relevant, [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03 decision ADR](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md) and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md). The original `PLAN / NOT_RUN` status above is the plan-time state, not the closeout state.

## Purpose

Establish an exact, reviewable inventory before moving paths or changing schemas. The inventory must distinguish producer, measurement boundary, scorer, validator, baseline owner, CI invocation, and release claim. It is not a list of filenames mistaken for completed evidence.

## Work

1. Enumerate Cargo `[[bench]]` targets and benchmark binaries via `./scripts/cargow metadata`, `Justfile` timing recipes, `tools/benchmark/manifest.json` families, retrieval/agent-outcome entrypoints, `.github/workflows` calls, artifact validators, and `tools/ci/{test-authority,proof-authority}.toml` consumers. Classify correctness tests and `sourcegraph_parity.py` separately; do not count them as timed benchmark results.
2. For each family record stable ID, owner, scenario/input source, output unit, raw artifact, scorer, environment and host policy, current gate, whether a baseline exists, and whether it is diagnostic or qualifying. Record unregistered and duplicated paths explicitly.
3. Set the one-current-authority cutover rule: old control plane remains authoritative per family until BM-07 accepts the replacement; two simultaneous green authorities are prohibited.
4. Classify files as normative protocol/registry/proof inputs versus planning/history. Bind the normative subset of this packet, new schemas/registry/CLI and their owning tests to the relevant source-closure profile(s) in `tools/ci/source_closure.py`; document every exclusion. Add mutation tests that changed, added, removed, and relevant dirty files invalidate the corresponding receipt. Do not pretend current external corpus bytes belong in Git source closure; bind them separately by digest. Do not make an unrelated plan-status edit invalidate a product proof without an explicit contract reason.

## DoD

- Exact inventory agrees with Cargo metadata, Just, CI and registered artifact families; unregistered producer and duplicate verdict owner yield policy failure.
- Each existing family has an explicit migration destination or documented retirement reason; no `latest/` file or historical receipt is accepted as a baseline by filename alone.
- Normative packet changes invalidate affected future source-bound benchmark proof, with negative tests; planning/history exclusions are listed. Existing RB/RBR source-closure requirements remain intact.
- Inventory records source HEAD/dirty state and is re-frozen at implementation start; this plan's 2026-09-26 snapshot is not used as an implementation receipt.

## Verification / exclusions

Use read-only Cargo metadata and registry/Just/CI scans first, then source-closure unit and mutation tests. No performance capture, external gold creation, or product algorithm change belongs to BM-00.
