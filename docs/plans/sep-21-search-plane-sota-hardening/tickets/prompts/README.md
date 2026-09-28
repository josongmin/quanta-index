# SEP-21 execution entrypoints

Current proof IDs, command bindings, dependencies, host class and staging state
are owned by `tools/ci/proof-authority.toml` and its independent checker. Select
live targets from `Justfile` and `tools/ci/test-authority.toml`; a command name,
old prompt or historical handoff is not a passing receipt.

| Stage | Entry command | Remaining acceptance |
| --- | --- | --- |
| P10 owner | `just proof-p10-state-migration-owner` | [S21-11](../S21-11-state-migration-backup-and-restore.md): disposable-root custody and exact-source owner result; Linux release/authorized target is separate |
| P11 protocol | `just rust-verify-hellgate-cross-repo <semantica-root>` | [S21-12](../S21-12-cross-repo-terminal-receipt-cutover.md): frozen source pair, dependency locks, attested daemon, positive/negative V2 and replay |
| P12A infrastructure | `QUANTA_PROOF_RAW_DIR=<external-raw-dir> just proof-p12a-proof-infrastructure` | [S21-13](../S21-13-release-evidence-and-sota-qualification.md): raw Python inventory/JUnit and source-bound P12A manifest; independent of P11 issuance |
| P12 final | `SEMANTICA_CHECKOUT=<frozen-root> just proof-authority-final-qualification` | Complete final-source graph and separate code/deploy/activate/rollback verdicts |

The P10 release node and P11 protocol/deployment/activation/rollback nodes remain
staged until their registered host, producer and operational results exist.
`proof-p11-{deployment,activation,rollback}` entries in the registry are not
executable action recipes yet. Do not issue a receipt from their names or infer
one action from another. The final recipe writes and then checks the aggregate
with `--require-all --bind-source`; an unready artifact is diagnostic only.

Freeze relevant source/dirty state, input/dependency locks, selected command,
release binary and host before proof. P10 must reject old roots and avoid legacy
import. P11 must compare actual producer payload, terminal receipt, candidate,
activation and exact ACK replay; a local CAS result is insufficient. P12A needs
complete raw selected/executed outcomes and the independent DAG oracle. Reject
missing, stale, wrong-source/host/binary, duplicate or partial input. Report
`NOT_RUN` or `BLOCKED` for every unexecuted dependency. Historical handoffs are
[separate archive audit](../handoffs/README.md), not final release prerequisites.
