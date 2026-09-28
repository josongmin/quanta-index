# S21-13 — Release evidence and qualification

Status: `ACTIVE — infrastructure exists; final qualification remains open`.

Accepted proof parsing, custody, selection and receipt contracts are in
[SEP-21-004](../../../adr/SEP-21-004-process-supervision-state-cutover-and-proof.md)
and [SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
`tools/ci/proof-authority.toml` owns current proof IDs, dependencies, host classes
and result modes; the checker keeps an independent required-DAG/verdict oracle.
[R0/R6](FINAL-RESIDUAL-EXECUTION-PLAN.md) and the
[residual ledger](CURRENT-RESIDUAL-2026-09-26.md) own unfinished work.

## Remaining release acceptance

- Collect actual selected/executed/ignored/failure inventory and raw terminal
  output for every registered proof on one final clean Quanta/Semantica source
  pair. Bind relevant dependency roots, configuration, corpus, model/provider,
  toolchain, release daemon binary and host. P00 foundation/owner checks do not
  replace Linux release targets, exact producer or operational action results.
- P12A may issue a Quanta exact-source infrastructure receipt from its own raw
  Python inventory and JUnit before P11. Its source-bound receipt is a final
  graph prerequisite, not a release verdict. Run the actual P12A owner recipe;
  local tests or an invented/stale manifest do not substitute.
- Qualify P10 current-format state and authorized target, then P11 live exact
  producer/daemon pair and V2 terminal chain. Deployment, activation and rollback
  require separate observed actions, manifests and a shared operational host.
  Cross-repo protocol passing does not imply any of those actions. Absence of
  authorization or host leaves each action `NOT_RUN`/`BLOCKED`.
- Reissue every required owner/release proof at the final source. Run the
  [final recipe](prompts/README.md) and validate all manifests/dependencies plus
  aggregate with `--require-all --bind-source`. The aggregate is the final
  release receipt only when it accepts the entire registered graph. A source,
  binary, dependency or required input change invalidates the affected proof.

The aggregate reports `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED` and
`ROLLBACK_PROVEN` separately. Code/process nodes may use distinct qualified
Linux host instances where their registered profile permits. The three
operational actions share one exact host identity. An archive of historical
handoffs is a different audit and is not a release dependency.

## Fixed oracle and measurement classes

Freeze corpus, host, model, profile, thresholds and independent expected outcomes
before running. Preserve separate classes: authority collisions/mixed-generation/
replay/recovery; exact-versus-capped query and ANN recall; judged NDCG/MRR/Recall;
p50/p95/p99 plus offered/accepted/completed/error; peak RSS/FD/thread/queue,
disk/WAL and drain; recovery/restore/GC; provider request/residual-task/token/
cost/model identity. Advisory, fake/hash or lower-scope evidence cannot replace
required real-provider, Linux performance, fault, installed-process or producer
proof. New targets must enter executable test/proof authority before issuance.

Reject missing, stale, dirty-mismatched, wrong-binary/host/source, duplicate,
partial, zero-selected, ignored-only, timeout and report-only inputs. Refuse
artifact absence, fake provider, in-process crash substitute, macOS substituted
for Linux, unchecked restored state and wrong producer checkout. A self-reported
PASS or registry-edited DAG cannot define the independent expected result.
The [purpose audit IDs](../../../analysis/quanta-index-purpose-validation-checklist.md)
retain mandatory P0/P1 coverage; a code-local result does not close external rows.

`PRODUCTION_READY` requires the registered M0–M4 dependency checkpoints, complete
current proof graph, accepted state restore/rollback drill, exact-pair terminal
receipt, qualified Linux process/performance and real-provider results, plus no
unresolved mandatory P0/P1, `BLOCKED` or `NOT_RUN`. The accepted aggregate and
observed deployment/activation/rollback verdicts remain separately required.
