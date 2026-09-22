# Recorded agent-outcome A/B/C evaluator

This tool reads completed agent trajectory records from JSONL. It does not run
agents, generate trajectories, infer missing test outcomes, or substitute
synthetic measurements. Each nonempty line is one arm of one paired trial.

```sh
python3 tools/benchmark/agent_outcome/__main__.py validate /path/to/recorded.jsonl
python3 tools/benchmark/agent_outcome/__main__.py summarize /path/to/recorded.jsonl
```

Both commands exit `0` only after every row and every A/B/C pair validates.
Invalid/missing data exits `2` with a machine-readable error on stderr and no
summary on stdout. `summarize` emits aggregate metrics and per-pair metrics as
JSON; redirect stdout to a file if an immutable receipt is needed. The output
includes `input_sha256` of the exact JSONL bytes. It makes no claim that the
submitted recording or human evidence labels are authentic; the capture system
must retain its source logs and test-run receipts under that digest.

## JSONL row contract (`schema_version: 1`)

Every row has exactly these fields:

| Field | Meaning |
| --- | --- |
| `task_id` | Stable task identifier, reused only with the same task digest and checkout commit |
| `task_digest` | `sha256:<64 lowercase hex>` of the frozen task specification |
| `checkout_commit` | Full 40-character lowercase Git commit of the task checkout |
| `trial_id` | Shared repetition/seed identifier for A, B and C |
| `arm` | Exactly `A`, `B` or `C`; each arm has the fixed `arm_kind` below |
| `arm_kind` | Fixed mapping: `A=no_index`, `B=production_router`, `C=oracle_gold_context` |
| `arm_config_digest` | Digest of the arm-specific index, retrieval and routing configuration; distinct across A/B/C and stable across all trials |
| `model_id`, `model_revision`, `model_config_digest` | Model and exact inference configuration |
| `scaffold_digest` | Hash of the frozen agent harness/prompt/scaffold |
| `budget` | `max_total_tokens`, `max_tool_calls`, `max_elapsed_ms` (positive integers), `max_cost_usd` (nonnegative JSON number) |
| `baseline_tests` | Nonempty map of immutable test IDs to `pass`/`fail`, with both kinds present |
| `post_tests` | Same test IDs, each `pass`/`fail`; no omitted/skipped/unknown results |
| `outcome_status` | Must be `completed`; timeout, crash or missing verification refuses the pair |
| `trajectory` | Ordered event array described below |
| `usage` | `input_tokens`, `output_tokens`, `tool_calls`, `elapsed_ms` (nonnegative integers), `cost_usd` (nonnegative JSON number) |

`trajectory` begins with `{ "seq": 0, "elapsed_ms": 0, "kind": "start" }`
and ends with a `finish` event. Every event has contiguous `seq` and
nondecreasing `elapsed_ms`:

- `tool_call` has a unique `call_id`.
- `evidence` has a unique `evidence_id`, `source_call_id` pointing to an earlier
  tool call, and boolean `useful`. The first `useful: true` event determines
  first-useful-evidence time. A trial may have no useful evidence; its time is
  `null` and is excluded from the conditional mean, while evidence coverage
  remains visible.
- `other` has no extra fields.

The evaluator counts tool calls from events and takes elapsed time from the
final event. Both must equal `usage`. Recorded token count, time, tool calls,
and cost must fit `budget`; cost itself is a captured value, not recalculated
from a changing provider price table. Duplicate keys, unknown fields, NaN,
Infinity, bad digests, inconsistent tests, incomplete pairs, mixed model,
scaffold or budget within a pair, drifting or aliased arm configurations, and conflicting task identity or baseline
across trials are refused.

`fail_to_pass` counts tests that failed at baseline and pass after the run.
`pass_to_pass` counts baseline passing tests that remain passing. A pair is
`resolved` only if all baseline failures pass and no baseline pass regresses.
The summary reports both numerators and denominators, cost, elapsed time,
tool calls and first useful evidence for each arm. Aggregate rates pool test
counts; `paired_resolved` reports win/loss/tie on the same task and trial.
No significance, confidence interval or benchmark-wide claim is inferred
from a small or self-selected recording set.
