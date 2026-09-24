# SEP-23 Retrieval Benchmark — Test and Qualification Plan

Status: `implementation-surfaces-present / qualification-gates-failed`. T00–T14 have implementation or contract-test surfaces, but adversarial replay has confirmed blocking gaps in promotion, protocol pins, test inventory and performance eligibility. The real pilot, valid final receipts and terminal verdict remain absent. T15/T16 stay conditional. This document does not convert focused test output into benchmark evidence.

## 0. Current execution boundary (2026-09-24)

Latest observed source: `20bd03fc98b86ee4bfb3a8a808e940ba6e090a09`. The retrieval implementation is byte-identical to the separately probed `93b736cd0f18ab698f5a12e23a2d09677a3d869d` source. At that source, generated adversarial fixtures showed: unchanged staged files fail `PAIR_VALID` after directory promotion; seven independently mutated protocol pins still pass; one pass plus 159 skips, an announced 99-test nextest suite with one terminal test, and a duplicated JUnit case pass contract proof; two tasks repeated to 1,000 warm observations pass `PERF_QUALIFIED`, and an impossible raw duration passes phase validation. These are verifier failures, not benchmark measurements. The current-source remediation and final proof rails are pending. See [INDEX.md](INDEX.md) for the finding-to-ticket map.

The following paragraph and table record an earlier implementation audit, not current qualification evidence:

The 2026-09-24 re-audit observed `main` at `f26796896e365a8eb66a516f959c460f792e7506` with concurrently modified relevant source. `python3 tools/ci/source_closure.py check --profile retrieval` refused that state, as required. Focused implementation rails at the same observed HEAD passed 160 Python contract tests, 31 retrieval-library tests, 20 chunking-contract tests and 22 receipt/proof-helper tests. The query-protocol authority now reconstructs the complete expected schedule from its seed and rejects a forged schedule even when the attacker recomputes its digest. These results are not source-closure-bound receipts; the earlier clean `fa17e0f14672548bf7116074b7064c7c5e455d03` admission and older dirty-checkout results are historical only. Retrieval receipts use schema v2 and embed an exact transitive source closure; relevant dirty/add/remove/HEAD drift fails closed, while unrelated shared-checkout dirt does not invalidate an otherwise exact closure.

| Surface | Current evidence | Authority limit |
| --- | --- | --- |
| Python/Rust contract | Current dirty-checkout focused rails completed with 160 Python contract tests, 31 retrieval-library tests, 20 chunking-contract tests and 22 receipt/proof-helper tests. The contract rejects forged uncertainty relationships, digest-valid but seed-invalid query protocols, cold/warm sample mixing, raw-sample/record disagreement and cold-only performance qualification. | Shared main remained dirty; this is not a source-closure-bound receipt, and fixtures are not a real pair. |
| Rust SDK path | The registered local rail covers owner-library and chunking contracts but does not issue the separate live `retrieval-sdk-proof` receipt. Previous live SDK coverage is not promoted to current-source authority here. | No final source-closure-bound SDK receipt was identified in the repository. The prior clean `fa17e0f` source check is not that receipt; this audit's subsequent edits require a fresh closure. |
| Test registration | Both Rust integration targets are catalogued. The registered SDK proof builds and exports an exact searchd pin. | Full post-change workspace nextest is not yet recorded. A package-only invocation without the recipe pin correctly fails closed. |
| Static policy | Semgrep: 0 findings. Derive allowlist: pass across `crates/` and `benchmarks/`; strict manual manifest decoders reject duplicate fields. | Policy GREEN is implementation evidence, not benchmark qualification. |

Current hard frontiers in code:

- The macOS pair driver now supports enforced `blinding=isolated`: the suite roots and the entire original checkout are denied, while both runners receive only a Git-free exact materialization of the admitted path/SHA universe. The proof, materialized corpus, records and process-resource artifacts are digest-bound and independently rechecked by the verdict. No non-macOS backend or real isolated pair receipt exists.
- The driver generates one digest-bound protocol per fresh root, and both runners echo cold probes, warmup/measurement permutations and raw warm samples. Capture preflight enforces 20 tasks, five roots, one Quanta route, warmup and 1,000 observations. Replay does not yet enforce the complete entry policy or phase/sample containment, so `PERF_QUALIFIED` is blocked by F4 until both paths use one authority.
- Receipt v2 binds role-tagged raw JUnit/nextest/runner artifacts and the verdict reparses them. It currently lacks a bound, complete required test identity inventory; internally consistent but partial or duplicate raw evidence can satisfy `CONTRACT_GREEN` (F3). A summary or count cannot substitute for the inventory.
- Qualified quality requires deterministic paired category-stratified bootstrap evidence plus category/language/repository summaries and no-answer abstention evidence. Counts, win/loss/tie totals, CI means, stratum counts and weighted means are cross-checked rather than accepted by shape alone.
- No committed/frozen W0-B pilot suite, real Semble pair, paired `run-manifest.json`, or terminal `verdict.json` was found in the repository. External artifacts were not supplied for this audit; this is not a claim about every host path.
- T15 and T16 are correctly conditional, but no real model-parity or incremental evidence artifact has been issued.
- The CI authority catalog assigns `sdk_roundtrip.rs` to generic workspace nextest. Its daemon prerequisite is handled by `cargow`; the full rail must be rerun after this working tree is committed and frozen.

## 1. Claims and evidence classes

| Claim | Required evidence | Must not be inferred from |
| --- | --- | --- |
| `CONTRACT_GREEN` | Frozen-data/schema/chunker unit and mutation tests | Documentation or a compile |
| `SDK_PATH_GREEN` | Separate searchd process; actual SDK publish, sealed receipt, activation and SDK query on pinned files | Direct IPC or `E2eRuntime::ingest_text*` fixture |
| `PAIR_VALID` | Both real runners, identical admitted file/query universe, source/model/provenance, complete records | Upstream Semble README number or one-sided run |
| `PERF_QUALIFIED` | Quiet same-host repetitions, phase boundaries, raw samples, resource and error accounting | Contended/debug/one-shot timing |
| `QUALITY_DELTA` | Independent labels, frozen eval set and paired score with uncertainty/strata | Pilot smoke, train-tuned labels or absent model route |

These states are separate. A 20-query pilot is labeled **exploratory-only**: it can prove the pipeline and expose regressions but cannot justify an all-language or production “beats Semble” statement. `Hash`/`hash-dev` only checks plumbing. The `potion-code` profile is implemented; admission still requires an exact built source revision and a verified local model snapshot, not mere source presence.

`PAIR_VALID` asserts paired protocol/universe validity (same commit, files, queries, `top_k`, output-unit policy, host, complete records). A blinded `QUALITY_DELTA` additionally requires proven runner blinding. Blinding is recorded as `blinding: isolated | attested`, plus `isolation_method` (how the runner was prevented from reading gold, e.g. separate suite access, path/permission denial, process sandbox) and an `access_block_log` (what was blocked/verified, or why only attestation holds). An `attested` run keeps `gold_access: false` as an attestation only and cannot claim an isolated quality verdict. T15 gates only the same-model claim, never `PAIR_VALID` itself. T16 is required only when an incremental-update claim is made.

W0-A comparison-contract freeze (only accepted artifact schema v3): the suite, the blinded query pack and every runner record carry an identical `comparison_contract` (`top_k`, `tokenizer`, `tokenizer_budget_version`, `output_unit_policy`); any mismatch refuses merge/score. Records preserve per-system raw captures (`captures{capture_id: ...}`: chunk strategy/config, runner binary identity, searchd binary identity, generation, receipt/activation digests, model identity) and each route references one `capture_id`. Unknown latency is `null`, never `0`; a `0` asserts an actually measured zero, and any speed claim with `null` on an eligible row fails `PERF_QUALIFIED`. V1/v2 or unknown artifacts are rejected without migration reads. Frozen chunk strategies: `whole_file`, `fixed_window_strict`, `fixed_window_line_aligned`, `brace_heuristic`, `semble_native` (Semble-owned chunker only). Frozen output-unit policy: `rank_prefix`. Frozen Semble revision: `0.6.0`.

W0-B qualification admission is a separate, schema-closed authority packet. `scope=qualified` requires it; `scope=exploratory` refuses it and quality/performance remain not-applicable. The packet binds the source and repository revisions, corpus/suite/query-pack bytes, approved license receipt, two distinct annotation authorities and receipts, adjudication receipt, model revisions/assets, Semble lockfile, host profile, cache regime and exact contract/SDK receipts. T17 is re-derived before capture and again from frozen run artifacts.

## 2. Test data and anti-leakage custody

1. **Tiny oracle repo**: generated for tests, contains multi-file Rust/Python examples with known byte/line answers, comments/strings, duplicate symbols, Unicode, CRLF, long functions, no-answer queries, generated/binary/symlink exclusions and one incremental edit. This tests mechanics, not search quality.
2. **Pilot real repo**: a clean pinned Rust checkout with at least 20 reviewed, balanced symbol/semantic/architecture queries, labeled exploratory-only. Freeze path manifest and exact file bytes once. Obtain an independently reviewed answer set before viewing either engine's results; do not derive gold from either output. Gold requires two-person independent adjudication: two reviewers judge each answer independently and reconcile disagreements under the frozen rubric, with the reconciliation recorded. If the proposed repo or labels are ineligible, record why and select another before running the comparison. Pilot completion proves machinery, not population-level superiority.
3. **Broader suite**: multiple pinned repos/languages and sizes, query categories and no-answer stratum where independently labeled. Publish all eligible/excluded counts by category and repo. Do not silently pool a Rust-only result with a 19-language published aggregate.
4. **Split**: tuning/development and frozen evaluation queries must not overlap by query, near-duplicate paraphrase or answer-bearing span; the evaluator already rejects exact label-span overlap, and W0 must define/review the stronger semantic-leakage check. Lock chunk sizes, ranking knobs, model and `top_k` before eval. Semble's public model-generated/model-checked labels are an external comparison set, not an independent holdout.
5. **Blinding**: the Quanta and Semble runners consume only a frozen query pack plus source files. Gold/grades are evaluator-only. Every run records `blinding: isolated | attested`, `isolation_method`, and an `access_block_log`. `isolated` requires a runner process that cannot read the suite path (separate suite access, enforced path/permission denial, or sandbox), with the block verified and logged. `attested` keeps `gold_access: false` as an attestation only and explicitly downgrades any quality claim to attested-only; a blinded `QUALITY_DELTA` requires `isolated`. A committed suite in the same accessible checkout is not blinded by naming convention.
6. **Relevance rubric**: W0 stage A must freeze the 0–3 grade meaning, how one candidate maps to one or more gold spans, partial overlap, same-file duplicates, ties and large-context penalty before inspecting eval results; W0 stage B freezes the two-person adjudicated gold set. Primary quality is graded NDCG@10 only for independently adjudicated grades under that rubric. Without them, report span Recall/MRR/BCY as diagnostics and mark `QUALITY_DELTA` unavailable; never invent grades from Quanta/Semble rankings. The rubric must not award a whole-file chunk full context-quality credit solely because it contains a short gold span.

## 3. Blocking correctness matrix

| ID | Layer / owner | Positive oracle | Negative or mutant that must fail |
| --- | --- | --- | --- |
| T00 | RB-00 corpus | One canonical sorted tracked-file inventory and byte digest for both runners | Dirty/wrong HEAD; ignored/extra/missing/symlink/binary/oversize file; divergent Semble filter |
| T01 | RB-01 suite | Valid pinned labels, distinct IDs, blind query-pack SHA, mandatory file universe with recomputed digest | Wrong file/line/byte hash; unsafe path; missing gold; accidental gold or grade in runner pack; universe digest mismatch |
| T02 | RB-01 split | Tuning vs eval separation, query families confined to one split, reviewed grade and category, explicit rationale-backed overlap allowlist only | Duplicate/normalized/shingle near-duplicate query, cross-split family, overlapping answer span across splits without a both-sides allowlist entry; invented no-answer label |
| T03 | RB-01 record | Every eligible `(task, route)` has one ordered result, capture-referenced route provenance, byte spans consistent with line projections, a comparison contract byte-equal to the pack, and a closed protocol lock independently matched to frozen capture inputs | Missing/duplicate result, unknown field, mismatched model/repo/query hash, nonfinite timing, null/0 timing confusion, timeout without measured duration, duplicate candidate byte span, contract or protocol-pin mismatch per field, dangling capture_id |
| T04 | RB-01 scoring | Hand-calculated byte-span Recall/MRR/NDCG and BCY golden cases; coverage is byte containment | Same-file wrong lines, partial byte span credited for full-line gold, duplicate chunk boosting, out-of-budget result credited |
| T05 | RB-02 process | Isolated daemon boots, SDK full client connects and readiness precedes publish | Wrong socket/state root, no readiness, stale daemon or accidental old index reuse |
| T06 | RB-02 SDK write | `SearchCorpusBatch` publish returns sealed receipt; CAS activation acknowledges exact identity | Direct IPC, mismatched digest, failed/partial seal, activation conflict, query before activation |
| T07 | RB-02 SDK read | SDK lexical/semantic/hybrid query produces real ranked spans under expected generation | Wrong route, stale generation, typed timeout/error converted to empty or success, capped result treated exhaustive |
| T08 | RB-03 chunk bytes | Original byte slice equals emitted text; byte/line bounds and stable ID agree | UTF-8 split, CRLF/off-by-one, BOM, EOF, zero-length, overflow, reordered/non-deterministic ID |
| T09 | RB-03 syntax | Parser-derived boundaries and declared fallback coverage on supported language | Unsupported grammar, parse error, long declaration, silent whole-file fallback, overlap/duplicate inflation |
| T10 | RB-03 semantics | Each strategy's dependent semantic sources are rebuilt under same pinned model | Reused vectors/source digest from another chunk strategy; `hash-dev` passed as model-quality route |
| T11 | RB-04 Semble | Pinned real Semble result mapped to exact tracked file and line span | Truncated snippet used as full chunk, path-prefix drift, missing model, skipped query, ignored-file mismatch |
| T12 | RB-05 pair | Same commit, files, queries, `top_k`, output-unit policy and host; complete raw rows, fully rebound protocol pins and entry/replay sample eligibility | One-sided run, public README score substituted, partial sample set, wrong host/model/cache state, two-task/1,000-observation qualification, malformed/unknown protocol pin |
| T13 | RB-05 report | Deterministic re-score of immutable records gives identical aggregates, disagreements and digests before and after atomic directory promotion | Score or digest changes with row order/path relocation, omitted errors/exclusions, raw sample longer than its containing phase, invalid artifact still labelled qualified |
| T14 | RB-06 command | One Quanta-only chunk A/B command; optional Semble pair command; external artifact root; promoted pair replay through a fresh public verdict process | CI implicitly downloads model/Semble, output dirties source tree, profile name claims unselected tests, stage-only validation mistaken for final-path proof |
| T15 | RB-00/RB-04 model parity (same-model claim only) | Same pinned Model2Vec weights, tokenizer and normalization produce vectors within a predeclared tolerance on code/query probes | Matching model name but different tokenizer, vector dimension, normalization or scores called “model-matched” |
| T16 | RB-02/RB-05 incremental (incremental claim only) | SDK update/rename/delete becomes visible only after acknowledged activation; old spans disappear | Stale hit, mixed generation, mtime-only freshness timestamp, full-rebuild time reported as incremental |
| T17 | RB-00 W0-B admission | Exact authority bundle verifies source/corpus/suite/license/two-person gold/adjudication/models/lockfile/host/contract+SDK receipts before a qualified run | Exploratory promotion; duplicate annotator; post-capture receipt tamper; stale source/model/host/verification digest; undeclared cache regime |

For T00/T11, an identical *file count* is insufficient: compare canonical path+file SHA lists. For T04, evaluate both raw chunk ranking and the declared same-file-collapse rule; never let one rule silently replace the other. Paired uncertainty uses deterministic within-category bootstrap resampling; every qualified eval task must carry its frozen category. For T06, validate SDK-issued `BatchReceipt` and `SearchPlaneSearchCorpusActivationCasAck`, not a daemon log line. For T00–T11 proof, freeze the exact required Python and nextest test identities for the command, package, target, features and source; bind collection and raw terminal artifacts to the receipt. Refuse any omitted or duplicated required identity, an announced/terminal count mismatch, a skipped required test or substituted unrelated pass. Do not hardcode the current test count. T15 failure blocks only the same-model control claim; an end-to-end system pair with disclosed models stays eligible. T15 and T16 are `NOT_APPLICABLE` when their respective claims are absent; `NOT_RUN` applies only when a required claim was selected but its evidence is absent. T17 blocks every qualified quality/performance state but never blocks an explicitly exploratory `PAIR_VALID` diagnostic.

## 4. Process integration and lifecycle scenarios

- Boot fresh daemon with isolated state root and separately pinned binary/config. Provision model before timed phases. Wait for readiness through a product surface; assert initial index is empty.
- Publish multiple files and chunks in one declared batching policy. Assert counts, scope digests, seal receipt, composite activation identity and query-visible generation. Repeat with a new state root to prove determinism.
- Exercise wrong manifest/revision, stale expected-active CAS, missing model asset, provider unavailable, timeout and daemon termination. Each must produce a typed failure record and **no** scored success; runner-owned process/state resources are released within a bounded timeout.
- Optional incremental track: edit/rename/delete one file at a frozen follow-up revision, publish through SDK, and verify old hits disappear/new hits become visible only after acknowledged activation. Measure freshness from accepted/durable producer receipt to query visibility; keep this distinct from full cold-build speed and existing synthetic freshness rail.
- Do not test the SDK path by linking the in-process fixture harness. `SDK_PATH_GREEN` requires a separately launched searchd and public SDK APIs for writes, activation and reads.

## 5. Semble fairness and adapter proof

Before a paired run, pin Semble commit/package, model weights/revision, Python/runtime dependencies, Quanta binary/model, repo commit and file filters. The [upstream method](https://github.com/MinishLab/semble/blob/main/benchmarks/README.md) compares code chunks and reports index and warm query timings separately; the local run is the only opponent measurement used here.

| Check | Rule |
| --- | --- |
| Corpus | Hash every admitted path+bytes on both sides. If Semble cannot index an identical set, fail the common-universe pair; separately report native coverage. |
| Path-mapping proof | Emit a path-mapping proof artifact: the explicit path map (Semble-visible path → canonical repo path for every admitted file) plus the both-side path+SHA diff (Quanta-side list vs Semble-side list, per-file SHA). Any mismatch fails the common-universe pair; the diff digest is a mandatory manifest field. |
| Query | Same exact text, order-independent query IDs, predeclared `top_k` and result budgets. Do not rewrite queries for one side. |
| Output | Convert native spans/ranks without changing order; prove each span against source bytes. Missing or malformed results are typed failures, not abstentions. |
| Model | Main system comparison records actual model and revision. Add a model-matched `potion-code` control only after T15 validates weights, tokenization and output behavior on fixed probes; a same-model comparison is still not a same-ranker comparison. T15 gates only that control claim. |
| Timing | Compare warm SDK/API layers separately from CLI process startup. Include total time-to-searchable and phase breakdown; do not remove embedding/model cost from one system only. |
| Attribution | Record Semble native index/query behavior, not a reimplementation or a copied upstream reported score. |
| Manifest | Mandatory fields: tokenizer/budget version, Semble dependency lockfile digest, and both-side path+SHA diff digest. A run missing any of these cannot be labeled authoritative. |

## 6. Measurement protocol and sample floors

- **Preflight:** freeze exact SHA of Quanta, Semble, suite and corpus; verify clean checkouts, CPU/memory/OS, toolchain, model file digests, process limits and no other benchmark/build load. The quiet host is a fixed host profile (pinned CPU/power plan, no concurrent builds or benchmarks, declared cache regime). Apply the check-record rule: check CPU identity/settings, concurrent-build/benchmark absence, and thermal/frequency sanity, and record each observation in the run manifest; there is no single load-average gate. Contention override yields diagnostic status only. Do not run both systems concurrently for timing.
- **Cold build:** prebuild binaries and provision/download models before measurement. For each repo/system, use at least five fresh index/state roots, alternating system order by repetition. Include file discovery, chunking, embedding, SDK publish/seal/activation and first successful query in `time_to_searchable`. Record cache regime explicitly (OS page cache/model loaded vs true process cold); never call a warm-cache build “cold start.”
- **Warm query:** first warm each index, then randomize query order from the same driver-issued permutation schedule without changing query text. Qualification requires each independent floor: at least 20 distinct admitted tasks, exactly one eligible Quanta route, at least one warmup pass/root, at least five fresh roots and at least 1,000 valid warm observations per eligible route; 20 queries × 10 measured repetitions/root × 5 roots is one valid configuration. Repetition cannot compensate for too few tasks. Entry and verdict must re-derive the same eligibility from frozen task/route/root/protocol data. Record per-query raw monotonic durations, error/timeout/partial counts, and keep the cold probe separate. For serial calls, raw warm durations must fit the enclosing warm measurement window within a declared clock/serialization tolerance, and cold duration must fit its cold window; Semble first-sample boundaries must agree with the frozen first-query evidence. p99 is descriptive unless the sample floor is met; do not gate on a five-run p99.
- **Resources:** sample the whole owned process tree and per-process RSS/CPU; measure index bytes from isolated roots, excluding shared model cache but reporting that cache's bytes separately. Count file discovery and parser/embedding caches. No system-wide `getrusage` of only the coordinator as a substitute for daemon+provider resource use.
- **Uncertainty:** publish query-paired deltas by repo/category plus bootstrap intervals where sample size supports them; use the query/repo as resampling unit, not individual repeated latency samples as independent relevance judgments. Predeclare primary metric and exclusions; no post-hoc threshold selection.
- **Incremental:** separate scenario and artifact: full initial index, one-file update, rename, delete, stale-hit check, and receipt-to-visible duration. Never blend these times into full-build median.
- **Concurrency:** the primary pair is single-query/single-process warm latency. Concurrency/QPS is a separate diagnostic unless both APIs admit the same declared client/concurrency and offered-load contract; report offered/accepted/completed/error counts and do not conflate it with the existing synthetic Quanta open-loop rail.

## 7. Execution ladder and command ownership

1. **Static PREP (cheap):** `just benchmark-prep-local` checks the shared benchmark control plane without rerunning retrieval contracts. `just retrieval-contract-local` checks the v3 evaluator/schemas, merge, matrix, receipts, verdict state machine, retrieval Rust library and chunking contract in a dirty-checkout edit loop. Complete code, adversarial regressions and normative documentation before the source freeze; issue `just retrieval-contract-proof <fresh-output-root>` on that frozen source. These are contract checks, not benchmark measurements; neither command substitutes for a real pair. A later bound document edit invalidates the source closure and dependent receipts.
2. **SDK process proof:** `just retrieval-sdk-proof <fresh-output-root>` runs T05–T07 and relevant T10 binding/determinism checks on a tiny repo with the actual runner and daemon binaries. It emits machine-derived `sdk_results.json` and a canonical receipt only from clean source. Dirty-checkout console green remains implementation evidence, not `SDK_PATH_GREEN` authority.
3. **Pilot pair (exploratory-only):** `just retrieval-pair <spec>` exists and freezes one repo/suite, runs Quanta and Semble sequentially, validates the pair and scores it. It has not been executed against an admitted W0-B pilot. If any shared-universe prerequisite fails, stop before expensive repetitions. W0-B exit (repo + gold + model + host freeze) remains the measurement-entry gate. Pilot output is `PAIR_VALID` or a typed refusal; quality/speed claims require the additional qualification conditions.
4. **Qualified full run:** repeat the same immutable protocol on the reviewed broader suite and quiet canonical host profile. Preserve raw records, complete per-query report, failed/ineligible rows and provenance. No committed baseline or registered `benchctl` family until its artifact/host/source controls are actually wired and tested.

Every closeout states the exact command, source SHA and dirty state, selected/executed/passed/failed counts, covered surface, excluded surface, artifact path, terminal verdict and failure class. `NOT_RUN` or missing evidence remains open. Product deployment/activation is not implied by benchmark qualification.

## 8. Verdict artifact JSON schema

Every paired run emits exactly one `verdict.json` under the external output root. It carries the five global states independently, per-state proof references, structured provenance, and one entry per scored comparison. No run is authoritative without this artifact. The machine-readable contract is `tools/benchmark/retrieval/verdict.schema.json` (`verdict_version` 2); this section is its human mirror. The JSON below illustrates fields and enum options; its placeholders are not a valid evidence receipt.

```json
{
  "verdict_version": 2,
  "states": {
    "CONTRACT_GREEN": "pass | fail | not_run",
    "SDK_PATH_GREEN": "pass | fail | not_run",
    "PAIR_VALID": "pass | fail | not_run",
    "PERF_QUALIFIED": "pass | fail | not_run | not_applicable",
    "QUALITY_DELTA": "pass | fail | not_run | not_applicable"
  },
  "state_evidence": {
    "CONTRACT_GREEN": {"reason": "receipts_verified", "proof_digest": "hex|null"},
    "SDK_PATH_GREEN": {"reason": "sdk_proof_verified", "proof_digest": "hex|null"},
    "PAIR_VALID": {"reason": "mapping_rederived", "proof_digest": "hex|null"},
    "PERF_QUALIFIED": {"reason": "no_speed_claim", "proof_digest": "hex|null"},
    "QUALITY_DELTA": {"reason": "attested_only", "proof_digest": "hex|null"}
  },
  "blinding": "isolated | attested",
  "isolation_method": "string describing how runner gold access was prevented",
  "access_block_log": "string with block verification entries, or attestation-only rationale",
  "missing_t_ids": [],
  "not_applicable_t_ids": ["T15", "T16"],
  "failure_class": "none | corpus_mismatch | blinding | provenance | model | host | scoring | infra",
  "provenance": {
    "quanta": {"source_sha": "40-hex git sha", "binary_digest": "hex"},
    "semble": {"revision": "0.6.0", "lockfile_digest": "hex"},
    "corpus": {"digest": "hex", "path_sha_diff_digest": "hex"},
    "suite": {"suite_digest": "hex", "query_pack_digest": "hex", "tokenizer_budget_version": "qb-v1"},
    "host": {"profile_digest": "hex", "check_record_digest": "hex"}
  },
  "counts": {"selected": 0, "executed": 0, "passed": 0, "failed": 0},
  "comparisons": [
    {
      "strategy": "whole_file",
      "baseline_route": "semble-hybrid",
      "candidate_route": "lexical",
      "primary_metric": "recall_at_10",
      "primary_delta": 0.0,
      "record_digest": "hex",
      "report_digest": "hex"
    }
  ]
}
```

Rules: `missing_t_ids` lists blocking IDs with no evidence; `not_applicable_t_ids` lists conditionally scoped IDs (T15 without a same-model claim, T16 without an incremental claim). The lists are disjoint. `failure_class` is `none` only when no state failed and no claimed conditional failed; `not_run`/`not_applicable` states do not taint it but their IDs stay listed in the appropriate list. Every `provenance` digest is re-derived from the run-manifest sibling artifacts by the verdict itself; a missing or mismatched mandatory digest fails the verdict. Manifest booleans/counts are never verdict authority. `comparisons[]` holds one entry per verified strategy x candidate comparison with the digests of the exact record and report scored; it is empty only when no comparison verified (then `PAIR_VALID` fails). RB-05 writes this artifact atomically with the pair output; RB-06 replays it at the final promoted path in a fresh process and requires identical canonical record/report identities.
