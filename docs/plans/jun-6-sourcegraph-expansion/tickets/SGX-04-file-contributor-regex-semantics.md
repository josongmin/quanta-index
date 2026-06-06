# SGX-04 — File Contributor Regex Semantics

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Land contributor lookup widening from the old exact canonical-string match to
name-or-email regex semantics without canonical raw-string fallback.

## Current Code Fact

- `file:has.contributor(<exact>)` is supported
- `file:has.contributor(/<regex>/)` is now supported
- contributor authority is now a structured identity set carrying `canonical`
  plus optional `name` / `email`
- external `semantica-codegraph-v2` ingress publish + live roundtrip proof for
  that structured authority is green for both `name` and `email` regex
  branches
- neighboring broad exact rail
  `quanta-runtime --lib history_wire_batch_maps_to_file_contributor_batch_v1`
  is also green

## Official Sourcegraph Baseline

- Sourcegraph describes `file:has.contributor(...)` in terms of contributor
  identity patterns, not just one exact opaque string

## Owner Seam

- contributor authority payload
- lexical contributor evaluator
- runtime/front-door/parity/corpus rails

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`

## Concrete Work Items

1. Widen history / contributor authority from opaque canonical strings to
   structured identities.
2. Keep current exact contributor behavior explicit as already-supported truth.
3. Add `/.../` contributor regex admission on the real owner seam.
4. Match regex only against normalized `name` OR `email`.
5. Prove no repo-level author fallback and no canonical raw-string regex
   fallback appear while widening.
6. Prove the external semantica producer publish + live roundtrip path.
7. Reflect the widened verdict in runtime/front-door/parity/corpus/guard/docs.

## First Increment

- pin current exact-only green behavior as not-backlog
- widen authority to separate `name` / `email` fields
- then land regex positive/miss/invalid rails

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution file_has_contributor_executes_on_sourcegraph_surface -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution file_has_contributor_supports_name_and_email_regex_without_canonical_fallback -- --nocapture
```

## Worker First Commands

```bash
rg -n "file:has\\.contributor|parse_file_contributor_arg|FileContributorIdentityEntry|contributors|regex" crates docs tools -S
sed -n '1180,1325p' crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs
```

## No-Go

- do not regex-match against the current single canonical string and call it
  Sourcegraph parity
- do not degrade to repo-level author search
- do not widen contributor semantics without proving the authority shape first

## DoD

- regex support lands only with a real structured authority shape and proof
- exact-only semantics remain preserved for bare scalar contributor queries
- canonical raw-string regex fallback does not exist
- semantica producer publish + live ingress roundtrip proof is green
- semantica neighboring broad exact rail is green for the contributor batch
  owner seam

## Not Done If

- regex is matched against the current opaque canonical string only
- repo-level author fallback is introduced
- exact-only current support regresses
- docs or guard still describe regex contributor as explicit unsupported
- semantica producer publish or live ingress roundtrip is not actually green
