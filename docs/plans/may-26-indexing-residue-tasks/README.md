# May 26 Indexing Residue Tasks

Status: `shipped`
Date: `2026-05-26`
Scope: post-`may-25-lexical-enhancement` residue only.

---

## 1. Objective

Freeze the **actual remaining code work** after the current lexical and
structural closeout wave.

This pack is intentionally narrower than `may-25-lexical-enhancement`.
It excludes:

- lexical work already owned by active `LXE-04`, `LXE-05`, `LXE-06` lanes
- structural live-path wiring already landed
- Sourcegraph lexical v1 subset already shipped
- cleanup tasks already completed in code

## 2. Current truth

Landed in code:

- live structural execution is no longer root-only; the truthful subset now
  executes root-kind exact, root capture, root-kind plus capture, ordered
  direct-child tree-walk, variadic sibling capture / wildcard skip, and
  `where` / `inside` / `outside` constraints over producer parse-tree
  authority
- native structural AST/IR now carries typed `Pattern`, `Where`, `Inside`,
  `Outside`, `HoleMany`, and `WildcardMany` forms
- the `LqStructuralBlock` carrier now preserves both the legacy `nodes` view
  and the richer `exprs` view so current wire/tests stay stable while richer
  semantics route through one authority-owned matcher
- structural public query surface executes `lang:`, `repo:`, and `file:` on the
  live subset
- Sourcegraph lexical route now rejects `patterntype:structural` with
  `BRIDGE_TRANSLATE_FAIL`
- Sourcegraph structural route now exists on the structural frontdoor and
  lowers one quoted/keyword structural body plus executable filters onto native
  structural execution

Intentionally unsupported after this pack:

- boolean composition of structural leaves remains explicitly unsupported on the
  public structural route
- Sourcegraph structural regex bodies and boolean pattern composition remain
  typed rejection paths

## 3. Landed surfaces

- `STR-02`: shipped
- `STR-03`: shipped
- `STR-04`: shipped
- `BRIDGE-02`: shipped
- `BRIDGE-03`: shipped as the supported Sourcegraph structural subset

Verification refresh (2026-05-27):

- current live-source rerun stayed green on:
  - `cargo check -p quanta-index-contract`
  - `cargo check -p quanta-index-sdk`
  - `cargo test -p quanta-index-searchd-runtime --test repo_map_end_to_end`
  - `cargo test -p quanta-index-searchd-runtime`
- owner-local structural/bridge proof was also rerun directly on the same tree:
  - `cargo test -p quanta-index-lq-structural`
  - `cargo test -p quanta-index-lq-bridge`
  - `cargo check -p quanta-index-search-plane --tests`
  - `cargo test -p quanta-index-searchd-runtime --test end_to_end structural_sourcegraph_query_ -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
- this re-proves the shipped residue slice on the current tree; it is not a
  broader frozen-tree or workspace-wide closure claim
- this pack-level closeout is narrower than the broader `may-25` program
  closeout; whole-program follow-up still belongs to the `may-25` ticket pack

## 4. Non-goals

- do not reopen `LXE-04`, `LXE-05`, `LXE-06`
- do not rebuild Sourcegraph lexical v1 parser/translator/runtime surface
- do not re-implement native `match { ... }` parsing
- do not redo structural `repo/file/lang` happy-path wiring
- do not start lexical/structural fused ranking in this pack

## 4.5 Follow-up outside this pack

The remaining closeout queue lives in `docs/plans/may-25-lexical-enhancement`,
not here. As of the same `2026-05-27` live rerun, that broader queue still
tracks:

- `LXE-10` observability and bridge sink
- `E2E-07` performance/chaos closeout
- broader `may-25` matrix/ticket drift cleanup

## 5. Ticket pack

- [tickets/INDEX.md](tickets/INDEX.md)
- [tickets/STR-02-authority-matcher-tree-walk-expansion.md](tickets/STR-02-authority-matcher-tree-walk-expansion.md)
- [tickets/STR-03-native-structural-semantics-ast-ir-expansion.md](tickets/STR-03-native-structural-semantics-ast-ir-expansion.md)
- [tickets/STR-04-structural-query-surface-expansion.md](tickets/STR-04-structural-query-surface-expansion.md)
- [tickets/BRIDGE-02-sourcegraph-structural-honesty-gate.md](tickets/BRIDGE-02-sourcegraph-structural-honesty-gate.md)
- [tickets/BRIDGE-03-sourcegraph-structural-syntax-and-lowering.md](tickets/BRIDGE-03-sourcegraph-structural-syntax-and-lowering.md)

## 6. Exit criteria

This pack is now closed:

1. the root-only structural executor is no longer the only truthful live shape
2. native structural AST/IR represents the next semantics wave in typed form
3. structural public surface is widened only where runtime authority exists
4. `patterntype:structural` no longer ambiguously falls through lexical
   execution
5. Sourcegraph structural syntax now has an honest supported subset with typed
   rejection for unsupported extensions
6. no additional implementation work remains inside this pack on the current
   tree; remaining closure work belongs to the broader `may-25` program
