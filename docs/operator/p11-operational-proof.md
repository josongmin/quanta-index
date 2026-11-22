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

Each executable P11 entry registers its producer's public Ed25519 key in
`attestation_public_key` using the two OpenSSH fields `ssh-ed25519 <base64>`.
The producer and replay hosts use `/usr/bin/ssh-keygen -Y`.
The supervised producer receives its dedicated signing agent socket through
`QUANTA_P11_SIGNER_SOCKET`. Access to this agent authorizes issuance and must
be confined to the approved producer on the selected host. Replay uses the
registered public key and the archived detached signature. Producers verify
returned signatures against the current statement. The operational producer
checks signing availability before invoking the first actor.

## Execution and publication

The isolated Python child verifies the actor and request SHA-256 values before
running any actor code. It executes the verified source bytes and passes the
verified request through an open anonymous file. Each actor receives
`--request <file>` with a closed request containing schema,
proof ID, fresh run ID, phase and target. Only the action receives `expected`.
Pre/post observers receive no expected success output. Each emits a closed
observation carrying the same run/phase/target and typed observed state. Action
completion alone cannot issue success: the independent post-state must satisfy
the registered transition.

The runner validates current prerequisites before mutation. The CLI holds the
proof store's write lock from prerequisite resolution through manifest
publication. Each phase checks the source pair, committed contract/actor bytes,
host and bound prerequisite and output bytes. The existing process owner
retains command/raw custody and stops its pinned group before returning.
The producer repeats these checks after signing before publishing the signature.
Immediately before starting the action, the producer publishes a durable
`blocked` manifest under the same lock. An interrupted action leaves that
diagnostic current. A successful post-observation can supersede it. Each P11
publication must advance its recorded end time or retry the exact current bytes.
The writer makes the immutable receipt and current alias durable before it
commits the archive index. All current consumers select the latest indexed end
time across the proof's archive directories and require the alias to match.
Branch names and tracking metadata remain in the archived receipts. A current
alias that differs from the index fails verification, including a standalone
`--manifest` check. `--bind-source` also checks the current checkouts.

An interruption can leave the immutable leaf staged before the index commit.
When a completed action's manifest publication fails, pass its saved
`terminal.json` to `write-proof-manifest.py` with the same proof ID and paired
checkout. The writer revalidates the terminal, reuses any identical staged
leaf and commits the receipt. The `pending.json` receipt records the separate
pre-action diagnostic.

The pre-observer must establish an admissible live state before the action can
run. Activation and restore require the attested deployment; deployment and
activation also refuse an already achieved transition. Operational prerequisite
edges must use the same target paths, configuration, release daemon and state
format. Each direct prerequisite must have ended before its successor starts.
The checker binds the full P11 dependency graph to the same source inputs and
binds release-daemon entries to the same binary and state format.

When enabled, provide `SEMANTICA_CHECKOUT` and a fresh repository-relative
`QUANTA_PROOF_RAW_DIR` under `artifacts/proof-authority/` or a Git-ignored output
directory. Equivalent CLI options are `--output` and `--paired-checkout`.
The canonical writer archives the raw contract, actor sources, requests,
command records and outputs. The checker matches the archived instructions to
the claimed Git commit, validates the recorded execution paths and recomputes
one action from the bound evidence. Its signature covers the content digests
of the contract, actors, requests, command records, outputs and the event
record containing the full execution environment, typed input digests, source pair
and prerequisite digests. Its execution window spans the first pre-observation
through the last post-observation. Archival replay reads the contract
registration, producer key and prerequisite graph at each receipt's recorded
Git commit. It can read the archive from another checkout with those Git objects.
The aggregate additionally requires a common host, target and configuration
across deployment, activation and restore-forward. Current verdicts compare
every dependency edge to the exact receipt selected for its prerequisite.
Archived manifests retain their original dependency references and use each
manifest's registered schema from its recorded Git commit.

The registry is not promoted by passing these owner tests. Actual Linux actions,
exact-pair acceptance and final aggregate qualification remain separate runs.

## P04 read-view proof

The P04 prerequisite collects two Native product read-view cases. Set
`QUANTA_INDEX_SEARCHD_BIN` to the absolute release daemon path and
`QUANTA_PROOF_RAW_DIR` to a fresh directory, then run
`just proof-p04-read-view-lifetime`. The runner pins a private executable copy
before collecting tests. On Linux each test hashes both live daemon processes
across restart, and the runner checks those hashes against the pinned copy.
The runner and archival replay require exactly the registered test cases.
Every observation must match the archived release daemon.

## Execution source binding

Run the registered Native release recipes or P12A from the checkout under
test with `QUANTA_PROOF_RAW_DIR` pointing to a fresh output directory outside
the source checkout. The runner captures the source before selection and uses
the same frozen checkout for collection and execution. It reuses a
read-only-mounted checkout or copies the Git index and working tree into a
private read-only projection. It checks the executed tree and the original
source after collection and execution. Native release runs keep the existing
Cargo lane and bind `Cargo.lock` and the pinned release daemon SHA-256.

For each run, set `execution_result.runs[].run_binding` to the printed binding
path, alongside `events`, `inventory` and, for Native release runs,
`daemon_observations`. Place the original files under the registered artifact
root and list their repository-relative paths in `artifacts`. Publication and
archival replay compare the binding to the issuing commit and the archived
runner bytes.

### P07 provider proof

HTTPS OpenAI configurations require `semantic.provider.work_budget` with
`inflight_requests_cap`, `inflight_bytes_cap`, `retry_attempts_cap`,
`total_cost_micros_ceiling`, and `openai_max_request_cost_micros`. The
daemon applies one process-lifetime ledger to physical Source and query
HTTP attempts. Each admitted attempt counts against the cost ceiling.

`just proof-p07-provider-boundary` first validates the registered
`provider_approval_sha256` against the provider JSON supplied at
`QUANTA_PROOF_P07_PROVIDER_CONFIG`. Supply the Native `semantic.provider`
object with the approved HTTPS origin, model and revision, the fixture's
tenant `[1; 16]`, and Source consent for the fixed synthetic text. Keep the
credential in the variable named by `api_key_env`. Fund exactly two
attempts through `total_cost_micros_ceiling` and set both retry limits to
zero. The runner pins the approved config for the selected test. Set
`inputs.provider.path` in the terminal input to a repository-relative file
containing the original approved config bytes.

The case verifies cancellation and shared budget settlement against a
controlled loopback provider, then uses the approved provider for one real
Source embedding and one ranked query. The next request must be refused
at the cost ceiling. Both launches use the same release daemon, and the
witness carries the approved provider SHA-256.

## Paired caller and kernel

`just rust-verify-hellgate-cross-repo <Semantica checkout>` uses clean Quanta and
Semantica checkouts, the actual nested Cargo lock/resolver and the two exact
caller/kernel tests. An executable P11 resolves the current passing P10 receipt
on the same Quanta source. It verifies the immutable P10 daemon archive and
executes a private copy with the same SHA-256. The P11 terminal carries the P10
state-root format. An optional `QUANTA_INDEX_SEARCHD_BIN` must match the P10
bytes.

Staged R5 can reuse the daemon recorded by `just rust-capture-release-build
daemon`. Set `QUANTA_RETIREMENT_DAEMON_BUILD` and
`QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256` to the capture directory and its receipt
digest. The selector verifies the compiler stream, Linux executable and current
clean Q source, then the runner pins the verified executable digest. A supplied
`QUANTA_INDEX_SEARCHD_BIN` must match those bytes. Without the recorded build,
staged R5 builds a Native daemon and checks it against `QUANTA_INDEX_SEARCHD_BIN`.
Provide `QUANTA_P11_R5_QBC_LANE` with an explicitly registered lane for either mode.
If the lane is new, admit it through the producer's
`scripts/quanta-build-cli lane-token issue --lane <lane> --reason <reason>` and
pass the returned token through `QUANTA_BUILD_LANE_TOKEN`. Missing registration
refuses before the recipe selects a daemon or starts a build.

The QBC metadata adapter reads the same auxiliary invocation's output using
its nonce, new run ID, exact command, stable owner status/file identity and
clean source/manifest/lock checks. This establishes dependency resolution only.
The recipe prefixes Quanta's Python import path, and its regular `tools`
package keeps imports within this checkout even when the paired working
directory contains a different package with the same name.
Selected Nextest inventory and execution retain QBC's immutable completion
locator and receipt checks. Effective exit125, source drift, missing completion
or output replacement cannot be treated as success.
The Python runner passes its verified custody copy to QBC as the daemon path
for both phases, including when the runner is invoked directly.
In both modes, the caller hashes the running Linux executable before and
after restart. The runner checks both observations against the pinned SHA-256,
binds the witness to the caller run's nonce, and rechecks its bytes before
publishing the capture index.

The typed selected-run archive is also used by the default recipe. Set
`QUANTA_P11_R5_EVIDENCE_ROOT` to a fresh absolute external path to choose its
location; otherwise the recipe retains it under a new `/private/tmp` directory.
The external index records `runner-candidate-only`. With an executable
registered pair, the Linux producer verifies each original QBC list/run
locator, receipt, stdout and stderr for both caller and kernel. It checks the
physical receipts again, closes the resolver and daemon checks and binds the QBC
toolchain digest and selected Cargo features before signing. After signing it
rechecks the source pair, P10 prerequisite and staged captures under the proof
store's write lock. It then publishes a terminal input and prints its path
as `paired-r5-release-terminal`. After the recipe succeeds, publish from the
Quanta checkout using that terminal path and the same Semantica checkout.

```sh
uv run --frozen python -B tools/ci/write-proof-manifest.py \
  --proof-id p11-cross-repo-cutover --terminal-input "$terminal_path" \
  --paired-checkout "$semantica_root"
```

Publication and archival replay verify the signature, exact tests, source pair, active
Cargo lock, release daemon, original QBC bytes and prerequisite receipts.
