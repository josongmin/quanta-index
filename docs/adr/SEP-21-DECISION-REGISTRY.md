# SEP-21 Frozen Decision Registry

Status: `Accepted`

Decided: 2026-09-21

This table is the compact implementation contract for S21-01 through S21-13. The four linked ADRs own rationale and
full semantics.

| ID | Frozen value | Owner | Blocking consumers |
|---|---|---|---|
| D-ID-01 | UTF-8 already-NFC, case-sensitive, 1..=512 bytes, controls rejected | contract-base IDs | S21-01 |
| D-ID-02 | `u32_be(domain byte length) || domain || payload`; distinct repository/logical domains; typed 32-byte SHA-256 | contract-base digest owner | S21-01/02/06/12 |
| D-ID-03 | `%`, `/`, `.` and dot-sequence identifiers are valid logical bytes but never path components | contract-base IDs | S21-01/11 |
| D-CAND-01 | exact integer-key `ArtifactIdentityV1`/candidate envelope; commitment is domain-framed envelope digest, object address is raw-byte digest | contract/RepoMap | S21-01/02/03/12 |
| D-QUAR-01 | exact integer-key incident envelope; P02B global sequence; P03 catalog-first projection/unlink crash protocol | catalog/RepoMap | S21-01/02/04/11 |
| D-LAYOUT-01 | `objects/sha256/aa/bb/<60hex>.cbor`; 0700 dirs, 0600 files, effective-UID/no-follow/inode/mode/nlink checks | RepoMap/runtime | S21-01/09/11 |
| D-AUTH-01 | SQLite candidate/activation ledger is sole RepoMap visibility authority | catalog | S21-02/05/11 |
| D-AUTH-02 | process-global `MutationCoordinatorV1`, prepared plan and fence | core/catalog/runtime | S21-04/09 |
| D-REC-01 | `OperationTerminalResultV2`, canonical CBOR v2, immutable replay bytes | contract/catalog | S21-04/11/12 |
| D-REC-02 | replay floor 1; terminal retention for root lifetime; offline floor advance only | catalog/migration | S21-04/11 |
| D-SEQ-01 | one state-root-global `1..=i64::MAX` sequence plus generic `catalog_sequence_event_v2`; allocator/event/domain row commit atomically; exhaustion refuses | catalog | S21-02/04/11 |
| D-READ-01 | one domain evidence each; handles held by `QueryReadViewV2` | search-plane/core | S21-05/06 |
| D-QUERY-01 | explicit exact/lower-bound/capped/interrupted/approximate outcomes | contract/core | S21-06/07 |
| D-CURSOR-01 | signed stateless HMAC cursor, 15m default/1h max, no unsigned decoder | contract/runtime | S21-06/07/11 |
| D-FOCUS-01 | non-empty strict resolution; unresolved refusal; no global fallback | RepoMap | S21-03/06 |
| D-EGRESS-01 | remote default-deny; query/source grants separate; complete profile mandatory | semantic/runtime | S21-08/13 |
| D-PROC-01 | supervisor owns all work/guards; 125s hard deadline; exits 0/70/128+signal | runtime | S21-09/10 |
| D-READY-01 | process readiness separate from repository/generation status | runtime/control | S21-10 |
| D-ROOT-01 | state-root format 2; offline-only migration; manifest-last atomic cutover | runtime/migration | S21-11 |
| D-XREPO-01 | protocol v2+contract digest handshake; breaking producer cutover | contract/deployment | S21-12 |
| D-PROOF-01 | strict registered proof graph, same binary/host/source binding | CI/release | S21-13 |
| D-PROOF-02 | source/source-pair domain digest plus manifest digest address indexed immutable receipt leaves; evidence/binaries are content-addressed; dependency edges reference exact archive leaves and never current aliases | CI/release | S21-00/13 |
| D-LANE-01 | `P00 → P01A → (P02A ∥ P02B) → P02I → P03 → P04…P11 → P12A → P12Q`; no other implementation parallelism | plan owner | all |
| D-HANDOFF-01 | each lane handoff binds canonical lane/ticket/proof/status, exact Git write set and current-clean source; P02I validates both parallel parents; P12A implements and P12Q executes transitive validation | plan/release | all |
| D-AGG-01 | P12A owns aggregate schema/writer/validator/final recipe; P12Q is source-read-only qualification and alone may issue terminal P12 proof | CI/release | S21-13 |

## Canonical type names

- `RepositoryRevisionIdentityV1`
- `LogicalGenerationIdentityV1`
- `CandidateCommitmentV1`
- `ActivationIdentityV1`
- `OperationTerminalResultV2`
- `PreparedMutationV1`
- `DomainReadEvidenceV2`
- `QueryReadViewV2`
- `ExecutionOutcomeV2`
- `CursorEnvelopeV2`
- `ResolutionProofV1`
- `DispatchContextV1`
- `ProcessReadinessV1`
- `MutationCoordinatorV1`
- `ProofManifestV1`

## Stable error-code authority

P00 freezes the migration structure and a source-bound baseline inventory, not a fictional final table while current
production producers remain free-form. Earlier static review observed 245 `CoreError::Typed` constructors across 75
files and 8 crates, 141 code-expression shapes, 88 existing `LexicalErrorCode` variants, at least 12 dynamic
synthesis/pass-through sites, 13 query-metric substring classifications, three dispatcher pass-throughs and stale
`BAD_REQUEST` decoder success; those numbers are discovery context, not frozen authority.

The reproducible P00 discovery baseline is `ErrorAuthorityInventoryV1`:

- schema: `tools/ci/error-authority-inventory.schema.json`;
- writer: `python3 tools/ci/write-error-authority-inventory.py` or `just proof-error-authority-inventory`;
- artifact: `artifacts/sep-21/p00/error-authority-inventory.json`;
- source scope: every `crates/*/src/**/*.rs` byte, sorted by repo-relative path;
- source digest: general SEP-21 framing with domain `quanta-index/error-authority-source/v1`; payload is, for each
  sorted file, `u32_be(path_len) || path_utf8 || u64_be(content_len) || raw_content`;
- regex matches are candidate discovery only and can never issue semantic closure.

P00 handoff records the artifact SHA-256, source digest, category counts and mandatory `closed=false`. The staged
`just proof-error-authority-closed` command deliberately fails. P01A must replace that rail with executable
enum/table/mapping/SDK validators plus registered owner-local tests; it must not turn regex counts into authority.

P01A owns the one-time migration to closed `SearchPlaneErrorCodeV2`: `ALL`, unique `as_wire_str`, exact
`from_wire_str`, manual serde, an exhaustive generic-core mapping and SDK preservation. Existing
`LexicalErrorCode` is a nested variant rendered as its existing flat wire code. `Unknown(String)`, `Other(String)`,
dynamic code synthesis, `&str` pass-through, substring/equality classification and unknown decoder success are
forbidden. P01A must generate and commit the complete accepted-code table artifact, cardinality and digest from the
migrated source. The schema is `tools/ci/search-plane-error-code-table.schema.json`; the committed artifact is
`tools/ci/inventory/search-plane-error-codes.json`; its file SHA-256 is the canonical table digest. `codes` is sorted
by wire bytes and `cardinality == len(codes)`. `enum_source_path` is exactly
`crates/quanta-index-contract/src/ipc/error.rs`. `enum_source_digest` uses the general framing with domain
`quanta-index/search-plane-error-enum-source/v2` and payload
`u32_be(path_len) || path_utf8 || u64_be(content_len) || raw_file_bytes`. P01A's validator must recompute that digest,
enforce sort/cardinality/table↔`ALL` equality, and run the exhaustive mapping/serde/SDK tests. Its handoff is invalid
while any free-form production path remains.

After P01A handoff, downstream lanes cannot add or change a wire error code. A newly discovered code requirement is
`BLOCKED` and returns to the P01A/P02I contract owner; P03-P12 may only consume the frozen table and digest.

Mandatory reserved/new codes include:

- `IDENTITY_NON_CANONICAL`
- `IDENTITY_EMPTY`
- `IDENTITY_TOO_LONG`
- `SEQUENCE_EXHAUSTED`
- `CANDIDATE_COMMITMENT_CONFLICT`
- `ACTIVATION_CAS_CONFLICT`
- `ACTIVATION_TARGET_NOT_SEALED`
- `OPERATION_FENCE_LOST`
- `OPERATION_REPLAY_FLOOR`
- `STATE_ROOT_FORMAT_UNSUPPORTED`
- `STATE_ROOT_SECURITY_POLICY_UNSUPPORTED`
- `CURSOR_INVALID`
- `CURSOR_EXPIRED`
- `CURSOR_CONTEXT_MISMATCH`
- `FOCUS_SUBJECT_NOT_FOUND`
- `PROVIDER_EGRESS_DENIED`
- `PROTOCOL_VERSION_UNSUPPORTED`
- `PROCESS_NOT_READY`

## Compatibility matrix

| Pair | Result | Mutation |
|---|---|---|
| old producer / V2 daemon | `PROTOCOL_VERSION_UNSUPPORTED` before body decode | 0 |
| V2 producer / old daemon | producer handshake refusal | 0 |
| V2 daemon / V1 root | `STATE_ROOT_FORMAT_UNSUPPORTED`; rebuild from current typed producer input | 0 |
| old daemon / V2 root | deployment fence; pair must never be started | 0 |
| unsigned/V1 cursor / V2 daemon | `CURSOR_INVALID` | 0 |
| persisted receipt v1 / V2 runtime | runtime refusal; no old-receipt importer | 0 |
| same operation/body after terminal result | exact persisted result replay | 0 additional |
| same operation key / different body | conflict | 0 |

## Proof and fixture names

- `identity_tuple_collision_v1`
- `receipt_v1_to_terminal_v2`
- `state_root_v1_to_v2_clean`
- `state_root_v1_collision_refused`
- `state_root_v2_old_binary_refused`
- `cursor_v1_unsigned_refused`
- `operation_replay_floor_v1`
- `cross_repo_protocol_v1_v2_refused`
- `state_root_two_process_lease_v2`

No decision in this registry has a compatibility shim or optional transitional field.
