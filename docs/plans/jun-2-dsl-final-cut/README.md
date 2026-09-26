# Jun 2 DSL Final Cut

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `closed`
Date: `2026-06-02`
Closed: `2026-06-02`
Scope: whole-DSL residue after the already-shipped runtime subset (JFC-00 through JFC-06).

This packet superseded the remaining active ownership from:

- [../may-25-lexical-enhancement/README.md](../may-25-lexical-enhancement/README.md)
- [../may-26-indexing-residue-tasks/README.md](../may-26-indexing-residue-tasks/README.md)
- [../may-27-dsl-master-closeout/README.md](../may-27-dsl-master-closeout/README.md)
- [../may-27-structural-dsl/README.md](../may-27-structural-dsl/README.md)

Historical packets stay in place as evidence. **Active whole-DSL ownership has returned to the proof inventory** ([../may-25-lexical-enhancement/dsl-proof-ledger.toml](../may-25-lexical-enhancement/dsl-proof-ledger.toml), [../may-25-lexical-enhancement/lexical-capability-matrix.md](../may-25-lexical-enhancement/lexical-capability-matrix.md)).

Post-closeout hardening on already-shipped surfaces is tracked separately in
[../jun-2-dsl-hardening/README.md](../jun-2-dsl-hardening/README.md). That
packet does not reopen this closeout verdict.

---

## 1. Scope Lock

This packet owned the remaining DSL work as one program:

- predicate surface proof and oracle closure (`JFC-01`)
- history date/window and diff-field substrate (`JFC-02`)
- runtime catalog substrate beyond `dirty:` (`JFC-03`)
- mixed lexical / structural set algebra (`JFC-04`)
- pure-negative structural root semantics (`JFC-04`)
- Sourcegraph / bridge widening after native semantics (`JFC-05`)
- bounded observability, perf, and chaos closeout (`JFC-06`)
- proof-ledger and matrix truth synchronization (`JFC-00`)

Explicitly excluded (still true after closeout):

- semantic / hybrid product-contract redesign outside existing DSL owner rails
- broad repo-map redesign unrelated to DSL query authority
- unrelated SDK UX redesign
- bridge-packet carriers misrepresented as search-result runtime rows

## 2. Current Code Truth

Current source-backed state:

- `e2e_full_corpus` closes a materially wide runtime subset:
  text/core lexical, `content:`, `type:path`, `type:repo`, `select:path`,
  `select:content`, `select:content.match`, phrase positive, timeout typed-fail,
  `patterntype:structural`, and the structural truthful subset with exact bindings
- `file.contains(...)` is now runtime-proved on the native dotted predicate surface (`runtime_native_file_contains_raw_hit`, `runtime_native_file_contains_phrase_hit`, `runtime_native_file_contains_phrase_miss`) plus the owner-local/native parity rails
  and the public SDK front-door / chaos rails (`sdk_frontdoor`, `e2e_perf_chaos`)
- `file.has.content(...)` is runtime-proved on the SG alias plus owner-local native predicate rail (`runtime_sourcegraph_file_has_content_*`, `tantivy_smoke`)
- `repo.has.file(...)` is executable only for `path:` / `name:` argument filters; on
  `docs-multi-repo.toml` the true-gate narrows by indexed `source_repo_id` (not a
  single-repo coincidence row)
- `repo:` positive allow-list is runtime-proved on `docs-multi-repo.toml` via producer-carried `ChunkRecord::source_repo_id` (`JFC-01`)
- public predicate front-door proof now shares one scenario authority across
  `dsl_scenarios` and `sdk_frontdoor`; `repo.has.file(...)` true-gate/miss and
  `file.contains(...)` hit/miss are exercised outside the runtime corpus closeout rail
- history now requires explicit `type:commit` or `type:diff`; the shipped surface executes
  `rev:`, `author:`, `committer:`, `message:`, keyword/phrase/raw `content:`,
  `before:`, `after:`, bare `since:` / `until:`, and the diff-family filters against
  `committer_time_ms` and dedicated diff hunk fields
- `file:` and `diff.*` are diff-surface-only on the history route; `type:commit file:...`
  and `type:commit diff.*:...` now fail closed with `INVALID_REQUEST` instead of empty-success
- predicate/regex/structural leaves are rejected at history/runtime-metadata validation time
  rather than surfacing as late executor drift
- qualified `since.time:` / `since.commit:` now execute on the same history authority as bare `since:` and resolve commit/ref/tag boundaries through materialized commit metadata
- history front-door proof is now split deliberately:
  `sdk_frontdoor` owns builder/transport proof for `since.time:` / `since.commit:` plus the
  new history invalid-request / predicate-leaf typed rejects, while `end_to_end` owns raw IPC
  proof for `after:` / `until:` / `diff.*` and the same fail-closed admission contract
- runtime metadata executes `dirty:yes` / `dirty:no` plus path/lang/content narrowing from a
  generation-pinned `RuntimeMetadataState` catalog (`PublishRuntimeCatalogBatch`)
- `dirty:no` now executes over the clean complement inside the pinned generation while `dirty:yes` continues to read the dirty authority directly; `dirty:only` now typed-rejects `RUNTIME_DIRTY_ONLY_UNSUPPORTED` instead of silently aliasing `dirty:yes`
- `changed:`, `stale:`, `snapshot:`, `meta.*`, `affected:`, and `invalidated_by:` execute against persisted generation-pinned runtime catalog fields;
  active proof is non-vacuous (`stale:` uses bound inversion, `snapshot:` / `meta.*`
  use shared-token decoy rows, `affected:` / `invalidated_by:` use direct positive/miss rows plus parity and chaos rails)
- runtime-catalog companion proof is now sibling-complete across public front-door,
  replay, and chaos:
  `sdk_frontdoor` covers `dirty:no` / `affected:` / `invalidated_by:`,
  `end_to_end` covers `snapshot:` / `meta.*`,
  `e2e_restart_replay_determinism` reopens `changed` / `stale` / `snapshot` /
  `meta.*` / `affected:` / `invalidated_by:` / `dirty:no`,
  and `e2e_perf_chaos` covers typed-reject/no-poison behavior for the widened families
- mixed lexical + structural boolean and pure-negative structural root execute on a
  generation-pinned candidate universe with set algebra before projection (`JFC-04`)
- Sourcegraph bridge adopts history timeref/diff filters and preserves lexical keywords on
  the structural route while quoted bodies lower to `match { ... }` (`JFC-05`)
- widened runtime/history/structural surfaces carry bounded `lq_*` metrics, typed-reject /
  recovery chaos rails (`e2e_perf_chaos`), public front-door scenario rails
  (`dsl_scenarios`, `sdk_frontdoor`, `end_to_end`), and restart persistence for
  catalog + mixed structural boolean (`e2e_restart_replay_determinism`) (`JFC-06`)
- `into:codeql`, `scope:results`, `with:lexical` remain intentional bridge-packet carriers
  (`bridge_directive_packet` + `golden_bridge`), not missing runtime rows

## 3. Ticket Lanes (all closed)

| ticket | status |
| --- | --- |
| [JFC-00](tickets/JFC-00-truth-freeze-and-scope-lock.md) | closed |
| [JFC-01](tickets/JFC-01-predicate-oracle-and-surface-closure.md) | closed |
| [JFC-02](tickets/JFC-02-history-date-and-diff-filters.md) | closed |
| [JFC-03](tickets/JFC-03-runtime-catalog-authority.md) | closed |
| [JFC-04](tickets/JFC-04-structural-set-algebra-and-negative-root.md) | closed |
| [JFC-05](tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md) | closed |
| [JFC-06](tickets/JFC-06-observability-and-chaos-closeout.md) | closed |
| [HISTORICAL-MAP](tickets/HISTORICAL-MAP.md) | reference |

## 4. Program-Level DoD (closeout verdict)

| # | criterion | verdict |
| --- | --- | --- |
| 1 | every DSL surface in `dsl.md` maps to exactly one proof status in ledger/matrix | **met** via [dsl-proof-ledger.toml](../may-25-lexical-enhancement/dsl-proof-ledger.toml) |
| 2 | every executable surface has direct proof or explicit typed fail-closed contract | **met** |
| 3 | `repo:` / `repo.has.file` oracle non-vacuous or explicitly scoped | **met** (`docs-multi-repo.toml`, `source_repo_id`) |
| 4 | history date/window and diff-field filters shipped or explicitly absent | **met** (qualified `since.*` shipped) |
| 5 | runtime catalog beyond `dirty:` has authority substrate or explicit absent claim | **met** (`affected:`/`invalidated_by:` shipped on edge-authority catalog) |
| 6 | mixed structural boolean + pure-negative root shipped or fail-closed | **met** |
| 7 | bridge carriers split from runtime search-result closeout | **met** |
| 8 | ledger, matrix, packet prose agree with code and rails | **met** (this closeout pass) |
| 9 | widened surfaces: bounded metrics + chaos/restart rails | **met** (`e2e_perf_chaos`, `e2e_restart_replay_determinism`) |

## 5. Closeout Verification (owner-local, not repo-wide hermetic)

```bash
./scripts/cargow test -p quanta-index-contract --lib chunk_record
./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke
./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge --test bridge_directive_packet --test property_translator_total
./scripts/cargow test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test end_to_end -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture
just rust-fuzz-smoke
python3 tools/ci/lint/check-public-api.py
```

## 6. Intentional Non-Runtime Carriers

- bridge-packet carriers: `into:codeql`, `scope:results`, `with:lexical`

## 7. Historical Evidence

- [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md)
- [../may-24-lexical-indexing-sourcegraph/SHIPPED.md](../may-24-lexical-indexing-sourcegraph/SHIPPED.md)
- [../may-27-dsl-master-closeout/README.md](../may-27-dsl-master-closeout/README.md)
- [../may-27-structural-dsl/README.md](../may-27-structural-dsl/README.md)
- [../may-25-lexical-enhancement/README.md](../may-25-lexical-enhancement/README.md)
