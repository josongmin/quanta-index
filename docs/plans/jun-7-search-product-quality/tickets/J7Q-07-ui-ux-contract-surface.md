# J7Q-07 — UI UX Contract Surface

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Define typed result-contract improvements that downstream UI consumers can use
without inventing search semantics themselves.

## Current Code Fact

- candidate payloads already ship snippets and scores
- explanations already exist
- highlight ranges, snippet offsets, and richer UI-targeted metadata are still
  thin

## Owner Seam

- result DTOs
- SDK consumer surface
- CLI rendering parity

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/query_responses.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/explanation.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/search.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-search-product-quality/COMMAND_AND_ARTIFACT_CONTRACT.md`

## Preferred Implementation Direction

- add typed fields for highlight spans, snippet offsets, and explanation
  sections before adding display sugar
- preserve deterministic semantics across contract, SDK, and CLI layers
- version DTO changes where ambiguity would otherwise appear
- UI consumers should not need regex parsing of free-form strings to render core
  search semantics

## Layer Boundary Clarification

- this ticket owns consumer-visible contract richness
- it does not own frontend rendering policy or product-web layout

## Concrete Work Items

1. Define highlight spans and snippet offsets where needed.
2. Define route/provenance fields useful to consumers.
3. Keep payloads deterministic and typed.
4. Add contract tests and rendering proofs.

## Required Outputs

- stable command:
  - `just rust-verify-quality-ui`
- canonical artifacts:
  - `artifacts/search-quality/ui/latest/summary.json`
  - `artifacts/search-quality/ui/latest/contract_snapshots.json`

## First Increment

- extend typed contracts only where a real consumer need is explicit

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-contract --test ipc_query_result_v2_contract -- --nocapture
./scripts/cargow test -p quanta-index-sdk --lib -- --nocapture
./scripts/cargow test -p quanta-index-searchctl --test cli_smoke -- --nocapture
```

## Worker First Commands

```bash
sed -n '1,260p' crates/quanta-index-contract/src/results/query_responses.rs
sed -n '1,260p' crates/quanta-index-sdk/src/search.rs
sed -n '1,260p' crates/quanta-index-searchctl/src/lib.rs
```

## No-Go

- do not add frontend-only shorthand to the grammar
- do not add opaque UI blobs where typed fields should exist

## Reviewer Rejection Checklist

- reject if a consumer still has to infer highlight anchors from raw snippets
- reject if contract changes are proven only in one layer
- reject if payload richness is delivered as opaque JSON blobs

## DoD

- downstream consumers can render more useful results without semantic guesswork

## Not Done If

- UI-relevant fields remain implicit in snippets and summaries only
- contract changes ship without wire and consumer proof
- consumer-facing contract snapshots are missing
