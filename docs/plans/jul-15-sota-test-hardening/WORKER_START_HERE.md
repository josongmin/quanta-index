# Worker Start Here

## Entry protocol

1. Recheck `git status --short`, `git log -1 --oneline`, and the relevant
   workflow/test source. This is a shared worktree; do not overwrite in-flight
   changes belonging to another owner.
2. Select one ticket from the status board whose dependencies are closed.
3. Freeze an owner-local failing rail before implementation. A broad daemon or
   workspace rail is not a substitute for that RED witness.
4. Change the authority owner and its test/guard in the same patch. Do not add
   an unconnected test file or a catalog-only claim.
5. Run the cheapest exact rail first, then the ticket's required tier rails.
   Use `just` or `./scripts/cargow`; record command, scope, exclusions, and
   result in the ticket/receipt.

## Current routing

- QIT-00: `tools/ci/test-authority.toml` and
  `tools/ci/lint/check-test-authority.py`.
- QIT-01: contract/IPC wire fixtures and SDK consumer proof.
- QIT-02/QIT-03: `quanta-index-semantic` generation build, persistence, and
  restart seams.
- QIT-04: owner-local synchronization state, then TSan as a broad detector.
- QIT-05: lexical, semantic, hybrid, and dispatcher pure reference oracles.
- QIT-06: `quanta-index-searchd-runtime/tests/sdk_frontdoor.rs` as the
  front-door boundary; internal API helpers cannot close the ticket.
- QIT-07/QIT-09: `Justfile`, correctness tooling, and GitHub workflow receipts.
- QIT-08: searchd harness and measured corpus fixtures after correctness
  invariants are independently closed.

## Required closeout evidence

- owner-local test name and result;
- command and exact tier;
- failing condition that the test would catch;
- receipt/artifact location when the rail is CI-owned;
- remaining external, performance, or platform boundary.

Do not mark a ticket `done` because it compiles, because an existing broad
suite is green, or because a test is present but never executed by its declared
rail.
