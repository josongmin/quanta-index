# RB-05 — Paired Quality, Speed and Resource Measurement

Status: `orchestration-landed / replay-gates-failed / measurement-blocked`

Depends on: RB-01, RB-02, RB-03, RB-04; requires RB-00 stage B measurement-entry gate

Owner: orchestration/reporting, not product ranking

## Current code status (2026-09-24)

`run.py` implements immutable input/receipt staging, sequential and alternating Quanta/Semble repetitions, record merge, manifest construction, latency-matrix re-derivation, deterministic report validation and the five-state verdict machine. A qualified run additionally requires the schema-closed W0-B admission bundle and re-derives its license, two independent annotation, adjudication, model, host, lockfile, source and contract/SDK receipt bindings both before capture and at verdict time. Exploratory runs return quality/performance as not-applicable. The driver now issues one digest-bound cold/warm query protocol per fresh root; both runners consume and echo the same randomized permutations plus raw warm samples, and the verdict rejects schedule/count/digest/first-sample drift while excluding cold probes from the warm matrix. Validation reconstructs the complete expected protocol from the frozen seed, task order and pass counts, so recomputing the digest over a forged permutation does not make it admissible. Qualified speed preflight enforces one Quanta route, at least 20 tasks, five roots, one warmup pass and 1,000 warm observations per route. Resource evidence includes aggregate plus per-process peak RSS/CPU, typed index measurement, model/parser/embedding-cache bytes, discovered files and indexed chunks; Semble's in-memory index uses a positive worker-observed RSS delta rather than a fabricated zero. For isolated quality on macOS, Seatbelt is default-deny with explicit read/write roots, the suite and original checkout remain denied, and both runners consume a Git-free materialized directory containing exactly the manifest-admitted path/SHA universe. No real admitted quiet-host run has yet qualified the implementation.

The prior free-form `host_profile` label is no longer authority. `host-profile` emits a frozen JSON fingerprint covering OS/CPU/toolchain and the normalized active-source power configuration; battery percentage/time observations do not churn the configuration digest. The verdict requires both host probes to match it with clean thermal/frequency/power and contention state. Numeric macOS scheduler/speed limits below 100 fail, and unavailable frequency telemetry cannot inherit `bounded` from the power plan. A `potion-code` speed claim additionally requires an explicit `quanta_model_dir` so model bytes cannot disappear into unowned cache state.

No real pair has run, so neither the performance authority nor the isolation authority has issued a qualified receipt. Attested capture remains available for diagnostics; an isolated claim fails closed without the versioned Seatbelt proof. A non-macOS isolation backend is not implemented.

Qualified capture now freezes clean retrieval source before staging, re-verifies it after capture, and cross-binds that closure to the contract/SDK receipts, protocol lock and run manifest. Isolated execution uses SHA-bound stage-local adapter/evaluator copies. `QUALITY_DELTA` refuses sub-floor samples and cross-checks paired count, win/loss/tie totals, bootstrap mean/bounds, category/language/repository stratum counts and weighted means, plus no-answer abstention evidence.

The latest adversarial audit found three open RB-05 gates. First, merged record provenance includes absolute input paths, so moving a valid staged pair to the final output path changes its re-derived record/report identity and makes `PAIR_VALID` fail. Second, the verdict accepts individually mutated protocol-lock fields and unknown keys that the producer recorded but the consumer did not independently bind. Third, 1,000 repetitions of only two distinct tasks can pass speed replay even though capture preflight rejects fewer than 20 tasks; phase validation also accepts a raw call longer than its containing phase. These are confirmed fixture reproductions, not real-pair measurements. Pending fixes must retain the shared cold/warm protocol and fail closed on mismatched pins, partial rows and impossible timing.

## Goal

Produce an auditable comparison that separates retrieval quality, chunking effects, index cost and warm-query latency instead of collapsing them into one “performance” score.

## Work

1. Execute all Quanta strategies and Semble on the same pinned repo/query set and declared eligible file universe. Alternate measurement order across repetitions; warm/cold state and model downloads are recorded separately. Prebuild binaries; compilation is not indexing time. The [test plan](TEST-PLAN.md) fixes sample floors and resampling units before measurement. The 20-query pilot is labeled exploratory-only.
2. Measure per phase: file discovery, chunking, model/provider preparation and embedding, SDK publish, seal/activation, first successful query, warm query, index bytes and whole owned process-tree peak RSS. Record host contention, timeouts, drops and typed errors. No `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` evidence is qualified. Measure on the fixed quiet-host profile and apply the check-record rule (CPU identity/settings, concurrent-build absence, thermal/frequency sanity recorded; no single load-average gate). Include an incremental update/rename/delete track separately from full build; T16 is required only when an incremental claim is made.
3. Report paired per-query quality (including category/language and failure cases), Recall/MRR/NDCG/BCY, and p50/p95/p99 with sample sizes. Quality is compared at the same result/context budget; latency is compared only for like-for-like API layers. Provide both common-coverage and native-coverage reports. Keep `PAIR_VALID` (paired protocol/universe validity) separate from a blinded `QUALITY_DELTA`: the latter requires `blinding: isolated` with logged `isolation_method`/`access_block_log`, otherwise the quality claim is attested-only.
4. Measure chunking ablations with the same Quanta model/query settings; distinguish producer effects from Quanta engine changes. An identical SDK transport alone is not proof of identical Semantica producer semantics.
5. Write run manifest, raw records, path-mapping proof artifact (path map plus both-side path+SHA diff; any mismatch fails the common-universe pair), report, and the `verdict.json` artifact per [TEST-PLAN.md](TEST-PLAN.md) §8 (five states, `blinding`/`isolation_method`/`access_block_log`, missing T-IDs, failure class, provenance digests) under an explicit external output directory; bind exact HEAD, binary, corpus, suite, query-pack, model, Semble revision, config and host digests. Mandatory manifest fields include tokenizer/budget version, Semble dependency lockfile digest, and path+SHA diff digest. Do not commit mutable latest-result artifacts as baselines.
6. Identify merged record members by stable capture identity and canonical content digest. Keep host-local paths out of digest-bound record/report bytes. Re-derive every retained protocol-lock pin from frozen authority, enforce a closed schema and reconcile the producer/consumer keys in one change. Entry and replay use one eligibility rule; raw sample sums must fit their monotonic phase windows within a declared tolerance.
7. Validate the stage, promote it atomically, then run the public verdict from a new process at the final path. Require identical re-derived merged record and report digests before/after promotion and refusal after a one-byte raw-record mutation. A stage-only pass is not pair closeout.

## Owner / expected files

- `tools/benchmark/retrieval/run.py`
- `tools/benchmark/retrieval/evaluator.py` (single scoring authority from RB-01; read-only consumer here)
- `tools/benchmark/retrieval/README.md`

`benchmarks/retrieval/src/record.rs` is owned solely by RB-02; this ticket consumes its records read-only and must not edit it.

## Acceptance / verification

- Refuse mixed source, corpus, suite, query pack, model or file universe when issuing a paired verdict. Diagnostic runs remain readable but cannot be labeled authoritative.
- Missing, malformed, error-only or partial route recordings cannot produce a green comparison. Unknown timings remain missing, not zero. A missing mandatory manifest field (tokenizer/budget version, Semble lockfile digest, path+SHA diff digest) fails the verdict.
- [TEST-PLAN.md](TEST-PLAN.md) T12–T13 and measurement sample floors are met before any speed delta is called qualified. T16 applies only to incremental claims.
- One report shows individual query disagreements and chunk spans, not only aggregates; no post-hoc threshold or query cherry-picking.
- Verify deterministic scoring/reporting from frozen records independently of rerunning expensive indexing.
- A successful stage-to-final relocation leaves merged record, report and verdict identities stable; path aliases and raw-record input order do not change them. Any changed content or pin still fails.
- Qualified-speed replay independently enforces the 20-task, one-route, five-root, warmup and 1,000-observation floors and rejects physically impossible raw/phase timing. Existing positive fixtures must use possible durations.
- Every run emits `verdict.json` per [TEST-PLAN.md](TEST-PLAN.md) §8 with five independent states, missing/not-applicable T-IDs, failure class, and provenance digests matching the run manifest.
