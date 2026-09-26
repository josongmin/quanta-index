# May-24 Lexical Kernel — Feature Scope Catalog

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


> Status: `Planning packet — scope SSOT for the LQ family`
> Parent: [rfc.md](rfc.md)
> Companion: [search-plane-implementation-tickets.md](../search-plane-implementation-tickets.md), [storage-architecture-endgame-implementation](../../ssot/may-23-storage-architecture-endgame-implementation.md)
> Authority posture: **breaking-first**. Features without a Claim Gate are not claimable. RFC text is not duplicated here — this doc adds the scope catalog, the cross-reference matrix, the Sourcegraph delta, the authority chain, the scale targets, the lifecycle states, and the open questions the RFC defers.
>
> **Architecture correction (2026-05-25)**: scope rows for LEX-05 / LEX-07 / STR-01 / RT-01 were originally authored assuming the search plane parses source bytes, walks git, or accepts a separate `apply_changes` IPC. The producer-authorship rule was ratified after that authoring — see [tickets/INDEX.md § 3.6](tickets/INDEX.md) for the full correction table and [docs/ssot/producer-handoff.md](../../ssot/producer-handoff.md) for the op catalogue. Truth-source rows below still name the producer correctly; engine-owner rows that mention "tree-sitter" or "git-walk" should be read as "producer-emitted parse tree / commit record decoded over the channel".

## 0. How to read this doc

- The RFC owns the **architecture and ticket pack**. This doc owns the **scope catalog** the RFC builds against.
- Every in-scope feature has: one-line summary, Sourcegraph equivalence (or "QI-extension"), engine owner, claim gate.
- Every Sourcegraph feature we do **not** ship is either in §2 (forbidden) or §3 (deferred-with-reason). Anything that is neither is an unscoped gap — flag it.
- Every in-scope feature appears in §4 with a ticket reference; gaps are explicitly flagged.

---

## 1. In-scope feature catalog

### 1.1 `LQ/Core-1.0` — Sourcegraph-compatible lexical core

Engine owner for all rows below: **lexical content engine** (Tantivy chunk index + sibling path/symbol indices) unless noted. See RFC § Engine Decomposition § Lexical content engine.

#### 1.1.1 Pattern leaves

| Feature | Summary | Sourcegraph equivalent | Engine | Claim gate |
|---|---|---|---|---|
| keyword leaf | bare token → stemmed keyword match against chunk text | bare word (`literal` / `standard` / `keyword` modes) | lexical content | parser + planner accepts; lexical content index returns hits; **no fuzzy default** asserted by negative test |
| exact phrase leaf | `"..."` → phrase match (token sequence) | `"..."` literal phrase | lexical content | phrase positions tracked in index; phrase test corpus golden set passes |
| raw string leaf | `'...'` → raw byte literal, minimal escaping | Sourcegraph raw escape (`patterntype:standard` raw segment) | lexical content | raw escapes preserved through parse → plan → recall round-trip |
| regex leaf | `/.../` → regex pattern against chunk text | `patterntype:regexp` or `/.../` segment | lexical content | regex compiled at plan time; bounded state count (see §7); golden regex corpus passes |

Note: fuzzy / approximate / suggestion is **not** a Core-1.0 leaf — see §3 (deferred-with-reason).

#### 1.1.2 Boolean composition

| Feature | Summary | Sourcegraph equivalent | Engine | Claim gate |
|---|---|---|---|---|
| AND | `a AND b` and adjacency `a b` | Sourcegraph default adjacency = AND | parser → planner | AST has `LqExpr::All`; adjacency normalization unit test |
| OR | `a OR b` | `a OR b` | parser → planner | AST has `LqExpr::Any`; precedence test passes |
| NOT | `NOT a` and `-a` | `-foo`, `NOT foo` | parser → planner | AST has `LqExpr::Not`; precedence `NOT > AND > OR` test passes |
| group | `(...)` precedence override | `(...)` | parser | parser preserves group structure; printer round-trip equal AST |
| precedence | `NOT > AND > OR` | matches Sourcegraph | parser | golden precedence corpus passes |
| negation prefix | `-foo` ≡ `NOT foo` | `-foo` | parser | parser normalizes `-` prefix into `LqExpr::Not` (one canonical form) |

#### 1.1.3 Filters — Sourcegraph baseline (RFC § Design Baseline)

| Filter | Summary | Sourcegraph equivalent | Engine | Claim gate |
|---|---|---|---|---|
| `repo:<pattern>` | restrict to repo whose name matches pattern | identical | catalog (repo resolver) → lexical | repo resolver returns set; planner pushes scope down; golden corpus |
| `file:<pattern>` | restrict to file whose path matches pattern | identical (canonical) | lexical path index | path index returns set; planner pushes scope down |
| `path:<pattern>` | alias of `file:` (Sourcegraph compat) | `path:` alias of `file:` | lexical path index | parser normalizes `path:` → `file:` in canonical AST (one path-scope rep per RFC § Filter semantics) |
| `lang:<id>` | restrict to language | identical | catalog (language metadata) | language metadata authority answers; planner pushes scope down |
| `rev:<rev>` | restrict to revision | identical | catalog (revision resolver) | revision resolver answers; planner pins generation; cross-references manifest catalog |
| `type:file\|path\|symbol\|commit\|diff` | dispatch query class | identical | planner | planner routes query family per RFC § Planner Model |
| `select:<projection>` | output projection (file / repo / symbol / content) | identical | planner / result assembler | projection applied at result-emit time; golden projection corpus |
| `count:<n\|all>` | result cap / explicit all | identical | executor | bound respected by deterministic merge (RFC § Execution Model) |
| `case:yes\|no` | case sensitivity | identical | parser → lexical content | tokenizer config switch; golden case corpus |
| `fork:yes\|no\|only` | include/exclude forks | identical | catalog (repo metadata) | repo metadata filter applied at scope resolution |
| `archived:yes\|no\|only` | include/exclude archived | identical | catalog (repo metadata) | repo metadata filter applied at scope resolution |

#### 1.1.4 Filters — Sourcegraph compat the RFC omitted

These exist in Sourcegraph but are not enumerated in RFC § `LQ/Core-1.0`. Inclusion ensures `Sourcegraph query ⊂ LQ/Core-1.0` (RFC § Compatibility Rules § hard compatibility goal). Each must be parser-accepted; semantic implementation may be deferred per row.

| Filter | Summary | Sourcegraph equivalent | Engine | Claim gate / status |
|---|---|---|---|---|
| `content:<pattern>` | explicit content pattern carrier (separates pattern from filter chain) | `content:"..."` | parser → lexical content | parser accepts; lowers to content-pattern leaf in canonical AST; **must ship in Core-1.0 for parse compat** |
| `visibility:public\|private\|any` | repo visibility filter | identical | catalog (repo metadata) | repo metadata authority answers; **parse compat MUST ship; semantic eval gated on producer metadata surface (flag — see §9 Q3)** |
| `patterntype:literal\|keyword\|standard\|regexp\|structural` | leaf interpretation mode | identical | parser-level mode | parser branches on mode; each mode test corpus passes — see §1.1.6 |
| `context:<name>` | named search context (saved scope) | identical | catalog (context store) | **deferred-with-reason (§3)** — namespace reserved; parser accepts and emits typed `NotImplemented` |
| `boost:<n>` | per-clause boost weight | identical | planner / rerank | parser accepts; lowered to `LqOptionSet`; ranking ties to LEX-06 |
| `index:yes\|no\|only` | request use/skip of indexed path | identical | planner | parser accepts; planner respects; Phase-1 only honors `index:only` since we are index-only (no fallback scan) |
| `timeout:<duration>` | request-level timeout override | identical | executor | parsed into `LqOptionSet::timeout`; executor honors bounded deadline |

#### 1.1.5 Predicate filters (Sourcegraph predicates)

Sourcegraph exposes predicates of the form `<filter>:<predicate>(...args)`. Each MUST be representable in the canonical AST even if executor support is deferred. Anything not implemented MUST fail closed with typed `NotImplemented` (RFC § Non-Negotiable Invariants).

| Predicate | Summary | Engine | Phase-1 status | Claim gate |
|---|---|---|---|---|
| `repo:has.file(<path-pattern>)` | restrict to repos containing a matching file | catalog + lexical path index | parser-only Phase 1; full eval Wave 2/3 | parser accepts; canonical AST carries `LqFilter::Repo(RepoPredicate::HasFile(...))`; eval gate requires path-index pushdown |
| `repo:has.commit.after(<timeframe>)` | restrict to repos with commits after timeframe | history engine + catalog | parser-only Phase 1; eval Wave 4 | requires history engine (LEX-07) |
| `repo:has.path(<path-pattern>)` | restrict to repos that have any matching path | catalog + lexical path index | parser Phase 1; eval Wave 2 | path index pushdown |
| `file:contains(<pattern>)` | restrict to files containing a content match | lexical content | parser Phase 1; eval Wave 2 | sub-recall pushdown |
| `file:has.content(<pattern>)` | alias of `file:contains` (newer SG syntax) | lexical content | parser Phase 1; eval Wave 2 | aliased to `file:contains` in canonical AST |

Open question Q1 (§9): are predicates evaluated at parse time (catalog static) or query time (executor)? RFC does not decide.

#### 1.1.6 `patterntype:` modes

| Mode | Semantics summary | Engine | Claim gate |
|---|---|---|---|
| `literal` | Sourcegraph default literal interpretation; whitespace = adjacency = AND; special chars escaped | parser | golden corpus reproducing SG `literal` results |
| `keyword` | newer SG default; bare tokens stemmed; quotes for exact | parser + tokenizer | tokenizer `en_stem` Phase 1 ([D4](../search-plane-implementation-tickets.md#open-design-decisions-central-register)); golden stemmed corpus |
| `standard` | newer SG hybrid: regex segments + literal segments | parser | parser mode-switch corpus |
| `regexp` | every pattern leaf treated as regex | parser → regex leaf | regex bounded-state corpus (§7) |
| `structural` | leaves are structural templates (handed to STR-01) | parser → structural planner | dispatches to structural engine; lexical does **not** pretend Tantivy regex is structural search (RFC § Structural engine § must not) |

#### 1.1.7 Sugar / normalization

| Sugar | Canonicalization | Source | Claim gate |
|---|---|---|---|
| `repo:<pattern>@rev` | `repo:<pattern> rev:<rev>` | RFC § Filter semantics, § Normalization examples | parser produces canonical AST; printer can re-print sugar or canonical (one mode) |
| `:[X]` | `$X` (structural metavar) | RFC § Normalization examples | structural parser normalizes |
| `:[...ARGS]` | `$...ARGS` (variadic structural metavar) | RFC § Normalization examples | structural parser normalizes |
| `foo bar` | `foo AND bar` | RFC § Normalization examples | parser adjacency normalization |

#### 1.1.8 Forbidden in Core-1.0 grammar

Per RFC § `@` policy:

- generic `@repo`, `@lang`, `@file` shorthand — **rejected at parse time** with typed error
- only `repo:<pattern>@rev` sugar is allowed in `LQ/Core-1.0`
- broader `@` UX shorthand belongs to editor/UI alias layer, not canonical grammar

Claim gate: parser test exists that rejects generic `@` shorthand with `CoreError::InvalidContract`.

---

### 1.2 `LQ/History-1.1` — history / commit / diff search

Engine owner for all rows: **history engine** (commit metadata index + diff hunk content index + revision catalog). See RFC § History engine.

#### 1.2.1 Type dispatch

| Feature | Summary | Sourcegraph equivalent | Claim gate |
|---|---|---|---|
| `type:commit` | search commit metadata (message/author/date) | identical | history planner reachable; commit metadata index returns hits |
| `type:diff` | search diff content | identical | history planner reachable; diff hunk index returns hits |

#### 1.2.2 Author / message / time

| Feature | Summary | Sourcegraph equivalent | Claim gate |
|---|---|---|---|
| `author:<pattern>` | match author name/email | identical | commit metadata index field |
| `committer:<pattern>` | match committer (vs author) | identical | commit metadata field separate from `author:` |
| `message:<pattern>` | match commit message body | identical | commit metadata field |
| `before:<timeref>` | commits before timestamp/relative | identical | commit metadata index time-range pushdown |
| `after:<timeref>` | commits after | identical | as above |
| `since:<timeref>` | inclusive `after` semantics (SG synonym) | identical | parser normalization or kept as alias; pick one in canonical AST |
| `until:<timeref>` | inclusive `before` synonym | identical | as above |

#### 1.2.3 Diff content filters

| Feature | Summary | Sourcegraph equivalent | Claim gate |
|---|---|---|---|
| `diff.added:<pattern>` | pattern present in added hunk lines | identical | diff hunk index has added-text field |
| `diff.removed:<pattern>` | pattern present in removed lines | identical | as above |
| `diff.touched:<pattern>` | pattern present in either added or removed lines | identical | as above |

#### 1.2.4 RFC-omitted history filters

These exist in Sourcegraph or are required for parity with common history-search workflows. RFC § `LQ/History-1.1` did not enumerate them — adding here so they are not lost.

| Feature | Summary | Sourcegraph equivalent | Phase status | Claim gate |
|---|---|---|---|---|
| `parent:<rev>` | commits with given parent | implicit via `repo:@rev` traversal in SG | Wave 4 | history planner accepts; commit catalog parent edge |
| `merge:yes\|no\|only` | include/exclude merge commits | identical | Wave 4 | commit metadata flag |
| `tag:<pattern>` | commits reachable from matching tag | identical | Wave 4 | tag catalog resolver |
| `revisions:<range>` | rev range `a..b` or `a...b` | identical | Wave 4 | revision range resolver |
| `since.time:` / `since.commit:` | qualified `since:` (time-based vs commit-based) | SG composite via `rev:` | Wave 4 | parser disambiguates; canonical AST carries one |

Open question Q2 (§9): is `since:` a generic alias that lowers to one of `since.time:` / `since.commit:` at parse time, or does the executor decide?

---

### 1.3 `LQ/Structural-1.2` — Semgrep-style structural search

Engine owner for all rows: **structural engine** (tree-sitter AST cache + normalized pattern IR + matcher runtime). See RFC § Structural engine.

#### 1.3.1 Pattern primitives

| Feature | Summary | Sourcegraph equivalent | Claim gate |
|---|---|---|---|
| `match { ... }` | structural pattern block | Sourcegraph `patterntype:structural` body | structural parser accepts; structural matcher runs against tree-sitter AST |
| `$X` | metavariable single | Sourcegraph `:[X]` (alias-normalized) | matcher captures; bindings exposed in result row |
| `$...X` | variadic metavariable | Sourcegraph `:[...ARGS]` (alias-normalized) | matcher captures variadic |
| `...` | wildcard ellipsis | Semgrep `...` | matcher supports between-nodes wildcard |
| `where <clause>` | constraint clause | Semgrep `where` | constraint evaluator over captures |
| `inside <pat>` | enclosing-context constraint | Semgrep `pattern-inside` | matcher checks ancestor scope |
| `outside <pat>` | negative-context constraint | Semgrep `pattern-not-inside` | matcher excludes ancestor scope |

#### 1.3.2 Alias normalization

| Sourcegraph form | Canonical form | Claim gate |
|---|---|---|
| `:[X]` | `$X` | parser normalizes once at AST construction |
| `:[...ARGS]` | `$...ARGS` | parser normalizes once |

#### 1.3.3 Sourcegraph richer structural

| Feature | Summary | Phase status | Claim gate |
|---|---|---|---|
| `:[hole.type1]` (type-constrained hole) | metavariable restricted to syntactic kind (expr, stmt, type-ref, ...) | **deferred-with-reason** (§3) Wave 5+ | namespace reserved (`$X : <kind>`); parser rejects with typed `NotImplemented` until matcher supports |

#### 1.3.4 Supported language set

Initial tree-sitter language set (Wave 5, STR-01). RFC enumerates none — fixing here.

| Language | tree-sitter grammar | Phase status |
|---|---|---|
| Rust | tree-sitter-rust | STR-01 ship |
| Python | tree-sitter-python | STR-01 ship |
| TypeScript / TSX | tree-sitter-typescript | STR-01 ship |
| JavaScript | tree-sitter-javascript | STR-01 ship |
| Go | tree-sitter-go | STR-01 ship |
| Java | tree-sitter-java | STR-01 stretch |
| C / C++ | tree-sitter-c, tree-sitter-cpp | post-STR-01 |
| Ruby | tree-sitter-ruby | post-STR-01 |

Open question Q4 (§9): is structural matching language-aware (per-grammar pattern IR) or unified across grammars (one pattern IR with per-grammar adapter)?

---

### 1.4 `LQ/Runtime-1.3` — runtime-aware metadata filters

Engine owner for all rows: **runtime metadata engine** (snapshot catalog + invalidation catalog + ownership/service/layer registry). See RFC § Runtime metadata engine.

#### 1.4.1 State-aware filters

The RFC enumerates `changed:` / `dirty:` / `stale:` / `affected:` but does not define semantic differences. Fixing here:

| Filter | Definition | Truth source | Authority |
|---|---|---|---|
| `changed:<scope>` | doc identity differs between two named snapshots (e.g. base vs head, prepared vs active) | prepared bundle delta vs currently active generation set | apply-changes catalog / `bundle_delta_applied` (T1.1) |
| `dirty:<scope>` | working tree state differs from latest indexed generation (mutation present, not yet a prepared bundle) | producer working-tree probe — **producer surface; search-plane consumes only when producer marks `dirty`** | producer's working-tree authority surfaced via prepared-bundle metadata field |
| `stale:<scope>` | indexed generation is older than producer's currently published head | generation catalog vs producer's latest prepare timestamp | generation catalog (`generation_catalog`) + producer publish channel |
| `affected:<scope>` | doc identity is downstream of a changed source (semantic-derivative or import-reverse) | invalidation catalog (RFC § Generation model semantic derivative) | invalidation catalog (post-Wave 5) |
| `invalidated_by:<doc_id>` | this doc's derivative was invalidated by a specific source | invalidation catalog edges | invalidation catalog |
| `snapshot:<name>` | scope queries to a named snapshot (e.g. `prepared`, `active`, named historical) | generation catalog | generation catalog state machine |

Open question Q5 (§9): is `dirty:` a real filter for the search-plane (read-only consumer) or strictly a producer-side concept? RFC says metadata filters belong here, but the truth source is producer.

#### 1.4.2 Metadata namespaced filters

| Filter | Summary | Truth source | Claim gate |
|---|---|---|---|
| `meta.owner:<id>` | owning team / human / service | ownership/service registry (post-Wave 5) | metadata catalog table; row exists; planner pushdown |
| `meta.service:<id>` | logical service identifier | ownership registry | as above |
| `meta.layer:<id>` | architectural layer tag | ownership registry | as above |
| `meta.surface:<id>` | surface / API boundary tag | ownership registry | as above |

These are **QI-extensions** beyond Sourcegraph (see §5).

---

### 1.5 `LQ/Bridge-1.4` — lexical candidate bridge

Engine owner for all rows: **bridge engine** (candidate export packet + downstream invocation builder + result-scope carrier). See RFC § Bridge engine.

#### 1.5.1 Directives

| Directive | Summary | Sourcegraph equivalent | Payload schema | Claim gate |
|---|---|---|---|---|
| `into:codeql` | route materialized lexical candidate set to CodeQL execution | none (QI-extension) | future contract type `LqDirective::IntoCodeQL { candidate_packet: LexicalCandidatePacket, query_spec: CodeQlQuerySpec }` | typed packet round-trips contract validator; CodeQL invocation builder accepts |
| `scope:results` | constrain outer query to a previous bridge result set | none (QI-extension) | future contract type `LqDirective::ScopeResults { handle: BridgeResultHandle }` | result-scope carrier resolves handle to a bounded candidate set |
| `with:lexical` | annotate that downstream engine must treat lexical output as **candidate set**, not authority | none (QI-extension) | future contract type `LqDirective::WithLexical` | downstream invocation builder asserts candidate-not-authority discipline |

#### 1.5.2 Error semantics when bridge target rejects

**FS-GAP-2 closure (2026-05-25)** per [tickets/INDEX.md § 3.4](tickets/INDEX.md) and [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md): the canonical bridge error set is locked to the five codes below plus the legacy three. The lock is mirrored verbatim in [rfc.md § Error Code Taxonomy § `BRIDGE_*`](rfc.md) so the two docs cannot drift.

| Condition | Canonical code | Source |
|---|---|---|
| Sourcegraph filter name has no LQ projection | `BRIDGE_UNSUPPORTED_FILTER` | [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) |
| Sourcegraph directive (`index:no`, fuzzy `~`, generic `@`, empty input) refused | `BRIDGE_UNSUPPORTED_DIRECTIVE` | [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) |
| Sourcegraph filter resolves to ≥ 2 LQ targets (defensive fail-closed) | `BRIDGE_AMBIGUOUS_FILTER` | [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) |
| producer's translator version disagrees with consumer's expected skew window | `BRIDGE_VERSION_PIN` | [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) (renamed from `BRIDGE_TRANSLATOR_VERSION_SKEW`) |
| SG syntax was parser-accepted by SG-side but the translator produced no LQ AST (defensive — translator-internal bugs) | `BRIDGE_TRANSLATE_FAIL` | [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) |
| downstream sink (e.g. CodeQL) refused the candidate packet | `BRIDGE_SINK_REJECTED` | [rfc.md § Error Code Taxonomy § `BRIDGE_*`](rfc.md) |
| candidate packet failed contract-crate validation | `BRIDGE_CANDIDATE_FORMAT_INVALID` | [rfc.md § Error Code Taxonomy § `BRIDGE_*`](rfc.md) |
| candidate set empty | `CoreError::NotFound` (not empty Ok per fail-closed policy) | repo-wide convention (`CoreError::NotReady` for absent-authority, `NotFound` for empty-with-authority-present) |

> Historical note: prior versions of this row listed `BRIDGE_CANDIDATE_OVERFLOW`, `BRIDGE_TARGET_UNAVAILABLE`, `BRIDGE_PROVENANCE_REJECTED`. Those names are superseded by the canonical set; the corresponding conditions surface under `BRIDGE_SINK_REJECTED` (overflow / target / provenance carried in `reason`) per the BRIDGE-01 lock.

---

## 2. Out of scope (forbidden)

Per RFC § Non-Goals and SSOT § Out of Scope. Each is a **hard reject** — implementation MUST NOT add a heuristic success path here.

| Item | Reason | Source |
|---|---|---|
| callgraph / dataflow / taint / PTA / semantic reasoning at lexical layer | lexical layer owns candidate authority only; semantic reasoning belongs to IR/exactifier lanes | RFC § Non-Goals, § Lexical content engine § must not |
| silently widening query semantics with fuzzy defaults | fuzzy must be explicit future operator; bare word is keyword | RFC § Non-Goals, § Pattern semantics |
| GQLang execution absorption | GQLang semantics live elsewhere | RFC § Non-Goals |
| mixing planner directives and analysis payloads in one untyped request bag | planner directives are typed; analysis payloads are typed; no untyped bag | RFC § Non-Goals |
| ad-hoc per-request git scans in product path | history must be indexed; no request-time `git log` | RFC § Canonical Incremental Write Pipeline § forbidden |
| per-query rebuild | steady-state writes are file/chunk delta scoped | RFC § Canonical Incremental Write Pipeline § forbidden |
| silent full-corpus rebuild in steady state | only bootstrap/recovery may full-rebuild | RFC § Generation model § rules |
| HTTP / TLS transport on search-plane | UDS only Phase 3; HTTP is future Phase 4+ | SSOT § What this plan does NOT cover |
| user authentication on UDS path | filesystem permissions only Phase 1; authz Phase 4+ | SSOT § What this plan does NOT cover |
| empty-result fallback when authority is absent | must return `CoreError::NotReady`, not empty `Vec` | search-plane-implementation-tickets § Fail-closed posture |
| heuristic success path when authoritative path is missing | breaking-first posture; explicit `NotImplemented` instead | CLAUDE.md § Agent change posture |

---

## 3. Deferred with reason

These are Sourcegraph (or general search-system) features we do **not** ship in the initial LQ family but **reserve the namespace** for. Parser MAY accept and emit typed `NotImplemented`; implementation lands in a later wave.

| Feature | Reason for deferral | Possible wave (if ever) |
|---|---|---|
| fuzzy / approximate / suggestion / typo-tolerance | RFC § Pattern semantics requires explicit future operator; not default. Belongs to UI/UX layer, not lexical kernel grammar | post-Wave 8 (new RFC required) |
| `r:` `f:` short aliases (Sourcegraph UI shorthand) | UI alias layer per RFC § `@` policy intent; not canonical grammar | UI / editor layer, never in core grammar |
| saved queries | stateful per-user feature; not a kernel concern | Phase 4+ |
| `context:<name>` (named search contexts) | requires user/identity model that does not exist Phase 1 (no auth Phase 1) | Phase 4+ (gated on authz) |
| ranking ML / personalization | RFC § Non-Negotiable Invariants: "no hidden filter/ranking semantics in helper string functions". Deterministic explainable rerank only Phase 1 | post-LEX-06 follow-up |
| diff visualization / blame integration | UI presentation layer; lexical kernel owns candidate truth only | UI / editor layer |
| multi-tenant authz / row-level visibility | filesystem perms Phase 1 only | Phase 4+ |
| `:[hole.type1]` typed structural holes | requires per-grammar type system in pattern IR | Wave 5+ (post-STR-01) |
| Sourcegraph `repo:contains.path` / `repo:contains.commit` aliases | newer SG aliases; canonical predicate forms `repo:has.path` / `repo:has.commit.after` cover them | parser-level alias added when SG canonical drift forces it |
| HTTP search API | UDS only Phase 3 | Phase 4+ |
| federated multi-repo cross-cluster search | single-cluster Phase 1 | Phase 4+ |
| query auto-complete / token suggest | UI / editor layer, not kernel | UI layer |

Distinction reminder: **out-of-scope (§2) = forbidden, will never be silently added**. **deferred-with-reason (§3) = namespace reserved, parser may pre-accept, executor returns typed `NotImplemented` until ticketed**.

---

## 4. Feature × ticket cross-reference matrix

Tickets are from RFC § Ticket Pack and § Canonical Execution Waves. Every in-scope feature MUST map to ≥1 ticket. Rows flagged "GAP" have no ticket — see end of section.

### 4.1 Core-1.0

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| keyword leaf | Core-1.0 | lexical content | LEX-01 (AST/parser), LEX-03 (lexical authority unification), LEX-06 (ranking/semantics) | 1, 2, 4 | parser conformance + global execution proof |
| exact phrase leaf | Core-1.0 | lexical content | LEX-01, LEX-03 | 1, 2 | phrase corpus golden set |
| raw string leaf | Core-1.0 | lexical content | LEX-01 | 1 | parser round-trip |
| regex leaf | Core-1.0 | lexical content | LEX-01, LEX-03, LEX-06 | 1, 2, 4 | bounded-state regex test |
| AND / OR / NOT / group / precedence / negation prefix | Core-1.0 | parser→planner | LEX-01 | 1 | precedence golden corpus |
| `repo:` | Core-1.0 | catalog→lexical | LEX-01, LEX-02 (front door), LEX-03 | 1, 2 | repo resolver returns set |
| `file:` / `path:` (alias) | Core-1.0 | lexical path | LEX-01, LEX-03 | 1, 2 | path index pushdown |
| `lang:` | Core-1.0 | catalog | LEX-01, LEX-03 | 1, 2 | language metadata answer |
| `rev:` | Core-1.0 | catalog | LEX-01, LEX-04 (incremental indexing kernel) | 1, 3 | revision resolver + generation pin |
| `type:file\|path\|symbol` | Core-1.0 | planner | LEX-01, LEX-05 (parallel executor) | 1, 3 | planner routes correctly |
| `type:commit\|diff` | Core-1.0 dispatch / History-1.1 exec | planner→history | LEX-01 (parse), LEX-07 (history engine) | 1, 4 | history planner reachable |
| `select:` | Core-1.0 | result assembler | LEX-01, LEX-06 | 1, 4 | projection corpus |
| `count:` | Core-1.0 | executor | LEX-01, LEX-05 | 1, 3 | bounded merge respects count |
| `case:` | Core-1.0 | parser→content | LEX-01, LEX-03 | 1, 2 | case corpus |
| `fork:` | Core-1.0 | catalog | LEX-01, LEX-02 | 1, 2 | repo metadata filter |
| `archived:` | Core-1.0 | catalog | LEX-01, LEX-02 | 1, 2 | repo metadata filter |
| `content:` | Core-1.0 | parser→lexical content | LEX-01 | 1 | parser lowers to content pattern leaf |
| `visibility:` | Core-1.0 | catalog | LEX-01 (parse), LEX-02 (front door incl. repo metadata) | 1, 2 | **GAP candidate** — parser accepts in LEX-01; semantic eval depends on producer surface (§9 Q3) |
| `patterntype:` (all modes) | Core-1.0 | parser-level mode | LEX-01 | 1 | each mode corpus passes; structural mode dispatches to STR-01 |
| `context:` | Core-1.0 (parse) | n/a (deferred §3) | LEX-01 (parse-only) | 1 | parser accepts; executor `NotImplemented` |
| `boost:` | Core-1.0 | planner/rerank | LEX-01 (parse), LEX-06 (ranking) | 1, 4 | option carried into rerank |
| `index:` | Core-1.0 | planner | LEX-01 | 1 | Phase-1 only honors `only` |
| `timeout:` | Core-1.0 | executor | LEX-01, LEX-05 | 1, 3 | executor deadline honored |
| `repo:<pattern>@rev` sugar | Core-1.0 | parser | LEX-01 | 1 | normalization test |
| `repo:has.file(...)` predicate | Core-1.0 (parse) / Core-1.0 (eval) | catalog + lexical path | LEX-01 (parse), LEX-03 (path index pushdown) | 1, 2 | parser carries `RepoPredicate`; eval gate Wave 2 |
| `repo:has.commit.after(...)` predicate | Core-1.0 (parse) / History-1.1 (eval) | history | LEX-01 (parse), LEX-07 (history) | 1, 4 | history-engine eval gate |
| `repo:has.path(...)` predicate | Core-1.0 (parse+eval) | catalog + lexical path | LEX-01, LEX-03 | 1, 2 | path pushdown |
| `file:contains(...)` predicate | Core-1.0 (parse+eval) | lexical content | LEX-01, LEX-03 | 1, 2 | content sub-recall pushdown |
| `file:has.content(...)` predicate | Core-1.0 (parse) | aliased to `file:contains` | LEX-01 | 1 | parser canonical normalization |
| `@`-shorthand rejection | Core-1.0 | parser | LEX-01 | 1 | parser reject test |

### 4.2 History-1.1

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| `type:commit` / `type:diff` | History-1.1 | history | LEX-07 | 4 | commit/diff index serves hits |
| `author:` / `committer:` / `message:` | History-1.1 | history | LEX-07 | 4 | commit metadata field tests |
| `before:` / `after:` / `since:` / `until:` | History-1.1 | history | LEX-07 | 4 | time-range pushdown |
| `diff.added:` / `diff.removed:` / `diff.touched:` | History-1.1 | history | LEX-07 | 4 | diff hunk index test |
| `parent:` / `merge:` / `tag:` / `revisions:` / `since.time:` vs `since.commit:` | History-1.1 (extension) | history | LEX-07 | 4 | **GAP if LEX-07 ticket body does not enumerate these; flag for ticket extension** |

### 4.3 Structural-1.2

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| `match { ... }` | Structural-1.2 | structural | STR-01 | 5 | matcher exists |
| `$X` / `$...X` / `...` | Structural-1.2 | structural | STR-01 | 5 | metavariable capture |
| `where` / `inside` / `outside` | Structural-1.2 | structural | STR-01 | 5 | constraint evaluator |
| `:[X]` / `:[...ARGS]` alias normalization | Structural-1.2 | parser | LEX-01 (alias) + STR-01 (matcher) | 1, 5 | parser normalizes once |
| `:[hole.type1]` typed holes | deferred §3 | structural | post-STR-01 | post-5 | namespace reserved |
| tree-sitter language set (Rust/Python/TS/JS/Go ship, Java stretch) | Structural-1.2 | structural substrate | STR-01 | 5 | per-language corpus golden set |

### 4.4 Runtime-1.3

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| `changed:` | Runtime-1.3 | runtime metadata + apply-changes catalog | RT-01 | 5 | delta catalog edge present |
| `dirty:` | Runtime-1.3 | runtime metadata (producer-sourced) | RT-01 | 5 | **GAP candidate** — depends on producer marking `dirty`; needs cross-repo coordination (§9 Q5) |
| `stale:` | Runtime-1.3 | runtime metadata (catalog age) | RT-01 | 5 | generation catalog age vs producer head |
| `affected:` | Runtime-1.3 | runtime metadata (invalidation catalog) | RT-01 + SEM-02 (incremental semantic derivatives) | 5, 7 | invalidation catalog edge |
| `invalidated_by:` | Runtime-1.3 | runtime metadata | RT-01 + SEM-02 | 5, 7 | invalidation catalog edge |
| `snapshot:` | Runtime-1.3 | generation catalog | RT-01 | 5 | snapshot state machine |
| `meta.owner:` / `meta.service:` / `meta.layer:` / `meta.surface:` | Runtime-1.3 | ownership registry | RT-01 | 5 | metadata registry table populated |

### 4.5 Bridge-1.4

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| `into:codeql` | Bridge-1.4 | bridge | BRIDGE-01 | 6 | typed candidate packet exists; CodeQL invocation builder accepts |
| `scope:results` | Bridge-1.4 | bridge | BRIDGE-01 | 6 | result-scope carrier resolves handle |
| `with:lexical` | Bridge-1.4 | bridge | BRIDGE-01 | 6 | candidate-not-authority discipline asserted in downstream call |

### 4.6 Cross-cutting

| Feature | LQ tier | Engine | RFC ticket | Wave | Claim gate |
|---|---|---|---|---|---|
| baseline / invariants freeze | all | n/a | LEX-00 | 1 | invariants doc + golden set baseline |
| global front door / surface contract cutover | Core-1.0 | n/a | LEX-02 | 2 | request owner is typed AST, not bag-of-fields |
| incremental lexical indexing kernel | Core-1.0 (write path) | lexical | LEX-04 | 3 | file-delta write/read generation proof |
| parallel executor + deterministic merge | Core-1.0 | executor | LEX-05 | 3 | deterministic merge proof + executor metrics |
| ranking, explain, lexical semantics | Core-1.0 | rerank | LEX-06 | 4 | explain artifact + deterministic ranking proof |
| semantic on lexical filter pushdown | (cross-domain) | semantic | SEM-01 | 6 | post-filter residue removed; lexical-universe planning is canonical |
| incremental semantic derivatives | (cross-domain) | semantic | SEM-02 | 7 | delta mutation proof |
| conformance, fences, final proof | all | conformance suite | OBS-01 | 8 | full conformance corpus + fences pass |

### 4.7 Flagged gaps (features without unambiguous ticket coverage)

- **`visibility:` semantic eval** — LEX-01 covers parser only; semantic eval depends on producer publishing repo-visibility metadata. No current ticket owns the catalog surface for this. → **flag for new sub-ticket in LEX-02 or new R-prefix ticket.**
- **`parent:` / `merge:` / `tag:` / `revisions:` / `since.time:` / `since.commit:`** — LEX-07 ticket body in RFC does not enumerate; require LEX-07 scope extension or sub-tickets. → **flag for LEX-07 scope amendment.**
- **`dirty:`** — RT-01 owns the filter but the truth source is producer (working-tree probe). No producer-side coordination is currently in scope per SSOT § Out of Scope. → **flag as cross-repo dependency (§9 Q5).**
- **`context:` (Core-1.0 parse-only acceptance)** — LEX-01 parses but executor `NotImplemented`. Long-term lifecycle owner is unclear (Phase 4+ authz). → **flag for explicit deferral note in LEX-01 scope.**

---

## 5. Sourcegraph compatibility delta

### 5.1 Sourcegraph features adopted as-is

- pattern leaves: bare word, `"..."`, `'...'` (raw), `/.../`
- boolean operators `AND` / `OR` / `NOT`, adjacency = AND, `-foo` = `NOT foo`, precedence `NOT > AND > OR`
- filters: `repo:` `file:` `path:` `lang:` `rev:` `type:` `select:` `count:` `case:` `fork:` `archived:` `content:` `visibility:` `patterntype:` `context:` (parse-only) `boost:` `index:` `timeout:`
- predicates: `repo:has.file` `repo:has.commit.after` `repo:has.path` `file:contains` `file:has.content`
- sugar: `repo:<pattern>@rev`
- history filters: `type:commit` `type:diff` `author:` `committer:` `message:` `before:` `after:` `since:` `until:` `diff.added:` `diff.removed:` `diff.touched:`

### 5.2 Sourcegraph features we normalize

| Input | Canonical form | Source |
|---|---|---|
| `foo bar` (adjacency) | `foo AND bar` | RFC § Normalization examples |
| `repo:<pattern>@rev` | `repo:<pattern> rev:<rev>` | RFC § Normalization examples |
| `:[X]` (Sourcegraph structural metavar) | `$X` (Semgrep-style) | RFC § Normalization examples |
| `:[...ARGS]` (variadic) | `$...ARGS` | RFC § Normalization examples |
| `-foo` (negation prefix) | `NOT foo` | RFC § Boolean semantics |
| `path:<pattern>` | `file:<pattern>` (one path-scope rep) | RFC § Filter semantics |
| `file:has.content(...)` | `file:contains(...)` | this doc — newer SG alias normalized to canonical |
| `since:<timeref>` | `since.time:` vs `since.commit:` (open question Q2) | this doc — pending decision |

### 5.3 Sourcegraph features we reject

| Feature | Reason |
|---|---|
| fuzzy / approximate / typo-tolerance default | RFC § Pattern semantics: "fuzzy or approximate search must be an explicit future operator, not default keyword behavior" |
| generic `@repo` / `@lang` / `@file` shorthand | RFC § `@` policy: only `repo:<pattern>@rev` sugar allowed |
| Sourcegraph UI alias short forms `r:` / `f:` | belong to editor/UI alias layer, not canonical grammar — RFC § `@` policy intent |
| post-filter ranking with hidden heuristics | RFC § Non-Negotiable Invariants: "no hidden filter/ranking semantics in helper string functions" |
| empty-result-on-absent-authority | repo policy: `CoreError::NotReady` instead — search-plane-implementation-tickets § Fail-closed posture |

### 5.4 Features we add beyond Sourcegraph (QI-extensions)

| Feature | Family | Reason |
|---|---|---|
| `meta.owner:` / `meta.service:` / `meta.layer:` / `meta.surface:` | Runtime-1.3 | architectural ownership metadata not present in Sourcegraph |
| `changed:` / `dirty:` / `stale:` / `affected:` / `invalidated_by:` / `snapshot:` | Runtime-1.3 | incremental / generation-addressed metadata authority surface |
| `into:codeql` directive | Bridge-1.4 | semantic bridge directive (CodeQL invocation as a planner sink) |
| `scope:results` directive | Bridge-1.4 | result-scope carrier for multi-step queries |
| `with:lexical` directive | Bridge-1.4 | candidate-not-authority discipline marker |
| typed planner errors instead of silent degradation | cross-cutting | RFC § Planner Model: "must fail with typed planner error", "must not silently degrade" |
| typed AST + parser + printer + planner SSOT | Core-1.0 | RFC § Canonical Query Model (Sourcegraph has no public typed AST contract) |

---

## 6. Authority chain per feature family

For each family, one paragraph covering: who provides truth, who validates at write, who serves at read, what failure model applies.

### 6.1 Core-1.0 (lexical content / path / symbol)

**Truth provider:** producer (`semantica-codegraph-v2`) publishes `PublishedSearchBundleManifest` containing chunk rows + symbol rows + path metadata. Search-plane is consumer.
**Write-time validation:** `quanta-index-control` validates manifest refs (`BundlePolicy::validate_artifact_ref`, T1.2) and applies deltas idempotently (`BundlePolicy::validate_delta`, T1.1). `searchd::app::materialize` (T3.5) inline reads + SHA-256 verifies bytes (D17).
**Read-time service:** `quanta-index-lexical` `TantivyLexicalAdapter` (T3.1) serves recall via the open store; `DomainQueryEngine` (T4.1) holds the open handle. Repo / lang / rev metadata is served from the control plane catalog.
**Failure model:** absent authority → `CoreError::NotReady` (no empty `Vec` fallback). Stale generation mismatch → fail closed per RFC § Generation model § rules. Per-query generation pin is captured at request start (T4.2 / D11).

### 6.2 History-1.1 (commit metadata + diff hunk content)

**Truth provider:** producer's git-walk at indexing time. Search-plane consumes already-indexed commit metadata + diff hunk artifacts. **No request-time git scan** (RFC § Canonical Incremental Write Pipeline § forbidden).
**Write-time validation:** commit metadata index is part of the manifest; same validation chain as Core-1.0 (manifest validate → SHA-256 verify → atomic build). Generation catalog (T1.2) records the commit/diff index alongside lexical.
**Read-time service:** history engine (LEX-07, post-Wave 4) holds commit metadata index reader + diff hunk index reader. Query planner routes `type:commit` / `type:diff` to this engine.
**Failure model:** unindexed history → `CoreError::NotReady`. Time-range pushed below the catalog's known revision span → typed `InvalidContract`. **No fallback to ad-hoc git log.**

### 6.3 Structural-1.2 (tree-sitter AST cache + pattern IR)

**Truth provider:** producer at indexing time emits per-file syntax cache + normalized pattern-match IR (per RFC § Recommended Concrete Engine Choices § Structural). Source of grammar correctness is the pinned tree-sitter grammar version.
**Write-time validation:** syntax cache versioning is part of structural generation (RFC § Generation model — structural cache generation). Cache invalidates on grammar version change or file content change.
**Read-time service:** structural engine (STR-01, Wave 5) walks AST + runs capture matcher. Returns binding-annotated candidates.
**Failure model:** missing syntax cache for a file → fail closed with typed `NotReady` (not silent fallback to lexical regex — RFC § Structural engine § must not). Grammar version mismatch → typed error.

### 6.4 Runtime-1.3 (runtime metadata + invalidation catalog + ownership registry)

**Truth provider:** mixed. `changed:` / `affected:` / `invalidated_by:` derive from apply-changes catalog (T1.1) + invalidation catalog (Wave 5). `dirty:` derives from producer's working-tree marker. `stale:` derives from generation catalog age vs producer's published head. `meta.*` derives from the ownership/service registry, populated by producer at indexing time.
**Write-time validation:** apply-changes catalog rows are validated by `BundlePolicy::validate_delta` (T1.1). Ownership registry rows validated by metadata-store (T3.3) when the manifest carries them.
**Read-time service:** runtime metadata engine (RT-01, Wave 5) — for Phase 1 this is the metadata store (T3.3) within `quanta-index-control` (D7 = Option A).
**Failure model:** absent metadata authority → `CoreError::NotReady` per repo policy. `dirty:` against a non-producer-marked deployment → `CoreError::NotImplemented` (no working-tree probe in this repo).

### 6.5 Bridge-1.4 (lexical candidate export + downstream invocation builder)

**Truth provider:** lexical content engine produces the candidate set; bridge engine **never** generates new truth. Candidate identity (repo, rev, generation, file, symbol) is preserved end-to-end.
**Write-time validation:** the bridge candidate packet is a contract type validated by `quanta-index-contract` policies at construction.
**Read-time service:** bridge engine (BRIDGE-01, Wave 6) builds the downstream invocation envelope. CodeQL execution itself is external — search-plane only emits the typed candidate packet + invocation spec.
**Failure model:** see §1.5.2 — overflow → `BRIDGE_CANDIDATE_OVERFLOW`, target unreachable → `BRIDGE_TARGET_UNAVAILABLE`, provenance rejected → `BRIDGE_PROVENANCE_REJECTED`. Bridge MUST NOT silently shrink the candidate set to fit downstream capacity.

---

## 7. Scale & capacity scope

Targets, not contracts. Operators tune per deployment.

**FS-GAP-3 closure (2026-05-25)** per [tickets/INDEX.md § 3.4](tickets/INDEX.md): two repo targets exist and serve different scopes — the per-process Phase-1 target and the per-cluster SLO target. Both are documented here; the RFC's 100,000-repo number is the cluster SLO ceiling, this doc's 10,000-repo number is the single-process Phase-1 ceiling.

| Scope | Target | Source |
|---|---|---|
| max repos per **single search-plane process** (Phase 1) | 10,000 | this doc — single-node catalog read fanout, no horizontal sharding |
| max repos per **cluster** (Phase 4+ horizontal scaling SLO) | 100,000 | [rfc.md § Capacity and SLO Targets](rfc.md) — cluster-level ceiling assuming N-process fanout |

The 10:1 ratio is the implied minimum horizontal-shard count once the cluster target is in scope. Phase 1 ships single-process only; the cluster target becomes claimable only when horizontal scaling lands (Phase 4+), per RFC § Claim Discipline. **No silent reconciliation**: the targets are different by design, not in conflict.

| Dimension | Initial target | Notes |
|---|---|---|
| max repos per cluster | 10,000 (process) / 100,000 (cluster) | see scope table above |
| max branches/refs per repo | 1,000 | revision catalog row count |
| max files per branch | 1,000,000 | manifest row count; per-file delta is steady-state path |
| max chunks per file | 10,000 | chunk row count per file in lexical content index |
| max boolean depth in single query | 32 | parser AST depth limit; over → typed `InvalidContract` |
| max regex states (compiled) | 100,000 | regex engine state-count cap; over → typed `InvalidContract` |
| max structural pattern depth | 16 | AST pattern depth limit |
| max candidate set size (default `count:`) | 1,000 | over → `count:all` required |
| max candidate set size (`count:all`) | 100,000 | hard ceiling; over → typed `InvalidContract` (bridge candidate overflow) |
| per-query p50 latency | < 50 ms | hot path (indexed, in-memory readers) |
| per-query p95 latency | < 250 ms | warm cache |
| per-query p99 latency | < 1000 ms | cold cache / large repo fanout |
| index build throughput (Phase 1) | ≥ 10 MB/s wall-clock | single-thread Tantivy build (D9 `std::thread::scope` parallel per-generation) |
| materialization sha256 verify | ≥ 200 MB/s | T3.5 inline read+verify |

These are aspirational targets. Wave 8 (OBS-01) owns the conformance suite that exercises the corners.

---

## 8. Feature lifecycle

States any feature family may occupy:

| State | Definition | Exit criterion → next state |
|---|---|---|
| `planned` | mentioned in RFC / this doc; no formal spec | scope catalog row exists + DSL grammar spec drafted |
| `spec'd` | this doc row + DSL grammar entry both present | failing test committed (TDD red) |
| `implemented` | ticket green; passes `just verify` + hexagonal lint | golden corpus published for the feature |
| `conformance-tested` | golden corpus passes for the feature | SLO defined + measured |
| `production-ready` | SLO measured + monitored | (terminal) |

Current state snapshot (informational; will drift — re-check before claim):

| Family | State |
|---|---|
| Core-1.0 pattern leaves | `spec'd` (this doc) — implementation = current `LqExpr::Raw/All/Any/Not` + `LqQuery` contract; Tantivy en_stem chunk index exists per T3.1 |
| Core-1.0 filters (frozen contract subset: `Repo` `File` `Path` `Lang` `Rev` `Select` `Type` `Custom`) | `implemented` for the contract subset; remaining filters (`count:` `case:` `fork:` `archived:` `content:` `visibility:` `patterntype:` `context:` `boost:` `index:` `timeout:`) are `planned` |
| Core-1.0 predicates | `planned` |
| History-1.1 | `planned` |
| Structural-1.2 | `planned` |
| Runtime-1.3 | `planned` |
| Bridge-1.4 | `planned` |

Per RFC § Claim Discipline, claims at each tier require:
- `Sourcegraph-compatible lexical core` → parser conformance + front-door parity + global unanchored execution proof
- `incremental indexing` → file-delta write/read generation proof
- `history search` → indexed commit/diff execution exists
- `structural search` → tree-sitter-backed matcher exists
- `runtime-aware filters` → metadata catalog truth exists
- `CodeQL bridge` → typed candidate export exists
- `semantic rebased on lexical` → lexical-universe planning replaces post-filter correctness

---

## 9. Open questions

These are scope ambiguities this document surfaces but cannot decide alone. Each blocks at least one feature row above.

- **Q1.** Predicate evaluation timing: is `repo:has.commit.after(<timeframe>)` evaluated at parse time (against catalog, statically resolving to a repo set) or at query time (planner-pushed into history engine)? Same for `repo:has.path` / `file:contains` / `file:has.content`. Affects §1.1.5, §4.1 (predicate rows), §6.2 (history authority chain). RFC § Planner Model lists routing rules but does not separate parse-time-resolved scope from query-time-pushed scope.
- **Q2.** `since:` disambiguation: does `since:<token>` lower at parse time into one of `since.time:` / `since.commit:` based on token shape, or does the executor branch? Affects §1.2.4. Sourcegraph treats `since:` as time-only; we want both.
- **Q3.** `visibility:` truth source: where does repo visibility come from? Producer-published metadata field, or a side-channel registry? No producer-side coordination is in scope per SSOT § Out of Scope. Affects §1.1.4, §4.1 (`visibility:` row), §4.7 (flagged gap).
- **Q4.** Structural matching language-awareness: does the structural pattern IR hold per-grammar variants (one IR per tree-sitter grammar) or one unified IR with a per-grammar lowering adapter? Affects §1.3.4, §6.3. RFC § Structural engine § recommended owner says "language-normalized pattern IR" but does not resolve.
- **Q5.** `dirty:` semantics in a read-only consumer: is `dirty:` a valid search-plane filter at all? Truth source is producer's working tree, which is out of scope for this repo per SSOT § Out of Scope. Either (a) deprecate `dirty:` from Runtime-1.3 entirely, (b) define it as "producer-marked-dirty" with a producer-side cross-repo dependency, or (c) move it to deferred-with-reason §3. Affects §1.4.1, §4.4 (`dirty:` row), §6.4.
- **Q6.** Sourcegraph `select:<projection>` projection set: which projections do we support (`select:repo` `select:file` `select:symbol` `select:content` `select:commit.diff.added` ...)? RFC § Design Baseline lists `select:` but not the projection enum. Affects §1.1.3 (`select:` row), §4.1.
- **Q7.** `count:all` ceiling vs §7 `100,000` hard limit: should `count:all` honor the §7 ceiling silently or fail closed when results exceed? Per repo fail-closed posture, the latter — but Sourcegraph compat may force a soft-cap-with-warning. Affects §1.1.3, §7. Default answer: **fail closed with typed `InvalidContract`** (no silent truncation), matching repo invariants.
- **Q8.** Bridge candidate identity stability across generations: if `into:codeql` is queued and a new generation activates mid-flight, does the candidate packet carry the original generation id (Phase 1 per-query pin, T4.2 / D11) or fall back to the new one? Default answer: **packet carries the pinned generation; downstream invocation MUST receive the pinned generation; no auto-rebase.** Affects §1.5, §6.5.
- **Q9.** Sub-language structural support order: STR-01 lists 5 ship grammars (Rust/Python/TS/JS/Go) — is there a priority order, or are they all gated together? Affects §1.3.4. Default answer: gate together; partial language coverage is `NotImplemented` per language, not a partial success path.
- **Q10.** `index:no` mode (force unindexed scan): RFC implies index-only Phase 1. Is `index:no` parse-rejected or accepted-then-`NotImplemented`? Affects §1.1.4. Default answer: accepted, executor returns typed `NotImplemented` (namespace reserved).

---

## 10. References

- [rfc.md](rfc.md) — May-23 Sourcegraph-Class Lexical Kernel RFC (parent)
- [search-plane-implementation-tickets.md](../search-plane-implementation-tickets.md) — current implementation tickets (E1–E5, T1.*–T5.*)
- [may-23-storage-architecture-endgame-implementation.md](../../ssot/may-23-storage-architecture-endgame-implementation.md) — SSOT for the search-plane
- [CLAUDE.md](../../../CLAUDE.md) — agent change posture, rule catalog
- [AGENT_RULE_CATALOG.md](../../../AGENT_RULE_CATALOG.md) — rule catalog (shared)
