# DSL proof inventory

Status: `DECLARED_PROOF_INVENTORY`; no fresh test result is asserted here.

The [machine-readable ledger](dsl-proof-ledger.toml) retains per-surface proof
owners, carrier kinds, expected behavior and companion rails for leaves, Boolean,
filters, history, runtime, structural patterns and directives. The generated
[Sourcegraph coverage](sourcegraph-filter-parity.md) names recognized
forms and source-test owners. These are inventories, not current-source support receipts.
Legacy carrier names in the table below identify historical proof ownership;
they are not selectors for the current test inventory. The ledger's rail IDs
are preserved as historical locators, not executable proof or a current pass.

Accepted contracts: [DSL](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md),
[Sourcegraph](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md),
[query/publication](../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md)
and [code search](../adr/SEP-27-003-code-search-source-and-preview-contract.md).

## Canonical lexical predicate boundaries

Keep these canonical names: `check-dsl-capability-truth.py` compares them with
`predicate_registry.rs` through the owner dump. Aliases are not independent
surfaces. The registry/shared parsers own current arguments; unsupported shapes
refuse typed. The table preserves the published boundaries, not a claim that its
historical runtime proofs were executed in this task. Six canonical predicate
rows are not yet in the TOML ledger; their existing proof owners remain here
so compaction does not erase that coverage mapping.

| Surface | Historical proof owner | Declared boundary |
| --- | --- | --- |
| `Predicate repo.has.file(...)` | TOML ledger | true-gate narrows by indexed `source_repo_id`; executable arg subset is one scalar path shorthand plus `path:` / `name:` regex matchers and `lang:` exact match on the indexed language field. Native alias `repo.has.path(...)`, SG sugar `repo:has.path(...)`, and scalar `repo:has.file(src/lib.rs)` all lower onto the same path matcher surface. The contract is owned by `predicate_registry::parse_repo_file_matchers`; other filters typed-fail |
| `Predicate repo.has.content(...)` | TOML ledger | repo-wide existence gate uses the shared content scalar subset (`keyword` / `phrase` / `raw-string` / `number`). Native alias `repo.contains.content(...)` lowers onto the same gate, and numeric scalars canonicalize to decimal keywords. Matching repos return the lexical hits already selected by the surrounding query; this is a repo gate, not a matching-file projection. Scoped repo-content families remain typed-fail |
| `Predicate repo.has.commit.after(...)` | `quanta-index-contract` IPC round-trip tests (`repo_commit_recency_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_commit_recency`, `search_plane_ingest_response_envelope_round_trip_repo_commit_recency_receipt`), `quanta-index-sdk` (`history_publish_repo_commit_recency_routes_through_ingest_transport`), `quanta-index-lexical` (`predicate_repo_has_commit_after_plans_through_tantivy_route`), `e2e_filter_execution` (`repo_has_commit_after_predicate_executes_on_sourcegraph_surface`, `repo_contains_commit_after_alias_executes_with_human_timeref_and_boolean_scope`, `repo_has_commit_after_rejects_invalid_timeref_typed`), `sdk_frontdoor` (`sourcegraph_repo_has_commit_after_positive`, `sourcegraph_repo_contains_commit_after_human_positive`, `sourcegraph_repo_has_commit_after_invalid_timeref_typed_fail`) | repo-wide existence gate is driven by source-repo keyed commit-recency authority materialized alongside the lexical generation, not by per-file text matches. Canonical `repo.has.commit.after(...)` and SG/native alias `repo.contains.commit.after(...)` share the same repo gate and reuse the shared timeref parser (`RFC3339`, date-only, duration, human phrase subset). Matching repos return the lexical hits already selected by the surrounding query; external producer emission of the authority batch is still a separate integration seam |
| `Predicate repo.has.meta(...)` | `quanta-index-contract` IPC round-trip tests (`repo_meta_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_meta`, `search_plane_ingest_response_envelope_round_trip_repo_meta_receipt`), `quanta-index-sdk` (`history_publish_repo_meta_routes_through_ingest_transport`), `quanta-index-lexical` (`predicate_repo_has_meta_arity`, `repo_meta_arg_admits_key_value_and_rejects_other_shapes`), `tantivy_smoke` (`tantivy_executes_repo_metadata_filters_when_bundle_payload_is_typed`), `e2e_filter_execution` (`repo_has_meta_predicate_executes_on_sourcegraph_surface`, `repo_has_meta_rejects_key_only_shape_typed`), `e2e_dual_syntax_lowering_parity` (`repo_has_meta_predicate_parity`, `repo_has_meta_key_only_typed_fail_parity`), `sdk_frontdoor` (`sourcegraph_repo_has_meta_positive`, `sourcegraph_repo_has_meta_key_only_typed_fail`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | repo-wide existence gate is driven by source-repo keyed repo-metadata authority materialized alongside the lexical generation, not by runtime catalog document metadata. Current executable subset is exactly one `key:value` filter argument with lowercase-normalized key and exact value semantics. Key-only shapes remain outside the admitted subset and must typed-fail until distinct authority/proof exists |
| `Predicate repo.has.topic(...)` | `quanta-index-contract` IPC round-trip tests (`repo_topic_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_repo_topic`, `search_plane_ingest_response_envelope_round_trip_repo_topic_receipt`), `quanta-index-sdk` (`history_publish_repo_topic_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `repo_topic_arg_accepts_one_textual_topic`, `repo_topic_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`repo_has_topic_predicate_executes_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`repo_has_topic_predicate_parity`), `sdk_frontdoor` (`sourcegraph_repo_has_topic_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | repo-wide existence gate is driven by source-repo keyed repo-topic authority materialized alongside the lexical generation. Admitted subset is exactly one textual topic scalar with lowercase exact topic-set semantics. Matching repos return the lexical hits already selected by the surrounding query; key/value repo metadata and repo topic are distinct authorities and must not be conflated |
| `Predicate repo.has.description(...)` | `quanta-index-contract` IPC round-trip tests (`search_plane_ingest_request_envelope_round_trip_repo_description`, `search_plane_ingest_response_envelope_round_trip_repo_description_receipt`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`), `e2e_filter_execution` (`repo_has_description_predicate_executes_on_sourcegraph_surface`, `repo_has_description_invalid_regex_fails_closed`, `repo_has_description_without_authority_fails_closed`) | repo-wide existence gate is driven by source-repo keyed repo-description authority materialized alongside the lexical generation (`RepoDescriptionIngestBatch` → `repo-description.cbor` shard), distinct from repo metadata and repo topics. The admitted subset is exactly one textual pattern scalar compiled as a regex and matched against each repo's verbatim description; a malformed pattern fails closed with `LEX_REGEX_*` and a missing authority fails closed with `REPO_DESCRIPTION_UNAVAILABLE`. The search-plane never reads source bytes — the producer publishes the description string |
| `Predicate file.contains(...)` | TOML ledger | native dotted predicate surface executes on the shared content scalar subset, including native alias `file.contains.content(...)` and numeric scalar canonicalization. File-scoped `file:` / `path:` / `lang:` constraints are executable at top level and conjunctive `AND`, reusing the existing file/lang substrate to narrow allowed paths. Scoped content predicates inside `OR` / `NOT` stay typed-fail with `LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED` |
| `Predicate file.has.content(...)` | TOML ledger | native dotted predicate name and SG alias share the same content-leaf substrate. Numeric scalars are admitted via the shared content contract, and `lang:`-scoped regex retains its declared runtime/parity owners. Like `file.contains(...)`, scoped forms are executable only at top level and conjunctive `AND`; scoped `OR` / `NOT` stays typed-fail |
| `Predicate file.has.owner(...)` | `quanta-index-sdk` (`history_publish_file_ownership_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `file_owner_arg_accepts_zero_or_one_textual_owner`, `file_owner_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`file_has_owner_executes_on_sourcegraph_surface`, `file_has_owner_executes_inside_boolean_scope`), `e2e_dual_syntax_lowering_parity` (`file_has_owner_predicate_parity`), `sdk_frontdoor` (`sourcegraph_file_has_owner_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | query-side file owner gate is driven by source-repo keyed file-ownership authority materialized alongside the lexical generation, not by runtime facets or repomap ownership. Admitted subset is zero args (`any owner`) or exactly one textual owner identity with lowercase exact matching. The executor narrows candidate ids directly by `(source_repo_id, repo_relative_path)` authority lookup, so same-path files in different repos do not alias |
| `Predicate file.has.contributor(...)` | `quanta-index-contract` IPC round-trip tests (`file_contributor_ingest_batch_round_trip`, `search_plane_ingest_request_envelope_round_trip_file_contributor`, `search_plane_ingest_response_envelope_round_trip_file_contributor_receipt`), `quanta-index-sdk` (`history_publish_file_contributor_routes_through_ingest_transport`), `quanta-index-lexical` (`registry_resolves_shipped_predicate_kinds`, `file_contributor_arg_accepts_one_textual_contributor`, `file_contributor_arg_rejects_bad_arity_and_non_textual_shapes`), `e2e_filter_execution` (`file_has_contributor_executes_on_sourcegraph_surface`), `e2e_dual_syntax_lowering_parity` (`file_has_contributor_predicate_parity`), `sdk_frontdoor` (`sourcegraph_file_has_contributor_positive`, `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`), `e2e_full_corpus` (`full_corpus_runtime_fixture_executes_real_rows_only`) | query-side file contributor gate is driven by source-repo keyed file-contributor authority materialized alongside the lexical generation. Admitted subset is exactly one textual contributor identity with lowercase exact matching. The executor narrows candidate ids directly by `(source_repo_id, repo_relative_path)` contributor lookup, so same-path files in different repos do not alias and repo-level author fallback is forbidden |

## Select actual proof

- `declared_runtime` in the ledger identifies a search-result proof owner;
  `declared_owner_local` identifies a companion owner. Neither is an executable
  selector or a current pass. The `proof_rail` IDs record the original owner
  mapping and include retired rails.
- `typed_fail_closed` declares parser/planner/runtime rejection, `parser_only`
  declares syntax shape, and `blocked` identifies missing direct proof. Keep them distinct.
- [runtime_rows.toml](../../crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml)
  is a frozen 242-row schema/ID inventory. Its registered
  [runtime_corpus_row_inventory_is_pinned](../../crates/quanta-index-searchd-runtime/tests/e2e_runtime_corpus_inventory.rs)
  test checks count and sorted-ID digest; it does **not** execute those queries.
  `e2e_full_corpus.rs` was retired. The registered
  [e2e_lexical_full_fidelity](../../crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs)
  rail contains six specialized rows that query a pinned old-Q process when
  selected. Other fidelity/SDK/restart/chaos rails need
  their own named test body and result before they count as current proof;
  bridge directives and `Empty` are not runtime-search results.
- `lowering.rs::structural_leaf_verdict` owns structural legality; the checker
  verifies the flat map against its independent expected verdicts.
- Resolve selected/executed nonempty tests through the current
  [test authority](../../tools/ci/test-authority.toml),
  mounted test source, Just profiles and a body-bearing result. An ID appearing
  only in this ledger or `runtime_rows.toml` is `NOT_RUN` for current-source
  execution. External metadata producer emission and qualification remain
  separately required where claimed.

The old proof/status table and repeated rail lists are recoverable at
`0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`. Per-surface machine-ledger entries are preserved; metadata points to the live ADR.
