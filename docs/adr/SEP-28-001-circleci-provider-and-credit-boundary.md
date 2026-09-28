# SEP-28-001: CircleCI provider and credit boundary

Status: accepted for repository configuration; hosted activation pending.

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
reachable. This static binding does not prove that a CircleCI project or
trigger exists.

`quanta-index` is private. The CircleCI Free plan has a finite monthly credit
pool for private builds; keep the heavy workflow manual, and review actual
credit usage before adding more automatic jobs. Credit exhaustion blocks runs
on the Free plan and must be reported as missing hosted verification, not a
passing check.

## Activation and limits

The CircleCI account available on 2026-09-28 is not a member of
`gh/josongmin`, so `circleci project create quanta-index --org gh/josongmin`
was refused. The repository is not yet connected to a CircleCI project.
Connect the GitHub repository to CircleCI, then configure triggers for
default-branch pushes and PR updates. Verify a run on the exact commit and
the emitted test-authority artifact before calling hosted CI active. A merge
queue or scheduled correctness claim additionally requires its own observed
trigger and run.

The former GitHub-only proof bundle dispatch, P00 hosted manifest, sanitizer,
Miri, mutation, dependency and parity jobs are not reproduced by this CircleCI
config. Their local commands remain available. Their absence from hosted
CircleCI is an explicit coverage gap, not an implied pass.
