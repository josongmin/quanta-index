# SEP-28-001: CircleCI provider and credit boundary

Status: accepted; CircleCI project and triggers registered, hosted run pending.

## Decision

GitHub Actions is disabled at the GitHub repository setting. Its three workflow
files have no automatic push, pull-request, merge-queue, or schedule triggers.
They remain as historical manual definitions and are not current CI authority.

`.circleci/config.yml` owns the replacement verification definition. The
default `regular` workflow runs tooling contracts, prompt-manager and Rust
policy checks, benchmark-control contracts, Semgrep, tracked agent-output
validation, fmt, clippy, and full-workspace nextest. It emits a legacy
test-authority receipt only for an observed PR or `main` run. The `run_heavy`
pipeline parameter defaults to false; setting it true selects the manual
full-workspace nextest and four bounded fuzz targets. No heavy schedule is
declared. `tools/ci/test-authority.toml` binds the selected commands to the
CircleCI config, while the guard checks that the jobs and fail-closed steps are
reachable. Static binding alone does not prove that a hosted run passed.

`quanta-index` is private. The CircleCI Free plan has a finite monthly credit
pool for private builds; keep the heavy workflow manual, and review actual
credit usage before adding more automatic jobs. Credit exhaustion blocks runs
on the Free plan and must be reported as missing hosted verification, not a
passing check.

## Activation and limits

The CircleCI account available on 2026-09-28 is not a member of
`gh/josongmin`; that legacy organization slug refused project creation. The
existing CircleCI-native organization `circleci/Q3G2VbitoZmaQSKihvptcF`
has a GitHub App connection that can list `josongmin/quanta-index` and its
`main` branch. The CLI created project `a705c75e-6631-4e99-afca-ada4e0040a4b`
and pipeline definition `38e6fc99-c983-4c03-bb02-da8d53bad4ae`, with
`.circleci/config.yml` and checkout both bound to GitHub repository ID
`1247685100`. `.circleci/info.yml` binds this checkout to the native project
slug for subsequent CLI commands.

Two enabled GitHub App triggers select default-branch pushes and pushes to
branches with an open PR; there is no schedule or all-branches push trigger.
The project enables redundant-workflow auto-cancel and disables secret
environment variables for fork PR jobs. Registration and repository access
were verified through the CLI, but the project had zero hosted runs at
registration. Verify a run on the exact commit and its emitted test-authority
artifact before calling hosted CI active. A merge queue or scheduled
correctness claim additionally requires its own observed trigger and run.

The former GitHub-only proof bundle dispatch, P00 hosted manifest, sanitizer,
Miri, mutation, dependency and parity jobs are not reproduced by this CircleCI
config. Their local commands remain available. Their absence from hosted
CircleCI is an explicit coverage gap, not an implied pass.
