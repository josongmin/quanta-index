# BM common test, evidence, and cutover contract

Status: **PLAN / NOT_RUN**. The commands and target package names introduced by BM tickets are planned interfaces, not currently available commands. Existing commands named below are source anchors, not claims that they passed on this revision.

## 1. Evidence states and identity

Every claim must be one of `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN`, or `NOT_APPLICABLE`. `VERIFIED` requires exact source revision and relevant dirty state; input/corpus/suite/config/model/dependency/toolchain and binary identity; exact command and raw result; output path and digest; covered/excluded scope. A source or bound-input change invalidates the receipt. A dirty local iteration may be useful but cannot be promoted to clean-source performance authority.

The new protocol has one common **envelope** and typed `micro`, `latency`, `load`, `freshness`, `retrieval`, `agent_outcome`, and `recorded_experiment` payloads. This is a new `BenchmarkEvidenceV1` contract; its version is independent of existing `BenchArtifactV1` with `schema_version: 2`, retrieval suite v3, and runner v5. Required envelope fields: protocol version, family/profile/case IDs, source closure, build/toolchain/lockfile/binary digests, input and output digests, environment and host policy, command/exit/timeout/interruption, raw-result references, measurement boundary, and verdict scope. A payload may add fields but may not redefine those identities. Unknown, missing, duplicated, stale, wrong-source, wrong-host, reordered, partial, forged, or tampered evidence fails closed. A digest of a self-authored `pass` summary is not independent proof. Keep one canonical wire schema/serializer and cross-language conformance vectors; Python and Rust may independently validate but must not invent divergent field semantics.

Run outputs are immutable: `<external-root>/runs/<run-id>/{manifest,raw,normalized,verdict}`. Write to a fresh staging directory, validate all referenced files, atomically promote, then update an advisory `latest` pointer. Never overwrite a run ID or use `latest` as a baseline. Retention/GC must preserve every admitted baseline and referenced raw input.

Status has two axes: `migration_infrastructure` (registered/executable/replayable) and `benchmark_claim` (diagnostic/quality-qualified/performance-qualified/blocked/not-run). A valid fixture replay proves the first axis only. Qualification also needs the corresponding independent gold, product index-universe proof, approved baseline and/or quiet-host execution. Evidence validation, metric comparison, baseline admission and release/deployment approval are distinct actions.

## 2. Independent oracles

| Question | Oracle | Must not be inferred from |
| --- | --- | --- |
| Rust hot-path correctness | Fixed fixture, property/invariant, or public API result | Criterion timing |
| Product query behavior | Existing scenario truth and public SDK/daemon response | Presence of a latency row |
| Exact lexical match | Pinned corpus bytes plus independently computed literal/regex truth | The tested index's own result |
| Developer search relevance | Independently adjudicated file/line/span qrels and frozen corpus view | Product top-10 or mechanically generated labels alone |
| Freshness | Before/after source bytes, activation/generation, and observable query results | `update_ms` alone |
| Agent outcome | Frozen task/baseline tests and raw trajectory/test receipts | A submitted aggregate boolean |

For incomplete pooled relevance judgments, keep `unjudged` distinct from `irrelevant`; report pool coverage and sensitivity. File-only captures cannot be cast into whole-file spans to obtain span-density credit. Per-product query transformation, ranking semantics, and indexed/searchable universe are explicit; incomparable products remain diagnostic.

## 3. Measurement protocol

- Correctness is a precondition to timing. Record errors, timeouts, drops, partial results, query identity, and output cardinality; do not silently filter failures from latency samples.
- Microbench: isolate setup from measured operation, use `black_box`/representative input sizes, retain raw Criterion output and case identity. Optional Iai-Callgrind is Linux-only work/cost diagnosis, not wall-clock speed qualification.
- Single-request daemon latency: define client start/end boundary, warm/cold/cache/root state, top-k, result shape, and host. Interleave baseline/candidate in blocked order and keep raw repetitions. Statistical unit is the independent block/root/query as appropriate, not every request as an independent experiment.
- Timing records include monotonic clock source/resolution, serialization/transport inclusion, warmup, CPU governor/power/thermal state when measurable, compiler profile and instrumentation mode. A preflight snapshot alone does not prove the host stayed quiet: reserve an exclusive host lease, monitor interference during capture, and reject a violated lease or unobserved interval from performance qualification.
- Offered load: predeclared arrival process/rates, offered/completed/dropped/timeout counts, latency distribution, load-generator saturation, RSS and disk. Closed-loop throughput does not substitute for open-loop capacity.
- Ingest/freshness: separate full build, fresh-root time-to-searchable, edit/add/delete/rename, reopen/restart and replay. Record index bytes/write amplification and stale-hit rate.
- Retrieval: same corpus view and query pack for every product in a comparison; per-repo and intent-stratum outcomes before aggregates. Keep lexical file metrics separate from RB span/context and agent outcome. Selection and thresholds are frozen before holdout; do not retune on the final holdout.
- Product comparison has two explicitly different lanes: `native_default` compares the product a developer would use; `controlled_mechanism` aligns filters, candidate depth and ranking where possible. Do not average their scores or timings. Remote/loopback, native/emulated architecture, warm index/cache and response-completion semantics must be reported separately. A product lacking equivalent query support or index-universe/rank proof is diagnostic for that stratum.
- Baseline: immutable, approved run ID + digest + compatible source/environment/corpus/config scope. Predeclare regression margin and uncertainty method; missing/incompatible baseline is `BLOCKED` or `NOT_RUN`, not a zero-regression pass.
- For multiple candidate changes, lock the candidate set and primary metric before holdout. Use paired comparisons with uncertainty that respects repository/query/root clusters; report every predeclared stratum and leave-one-repository-out sensitivity. Do not convert a small number of repositories or repeated requests on one root into many independent samples.

## 4. Verification ladder

1. **Contract/local:** schema/Serde round trips; exact registry inventory; mutation fixtures for missing/extra/duplicate/stale/wrong-source/wrong-host/partial/timeout/tamper; package dependency-direction check. Dirty checkout results are local diagnostics.
2. **Producer integration:** at least one real daemon/SDK run, one crate-local microbench, one Python adapter, and one recorded-only evaluator through the new CLI; independent raw-result re-derivation.
3. **Cutover parity:** old and new validators inspect the *same frozen raw inputs* and agree on every shared metric/verdict. Schema-inexpressible metrics are explicitly excluded, never filled with defaults. A disagreement blocks cutover and gets a root-cause test.
4. **Clean-source replay:** exact snapshot, source closure, binary and external input digests frozen; fresh process validates immutable run from raw evidence and reproduces the verdict. CI job and local CLI call the same registry path.
5. **Qualified measurement:** only the profile's declared host/input/gold and sample floor can issue quality or performance status. A focused test or replay alone does not satisfy this step. If external inputs are absent, infrastructure can be `VERIFIED` while the corresponding benchmark claim stays `BLOCKED` or `NOT_RUN`.

Current anchors include `just benchmark-prep-local`, `just retrieval-contract-local`, `just retrieval-contract-proof <external-root>`, `just retrieval-sdk-proof <external-root>`, `python3 tools/benchmark/benchctl.py list`, and `./scripts/cargow --lane bench-lane bench --workspace --all-features --locked --no-run`. Select owning checks first; run the full workspace/quiet-host rail only after shared writers and source are stable. Do not treat the compile-only command as a performance result.

## 5. Final DoD for the migration packet

- One registry enumerates every timing producer, recorded evaluator, metric owner, CI job, baseline policy, host policy, source/input closure and evidence type. An unregistered producer/bench target or a second live comparator path fails policy.
- Production crates have no normal dependency on benchmark-only protocol/CLI crates. Existing public API behavior and test-harness consumers survive layout changes.
- All current **claims** use the typed envelope and immutable run directories, which reference unmodified native raw files (for example Criterion output or adapter response bytes). Old artifacts are readable only through an explicitly named historical replay path, never auto-upcast into qualification.
- CI, Justfile, docs and registry select the same commands and profiles. Mutation tests prove wrong-source/dirty/host/binary/corpus/suite/partial-result refusals.
- Each migrated producer type has a clean-source representative real run where its prerequisites are available; recorded-only and external-input families have complete fixed-fixture replay and explicit missing-input status. Every result has path/digest/raw evidence and exclusions. This is **migration infrastructure DoD**, not qualified external retrieval/agent outcome.
- A qualified benchmark claim additionally requires that family's admitted real input, independent oracle, host and sample floor. Missing external approval/gold/quiet host is reported as `BLOCKED` or `NOT_RUN` on this second axis only; no synthetic stand-in or false `VERIFIED` is permitted.
