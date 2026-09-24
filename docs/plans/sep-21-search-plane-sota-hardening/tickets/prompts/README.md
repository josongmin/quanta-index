# SEP-21 prompt archive and current entrypoints

The original P00-P12 lane prompts were written before several source changes.
They are design history, not an instruction to replay every lane or issue
backdated handoffs. Never paste an old prompt into a new task as current
authority. Freeze source and inspect the registry and implementation first.

Current entrypoints:

- [SEP-21 index](../INDEX.md): architecture and ticket links.
- [Residual work](../FINAL-RESIDUAL-EXECUTION-PLAN.md): current action map.
- [Execution evidence](../EXECUTION-PROGRESS.md): commands for live status.
- [P10 state custody](P10-state-migration.md): current-format operations and
  legacy refusal; the old importer instruction is retired.
- [P12A proof custody](P12A-final-proof-infrastructure.md): review and issue
  proof for the existing aggregate/handoff infrastructure.
- [P12Q final qualification](P12-final-qualification.md): terminal proof
  after every registered dependency and operational receipt is available.

The other lane prompts preserve historical design intent only. Their
prerequisite SHAs, writer boundaries, test counts, and future-tense work
claims require current-source review. `tools/ci/proof-authority.toml` and
`Justfile` define executable proof identity and commands; a prompt cannot
promote a staged node. Deploy, activate, rollback, external provider use, and
real state-root cutover require their own operational authority and receipts.
