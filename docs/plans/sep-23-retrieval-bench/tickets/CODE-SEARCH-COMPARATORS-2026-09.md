# Code-search comparator plan and local inventory — 2026-09-26

Status: **two exploratory Quanta–Semble pairs completed; no qualified quality/performance result**. The original local inventory began at HEAD `b513e04237907f84d12f06e90b5d511fb1a9c61e`; the current implementation checks below ran at HEAD `6b662989b75258c6b90ebae6e881ff2f0886e58b` with a dirty shared worktree. The [RB test plan](TEST-PLAN.md) and [RBR profile contract](../../sep-26-retrieval-remediation/tickets/PROFILE-CONTRACT.md) govern qualification; this document does not weaken either. These are dirty-source implementation checks, not clean-source receipts.

## Where Semble is

The benchmark's Semble is an **external Python 3.13 virtual environment**, not vendored source in `quanta-index` and not the old per-pair `rep-00/semble` output directory.

| Item | Observed local value | Evidence/status |
| --- | --- | --- |
| Interpreter | `/Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/semble-venv/bin/python` | `VERIFIED`: exists; points to local CPython 3.13 arm64. |
| Installed package | `/Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/semble-venv/lib/python3.13/site-packages/semble/` | `VERIFIED`: `importlib.metadata.version("semble") == "0.6.0"`; module resolves inside this venv. |
| Dependency pins | `/Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/semble-env-pins.txt` | `VERIFIED`: SHA-256 `e75b964efae590d5f10797885f9892a8f78460febc96e5570da85f2237505aac`; contains `semble==0.6.0`. A version-pinned distribution set is not a per-wheel archive hash. |
| Semble cache root | `/Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/cache` | Used by the four profile captures below; profile output binds model-asset digest. |
| Model revision | `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b` | Recorded in the earlier external pair spec; **not** a new model parity or asset-digest proof. |
| In-repo adapter | [`tools/benchmark/retrieval/semble.py`](../../../../tools/benchmark/retrieval/semble.py) | Adapter/worker, not the Semble installation. |

The external frozen **candidate** corpus is `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-v4/corpus-set.json` (observed SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33`). It covers 10 repositories, five languages and 1,480 admitted code files. Checkouts, per-repository manifests and previous raw pairs stay under that external corpus root. The [baseline record](CODE-SEARCH-BASELINE-2026-09.md) explains their diagnostic scope and observed query-form effects. These absolute paths are this host's inventory; runners must receive explicit external paths rather than assuming that all hosts share them.

## Comparison matrix

| Track | Candidate | Comparator | Claim boundary |
| --- | --- | --- | --- |
| Primary code-search product | Quanta `lexical` | Sourcegraph `patternType:keyword` at pinned repo revision | Same exact user query text and admitted source bytes. Sourcegraph is a product comparator, not a Tantivy-equivalent scoring algorithm. |
| Lexical lane control | Quanta `lexical` | Semble `lexical-only` | Requires a real pinned Semble single-lane capture and proof that dense/encode calls are zero. Semble `native-default` is **not** lexical-only. |
| Hybrid product | Quanta `hybrid` | Semble `native-default` | Preserve Semble's automatic alpha and reranking; report actual alpha and native chunking. Different model implementations cannot be called model-matched without T15. |
| Secondary code-search checks | Same frozen Quanta route | OpenGrok and codespelunker (`cs`), individually | Add only after the primary track's corpus/export contract works. Neither replaces Sourcegraph or Semble. |

Sourcegraph's Zoekt backend is part of its product path, not another independent product row. Do not substitute generic document BM25 engines for code-search comparators. Product query language and ranking differ: keep a strict identical-user-text lane and, if useful, a separately named product-native syntax lane. Never pool the two. Official Sourcegraph documentation describes [serving local Git repositories with `src serve-git`](https://sourcegraph.com/docs/admin/code-hosts/src-serve-git) and [revision/keyword search filters](https://sourcegraph.com/docs/code-search/queries). Pin the actual deployment/CLI image digests and test their behavior; documentation is not evidence that a local deployment succeeded.

## Execution update — 2026-09-26

### Semble profile capture (development diagnostic)

All four Semble 0.6.0 profiles were captured against the external GIN projection (99 files, 20 existing mechanically generated tasks) under `/Users/songmin/Documents/code-new/qi-rb-rbr03-2026-09-26-gin-rerun-01/`. The adapter was corrected to trace actual rerank activation and lane calls, then all profiles were rerun. All four captures mapped 99/99 admitted paths with no skipped, extra or mismatched files. Shared mapping-proof SHA-256: `1c3386d4524106205aaacd674d41cddc58bf5d67464a04d16de09eeab7cfb645`.

| Profile | Rerank observed | Lanes observed (BM25 / semantic / encode) | Native artifact SHA-256 |
| --- | --- | --- | --- |
| `native-default` | true | 40 / 40 / 40 | `d926897fc3fb3ec4907ff0fcef00d2310423c3de486a7288f5ebe1f5a98deb26` |
| `hybrid-no-rerank` | false | 40 / 40 / 40 | `fcdb427a351fb7600d6acc4e4fc4c92212960630678618fec7350bf34314968c` |
| `lexical-only` | false | 40 / 0 / 0 | `38ce54ca753a9610d8ab28c5a5427020d59356cb81ed2baade53f0cc4ec37185` |
| `semantic-only` | false | 0 / 40 / 40 | `a2b4f0ac306dd71ece791e752292863d76edde66a6c37e3baa45e7aff67f1c22` |

The runs establish profile activation and raw capture only. The task pack is not independent gold, so `QUALITY_DELTA=NOT_RUN`; the host was not qualified quiet, so `PERF_QUALIFIED=NOT_RUN`. No fresh Quanta-vs-Semble pair or `PAIR_VALID` verdict was produced for this GIN set. Model/pin bindings in these captures are: lockfile SHA-256 `e75b964efae590d5f10797885f9892a8f78460febc96e5570da85f2237505aac`, model-asset SHA-256 `ea909b7defe7804ce18bf003ef60a437b54782541819ab6b7004c36fd9eea5d0`, worker SHA-256 `7424db07a5bc9f064271f42407602be500a66443f8525fed132a7e24b2e8e895`. Captures bind the earlier dirty adapter SHA-256 `b83ed87ee6e58d49bd8f6dd4856a81a6b3f531c69dadb3f154125d127c561dfd`; the subsequent audit hardening below makes them source-stale for current-code replay. Retain them as historical external diagnostics, not current fixtures.

### Sourcegraph setup boundary

Sourcegraph 6.8.0 is running locally in Docker at `127.0.0.1:7080` (amd64 emulation; image digest `sha256:60103ca14929dfc9d23f149658a5d4fff4cd12d1687cdd5ea7ba3ae2e44569df`). The external `src serve-git` projection is at `127.0.0.1:3434`; the GIN repository listing was observed. Projection checkout: `/Users/songmin/Documents/code-new/qi-rb-sourcegraph-2026-09-26/repos/gin`, commit `97364a98bb1793b12aae05032716988b10b9c871`; original GIN commit `d3ffc998…` is recorded in the external receipt set `/Users/songmin/Documents/code-new/qi-rb-sourcegraph-2026-09-26/receipts/`.

The server is private mode; unauthenticated search returned HTTP 401. CLI bootstrap created a local admin credential under that external receipts directory (mode 0600), but no browser Terms of Service acceptance was made. No repository was registered/indexed, no authenticated search was run, and there are no Sourcegraph result rows. **Do not treat Sourcegraph as installed-and-queried or as an oracle yet.** Continuing authenticated product use requires the operator's explicit acceptance decision. No token is documented here.

An offline Stream API V3 capture parser now exists in `tools/benchmark/retrieval/sourcegraph.py`, with 12 adversarial tests wired into the retrieval contract test rail. It binds exact query/repo/revision, raw response bytes and declared indexed-universe evidence; parser outputs are always `diagnostic_unqualified` and treat stream order as observed order only. The canonical request uses `type:file` and `patternType:keyword`; it rejects boolean operators, negative terms, regex syntax and other query syntax instead of silently changing the user query. This deliberately limits task coverage and must be reported for any Sourcegraph comparison. It cannot prove that the server indexed every admitted file. Actual Sourcegraph retrieval remains `NOT_RUN`.

The shared pair driver now consumes pair-spec v2, binds the per-system `execution_profiles` plus digest in protocol-lock v2, and emits runner records v5 with a canonical profile and digest on each capture. Verdict replay checks profile identity and retains v3/v4 runner records only for historical inspection/replay, not current merges. The diagnostic sidecar is v3 and validates contribution lanes against executed lanes, including the documented `hybrid.` trace prefix. These implementation changes do not qualify a real product pair. Sourcegraph/OpenGrok emulation on this arm64 host is diagnostic for latency; qualified cross-product speed still needs the same quiet native host and pinned deployment profile.

### Follow-up code audit — 2026-09-26

- Sourcegraph keyword queries previously accepted `foo OR bar` as plain user text, although `OR` is an operator in Sourcegraph's [query language](https://sourcegraph.com/docs/code-search/queries). The parser now refuses operator/regex forms, limits results to file content, and rejects malformed line matches, missing or regressed progress counts, and incomplete final counts. The source remains an offline diagnostic adapter; no indexed-universe or authenticated-request authority was added.
- The Semble worker validator previously accepted an aggregate BM25/semantic call count inconsistent with its per-query events, and accepted inconsistent single-lane candidate depths or out-of-range observed alpha. It now checks those relationships before emitting a normalized record. The earlier four profile captures predate this validator; the current-worker rerun below supersedes them for local profile diagnostics.

The current adapter (`semble.py` SHA-256 `f6d54dac8e78799fd90c075aaef8156f8a165dcf7823cb7474d8713ae4127bdf`) and worker (`worker.py` SHA-256 `c6e3a98fdbf5e5421629dbbcff2708c836f00d3330e2f97bd13be0156dc59ca6`) completed all four profiles against the same clean GIN commit, 99-file manifest and 20-task query pack under `/Users/songmin/Documents/code-new/qi-rb-rbr03-audit-2026-09-26-gin-01/`. Every run reported 40 execution events and 99 admitted/99 observed paths with zero skipped, extra or mismatched files; mapping diff digest `1e1ec19411463eca9e0b8185793110af40f582c037b42e5e68da16db4e7f872d`. The adapter checked per-event lane depth/count consistency before writing each record. The external artifacts contain `native.json`, `record.json`, `mapping-proof.json`, and `adapter-manifest.json` for each profile.

| Current-worker profile | BM25 / semantic / encode calls | Rerank | `native.json` SHA-256 |
| --- | --- | --- | --- |
| `native-default` | 40 / 40 / 40 | true | `2060b4468388840ef0d56baa292ef5959c7d6e771eaf7de0f989eabc15a3425b` |
| `hybrid-no-rerank` | 40 / 40 / 40 | false | `e81ed67c2c55e9eaa69639456916f054e08b0c88ad131f7308a9dc9feac90f38` |
| `lexical-only` | 40 / 0 / 0 | false | `e5e8bb923b7b2dce2c9e9fc96c51bdef74d23ed14ba3bd1ae66ce8d3d0975411` |
| `semantic-only` | 0 / 40 / 40 | false | `c99b910db6fe855023553c9c0a7dee6c0e730b470ad355ae0b3aaee587b6a07c` |

This is a new development diagnostic, not a paired Quanta result or an admitted W0-B run. It used one warmup pass and one measured repetition on a non-qualified host; its timings cannot establish `PERF_QUALIFIED`, and its mechanically generated tasks cannot establish `QUALITY_DELTA`.

Exact native-default command (other three runs changed `--output-root`, `--run-id`, and `--semble-profile` to the table's mode; `hybrid-no-rerank` also supplied `--alpha 0.5`):

```sh
uv run --locked --extra dev python tools/benchmark/retrieval/semble.py run \
  --repo /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/checkouts/gin \
  --manifest /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/pairs-bare-lexical-frozen/gin/corpus-manifest.json \
  --query-pack /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/symbol-route-ablation-inputs-01/gin/query-pack.json \
  --top-k 10 --python /Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/semble-venv/bin/python \
  --lockfile /Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/semble-env-pins.txt \
  --lockfile-sha256 e75b964efae590d5f10797885f9892a8f78460febc96e5570da85f2237505aac \
  --cache-root /Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/cache \
  --output-root /Users/songmin/Documents/code-new/qi-rb-rbr03-audit-2026-09-26-gin-01/native-default \
  --model-id minishlab/potion-code-16M-v2 --model-revision e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b \
  --route semble-hybrid --run-id audit-native-default-20260926 --blinding attested \
  --isolation-method audit-process-only --access-block-log no-gold-path-passed \
  --seed 42 --warmup-passes 1 --repetitions 1 --semble-profile native-default --timeout-secs 1800
```

### Current implementation checks

- Source at verification: HEAD `6b662989b75258c6b90ebae6e881ff2f0886e58b` plus a dirty shared worktree; no clean-source closure/receipt was issued. Re-freeze before using these as source-bound qualification evidence.
- `uv run --locked --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_retrieval_contract_proof.py -q`: **287 passed, 32 subtests passed** (2026-09-26, after the actual-pair validator fixes). Includes the 12 Sourcegraph parser tests, Semble per-event inconsistency mutants, runner-v5 profile/merge tests, pair/verdict replay tests, and retrieval proof-helper tests.
- `./scripts/cargow --lane test-daemon-lane test -p quanta-index-retrieval-bench --lib --test chunking_contract --all-features --locked`: **72 library + 25 chunk-contract tests passed** after the hybrid lane-prefix regression fix.
- `QUANTA_INDEX_SEARCHD_BIN=<pinned target>/debug/quanta-index-searchd ./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked --success-output final`: **14 SDK roundtrip tests passed**. The explicitly built/pinned daemon was required; an initial unpinned invocation refused, as designed. The actual-runner probe also exposed and fixed a required `--refusal-out` fixture omission and the `hybrid.lexical`/`hybrid.dense` trace-name mismatch.
- RBR-07 full-vector parity: generated **9 × 256** vectors with the pinned Semble venv/model2vec 0.9.0 into external fixture `/Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/rbr07-full-vector-reference.json` (SHA-256 `1de240e14b83c0a4ef47f4122b17a5be90c06610efc4117ee8fa2b3ea503af84`). Exact model weights/tokenizer/config hashes matched Rust pins (`75cf7a6c…`, `107bbdcb…`, `148e5691…`); the ignored Rust parity test was explicitly enabled and **1 passed** against that fixture. No model or fixture was copied into the repository.
- `git diff --check`, Python compileall, JSON parsing of runner/pair/run-manifest schemas, and scoped `rustfmt --edition 2024 --check` over the touched retrieval Rust files: **passed** after documentation and formatting updates.
- Workspace-wide `cargo fmt --all -- --check`: **fails only on unrelated untracked** `crates/quanta-index-semantic/tests/exact_ann_decomposition.rs`; that file is outside this retrieval task and was left untouched.
- Follow-up static checks: `uv run --locked --extra dev ruff check` over the touched Sourcegraph/Semble/benchmark test modules, scoped `ruff format --check` over the two new Sourcegraph modules, `git diff --check`, and `python3 tools/prompt-manager/pm.py lint`: **passed**. The existing large Semble and benchmark test modules are not globally Ruff-formatted; the scoped format claim applies only to the two new Sourcegraph modules.
- `python3 tools/ci/source_closure.py check --profile retrieval`: **FAILED as designed** (`refusing dirty relevant source`, exit 1). This is a clean-source qualification boundary, not a failed functional test. Do not issue current-source proof receipts from this checkout state.

These are local dirty-tree implementation checks, not RB qualification. They are not clean-source receipts and do not prove merge/deployment/activation. No W0-B admission, licensed Sourcegraph query, independent quality result, or qualified speed result was issued.

### Actual exploratory pairs — Gin and ripgrep, 2026-09-26

The current Rust runner (`sha256:54dd4e5ecac1cd520650c19cfd7edfc7564541b5b3df11e72d25fad6209a9bb6`) and searchd (`sha256:2d937be35339bc7a240b66ba629728e61efa15f0d7b0319d125515b04b088782`) were built at HEAD `6b662989b75258c6b90ebae6e881ff2f0886e58b` with dirty relevant source. The external Semble venv was pinned to 0.6.0, dependency-lock SHA-256 `e75b964efae590d5f10797885f9892a8f78460febc96e5570da85f2237505aac`, model revision `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`. Quanta used `natural_language`/UCD17-v2; Semble used `native-default` with upstream content rerank. Both consumed each repository's same frozen query pack and corpus manifest; their execution profiles are **not algorithmically identical**. Both pairs used one fresh root, one warmup pass and one measured repetition, `top_k=10`, `fixed_window_strict` 1024-byte windows/256-byte overlap, with lexical, semantic and hybrid Quanta routes. The 20 tasks per repo are mechanically generated symbol-definition lookups with **unreviewed** gold, not an independent holdout.

Commands: `uv run --locked --extra dev python tools/benchmark/retrieval/run.py pair --spec /Users/songmin/Documents/code-new/qi-rb-runtime-08a6ced3/pair-spec-audit-gin-v2.json` and the analogous `pair-spec-audit-ripgrep-v2.json`. Failed Gin staging roots `/private/tmp/g26.staging` and `/private/tmp/g27.staging` are retained for failure forensics. They exposed two real driver mismatches: v3 contribution lane `lexical` was not reconciled with executed `hybrid.lexical`, and the pair/verdict driver assumed a Quanta record had one capture although the runner emits one per route. The validator now accepts only the exact or `hybrid.`-qualified lane and checks all capture identities agree on system and strategy; malformed-namespace and mixed-identity regression tests were added. Neither failed staging root was promoted or scored.

| Repo / commit / manifest | Pair result | Semble hybrid | Quanta hybrid | Quanta lexical | Quanta semantic |
| --- | --- | --- | --- | --- | --- |
| Gin `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`; 99 files | `/private/tmp/g28`; `PAIR_VALID=pass`; 80/80 attempted, 0 errors | Recall@10 1.00; NDCG@10 0.07758; mean query 7.76 ms | 0.75; 0.02927; 76.05 ms | 0.35; 0.00940; 16.06 ms | 0.60; 0.02575; 27.40 ms |
| ripgrep `af60c2de9d85e7f3d81c78601669468cf02dabab`; 88 files | `/private/tmp/r28`; `PAIR_VALID=pass`; 80/80 attempted, 0 errors | Recall@10 0.60; NDCG@10 0.03279; mean query 1.38 ms | 0.25; 0.01039; 72.90 ms | 0.10; 0.00195; 21.49 ms | 0.25; 0.00482; 14.42 ms |

The table's retrieval figures are the report's **chunk** rank metrics and route mean query latency, not an end-to-end user latency or qualified performance comparison. Gin's Quanta hybrid minus Semble NDCG@10 is `-0.04831`; ripgrep's is `-0.02240`. For these tasks, the observed Quanta hybrid is slower and retrieves fewer gold spans. The same 20 task IDs and **original query hashes** were used by both systems per repository (sorted task/hash digest Gin `15be77d49c167c1454251be9b4a181cb7b0781fba477059026cb9758f54629c6`, ripgrep `9f10f561d8c9fb2f12cab1ad8834bbd8e984200a727f5f31a5cd9f609802416d`). Quanta's effective lexical request is intentionally transformed by its natural-language query planner, while Semble submits the original text; this is a product-policy difference, not a different user query. The report labels all Quanta rows `capped` because top-10 was filled; Semble rows are `success`. Neither is a process error. Both pairs' manifest replay with `run.py verdict --repo <frozen checkout> --suite <frozen suite> --run-manifest <pair>/run-manifest.json --out <external replay file>` reproduced the promoted verdict **byte-for-byte**. Gin `run-manifest.json` SHA-256 `8734ea86d675b572c432953b97ed718b738580d19fc52f4761bfd67649d0929f`, verdict SHA-256 `b5bca0a2999a90e81ecf5de3d426dbd5bb8496fb4f5cc0b7376b6618addaa9bf`; ripgrep manifest `e8fb45d3881d37182b153f70f3f7927fffb01d10e494ad5540da00008f9b731b`, verdict `fb97056e821be81e438826df8c78a5a5fd4fa108f5d9c3c2cf164ba18e948900`.

Verdict states for **each** pair: `PAIR_VALID=pass`, `CONTRACT_GREEN=not_run`, `SDK_PATH_GREEN=not_run`, `QUALITY_DELTA=not_applicable`, `PERF_QUALIFIED=not_applicable`. This is a working end-to-end **exploratory** benchmark, not proof that the quality or speed target is met. Different profile semantics, unreviewed mechanical gold, two of ten repositories, one measured observation per task, dirty source and an unqualified host all limit inference. The next useful diagnosis is a paired per-task miss analysis (definition span vs top-10 returned spans, lexical tokenization, hybrid fusion and rerank, query-plan overhead) before tuning, followed by independent adjudicated holdout and qualified speed protocol. Do not tune on these 40 tasks and then report them as holdout performance.

## Ordered work and exit criteria

1. **Freeze inputs before inspecting results.** Recheck the 10 external clean Git checkouts, per-repository commit and sorted path/SHA manifests, license/attribution admission, exclusions and language coverage. Preserve old 200 mechanically generated symbol tasks as a **development diagnostic**, not the independent quality holdout. Predeclare query strata: bare symbol, declaration/reference disambiguation, phrase/error text, path-aware lookup, semantic behavior, architecture/flow and genuine no-answer. Keep query form and repo/language strata visible.
2. **Carry Semble RBR-03 into an admitted pair.** The four current-worker diagnostic profiles and their package/model/worker identities are recorded above and reflected in the [RBR-03 index](../../sep-26-retrieval-remediation/tickets/INDEX.md). Re-run the selected frozen profile under the pair driver after W0-B admission; prove the same query schedule and profile across cold, warmup and measured phases and replay the promoted pair. The old clean-source receipt remains source-stale, and these diagnostics are not a qualified pair.
3. **Index and query Sourcegraph after operator acceptance.** The local server and `src serve-git` projection exist, but authenticated product use requires the operator's Terms decision. Then register the pinned local repository, query each fixed revision with `type:file patternType:keyword`, explicit result limits and truncation/timeout detection. Prove indexed path+SHA equality to the admitted manifest, not merely equal file counts. A mismatched or partially indexed repository is refused, not scored as zero relevance.
4. **Add external adapter and independent oracle.** Preserve raw API/CLI output, errors, result order, repo/revision/path, line/span and query identity; normalize without reranking. Prove that the selected endpoint supplies a stable, complete ranked result sequence; otherwise restrict that product to an explicitly unordered coverage diagnostic. Deterministic fixtures must reject missing, duplicate, stale, malformed, reordered or truncated records. Use separately adjudicated, blinded gold frozen **before** product results are viewed. Do not synthesize license approval or two independent judgments. Compare file-level quality across every supported product; score span/byte-aware quality only where native output can be mapped and verified. Report the two units separately.
5. **Run in increasing scope.** Two-repository exploratory smoke is now complete above → all-10-repository exploratory matrix → independent untouched holdout quality run → quiet-host performance run. Report per-repo/language/query-stratum NDCG/Recall/MRR as eligible, no-answer behavior and candidate depth. A metric whose output unit or gold is unavailable is `NOT_APPLICABLE` or `NOT_RUN`, not zero. Retain errors, exclusions and paired query IDs. Never blend the old tuned symbol diagnostic into the holdout result.
6. **Qualify speed separately.** Measure total time-to-searchable and phase costs apart from warm API query latency; record cache regime, native architecture, process tree RSS, index bytes, errors/timeouts and order alternation. The RB test plan requires at least 20 distinct tasks, five fresh roots and 1,000 valid warm observations per eligible route on a quiet same host. Emulated or contended local runs remain diagnostic. Replay final artifacts after atomic promotion and issue exact-source proof receipts only after code/docs settle.

Required terminal outputs are a frozen corpus/query/gold admission record, per-product configuration and binary/model digests, indexed-universe proofs, raw captures, adapter/evaluator tests, replayable verdicts, and a claim table with `PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` stated independently. A successful install, index, smoke, or exploratory `PAIR_VALID` alone is not a quality or performance qualification.
