# P11 operational proof contract

`just proof-p11-deployment`, `just proof-p11-activation` and
`just proof-p11-rollback` use the canonical operational producer and existing
manifest writer/checker/aggregate. They currently refuse because the registry
entries are staged. No concrete target adapter or operational contract is
registered. Local Python fixtures establish control behavior only.

## Registered inputs

An executable operational entry must use `execution_mode = "operational-action"`
and name one tracked `operational_contract` file. The contract is a closed
object with `schema_version`, `proof_id`, `target`, `actors`, `timeout_seconds`
and `expected`. `tools/ci/proof_operational_result.py` owns its semantic grammar.

- Target: actual Linux host identity, absolute installed binary/config/state
  paths. The host identity uses machine-id, hostname and the existing manifest
  host/capacity digest; missing host observations refuse execution.
- Actors: tracked Python sources for `pre`, `action`, `post`. Pre/post source
  paths and bytes must differ from the action source. Each is a separate
  invocation. This is a custody check; concrete observers still need independent
  domain oracles. The runner does not invent them or accept actor overrides.
- Expected deployment: attested daemon digest, configuration digest and current
  state format. Pre/post must show a real transition.
- Expected activation: deployment identity plus selected generation and a fixed
  independently established query-result digest. The generation must change.
- Expected restore-forward: deployment identity plus backup data digest,
  backup root incarnation and sequence high-water. Data/sequence must match the
  backup, and the restored incarnation must differ from both backup and prior root.

Actual deployment/service management, activation/query protocol and backup-data
observer implementations depend on the selected target. Register their concrete
contracts after those inputs are defined; generic shell exit codes are insufficient.

## Execution and publication

Each actor receives `--request <file>` with a closed request containing schema,
proof ID, fresh run ID, phase and target. Only the action receives `expected`.
Pre/post observers receive no expected success output. Each emits a closed
observation carrying the same run/phase/target and typed observed state. Action
completion alone cannot issue success: the independent post-state must satisfy
the registered transition.

The runner validates current prerequisites before mutation. It checks source
pair, contract/actor bytes, actual host and bound immutable prerequisite bytes
before/after each phase. The existing process owner bounds output/timeout,
retains command/raw custody and kills its pinned group before returning.
Failed/partial execution does not publish a completed action record.

When enabled, provide a fresh repository-relative `QUANTA_PROOF_RAW_DIR` and
`SEMANTICA_CHECKOUT`. Equivalent CLI options are `--output` and
`--paired-checkout`. The canonical writer archives the raw contract, actor
sources, requests, command records and outputs. Archived actor/contract code
remains subject to dirty-source detection. The checker recomputes one action,
checks chronology within the manifest window and refuses mixed test results.
The aggregate additionally requires a common host, target and configuration
across deployment, activation and restore-forward.

The registry is not promoted by passing these owner tests. Actual Linux actions,
exact-pair acceptance and final aggregate qualification remain separate runs.
