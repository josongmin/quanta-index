# J7Q-03 — Large Corpus Scale Tiers

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Define reproducible scale tiers and prove search behavior on corpora larger than
current toy and medium fixtures.

## Current Code Fact

- warm/cold benchmark infrastructure exists
- current scale proof is still mostly scenario-fixture sized
- scan-vs-index scaling is documented as exploratory, not a blocking gate

## Owner Seam

- warm runner
- cold runner
- harness authority generator
- benchmark docs

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bench_support.rs`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`

## Preferred Implementation Direction

- generate seeded synthetic corpora with persisted tier manifests
- define per-tier route mix so large-corpus claims are not pathologically narrow
- track ingest, open, reopen, query, restart, and memory as separate budgets
- manifests should record repo count, file-size distribution, hit density, and
  symbol density

## Layer Boundary Clarification

- this ticket owns reproducible scale evidence
- it does not own route-specific tail hardening policy from `J7Q-04`

## Concrete Work Items

1. Define `small`, `medium`, `large`, `xlarge` corpus tiers.
2. Add a synthetic authority generator for those tiers.
3. Measure ingest, open, reopen, query, restart, and memory footprint.
4. Record current scale limits honestly.

## First Increment

- land one deterministic large synthetic corpus tier

## Red Rail To Pin First

```bash
just rust-bench-dsl-truth
just rust-bench-dsl-warm
```

## Worker First Commands

```bash
sed -n '1,260p' tools/benchmark/README.md
sed -n '1,260p' crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs
sed -n '1,260p' crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs
```

## No-Go

- do not claim scale from toy fixtures
- do not mix exploratory scaling notes with blocking tier gates
- do not report one large-corpus number without its tier manifest

## Reviewer Rejection Checklist

- reject if “large” is not tied to a manifest and seed
- reject if one local corpus snapshot is used as a universal scale claim
- reject if tier results cannot be rerun on another machine with the same input

## DoD

- reproducible scale tiers exist and are measurable

## Not Done If

- large-corpus behavior is still inferred from small fixtures
- tier boundaries are not documented
