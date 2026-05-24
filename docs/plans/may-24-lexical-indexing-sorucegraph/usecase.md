# May-24 Lexical Kernel — Usecase Catalog & Golden Query Conformance Corpus

Status: `Planning packet companion — conformance corpus`

Companion to:

- [rfc.md](rfc.md) — Sourcegraph-class Lexical Kernel RFC (LQ family definition).
- [feature-scope.md](feature-scope.md) — forward reference, authored in parallel; describes which LQ tiers are in/out of scope per wave.
- [dsl.md](dsl.md) — forward reference, authored in parallel; canonical grammar / typed AST / printer.

Upstream baseline references:

- <https://sourcegraph.com/docs/code-search/queries>
- <https://sourcegraph.com/docs/code_search/reference/queries>
- <https://sourcegraph.com/docs/code-search/working/search_filters>

> P0 reviewer finding addressed: this document is the conformance corpus. Every usecase carries a golden query string + expected result-shape, and every anti-usecase carries a typed error code. This is the canonical answer to "no conformance suite reference."

---

## 0. Conventions

### Result-shape vocabulary

All `ok` responses are expressed in the frozen contract types:

- `SearchPlaneLexicalQueryResponse { generation: PublishedGenerationSet, results: Vec<LexicalCandidate> }`
- `LexicalCandidate { candidate_id, repo_id, revision_id, manifest_generation, repo_relative_path, start_line, end_line, score, snippet }`

Shape categories used in tables:

| token | meaning |
| --- | --- |
| `empty` | `results = []`, `generation` still bound to a real published set |
| `single` | exactly one `LexicalCandidate` |
| `multi` | two or more, total `<= count` option, deterministic order |
| `paginated` | client supplied `count:<n>` and a cursor / early-stop reason is recorded |
| `error:<CODE>` | not a `LexicalQueryResponse`; envelope carries typed error `<CODE>` |

Default ordering is `score desc, repo_id asc, repo_relative_path asc, start_line asc` unless the row says otherwise. All result rows are bound to one `PublishedGenerationSet`; cross-generation reads are forbidden.

### Engine columns

`L` lexical content, `P` path, `S` symbol, `H` history (commit/diff), `T` structural (tree-sitter), `R` runtime metadata catalog, `B` CodeQL bridge.

### Parity column

| token | meaning |
| --- | --- |
| `SG=` | byte-for-byte identical to Sourcegraph reference behavior |
| `SG~` | normalized from Sourcegraph (e.g. `foo bar` -> `foo AND bar`, `:[X] -> $X`) |
| `Q+` | quanta-extension (no direct Sourcegraph equivalent) |
| `SG!` | deliberate divergence from Sourcegraph; see notes |

### Error codes (SSOT, SCREAMING_SNAKE_CASE)

| code | meaning |
| --- | --- |
| `PARSE_ERROR` | grammar reject before planner |
| `INVALID_FILTER` | filter name unknown or value malformed |
| `FORBIDDEN_SYNTAX` | grammar-legal but explicitly banned (e.g. regex backreference, `@` shorthand) |
| `PLAN_ERROR` | parser ok but planner cannot route (e.g. `type:commit` + `match { ... }`) |
| `UNSUPPORTED_COMBO` | engines exist but combination is rejected by `LqPlannerV1` |
| `TIMEOUT_EXCEEDED` | `timeout:` budget hit |
| `OVERSIZED_REQUEST` | raw request body > 16 MiB |
| `GENERATION_MISMATCH` | explicit `request.generation` does not match a published set |
| `ACL_DENIED` | repo not authorized for tenant (fail-closed) |
| `TENANT_ISOLATION` | cross-tenant leakage attempt (defense-in-depth on top of `ACL_DENIED`) |
| `CANCELLED` | client cancellation observed cleanly |
| `BRIDGE_REJECTED` | downstream bridge (e.g. CodeQL) refused the candidate packet |
| `NOT_IMPLEMENTED` | grammar accepts, planner explicitly typed-not-yet (cutover discipline) |

---

## 1. Personas

| ID | Name | Context | LQ tier mix |
| --- | --- | --- | --- |
| `P1` | Code Reviewer | Reviewing a PR; needs to locate the implementation of an identifier the diff references, and to confirm no other call sites are touched. Wants fast, deterministic, repo-scoped lookups. Heavy reliance on symbol and exact-phrase queries; rarely needs history. | Core (heavy), Structural (light) |
| `P2` | SRE / Incident Responder | Paged at 03:00 with a stack frame and a service name. Needs to find the most recent diff that touched a function, and whether the offending file changed since the last good build. Mixes history with runtime metadata. | Core, History, Runtime |
| `P3` | Security Auditor | Hunts for callers of dangerous primitives (`exec`, `unsafe`, `unwrap`, raw SQL) across the entire fleet. Needs cross-repo recall with strong filters and a clean handoff to CodeQL for dataflow proof. Cannot tolerate fuzzy defaults. | Core, Structural, Bridge |
| `P4` | Refactoring Engineer | Migrating callers off a deprecated API. Needs precise, exhaustive identifier+signature search, plus diff history to confirm migration coverage. Cares about `count:all` correctness. | Core, Structural, History |
| `P5` | New-hire Onboarder | Wants broad, exploratory examples of "how do we use X here." Tolerates multi-hit recall, expects natural keyword + lang filtering. Rarely uses history or structural. | Core (very heavy) |
| `P6` | Build / CI Operator | Automation: pre-merge bots, code-owner checks, license sweeps, "did anything in `meta.layer:storage` change since the last release?" Mostly programmatic. Strong dependency on runtime metadata filters and predicate filters. | Core, Runtime, History |

---

## 2. Usecase Catalog

> 70 usecases across categories A–I. IDs are stable identifiers for the golden-file conformance corpus (§6).

### A. Lexical content (keyword / phrase / regex)

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-LEX-01` | Bare keyword | `P5` | `fooBar` | `multi`, score desc | – | L | parser yields one `Keyword("fooBar")` leaf; planner routes to lexical content engine; all hits contain literal `fooBar` | `SG=` |
| `UC-LEX-02` | Multi-word adjacency = AND | `P5` | `tokio runtime` | `multi`, score desc | – | L | normalized AST: `And(Keyword("tokio"), Keyword("runtime"))`; every hit contains both | `SG~` |
| `UC-LEX-03` | Exact phrase | `P1` | `"async fn handle"` | `multi`, score desc | – | L | phrase token; hits contain the bytes contiguously (modulo whitespace per token rules) | `SG=` |
| `UC-LEX-04` | Raw string | `P1` | `'C:\\Users\\%'` | `multi`, score desc | – | L | minimal-escape raw string; backslashes preserved verbatim | `SG=` |
| `UC-LEX-05` | Regex literal | `P3` | `/fn\s+handle_\w+/` | `multi`, score desc | – | L | RE2-class regex; no backreference; compiles in planner | `SG=` |
| `UC-LEX-06` | Regex with line anchor | `P3` | `/^fn foo/` | `multi`, score desc | – | L | `^` anchors per-line in chunked content; verified via stored `start_line` | `SG=` |
| `UC-LEX-07` | `repo:` exact | `P1` | `repo:github.com/quanta/index foo` | `multi` | repo | L | repo-id resolution at plan time; non-matching repos pruned pre-fanout | `SG=` |
| `UC-LEX-08` | `repo:` regex pattern | `P3` | `repo:^github\.com/foo/.*$ unwrap` | `multi`, group by repo | repo | L | repo pattern compiled to RE2; planner expands to repo-id set | `SG=` |
| `UC-LEX-09` | `repo@rev` sugar | `P2` | `repo:foo@main panic!` | `multi` | repo, rev | L | normalized AST: `repo:foo rev:main`; printer round-trips back to sugar form | `SG~` |
| `UC-LEX-10` | `file:` content scope | `P1` | `file:src/ TODO` | `multi` | file | L,P | file pattern is content-scope; printer keeps as `file:` not `path:` | `SG=` |
| `UC-LEX-11` | `path:` alias normalization | `P1` | `path:^src/lib/ Trait` | `multi` | path | P,L | parser accepts `path:`; normalized AST stores as canonical path-scope token (one form per RFC §Filter semantics) | `SG~` |
| `UC-LEX-12` | `lang:` filter | `P5` | `lang:rust Iterator` | `multi` | lang | L | language metadata lookup at indexing time; results all have `repo_relative_path` of a Rust file | `SG=` |
| `UC-LEX-13` | Exclusion `-foo` | `P1` | `Iterator -dyn` | `multi` | – | L | parsed as `And(Keyword("Iterator"), Not(Keyword("dyn")))` | `SG=` |
| `UC-LEX-14` | File exclusion `-file:test` | `P1` | `Iterator -file:test` | `multi` | file (negated) | L,P | hits exclude paths matching `test` substring | `SG=` |
| `UC-LEX-15` | Combined filters + boolean | `P3` | `repo:r1 lang:rust foo AND bar` | `multi` | repo, lang | L | one combined plan; no post-filter residue | `SG=` |
| `UC-LEX-16` | `case:yes` | `P1` | `case:yes Foo` | `multi` | case option | L | case-sensitive recall; lowercase `foo` does not match | `SG=` |
| `UC-LEX-17` | `case:no` (default) | `P5` | `case:no Foo` | `multi` | case option | L | folded recall; explicit form must equal default behavior | `SG=` |
| `UC-LEX-18` | `count:100` | `P5` | `Iterator count:100` | `paginated`, up to 100 | count option | L | hard upper bound; executor records early-stop reason if exhausted | `SG=` |
| `UC-LEX-19` | `count:all` | `P4` | `deprecated_api count:all` | `multi`, no early stop | count option | L | executor must not silently truncate; metric `early_stop_reason=none` | `SG=` |
| `UC-LEX-20` | `timeout:5s` | `P6` | `Iterator timeout:5s` | `multi` or `error:TIMEOUT_EXCEEDED` | timeout option | L | bounded budget; partial results not silently returned on timeout — typed error | `SG!` (SG returns partial; quanta is fail-closed per RFC §Non-Negotiable Invariants) |
| `UC-LEX-21` | `patterntype:regexp` switch | `P3` | `patterntype:regexp fn\s+\w+` | `multi` | option | L | switches default leaf interpretation; equivalent to `/fn\s+\w+/` | `SG=` |
| `UC-LEX-22` | Boolean OR | `P3` | `panic! OR unwrap()` | `multi` | – | L | `Or(Keyword("panic!"), Phrase("unwrap()"))`; precedence per RFC | `SG=` |
| `UC-LEX-23` | Boolean grouping | `P3` | `(panic OR unwrap) lang:rust` | `multi` | lang | L | explicit paren-group binds tighter than top-level adjacency | `SG=` |
| `UC-LEX-24` | NOT precedence | `P1` | `foo OR NOT bar` | `multi` | – | L | parses as `Or(foo, Not(bar))` per RFC `NOT > AND > OR` | `SG=` |
| `UC-LEX-25` | `select:repo` projection | `P6` | `Iterator select:repo` | `multi` deduplicated by repo | select | L | result projection collapses to one row per repo; `repo_relative_path` may be the first-hit representative | `SG=` |
| `UC-LEX-26` | `select:file` projection | `P5` | `Iterator select:file` | `multi` dedup by file | select | L | one row per `(repo, repo_relative_path)` | `SG=` |
| `UC-LEX-27` | `fork:no` | `P3` | `fork:no Iterator` | `multi` | fork | L | excludes forks at plan time; catalog flag check | `SG=` |
| `UC-LEX-28` | `archived:no` | `P3` | `archived:no Iterator` | `multi` | archived | L | excludes archived repos at plan time | `SG=` |

### B. Predicate filters

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-PRED-01` | `repo:has.file(...)` | `P6` | `repo:has.file(path:Cargo\.toml) tokio` | `multi` | predicate | L,P | repo set narrowed to those with a matching file before content fanout | `SG=` |
| `UC-PRED-02` | `repo:has.commit.after(...)` | `P2` | `repo:has.commit.after(yesterday) regression` | `multi` | predicate | L,H | history catalog answers predicate without request-time git scan | `SG=` |
| `UC-PRED-03` | `repo:has.path(...)` | `P6` | `repo:has.path(.github/workflows) name:` | `multi` | predicate | L,P | path index answers predicate | `SG=` |
| `UC-PRED-04` | `file:contains(...)` | `P4` | `file:contains(impl Display) for Foo` | `multi` | predicate | L | file-level pre-filter then content match | `SG=` |
| `UC-PRED-05` | `file:has.content(...)` | `P3` | `file:has.content(license:MIT) crypto` | `multi` | predicate | L | content predicate is a separate pre-pass | `SG=` |
| `UC-PRED-06` | Boolean predicate composition | `P3` | `(repo:has.file(path:Cargo\.toml) OR repo:has.file(path:go\.mod)) panic` | `multi` | predicates | L,P | predicate AST composes under `Or`/`And` | `SG=` |
| `UC-PRED-07` | Negated predicate | `P6` | `-repo:has.file(path:LICENSE) crypto` | `multi` | predicate (neg) | L,P | repos without LICENSE only | `SG=` |

### C. Symbol search

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-SYM-01` | Bare symbol | `P1` | `type:symbol Foo` | `multi`, ordered by score | type | S | symbol index hit; `repo_relative_path` + `start_line` point to definition | `SG=` |
| `UC-SYM-02` | Symbol kind = function | `P4` | `type:symbol kind:function handler` | `multi` | type, kind | S | symbol index filters by kind; **contract gap**: `LexicalCandidate` does not currently carry `kind` — flagged in §3 | `SG=` |
| `UC-SYM-03` | Symbol in path | `P1` | `type:symbol file:src/.* Bar` | `multi` | type, file | S,P | path-pre-filter then symbol scan | `SG=` |
| `UC-SYM-04` | Symbol regex | `P3` | `type:symbol /^Bar/` | `multi` | type | S | regex applies to symbol name index | `SG=` |
| `UC-SYM-05` | Symbol case-sensitive | `P1` | `type:symbol case:yes Foo` | `multi` | type, case | S | case option scoped to symbol planner | `SG=` |
| `UC-SYM-06` | Symbol with repo | `P3` | `type:symbol repo:^github\.com/q/.* parse_query` | `multi` | type, repo | S | repo-fanout then symbol shard | `SG=` |

### D. History (commit / diff)

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-HIST-01` | Commit message search | `P2` | `type:commit message:fix` | `multi`, recency desc | type, message | H | commit metadata index hit; **contract gap**: response carries `LexicalCandidate` only; commit-specific fields (commit_id, author, ts) are not modeled — flagged in §3 | `SG=` |
| `UC-HIST-02` | Author filter | `P4` | `type:commit author:alice` | `multi`, recency desc | type, author | H | author lookup at indexing time | `SG=` |
| `UC-HIST-03` | before/after | `P2` | `type:commit before:2024-01-01 after:2023-01-01` | `multi`, recency desc | type, before, after | H | date range narrowing | `SG=` |
| `UC-HIST-04` | Diff added text | `P2` | `type:diff diff.added:TODO` | `multi` | type, diff.added | H | diff hunk index hit on `+` lines | `SG=` |
| `UC-HIST-05` | Diff removed text | `P4` | `type:diff diff.removed:fn foo` | `multi` | type, diff.removed | H | diff hunk index hit on `-` lines | `SG=` |
| `UC-HIST-06` | Diff touched file | `P2` | `type:diff file:src/lib.rs` | `multi`, recency desc | type, file | H,P | diff catalog filtered by path | `SG=` |
| `UC-HIST-07` | Combined diff query | `P3` | `type:diff author:alice diff.added:unwrap` | `multi` | type, author, diff.added | H | single composed plan; no per-query git scan | `SG=` |
| `UC-HIST-08` | Range diff | `P4` | `type:diff rev:main...feature` | `multi` | type, rev | H | range resolved via revision catalog at indexing time | `SG=` |

### E. Structural

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-STR-01` | Simple match | `P4` | `match { fn $X(...) { ... } }` | `multi`, no implicit ordering | – | T | tree-sitter matcher binds `$X`; **contract gap**: metavariable bindings are not represented in `LexicalCandidate` — flagged in §3 | `SG~` |
| `UC-STR-02` | Type constraint | `P3` | `match { :[hole.type1=String] }` | `multi` | – | T | type-constrained metavariable; alias `:[X]` -> `$X` per RFC | `SG~` |
| `UC-STR-03` | `inside` operator | `P3` | `inside: { fn handler { ... } } match { unwrap() }` | `multi` | – | T | `inside` scopes the match to nodes within `handler` | `SG~` |
| `UC-STR-04` | `outside` operator | `P3` | `outside: { fn test_$_ { ... } } match { panic!(...) }` | `multi` | – | T | `outside` excludes matches whose nearest enclosing matches the outer pattern | `Q+` |
| `UC-STR-05` | `where` clause | `P4` | `match { $F($X) } where $F == "exec"` | `multi` | – | T | metavariable constraint enforced post-bind | `SG~` |
| `UC-STR-06` | Variadic capture | `P4` | `match { handle($...ARGS) }` | `multi` | – | T | variadic capture binds zero-or-more args | `SG~` |
| `UC-STR-07` | Alias normalize `:[X]` ≡ `$X` | `P3` | `match { :[X] ( :[...ARGS] ) }` | `multi` | – | T | parser normalizes alias `:[X]` -> `$X` and `:[...ARGS]` -> `$...ARGS`; printer canonicalizes | `SG~` |

### F. Runtime metadata

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-RT-01` | Changed since | `P2` | `changed:since=2024-01-01 panic!` | `multi`, recency desc | runtime | L,R | metadata catalog narrows doc set before content fanout | `Q+` |
| `UC-RT-02` | Affected by symbol | `P2` | `affected:fn=foo` | `multi` | runtime | R,S | invalidation catalog: docs whose derivatives depend on `fn foo` | `Q+` |
| `UC-RT-03` | Stale older than | `P6` | `stale:before=2024-01-01` | `multi` | runtime | R | snapshot catalog query | `Q+` |
| `UC-RT-04` | Snapshot pin | `P2` | `snapshot:HEAD~3 Iterator` | `multi` | runtime | L,R | reader binds to historical generation set | `Q+` |
| `UC-RT-05` | `invalidated_by:` | `P6` | `invalidated_by:rebuild=lexical` | `multi` | runtime | R | catalog returns docs marked invalid by named rebuild | `Q+` |
| `UC-RT-06` | Namespaced metadata | `P6` | `meta.owner:team-search Iterator` | `multi` | runtime | L,R | metadata table keyed by canonical doc identity | `Q+` |
| `UC-RT-07` | Combined runtime + metadata | `P6` | `changed:since=1d meta.layer:storage` | `multi`, recency desc | runtime | R | composed catalog query, no content engine touched | `Q+` |
| `UC-RT-08` | `dirty:` filter | `P2` | `dirty:yes meta.service:payments` | `multi` | runtime | R | apply-changes outbox lookup; result must include only uncommitted-locally docs | `Q+` |

### G. Bridge

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-BR-01` | `into:codeql` directive | `P3` | `unsafe { $_ } into:codeql` | bridge candidate packet (not `LexicalQueryResponse`) | directive | L,B | lexical materializes candidate set, bridge wraps with repo/rev/generation provenance | `Q+` |
| `UC-BR-02` | `scope:results` | `P3` | `repo:r1 dangerous_fn scope:results into:codeql` | bridge packet scoped to prior result set | directive | B | bridge consumes the lexical candidate set, not raw query text | `Q+` |
| `UC-BR-03` | `with:lexical` | `P3` | `with:lexical into:codeql /strcpy\(/` | bridge packet | directive | L,B | composition: explicit `with:lexical` selects upstream engine for the bridge | `Q+` |
| `UC-BR-04` | Bridge CodeQL parse fail | `P3` | `into:codeql:malformed-qls-payload Iterator` | `error:BRIDGE_REJECTED` | directive | B | bridge surfaces typed `BRIDGE_REJECTED` with downstream message; no silent skip | `Q+` |

### H. Cross-cutting / edge

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-EDGE-01` | Empty query | any | `` (empty string) | `error:PARSE_ERROR` | – | – | parser refuses; no `*`-everything semantics | `SG!` (SG allows in some contexts; quanta refuses always — see RFC §Non-Negotiable Invariants) |
| `UC-EDGE-02` | Unbalanced quote | any | `"unterminated phrase` | `error:PARSE_ERROR` | – | – | tokenizer error with position | `SG=` |
| `UC-EDGE-03` | Regex compile fail | `P3` | `/foo(/` | `error:PARSE_ERROR` | – | – | RE2 compile error surfaces at parse time | `SG=` |
| `UC-EDGE-04` | Unsupported combo: commit + structural | any | `type:commit match { fn $X { ... } }` | `error:UNSUPPORTED_COMBO` | type | – | planner rejects per RFC §Planner Model | `Q+` |
| `UC-EDGE-05` | Oversized request | `P6` | request body > 16 MiB | `error:OVERSIZED_REQUEST` | – | – | front-door rejects before parse | `Q+` |
| `UC-EDGE-06` | Timeout exceeded | any | `timeout:1ms count:all /.*/` | `error:TIMEOUT_EXCEEDED` | timeout | L | bounded executor cancels and surfaces typed error | `SG!` |
| `UC-EDGE-07` | Generation mismatch | `P6` | request with `request.generation = G_old`, `Iterator` | `error:GENERATION_MISMATCH` | – | – | catalog has advanced past `G_old`; fail-closed | `Q+` |
| `UC-EDGE-08` | ACL miss | any | `repo:secret/private Iterator` (tenant lacks read) | `error:ACL_DENIED` | repo | – | repo unauthorized; fail-closed empty + typed error envelope (no leakage of repo existence) | `Q+` |
| `UC-EDGE-09` | Multi-tenant isolation | any | tenant `A` query mentioning a `B`-only repo | `error:TENANT_ISOLATION` | repo | – | defense-in-depth on top of ACL; assert via two-tenant property test | `Q+` |
| `UC-EDGE-10` | Mixed-case filter name | `P5` | `Repo:foo bar` | `error:INVALID_FILTER` | – | – | filter names are case-sensitive lowercase | `SG=` |

### I. Operator / automation

| ID | Title | Persona | Golden query | Result shape | Filters | Engines | Acceptance | Parity |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `UC-OPS-01` | High-QPS saturation | `P6` | sustained `Iterator` at 5x designed QPS | mix of `multi` + `error:CANCELLED` | – | L | executor sheds load via bounded concurrency; no orphan work; metrics emitted per RFC §Execution Model | `Q+` |
| `UC-OPS-02` | Client cancellation | `P6` | client closes connection mid-query | `error:CANCELLED` | – | L | shard workers honor cancellation token; no goroutine/task leak | `Q+` |
| `UC-OPS-03` | Cold-start latency budget | `P6` | first `Iterator` after process boot | `multi` within budget X | – | L | warmup metric recorded; p99 cold-start within budget defined in `feature-scope.md` | `Q+` |
| `UC-OPS-04` | p99 latency target | `P6` | 10k-sample sweep of `UC-LEX-01` shape | aggregate p99 within budget | – | L | benchmark harness asserts steady-state p99 | `Q+` |
| `UC-OPS-05` | Deterministic merge | `P6` | same query, twice, same generation set | byte-identical `results` order | – | L | deterministic merge per RFC §Execution Model | `Q+` |
| `UC-OPS-06` | Explain output | `P4` | `Iterator` with explain envelope requested | `multi` + `SearchExplanation` populated | – | L | response includes the engine routing trace | `Q+` |
| `UC-OPS-07` | `count:all` determinism | `P4` | `deprecated_api count:all` twice | identical result count + order across runs | – | L | no probabilistic early-stop on `count:all` | `SG=` |

---

## 3. Result-shape contracts

Every `ok` row above must serialize as `SearchPlaneLexicalQueryResponse`. Error rows must serialize as the IPC error envelope carrying one code from §0.

### Contract gaps flagged (must be resolved before Wave-5 conformance)

| Gap ID | Affects | Issue | Proposed resolution |
| --- | --- | --- | --- |
| `GAP-01` | `UC-SYM-02` | `LexicalCandidate` does not carry `symbol_kind`. Symbol-kind queries can be planned, but the kind cannot be returned to the caller. | Either (a) add a `symbol_kind: Option<SymbolKind>` field to `LexicalCandidate` or (b) introduce a `SymbolCandidate` sibling type. Decision belongs in `dsl.md` / contract freeze. |
| `GAP-02` | `UC-HIST-01..08` | History results currently must squeeze into `LexicalCandidate`, which has no `commit_id`, `author`, `committed_at`, `parent`, or hunk metadata. | Introduce typed sibling `CommitCandidate` and `DiffHunkCandidate` and an enum on the response, or a dedicated `SearchPlaneHistoryQueryResponse`. |
| `GAP-03` | `UC-STR-01..07` | Structural results need metavariable bindings (e.g. `$X -> "foo"`). `LexicalCandidate.snippet` cannot carry typed bindings. | Add `StructuralCandidate { bindings: Map<MetaVar, Span> }` sibling, or attach a `structural_bindings` side-table keyed by `candidate_id`. |
| `GAP-04` | `UC-BR-01..04` | Bridge directive produces a candidate packet, not a `LexicalQueryResponse`. There is no contract type yet for the bridge envelope. | Define `BridgeCandidatePacket { generation, scope, candidates: Vec<LexicalCandidate>, bridge_target }` in the contract crate. |
| `GAP-05` | `UC-OPS-06` | `SearchExplanation` exists (referenced in `responses.rs`) but its public schema is not exercised by any current usecase. | Pin a minimum explain schema (planner trace + engines touched + early-stop reason) and add a serde roundtrip test. |
| `GAP-06` | `UC-EDGE-01..10`, `UC-OPS-01..02` | The typed-error envelope is implied throughout but the SCREAMING_SNAKE_CASE code enum is not yet in `quanta-index-contract`. | Add `LexicalQueryError { code: LexicalErrorCode, message: String, position: Option<TokenSpan> }` to the contract crate; codes match §0 table. |

Usecases that **cannot be expressed in the current LQ family without extension**:

- `UC-RT-08` (`dirty:`) — RFC §LQ/Runtime-1.3 lists `dirty:` but the runtime metadata catalog does not yet have an apply-changes outbox surface in the search plane. Conformance row stays in the corpus; the gating wave is `RT-01`.
- `UC-BR-01..04` — bridge tier (`LQ/Bridge-1.4`) is intentionally Wave-6; conformance rows must run in `pending` state until `BRIDGE-01` lands.

---

## 4. Anti-usecase catalog (MUST be rejected)

These queries are grammar-legal-looking but the kernel must refuse them. Each row is a golden negative test.

| Anti-ID | Description | Golden query | Expected error | Notes |
| --- | --- | --- | --- | --- |
| `AC-01` | Fuzzy match by default | `~similar_to_this` | `FORBIDDEN_SYNTAX` | RFC §Non-Negotiable Invariants: no fuzzy-by-default. Fuzzy may exist only as an explicit future operator. |
| `AC-02` | Generic `@` shorthand outside `repo:@rev` | `@lang=rust foo` | `FORBIDDEN_SYNTAX` | RFC §`@` policy; only `repo:<pat>@rev` is sugar. |
| `AC-03` | Generic `@file` shorthand | `@file=src/lib.rs foo` | `FORBIDDEN_SYNTAX` | Same as `AC-02`. |
| `AC-04` | SQL injection in filter value | `repo:foo'); DROP TABLE x;-- bar` | `INVALID_FILTER` | filter values are parsed as typed tokens, never concatenated into a query string. |
| `AC-05` | Regex backreference | `/(foo)\1/` | `FORBIDDEN_SYNTAX` | RE2-class only; no backreferences. |
| `AC-06` | Regex lookbehind | `/(?<=foo)bar/` | `FORBIDDEN_SYNTAX` | RE2-class only. |
| `AC-07` | Unbounded recursion in structural pattern | `match { $X($X($X(...))) }` with depth > planner cap | `PLAN_ERROR` | structural planner enforces a recursion depth ceiling. |
| `AC-08` | Request-time gitserver call | any predicate that would require a live git fetch | `PLAN_ERROR` | RFC §Canonical Incremental Write Pipeline forbids request-time git scan; predicate must resolve from indexed catalog only. |
| `AC-09` | `type:commit` + `match { ... }` | `type:commit match { fn $X { ... } }` | `UNSUPPORTED_COMBO` | RFC §Planner Model: unsupported combinations fail with typed planner error. |
| `AC-10` | Unknown filter name | `not_a_filter:value foo` | `INVALID_FILTER` | filter universe is closed. |
| `AC-11` | Helper-string lowering | (internal-only) request bypassing parser via raw helper string | `PARSE_ERROR` | RFC §Non-Negotiable Invariants: no hidden filter/ranking semantics in helper strings. |
| `AC-12` | Source-bound anchored fallback | request omitting all scope filters in product path | `PARSE_ERROR` or `PLAN_ERROR` | RFC §Non-Negotiable Invariants: no query-owned source-bound fallback in product path. |
| `AC-13` | Mixing `into:codeql` with `type:diff` | `type:diff diff.added:unwrap into:codeql` | `UNSUPPORTED_COMBO` | bridge is `with:lexical` only in Wave-6; history-bridge is not in scope. |
| `AC-14` | Tenant-leak attempt | tenant A query for tenant B repo id | `TENANT_ISOLATION` | defense-in-depth on top of `ACL_DENIED`. |
| `AC-15` | Stale explicit generation | request pinning a retired generation | `GENERATION_MISMATCH` | fail-closed per RFC §Generation model. |

---

## 5. Coverage matrix

Rows = LQ tier feature families. Columns = personas. `*` = primary user, `.` = occasional user, blank = not expected.

| Feature family | P1 reviewer | P2 SRE | P3 sec | P4 refactor | P5 onboard | P6 ops |
| --- | --- | --- | --- | --- | --- | --- |
| Keyword / phrase (`UC-LEX-01..04`) | `*` | `*` | `*` | `*` | `*` | `.` |
| Regex (`UC-LEX-05..06`, `UC-LEX-21`) | `.` | `*` | `*` | `.` |   | `.` |
| `repo:` / `repo@rev` (`UC-LEX-07..09`) | `*` | `*` | `*` | `*` | `.` | `*` |
| `file:` / `path:` (`UC-LEX-10..11`, `-file:`) | `*` | `.` | `*` | `*` | `.` | `.` |
| `lang:` (`UC-LEX-12`) | `.` |   | `*` | `*` | `*` | `.` |
| Boolean / NOT (`UC-LEX-13..15, 22..24`) | `*` | `.` | `*` | `*` | `.` | `.` |
| `case:` (`UC-LEX-16..17`) | `*` |   | `.` | `*` |   |   |
| `count:` (`UC-LEX-18..19`) |   |   | `*` | `*` | `.` | `*` |
| `timeout:` (`UC-LEX-20`) |   | `.` |   |   |   | `*` |
| `select:` (`UC-LEX-25..26`) |   |   | `.` |   | `*` | `*` |
| `fork:` / `archived:` (`UC-LEX-27..28`) |   |   | `*` |   |   | `.` |
| Predicate filters (`UC-PRED-01..07`) |   | `.` | `*` | `.` |   | `*` |
| Symbol search (`UC-SYM-01..06`) | `*` | `.` | `*` | `*` | `.` |   |
| History (`UC-HIST-01..08`) |   | `*` | `.` | `*` |   | `.` |
| Structural (`UC-STR-01..07`) | `.` |   | `*` | `*` |   |   |
| Runtime metadata (`UC-RT-01..08`) |   | `*` |   |   |   | `*` |
| Bridge (`UC-BR-01..04`) |   |   | `*` |   |   |   |
| Edge / errors (`UC-EDGE-01..10`) | `.` | `.` | `.` | `.` | `.` | `*` |
| Ops / automation (`UC-OPS-01..07`) |   | `.` |   |   |   | `*` |

### Gaps identified by the matrix

- **`UC-STR-04` (`outside` operator)**: only used by `P3`. Acceptable — `outside` is a quanta-extension and security audit is its primary client.
- **`UC-RT-05` (`invalidated_by:`)**: only `P6`. Acceptable — invalidation queries are inherently an operator concern; will be re-reviewed once a second non-operator consumer surfaces.
- **`UC-BR-04` (bridge parse fail)**: only `P3`. Acceptable — security audit is the only bridge user in v1.
- **No persona uses `select:repo`/`select:file` as primary except `P5`/`P6`.** Flag for scope review: confirm these projections are still in scope for `LEX-06` ranking work, or downgrade to v2.
- **No persona currently uses `LQ/Bridge-1.4` for anything other than security audit.** This matches RFC intent (bridge is downstream of lexical authority, not a broad-audience feature) — no scope change recommended.

---

## 6. Conformance reference plan

### CI gating

This corpus becomes a CI conformance gate as follows:

1. golden files live under `tools/ci/conformance/lq/` (proposed path, not yet created).
2. each `UC-*` and `AC-*` row maps 1:1 to one golden file.
3. CI rail: `cargo test -p quanta-index-contract --test lq_conformance` runs every golden file through the live parser + planner + (eventually) executor.
4. agent output schema applies: any conformance run that cannot produce evidence for a row reports that row as `blocked`, not `ok`, per `tools/ci/agent/agent_output.schema.json`.

### Golden file format (proposed)

TOML (preferred) — flat keyspace, easy to diff, no YAML indentation hazards:

```toml
id = "UC-LEX-01"
title = "Bare keyword"
persona = "P5"
query = "fooBar"
tier = "core"
engines = ["lexical_content"]
parity = "SG="

[expected]
kind = "ok"
shape = "multi"
ordering = "score_desc"
min_results = 1

[expected.response]
type = "SearchPlaneLexicalQueryResponse"

[[expected.invariants]]
all_snippets_contain = "fooBar"
```

Anti-usecase:

```toml
id = "AC-05"
title = "Regex backreference"
query = "/(foo)\\1/"

[expected]
kind = "error"
code = "FORBIDDEN_SYNTAX"
```

### Versioning policy

1. each row is pinned to a `parity` token (`SG=`, `SG~`, `Q+`, `SG!`).
2. when an upstream Sourcegraph release changes semantics for an `SG=` row:
   - **breaking-first posture (per CLAUDE.md)**: update the golden row to match the new upstream behavior in the same PR.
   - downgrade the row to `SG~` if the change cannot be matched without a quanta-side normalization step, and document the normalization.
   - flip to `SG!` only with explicit RFC amendment.
3. `Q+` rows do not track upstream; they version with the quanta contract crate version.
4. `SG!` rows must reference the RFC section that justifies the divergence.

### Authoring discipline

- adding a new usecase requires adding both the row here and a golden file in the same PR.
- removing a row requires an RFC amendment.
- a row may not move from `error` to `ok` (or vice versa) without an RFC amendment.

---

## 7. Out-of-corpus

The following Sourcegraph surface area is **deliberately excluded** from this conformance corpus. Each exclusion has a reason.

| Feature | Reason for exclusion |
| --- | --- |
| Saved queries | UI/persistence concern; not a parser/planner property. |
| Search contexts | Tenant-scoped naming layer; covered by ACL + tenant-isolation invariants (`UC-EDGE-08..09`), not by replicating Sourcegraph's context object. |
| Notebooks | Document format; not a query semantics property. |
| Result UI behaviors (highlight, hover, breadcrumb) | Frontend rendering; not part of the search plane contract. |
| Ranking quality benchmarks (NDCG, MRR vs. Sourcegraph) | Separate work stream owned by `LEX-06`; conformance corpus asserts ordering determinism and shape, not ranking quality. |
| Fuzzy / approximate match | Explicitly forbidden in `LQ/Core-1.0`; appears only in `AC-01`. |
| Code-intel "go to definition" cross-references | Belongs to a cross-ref planner family (RFC §Planner Model item 5), tracked separately. |
| Batch / streaming query APIs | Transport concern; this corpus is about query semantics, not wire format. |

---

## Appendix: Row count by category

| Category | Count |
| --- | --- |
| A. Lexical content | 28 |
| B. Predicate filters | 7 |
| C. Symbol | 6 |
| D. History | 8 |
| E. Structural | 7 |
| F. Runtime metadata | 8 |
| G. Bridge | 4 |
| H. Cross-cutting / edge | 10 |
| I. Operator / automation | 7 |
| **Usecase total** | **85** |
| Anti-usecase (`AC-*`) | 15 |
| **Conformance corpus total** | **100** |
