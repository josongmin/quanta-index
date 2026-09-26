# L1 -> L0: pure primitive admission before LogicalEmpty

State: **FAILED**, reproduced on a source-stable dispatcher run. Shared API
change is **NOT_RUN / not applied** because L0 owns the interface freeze.
No cross-task message was sent.

## Reproduction and expected result

- Request: Text, native query `lang:python "!!!"`, typed language `rust`.
- Expected: `LEX_TEXT_QUERY_NO_TOKENS`, before opening a snapshot or executing
  search. Native literal validation already supplies this code.
- Actual: successful empty Text response, Exact(0), LogicalEmpty proof, no
  executed engine. Empty math is correct; accepting the invalid phrase is not.
- Initial source-stable receipt: `/tmp/quanta-l1-green.diz44g/dispatcher-current-2.receipt.json`.
  Command: `./scripts/cargow test --locked -j 2 -p quanta-index-search-plane --lib --message-format=json l1_`.
  It ran 12 tests (7 passed, 5 failed); four failures share the separate Symbol
  cursor lowering bug now repaired in L1. The phrase failure is independent.
- Latest selected receipt is indexed in `L1_PROOF.json`; the expanded L1
  selection also covers structural complete-set propagation. Historical RED
  must not be treated as the final source identity.
- Current regression: `contradictory_languages_do_not_hide_tokenless_phrase` in
  `query_dispatcher/tests/l1_query_domain_window.rs`.

## Minimum proposed integration seam (requires L0 decision)

Expose the existing lexical engine's pure primitive admission through the
already-injected opener, without acquiring a read view. Suggested mandatory
method on `LexicalIndexOpenPort`:

```rust
fn preflight_query_primitives(
    &self,
    plan: &ValidatedLexicalPlan,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError>;
```

No default success, no second tokenizer/regex implementation, and no
search-plane dependency on the concrete lexical adapter. L0 may instead move
the canonical pure validation into a shared backend-independent owner; both
consumers must call that same owner.

Callers in L1-owned files, after immutable domain admission and before language
composition or history/read-view acquisition:

- `planning.rs::plan_lexical_text_query`: use the already-produced pure plan
  (the supported `rev:at.time` selection filter is removed only for that plan).
- `routes/lexical.rs::symbol_with_execution`: use the Symbol-endpoint plan.
- Native admission must share the same primitive validation; capability and
  producer-state checks remain on the pinned read handle.

The implementation must reuse current `LexicalPlanner`, `query_errors`, and
normalizer/regex owners. Do not call the entire metadata-dependent preflight
with fabricated `has_repo_metadata = false`. Audit expression and Content
filter leaves, options that alter primitive interpretation, and predicate
arguments. Tokenless phrase and Keyword/Content behind empty predicates are reproduced;
other primitive cases remain required census, not established defects.

The trait is implemented by `LexicalAdapter` and multiple repository test
openers. A new mandatory method needs the full implementation census. For
instrumented dispatcher tests, record invocation and inject the canonical
admission outcome; native adapter tests must prove the real literal semantics.
A real assembled public-path test belongs to L0 and cannot be replaced by the
instrumented port test.

## Current implementation and primitive census

The full repository Rust token census found eight explicit implementations:

- `lexical/src/adapter_open.rs`: `LexicalAdapter`.
- `search-plane/src/control_dispatcher.rs`: `EchoLexicalOpener`,
  `LedgerWritingOpener<EchoLexicalOpener>`, `ForeignDigestOpener`.
- `search-plane/src/search_corpus_lifecycle.rs`: `EchoLexicalOpener`.
- `search-plane/src/query_dispatcher/tests/support/lexical.rs`:
  `RejectLexicalOpener`, `StubLexicalOpener`, `RecordingLexicalOpener`.

Paths above are relative to `crates/quanta-index-`. Raw census:
`l1-proof/current/resume-census-1.log.gz`; compressed and raw digests are
indexed with `resume-census.receipt.json`. The eight implementations are unchanged.
The installed composition already supplies the adapter through
`searchd-runtime/src/lib.rs`; no new concrete adapter dependency is necessary
in search-plane.

Do not expose `LexicalPlanner::validate_expr` alone as complete primitive
admission. Current source has these distinct meanings to preserve:

- `plan_leaf(Phrase)` performs real phrase tokenization, including no-token and
  token-length refusal; keyword leaves are only represented there, and their
  actual token validation lives in `query_errors::text_query_tokens`.
- Explicit Regex leaves are planned, but Keyword/RawString under Regexp options
  enter the regex engine in `compile_leaf`. Case policy also affects the
  execution source through `regex_source_for_options`.
- `LqFilter::Content` invokes `compile_leaf` outside the expression tree.
- Predicate scalar/arity checks use `predicate_registry`; metadata availability
  and coverage remain view-dependent and cannot be fabricated as false.
- Full query `plan_filters` includes producer-dependent unavailable entries.
  Separate pure input refusal from those capability checks.

L1's regex preflight regression additionally reproduced loss of the existing
`LEX_REGEX_DIALECT_PARSE_ERROR` code as `INVALID_REQUEST`. L1 repaired that
mapping in `searcher/planner_errors.rs` by reusing `map_regex_plan_error`; the
shared admission owner must preserve the same mapper. This does not implement
the missing dispatcher seam. Keyword/Content are additionally reproduced below. Remaining primitive census
entries are integration requirements, not newly reproduced behavioral defects.

After wiring: rerun the complete `l1_` dispatcher selection and native L1 target
with new before/after source manifests. Preserve valid language contradiction's
no-read/no-execution behavior and all cursor/request context checks.

## Additional native same-boundary regression

`predicate_result_empty_cannot_hide_keyword_or_content_tokens` checks tokenless
Keyword input both as an expression child and inside `LqFilter::Content`, after
`repo.has.content(needle)` / `repo.has.content(absent_term)`. It checks indexed and
manual execution and both `search` and `search_all`. The eight nonempty-predicate
controls retain `LEX_TEXT_QUERY_NO_TOKENS`; the eight empty-predicate combinations
incorrectly return `[]`.

Latest native rail: `native-all-resume-3`, **FAILED**, 85 passed /
1 failed of 86. Source manifest:
`0e0efb284fde4b7664cef66dda9dfff0cda47f67b8cb4882f99c09bfe54a049f`.
Latest dispatcher rail: `dispatcher-all-resume-2`, **FAILED**,
52 passed / 1 failed of 53.
Full commands, source revalidation and raw logs are in `L1_PROOF.json`.
The earlier `native-all-resume-1` changed normalizer source and remains stale.

The source concern is consistent with `LexicalPlanner::plan_leaf(Keyword)`
constructing a Content plan without token admission and `plan_filters` leaving
nested Content leaves for later compilation. The shared fix must cover native
predicate emptiness and dispatcher language emptiness. Do not close the whole
seam with a dispatcher-only phrase special case.
