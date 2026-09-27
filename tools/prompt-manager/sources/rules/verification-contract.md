## Verification Contract

Use `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN`, or `NOT_APPLICABLE` for requested
verification scopes, not every statement. `FAILED` means an executed check
failed; `BLOCKED` means a necessary input is absent or invalid; `NOT_RUN` means
the required check was not executed.

Scale verification to the claim and risk:

- Routine edits, audits and focused tests: inspect relevant source/dirty state,
  run the narrowest decisive check, and report command, observed result and
  scope/limits. Terminal output is sufficient; no saved log, snapshot, receipt,
  digest, environment inventory or clean checkout is required by default.
- Formal replay, release or qualified benchmark claims: bind relevant source,
  inputs, dependencies, config, binaries and environment. Record artifact
  paths/digests only where the selected contract requires them. Recheck affected
  evidence when relevant inputs change; unrelated dirty work does not invalidate
  a focused result.

Keep expected outcomes independent of the implementation: fixed goldens, public
contracts, reference implementations, invariants or observable behavior. Do not
invent success from missing, stale, malformed, partial or interrupted results.
Compilation is not test success; focused tests are not full-suite qualification;
local proof is not E2E; benchmark completion is not comparison validity.

Do not create or commit one-off logs, snapshots, receipts, probes or executables
inside the repository unless explicitly requested. If needed, put disposable
output outside the checkout and summarize results in the existing ticket.
Verification does not itself authorize new evidence files. Reusable product
tests/tools are distinct from per-run evidence dumps.
