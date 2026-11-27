# Execution Admission and Terminal Recovery

Status: `Accepted`

Decided: 2026-11-12

Scope: implemented C0/P09 coordinator, QBC external claim, installed-runner
and paired release execution contracts. Product-body acceptance remains in
[B05](../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md) and the
[Native release ledger](../plans/nov-20-native-cutover-closeout/tickets/08-release-activation.md).

## Context

The completed B01–B04 implementation separates execution intent, canonical
admission, external execution and verified terminal publication. A lost control
response, empty Docker listing or successful input preflight cannot establish
that a product body completed. Recovery must preserve the original attempt
until its identity, admission and actual terminal state are known.

## B01 C0 and coordinator

- Normal completion and restart recovery use the same QBC completed-run reader.
  Bind locator nonce, request/source digests and lane to the dispatch; bind
  receipt path/SHA, run ID and exit to the immutable QBC receipt. An available
  owner result must agree. Success requires that owner's publication commit
  and the exact current body witness. A standalone JSON result is insufficient.
- Persist the prepared nonce, command and log before spawning. The private
  child inherits the coordinator lock and records PID, nonce and execution
  intent before `exec`. Intent alone is not actual QBC admission. Preserve
  damaged/legacy records and isolate their task instead of inventing a result.
- Resume the same prepared attempt only after confirming no execution. The
  known pre-entry busy recovery requires current child exit 75, a dead child,
  no completion/result and the same registered, empty, unlocked canonical
  lane without a target. Probe the canonical exclusive lease before resubmission.
  A previous exit 75 cannot substitute for a new child's unknown exit.
  Live, partial or unconfirmed runs remain unresolved.
- Admission wait and body deadline are separate. Bound child cleanup to the
  process group owned by this attempt; retain ambiguous external work and its
  claim. C0 can be evaluated independently of missing P09 guest memory, an
  empty Docker listing or retryable pre-claim work. Actual shared claims,
  uncertain host observation and live successor reservations still block.

Source: [coordinator](/Users/songmin/.codex/task-evidence/qidx-c0-reboot-recovery.q2kIxd/p09-linux-run/run-c0-p09.py),
[resource observation](/Users/songmin/.codex/task-evidence/qidx-c0-reboot-recovery.q2kIxd/p09-linux-run/host_admission.py).

## B02 QBC external claim

- Under the existing ordinary exclusive lease, publish the immutable v1 claim
  and fsync its parent directory before allowing Docker creation. A link-sync
  failure retains the exact token/external ID for recovery and another sync.
- Clearing requires the same lease and exact token/ID on disk, then unlink and
  parent-directory fsync. Propagate sync failure; do not confirm terminal cleanup.
  After interrupted deletion, verify absence and sync under the same identity
  and an ordinary exclusive lease. Recover a visible claim only by its exact pair.
- Preserve existing v1 formats and shared/exclusive admission. Directory sync
  belongs to this claim boundary; other immutable-writer callers are unchanged.

Source: [QBC external lease](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/quanta-build-cli/quanta_build_cli.py).

## B03 P09 execution and recovery

- Pin the local Unix Docker endpoint and daemon ID before admission observation.
  Remove ambient host/context overrides and use that connection for observation,
  Python execution and the shell. Verify daemon identity at mutation and absence
  confirmation; verify container name, ID, task label and attempt label.
- Persist one token/name and connection; durably publish its QBC claim before
  create intent and Docker creation. Recover lost create/start responses under
  the same token, name and labels. An empty listing does not clear the claim or
  authorize a fresh attempt. Uncertain start recovers the existing container.
- Detached start, wait and inspect establish the actual daemon exit. A control
  timeout is not a container exit. Restart a created container only after
  establishing that the earlier start was unsent/refused; never restart an
  exited container. Preserve the suite 7,200-second, control 30-second and create
  120-second limits with bounded cleanup.
- Record actual exit and logs, confirm removal, durably clear the matching claim,
  then confirm terminal. Uncertain observation returns retryable recovery with
  the same journal. A confirmed daemon exit 75 retains its terminal-failure
  meaning. Cleanup-only does not create a new claim, container or body while
  another owner's reservation holds. Pre-name-claim attempts retain their
  original confirmed container/daemon identity for cleanup.

Source: [installed owner](/Users/songmin/.codex/task-evidence/qidx-c0-reboot-recovery.q2kIxd/p09-linux-run/installed_delta_owner.py),
[container entrypoint](/Users/songmin/.codex/task-evidence/qidx-c0-reboot-recovery.q2kIxd/p09-linux-run/delta-container.sh).

## B04 attempt inputs and CLI

- Require an absolute `--attempt-input` using `p09-installed-delta-attempt/v1`.
  Bind capture SHA, source binding, QBC root/lane/source digest, exact selector,
  runner/support roots, preflight and expected worker/searchd binaries with
  original receipts and public packet. Do not derive current expectations from
  V83 defaults, ambient attempt variables or a previous failure receipt.
- Validate current source and provenance before execution. Carry the selected
  input SHA through child and journal. Bind token-specific suite/QBC receipt SHA,
  run ID, source and body witness to the same attempt; bind the observed private
  searchd bytes and signed worker/environment evidence to the selected binaries.
  Another attempt's receipt, even at the same source, cannot establish success.
- Preflight and create consume the same capture, roots and pinned connection.
  Resolve the first cohort against its selected source without requiring an
  earlier completed run. Bound supplier/process waits. Diagnostics go to stderr;
  successful `--validate-inputs` stdout is one complete JSON value. Validation
  and resolve-only execution do not qualify an installed product body.

Source: the installed owner and container entrypoint above. Supplier builds stay
with their canonical producers; reuse artifacts only when relevant inputs match.

## Reviewed Native worker source pairs

The owned-surface guard pairs the common pending-virtual owner with its reviewed
Native caller. It also checks the constructor method and the existing verified
switch call arguments. The Native caller imports `selected_definition_v222`
from the common owner. Approval applies to the exact reviewed pair; a mismatched
owner, caller or constructor returns to the owning review before supply.
[B05 current guard and worker acceptance](../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md#현재-상태)

## Shared execution and handoff

The B05 coordinator owns the current C0/P09 attempt and consumes the canonical
QBC and cargow admission. Reconcile the coordinator, child, P09 journal, task
lock, QBC claim and pinned Docker daemon before another dispatch. Native and Java
PTA work retain their own source owners and host/VM reservations. Match the
actual source, target, features, recipe, toolchain and binary SHA before reusing
a worker/searchd supply. A new input returns to the owning supplier.

After an interrupted Root run, R16 and Caller/Published resume the recorded
source, lockfile and requested proofs. The resume entry refuses a duplicate
child while the existing SDK or Caller lock is held. The requested CP2 mixed
positive and mutant remain bound to the resumed Published input and terminal.
[F Root execution](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)

The Native release gate can consume the installed P09 receipt for matching
inputs. It also requires its own view, delete/no-op and restart checks. NC-F
owns SDK60, Large, XL and Mixed acceptance. The recorded F1/F2 termination and
lease return remain available for the original cohort.
[B05](../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md#current-resume-contract) ·
[Native acceptance](../plans/nov-12-native-cutover/DOD.md) ·
[F1/F2 evidence](../plans/nov-5-indexing-bugbash/WORKPLAN.md#final-remaining-audit-20261111)

## Paired release execution custody

The P11 paired launcher starts Bash in `-p` mode and clears inherited shell,
Python and Git inputs before checking the exact clean Q/S worktrees. It
checks the S resolver helper against the pinned Git blob and executes a private
copy. Python starts in isolated mode with a fresh cache prefix and bytecode
writes disabled. Resolver and metadata descendants receive neither the signer
socket nor an SSH-agent alias for it. Signed proof publication checks the
destination against the opened directory identity before and after publishing.
On a mismatch it removes only the entries created by that publication.
[Paired launcher](../../scripts/verify-repomap-cross-repo.sh) ·
[Paired result verifier](../../tools/ci/paired_r5_result.py) ·
[Operational proof](../../tools/ci/proof_operational_result.py)

P12A control verification on Q `252ec2ec7` and its recorded runner changes
passed 605/605. Earlier control runs and the source-bound Linux arm64
qualification inputs retain their own recorded scope. Formal P11 needs the
same approved P10 executable and actual paired caller/kernel runs. Deployment,
activation and rollback additionally need the approved installed observation.
[Release acceptance](../plans/nov-20-native-cutover-closeout/tickets/08-release-activation.md) ·
[Runner and proof history](../ARCHIVE-INDEX.md#2026-11-23-native-closeout-history-compaction)

## R25 source-bound supply

The v34 review input preserves the existing Root locks, S and linked-source
lease, then Q admission. Before import, the supplier checks its canonical SDK
root and the 29 pinned paths in the dependency readset. The guest receives the checked
manifest read-only. The original Linux terminal and queued `rustc` observation
share compiler identity bytes read once under the Source lease. The supplier
rechecks those bytes and the Source/spec before accepting the result. A failed
final check withdraws the newly published terminal and result while preserving
the previous and rejected terminal evidence. [R25 supplier ledger](../plans/nov-20-native-cutover-closeout/tickets/02-semantica-runtime-sdk.md)

The recorded v34 control review passed 63/63 Python checks and 29/29 installed
manifest checks. Formal supply still requires the same frozen S/Q/GQLang and
Cargo inputs, effective Linux AArch64 guest settings, and the original Q build
terminal and ELF. The running-size check then uses the Semantica AArch64
libtest, with an 8,388,608-byte worker stack. Its measurement leaves
`upper_bound_proven` false, so acceptance requires both Native bodies and the
remaining TS/JS and Java bodies. An earlier ELF refused Native execution at a
10,773,120-byte path floor. [Source-bound control history](../ARCHIVE-INDEX.md#2026-11-23-native-supplier-and-runtime-history)

## Validation and remaining acceptance

The [recovery index](../ARCHIVE-INDEX.md#2026-11-12-completed-lexical-and-runner-packets)
retains the original implementation/activation evidence and exact ticket
preimages. Those source-scoped regressions do not qualify current Rust bodies.
[B05](../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md) owns current
runner repairs, admission, current-cohort supply and attempt input, installed
P09 exact 1/1, container/claim cleanup and ledger reconciliation. C0 exact 1/1
is already verified for its recorded source; B05 retains that receipt rather
than issuing another C0 execution. Current P09 acceptance does not inherit it.
