# LQ Family DSL — Formal Grammar, Expression, Normalization

Status: `Planning packet — DSL freeze candidate`
Parent RFC: [rfc.md](rfc.md)
Sibling planning docs (forward references, authored in parallel): `feature-scope.md`, `usecase.md`
Owning grammar version: `LQ/Core-1.0` with extensions `LQ/History-1.1`, `LQ/Structural-1.2`, `LQ/Runtime-1.3`, `LQ/Bridge-1.4`.

This document closes the reviewer P0 gaps left open by the parent RFC sections
*Canonical Grammar Semantics*, *Pattern semantics*, *Boolean semantics*,
*Filter semantics*, and *LQ Family*:

1. default `patterntype:` is pinned (Section 4)
2. regex dialect is pinned to RE2 (Section 3)
3. filter combination semantics is pinned (Section 6)
4. predicate filter sub-grammar is formalized (Section 7)
5. structural pattern language is formalized as its own mini-language (Section 8)

The doc is the authoritative grammar contract. Anything in the RFC that is
softer than this doc is superseded here. Anything not specified here is
**not** part of `LQ/Core-1.0` and must be rejected at parse time —
no silent acceptance, per RFC *Non-Negotiable Invariants*.

---

## 1. Lexical structure

### 1.1 Character set

- input is UTF-8, mandatory
- non-UTF-8 input is rejected with `PARSE_INVALID_UTF8` and a byte offset
- BOM (`U+FEFF`) at offset 0 is tolerated and stripped before tokenization
- any other occurrence of `U+FEFF` is preserved as raw codepoint inside
  quoted/raw/regex leaves only; bare-position `U+FEFF` is `PARSE_LEX_ERROR`

### 1.2 Whitespace

- whitespace tokens: `U+0020` SPACE, `U+0009` TAB, `U+000A` LF, `U+000D` CR
- runs of whitespace collapse to a single token boundary
- whitespace is significant only as a token separator; it never appears
  inside a canonical AST node
- inside `"…"`, `'…'`, `/…/`, and `match { … }` bodies whitespace is preserved literally

### 1.3 Comments

- there are **no** comments in `LQ/Core-1.0` query text
- this matches Sourcegraph; query strings are short-lived and not source code
- any `//`, `/* */`, `#` is parsed as ordinary input (typically as a keyword
  or as part of a regex/phrase). It is not stripped.

### 1.4 Identifier characters

An *identifier* (used as a filter name, predicate name, directive name,
language id, type tag) matches:

```
identifier        = ident-start { ident-continue }
ident-start       = ASCII-letter | "_"
ident-continue    = ASCII-letter | ASCII-digit | "_" | "." | "-"
ASCII-letter      = "A" … "Z" | "a" … "z"
ASCII-digit       = "0" … "9"
```

Filter names are case-insensitive on lookup, lower-cased in canonical form.
Predicate names (`has.file`, `has.commit.after`) carry interior `.` —
the dot is **not** a path separator in the AST; it is part of the
identifier value and is preserved verbatim.

### 1.5 Quoted strings

Three quote forms with distinct semantics. All are pattern leaves.

| Form     | Name         | Escapes recognized               | Semantics                          |
|----------|--------------|----------------------------------|------------------------------------|
| `"…"`    | Phrase       | `\\`, `\"`, `\n`, `\r`, `\t`     | tokenized exact phrase             |
| `'…'`    | RawString    | `\\`, `\'` only                  | literal substring, no tokenization |
| `/…/`    | Regex        | `\\`, `\/` only                  | RE2 regex (see §3)                 |

Rules:

- unterminated quote -> `PARSE_LEX_ERROR{expected=close-quote}`
- `\u{XXXX}` Unicode escapes are **not** recognized — write the codepoint directly
- inside `/…/`, `\/` is the only escape that affects delimiter parsing;
  all other backslash sequences are forwarded to the RE2 compiler
- inside `'…'`, no other escape exists; `\n` inside a RawString is the
  two-character sequence `\`, `n`

### 1.6 Maximum query length

- hard cap: **16 KiB** (`16384` bytes after UTF-8 validation,
  before tokenization)
- exceeds cap -> `PARSE_OVERSIZED` with actual byte size
- this cap matches the IPC frame budget headroom; it is a parser-layer
  invariant, not a tunable per-request value

### 1.7 Token kinds (after lex)

```
TOKEN
  = KEYWORD        // bare word
  | PHRASE         // "…"
  | RAWSTRING      // '…'
  | REGEX          // /…/
  | LPAREN         // (
  | RPAREN         // )
  | LBRACE         // {
  | RBRACE         // }
  | COLON          // :
  | AT             // @  -- only legal in repo:<pat>@rev sugar
  | DASH           // -  -- only legal as prefix negation
  | KW_AND         // AND
  | KW_OR          // OR
  | KW_NOT         // NOT
  | FILTER_NAME    // identifier immediately followed by COLON
  | EOF
```

Token kinds `KW_AND`, `KW_OR`, `KW_NOT` match only the uppercase forms
`AND`, `OR`, `NOT` exactly. Lowercase `and` / `or` / `not` is a KEYWORD.
This matches Sourcegraph and is required for backwards compatibility.

---

## 2. Formal grammar (EBNF)

### 2.1 `LQ/Core-1.0`

```
(* top-level *)
Query           = [ Expression ] { Filter } { Directive } ;

(* boolean layer *)
Expression      = OrExpression ;
OrExpression    = AndExpression { "OR" AndExpression } ;
AndExpression   = NotExpression { ( "AND" | implicit-and ) NotExpression } ;
NotExpression   = ( "NOT" Atom ) | ( "-" Atom ) | Atom ;
Atom            = PatternLeaf | "(" Expression ")" ;

(* implicit-and exists only in patterntype modes that pin it;
   see §4 mode matrix. In `keyword` mode, adjacency is a ranking
   signal, not strict AND, and `implicit-and` is replaced by
   `adjacency-link`. *)

(* pattern leaves *)
PatternLeaf     = Keyword | Phrase | RawString | Regex ;
Keyword         = keyword-char { keyword-char } ;
keyword-char    = ? any non-whitespace,
                    not in { "(", ")", "{", "}", ":", "/", "\"", "'" },
                    not starting with "-" at expression position ? ;
Phrase          = '"' { phrase-char } '"' ;
RawString       = "'" { raw-char } "'" ;
Regex           = "/" { regex-char } "/" ;

(* filters *)
Filter          = FilterName ":" FilterValue ;
FilterName      = identifier ;
FilterValue     = ( PredicateCall | bare-value | Phrase | RawString | Regex ) ;
bare-value      = filter-value-char { filter-value-char } ;
filter-value-char
                = ? any non-whitespace,
                    not in { "(", ")", "{", "}" } ? ;

(* predicate sub-grammar — §7 *)
PredicateCall   = "has" "." predicate-tail "(" [ ArgList ] ")" ;
predicate-tail  = identifier { "." identifier } ;
ArgList         = Arg { "," Arg } ;
Arg             = ( ArgKey "=" ArgValue ) | ArgValue ;
ArgKey          = identifier ;
ArgValue        = Phrase | RawString | Regex | bare-value ;

(* directives — §9 *)
Directive       = DirectiveName ":" DirectiveValue ;
DirectiveName   = "into" | "scope" | "with" ;
DirectiveValue  = identifier ;
```

### 2.2 `LQ/History-1.1` extensions

```
(* Adds these filter names with constrained value grammars.
   Filters slot into the Filter production from §2.1.            *)

HistoryFilter   = "author"    ":" FilterValue
                | "committer" ":" FilterValue
                | "message"   ":" FilterValue
                | "before"    ":" DateOrDuration
                | "after"     ":" DateOrDuration
                | "since"     ":" DateOrDuration
                | "until"     ":" DateOrDuration
                | "diff.added"    ":" FilterValue
                | "diff.removed"  ":" FilterValue
                | "diff.touched"  ":" FilterValue ;

DateOrDuration  = RFC3339-date | duration ;
duration        = digit { digit } duration-unit ;
duration-unit   = "s" | "m" | "h" | "d" | "w" | "mo" | "y" ;

(* type:commit and type:diff are accepted in LQ/Core-1.0 grammar already
   — they only acquire planner authority when LQ/History-1.1 is active. *)
```

### 2.3 `LQ/Structural-1.2` extensions

```
(* match { … } is a Pattern leaf alternative *)
PatternLeaf    += StructuralBlock ;
StructuralBlock = "match" "{" StructuralBody "}" ;
StructuralBody = StructuralExpr { StructuralExpr } ;

StructuralExpr = StructuralPattern
               | "where"   StructuralConstraint
               | "inside"  "{" StructuralBody "}"
               | "outside" "{" StructuralBody "}" ;

StructuralPattern
               = { structural-token } ;

structural-token
               = Hole | NamedHole | AnonHole | structural-literal ;

NamedHole      = "$" identifier ;                  (* single-token   *)
MultiHole      = "$" "..." identifier ;            (* multi-token    *)
AnonHole       = "..." ;                           (* anonymous wild *)
TypedHole      = ":[" "hole.type" "=" type-name "]" ;
SgAliasHole    = ":[" identifier "]"               (* desugared to NamedHole *)
               | ":[" "..." identifier "]" ;       (* desugared to MultiHole *)

StructuralConstraint
               = HoleRef "==" HoleRef
               | HoleRef "==" Phrase
               | HoleRef "==" RawString ;
HoleRef        = "$" [ "..." ] identifier ;

type-name      = identifier ;
structural-literal
               = ? any source codepoint not introducing a hole token ? ;
```

### 2.4 `LQ/Runtime-1.3` extensions

```
RuntimeFilter   = "changed"        ":" RevSpec
                | "dirty"          ":" yes-no
                | "stale"          ":" yes-no
                | "affected"       ":" FilterValue
                | "invalidated_by" ":" FilterValue
                | "snapshot"       ":" identifier
                | "meta" "." identifier ":" FilterValue ;

yes-no          = "yes" | "no" | "only" ;
```

### 2.5 `LQ/Bridge-1.4` extensions

```
(* directives only — no new pattern leaves *)
BridgeDirective = "into"  ":" ( "codeql" )
                | "scope" ":" ( "results" )
                | "with"  ":" ( "lexical" ) ;
```

`LQ/Bridge-1.4` reserves `into`, `scope`, `with` as directive names.
They are forbidden as filter names in `LQ/Core-1.0`.

---

## 3. Pattern leaf semantics

### 3.1 `Keyword` (bare word)

- canonical form: the verbatim UTF-8 byte sequence
- **no automatic camelCase / snake_case split** at the grammar layer;
  any token splitting is the indexing layer's tokenizer concern, not the
  parser's. Two queries `getUserName` and `get user name` must produce
  distinct ASTs.
- case sensitivity defaults: see §4 mode matrix
- mapping: keyword leaf maps to one of
  - Tantivy `TermQuery` against the configured analyzer's term stream
    for `text` / `path` / `symbol` field (planner picks field set from
    active `type:` filter), or
  - Tantivy `BooleanQuery` over a tokenizer-expanded term set when the
    analyzer emits more than one term for the leaf
- the keyword leaf must **not** be silently widened to fuzzy matching;
  the RFC invariant *no fuzzy-by-default keyword semantics* is binding

### 3.2 `Phrase ("…")`

- canonical form: decoded codepoint string (escape sequences resolved)
- semantics: exact contiguous span in the tokenized term stream
- mapping: Tantivy `PhraseQuery` with slop = 0
- empty phrase `""` -> `PARSE_INVALID_FILTER_VALUE` at the relevant
  Atom position
- escapes: `\\`, `\"`, `\n`, `\r`, `\t`; any other `\X` is `PARSE_LEX_ERROR`

### 3.3 `RawString ('…')`

- canonical form: decoded codepoint string with `\\` -> `\` and `\'` -> `'` only
- semantics: literal substring; tokenization is **not** applied
- mapping: requires a trigram or N-gram index for non-degenerate matching.
  If the active backend does not expose such an index, planning fails
  with `PLAN_LIMIT_EXCEEDED{dimension=rawstring-backend, limit=0}` —
  not silently degraded to phrase. RawString is a load-bearing distinct
  pattern type, not a phrase synonym.
- in the current substrate (Tantivy chunk index without ngram), RawString
  on the bare expression position is conditionally accepted; see
  Section 13 for the cap.

### 3.4 `Regex (/…/)`

- **dialect: RE2** (Rust `regex_syntax` default flavor).
- enabled features:
  - line anchors `^`, `$` (see anchor rules below)
  - character classes `\d`, `\w`, `\s`, `[…]` including ranges and negation
  - alternation `|`
  - non-capturing groups `(?:…)`
  - capturing groups `(…)` (capture indexes are reachable to the
    structural matcher only; lexical regex ignores captures)
  - quantifiers `*`, `+`, `?`, `{n}`, `{n,}`, `{n,m}` — greedy and reluctant
  - Unicode property classes `\p{L}` etc.
- forbidden features (parser rejects with `PARSE_FORBIDDEN_SYNTAX`):
  - backreferences `\1` … `\9`
  - lookahead `(?=…)`, `(?!…)`
  - lookbehind `(?<=…)`, `(?<!…)`
  - possessive groups `(?>…)`
  - named-capture references `\k<name>` (named *captures* `(?P<name>…)`
    are also rejected — keep the surface single-shape)
  - inline flag groups that would change ASCII/Unicode mode mid-pattern
    `(?u-i)` etc. The only inline flag accepted is leading `(?i)`,
    which is normalized into the `case:no` option (see §10).
- anchor rules:
  - `^` matches start-of-line; `$` matches end-of-line
  - start-of-file / end-of-file semantics are reached only via the
    explicit `\A` and `\z` escapes (RE2 supports these)
  - this matches Sourcegraph default regex behavior
- mapping: Tantivy `RegexQuery` over the configured field set
  (`text` and `path` by default; planner narrows per `type:`)
- compile-time cost cap: **NFA state count <= 100_000**.
  Computed via `regex_syntax::hir::analysis::Properties` upper bounds
  before compilation; exceeded -> `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa,
  limit=100000}`. The cap is configurable per deployment but the parser
  refuses to drop below `1_000` (would make the language useless).

---

## 4. `patterntype:` mode matrix

This is the P0 fix.

| Mode          | Pattern leaf default | Adjacency semantics                              | Default case | Regex literal allowed?                  |
|---------------|----------------------|--------------------------------------------------|--------------|-----------------------------------------|
| `literal`     | RawString            | strict AND with phrase semantics                 | sensitive    | only via `/…/` operator                 |
| `keyword`     | Keyword              | adjacency = ranking boost only, **not** strict AND | insensitive  | via `/…/` operator                      |
| `standard`    | Keyword              | adjacency = strict AND                           | insensitive  | via `/…/` operator                      |
| `regexp`      | Regex                | adjacency = AND of regex sub-queries             | sensitive    | bare `/…/` allowed at top level         |
| `structural`  | StructuralBlock      | n/a — single match block per query               | n/a          | n/a                                     |

### 4.1 Default mode for `LQ/Core-1.0`

**Pinned default: `standard`.**

Rationale:

1. Sourcegraph's own default since v5.0 is `standard`. The RFC's
   compatibility goal `Sourcegraph query ⊂ LQ/Core-1.0` is satisfied
   only if defaults align.
2. `standard` keeps adjacency = strict AND, which is the behavior the
   RFC's *Boolean semantics* section pins (`adjacency means AND`).
   `keyword` would contradict that section.
3. `standard` keeps regex an explicit operator, preventing accidental
   regex compilation on bare user input.

Cross-reference: RFC §*Canonical Grammar Semantics → Boolean semantics*.

### 4.2 Mode interaction with operators

- `AND`, `OR`, `NOT`, `-` keep meaning across all modes
- `()` grouping keeps meaning across all modes
- in `keyword` mode, the rewrite `foo bar` -> `foo AND bar` does
  **not** happen; instead the AST records `AdjacencyLink(foo, bar)`
  which the ranker consumes
- in `regexp` mode, bare tokens `foo bar` parse as `Regex(foo)` and
  `Regex(bar)` (each compiled separately) joined by adjacency AND
- in `structural` mode, only a single top-level `match { … }` block
  is permitted as the Expression; other patterns are
  `PARSE_UNSUPPORTED_COMBO{offending-pair="structural, non-match-leaf"}`

### 4.3 Switching mode mid-query

- `patterntype:` is a filter, so it can in principle appear anywhere
- canonical rule: only **one** `patterntype:` is accepted per query;
  duplicates -> `PARSE_INVALID_FILTER_VALUE{filter=patterntype}`
- effective mode is fixed for the whole AST; there is no scoped mode

---

## 5. Boolean evaluation rules

### 5.1 Precedence

`NOT > AND > OR`. Same as Sourcegraph.

Right-most position of `NOT` binds tightest. `()` overrides precedence.

### 5.2 Associativity

Left-to-right. `a OR b OR c` parses as `(a OR b) OR c`.
Canonical AST collapses chains of the same operator into n-ary nodes
during normalization (§10), so the runtime semantics are associative.

### 5.3 Adjacency

In `standard` and `regexp` modes, two adjacent atoms with no explicit
operator are joined by implicit AND.

In `keyword` mode, adjacency does **not** produce an AND node; it
produces an `AdjacencyLink` annotation on the parent AND node.
The ranker promotes documents where the same tokens appear within a
configurable window (BM25 + proximity boost). Default window: `8 tokens`.
Window override: not exposed in `LQ/Core-1.0`; deferred.

In `literal` mode, adjacency joins atoms into a single phrase-like AND;
this matches Sourcegraph literal mode.

### 5.4 Negation

`NOT foo` and `-foo` are equivalent post-normalization. Canonical AST
stores both as `Not(…)`. `-` is only legal at expression position
(start of an Atom slot) and only when not immediately followed by another
operator token.

### 5.5 Empty and dangling forms

- `()` -> `PARSE_SYNTAX_ERROR{expected=Expression}`
- trailing `AND`, `OR`, `NOT` -> `PARSE_SYNTAX_ERROR`
- empty query string (zero non-whitespace tokens) is a legal but
  inert AST: `LqQuery { expr=Empty, filters=[], … }`. The planner
  rejects it with a planner-layer typed error, not a parser error.
  This split keeps the parser idempotent (see §10).

---

## 6. Filter semantics

### 6.1 General rules

- filter names are case-insensitive on lookup, lower-cased in canonical AST
- multiplicity:
  - **AND between different filter kinds**: `repo:a file:b` =>
    `repo:a AND file:b`
  - **OR within the same kind**: `repo:a repo:b` => `repo:(a OR b)`
  - `-repo:a` is `NOT repo:a` and continues to AND with siblings
- value pattern type per filter is pinned below; values that fail the
  pinned grammar -> `PARSE_INVALID_FILTER_VALUE`
- unknown filter name -> `PARSE_UNKNOWN_FILTER`

### 6.2 Filter table

| Filter        | Value pattern type     | Canonical form / desugar                                 | Notes                                                            |
|---------------|------------------------|----------------------------------------------------------|------------------------------------------------------------------|
| `repo:`       | regex (RE2)            | `RepoFilter{ pattern: Regex, revs: [] }`                 | `repo:foo@rev` desugars to `RepoFilter{pattern=foo, revs=[rev]}` |
| `file:`       | regex (RE2)            | `FileFilter{ pattern: Regex, scope: NameAndPath }`       | matches both file name and full path (Sourcegraph spec)          |
| `path:`       | regex (RE2)            | `FileFilter{ pattern: Regex, scope: PathOnly }`          | canonical alias surface; AST keeps one type, scope distinguishes |
| `lang:`       | enum identifier        | `LangFilter{ id: LangId }`                               | enumerated set, see §6.3                                         |
| `rev:`        | rev-spec               | `RevFilter{ spec: RevSpec }`                             | branch / tag / sha / range                                       |
| `type:`       | enum                   | `TypeFilter{ kind }`                                     | exclusive; see §6.4                                              |
| `select:`     | enum                   | `SelectFilter{ dim }`                                    | see §6.5                                                         |
| `count:`      | integer or `all`       | `CountOption{ bound: Bounded(N) | All }`                 | cap on `Bounded`: 10_000                                         |
| `case:`       | `yes` / `no`           | `CaseOption{ sensitive: bool }`                          | default per patterntype                                          |
| `fork:`       | `yes` / `no` / `only`  | `ForkFilter{ mode }`                                     |                                                                  |
| `archived:`   | `yes` / `no` / `only`  | `ArchivedFilter{ mode }`                                 |                                                                  |
| `visibility:` | `public`/`private`/`any` | `VisibilityFilter{ mode }`                             |                                                                  |
| `context:`    | identifier             | `ContextFilter{ name }`                                  | deferred — parser accepts, planner emits `PLAN_UNKNOWN_PREDICATE`-class typed defer error |
| `boost:`      | float                  | `BoostOption{ factor: f32 }`                             | deferred — parser accepts, planner refuses                       |
| `index:`      | `yes` / `no` / `only`  | `IndexOption{ mode }`                                    | this stack is always indexed; `no` -> typed planner error        |
| `timeout:`    | duration               | `TimeoutOption{ budget: Duration }`                      | overrides default per-query CPU budget                           |
| `content:`    | follows current patterntype | `ContentFilter{ leaf }`                              | identical semantics to bare pattern at top level                 |
| `patterntype:`| enum                   | `PatternTypeOption{ mode }`                              | see §4                                                           |

### 6.3 Canonical `lang:` identifiers

Enumerated. Unknown -> `PARSE_INVALID_FILTER_VALUE{filter=lang}`.

```
rust, python, typescript, javascript, tsx, jsx, go, java, kotlin,
scala, swift, cpp, c, csharp, objective-c, ruby, php, perl, lua,
elixir, erlang, haskell, ocaml, fsharp, clojure, scheme, racket,
r, julia, dart, zig, nim, crystal, d, fortran, ada, cobol, vb,
shell, bash, zsh, fish, powershell, sql, html, css, scss, sass,
less, vue, svelte, markdown, yaml, toml, json, json5, ini, xml,
dockerfile, makefile, cmake, terraform, hcl, protobuf, thrift,
graphql, solidity, vyper, move, cairo
```

Alias resolution before canonicalization:

```
js   -> javascript
ts   -> typescript
py   -> python
rb   -> ruby
cs   -> csharp
md   -> markdown
yml  -> yaml
```

This list is normative for `LQ/Core-1.0`. Additions require a documented
DSL minor version bump.

### 6.4 `type:` values

`type:` is mutually exclusive across these values:

```
file | path | symbol | commit | diff | repo
```

`type:file` is the implicit default. Two `type:` filters in one query
-> `PARSE_INVALID_FILTER_VALUE{filter=type}`. `type:commit` and
`type:diff` activate the `LQ/History-1.1` planner; using them without
that family loaded -> `PARSE_UNSUPPORTED_COMBO`.

### 6.5 `select:` values

```
repo | file | path | symbol | content | content.match
```

`select:` chooses the projection dimension of the result stream.
Multiple `select:` -> `PARSE_INVALID_FILTER_VALUE{filter=select}`.

### 6.6 `rev:` grammar

```
RevSpec    = single-rev | range-rev ;
single-rev = identifier | sha-hex ;
sha-hex    = ? 7-64 lowercase hex characters ? ;
range-rev  = single-rev ".." single-rev          (* two-dot range *)
           | single-rev "..." single-rev ;       (* three-dot range *)
```

- `repo:foo@bar` desugars to `repo:foo rev:bar` (Sourcegraph sugar)
- multiple `rev:` for the same repo combine OR-within-kind on the
  `revs` array of the binding `repo:` filter; if no `repo:` filter is
  present in the query, the `rev:` applies globally

### 6.7 Pattern kind per filter (glob vs regex vs exact)

This was an implicit-default hazard in the RFC. Pinned here:

- `repo:` and `file:` and `path:` values are **regex**, RE2 dialect.
  Anchoring is **not** implied; `repo:core` matches any repo whose
  path contains `core`. Anchor explicitly with `^core$`.
- `lang:`, `type:`, `select:`, `fork:`, `archived:`, `visibility:`,
  `case:`, `index:` are **enum** with exact match
- `count:`, `timeout:`, `boost:` are **scalar** typed
- `rev:` is rev-spec grammar (§6.6)
- predicate args carry their own value grammar (§7)

No filter value is interpreted as a glob in `LQ/Core-1.0`. Glob would
be a separate filter kind and is not introduced here.

---

## 7. Predicate filter sub-grammar

### 7.1 General shape

```
<filter-name> ":" "has" "." <method-tail> "(" [ arg-list ] ")"
```

The `has.` prefix is mandatory. There are no non-`has.` predicates in
`LQ/Core-1.0`.

### 7.2 Argument grammar

```
arg-list   = arg { "," arg } ;
arg        = ( arg-key "=" arg-value ) | arg-value ;
arg-key    = identifier ;
arg-value  = Phrase | RawString | Regex | bare-value ;
```

Type coercion table (applied during normalization):

| Declared arg type | Accepted leaf forms                | Canonical form     |
|-------------------|------------------------------------|--------------------|
| `regex`           | `Regex`, `bare-value`              | `Regex`            |
| `string`          | `Phrase`, `RawString`, `bare-value`| `Phrase`           |
| `path-pattern`    | `Regex`, `bare-value`              | `Regex` over path  |
| `duration`        | `bare-value` matching duration     | `Duration`         |
| `date`            | `bare-value` matching RFC3339      | `Date`             |
| `bool`            | `yes` / `no`                       | `bool`             |
| `identifier`      | `bare-value` matching identifier   | `Identifier`       |

Mismatch -> `PARSE_INVALID_FILTER_VALUE{filter=<name>, arg=<key>}`.

### 7.3 Supported predicates

Per-filter predicate registry. The planner is allowed to reject any of
these with `PLAN_UNKNOWN_PREDICATE` if the active engine doesn't
provide the implementation, but the **parser** must accept the grammar.

`repo:` predicates:

| Predicate                  | Args                                      | Arg types                    |
|----------------------------|-------------------------------------------|------------------------------|
| `repo:has.file(...)`       | `path` (positional or keyword)            | `path-pattern`               |
| `repo:has.path(...)`       | positional                                | `path-pattern`               |
| `repo:has.content(...)`    | positional                                | `regex`                      |
| `repo:has.commit.after(d)` | positional `d`                            | `date` or `duration`         |
| `repo:has.commit.message(p)` | positional `p`                          | `regex`                      |
| `repo:has.tag(t)`          | positional `t`                            | `string`                     |
| `repo:has.description(p)`  | positional `p`                            | `regex`                      |
| `repo:has.topic(t)`        | positional `t`                            | `string`                     |

`file:` predicates:

| Predicate                   | Args                | Arg types     |
|-----------------------------|---------------------|---------------|
| `file:contains.content(p)`  | positional `p`      | `regex`       |
| `file:contains(p)`          | positional `p`      | `regex`       |
| `file:has.content(p)`       | positional `p`      | `regex`       |
| `file:has.owner(o)`         | positional `o`      | `string`      |
| `file:has.contributor(c)`   | positional `c`      | `string`      |

### 7.4 Evaluation model

Pin:

- **index-time evaluation** when an index exists that can answer the
  predicate (the typical case for `repo:has.file`, `repo:has.tag`,
  `file:contains.content`)
- **query-time evaluation** otherwise (e.g. `repo:has.commit.message`
  when no commit-message index is built yet)
- pure parse-time evaluation does **not** exist; predicates are always
  semantic, never constant-folded at parse time

Predicate negation: `-repo:has.file(path:foo.go)` is `NOT repo:has.file(...)`.
The negation flips at the *predicate* node, not at individual args.

Composition: predicates AND with sibling filters of any kind and with
the pattern expression. Within the same `repo:` filter, multiple
predicates are AND'd (matching Sourcegraph). To OR predicates, write
`(repo:has.file(a) OR repo:has.file(b))` — note that this requires the
predicate to be its own Atom; the grammar in §2.1 lifts `FilterValue`
to `PredicateCall` only inside `Filter`, not inside `Atom`. Composition
inside `Atom` is therefore done by repeating the whole `filter:` token.

---

## 8. Structural sub-grammar (`LQ/Structural-1.2`)

### 8.1 Holes

```
Hole        = "$" identifier              (* single-token *)
            | "$" "..." identifier        (* multi-token  *)
            | "..."                       (* anonymous wildcard *)
```

- single-token hole binds one syntactic token (identifier, literal,
  punctuation atom — tree-sitter granularity)
- multi-token hole binds an n-ary span; greedy by default, shortest at
  language-rule boundaries (function body, block, expression list)
- anonymous wildcard binds zero-or-more tokens; never captures; cannot
  be referenced by `where`

### 8.2 Typed holes

```
TypedHole   = ":[" "hole.type" "=" type-name "]"
type-name   ∈ { "expr", "stmt", "decl", "ident", "literal",
                "type-ref", "block", "param", "param-list",
                "arg", "arg-list", "string-literal", "number-literal",
                "comment", "attr", "macro-call", "lambda" }
```

Unknown `type-name` -> `PARSE_INVALID_FILTER_VALUE{filter=hole.type}`.
Additional types require a documented DSL minor version bump.

A typed hole binds the named single-token slot but with the constraint
that the matched tree-sitter node has the specified semantic role.

### 8.3 Equality constraints

```
where-clause     = "where" constraint-conj ;
constraint-conj  = constraint { "AND" constraint } ;
constraint       = HoleRef "==" HoleRef
                 | HoleRef "==" Phrase
                 | HoleRef "==" RawString ;
```

- `where $X == $Y` requires both holes to be bound and to capture
  byte-identical spans
- `where $X == "foo"` requires `$X` to bind a token whose text equals
  `foo`
- `OR` and `NOT` inside `where` are not in `LQ/Structural-1.2`;
  deferred

### 8.4 Context operators

```
context-op      = "inside"  "{" StructuralBody "}"
                | "outside" "{" StructuralBody "}" ;
```

- `inside { P }` restricts matches to those whose tree-sitter parent
  chain contains a match for body `P`
- `outside { P }` is the negation: match only when the parent chain
  contains **no** match for `P`
- context bodies are themselves full structural patterns and may
  contain holes, `where`, and nested `inside`/`outside`

### 8.5 Language anchoring

Structural patterns are tree-sitter-based and therefore language-aware.
Language resolution order:

1. explicit `lang:<id>` filter in the same query -> pinned
2. else, the union of `lang:<id>` implied by `file:` regex matches
   intersected with the project's language registry
3. else, the structural plan emits one sub-plan per supported
   language and the matcher dispatches per matched file

The structural matcher **never** guesses language from pattern text.
If language resolution returns the empty set, the planner emits
`PLAN_UNKNOWN_PREDICATE{predicate=structural, dimension=language}`.

### 8.6 Sourcegraph alias normalization

Done at the lexer stage so the AST never carries the alias form:

```
:[X]        -> $X
:[...ARGS]  -> $...ARGS
```

The `:[…]` form is **only** valid inside a `match { … }` block; outside
it -> `PARSE_FORBIDDEN_SYNTAX`.

---

## 9. Directive grammar (`LQ/Bridge-1.4`)

### 9.1 Reserved directive names

```
into  : sink-name
scope : carrier-name
with  : modifier-name
```

### 9.2 Accepted values

| Directive | Accepted values | Default if omitted |
|-----------|-----------------|--------------------|
| `into:`   | `codeql`         | none — directive optional |
| `scope:`  | `results`        | `results` |
| `with:`   | `lexical`        | `lexical` |

Other values reserved; unknown value -> `PARSE_INVALID_FILTER_VALUE`.

### 9.3 Composition

- a directive that doesn't appear in the parser whitelist -> `PARSE_UNKNOWN_FILTER`
- directives compose with filters; ordering between filter and directive
  in source text is not significant
- directives apply **after** filtering, **before** merge — they
  transform the lexical candidate set into an alternate output sink
- a query with `into:codeql` but no candidate-producing pattern
  expression -> `PARSE_UNSUPPORTED_COMBO{offending-pair="into:codeql, empty-expression"}`
- `into:codeql` combined with `type:repo` or `type:diff` ->
  `PARSE_UNSUPPORTED_COMBO`; the bridge consumes file/symbol candidates only

---

## 10. Normalization rules

Canonical normalization is one pass with sub-steps in fixed order.

```
raw bytes
  └─> 1. UTF-8 validate                      [PARSE_INVALID_UTF8]
  └─> 2. size check                          [PARSE_OVERSIZED]
  └─> 3. BOM strip
  └─> 4. tokenize                            [PARSE_LEX_ERROR]
  └─> 5. parse to AST                        [PARSE_SYNTAX_ERROR]
  └─> 6. desugar
        - repo:foo@bar       -> repo:foo rev:bar
        - :[X]               -> $X
        - :[...ARGS]         -> $...ARGS
        - (?i)pat            -> pat with case:no recorded on options
        - lang aliases       -> canonical lang id (§6.3)
  └─> 7. alias resolve
        - path: vs file:     -> single FileFilter with scope tag
        - patterntype default-> Mode::Standard if absent
  └─> 8. constant fold
        - NOT NOT X          -> X
        - (X AND Y) AND Z    -> AND(X, Y, Z)               (n-ary collapse)
        - (X OR Y) OR Z      -> OR(X, Y, Z)
        - AND() / OR() empty -> identity element removed
        - duplicate filters  -> deduped (OR-within-kind already merged)
  └─> 9. emit canonical AST  (LqQueryV1, see RFC §Canonical Query Model)
```

### 10.1 Idempotency invariant

Let `normalize(q)` denote the canonical AST and `print(a)` denote the
canonical printer (deterministic ASCII reproduction of the AST). Then:

```
normalize(print(normalize(q))) == normalize(q)        (* hard invariant *)
print(normalize(print(normalize(q)))) == print(normalize(q))
```

This is required by RFC *Canonical Read Pipeline* step 2.

### 10.2 Idempotency caveats

- the `keyword`-mode adjacency-link annotation is **not** symmetric over
  reorder; the canonical AST stores adjacency ordering, and the printer
  preserves it. Idempotency holds, but commutativity of AND does not.
- `OR-within-kind` filter merging is order-stable: the canonical form
  sorts pattern strings lexicographically before merge. This ensures
  `repo:a repo:b` and `repo:b repo:a` produce the same AST and the same
  printed form.
- normalization does **not** evaluate predicates; the predicate AST
  node is normalized only at the arg-coercion layer (§7.2)

---

## 11. Canonical hash

The canonical AST must hash stably for cache keys, audit trails, and
plan dedup.

### 11.1 Serialization

- format: **CBOR**, RFC 8949, canonical (deterministic) encoding rules
  (RFC 8949 §4.2.1: sorted map keys, shortest int encoding, definite
  lengths only, no tags except the explicit version tag below)
- root: a single CBOR array `[version, ast]` where `version` is the
  string `"LQ/Core-1.0"` plus extension tags ordered lexicographically
- AST node encoding: each node is a CBOR map keyed by short string
  tags; the tag set is fixed in the contract crate

### 11.2 Hash function

- **SHA-256** over the CBOR bytes
- the resulting 32-byte digest is the canonical query hash
- hex-encoded representation: lowercase, no separator

### 11.3 Carrier type

The hash is carried on the contract crate type
`LqCanonicalHashV1` (placeholder name; the typed contract crate from
the RFC owns the final name). Carriers must:

- include the schema version string
- include the digest bytes
- include a creation timestamp (UTC)
- be `Eq + Hash + Serialize + Deserialize`

### 11.4 Stability guarantee

Hash stability holds across:

- process restarts
- machine architectures (CBOR canonical form is byte-deterministic)
- compiler upgrades within the same DSL version

Hash stability does **not** hold across:

- DSL version bumps (the `version` field changes)
- normalization rule changes (which require a DSL minor bump)

---

## 12. Error taxonomy

All grammar / plan-layer violations are typed. Production paths must
never silently degrade — RFC invariant *fail-closed by default*.

| Error code                    | When                                      | Carries                                |
|-------------------------------|-------------------------------------------|----------------------------------------|
| `PARSE_INVALID_UTF8`          | non-UTF-8 input                           | byte offset                            |
| `PARSE_OVERSIZED`             | input exceeds 16 KiB                      | actual byte size                       |
| `PARSE_LEX_ERROR`             | tokenizer failure                         | offset, expected, found                |
| `PARSE_SYNTAX_ERROR`          | grammar failure                           | position, expected production          |
| `PARSE_INVALID_REGEX`         | regex compile failure                     | regex source, RE2 error message        |
| `PARSE_FORBIDDEN_SYNTAX`      | lookbehind / generic `@` / etc.           | offending token, token position        |
| `PARSE_UNKNOWN_FILTER`        | filter name not in registry               | filter name                            |
| `PARSE_INVALID_FILTER_VALUE`  | filter value fails pinned grammar         | filter name, value, expected shape     |
| `PARSE_INVALID_PATTERNTYPE`   | unknown patterntype mode                  | mode value                             |
| `PARSE_UNSUPPORTED_COMBO`     | mutually-exclusive filters / etc.         | offending pair                         |
| `PLAN_UNKNOWN_PREDICATE`      | predicate not in active engine registry   | predicate name, filter name            |
| `PLAN_LIMIT_EXCEEDED`         | depth / regex NFA / state caps            | dimension, limit, actual               |

Mapping to surface error layer is contract-crate concern; this doc only
pins the taxonomy.

---

## 13. Limits and budgets

Proposed defaults. All values are configurable via deployment config,
but the parser refuses to go below the floor noted in parentheses.

| Dimension                          | Default      | Floor      |
|------------------------------------|--------------|------------|
| max query length (bytes)           | 16 KiB       | 1 KiB      |
| max AST depth                      | 32           | 8          |
| max boolean fan-out per node       | 64           | 8          |
| max filter count per kind          | 32           | 4          |
| max regex NFA state count          | 100_000      | 1_000      |
| max structural pattern node count  | 256          | 64         |
| max top-K (count:)                 | 10_000       | 100        |
| per-query memory soft limit        | 256 MiB      | 16 MiB     |
| per-query CPU soft limit           | 5 s          | 250 ms     |

Exceeding any cap -> `PLAN_LIMIT_EXCEEDED` with `dimension` and `limit`.
Soft limits (memory, CPU) emit a typed early-stop signal in the result
stream; they do not silently truncate.

---

## 14. Compatibility test corpus reference

Ownership split:

- this doc owns the grammar contract
- the parallel-authored `usecase.md` owns the golden test corpus
- the parallel-authored `feature-scope.md` owns the feature-level
  scope statement

The contract this doc binds:

1. every parseable Sourcegraph query in the corpus **must** parse here
   without source rewrite (RFC *Compatibility Rules*)
2. every such query **must** produce a canonical AST that survives the
   idempotency invariant (§10.1)
3. every such query **must** hash stably under §11
4. queries that exercise features outside `LQ/Core-1.0 + History-1.1 +
   Structural-1.2 + Runtime-1.3 + Bridge-1.4` are out of contract; the
   corpus owner marks them as such

Tests live as TOML/YAML files next to this doc, owned by the corpus
agent. The parser implementation crate is required to run them as part
of `cargo test --workspace`.

---

## 15. Cross-references

- RFC sections superseded by this doc:
  - *Canonical Grammar Semantics → Pattern semantics* — supplemented
    by §3 (regex dialect, RawString backend requirement)
  - *Canonical Grammar Semantics → Boolean semantics* — supplemented
    by §4 (mode-dependent adjacency) and §5
  - *Canonical Grammar Semantics → Filter semantics* — supplemented
    by §6, §7
  - *LQ Family → LQ/Structural-1.2* — formalized in §8
  - *Compatibility Rules → normalization examples* — formalized in §10
- forward references (siblings authored in parallel, not yet landed):
  `feature-scope.md`, `usecase.md`
- when those siblings land, run `python3 tools/ci/lint/lint-doc-paths.py`
  to confirm any markdown links added back to this doc resolve

---

## 16. Non-Negotiable DSL Invariants

Restated for enforcement:

1. no comment syntax in query text
2. no fuzzy-by-default keyword semantics (RFC echo)
3. no silent regex dialect drift away from RE2
4. no silent patterntype default drift away from `standard`
5. no silent acceptance of unknown filters / unknown predicates
6. no silent rewrite of `path:` vs `file:` semantics
7. normalization is idempotent (§10.1) — any change that breaks this
   requires a DSL minor version bump
8. canonical hash is stable across processes (§11.4)
9. structural patterns never guess language (§8.5)
10. directives never bypass filters (§9.3)
