# P11 — Exact-pair producer and daemon cutover

The P11 proof nodes are staged in `tools/ci/proof-authority.toml`. The
Quanta V2 activation request carries `expected_active`, and the catalog
checks it before sequence allocation. This source behavior does not establish
Semantica producer compatibility or release qualification.

Use [S21-12](../S21-12-cross-repo-terminal-receipt-cutover.md) and the
[residual plan](../FINAL-RESIDUAL-EXECUTION-PLAN.md). Before implementation
or proof, freeze both repositories' HEAD, dirty state, dependency roots,
contract and SDK versions, and the release daemon binary identity. Inspect
Semantica's current producer code; do not reuse an old dirty snapshot or
assume that its durable intent retains the original prior-head token.

Required proof separates:

1. Producer source and prepared payload from the canonical request/body,
   operation journal terminal receipt, sealed candidate, activation
   commitment, and replay result.
2. The exact Quanta/Semantica source pair, dependency locks, release binary,
   Linux host, and positive and negative protocol cases.
3. `p11-cross-repo-cutover`, `p11-deployment`, `p11-activation`, and
   `p11-rollback` as four distinct manifests. A protocol pass does not
   imply deployment, activation, or rollback.

The registered cross-repo command is `just rust-verify-hellgate-cross-repo`.
Check its actual target selection and paired input before promoting P11 from
staged. The registered deploy/activate/rollback commands must become real
independently issuable actions before their nodes can be promoted. Missing
producer input, wrong source/binary/host, stale prior-head expectation,
identity-only ACK, or synthetic receipts leave the corresponding node
`BLOCKED` or `NOT_RUN`.
