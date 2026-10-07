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
The pre-observer must establish an admissible live state before the action can
run. Activation and restore require the attested deployment; deployment and
activation also refuse an already achieved transition. Operational prerequisite
edges must use the same target paths and configuration before any actor runs,
and the manifest checker enforces this continuity on archived dependencies.

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

## Paired caller and kernel

`just rust-verify-hellgate-cross-repo <Semantica checkout>` uses clean Quanta and
Semantica checkouts, the actual nested Cargo lock/resolver, a fresh release
daemon and the two exact caller/kernel tests. Provide
`QUANTA_INDEX_SEARCHD_BIN` with the matching executable and
`QUANTA_P11_R5_QBC_LANE` with an explicitly registered lane in that producer
checkout. If the lane is new, admit it through the producer's
`scripts/quanta-build-cli lane-token issue --lane <lane> --reason <reason>` and
pass the returned token through `QUANTA_BUILD_LANE_TOKEN`. Missing registration
refuses before the expensive daemon build; the recipe does not invent a lane.

The QBC metadata adapter reads the same auxiliary invocation's output using
its nonce, new run ID, exact command, stable owner status/file identity and
clean source/manifest/lock checks. This establishes dependency resolution only.
The recipe prefixes Quanta's Python import path, and its regular `tools`
package keeps imports within this checkout even when the paired working
directory contains a different package with the same name.
Selected Nextest inventory and execution retain QBC's immutable completion
locator and receipt checks. Effective exit125, source drift, missing completion
or output replacement cannot be treated as success.

The typed selected-run archive is also used by the default recipe. Set
`QUANTA_P11_R5_EVIDENCE_ROOT` to a fresh absolute external path to choose its
location; otherwise the recipe retains it under a new `/private/tmp` directory.
It remains `runner-candidate-only`, separate from installed Linux transitions
and the operational registry. A refused producer completion stays refused even
when both dependency resolutions pass.
