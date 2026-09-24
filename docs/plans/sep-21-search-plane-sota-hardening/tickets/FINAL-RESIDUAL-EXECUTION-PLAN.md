# SEP-21 residual execution plan

This is the current work map. The previous dated dirty-checkout overlays and
conditional importer plan are retained in Git history, not as executable
instructions. Reinspect code and proof registry at the source revision used
for each run; this document is not a receipt.

## Contract already present in source

- The CLI exposes `backup-state`, `restore-state`, and `verify-state`.
  `migrate-state` is not a command. Boot refuses legacy state instead of
  converting it. See [the operator runbook](../../../operator/state-cutover-runbook.md).
- `tools/ci/proof-aggregate.schema.json`,
  `tools/ci/write-proof-aggregate.py`,
  `tools/ci/lint/check-proof-authority.py`, and
  `tools/ci/lint/handoff_validation.py` implement the aggregate schema,
  producer, validator, and handoff DAG checks. `Justfile` has the P12A owner
  recipe and final producer recipe. Code presence is not a P12A manifest.
- `tools/ci/proof-authority.toml` is the operational registry. The checker
  independently fixes the expected proof dependency DAG and verdict inputs.
  Keep these distinct so editing the registry cannot redefine its own oracle.
- PR CI generates a fresh P00 receipt. The all-proof release gate in
  `.github/workflows/correctness.yml` is an explicit proof-bundle dispatch,
  not an ordinary PR gate.

## Work still requiring evidence

1. **Owner and release split.** Run each executable owner recipe against one
   frozen clean source. Register missing authority before promoting any staged
   release node. An owner pass cannot satisfy its Linux process or external
   release node.
2. **P10 state custody.** Verify current-format backup, restore, refusal, and
   restore-forward using disposable roots. Inventory real target roots and
   retained-data obligations before any cutover. A legacy root needing retained
   data stays blocked until an explicit producer rebuild and data decision;
   do not resurrect a snapshot-to-source-IR importer.
3. **P11 exact pair.** Freeze the Quanta/Semantica revisions and dependency
   locks, prove producer-to-daemon terminal commitments, and bind the release
   binary. Issue deployment, activation, and rollback as separate proofs.
   Code or a focused local test does not establish a paired release receipt.
4. **P12A custody.** Validate the existing shared no-follow reader and
   pinned-parent writer with independent malformed, symlink, swap, digest,
   wrong-source, and handoff-order oracles. Issue an authentic P12A exact-pair
   manifest only after its P11 dependency and infrastructure handoff exist.
5. **P12Q final qualification.** On the final clean source pair, issue current
   manifests for every registered dependency, validate the historical handoff
   chain and final-source proof DAG, produce the aggregate, and issue the P12
   manifest. The same attested release binary and required Linux host must
   bind every applicable release proof. Missing, staged, stale, or non-passing
   inputs keep production readiness false.

Use [EXECUTION-PROGRESS.md](EXECUTION-PROGRESS.md) to query live status.
Do not recreate historical handoffs or convert an old `passed` alias into a
current receipt. If authentic historical evidence cannot be recovered, record
the gap and revise the acceptance contract explicitly before P12Q.
