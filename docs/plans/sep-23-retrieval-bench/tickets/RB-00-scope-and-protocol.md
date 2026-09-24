# RB-00 — Scope Freeze and Benchmark Protocol

Status: `stage-a-implemented / stage-b-authority-schema-landed / external-freeze-blocked`

Depends on: none

Owner: benchmark plan/protocol; no product runtime edits

## Current code status (2026-09-24)

Stage A's artifact and protocol shapes are implemented in the v3 suite/runner schemas, pair/run-manifest/verdict schemas, evaluator, mapping-proof contract and negative tests. Its graded-relevance rubric now fixes first full-span coverage and union-byte context-density gain in [TEST-PLAN.md](TEST-PLAN.md) §2.6, with a distinct scorer identity and independent hand-calculated oracles. Stage B's machine authority is implemented in `admission.schema.json`: qualified capture requires and re-verifies the exact source/corpus/suite/query pack, approved license, two distinct annotation receipts, adjudication, models, Semble lockfile, categorized eval strata, host/cache regime and contract/SDK receipts. Stage B has not externally exited: this repository contains no issued authority packet for an approved pilot. No real quality or speed claim is authorized.

## Goal

Freeze the benchmark's comparison unit and data custody before runner work. Prevent a fast fixture rail, a mismatched file universe, or a Semble README number from becoming a head-to-head result.

## Work

### Stage A — protocol + schema freeze (unblocks W1 scaffolding/scorer)

A1. Inventory the original evaluator baseline, SDK publish/query APIs, searchd lifecycle, supported languages/file filters, semantic model profiles and existing benchmark artifact/manifest rules. Record what can be reused and what is absent. This inventory produced the current v3 authority; old artifacts are rejected.
A2. Define one versioned run protocol: pinned repo commit, exact tracked-file manifest and exclusions, query-pack digest, per-system version/build/model/chunker configuration, result spans, timings, receipts, errors and host. Use the existing evaluator's blind query-pack and file-hash rules; extend rather than fork it.
A3. Specify corpus policy: same source bytes and admitted file universe for both systems; distinguish common-coverage scoring from native-coverage reporting. Explicitly handle large files, generated files, binary files, unsupported languages and ignore rules. No silent subset reduction.
A4. Use one current suite/runner shape, common-file-universe construction and label isolation mechanism; old shapes have no migration reader. Every run records `blinding: isolated | attested` plus `isolation_method` and an `access_block_log`; `gold_access: false` without process isolation remains an attestation, not proven blinding.
A5. Freeze the graded relevance rubric shape and primary NDCG@10 candidate mapping before viewing eval results, including the two-person independent gold adjudication rule.

### Stage B — repo + gold + model + host freeze (measurement-entry gate)

B1. Inspect Semble benchmark data format, pinned repositories, license/attribution, installation and model cache requirements. Select a small reviewed exploratory-only pilot and a separate holdout; do not assert that all published queries are already valid under the current Quanta suite contract.
B2. Freeze the pilot repo commit, admitted manifest, tokenizer/budget version, and the two-person adjudicated gold set before either engine's results are viewed.
B3. Prove the Semble path-mapping on the frozen manifest (path map plus both-side path+SHA diff); a mismatch fails the common-universe pair.
B4. Audit the in-flight `potion-code` profile on a clean source revision and pin the exact local model files, revision, and Semble dependency lockfile before designating it as the model-matched Semble control. A compiled but unqualified embedder is not an admitted benchmark route. Independently verify whether identical Model2Vec model files, tokenization and normalization produce equivalent vectors; a shared model name alone does not establish model parity. T15 gates only the same-model claim.
B5. Freeze correctness and speed measurement definitions, including the quiet host fixed as a host profile (pinned CPU/power plan, no concurrent builds/benchmarks, thermal/frequency sanity under the check-record rule — no single load-average gate) and cache regime, before running opponents. Record non-comparable configurations as `ineligible`, not PASS or a zero score.
B6. Issue the schema-closed W0-B admission manifest and its independent evidence files before capture. The driver copies and verifies them into the stage; the verdict re-verifies the frozen bytes. Exploratory output cannot be upgraded by editing `scope` or adding a claim after capture.

## Owner / expected files

- `tools/benchmark/retrieval/README.md`
- `tools/benchmark/retrieval/admission.schema.json`
- `tools/benchmark/retrieval/suites/` manifest and provenance documents
- This packet's index and protocol decisions

## Acceptance / verification

- Stage A exit: protocol contains explicit unit of relevance (file and line span), graded-label policy, tie/dedup policy, `top_k`, tokenizer/budget policy, no-answer stratum, run ordering, exclusion accounting, blinding enum fields, and the path-mapping proof artifact shape.
- Stage B exit (measurement-entry gate): pilot repo/manifest/gold/model/host frozen; Semble revision, dependency lockfile, and data license pinned before its annotations are used; no copied repo or model cache is committed. Mandatory manifest fields (tokenizer/budget version, Semble lockfile digest, path+SHA diff digest) are defined.
- A reviewer can determine from the manifest whether the same corpus/query universe and model class were measured.
- [TEST-PLAN.md](TEST-PLAN.md) T00–T02/T15/T17 and the Semble feasibility checks have named owners, negative tests and a stop condition before capture. T15 gates only the same-model claim; T17 gates every qualified quality/performance state.
- This ticket produces no quality or speed claim.
