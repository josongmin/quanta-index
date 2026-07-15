# Jul 15 SOTA Test Hardening

Status: `active plan; not a closure claim`

Purpose: turn the existing broad test suite into an auditable proof system. The
required unit is an invariant with one owner and independently meaningful proof
roles, not a growing test count.

Truth snapshot:

- committed baseline: `b3140f8`;
- the live worktree also contains uncommitted QIT-00/QIT-02/QIT-09 changes;
- this packet records those as `in-flight` until their owner-local tests and CI
  receipts are green and the changes are committed;
- external ingress, live-provider, and production-scale evidence are never
  promoted from repo-local green.

The target state has five authorities:

1. invariant -> owner -> executable proof map;
2. model-based lifecycle plus crash-consistency evidence;
3. independent differential and metamorphic query oracles;
4. risk-based coverage, mutation, and fuzz gates;
5. PR, merge, nightly, and weekly/release receipts with explicit promotion.

Read order:

1. [WORKER_START_HERE.md](WORKER_START_HERE.md)
2. [NO_GO_RULES.md](NO_GO_RULES.md)
3. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
4. [TEST_INVARIANT_MATRIX.md](TEST_INVARIANT_MATRIX.md)
5. [CI_TIER_MATRIX.md](CI_TIER_MATRIX.md)
6. [tickets/00-ticket-status-board.md](tickets/00-ticket-status-board.md)
7. [tickets/TICKET_DEPENDENCY_DAG.md](tickets/TICKET_DEPENDENCY_DAG.md)

Scope:

- QIT-00 through QIT-09 only;
- source and CI evidence supersede this plan;
- no product behavior change is authorized solely by this document.
