# P12A — Proof infrastructure custody and receipt

The aggregate schema, writer, validator, shared handoff validator, and final
recipe are present in source. This prompt is for current-source review,
correction, and exact-source P12A proof issuance; do not reimplement a second
aggregate or file-custody stack.

Freeze Quanta HEAD/dirty state and collect P12A's raw Python inventory and
JUnit. P11's exact pair is a final aggregate prerequisite; its absence does
not prevent issuing a source-bound P12A
receipt. Deployment, activation, and rollback remain separate final aggregate
dependencies.

Review these existing owners:

- `tools/ci/proof-aggregate.schema.json` and `write-proof-aggregate.py`
- `tools/ci/lint/check-proof-authority.py` and
  `tools/ci/lint/handoff_validation.py`
- `tools/ci/write-proof-manifest.py`, proof and test authority registries
- `Justfile` P12A owner and final qualification recipes
- `tools/ci/tests/test_{write_proof_aggregate,write_proof_manifest,check_proof_authority,check_lane_handoff,handoff_validation}.py`

Required behavior: validate the current source pair, dependency receipt
digests, binary/host binding, and separate
`CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, and
`ROLLBACK_PROVEN` verdicts. Missing, malformed, stale, symlinked, reordered,
wrong-source, or wrong-host input must not become readiness. Preserve the
independent expected DAG; the registry must not define its own oracle.

Run `just proof-p12a-proof-infrastructure` with `QUANTA_PROOF_RAW_DIR` on the
frozen source. Issue `p12a-proof-infrastructure` with the registered
exact-source writer only after archiving the matching inventory and JUnit.
The P12A receipt and final aggregate run separately on one final clean
source pair; do not infer release readiness from a P12A test result.
