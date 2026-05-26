# May 27 DSL Master Closeout + Structural V2

Status: `active-master-packet`
Date: `2026-05-27`
Scope: current-source closeout packet for the remaining DSL whole-program work.

This packet supersedes the remaining active ownership from:

- [../may-25-lexical-enhancement/README.md](../may-25-lexical-enhancement/README.md)
- [../may-26-indexing-residue-tasks/README.md](../may-26-indexing-residue-tasks/README.md)
- [../may-27-structural-dsl/README.md](../may-27-structural-dsl/README.md)

Historical tickets stay in place for evidence. Active ownership is re-anchored
through [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md).

---

## 1. Scope Lock

This packet owns the remaining DSL work as a whole program:

- structural V2 on the native route
- Sourcegraph structural V2 on top of native truth
- structural bridge / CodeQL follow-on
- bounded-label observability normalization
- `E2E-07` performance and chaos closeout
- producer-side history / runtime / structural emission handoff proof

Explicitly excluded:

- semantic ownership redesign (`SEM-OWN-*`)
- semantic / hybrid public contract redesign
- SDK-only external entry redesign outside the existing DSL runtime packet

## 2. Current Source Truth

Current source-backed state, not stale packet prose:

- native structural execution already ships the truthful subset over
  producer-authored parse-tree authority
- structural dispatcher already accepts structural-only boolean trees over
  `match { ... }` leaves plus executable `repo:` / `file:` / `lang:` filters;
  mixed lexical / structural boolean remains typed fail-closed
- Sourcegraph structural lowering now ships quoted bodies, boolean composition,
  typed-hole surface parity, and regex bodies by rewriting only into native
  executable structural semantics; broader non-executable filter breadth stays
  typed fail-closed
- producer-side history / runtime / structural channel ops already exist in the
  contract and are wired into search-side ingest/readiness paths
- bounded query observability already exists as a closed-dimension
  `MetricSample + Dimensions` sink; remaining work is route coverage, proof, and
  doc drift closure rather than inventing a new metrics subsystem
- `select:path` and `select:content.match` already have owner proof; remaining
  work is matrix re-anchoring rather than executor bring-up

This packet therefore treats the remaining work as:

- new behavior where source truly lacks it
- proof, surface normalization, and ownership correction where source already
  moved ahead of docs

## 3. Direction

The direction remains `breaking-first` and `fail-closed`.

Required:

- producer-authored authority remains the only structural truth source
- unsupported shapes stay typed and explicit
- public acceptance widens only when executor truth and proof exist
- metrics stay bounded; no free-text dimensions

Forbidden:

- search-side reparsing as a structural authority source
- lexical fallback disguised as structural execution
- widening SG structural beyond native executable semantics
- exporter/productization work beyond the current typed sink

## 4. Active Ticket Lanes

- [tickets/MSTR-00-truth-freeze-and-historical-map.md](tickets/MSTR-00-truth-freeze-and-historical-map.md)
- [tickets/MSTR-01-surface-and-matrix-closure.md](tickets/MSTR-01-surface-and-matrix-closure.md)
- [tickets/MSTR-02-producer-handoff-end-to-end.md](tickets/MSTR-02-producer-handoff-end-to-end.md)
- [tickets/MSTR-03-structural-core-v2.md](tickets/MSTR-03-structural-core-v2.md)
- [tickets/MSTR-04-sourcegraph-and-bridge-v2.md](tickets/MSTR-04-sourcegraph-and-bridge-v2.md)
- [tickets/MSTR-05-observability-and-perf-chaos.md](tickets/MSTR-05-observability-and-perf-chaos.md)

## 5. Dependency Order

Execution order is fixed:

1. `MSTR-00` truth freeze + historical ownership map
2. `MSTR-01` surface / capability matrix closure
3. `MSTR-02` producer handoff end-to-end proof
4. `MSTR-03` structural core V2
5. `MSTR-04` Sourcegraph + bridge V2
6. `MSTR-05` observability + perf / chaos closeout

Rules:

- `MSTR-01` cannot claim rows beyond current owner proof.
- `MSTR-03` must land native semantics before `MSTR-04` widens the translator.
- `MSTR-04` cannot introduce lexical fallback.
- `MSTR-05` must preserve the existing metric dimension schema.

## 6. Near-Term Exit Criteria

This packet is materially complete only when all are true:

1. structural-only boolean composition executes natively with deterministic
   candidate selection
2. mixed lexical / structural boolean remains typed fail-closed
3. current capability matrix rows align with actual proof for `select:path`,
   `select:content.match`, explain / bridge / runtime rows, and route-specific
   typed failure behavior
4. producer-side history / runtime / structural emission has real positive proof
   rather than fixture-only evidence
5. structural / bridge / history / runtime query paths emit bounded metrics with
   no raw query text or file path leakage
6. Sourcegraph structural route and structural bridge packet stay aligned with
   native semantics, including regex-body lowering without lexical fallback

## 7. Historical Evidence

- [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md)
- [../may-27-structural-dsl/README.md](../may-27-structural-dsl/README.md)
- [../may-26-indexing-residue-tasks/README.md](../may-26-indexing-residue-tasks/README.md)
- [../may-25-lexical-enhancement/README.md](../may-25-lexical-enhancement/README.md)
