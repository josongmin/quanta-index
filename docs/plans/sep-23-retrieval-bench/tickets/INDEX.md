# SEP-23 Retrieval Benchmark — Ticket Index

Status: `implementation-landed / qualification-blocked`. The benchmark machinery is present, but no W0-B pilot freeze, paired pilot receipt, or qualified quality/performance verdict exists. This packet is not evidence that Quanta beats Semble.

Required verification contract: [TEST-PLAN.md](TEST-PLAN.md). Its T00–T16 matrix and qualification ladder are part of every ticket's acceptance, not optional follow-up work.

## Current code audit (2026-09-24)

Latest implementation audit started at `54cfb96fad859f733c1e095612016a82cff4945b`; the shared `main` continued advancing during implementation and was observed at `41068a5e53f45f2f0dfea996614e87454ef0c3f6` after the receipt-schema commits. The changes and results below are working-tree implementation evidence, not an exact-source qualification receipt. Retrieval receipts bind a v2 source closure: the transitive local Cargo package set plus retrieval proof tools/schemas, exact file list, per-file SHA-256 and full Git revision. Unrelated dirty paths are allowed; a relevant dirty/add/remove/HEAD change refuses capture or receipt issuance. Re-freeze the final HEAD after shared-writer activity stops.

| Ticket / wave | Current code state | Remaining terminal condition |
| --- | --- | --- |
| RB-00 / W0-A | v3 suite/runner comparison contract, blinding fields, common-universe/path+SHA rules and verdict schemas are implemented and mutant-tested. | W0-B is not frozen: no approved pilot repo/manifest, independently adjudicated gold, admitted model assets, Semble lockfile/license record, or quiet-host profile. |
| RB-01 / W1A | Single evaluator supports exact v1/v2 migration reads and authoritative v3 validation/scoring; v3 pack, split-leakage, byte-span, capture and contract mutants are present. | No qualified external suite or real-pair scoring artifact has been issued. |
| RB-02 / W1B | Benchmark-only Rust runner uses the public SDK and a separate pinned searchd. A subprocess integration test executes the actual runner binary and binds its SHA, sealed receipt, activation ACK and v3 record. The harness writes daemon logs outside the state root. Live real-daemon negatives now cover missing pinned model, provider unavailable, stale activation CAS, readiness timeout and post-spawn termination; each fails typed without hits/record and uses bounded owned-process cleanup. The driver freezes `failure.json`, stderr/resource digests and record-presence state for failed capture processes and cleans the entire owned process group. `retrieval-sdk-proof <fresh-out>` emits machine-counted results plus a source-closure-bound receipt. | Final source-closure-bound SDK receipt remains absent; the live negatives are implementation evidence, not a pilot capture. |
| RB-03 / W1C | Whole-file, strict/line-aligned windows and Rust syntax chunking plus independent span/coverage validation are implemented. The manifest decoders are manual/duplicate-rejecting, Semgrep is green, and the derive allowlist covers `crates/` and `benchmarks/`. | No real ablation artifact exists. |
| RB-04 / W1D | Semble 0.6.0 adapter, lockfile/interpreter/model-asset checks, mapping proof and v3 normalization are implemented and fixture-tested. | No admitted real Semble environment/pilot capture has been recorded. |
| RB-05 / W2 | Pair orchestration, immutable staging, manifest/verdict validation, deterministic re-score and conditional T15/T16 handling are implemented. Phase and resource evidence is schema-closed down to first/warm query, per-process RSS/CPU and owned storage/cache bytes. Isolated capture denies the complete source checkout and runs both systems over an exact Git-free materialized corpus. Host authority is a frozen fingerprint artifact, not a label. | No admitted paired pilot has exercised the performance or isolation authority. Non-macOS isolation backends are not implemented. |
| RB-06 / W3 | Named proof/capture/verdict/host-profile commands and Rust integration targets exist. Receipt v2 binds the retrieval source closure plus role-tagged raw JUnit/nextest/runner evidence, which the verdict reparses. | No final clean-source contract/SDK receipt or real paired verdict exists; pilot/broader qualification remain unrun. |

Latest focused execution on the dirty shared checkout:

- `python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_write_verification_receipt.py -q`: 157 passed, 0 failed on the current dirty implementation tree. This is not a qualification receipt.
- Retrieval Rust package inventory is now 63 tests. On the current dirty implementation tree, the 49 lib/chunking tests, 12 explicit-searchd-pinned SDK tests, and 2 binary contract tests all passed; this split execution is implementation evidence, not a receipt.
- The 12-test parallel SDK run marked `stale_state_root_is_refused` as nextest `LEAK` while passing; its immediate isolated rerun passed in 0.013s without a leak marker. Preserve this as scheduler/process-attribution noise to recheck on the final clean receipt instead of hiding it or promoting it to a product failure.
- A deliberately unpinned package-only nextest invocation remains expected to fail live daemon tests with `searchd binary is not pinned`; this is not a qualified Rust receipt and confirms the pin is mandatory outside the registered proof recipe.
- On `f2897a28`, live SDK tests initially reproduced `STATE_ROOT_FORMAT_UNSUPPORTED` because the harness placed logs inside the fresh state root. Commit `7d4994b9` moved logs to sibling evidence without weakening state-format detection. The current 63-test inventory adds exact materialized-corpus coverage.
- The selected live SDK test under the broad workspace build/pin path passed before this patch set; a full post-change workspace qualification receipt has not been run.
- `python3 tools/ci/lint/check-test-authority.py`: pass on the audited working tree.
- `bash scripts/run-semgrep.sh benchmarks/retrieval/src`: pass, 0 findings.
- `python3 tools/ci/lint/check-rust-derive-allowlist.py`: pass with owned roots `crates/` and `benchmarks/`.
- Receipt/result producer plus Cargo-lane regressions: 38 passed, 0 failed.

Static policy contradiction is resolved: both Semgrep and the widened derive inventory inspect the benchmark source and pass. This does not qualify benchmark claims.

### Required closeout order

1. Commit/freeze the retrieval implementation, then run the registered workspace rail on that exact source; require the live SDK test to pass with the `cargow`-injected explicit daemon pin.
2. Run `retrieval-contract-proof` and `retrieval-sdk-proof` on one frozen clean source and freeze their emitted summaries/receipts into the W0-B stage. Keep `sdk_results.json` bound to the actual runner record/binary; never hand-author verdict inputs.
3. Freeze W0-B inputs outside the checkout: pilot commit and admitted manifest, reviewed license/attribution, two-person adjudicated gold, model assets/revision, Semble lockfile/interpreter, tokenizer/budget version and quiet-host/cache profile.
4. Exercise the verified phase/RSS artifacts on the admitted pair before enabling `claims.speed`. For `claims.quality`, use `blinding=isolated` with an external `suite_secret_root`; the macOS Seatbelt proof must survive verdict re-verification.
5. Retain the live missing-model/provider/CAS/timeout/termination negatives in the frozen SDK receipt; do not replace them with classification-only tests.
6. Run the exploratory pair first, issue `run-manifest.json` and `verdict.json`, inspect every exclusion/failure, then run broader qualification. Re-freeze HEAD and source digests after every code or document change that affects the protocol.

## Objective and ownership

Run reproducible real-repository search-quality and performance comparisons **entirely from quanta-index**. Vary chunking here, publish and query through `quanta-index-sdk`, and compare against Semble on the same pinned corpus and queries. Semantica is not a build, runtime, or test prerequisite. The benchmark matches Semantica's Quanta SDK integration boundary, not necessarily Semantica's producer-generated content.

Implemented authorities:

- `tools/benchmark/retrieval/{evaluator.py,suite.schema.json,runner.schema.json}`: v3 frozen-label validation, blind query pack, byte-span/context-budget scoring and legacy v1/v2 migration reads. Candidate generation remains runner-owned.
- `quanta-index-sdk::SearchCorpusBatch::{replace_generation,replace_scope,replace_semantic_scope}`, `SearchCorpusNamespace::publish_and_activate`, and SDK query namespaces: product ingest/read path. The benchmark must not use `E2eRuntime::ingest_text*` or dispatch IPC directly.
- Existing search-quality/latency rails remain engine regression and operational evidence. They are not replaced or reclassified as real-repository retrieval quality.
- Current checkout has unrelated in-flight changes, including a local `potion-code`/Model2Vec path. No clean-source, model-backed head-to-head qualification exists merely because that code is present. Freeze a clean implementation and model asset digest before quality capture.

## Repository layout

```text
benchmarks/retrieval/                         # benchmark-only Cargo workspace member
  Cargo.toml
  src/{main,corpus,batch,sdk,record}.rs
  src/chunking/{mod,whole_file,fixed_window,syntax}.rs
  tests/{chunking_contract,sdk_roundtrip}.rs
tools/benchmark/retrieval/                    # evaluator, schemas and paired driver
  evaluator.py
  suite.schema.json
  runner.schema.json
  {run,semble}.py
  suites/                                     # reviewed suite metadata/labels, not cloned repos
  README.md
tools/ci/tests/test_retrieval_benchmark.py    # contract and mutation tests
Justfile                                     # named prep/run entry points
```

The actual owner paths are listed in each ticket; do not create a duplicate scorer. Cloned repositories, model caches, indexes, raw timings, and reports stay outside the source checkout under an explicit output root.

## Execution waves and dependency graph

W0 is split into two exit stages. **Stage A** (protocol + schema freeze) unblocks scaffolding and scorer work. **Stage B** (repo + gold + model + host freeze) is the measurement-entry gate: no authoritative capture or timing work starts before it exits.

| Wave | Ticket | Outcome | Depends on |
| --- | --- | --- | --- |
| W0-A | [RB-00](RB-00-scope-and-protocol.md) stage A | Freeze protocol, schema contract, blinding/file-universe rules, rubric shape | — |
| W1A | [RB-01](RB-01-suite-and-scoring.md) | Reuse/extend blind suite and single evaluator | W0-A |
| W1B | [RB-02](RB-02-sdk-runner.md) | Real repo → actual SDK publish/activate/query runner | W0-A |
| W1C | [RB-03](RB-03-chunking-ablation.md) | Benchmark-owned chunking strategies and boundary oracle | W0-A; integrates after RB-02 batch contract |
| W1D-env | [RB-04](RB-04-semble-adapter.md) env audit | Semble environment/data/license audit | W0-A |
| W0-B | [RB-00](RB-00-scope-and-protocol.md) stage B | Freeze pilot repo/gold/model/host; manifest-entry gate | W0-A |
| W1D | [RB-04](RB-04-semble-adapter.md) | Pinned Semble adapter on the identical file/query universe | W0-B, RB-01 record contract |
| W2 | [RB-05](RB-05-paired-measurement.md) | Paired quality/speed/resource report and contamination controls | RB-01–RB-04, W0-B gate |
| W3 | [RB-06](RB-06-verification-and-closeout.md) | Tests, registered command, pilot and broader qualification | RB-05 |

W1A/B, W1C strategy code, and Semble environment/data auditing may proceed in parallel after W0-A. W1D's record adapter waits for the RB-01 contract and the W0-B frozen manifest. W1C must not finalize batch assembly before RB-02 establishes the SDK publisher interface. Schema, `Justfile`, manifest, and aggregate verdict edits have one integrator. No ticket silently changes another lane's owner files. W0-B must resolve the exact common-file-universe, gold-adjudication, model-asset, and host-profile questions before W2 timing work. `benchmarks/retrieval/src/record.rs` is owned solely by RB-02; RB-05 consumes its records read-only (see RB-02/RB-05).

## Measurement contract

- Primary comparison: paired, identical repo commit, admitted file universe, query text, label version, `top_k`, output unit and machine. Report excluded files/queries and why; unsupported coverage is not scored as a zero or silently dropped.
- Quality: primary graded NDCG@10 **only after** the W0 grade/candidate-mapping rubric and independent judgments are frozen; otherwise the pilot is pipeline proof, not a quality verdict. Report span-aware Recall@k and MRR, existing BCY@2k/4k/8k/16k, and no-answer abstention only for a genuinely labeled no-answer stratum. BCY and candidate size prevent whole-file containment from masquerading as useful context. File-only recall is separate and never substitutes for span credit. Collapse duplicate chunks from one file only under a declared deterministic rule.
- Chunking: chunk count/bytes/tokens, overlap/duplication, invalid/out-of-bounds spans, indexing throughput, quality per strategy. `whole_file` is a diagnostic control, not the production-quality default.
- Performance: time-to-searchable from start of corpus processing through acknowledged activation; separately report chunking, embedding/provider, SDK publish, seal/activation, warm SDK query p50/p95/p99, errors/timeouts, peak RSS and index disk. CLI startup is a separate surface. Repeat under a quiet same host; end-to-end system speed can be compared with each model disclosed, but must not be attributed to the engine alone when models differ.
- Retrieval configurations: identify Quanta lexical, semantic and hybrid profiles and model revision. The deterministic Hash embedder is a correctness control, never the quality opponent for Semble's Model2Vec-based profile. Each route's actual capabilities and unavailable/degraded status are recorded.
- Authority: clean exact source SHA, built binary digest, corpus/labels/query pack digest, strategy and model configuration, Semble revision, host/resource profile and raw per-query results. Dirty or contended runs are diagnostic only. Mandatory manifest fields include tokenizer/budget version, the Semble dependency lockfile digest, and the both-side path+SHA diff digest; a manifest missing any of these cannot issue an authoritative verdict.
- Blinding is recorded as `blinding: isolated | attested`, plus `isolation_method` and an `access_block_log` describing how runner access to gold was prevented (or why only attestation holds). `PAIR_VALID` (paired protocol/universe validity) and a blinded `QUALITY_DELTA` (quality claim under proven blinding) are separate verdicts; an attested-only run cannot silently upgrade to an isolated quality claim.
- The current authoritative contract is v3. Legacy v1 requires both answerable and no-answer eval tasks; v2/v3 permit an all-answerable external suite without inventing no-answer tasks. V1/v2 are migration inputs and cannot satisfy a v3 pair.
- Every closeout emits the verdict artifact defined in [TEST-PLAN.md](TEST-PLAN.md) §8 (five states, missing T-IDs, failure class, provenance digests). T15 gates only the same-model claim; T16 is required only when an incremental-update claim is made.

Semble's [published methodology](https://github.com/MinishLab/semble/blob/main/benchmarks/README.md) supplies candidate repos/queries and a reproducibility target, **not** a directly comparable baseline number. Its published labels are model-generated and model-checked; use a reviewed subset and an independently judged holdout. Do not vendor its annotations or source before checking license and attribution. Compare fresh local runs, not Quanta measurements against the upstream README table.

## W0 decisions that block authoritative capture

These are unresolved implementation inputs, not defaults for the runner to guess. Stage A unblocks scaffolding/scorer work; stage B gates measurement entry.

Stage A — protocol + schema freeze:

A1. Versioned run protocol, suite/runner schema contract, and blinding rule (`blinding: isolated | attested` with `isolation_method` and `access_block_log` fields).
A2. Common-file-universe construction rule, path+SHA comparison method, and the Semble path-mapping proof artifact shape.
A3. Frozen 0–3 grading/candidate-mapping rubric shape, primary metric, tuning/eval isolation, and two-person independent gold adjudication rule. If independent grades are absent, no NDCG-based superiority claim.

Stage B — repo + gold + model + host freeze:

B1. Exact pilot repository commit, admitted code-only file manifest, reviewed annotations/license, and tokenizer/budget version. A candidate pilot is not an approved benchmark set.
B2. Semble path-mapping proof on the frozen manifest (path map plus both-side path+SHA diff). If Semble cannot consume the exact manifest with stable original path mapping, the common-universe comparison is blocked until an identical isolated corpus is proven; native-coverage reports remain separate.
B3. Clean Quanta source/binary, validated local `potion-code` model assets, and the Semble dependency lockfile. T15 gates only the same-model claim; end-to-end system comparison with disclosed models does not require it.
B4. Two-person adjudicated gold set frozen before either engine's results are viewed.
B5. Quiet canonical host fixed as a host profile (pinned CPU/power plan, no concurrent builds/benchmarks, thermal/frequency sanity recorded via check-record rule), plus resource sampling method and cache regime for speed qualification. There is no single load-average gate; developer-laptop numbers can diagnose but do not become a cross-system speed baseline by label alone.

## Go/no-go

1. A clean, pinned one-repository pilot (initial target: a supported Rust repository and at least 20 reviewed queries) produces both Quanta and Semble recorded results on the exact admitted file set. The 20-query pilot is labeled exploratory-only: it proves the pipeline, never population-level superiority. If the public set cannot meet this contract, choose another pinned pilot and record the reason.
2. The evaluator rejects altered gold/file hashes, label leakage into runner input, missing route/query results, unknown fields, stale source, and incompatible provenance.
3. SDK receipts attest publish and activation before any query is scored. No direct IPC or fixture-only ingest path is accepted.
4. Chunking ablations vary one declared factor at a time; dependent semantic data is regenerated or the route is explicitly ineligible.
5. Broader language/repository coverage follows pilot correctness. No arbitrary “beats Semble” threshold is invented or selected after seeing results; report paired deltas, uncertainty and failures.
6. W0-A exit is required before W1 scaffolding/scorer finalization; W0-B exit is required before any authoritative capture. The [test plan](TEST-PLAN.md) verdict artifact must reach `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`, and (for speed claims) `PERF_QUALIFIED` independently, with missing T-IDs and failure class recorded. Failed or missing stages cannot be summarized as a pass.
