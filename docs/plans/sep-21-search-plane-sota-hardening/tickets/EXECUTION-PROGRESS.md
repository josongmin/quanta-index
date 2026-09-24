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
