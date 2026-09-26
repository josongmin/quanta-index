# SEP-26-002 — Retrieval Observation, Experiment and Default Policy

Status: `Accepted`

Decided: 2026-09-27

Source campaign: RBR-01, RBR-03, RBR-07, RBR-09, RBR-10 and RBR-11

## Context

Earlier diagnostics conflated requested settings with executed work, omitted unobserved values as zero and combined
durable receipts with transient stage timing. Comparator profiles and performance experiments also lacked one common
rule for default changes.

## Decision

### Observation semantics

- `executed` and `contributed` are distinct for every lane.
- Missing observations remain explicit null or typed absence; zero is a measured value.
- Query-stage observation has the exact startup values `enabled` or `disabled`. Disabled mode omits query-stage clocks,
  trace allocation and storage while preserving operational and deadline clocks.
- Ingest stage observations are transient response data. Durable publish and activation receipts do not absorb timing.
- Server stage time, SDK wall time and runner sidecar serialization time remain separate measurements.

An on/off comparison uses the same request, source, binary, configuration and roomy deadline, then verifies result,
ordering, page, cursor and failure equivalence. Tight-deadline effects and overhead are separate measurements.

### Comparator profiles

The comparator has four explicit modes: `native-default`, `hybrid-no-rerank`, `lexical-only` and `semantic-only`.
Cold, warmup and measured phases dispatch through the same pinned profile. Records bind requested and actual alpha,
rerank state, lane counts, candidate depth, query digest, profile digest, upstream function/source identity and library
version. Endpoint alpha values do not imply that only one lane executed.

### Semantic and ANN proof

Encoder parity uses full vectors, pinned model/tokenizer/config identities, canonical adversarial inputs, norms and
pairwise directional checks. ANN diagnosis compares production-served results with an independent exhaustive scan over
the same fully bound row set. Short-result, filtering, pagination and churn cases remain separate. A bounded proof does
not establish general recall, quality or performance.

### Product defaults

Experimental hybrid fetch floor accepts only `25`, `50` or `100`; the default remains `100`. Requested policy,
effective daemon configuration and the actual initial-fetch trace must agree. Refill, ceiling and generation pinning
remain unchanged.

Chunking, ranking, fetch and ingest optimization follow a finite development matrix. A default changes only after the
predeclared effect metric, quality guard, failure accounting and final holdout gate in
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md). Missing or inconclusive evidence preserves the
existing default.

### Resource accounting

Process-tree discovery follows live parent-child connectivity before applying RSS policy. A live zero-RSS connecting
node cannot hide positive-RSS descendants. Malformed and duplicate PIDs, an unobserved root and cleanup failure are
explicit failures. Platform APIs that cannot bind PID reuse to a stable process-start identity retain that limitation
in their proof scope.

The macOS v1 metric rows contain only positive-RSS processes, so a live zero-RSS
root may be absent from those rows after sampling has observed it. Replay
refuses duplicate emitted PIDs, a per-process peak above the tree peak, and
per-process samples above the global sample count; it does not infer root
observation or PID start identity from the filtered metric rows alone.

## Consequences

- Diagnostic presence is not a performance result.
- Local parity, ANN and process-sampler runs prove only their bound inputs and platforms.
- No experiment may promote a new product default from self-reported summaries, partial runs or filtered successes.

## Historical record

Completed implementation checkpoints and raw artifact references are
recoverable with `git show eff53181:<path>`. Remaining execution is owned by
[the active SEP-26 packet](../plans/sep-26-retrieval-remediation/tickets/INDEX.md).
