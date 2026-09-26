# SEP-23 Retrieval Benchmark — Ticket Index

Status: `F1–F4-remediation-implemented / size-aware-rubric-implemented / exploratory-pair-captured / W0-B-pending`. The shared cold/warm protocol, replay, pin and proof gates have focused regression coverage. Earlier clean-source contract and SDK receipts were issued; their exact revision and digests must be read from the external receipts, not inferred from this document. Subsequent source edits require renewed proof. Historical diagnostic captures and the newer external ten-repository route ablations are non-qualifying; no W0-B admission or qualified quality/performance verdict exists. This packet is not evidence that Quanta beats Semble.

The code-search comparator matrix and 2026-09-26 execution update are in [CODE-SEARCH-COMPARATORS-2026-09.md](CODE-SEARCH-COMPARATORS-2026-09.md): four Semble profile captures, two replayed exploratory Quanta–Semble pairs, and GIN/ripgrep Sourcegraph, OpenGrok, and codespelunker captures. All are development diagnostics; the initial Sourcegraph GIN sentence-form set included an HTTP 504 and retains a separate successful retry and complete later warm-state set. There is no admitted cross-product quality or qualified performance result. This record does not change the W0-B or T00–T17 gates below.

2026-09-25 corpus expansion and actual pair: [CODE-SEARCH-BASELINE-2026-09.md](CODE-SEARCH-BASELINE-2026-09.md) records a 10-repository/5-language/1,480-code-file **candidate** set and 10 Quanta–Semble symbol-diagnostic pairs (200 identical paired queries), with `PAIR_VALID=pass` and byte-identical verdict replays. The roster, checkouts, manifests, inputs, captures and reports live outside this repository at `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/`; the in-repo generator rejects corpus paths inside the checkout. Five repositories overlap Semble's public benchmark and five do not. Semble ranked mechanically labeled declaration spans higher in all ten repository means, but independent gold, untuned predeclaration, same-model proof, clean-source receipts and qualified speed are absent. The historical diagnostic numbers below are unchanged and do not become quality evidence.

The baseline also separates the original **sentence-form hybrid** result from sentence-form lexical/semantic ablations and a **bare-identifier lexical** ablation. Sentence-form Quanta lexical returned zero candidates on all 200 tasks, whereas bare identifiers reached exact-span Recall@10 of 0.98; the bare-identifier NDCG@10 remained below Semble's default hybrid. These are different query forms and Semble is not a pure lexical control. Read the baseline's route section and exact external artifacts before using any number.

Subsequent source work adds record-bound returned-window/lane diagnostics and runner-side timing detail. New protocol-locked reports mark `Recall@20=not_applicable` when `top_k=10`; historical reports retain the capped calculation only for immutable replay and must not be described as measured @20. This is instrumentation, not a fresh pair or qualified claim; see the baseline's follow-up section. Existing diagnostic captures predate the new sidecar.

## Current closeout boundary (2026-09-24)

Historical Python/Rust/SDK receipts bind only their exact revision, not later shared-main source. Issue all three on one clean final revision after this packet and concurrent transitive Rust changes settle. A passing source-closure check only proves the checkout is eligible for proof capture; it does not revive old receipts. Read the current exact revision and digests from external receipts, not this source-bound packet: editing this packet after capture would stale them again.

The remaining dependencies are ordered, not interchangeable:

1. W0-A rubric: the 0–3 grade meanings, exact candidate-to-gold matching, partial/overlap/tie/same-file rules and union-byte context-density gain/normalization are now fixed in TEST-PLAN §2.6 before real paired outcomes. The evaluator uses a distinct scorer identity and hand-calculated exact-span, whole-file, multi-span, overlap and repeat-coverage oracles. This is code/protocol implementation, not external gold approval or a real quality verdict.
2. W0-B external admission: provide an approved corpus license/attribution receipt, two independent annotation receipts plus adjudication, pinned pilot/holdout files and queries, both model asset/revision identities, Semble 0.6.0 environment/lockfile, and the fixed quiet-host/cache profile. The admission schema is a validator, not the source of these independent facts. The diagnostic pair below supplied a separate Semble venv, revision-pinned model, and local suite, but did not supply the independent W0-B authority packet or a quiet host.
3. Qualified capture: the exploratory diagnostic below exercised both real runners and promoted/replayed a pair. Next freeze the admitted pilot, run isolated quality and quiet-host speed pairs with the prescribed sample floors, and replay the public verdict from each final path. Require exact `PAIR_VALID`, `PERF_QUALIFIED`, and `QUALITY_DELTA` artifacts before a comparative claim. The exploratory pair and clean contract/SDK receipts cannot establish qualified quality or speed.

Item 2 needs independent benchmark-authority/reviewer inputs; do not fabricate grades, license approval, independent annotators, model parity, or host evidence to make a local run green. A diagnostic or attested run may exercise plumbing but remains explicitly non-qualifying.

### Real diagnostic pair (2026-09-24; non-qualifying)

The driver at clean source `96642c0691586702ca3c6b0c4d72b01230d10da0` ran Quanta's `potion-code` hybrid route and an offline Semble 0.6.0 hybrid route against the same seven-file source manifest and 20-query, mechanically labeled self-code suite. The model revision was `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`; the Semble environment was version-pinned and content-digested, not package-hash-locked. A first attempt failed before capture because the long staging path exceeded macOS `SUN_LEN`; the successful run used a short `/private/tmp/p` staging root and `fixed_window_strict` (1,024 bytes) for Quanta. Its complete output was copied to `/Volumes/Extreme SSD 1/qi-rb-real-pilot.nnfOQT/pair-01` and the verdict re-derived there in a fresh process. The original and relocated verdicts have identical SHA-256 `5966579630c265b3afb166e35d5677b3837d229dedb7bb0ee7e61ab01130d7e4`; the run-manifest SHA-256 is `354a18f5b66aab8fc93fe3ea7f262439fd9ec618257ad4fb28f69e21688df5b8`.

The first diagnostic verdict is `PAIR_VALID=pass`, `CONTRACT_GREEN=not_run`, `SDK_PATH_GREEN=not_run`, `QUALITY_DELTA=not_applicable`, `PERF_QUALIFIED=not_applicable`. A second run on the same clean source and suite attached the contemporaneous raw Python/Rust/SDK receipt set. Its output is `/Volumes/Extreme SSD 1/qi-rb-real-pilot.nnfOQT/pair-02`; the promoted and relocated fresh-process verdicts are byte-identical at SHA-256 `dbcc2a6b41eabd98d75f55b2a2d5435017bb86a3d8cf2487103814ea5f95be9f`, with run-manifest SHA-256 `d76366fc02bcc6ad24e0b402d69a000182615701ae1aa5eb854d968d1accf08a`. Its states are `CONTRACT_GREEN=pass`, `SDK_PATH_GREEN=pass`, `PAIR_VALID=pass`, `QUALITY_DELTA=not_applicable`, `PERF_QUALIFIED=not_applicable`.

The external volume later disappeared from the OS device list, so its copy is currently unavailable. The surviving `/private/tmp/q` pair was copied byte-for-byte to `/Users/songmin/Documents/code-new/qi-rb-evidence-2026-09-24` (332 regular files, no symlinks; `diff -qr` clean). A local detached worktree at `/private/tmp/qi-rb-966-replay` re-derived the preserved run manifest in a fresh process; its verdict SHA-256 is again `dbcc2a6b41eabd98d75f55b2a2d5435017bb86a3d8cf2487103814ea5f95be9f`. This local copy is the available diagnostic artifact until the external volume returns. It does not revive the old source-bound receipts for later main revisions.

Both systems returned the admitted file universe; at the 2,000-token budget the first diagnostic's block-recall scores were Quanta 0.85 and Semble 0.70, but the 20 tasks have self-authored, non-adjudicated grade-1 gold and a single repetition. These numbers are debugging observations only, not comparative quality or latency evidence. The frozen pair source predates this documentation edit and later shared-main changes; this record does not qualify current main.

Post-pair hardening on current main adds a pre-stage Unix socket pathname check and makes Semble's digest-pinned environment freeze an *exact* distribution-set comparison. This prevents a late `SUN_LEN` daemon failure and an undeclared extra package from silently entering a comparison. The freeze digest is not a per-wheel archive hash or independent installer receipt. These edits require new clean-source contract/SDK proof before any current-main qualification.

Required verification contract: [TEST-PLAN.md](TEST-PLAN.md). Its T00–T17 matrix and qualification ladder are part of every ticket's acceptance, not optional follow-up work.

## Adversarial audit and remediation (2026-09-24)

At `20bd03fc98b86ee4bfb3a8a808e940ba6e090a09`, the retrieval implementation was byte-identical to the source probed at `93b736cd0f18ab698f5a12e23a2d09677a3d869d`. Existing focused tests passed (182 Python, 31 retrieval-library and 20 chunking-contract Rust), but generated adversarial fixtures reproduced four verifier defects. Commits `688238a8`, `f42adc5f`, and `64938975` implement their remediation, pair-promotion replay, and a positive five-root/20-task/1,000-observation verdict test. At `a634b90a`, clean-source proof executed 173 Python contract, 51 Rust contract and 12 SDK tests with zero skips and issued three source-closure-bound receipts. That receipt set is revision-specific; any later relevant source or document change requires reissuance. Earlier source closure and test results are historical and cannot qualify a changed source.

| Finding | Reproduced on audited baseline | Implemented remediation; final proof still required |
| --- | --- | --- |
| F1 / T12–T14 | Renaming a valid staged pair without changing any file bytes turned `PAIR_VALID=pass` into `fail` on replay. | Relative capture provenance and successful-promotion fresh-process replay regression. |
| F2 / T03/T11/T12/T17 | Seven mutations of recorded protocol pins retained `PAIR_VALID=pass`. | Closed 16-key protocol lock and independent frozen-input/capture binding; pin mutants now have rejection tests. |
| F3 / T00–T11 | Partial/duplicate JUnit and incomplete nextest streams certified contract proof. | Complete Python/Rust/SDK identity inventories, terminal evidence checks and receipt-revision Git blob binding. |
| F4 / T12–T13 | Two tasks repeated to 1,000 observations qualified speed; impossible raw duration was accepted. | Shared entry/replay eligibility and phase/sample containment regressions. |

The implementation and contract/SDK proof are complete at their receipt revision. Replay the promoted pair on admitted inputs before closing the workstream. The external W0-B authorities and admitted quiet-host pilot remain unsupplied.

RB-01's earlier whole-file counterexample is addressed by the W0-A context-density rubric: a rank-1 10-byte exact hit scores 1.0, while a 1 MB candidate containing those 10 bytes scores 0.00001. The scorer and independent oracles are implementation evidence; `QUALITY_DELTA` still requires real W0-B adjudicated gold, isolation and an admitted pair. Graded fixture output is not quality superiority evidence.

Qualified quality and speed now fail if any prerequisite state is not `pass`: `PAIR_VALID`, `CONTRACT_GREEN`, or `SDK_PATH_GREEN`. A protocol-lock pin mutation and raw contract/SDK evidence corruption previously left the qualified state green; paired source-bound regression fixtures now require both claims to fail. This is a verifier dependency repair, not a real paired result.

## Earlier implementation audit (historical, 2026-09-24)

The 2026-09-24 re-audit at `f26796896e365a8eb66a516f959c460f792e7506` verified 160 Python contract tests, 31 retrieval-library tests, 20 chunking-contract tests and 22 receipt/proof-helper tests on the observed dirty checkout. It also added deterministic seed-schedule reconstruction so a self-consistent forged query protocol is rejected. `python3 tools/ci/source_closure.py check --profile retrieval` correctly refused the concurrently modified relevant source, so these are implementation results, not a source-bound receipt. The earlier clean `fa17e0f14672548bf7116074b7064c7c5e455d03` admission and older dirty-checkout results remain historical only. Retrieval receipts bind a v2 source closure: the transitive local Cargo package set plus retrieval proof tools/schemas, exact file list, per-file SHA-256 and full Git revision. Unrelated dirty paths are allowed; a relevant dirty/add/remove/HEAD change refuses capture or receipt issuance.

| Ticket / wave | Current code state | Remaining terminal condition |
| --- | --- | --- |
| RB-00 / W0-A | v3 suite/runner artifact contract, size-aware relevance rubric and schema-closed W0-B admission validator are implemented. Qualified admission binds the exact source/corpus/suite/query pack, license, two annotation receipts, adjudication, models, lockfile, host and contract/SDK receipts. | No real W0-B authority packet has been issued by external owners. |
| RB-01 / W1A | Single evaluator accepts the current v3 suite and v5 runner record; immutable v3/v4 runner branches remain replay-only, and unknown stamps fail closed. Context-density NDCG and independent oracles address whole-file over-credit. Paired uncertainty uses deterministic within-category bootstrap with an evidence-derived seed. | No qualified external suite or real-pair scoring artifact has been issued. |
| RB-02 / W1B | Benchmark-only Rust runner uses the public SDK and a separate pinned searchd. A subprocess integration test executes the actual runner binary and binds its SHA, sealed receipt, activation ACK and v3 record. The harness writes daemon logs outside the state root. Live real-daemon negatives now cover missing pinned model, provider unavailable, stale activation CAS, readiness timeout and post-spawn termination; each fails typed without hits/record and uses bounded owned-process cleanup. The driver freezes `failure.json`, stderr/resource digests and record-presence state for failed capture processes and cleans the entire owned process group. `retrieval-sdk-proof <fresh-out>` emits machine-counted results plus a source-closure-bound receipt. | Clean-source SDK proof passed 12/12 at `a634b90a`; no admitted pilot capture exists. |
| RB-03 / W1C | Whole-file, strict/line-aligned windows and Rust syntax chunking plus independent span/coverage validation are implemented. The manifest decoders are manual/duplicate-rejecting, Semgrep is green, and the derive allowlist covers `crates/` and `benchmarks/`. | No real ablation artifact exists. |
| RB-04 / W1D | Semble 0.6.0 adapter, lockfile/interpreter/model-asset checks, mapping proof and v3 normalization are implemented and fixture-tested. A real exploratory capture on a separate pinned venv passed pair validation at source `96642c06`. | No W0-B-admitted Semble pilot capture has been recorded. |
| RB-05 / W2 | Pair orchestration, immutable staging, admission/manifest/verdict validation and a shared cold-probe/warm-query protocol are present. F1/F2/F4 fixes now cover relocation, closed protocol pins and common speed eligibility. Isolated capture uses a default-deny Seatbelt allowlist over an exact Git-free corpus. | Synthetic promoted-pair replay passes; admitted performance/isolation capture remains absent and no non-macOS isolation backend exists. |
| RB-06 / W3 | Named proof/capture/verdict/host-profile commands and Rust integration targets exist. Receipt v2 binds retrieval source closure, raw JUnit/nextest/runner evidence and required test inventories. | Clean-source contract/SDK receipts issued at `a634b90a`; real paired verdict remains absent. |

Historical focused execution on the earlier dirty shared checkout:

- `python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_write_verification_receipt.py -q`: historical 157 passed, 0 failed on the earlier dirty implementation tree. This is not a qualification receipt or a current-source count.
- The earlier retrieval Rust package inventory was 63 tests. On that dirty implementation tree, the 49 lib/chunking tests, 12 explicit-searchd-pinned SDK tests, and 2 binary contract tests passed; this split execution is implementation evidence, not a current-source receipt.
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

1. Integrate implementation, adversarial regressions and this packet's normative documents; resolve protocol-lock producer/consumer keys together. Then commit/freeze one retrieval source closure and run the registered workspace rail on that exact source, including the live SDK test with the `cargow`-injected daemon pin.
2. Run `retrieval-contract-proof` and `retrieval-sdk-proof` on that frozen clean source only after the full required test inventory, raw terminal evidence and duplicate/skip refusals pass. Freeze their summaries/receipts into the W0-B stage. Keep `sdk_results.json` bound to the actual runner record/binary; never hand-author verdict inputs.
3. Freeze W0-B inputs outside the checkout: pilot commit and admitted manifest, reviewed license/attribution, two-person adjudicated gold, model assets/revision, Semble lockfile/interpreter, tokenizer/budget version and quiet-host/cache profile.
4. Exercise the verified phase/RSS artifacts on the admitted pair before enabling `claims.speed`. For `claims.quality`, use `blinding=isolated` with an external `suite_secret_root`; the macOS Seatbelt proof must survive verdict re-verification.
5. Retain the live missing-model/provider/CAS/timeout/termination negatives in the frozen SDK receipt; do not replace them with classification-only tests.
6. Run the exploratory pair first, issue `run-manifest.json` and `verdict.json`, atomically promote the output and rerun the public verdict from a fresh process against the final path. Require stable canonical record/report digests and inspect every exclusion/failure before broader qualification. Re-freeze HEAD and source digests after every bound code or document change; reissue stale receipts.

## Objective and ownership

Run reproducible real-repository search-quality and performance comparisons **entirely from quanta-index**. Vary chunking here, publish and query through `quanta-index-sdk`, and compare against Semble on the same pinned corpus and queries. Semantica is not a build, runtime, or test prerequisite. The benchmark matches Semantica's Quanta SDK integration boundary, not necessarily Semantica's producer-generated content.

Implemented authorities:

- `tools/benchmark/retrieval/{evaluator.py,suite.schema.json,runner.schema.json}`: current v5 runner validation, blind query pack, and byte-span/context-budget scoring. Historical v3/v4 records are validated only by their immutable legacy branches; unknown artifact stamps are rejected. Candidate generation remains runner-owned.
- `quanta-index-sdk::SearchCorpusBatch::{replace_generation,replace_scope,replace_semantic_scope}`, `SearchCorpusNamespace::publish_and_activate`, and SDK query namespaces: product ingest/read path. The benchmark must not use `E2eRuntime::ingest_text*` or dispatch IPC directly.
- Existing search-quality/latency rails remain engine regression and operational evidence. They are not replaced or reclassified as real-repository retrieval quality.
- The `potion-code`/Model2Vec path is present in committed code. Its presence is not a model-backed head-to-head qualification. Freeze the exact implementation and model asset digest before quality capture.

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
- The current runner artifact contract is v5; the suite remains v3. Historical runner v3/v4 inputs are accepted only by their immutable version-specific validators. All-answerable external suites are valid without invented no-answer tasks. V1/v2 are rejected, not read as migration inputs.
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
