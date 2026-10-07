# Test hardening — remaining acceptance

Status: `ACTIVE_RESIDUAL`

Executable test authority, invariant proof roles, workflow bindings, ignored-test
policy and tier receipt machinery are implemented. They are consolidated in
[SEP-27-005](../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
The old `b3140f8`/shared-worktree progress snapshot and worker scaffolding are
retired. Current acceptance is in [the residual board](tickets/00-ticket-status-board.md).

Source owners: `tools/ci/test-authority.toml`, `check-test-authority.py`,
`inventory/wire-surface.toml`, `check-wire-inventory.py`, current model/oracle tests,
GitHub workflows and their actual terminal outputs. A static guard verifies
registration/wiring; it does not prove semantic independence or execute Rust.
External ingress/provider, full lifecycle/crash/concurrency and production-scale
proof remain separate. Historical bodies are in [the plan archive](../../ARCHIVE-INDEX.md#historical-record-recovery).
