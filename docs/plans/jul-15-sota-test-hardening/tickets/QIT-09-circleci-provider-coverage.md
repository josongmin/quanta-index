# QIT-09 — CircleCI execution and former Actions coverage

Status: `OPEN`. Owner: repository tools/CI maintainer. The contract/IPC,
semantic/searchd, embedding and release-proof owners retain their respective
behavioral evidence. Authority: [SEP-28-001](../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md)
and [S21-13](../../sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md).

## Observed failure and RCA boundary

On 2026-10-03, CircleCI regular runs for `main` including commits `a867f47b`
and `549f83f7`
ended `failed` in about one second. Neither `verify` nor `verify-python` reached
checkout; job detail reported `Task information unavailable` with an unset
start time. GitHub kept both `ci/circleci` commit contexts `pending` after the
CircleCI workflow ended, including on `549f83f7`. `circleci config validate` accepted the config;
project and organization `is_running_disabled` were false, project
`can_set_github_status` was true, and both GitHub App triggers were enabled.
The draft PR head `3a805980` repeated the same one-second pre-start failure
and pending contexts. These observations locate the first failure before
repository commands. They do **not** identify exhausted credits, executor
entitlement, provider scheduling, or GitHub status delivery as the cause.
CircleCI's [Ubuntu 24.04 image catalog](https://circleci.com/developer/machine/image/ubuntu-2404)
still lists the configured `2026.05.1` tag, so a removed image tag is not
supported by the available evidence.

The observed transition is bounded: the `22320380` main run created at
2026-10-02 07:20 UTC ran both jobs for over five minutes and GitHub recorded
terminal `failure` for each context. The later `f4bc41c` main run created at
19:46 UTC ended in one second, with both contexts still `pending`. The
intervening time has no observed run in this audit. This narrows the provider
investigation without assigning a cause or attributing every earlier failure
to the same pre-start condition.
The [public CircleCI status page](https://status.circleci.com/) listed no
Linux Machine incident for that window when checked on 2026-10-03; its
absence cannot exclude an account-specific or unposted provider failure.

The same organization also ran `quanta-memory-platform` at 2026-10-03
06:29 UTC (`5c560712`): its `full-extraction` job ended in one second with
an unset start time and the same `Task information unavailable` step. Its
2026-09-26 run `0f2a0855` succeeded with the same
`ubuntu-2404:2026.05.1` machine image and `large` resource class. The
quanta-index PR head `c90d32d0` repeated the pre-start failure at 06:29 UTC
(`3bb3ed25`), with both GitHub contexts still `pending`. A repository-only
source or config fault does not explain the cross-project symptom; organization
credit/entitlement, shared machine execution and provider scheduling remain
unresolved candidates. The inspected CircleCI run and job API responses do
not expose the billing balance or a more specific task rejection reason.

Separately, `just rust-module-cycles` failed on two IPC cycles at that commit.
Local `just rust-policy` replay also exposed stale ingest enum inventory paths,
two manifest-format inventory versions, an ignored-test exception path and a
generic public error without `Display`/`Error` implementations. The branch
repairs those source gates; local policy passing does not replace a hosted run.
`verify-python` reaches them through `just rust-policy` once its job starts.
The source failures and pre-start hosted failure require separate closure.

## Execution and status repair

1. Inspect [CircleCI Plan Overview/Usage](https://circleci.com/docs/guides/plans-pricing/credits/)
   for the exact organization, job-page
   error banner, machine executor entitlement and provider incident details.
   Record the observed cause and action; do not infer credit exhaustion from
   the one-second failure alone.
2. On one exact commit, observe checkout and command output for both regular
   jobs. Confirm that GitHub's two `ci/circleci` contexts reach terminal states
   for the same SHA. A CircleCI `failed` workflow with GitHub `pending` is an
   unresolved status-completion gap. Determine whether the provider treats
   pre-start cancellation as a skipped job or failed job before assigning the
   fault to the GitHub integration.
3. After source fixes, require both jobs to pass and validate the emitted
   test-authority artifact for that SHA. No local check, static workflow guard,
   earlier SHA, or manually posted commit status substitutes for the hosted run.
4. GitHub's branch-protection and rulesets APIs returned a private-repository
   plan restriction on 2026-10-03. Until an enforceable repository rule is
   available and observed, the maintainer must check the exact SHA and hosted
   artifact before calling `main` qualified or promoting a release. A direct
   push to `main` is publication, not qualification.

## Interim local regular replay

While hosted jobs fail before checkout or remain pending, replay the command
blocks of both `.circleci/config.yml` regular jobs on one clean, fixed HEAD.
Run `verify`'s format, full-workspace Clippy, nextest inventory and nextest
execution, then `verify-python`'s module snapshot, tooling tests, prompt and
Python lints, Rust/benchmark policies, Semgrep, tracked agent-output
validation and P00 owner tests. Keep disposable nextest events and inventory
outside the checkout and report the exact command, host, selected/executed
count and exit status in the PR. A macOS replay does not establish Linux
Machine behavior, terminal GitHub checks or a hosted test-authority artifact.
The hosted and release gates above remain open until their own evidence exists.

## Former Actions-only coverage

The current CircleCI regular job covers format, clippy, full-workspace nextest
and policy/tooling checks. Its explicit heavy job covers nextest and four
bounded fuzz targets. The following are **not** inherited from disabled GitHub
Actions. Each owner must either port a reachable CircleCI rail with exact-source
output or run the registered local/qualified-host rail and state its narrower
authority. A local result must not be labeled hosted CI.

| Coverage | Owner | Requirement and closure |
| --- | --- | --- |
| Proof-bundle dispatch and P00 hosted manifest | release-proof / tools CI | **Required for S21-13 release aggregate.** CircleCI's P00 owner tests do not emit the former hosted manifest. Port the exact-pair bundle gate or keep the hosted slot `NOT_RUN` and execute the registered proof on its selected qualified host. |
| Native race detector | semantic/searchd owner | **Required by QIT-04.** Select the affected concurrent owner targets and native host; TSan can supply detector evidence but does not replace the deterministic linearization oracle. |
| Mutation and fuzz coverage | QIT-07 / query and correctness owners | **Required for selected risk owners.** Record target selection, thresholds, survivor decisions and minimized inputs. The four CircleCI heavy fuzz targets alone do not close the QIT-07 mutation/fuzz matrix. |
| Guarded public API and changed-line coverage | contract/SDK and correctness owners | **Required when the respective public-surface or selected production-line policy applies.** `rust-public-api` and the former PR 90% changed-line coverage job are absent from CircleCI; select exact-source local/qualified-host rails or port them before claiming those gates. |
| Miri, cargo-careful and ASan | contract/core and IPC owners | **Conditional diagnostic evidence.** Select a bounded target, platform and claim explicitly; their absence is `NOT_RUN` when selected, not a general release failure by itself. |
| Rustdoc, bench build, LLVM lines, cargo-machete/udeps | tools CI and affected crate owners | **Unassigned as mandatory CI gates.** Decide each gate's owner and promotion scope; retain the command as local diagnostics until a selected release or PR contract makes it required. CircleCI's Rust 1.92 nextest/clippy does not prove the former `--all-targets` MSRV no-run target selection. |
| Pinned model parity | embedding owner | **Required before an encoder-parity claim** under SEP-26-002. Run the exact pinned model/reference pair; bind artifact bytes and model identity. |

Closure requires observed terminal runs and artifacts on the selected final
source, plus explicit outcomes for every selected coverage row. The absence of
a selected rail is `NOT_RUN`, not GREEN.

## Release evidence remaining after CI restoration

Restoring regular CircleCI does not close the original QIT-09 release scope:
weekly/release evidence hierarchy, artifact retention and the complete release
proof DAG remain open. The release-proof owner follows
[S21-13](../../sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md)
and the registered proof-authority graph, with exact-source manifests and
terminal results for every selected dependency. A passing PR job cannot be
promoted into a weekly or release verdict by changing this ticket's status.
