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
project and organization `is_running_disabled` were false, and both GitHub App
triggers were enabled. These observations locate the first failure before
repository commands. They do **not** identify exhausted credits, executor
entitlement, provider scheduling, or GitHub status delivery as the cause.

Separately, `just rust-module-cycles` failed on two IPC cycles at that commit.
Once jobs start, `verify-python` reaches this check through `just rust-policy`.
The source failure and pre-start hosted failure require separate closure.

## Execution and status repair

1. Inspect CircleCI Plan Overview/Usage for the exact organization, job-page
   error banner, machine executor entitlement and provider incident details.
   Record the observed cause and action; do not infer credit exhaustion from
   the one-second failure alone.
2. On one exact commit, observe checkout and command output for both regular
   jobs. Confirm that GitHub's two `ci/circleci` contexts reach terminal states
   for the same SHA. A CircleCI `failed` workflow with GitHub `pending` remains
   a status-delivery defect, even when the build itself fails correctly.
3. After source fixes, require both jobs to pass and validate the emitted
   test-authority artifact for that SHA. No local check, static workflow guard,
   earlier SHA, or manually posted commit status substitutes for the hosted run.
4. GitHub's branch-protection and rulesets APIs returned a private-repository
   plan restriction on 2026-10-03. Until an enforceable repository rule is
   available and observed, the maintainer must check the exact SHA and hosted
   artifact before calling `main` qualified or promoting a release. A direct
   push to `main` is publication, not qualification.

## Former Actions-only coverage

The current CircleCI regular job covers format, clippy, full-workspace nextest
and policy/tooling checks. Its explicit heavy job covers nextest and four
bounded fuzz targets. The following are **not** inherited from disabled GitHub
Actions. Each owner must either port a reachable CircleCI rail with exact-source
output or run the registered local/qualified-host rail and state its narrower
authority. A local result must not be labeled hosted CI.

| Coverage | Owner | Decision needed before claim |
| --- | --- | --- |
| Proof-bundle dispatch and P00 hosted manifest | release-proof / tools CI | Required for the S21-13 release aggregate; port or retain `NOT_RUN` for the hosted slot and execute the registered exact-pair proof independently. |
| Miri and cargo-careful | contract/core owner | Select bounded targets and host/toolchain; record result when the release contract selects this evidence. |
| TSan and ASan | semantic/searchd and IPC owners | Bind target, platform and race/memory-safety claim; QIT-04's native race-detector evidence remains open until executed. |
| Mutation and unused-dependency checks | QIT-07 / tools CI | Select owner targets, survivor policy and dependency scope; do not infer coverage from configured jobs. |
| Pinned model parity | embedding owner | Run the exact pinned model/reference pair before a parity claim; artifact bytes and model identity must be bound. |

Closure requires observed terminal runs and artifacts on the selected final
source, plus explicit outcomes for every selected coverage row. The absence of
a selected rail is `NOT_RUN`, not GREEN.
