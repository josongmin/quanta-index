# May-25 Closeout Plan — Sourcegraph Compat + LQ DSL

> Status: `Historical closeout packet — repo-internal closeout landed`
> Scope: remaining work only; historical shipped claims stay in [implementation-plan.md](implementation-plan.md)
> Parent packet: [implementation-plan.md](implementation-plan.md)
> Companion index: [tickets/INDEX.md](tickets/INDEX.md)
> Posture: **breaking-first**, **fail-closed**, **one front door**, **one active contract**, **one lexical authority**

---

## 1. Purpose

This packet exists as the execution record for the repo-internal closeout that took crate-local LQ work to a live, verified query path.

Current truth:

- parser / normalizer / canonical hash are on the active path through [`quanta-index-lq-norm`](../../../crates/quanta-index-lq-norm/)
- Sourcegraph parser / translator are on the active path through [`quanta-index-lq-bridge`](../../../crates/quanta-index-lq-bridge/)
- the active query contract is versioned / typed, and the legacy `Custom` escape hatches are removed from the live wire
- [`crates/quanta-index-search-plane`](../../../crates/quanta-index-search-plane/) routes lexical / semantic / hybrid requests through the closed repo-first intake path; `searchd` is the transport/runtime shell around that path, and the workspace proof rails are green
- hard DSL scenario coverage lives in [`crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`](../../../crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs)
- structural / history live success remains producer-gated; current repo behaviour is explicit fail-closed / unavailable until producer commit/diff / parse-tree ops arrive
- external producer / caller entry is not yet fully frozen to SDK-only for the remaining history / runtime / structural cutover; follow-on is tracked in [tickets/SDK-ENTRY-01.md](tickets/SDK-ENTRY-01.md)
- real-engine conformance CI, deployment-side observability, and the bridge downstream sink remain open integration tasks

The remaining value of this packet is the dependency history and the boundary between landed repo work and external cutover work.

---

## 2. Closeout definition

`Sourcegraph-compatible LQ DSL` is claimable only when all of the following are true:

1. active request intake accepts either LQ text or Sourcegraph text and lowers both through one canonical pipeline
2. active wire contract is versioned, typed, and has no legacy `Custom` escape hatch
3. the active `search-plane` lexical query path routes through one lexical planner, not a single-engine direct call
4. semantic / hybrid queries are rebased onto the lexical filter universe
5. bridge means one thing: `CodeQL candidate export`, not "Sourcegraph syntax translator"
6. producer-authored channel ops needed by history / runtime / structural are either fully wired or explicitly fail closed with the chosen Option A/B contract
7. corpus / RFC / feature-scope / DSL docs agree on the same claim boundary

Anything short of that stays `partial`, even if many individual crates are green.

---

## 3. Non-negotiable execution rules

- no dual query surface kept alive after cutover
- no parser bypass that silently falls back to `Raw(String)`
- no "best effort" Sourcegraph compatibility claim from grep or crate presence
- no search-side source parsing / git walking resurrection; producer-authorship correction stands
- no giant big-bang PR; cut by write scope and front-door dependency

---

## 4. Remaining work, grouped structurally

### 4.1 Contract closeout

- active query contract v2 under `quanta-index-contract::query::*`
- result / explanation / bridge / structural / history carrier completion
- remove legacy `Custom` escape hatches
- make `lq_version` mandatory on active query wire

### 4.2 Front-door closeout

- add one typed text-intake surface for LQ / Sourcegraph
- lower Sourcegraph text through bridge parser / translator
- lower LQ text through tokenizer / parser / normalizer / hash pipeline
- route both onto one planner-owned active query model

### 4.3 Lexical authority closeout

- replace direct single-engine lexical search with planner-selected shard fanout
- unify content / path / symbol / regex / trigram / phrase rails
- land deterministic merge + cancellation on the active query path

### 4.4 Semantic / bridge closeout

- make semantic queries lexically scoped first
- make hybrid use the lexical planner as the universe-defining step
- split "Sourcegraph syntax intake" from "CodeQL candidate export" and ship both honestly

### 4.5 Producer / channel closeout

- finalize proposed channel ops and wire-shape ownership
- resolve STR-01 Option A/B
- close history / dirty / parse-tree handshake residue
- route the remaining producer authority families through `quanta-index-sdk` only; no raw external channel / socket path survives the cutover

### 4.6 SDK-only entry closeout

- external query / control / ingest entry must terminate at [`quanta-index-sdk`](../../../crates/quanta-index-sdk/)
- query side keeps text-first entry (`lexical().query()` / `sourcegraph().query()`); history / runtime / structural expand under that front door
- producer side extends `LexicalBatch` for commit / diff / dirty / parse-tree authority instead of adding ad hoc ingress surfaces

### 4.7 Claim / corpus / doc closeout

- add missing UC-HYB / UC-INC / catalog-miss rows
- reconcile RFC / feature-scope / DSL mismatches
- remove stale references left after producer-authorship correction

---

## 5. Dependency waves and parallel lanes

### Wave 0 — honesty freeze + cutover contract

Goal: freeze what "done" means before touching runtime paths.

#### Lane W0-A — packet / doc freeze

- Owner label: `워커A docs-closeout-freeze`
- Write scope:
  - [implementation-plan.md](implementation-plan.md)
  - [tickets/INDEX.md](tickets/INDEX.md)
  - [feature-scope.md](feature-scope.md)
  - [usecase.md](usecase.md)
  - [dsl.md](dsl.md)
- First PR shape:
  - add explicit closeout note that crate-local shipped != front-door shipped
  - mark RFC roll-up gaps as active closeout items
  - record the chosen front-door cutover order from this packet
- Expected compile fallout: none
- Gate:
  - no behavioral claim text contradicts current code
  - all remaining work items have one owner lane in this packet

#### Lane W0-B — contract decision lock

- Owner label: `워커B query-wire-lock`
- Write scope:
  - `crates/quanta-index-contract/src/query/*.rs`
  - `crates/quanta-index-contract/src/ipc/{requests,envelopes}.rs`
  - `crates/quanta-index-core/src/domains/{lexical,semantic,hybrid}/*.rs`
- First PR shape:
  - freeze active query-wire target
  - decide which fields move from parse-time-only AST into active wire
  - remove any ambiguity around `Custom`, `timeout_ms`, `count_all`, `limit`, `patterntype`, directives
- Expected compile fallout:
  - every crate importing `LqQuery`, `LqExpr`, `LqFilter`, `LqDirective`, `LqOptionSet`
- Gate:
  - `cargo test -p quanta-index-contract -p quanta-index-core -p quanta-index-searchd`
  - no `Custom` variant left on the active query wire

### Wave 1 — active contract closure

Goal: make the active wire model structurally compatible with the claimed DSL boundary.

#### Lane W1-A — query wire v2

- Owner label: `워커C contract-query-v2`
- Depends on: `W0-B`
- Write scope:
  - `crates/quanta-index-contract/src/query/{expression,filters,directives,options,mod}.rs`
  - `crates/quanta-index-contract/src/ipc/{requests,envelopes}.rs`
  - `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- First PR shape:
  - add `lq_version`
  - replace stringly `filter` / `option` / `directive` carriers with typed variants needed by the closeout scope
  - add missing leaf kinds required for live lowering
  - keep unsupported-but-reserved constructs fail-closed and typed
- Expected compile fallout:
  - `searchd`, `core`, `ipc`, test fixtures, any manual CBOR round-trips
- Gate:
  - contract round-trip tests updated
  - query-path tests compile with no legacy field names

#### Lane W1-B — result carrier completion

- Owner label: `워커D contract-result-carriers`
- Parallel with: `W1-A` (disjoint write set)
- Write scope:
  - `crates/quanta-index-contract/src/results/*.rs`
  - `crates/quanta-index-contract/src/lex/*.rs`
  - tests under `crates/quanta-index-contract/tests/`
- First PR shape:
  - promote canonical carriers for symbol / commit / diff / structural / bridge / explanation payloads
  - delete crate-local placeholder result envelopes where possible
- Expected compile fallout:
  - `lq-history`, `lq-structural`, `lq-bridge`, `lq-ranker`, `searchd` explain path
- Gate:
  - all new carrier types have hand-rolled CBOR round-trip tests
  - `SearchExplanation` is no longer summary-only on the canonical path

#### Lane W1-C — producer handoff cut docs

- Owner label: `워커E producer-handoff-cut`
- Parallel with: `W1-A`, `W1-B`
- Write scope:
  - `docs/ssot/producer-handoff.md`
  - `docs/handoffs/lq-contract-1.0-pre.md`
  - `docs/handoffs/lq-history-1.1.md`
- First PR shape:
  - cut the deferred handoff docs from SSOT
  - pin active wire ownership and bump policy for the new contract surfaces
- Expected compile fallout: none
- Gate:
  - no remaining `deferred` handoff row for PRE-CONTRACT-EXT / LEX-07

### Wave 2 — front-door intake closure

Goal: put `lq-norm` and `lq-bridge` on the live intake path.

#### Lane W2-A — LQ text intake

- Owner label: `워커F lq-intake`
- Depends on: `W1-A`
- Write scope:
  - `crates/quanta-index-searchd/src/app/`
  - `crates/quanta-index-ipc/src/`
  - `crates/quanta-index-contract/src/ipc/`
- First PR shape:
  - add one explicit text-intake request surface for canonical LQ text
  - tokenize → parse → normalize → hash before dispatch
  - reject invalid / oversized / unsupported input at intake, not deep in adapters
- Expected compile fallout:
  - request codec, server dispatch, CLI smoke, end-to-end request fixtures
- Gate:
  - `searchd` has a request shape that can carry LQ text directly
  - intake-path tests prove parser errors surface as typed failures

#### Lane W2-B — Sourcegraph text intake

- Owner label: `워커G sg-intake`
- Depends on: `W1-A`
- Parallel with: `W2-A`
- Write scope:
  - `crates/quanta-index-lq-bridge/src/*`
  - `crates/quanta-index-searchd/src/app/`
  - `crates/quanta-index-contract/src/ipc/`
- First PR shape:
  - add one explicit Sourcegraph intake path
  - `parse_sourcegraph` + `translate` feed the same planner-owned active query model
  - no auto-magic syntax guessing in the first cut; syntax is explicit on the request
- Expected compile fallout:
  - bridge golden tests may need request-envelope fixtures
- Gate:
  - active runtime path executes at least one Sourcegraph query end to end
  - unsupported SG syntax still fails closed with typed bridge codes

#### Lane W2-C — lowering boundary lock

- Owner label: `워커H lowering-boundary`
- Depends on: `W1-A`
- Parallel with: `W2-A`, `W2-B`
- Write scope:
  - `crates/quanta-index-core/src/domains/lexical/`
  - new lowering module under `crates/quanta-index-core/src/domains/lexical/`
  - focused tests in `crates/quanta-index-core/tests/`
- First PR shape:
  - define one lowering boundary from normalized / translated intake forms into the active planner model
  - ban direct `Raw(String)` bypass on the new front door
- Expected compile fallout:
  - lexical policy / inbound / outbound traits, searchd dispatcher
- Gate:
  - one place owns lowering; no second query-shape bridge in `searchd`

### Wave 3 — lexical authority unification

Goal: replace direct single-engine lexical search with planner-selected shard execution.

#### Lane W3-A — planner + shard fanout

- Owner label: `워커I lexical-planner`
- Depends on: `W2-C`
- Write scope:
  - `crates/quanta-index-core/src/domains/lexical/{inbound,outbound,service}.rs`
  - `crates/quanta-index-lexical/src/`
  - `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- First PR shape:
  - planner chooses content / regex / trigram / phrase / symbol / path rails
  - active lexical query path no longer calls one undifferentiated `search(&LqQuery, top_k)`
- Expected compile fallout:
  - lexical open/search traits, adapter caches, searchd lexical path
- Gate:
  - one real path exists from query leaf/filter kind to owning shard
  - tests cover at least one concrete route for phrase, regex, symbol, raw substring

#### Lane W3-B — deterministic merge + cancellation

- Owner label: `워커J merge-closeout`
- Depends on: `W3-A`
- Write scope:
  - `crates/quanta-index-core/src/domains/lexical/`
  - `crates/quanta-index-lq-ranker/src/`
  - `crates/quanta-index-lq-hybrid/src/` if merge helpers are shared
- First PR shape:
  - bounded fanout
  - cooperative cancellation checkpoints
  - one total-order merge tuple enforced on the active lexical path
- Expected compile fallout:
  - ranking / explain path, any lexical candidate ordering assertions
- Gate:
  - deterministic order proof on active front door
  - no per-shard partial-success leakage on cancellation / timeout

### Wave 4 — semantic / bridge parity closure

Goal: close the two large remaining roll-up gaps honestly.

#### Lane W4-A — semantic rebased on lexical universe

- Owner label: `워커K sem-on-lex`
- Depends on: `W3-A`
- Write scope:
  - `crates/quanta-index-contract/src/query/requests.rs`
  - `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  - `crates/quanta-index-core/src/domains/semantic/*.rs`
  - `crates/quanta-index-semantic/src/`
- First PR shape:
  - semantic request stops being "query_text + lexical_filters bag"
  - planner computes lexical universe first, semantic ANN runs inside it
- Expected compile fallout:
  - semantic request wire, semantic tests, hybrid tests
- Gate:
  - semantic query path cannot bypass lexical universe definition

#### Lane W4-B — actual CodeQL bridge

- Owner label: `워커L codeql-bridge`
- Depends on: `W1-B`, `W2-B`
- Write scope:
  - `crates/quanta-index-lq-bridge/src/`
  - `crates/quanta-index-contract/src/results/` or bridge-specific carrier file
  - `crates/quanta-index-searchd/src/app/`
- First PR shape:
  - split "SG syntax translator" from "CodeQL candidate export"
  - ship `BridgeCandidatePacket` and provenance-preserving sink envelope
  - make `into:codeql` / `scope:results` / `with:lexical` mean one thing
- Expected compile fallout:
  - bridge tests, explain / candidate export paths, docs references to BRIDGE-01
- Gate:
  - RFC `BRIDGE-01` no longer points at the wrong implementation target

### Wave 5 — producer / runtime / structural handshake closure

Goal: close the remaining cross-repo ambiguity that blocks honest end-state claims.

#### Lane W5-A — producer op finalization

- Owner label: `워커M producer-op-finalize`
- Depends on: `W1-B`
- Write scope:
  - `crates/quanta-index-contract/src/channel/` if expanded
  - `docs/ssot/producer-handoff.md`
  - `docs/ssot/channel-architecture.md`
  - consumers in `lq-history`, `lq-runtime`, `lq-structural`
- First PR shape:
  - resolve `AMB-PROD-1..11` that are still real blockers
  - pin `UpsertCommit` / `UpsertDirty` / `UpsertParseTree` ownership and ordering rules
- Expected compile fallout:
  - channel CBOR, replay logic, history/runtime/structural consumer tests
- Gate:
  - no remaining "proposed, pending producer agreement" row for the ops needed by chosen closeout scope

#### Lane W5-B — STR-01 Option lock

- Owner label: `워커N str-option-lock`
- Depends on: `W5-A`
- Write scope:
  - `tickets/STR-01.md`
  - `implementation-plan.md`
  - runtime code only if Option A is selected
- First PR shape:
  - choose Option A or Option B explicitly
  - delete dead branch language from the packet
- Expected compile fallout: low if Option B, moderate if Option A
- Gate:
  - STR claim boundary is singular; no dual-status wording remains

### Wave 6 — claim gate closeout

Goal: align code, corpus, and docs so the repo can make one end-to-end claim without footnotes.

#### Lane W6-A — corpus / RFC / DSL reconciliation

- Owner label: `워커O corpus-doc-closeout`
- Depends on: `W4-A`, `W4-B`, `W5-B`
- Write scope:
  - `usecase.md`
  - `dsl.md`
  - `feature-scope.md`
  - `rfc.md`
  - `tickets/INDEX.md`
- First PR shape:
  - add missing `UC-HYB-*`, `UC-INC-*`, catalog-miss rows
  - reconcile bridge taxonomy, `count:` cap, `lang:` ship-set, scale targets
  - remove stale post-correction references
- Expected compile fallout: none
- Gate:
  - no open `UC-GAP-*`, `DSL-GAP-*`, `FS-GAP-*`, `RFC-GAP-*` rows for the shipped scope

#### Lane W6-B — final verification rail

- Owner label: `워커P final-proof-rail`
- Depends on: all prior waves
- Write scope:
  - tests / CI rails only; no product behavior changes
- First PR shape:
  - one narrow end-to-end proof rail for:
    - LQ text intake
    - Sourcegraph text intake
    - lexical planner routing
    - semantic-on-lex pushdown
    - CodeQL bridge packet
  - keep workspace-wide proof separate from crate-local proof
- Expected compile fallout:
  - CI/runtime tests only
- Gate:
  - workspace closeout command is green
  - claim language in docs matches the real green rail

---

## 6. Recommended PR order

Use this order. Parallelize only within a wave.

1. `W0-A docs-closeout-freeze`
2. `W0-B query-wire-lock`
3. `W1-A contract-query-v2`
4. `W1-B contract-result-carriers`
5. `W1-C producer-handoff-cut`
6. `W2-A lq-intake`
7. `W2-B sg-intake`
8. `W2-C lowering-boundary`
9. `W3-A lexical-planner`
10. `W3-B merge-closeout`
11. `W4-A sem-on-lex`
12. `W4-B codeql-bridge`
13. `W5-A producer-op-finalize`
14. `W5-B str-option-lock`
15. `W6-A corpus-doc-closeout`
16. `W6-B final-proof-rail`

---

## 7. Immediate next step

Do not start with `semantic` or `bridge` polish.

Start with:

1. `W0-B query-wire-lock`
2. `W1-A contract-query-v2`
3. `W2-A/W2-B` intake split

Without those, every later claim still sits behind the wrong front door.
