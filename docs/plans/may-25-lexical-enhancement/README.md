# May 25 Lexical Enhancement Closeout

Status: `proof-accounted`
Date: `2026-06-02`
Scope: LQ DSL, Sourcegraph lowering, lexical/history/structural/runtime query rails, and bridge directive companion rails

Whole-DSL execution ownership closed in
[../jun-2-dsl-final-cut/README.md](../jun-2-dsl-final-cut/README.md) (`closed` 2026-06-02).
This packet remains the authoritative proof inventory and capability matrix for
all accounted surfaces.

이 pack은 더 이상 "전부 completed"로 닫지 않는다. 현재 목표는 `docs/plans/may-24-lexical-indexing-sourcegraph/dsl.md`의 각 표면이 정확히 하나의 proof state를 가지게 하는 것이다.

## Current Contract

- authoritative proof inventory:
  [lexical-capability-matrix.md](lexical-capability-matrix.md)
- machine-readable proof inventory:
  [dsl-proof-ledger.toml](dsl-proof-ledger.toml)
- primary runtime closeout rail:
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- companion rails:
  `dsl_scenarios.rs`
  `e2e_lexical_full_fidelity.rs`
  `e2e_dual_syntax_lowering_parity.rs`
  `sdk_frontdoor.rs`
  `end_to_end.rs`
  `e2e_perf_chaos.rs`
  `e2e_restart_replay_determinism.rs`
  `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`

## What Changed In This Closeout Pass

- `runtime_rows.toml`를 executable query inventory로 확장했다.
  text, history, structural, runtime-metadata row가 한 파일에서 닫힌다.
- public front-door scenario authority를 추가했다.
  `tests/common/frontdoor_scenarios.rs`가 widened predicate/history/runtime
  surface를 한 번 정의하고 `dsl_scenarios`, `sdk_frontdoor`, `end_to_end`
  companion rails가 이를 공유한다.
- `e2e_full_corpus.rs`가 더 이상 set-like 비교를 하지 않는다.
  exact ordered ids, duplicate-free success, typed runtime error, provenance contract를 강제한다.
- low-signal runtime row를 걷어냈다.
  single-repo fixture에서 base query와 구분되지 않던 `repo` positive, `repo.has.file` positive는 closeout inventory에서 내렸다.
- code-first audit로 predicate truth를 다시 맞췄다.
  `file.contains(...)`는 native runtime row + owner-local tantivy rail로 승격했고, `file.has.content(...)`는 SG alias runtime row + owner-local tantivy rail로 닫았다 (`jun-2` / `JFC-01` closeout).
- closeout fixture를 same-path multi-chunk shape까지 확장했다.
  `docs-select-file.toml`과 candidate-id 기반 mapping으로 `count:all` full recall과 `select:file` per-path collapse를 non-vacuous runtime rail로 승격했다.
- runtime row schema를 path/snippet/binding assertion까지 올렸다.
  `quanta-index-corpus-smoke` loader가 `expected_paths`, `expected_snippets`, `expected_bindings` 계약을 직접 검증하고,
  `e2e_full_corpus.rs`는 text/runtime-metadata path+snippet, structural binding까지 exact ordered assertion을 건다.
- `content:`, `type:repo`, `type:path`, `select:path`, `select:content`, `select:content.match`를 runtime closeout rail로 승격했다.
  `type:repo`와 `type:path`는 lexical/core owner seam에서 deterministic representative collapse를 열고, runtime corpus에서 exact ids/paths/snippets로 닫았다.
- `Phrase`, `patterntype:structural`, `timeout`, structural subset(`root capture`, `where`, `inside`, `outside`, named/anonymous/variadic holes, typed holes`)을 runtime closeout rail로 승격했다.
  structural fixture는 fixed synthetic helper 대신 generic `structural_tree` carrier로 올렸고, runtime rail은 exact candidate ids와 binding spans를 같이 assert한다.
- `e2e_lexical_full_fidelity.rs`를 specialized regression rail로 축소했다.
  broad filter/type/select inventory duplicate는 `runtime_rows.toml`에만 남기고,
  path leakage, repo predicate under `OR`, phrase adjacency, default casefold, trigram false-positive rejection만 유지한다.
- `e2e_lexical_full_fidelity.rs`와 `e2e_dual_syntax_lowering_parity.rs`의 `sort + dedup` 허위-green을 제거했다.
- lexical text front door가 raw searcher 순서를 그대로 노출하지 않도록
  `crates/quanta-index-search-plane/src/query_dispatcher.rs`에서 결과 안정화 정렬을 적용했다.
- `lexical-capability-matrix.md`를 `executed` 중심 문서에서
  `active runtime / active owner-local / typed fail-closed / parser_only / blocked`
  기준 proof ledger로 재작성했다.
- companion E2E breadth를 widened surface family 기준으로 다시 맞췄다.
  `sdk_frontdoor`는 builder/transport proof, `end_to_end`는 raw IPC proof,
  `e2e_restart_replay_determinism`은 runtime-catalog sibling replay proof,
  `e2e_perf_chaos`는 family-complete no-poison/metric proof를 맡는다.

## Current Proof Split (post jun-2 closeout)

- `active runtime`
  lexical keyword/raw/regex/content; `repo:` allow-list (`source_repo_id` multi-repo oracle);
  `repo.has.file` existence gate; `file.contains` native predicate; `file.has.content` SG alias; file/path/lang/case/count;
  history commit/diff + date/window + qualified `since.time` / `since.commit` + diff-field filters; runtime catalog (`dirty`, `changed`, `stale`, `snapshot`, `meta.*`, `affected`, `invalidated_by`);
  phrase, timeout; structural truthful subset; mixed lexical/structural boolean; pure-negative structural root
- `public/replay/chaos breadth`
  shipped widened predicate/history/runtime surfaces are no longer runtime-corpus-only:
  `dsl_scenarios`, `sdk_frontdoor`, `end_to_end`, `e2e_perf_chaos`, and
  `e2e_restart_replay_determinism` provide companion proof for transport, replay,
  and no-poison behavior
- `active owner-local`
  bridge directives (`into:codeql`, `scope:results`, `with:lexical`); specialized regression rails (`e2e_lexical_full_fidelity`, etc.)
- `typed fail-closed`
  unsupported predicate extensions
- `carrier split`
  bridge-packet carriers are not runtime search-result rows; see `dsl-proof-ledger.toml` `carrier_kind`

## Closeout Rule

- `implemented subset green`과 `full spec accounting complete`는 다른 주장이다.
- 현재 이 pack은 spec accounting은 완료 대상으로 관리하지만, 각 표면의 live status는 matrix에 적힌 그대로다.
- workspace-wide green claim은 하지 않는다. 이 문서는 owner-local DSL closeout truth만 다룬다.
