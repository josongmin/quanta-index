# Lexical Capability Matrix

> Active proof inventory. Completed execution tickets are historical. Current
> architecture is owned by [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md)
> and [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md).

Status: `proof-accounted`
Date: `2026-06-02`
Historical program owner: `git show eff53181:docs/plans/jun-2-dsl-final-cut/tickets/JFC-00-truth-freeze-and-scope-lock.md`
Historical evidence origin: `git show eff53181:docs/plans/may-25-lexical-enhancement/tickets/LXE-00-truth-freeze-and-executable-matrix.md`
Current decision authority: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md)
Machine-readable ledger: [dsl-proof-ledger.toml](dsl-proof-ledger.toml)

이 문서는 원래 DSL 표면(`git show eff53181:docs/plans/may-24-lexical-indexing-sourcegraph/dsl.md`)별 현재 proof inventory다. 목적은 "전부 green" 선언이 아니라 각 표면이 현재 어떤 증거 상태인지 정확히 하나로 분류하는 것이다.

## Status Legend

- `active runtime`: `e2e_full_corpus` closeout rail에서 실 query/실 carrier로 증명됨
- `active owner-local`: 현재 tree에서 live proof는 있으나 owner-local companion rail에만 있음
- `typed fail-closed`: 현재 구현이 typed error 또는 parse rejection으로 닫힘
- `parser_only`: parser/normalizer shape는 있으나 실행 proof가 없음
- `blocked`: 현재 tree에 직접 증명 레일이 없어서 closeout claim을 올리면 거짓이 됨

## Primary Rails

- Runtime closeout:
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- Text and SG parity companion rails:
  `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- Structural/history/runtime companion rails:
  `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
  `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
  `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- Bridge companion rail:
  `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`

## Code-Owned Capability Sources

These are the code SSOTs the documentation tracks; the `jun-2-dsl-advanced`
drift checker (`tools/ci/lint/check-dsl-capability-truth.py`, ADV-04) reads them
so the ledger/matrix cannot silently diverge from the executable subset:

- predicate subset: `crates/quanta-index-lexical/src/predicate_registry.rs`
  (`PREDICATE_REGISTRY` — the only enumeration of executable lexical predicate
  names and their lowering kind)
- SG structural legality: `crates/quanta-index-search-plane/src/lowering.rs`
  (`structural_leaf_verdict` leaf-kind verdict matrix; `rewrite_sourcegraph_structural_expr`
  dispatches on it). The checker freezes this map against
  `EXPECTED_STRUCTURAL_VERDICTS` (code-vs-snapshot), not against this prose.

## Leaves And Boolean

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `Keyword` | `active runtime` | `runtime_native_content_term`, `runtime_sourcegraph_patterntype_standard_casefold`, `runtime_sourcegraph_patterntype_keyword_casefold`, `runtime_sourcegraph_patterntype_literal_casefold_hit` | bare keyword and explicit SG pattern modes execute on the lexical text rail with exact ordered ids for the current live subset |
| `Phrase` | `active runtime` | `runtime_sourcegraph_phrase_positive`, `runtime_sourcegraph_phrase_negative`, `e2e_lexical_full_fidelity` (`phrase_exact_adjacent_matches`) | public phrase query surface is runtime-proved; parity/recovery details stay on companion rails |
| `RawString` | `active runtime` | `runtime_native_raw_substring` | raw substring executes through lexical raw-substring path |
| `Regex` | `active runtime` | `runtime_native_regex`, `runtime_sourcegraph_regexp_option` | regex executes on lexical route with exact verify |
| `StructuralBlock` | `active runtime` | `runtime_native_structural_root_kind`, `runtime_sourcegraph_patterntype_structural_named_hole`, `runtime_native_structural_unsupported_lang` | runtime subset is live on structural route, and the closeout rail asserts exact structural bindings as well as candidate ids |
| `Predicate repo.has.file(...)` | `active runtime` | `runtime_*_repo_has_file_true_gate_multi_repo`, `runtime_*_repo_has_file_scalar_path_*`, `runtime_*_repo_has_path_*`, `runtime_sourcegraph_repo_has_file_miss_multi_repo`, `runtime_native_repo_has_file_not_false_multi_repo`, `tantivy_repo_has_file_true_gate_narrows_by_indexed_source_repo_id`, `tantivy_executes_repo_has_file_predicate_with_lang_matcher`, `e2e_filter_execution` (`repo_has_path_alias_executes_on_sourcegraph_surface`, `repo_has_file_scalar_path_executes_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`repo_has_file_scalar_path_predicate_parity`, `repo_has_file_lang_predicate_parity`, `repo_has_path_alias_parity`, `repo_has_path_native_alias_parity`), `dsl_scenarios`, `e2e_perf_chaos`, `e2e_lexical_full_fidelity`, `tantivy_smoke` | true-gate narrows by indexed `source_repo_id`; executable arg subset is one scalar path shorthand plus `path:` / `name:` regex matchers and `lang:` exact match on the indexed language field. Native alias `repo.has.path(...)`, SG sugar `repo:has.path(...)`, and scalar `repo:has.file(src/lib.rs)` all lower onto the same path matcher surface. The contract is owned by `predicate_registry::parse_repo_file_matchers`; other filters typed-fail |
| `Predicate repo.has.content(...)` | `active runtime` | `runtime_*_repo_has_content_true_gate_multi_repo`, `runtime_native_repo_contains_content_*`, `runtime_*_repo_has_content_number_*`, `tantivy_repo_has_content_true_gate_narrows_by_indexed_source_repo_id`, `tantivy_executes_repo_has_content_predicate_under_or_and_not`, `e2e_filter_execution` (`repo_has_content_predicate_executes_on_sourcegraph_surface`, `repo_contains_content_alias_executes_on_sourcegraph_surface`, `numeric_content_predicates_execute_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`repo_has_content_predicate_parity`, `repo_contains_content_native_alias_parity`, `repo_has_content_number_parity`), `tantivy_smoke` (`tantivy_executes_native_predicate_aliases`, `tantivy_executes_numeric_content_predicates`) | repo-wide existence gate is runtime-proved on the shared content scalar subset (`keyword` / `phrase` / `raw-string` / `number`). Native alias `repo.contains.content(...)` lowers onto the same gate, and numeric scalars canonicalize to decimal keywords. Matching repos return the lexical hits already selected by the surrounding query; this is a repo gate, not a matching-file projection. Scoped repo-content families remain typed-fail |
| `Predicate repo.has.commit.after(...)` | `active runtime` | `quanta-index-contract` IPC round-trip tests (`repo_commit_recency_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_commit_recency`, `search_plane_ingest_response_envelope_round_trip_repo_commit_recency_receipt`), `quanta-index-sdk` (`history_publish_repo_commit_recency_routes_through_ingest_transport`), `quanta-index-lexical` (`predicate_repo_has_commit_after_plans_through_tantivy_route`), `e2e_filter_execution` (`repo_has_commit_after_predicate_executes_on_sourcegraph_surface`, `repo_contains_commit_after_alias_executes_with_human_timeref_and_boolean_scope`, `repo_has_commit_after_rejects_invalid_timeref_typed`), `sdk_frontdoor` (`sourcegraph_repo_has_commit_after_positive`, `sourcegraph_repo_contains_commit_after_human_positive`, `sourcegraph_repo_has_commit_after_invalid_timeref_typed_fail`) | repo-wide existence gate is driven by source-repo keyed commit-recency authority materialized alongside the lexical generation, not by per-file text matches. Canonical `repo.has.commit.after(...)` and SG/native alias `repo.contains.commit.after(...)` share the same repo gate and reuse the shared timeref parser (`RFC3339`, date-only, duration, human phrase subset). Matching repos return the lexical hits already selected by the surrounding query; external producer emission of the authority batch is still a separate integration seam |
| `Predicate repo.has.meta(...)` | `active runtime` | `quanta-index-contract` IPC round-trip tests (`repo_meta_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_meta`, `search_plane_ingest_response_envelope_round_trip_repo_meta_receipt`), `quanta-index-sdk` (`history_publish_repo_meta_routes_through_ingest_transport`), `quanta-index-lexical` (`predicate_repo_has_meta_arity`, `repo_meta_arg_admits_key_value_and_rejects_other_shapes`), `tantivy_smoke` (`tantivy_executes_repo_metadata_filters_when_bundle_payload_is_typed`), `e2e_filter_execution` (`repo_has_meta_predicate_executes_on_sourcegraph_surface`, `repo_has_meta_rejects_key_only_shape_typed`), `e2e_dual_syntax_lowering_parity` (`repo_has_meta_predicate_parity`, `repo_has_meta_key_only_typed_fail_parity`), `sdk_frontdoor` (`sourcegraph_repo_has_meta_positive`, `sourcegraph_repo_has_meta_key_only_typed_fail`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | repo-wide existence gate is driven by source-repo keyed repo-metadata authority materialized alongside the lexical generation, not by runtime catalog document metadata. Current executable subset is exactly one `key:value` filter argument with lowercase-normalized key and exact value semantics. Key-only shapes remain outside the admitted subset and must typed-fail until distinct authority/proof exists |
| `Predicate repo.has.topic(...)` | `active runtime` | `quanta-index-contract` IPC round-trip tests (`repo_topic_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_topic`, `search_plane_ingest_response_envelope_round_trip_repo_topic_receipt`), `quanta-index-sdk` (`history_publish_repo_topic_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `repo_topic_arg_accepts_one_textual_topic`, `repo_topic_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`repo_has_topic_predicate_executes_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`repo_has_topic_predicate_parity`), `sdk_frontdoor` (`sourcegraph_repo_has_topic_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | repo-wide existence gate is driven by source-repo keyed repo-topic authority materialized alongside the lexical generation. Admitted subset is exactly one textual topic scalar with lowercase exact topic-set semantics. Matching repos return the lexical hits already selected by the surrounding query; key/value repo metadata and repo topic are distinct authorities and must not be conflated |
| `Predicate repo.has.description(...)` | `active runtime` | `quanta-index-contract` IPC round-trip tests (`search_plane_ingest_request_envelope_round_trip_repo_description`, `search_plane_ingest_response_envelope_round_trip_repo_description_receipt`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`), `e2e_filter_execution` (`repo_has_description_predicate_executes_on_sourcegraph_surface`, `repo_has_description_invalid_regex_fails_closed`, `repo_has_description_without_authority_fails_closed`) | repo-wide existence gate is driven by source-repo keyed repo-description authority materialized alongside the lexical generation (`RepoDescriptionIngestBatch` → `repo-description.cbor` shard), distinct from repo metadata and repo topics. The admitted subset is exactly one textual pattern scalar compiled as a regex and matched against each repo's verbatim description; a malformed pattern fails closed with `LEX_REGEX_*` and a missing authority fails closed with `REPO_DESCRIPTION_UNAVAILABLE`. The search-plane never reads source bytes — the producer publishes the description string |
| `Predicate file.contains(...)` | `active runtime` | `runtime_native_file_contains_raw_hit`, `runtime_*_file_contains_content_phrase_*`, `runtime_native_file_contains_number_*`, `runtime_*_file_contains_scoped_*`, `sdk_frontdoor`, `e2e_filter_execution` (`file_contains_content_alias_executes_on_sourcegraph_surface`, `numeric_content_predicates_execute_on_sourcegraph_surface`, `scoped_file_content_predicates_execute_on_sourcegraph_surface`, `scoped_file_content_predicates_fail_closed_under_or_not_and_bad_matchers`), `e2e_perf_chaos`, `tantivy_smoke` (`file_has_content_predicate_phrase_and_regex`, `tantivy_executes_native_predicate_aliases`, `tantivy_executes_numeric_content_predicates`, `tantivy_executes_scoped_file_content_predicates_and_fails_closed_in_or_not`), `e2e_dual_syntax_lowering_parity` (`file_contains_raw_substring_parity`, `file_contains_phrase_parity`, `file_contains_content_alias_parity`, `file_contains_content_native_alias_parity`, `file_contains_number_parity`, `file_contains_scoped_path_parity`, `file_contains_scoped_file_parity`, `file_contains_scoped_and_parity`, `file_contains_scoped_or_typed_fail_parity`, `file_contains_scoped_not_typed_fail_parity`, `file_contains_scoped_unknown_matcher_typed_fail_parity`, `file_contains_multiple_scalars_typed_fail_parity`) | native dotted predicate surface executes on the shared content scalar subset, including native alias `file.contains.content(...)` and numeric scalar canonicalization. File-scoped `file:` / `path:` / `lang:` constraints are executable at top level and conjunctive `AND`, reusing the existing file/lang substrate to narrow allowed paths. Scoped content predicates inside `OR` / `NOT` stay typed-fail with `LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED` |
| `Predicate file.has.content(...)` | `active runtime` | `runtime_sourcegraph_file_has_content_regex_hit`, `runtime_sourcegraph_file_has_content_phrase_miss`, `runtime_native_file_has_content_number_hit`, `runtime_*_file_has_content_scoped_lang_regex_hit`, `tantivy_smoke` (`file_has_content_predicate_phrase_and_regex`, `tantivy_executes_numeric_content_predicates`, `tantivy_executes_scoped_file_content_predicates_and_fails_closed_in_or_not`), `e2e_filter_execution` (`numeric_content_predicates_execute_on_sourcegraph_surface`, `scoped_file_content_predicates_execute_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`file_has_content_regex_parity`, `file_has_content_number_parity`, `file_has_content_scoped_lang_regex_parity`) | native dotted predicate name and SG alias share the same content-leaf substrate. Numeric scalars are admitted via the shared content contract, and `lang:`-scoped regex is runtime/parity proved. Like `file.contains(...)`, scoped forms are executable only at top level and conjunctive `AND`; scoped `OR` / `NOT` stays typed-fail |
| `Predicate file.has.owner(...)` | `active runtime` | `quanta-index-sdk` (`history_publish_file_ownership_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `file_owner_arg_accepts_zero_or_one_textual_owner`, `file_owner_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`file_has_owner_executes_on_sourcegraph_surface`, `file_has_owner_executes_inside_boolean_scope`), `e2e_dual_syntax_lowering_parity` (`file_has_owner_predicate_parity`), `sdk_frontdoor` (`sourcegraph_file_has_owner_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | query-side file owner gate is driven by source-repo keyed file-ownership authority materialized alongside the lexical generation, not by runtime facets or repomap ownership. Admitted subset is zero args (`any owner`) or exactly one textual owner identity with lowercase exact matching. The executor narrows candidate ids directly by `(source_repo_id, repo_relative_path)` authority lookup, so same-path files in different repos do not alias |
| `Predicate file.has.contributor(...)` | `active runtime` | `quanta-index-contract` IPC round-trip tests (`file_contributor_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_file_contributor`, `search_plane_ingest_response_envelope_round_trip_file_contributor_receipt`), `quanta-index-sdk` (`history_publish_file_contributor_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `file_contributor_arg_accepts_one_textual_contributor`, `file_contributor_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`file_has_contributor_executes_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`file_has_contributor_predicate_parity`), `sdk_frontdoor` (`sourcegraph_file_has_contributor_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | query-side file contributor gate is driven by source-repo keyed file-contributor authority materialized alongside the lexical generation. Admitted subset is exactly one textual contributor identity with lowercase exact matching. The executor narrows candidate ids directly by `(source_repo_id, repo_relative_path)` contributor lookup, so same-path files in different repos do not alias and repo-level author fallback is forbidden |
| Predicate names / arg shapes outside shipped subset | `typed fail-closed` | `quanta-index-lexical` `predicate_registry` (`PREDICATE_REGISTRY`, `PREDICATE_ALIASES`, shared content parsers) + planner typed-unavailable paths + bridge rejection rails | unsupported predicate lowering does not silently widen; canonical executable names are registry-owned, native aliases must canonicalize onto them, and arg shapes outside the admitted shared/scoped contracts remain typed-fail |
| `Empty` | `parser_only` | `lq-norm` parser/normalizer keep `LqExpr::Empty`; no runtime row | empty query is a parser shape, not a closeout-green runtime surface |
| `Not` | `active runtime` | `runtime_native_boolean_not` | lexical boolean negation executes with exact ordered ids |
| `All` | `active runtime` | `runtime_native_boolean_and` | lexical boolean AND executes on text rail |
| `Any` | `active runtime` | `runtime_native_boolean_or` | lexical boolean OR executes on text rail |

## Pattern Options

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `patterntype:literal` | `active runtime` | `runtime_sourcegraph_patterntype_literal_casefold_hit` | explicit literal mode currently executes on the lexical text rail with the same case-folded behavior as the shipped subset |
| `patterntype:keyword` | `active runtime` | `runtime_sourcegraph_patterntype_keyword_casefold` | explicit keyword mode executes on the lexical text rail |
| `patterntype:standard` | `active runtime` | `runtime_sourcegraph_patterntype_standard_casefold` | explicit standard mode executes on the lexical text rail |
| `patterntype:regexp` | `active runtime` | `runtime_sourcegraph_regexp_option` | regexp mode executes on lexical regex route |
| `patterntype:structural` | `active runtime` | `runtime_sourcegraph_patterntype_structural_named_hole`, `e2e_dual_syntax_lowering_parity` | structural mode is runtime-proved on the dedicated structural route |
| `case` | `active runtime` | `runtime_native_case_sensitive_miss` | case-sensitive mismatch is asserted on runtime rail |
| `count:N` | `active runtime` | `runtime_native_count_bound` | bounded count truncation is asserted with exact ordered ids |
| `count:all` | `active runtime` | `runtime_sourcegraph_count_all` | `count:all` now bypasses request `top_k` and returns full recall on the runtime rail |
| `timeout` | `active runtime` | `runtime_native_timeout_typed`, `e2e_perf_chaos`, `sdk_frontdoor`, `end_to_end` | runtime rail proves typed lexical timeout; recovery invariant stays on companion rails |

## Core Filters

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `repo:` | `active runtime` | `runtime_sourcegraph_repo_miss`, `runtime_native_repo_allow_list_corp_a`, `runtime_native_repo_allow_list_corp_b`, `runtime_native_repo_allow_list_universe_baseline`, `tantivy_smoke` | `repo:` allow-list matches `ChunkRecord::source_repo_id` when present; `docs-multi-repo.toml` proves non-vacuous positive/miss/universe baselines |
| `file:` | `active runtime` | `runtime_native_file_filter` | file filter narrows content hits by path |
| `path:` | `active runtime` | `runtime_sourcegraph_path_filter` | path-only filter executes on runtime text rail |
| `lang:` | `active runtime` | `runtime_native_lang_filter` | language filter executes on runtime text rail |
| `rev:` | `active runtime` | `runtime_sourcegraph_history_commit_rev_ref`, `runtime_sourcegraph_history_commit_rev_tag` | rev is live on history `type:commit`; lexical text rail still fail-closes producerless shapes |
| `type:file` | `active runtime` | `runtime_native_type_file_excludes_symbol_only` | file type excludes symbol-only matches on the live runtime rail |
| `type:path` | `active runtime` | `runtime_native_type_path_projection` | path type now projects text hits down to one deterministic representative per path on the runtime rail |
| `type:symbol` | `active runtime` | `runtime_native_type_symbol` | symbol type routes to symbol docs through live runtime rail |
| `type:commit` | `active runtime` | `runtime_native_history_commit`, `runtime_native_history_author`, `runtime_native_history_committer`, `runtime_native_history_message`, `sdk_frontdoor`, `end_to_end`, `e2e_perf_chaos` | commit history route executes with exact ids; history now requires explicit `type:commit` or `type:diff`, and `type:commit file:/diff.*` combinations fail closed with `INVALID_REQUEST` |
| `type:diff` | `active runtime` | `runtime_native_history_diff`, `end_to_end`, `e2e_perf_chaos` | diff history route executes on live runtime rail; `file:` and `diff.*` are owned by this route rather than silently emptying on commit queries |
| `type:repo` | `active runtime` | `runtime_native_type_repo` | repo type now projects text hits down to one deterministic representative per repo on the runtime rail |
| `select:repo` | `active runtime` | `runtime_native_select_repo` | repo projection collapses to one representative |
| `select:file` | `active runtime` | `runtime_native_select_file_projection` | file projection collapses same-path multi-chunk hits to per-path representatives on the runtime rail |
| `select:path` | `active runtime` | `runtime_native_select_path_projection` | path projection collapses same-path multi-chunk hits to per-path representatives on the runtime rail |
| `select:symbol` | `active runtime` | `runtime_sourcegraph_select_symbol` | symbol projection executes through live symbol carrier |
| `select:content` | `active runtime` | `runtime_native_select_content_projection` | content projection is now asserted on the runtime rail with exact ordered ids, paths, and snippets |
| `select:content.match` | `active runtime` | `runtime_native_select_content_match_projection` | content-match projection is now asserted on the runtime rail with exact ordered ids, paths, and snippets |
| `fork:` | `active runtime` | `runtime_native_fork_filter_only_miss`, `runtime_sourcegraph_fork_typed_unavailable` | `fork:only` excludes non-fork repos when metadata exists; producerless path stays typed-unavailable |
| `archived:` | `active runtime` | `runtime_sourcegraph_archived_filter_only_miss`, `dsl_scenarios`, `end_to_end` | runtime rail proves metadata-backed exclusion on non-archived repos; positive metadata path remains exercised on companion rails |
| `visibility:` | `active runtime` | `runtime_sourcegraph_visibility_filter_miss`, `runtime_sourcegraph_visibility_typed_unavailable` | mismatched visibility excludes hits when metadata exists; producerless path stays typed-unavailable |
| `context:` | `active runtime` | `runtime_sourcegraph_context_filter_miss`, `dsl_scenarios`, `end_to_end` | runtime rail proves context mismatch exclusion; positive context match remains exercised on companion rails |
| `content:` | `active runtime` | `runtime_native_content_filter_positive`, `runtime_native_content_filter_miss` | content filter now has direct positive and path-leak negative proof on the runtime rail |

## History Extension

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `author:` | `active runtime` | `runtime_native_history_author` | commit author filter executes on history route |
| `committer:` | `active runtime` | `runtime_native_history_committer` | commit committer filter executes on history route |
| `message:` | `active runtime` | `runtime_native_history_message` | commit message filter executes on history route |
| `before:` | `active runtime` | `runtime_native_history_before`, `runtime_native_history_before_invalid` | `committer_time_ms` upper bound; invalid timeref → `HISTORY_INVALID_TIMEREF` |
| `after:` | `active runtime` | `runtime_native_history_after`, `end_to_end`, `e2e_perf_chaos` | `committer_time_ms` lower bound (strict) with raw IPC and no-poison companion proof |
| `since:` | `active runtime` | `runtime_native_history_since`, `runtime_native_history_since_time`, `runtime_native_history_since_commit_ref`, `runtime_native_history_since_commit_unknown`, `sdk_frontdoor`, `e2e_perf_chaos` | `since:` is inclusive lower bound on `committer_time_ms`; qualified `since.time:` / `since.commit:` execute on the same history authority and unknown refs typed-fail `HISTORY_INVALID_TIMEREF`, with builder/transport proof on `sdk_frontdoor` |
| `until:` | `active runtime` | `runtime_native_history_until`, `end_to_end`, `e2e_perf_chaos` | bare `until:` is inclusive upper bound on `committer_time_ms` with raw IPC and no-poison companion proof |
| `diff.added:` | `active runtime` | `runtime_native_history_diff_added`, `runtime_native_history_diff_added_miss`, `end_to_end`, `e2e_perf_chaos` | narrows `added_text` only (not concat search buffer) |
| `diff.removed:` | `active runtime` | `runtime_native_history_diff_removed`, `end_to_end`, `e2e_perf_chaos` | narrows `removed_text` only |
| `diff.touched:` | `active runtime` | `runtime_native_history_diff_touched`, `runtime_native_history_diff_touched_secondary`, `end_to_end`, `e2e_perf_chaos` | narrows `touched_text` only |

## Runtime Extension

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `dirty:` | `active runtime` | `runtime_sourcegraph_runtime_dirty`, `runtime_sourcegraph_runtime_dirty_missing_filter`, `runtime_sourcegraph_runtime_dirty_only_unsupported`, `runtime_sourcegraph_runtime_dirty_no`, `sdk_frontdoor`, `e2e_perf_chaos` (`runtime_catalog_dirty_only_rejects_typed_and_does_not_poison_next_query`, `runtime_catalog_dirty_no_executes_and_does_not_poison_next_query`), `e2e_restart_replay_determinism` | runtime-metadata route executes `dirty:yes` over `dirty_docs` and `dirty:no` over the clean complement inside the pinned generation; `dirty:only` typed-fails `RUNTIME_DIRTY_ONLY_UNSUPPORTED`, and authority-less queries still typed-fail |
| `changed:` | `active runtime` | `runtime_sourcegraph_runtime_changed_positive`, `runtime_sourcegraph_runtime_changed_miss`, `runtime_sourcegraph_runtime_catalog_not_ready` | `changed:since=<timeref>` matches generation-pinned `changed_docs` by `applied_at_ms`; missing catalog typed-fails `RUNTIME_CATALOG_NOT_READY` |
| `stale:` | `active runtime` | `runtime_sourcegraph_runtime_stale_positive`, `runtime_sourcegraph_runtime_stale_miss`, `runtime_sourcegraph_runtime_stale_head_not_ahead_miss`, `e2e_dual_syntax_lowering_parity` (`runtime_stale_filter_parity`, `runtime_stale_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `stale:before=<timeref>` matches only when `producer_head_applied_at_ms > generation_materialized_at_ms` and `generation_materialized_at_ms <` bound; proof uses both bound inversion and head-not-ahead miss rows so a no-op comparator cannot pass |
| `affected:` | `active runtime` | `runtime_sourcegraph_runtime_affected_positive`, `runtime_sourcegraph_runtime_affected_miss`, `sdk_frontdoor`, `e2e_dual_syntax_lowering_parity` (`runtime_affected_filter_parity`, `runtime_affected_filter_miss_parity`), `e2e_perf_chaos` (`runtime_catalog_affected_executes_and_does_not_poison_next_query`), `e2e_restart_replay_determinism` | `affected:<scope>` matches generation-pinned edge-authority catalog entries keyed by scope and narrows the current runtime universe without lexical fallback |
| `invalidated_by:` | `active runtime` | `runtime_sourcegraph_runtime_invalidated_by_positive`, `runtime_sourcegraph_runtime_invalidated_by_miss`, `sdk_frontdoor`, `e2e_dual_syntax_lowering_parity` (`runtime_invalidated_by_filter_parity`, `runtime_invalidated_by_filter_miss_parity`), `e2e_perf_chaos` (`runtime_catalog_invalidated_by_executes_and_does_not_poison_next_query`), `e2e_restart_replay_determinism` | `invalidated_by:<source>` matches generation-pinned edge-authority catalog entries keyed by source and narrows the current runtime universe without lexical fallback |
| `snapshot:` | `active runtime` | `runtime_sourcegraph_runtime_snapshot_active`, `runtime_sourcegraph_runtime_snapshot_miss`, `runtime_sourcegraph_runtime_snapshot_unknown`, `end_to_end`, `e2e_dual_syntax_lowering_parity` (`runtime_snapshot_filter_parity`, `runtime_snapshot_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `snapshot:<name>` matches membership in persisted snapshot sets; proof uses shared-token member/non-member docs so a no-op membership filter cannot pass; unknown names typed-fail `SNAPSHOT_UNKNOWN` |
| `meta.owner:` | `active runtime` | `runtime_sourcegraph_runtime_meta_owner`, `runtime_sourcegraph_runtime_meta_owner_miss`, `end_to_end`, `e2e_dual_syntax_lowering_parity` (`runtime_meta_owner_filter_parity`, `runtime_meta_owner_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `meta.owner:<id>` requires exact owner facet match on `doc_facets`; proof uses shared-token decoys with mismatched owner facets so a no-op facet filter cannot pass |
| `meta.service:` | `active runtime` | `runtime_sourcegraph_runtime_meta_service`, `runtime_sourcegraph_runtime_meta_service_miss`, `end_to_end`, `e2e_dual_syntax_lowering_parity` (`runtime_meta_service_filter_parity`, `runtime_meta_service_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `meta.service:<id>` requires exact service facet match on `doc_facets`; proof uses shared-token decoys with mismatched service facets so a no-op facet filter cannot pass |
| `meta.layer:` | `active runtime` | `runtime_sourcegraph_runtime_meta_layer`, `runtime_sourcegraph_runtime_meta_layer_miss`, `end_to_end`, `e2e_dual_syntax_lowering_parity` (`runtime_meta_layer_filter_parity`, `runtime_meta_layer_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `meta.layer:<id>` requires exact layer facet match on `doc_facets`; proof uses shared-token decoys with mismatched layer facets so a no-op facet filter cannot pass |
| `meta.surface:` | `active runtime` | `runtime_sourcegraph_runtime_meta_surface`, `runtime_sourcegraph_runtime_meta_surface_miss`, `end_to_end`, `e2e_dual_syntax_lowering_parity` (`runtime_meta_surface_filter_parity`, `runtime_meta_surface_filter_miss_parity`), `e2e_perf_chaos`, `e2e_restart_replay_determinism` | `meta.surface:<id>` requires exact surface facet match on `doc_facets`; proof uses shared-token decoys with mismatched surface facets so a no-op facet filter cannot pass |

## Structural Mini-Language

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| root-kind and root capture | `active runtime` | `runtime_native_structural_root_kind`, `runtime_native_structural_root_capture` | runtime closeout rail asserts exact candidate ids and binding spans |
| `where` | `active runtime` | `runtime_native_structural_where` | constraint matching is runtime-proved with exact binding spans |
| `inside` | `active runtime` | `runtime_native_structural_inside` | inside scope matching is runtime-proved with exact binding spans |
| `outside` | `active runtime` | `runtime_native_structural_outside` | outside scope matching is runtime-proved with exact binding spans |
| named / anonymous / variadic holes | `active runtime` | `runtime_sourcegraph_patterntype_structural_named_hole`, `runtime_native_structural_anonymous_wildcard`, `runtime_native_structural_variadic` | named capture, anonymous wildcard, and variadic tree-walk are runtime-proved |
| typed holes (`expr`, `stmt`, `item`, `type`) | `active runtime` | `runtime_native_structural_typed_expr`, `runtime_native_structural_typed_item`, `runtime_native_structural_typed_stmt`, `runtime_native_structural_typed_type` | typed-hole subset is runtime-proved with exact binding spans |
| mixed lexical + structural boolean AND | `active_runtime` | `runtime_native_structural_mixed_lexical_and`, `e2e_dual_syntax_lowering_parity`, `sdk_frontdoor`, `e2e_perf_chaos` | lexical and structural leaves intersect on candidate-id set algebra before projection |
| mixed lexical + structural boolean OR | `active_runtime` | `runtime_native_structural_mixed_lexical_or`, `runtime_sourcegraph_structural_mixed_lexical_or`, `runtime_sourcegraph_structural_mixed_raw_string_or`, `e2e_dual_syntax_lowering_parity`, `sdk_frontdoor`, `e2e_perf_chaos` | OR unions mixed-domain candidates; raw-string siblings are runtime-proved on the SG route; divergent repo-scoped filters under mixed OR remain bridge fail-closed |
| mixed lexical + structural boolean AND NOT | `active_runtime` | `runtime_native_structural_mixed_lexical_and_not`, `runtime_sourcegraph_structural_mixed_lexical_and_not`, `runtime_sourcegraph_structural_mixed_raw_string_and_not`, `runtime_sourcegraph_structural_mixed_predicate_scalar_path_and_not`, `e2e_dual_syntax_lowering_parity`, `sdk_frontdoor`, `e2e_perf_chaos` | bounded NOT subtracts structural or lexical siblings from the seeded mixed-domain candidate set; SG route now has runtime/parity proof for raw-string and predicate NOT siblings |
| pure negative structural root (`NOT match { ... }`) | `active_runtime` | `runtime_native_structural_pure_negative_root`, `sdk_frontdoor`, `e2e_perf_chaos` | root `NOT` subtracts from explicit generation-pinned chunk universe |

## Directives

| Surface | Status | Proof rail | Current live behavior |
| --- | --- | --- | --- |
| `into:codeql` | `active owner-local` | `crates/quanta-index-lq-bridge/tests/bridge_directive_packet.rs` | bridge-packet carrier only; intentionally excluded from runtime search-result corpus |
| `scope:results` | `active owner-local` | `crates/quanta-index-lq-bridge/tests/bridge_directive_packet.rs` | bridge-packet carrier only; intentionally excluded from runtime search-result corpus |
| `with:lexical` | `active owner-local` | `crates/quanta-index-lq-bridge/tests/bridge_directive_packet.rs` | bridge-packet carrier only; intentionally excluded from runtime search-result corpus |

## Notes

- `runtime_rows.toml`는 executable search-result inventory만 담는다. bridge packet, public SDK packet, parser-only grammar shape는 companion rail에 남긴다.
- `active owner-local`은 green이지만 아직 main closeout rail로 승격되지 않은 표면이다. 이 상태를 `active runtime`으로 부풀려 적지 않는다.
- `typed fail-closed`는 parser reject든 planner/runtime typed error든, 현재 tree가 silent success 대신 명시적 실패를 주는 경우를 뜻한다.
- `dsl-proof-ledger.toml`의 `carrier_kind`는 `search_result`, `bridge_packet`, `parser_shape`를 구분한다. directive와 empty query는 closeout carrier가 다르므로 runtime corpus에 올리지 않는다.
