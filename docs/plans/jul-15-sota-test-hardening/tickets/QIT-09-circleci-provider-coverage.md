# QIT-09 — Hosted CI and selected coverage acceptance

Status: `ACTIVE_RESIDUAL`. Owner: tools/CI plus affected contract/IPC,
semantic/searchd, embedding and release owners.
[SEP-28-001](../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md)
owns the CircleCI decision; [.circleci/config.yml](../../../../.circleci/config.yml)
and [test authority](../../../../tools/ci/test-authority.toml) own executable rails.
[S21-13](../../sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md)
owns weekly/release/DAG promotion. This cleanup makes no live provider/billing claim.

## Completed main scope

`VERIFIED` on 2026-10-07: source `c6a9120d` required docs/static/Python/tests/bench/
verify GitHub contexts succeeded. CircleCI job2011's original receipt names that
revision; downloaded raw/inventory SHA-256 matches and terminal suites total
4,241 passed, zero failed. The source-bound completion and commands live in
[OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints).
Aux PR coverage was pending in a separate workflow. Do not reopen completed main
CI because that auxiliary status or a later source is pending.
Main `9def97ac` contains the receipt context/tier preflight implementation.
The following `8642fa9b` regular main scope also completed: owners rechecked all
six terminal jobs, 4,268 passed/zero failed/30 skipped, and matching source,
command, inventory and raw/result digests. This closes the existing CI/F15
follow-up at that SHA. Later cache-test extensions and separately selected
PR/release scopes retain their own acceptance.

## Remaining hosted acceptance

1. Observe the selected final SHA's regular `verify`/`verify-python` checkout,
   commands, terminal result and emitted test-authority/P00 artifacts. Both jobs
   must pass for that hosted scope; config validation, local passes and older runs
   cannot issue it. PR coverage separately needs its selected passing job.
2. Require matching terminal GitHub `ci/circleci` contexts **selected for the
   claimed scope**. A pending required context leaves that scope unresolved;
   auxiliary PR contexts do not undo completed regular main jobs. For a
   pre-start refusal, record the actual job API/banner/plan or provider cause;
   one-second duration alone cannot identify credits, entitlement or scheduling.
3. Separate provider admission failures from source lint/test failures. The
   historical campaign observed both credit-blocked runs and later commands
   executing/failing. Those source/credit/run identities remain historical; each
   repaired source needs its own relevant final terminal proof.
4. Observe actual promotion-rule enforcement where claimed. If unavailable, the
   maintainer must check exact SHA and hosted artifacts before promotion. Do not
   infer current visibility, plan, credit balance or protection from old API reads.

When hosted execution is unavailable, replay the selected regular command blocks
from `.circleci/config.yml` on one declared source/host, using current catalogs.
Record command, selected/executed counts, platform exclusions and terminal result;
disposable inventory/events stay outside the checkout. A macOS/local development
rail retains that scope; Linux Machine, hosted status/artifact and release gates
require their own actual results. Resolve affected source failures before promotion.

## Selected coverage beyond regular jobs

Regular/static enrollment and the manual heavy four-fuzz definition are existing
configuration. Select each required scope from current authority; disabled former
Actions jobs do not transfer evidence automatically.

| Coverage / owner | Acceptance |
| --- | --- |
| Release proof bundle / tools | Required by S21-13: actual source-bound P00 plus exact-pair bundle/graph or explicit missing selected slot. A configured producer is not a hosted receipt. |
| Native race detector / semantic-searchd | Required by QIT-04: affected concurrent targets and native host; detector output supplements the deterministic linearization oracle. |
| Mutation/fuzz / QIT-07 risk owners | Actual target selection, thresholds, survivor dispositions/expiry and minimized inputs. Four heavy fuzz targets do not close the broader matrix. |
| Public API / contract-SDK | Execute `rust-public-api` on exact source when selected or port it to hosted CI. PR-only configured 90% changed-line coverage needs its own passing run. |
| Miri/cargo-careful/ASan / core-IPC | Conditional bounded diagnostic target/platform/claim. Missing selected execution stays `NOT_RUN`; no universal mandatory gate is inferred. |
| LLVM lines/cargo-udeps / tools-crates | Decide required owner and promotion scope; retain local diagnostics until selected. MSRV/Rustdoc/bench/machete regular definitions still need actual terminal results. |
| DSL/systems benchmark / benchmark | Selected latency/load claims require the dedicated qualified host and registered evidence. Shared CircleCI Machine, a schedule name or generic heavy job cannot establish that host/workload. |
| Pinned model parity / embedding | Selected encoder claim binds actual pinned model/reference bytes and execution; hash/spy results do not qualify it. |

The [QIT residual board](00-ticket-status-board.md) retains all quantitative
proposals and independent owner scopes. Decide each missing selected rail's
hosted or explicitly narrower local authority; zero-selected/skipped/stale or
missing evidence cannot close it. A passing PR job does not issue weekly/release
hierarchy, retention or the complete proof DAG by changing a status label.

## History

Exact pre-start errors, credit rejection, subsequent source failures, pipeline
IDs/URLs, source SHAs and local replay decisions are recoverable through
[the plan archive](../../../ARCHIVE-INDEX.md#historical-record-recovery).
They are not current provider state or current-source passing receipts.
