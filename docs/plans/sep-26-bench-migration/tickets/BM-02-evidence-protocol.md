# BM-02 — Typed benchmark evidence and immutable runs

Status: `PLAN / NOT_RUN`. Priority: P0. Depends on: BM-00. Common gates: [TEST-PLAN](TEST-PLAN.md).

Implementation/verification/qualification verdicts for this ticket are recorded in [CLOSEOUT.md](CLOSEOUT.md) and, where relevant, [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03-DECISION.md](BM-03-DECISION.md) and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md). The original `PLAN / NOT_RUN` status above is the plan-time state, not the closeout state.

## Purpose

Replace the latency-row-shaped universal `BenchArtifactV1` with one common provenance envelope and explicit metric payload types. Make a result reproducible from raw evidence rather than a trusted `pass` flag or `latest` path.

## Work

1. Create `benchmarks/bench-protocol` with Serde types, strict schema/version handling, canonical digest rules, typed error/status, and bounded path validation. Name the new wire contract `BenchmarkEvidenceV1` to avoid conflating it with `BenchArtifactV1`/schema 2 or retrieval suite/runner versions. Define payloads for `micro`, `latency`, `load`, `freshness`, `retrieval`, `agent_outcome`, and `recorded_experiment`; each has its own required raw samples and failure semantics.
2. Bind full Git revision and relevant dirty policy, source closure, Cargo.lock/toolchain/build flags, runner and product binary SHA-256, corpus/view/suite/query/model/config digests, host/OS/arch/CPU/runtime profile, exact command, result boundary and clock, exit/timeout/interruption, and referenced raw artifacts. An absent field is typed `unavailable` with a reason only where the family contract permits it; never silently use zero or `null` as success. Avoid leaking secrets, private home paths and raw credentials in environment capture.
3. Implement fresh staging, immutable run IDs, full referenced-file digest verification, atomic promotion, and advisory `latest` pointer. Baselines point to immutable run IDs and digests. Support safe retention: a referenced admitted baseline and its raw inputs cannot be garbage-collected.
4. Keep historical `BenchArtifactV1` and retrieval runner v3/v4/v5 readers isolated and read-only for replay. No auto-upcaster from historical evidence to current qualification. During migration, compare shared raw metrics but keep one current writer per family.
5. Generate/check one canonical wire schema and cross-language test vectors from the typed contract; a Python scorer can remain independent, but field meanings and canonical bytes may not fork. Separate source/build provenance from evidence authenticity: a locally generated SHA proves byte identity, not independent truth or that a remote product indexed the claimed universe. Record the attestation authority and its limitation.

## DoD

- Round-trip fixtures preserve all fields; unknown version/field, duplicate JSON key, malformed digest, missing or extra raw file, symlink/path escape, reordered samples where order matters, tampered file, wrong binary/source/host/input, partial result, timeout, and repeated run ID are rejected.
- Crash before promotion leaves no admissible partial run. Replaying a promoted run in a fresh process recomputes its verdict byte-for-byte or refuses a changed input.
- Typed payloads cannot be confused: file-only retrieval cannot become a span result; instruction count cannot become wall latency; agent success cannot be inferred from retrieval NDCG.
- The protocol crate has no dependency on product implementation crates or a cloud service. Producer binaries may depend on it as a benchmark-only dependency; shipping binaries must not.

## Verification / exclusions

Use focused Rust unit/property tests, fault-injection around staging/promotion, cross-language canonical JSON vectors, and independent raw-file digest checks. This ticket defines evidence structure; it does not approve relevance gold, set latency budgets, or replace product scorers.
