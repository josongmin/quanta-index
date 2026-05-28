# LDB-01 — Semantic Generation Layout and Manifest Contract

Status: `done` (2026-05-29)
Parent: [../README.md](../README.md)
Depends on: [LDB-00-truth-freeze-and-backend-decision.md](LDB-00-truth-freeze-and-backend-decision.md)

## 0. Outcome

Implemented in `crates/quanta-index-semantic`: `layout.rs` owns the
generation-local directory shape (`{semantic_root}/{repo}/{revision}/g{gen}/` +
`dataset/` + `semantic-manifest.cbor` + `MARKER_READY` + `MARKER_SEALED`);
`manifest.rs` defines `SemanticManifest` (LDB-01 §3 fields) with a manual
`ciborium` codec and `validate_scope` fail-closed open guard; `dataset.rs`
defines the columnar row shard (LDB-01 §4 data contract). Markers/manifest/
checksum semantics covered by `tests/persisted_semantic.rs` (unsealed →
not-ready, sealed-empty → empty hits, manifest/dataset corruption → fail-closed).

## 1. Purpose

Define the durable semantic generation shape before the adapter writes a single
byte.

## 2. Required durable layout

Semantic generations move to a lexical-like layout:

```text
{state_root}/indexes/semantic/{repo_id}/{revision_id}/g{generation}/
  dataset/...
  semantic-manifest.cbor
  MARKER_READY
  MARKER_SEALED
```

Exact file names may change inside the adapter ticket, but this ticket owns the
shape invariants:

- one directory per `(repo, revision, generation)`
- manifest lives beside the dataset, not in a global shared registry
- readiness/seal markers are explicit and generation-local

## 3. Required manifest fields

Minimal manifest contract:

- `repo_id`
- `revision_id`
- `generation`
- `manifest_digest`
- `model_contract`
- `distance_metric`
- `normalization`
- `row_count`
- `built_at` or equivalent monotonic build marker

This manifest is internal to `quanta-index`, but it is correctness-critical.
Open path must fail closed on manifest mismatch or incomplete state.

## 4. Data contract

Persisted semantic rows must be sufficient to rebuild `LexicalCandidate`
results without auxiliary replay:

- `embedding_id`
- `repo_relative_path`
- `start_line`
- `end_line`
- `snippet`
- vector payload

Optional row-local repo/revision/generation columns are acceptable but not
required if the generation directory itself is the scope authority.

## 5. Work items

- define the manifest struct and marker semantics
- define how `replace_scopes` and `tombstone_scopes` map onto durable dataset
  mutations
- define incomplete-build posture:
  - no `MARKER_READY`
  - no `MARKER_SEALED`
  - open path returns typed not-ready / storage error
- define deletion and empty-generation posture explicitly

## 6. Test plan

- layout tests proving semantic generation directories are generation-local and
  deterministic
- manifest encode/decode tests
- marker tests proving unsealed/incomplete generations fail closed
- sealed-empty generation test proving empty hits are allowed only for an
  explicit sealed-empty generation

## 7. DoD

- durable semantic layout is frozen and generation-local
- manifest fields are explicit enough to validate query-time compatibility
- adapter implementation no longer has to invent marker semantics ad hoc

## 8. Failure modes

- hiding semantic completeness only inside vendor-specific dataset metadata
- storing semantic readiness in a global mutable table that breaks generation
  pinning
- allowing open path to guess around missing markers
