# P12A — Proof infrastructure custody and receipt

The aggregate schema, writer, validator, shared handoff validator, and final
recipe are present in source. This prompt is for current-source review,
correction, and exact-pair P12A proof issuance; do not reimplement a second
aggregate or file-custody stack.

Freeze Quanta HEAD/dirty state and Semantica HEAD/lockfile. Validate the P11
exact-pair prerequisite and authentic P00-P11 historical handoffs before
issuing a P12A receipt. Deployment, activation, and rollback are separate
P12Q dependencies; their absence does not convert P12A owner code into a
release qualification.

Review these existing owners:

- `tools/ci/proof-aggregate.schema.json` and `write-proof-aggregate.py`
- `tools/ci/lint/check-proof-authority.py` and
  `tools/ci/lint/handoff_validation.py`
- `tools/ci/write-proof-manifest.py`, proof and test authority registries
- `Justfile` P12A owner and final qualification recipes
- `tools/ci/tests/test_{write_proof_aggregate,write_proof_manifest,check_proof_authority,check_lane_handoff,handoff_validation}.py`

Required behavior: validate the fixed P00-P11 handoff chain and P02 fork/join,
source pair, dependency receipt digests, binary/host binding, and separate
`CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, and
`ROLLBACK_PROVEN` verdicts. Missing, malformed, stale, symlinked, reordered,
wrong-source, or wrong-host input must not become readiness. Preserve the
independent expected DAG; the registry must not define its own oracle.

Run `just proof-p12a-proof-infrastructure` on the frozen source. This is
owner code proof only. Issue `p12a-proof-infrastructure` using the registered
exact-pair writer only when its P11 dependency, infrastructure handoff, and
terminal evidence are authentic. P12Q then runs separately on one final clean
source pair; do not issue P12 from a P12A test result.
