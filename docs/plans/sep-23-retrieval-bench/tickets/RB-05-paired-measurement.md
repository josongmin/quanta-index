# RB-05 — Paired Quality, Speed and Resource Measurement

Status: `orchestration-and-perf-authority-landed / measurement-blocked`

Depends on: RB-01, RB-02, RB-03, RB-04; requires RB-00 stage B measurement-entry gate

Owner: orchestration/reporting, not product ranking

## Current code status (2026-09-24)

`run.py` implements immutable input/receipt staging, sequential and alternating Quanta/Semble repetitions, record merge, manifest construction, latency-matrix re-derivation, deterministic report validation and the five-state verdict machine. Provenance and tamper mutants are covered by focused tests. Phase evidence now separates discovery, chunk/index, model/provider preparation, publish/seal/activation, first query, remaining warm queries and unattributed time. Resource evidence includes aggregate plus per-process peak RSS/CPU, index/model/parser/embedding-cache bytes, discovered files, indexed chunks and disk-vs-memory ownership. The verdict validates the closed schemas, phase sums, record bindings and metric completeness before `PERF_QUALIFIED` can pass. For isolated quality on macOS, the suite remains evaluator-only, the original checkout is denied in full, and both runners consume a Git-free materialized directory containing exactly the manifest-admitted path/SHA universe. The verdict re-enumerates that corpus and rebinds the Seatbelt policy, proof, resources and runner records before `QUALITY_DELTA` can pass.

The prior free-form `host_profile` label is no longer authority. `host-profile` emits a frozen JSON fingerprint covering OS/CPU/toolchain and the measured power configuration; `pair` stages it as an artifact, and the verdict requires both host probes to match it with clean thermal/frequency/power and contention state. A `potion-code` speed claim additionally requires an explicit `quanta_model_dir` so model bytes cannot disappear into unowned cache state.

No real pair has run, so neither the performance authority nor the isolation authority has issued a qualified receipt. Attested capture remains available for diagnostics; an isolated claim fails closed without the versioned Seatbelt proof. A non-macOS isolation backend is not implemented.

## Goal

Produce an auditable comparison that separates retrieval quality, chunking effects, index cost and warm-query latency instead of collapsing them into one “performance” score.

## Work

1. Execute all Quanta strategies and Semble on the same pinned repo/query set and declared eligible file universe. Alternate measurement order across repetitions; warm/cold state and model downloads are recorded separately. Prebuild binaries; compilation is not indexing time. The [test plan](TEST-PLAN.md) fixes sample floors and resampling units before measurement. The 20-query pilot is labeled exploratory-only.
2. Measure per phase: file discovery, chunking, model/provider preparation and embedding, SDK publish, seal/activation, first successful query, warm query, index bytes and whole owned process-tree peak RSS. Record host contention, timeouts, drops and typed errors. No `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` evidence is qualified. Measure on the fixed quiet-host profile and apply the check-record rule (CPU identity/settings, concurrent-build absence, thermal/frequency sanity recorded; no single load-average gate). Include an incremental update/rename/delete track separately from full build; T16 is required only when an incremental claim is made.
3. Report paired per-query quality (including category/language and failure cases), Recall/MRR/NDCG/BCY, and p50/p95/p99 with sample sizes. Quality is compared at the same result/context budget; latency is compared only for like-for-like API layers. Provide both common-coverage and native-coverage reports. Keep `PAIR_VALID` (paired protocol/universe validity) separate from a blinded `QUALITY_DELTA`: the latter requires `blinding: isolated` with logged `isolation_method`/`access_block_log`, otherwise the quality claim is attested-only.
4. Measure chunking ablations with the same Quanta model/query settings; distinguish producer effects from Quanta engine changes. An identical SDK transport alone is not proof of identical Semantica producer semantics.
5. Write run manifest, raw records, path-mapping proof artifact (path map plus both-side path+SHA diff; any mismatch fails the common-universe pair), report, and the `verdict.json` artifact per [TEST-PLAN.md](TEST-PLAN.md) §8 (five states, `blinding`/`isolation_method`/`access_block_log`, missing T-IDs, failure class, provenance digests) under an explicit external output directory; bind exact HEAD, binary, corpus, suite, query-pack, model, Semble revision, config and host digests. Mandatory manifest fields include tokenizer/budget version, Semble dependency lockfile digest, and path+SHA diff digest. Do not commit mutable latest-result artifacts as baselines.

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
- Every run emits `verdict.json` per [TEST-PLAN.md](TEST-PLAN.md) §8 with five independent states, missing/not-applicable T-IDs, failure class, and provenance digests matching the run manifest.
