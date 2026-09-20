# SEP-21 Frozen Decision Registry

Status: `Accepted`

Decided: 2026-09-21

This table is the compact implementation contract for S21-01 through S21-13. The four linked ADRs own rationale and
full semantics.

| ID | Frozen value | Owner | Blocking consumers |
|---|---|---|---|
| D-ID-01 | UTF-8 already-NFC, case-sensitive, 1..=512 bytes, controls rejected | contract-base IDs | S21-01 |
| D-ID-02 | length-delimited BE canonical tuples; domain-separated SHA-256 | contract-base digest owner | S21-01/02/06/12 |
| D-LAYOUT-01 | `objects/sha256/aa/bb/<60hex>.cbor`; 0700 dirs, 0600 files, effective-UID/no-follow/inode/mode/nlink checks | RepoMap/runtime | S21-01/09/11 |
| D-AUTH-01 | SQLite candidate/activation ledger is sole RepoMap visibility authority | catalog | S21-02/05/11 |
| D-AUTH-02 | process-global `MutationCoordinatorV1`, prepared plan and fence | core/catalog/runtime | S21-04/09 |
| D-REC-01 | `OperationTerminalResultV2`, canonical CBOR v2, immutable replay bytes | contract/catalog | S21-04/11/12 |
| D-REC-02 | replay floor 1; terminal retention for root lifetime; offline floor advance only | catalog/migration | S21-04/11 |
| D-SEQ-01 | one state-root-global positive sequence from transactional `catalog_sequence_v2` | catalog | S21-02/04/11 |
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

S21-01 introduces closed `SearchPlaneErrorCodeV2`; messages are non-authoritative. Mandatory new codes:

- `IDENTITY_NON_CANONICAL`
- `IDENTITY_TOO_LONG`
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
| V2 daemon / V1 root | `STATE_ROOT_FORMAT_UNSUPPORTED`; offline importer required | 0 |
| old daemon / V2 root | deployment fence; pair must never be started | 0 |
| unsigned/V1 cursor / V2 daemon | `CURSOR_INVALID` | 0 |
| persisted receipt v1 / V2 runtime | runtime refusal; offline migration input only | 0 |
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
