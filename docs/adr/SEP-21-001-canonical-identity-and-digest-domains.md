# SEP-21-001 — Canonical Identity and Digest Domains

Status: `Accepted`

Decided: 2026-09-21

Amended: 2026-09-21 — exact framing, canonical schemas and split ownership were added before any V1 product writer
was released. No earlier or alternative V1 byte form is accepted.

Gate owner: S21-00; blocks S21-01, S21-02, S21-06, S21-07 and S21-12

## Context

`RepoId` and `RevisionId` currently accept arbitrary strings, while RepoMap persistence joins escaped components
with `--`. Logical generation, content identity, storage address and active serve identity are not separate types.

## Decision

### Identifier policy

- wire representation is UTF-8 text that is already NFC;
- decoding never silently normalizes;
- comparison is case-sensitive and byte-exact after validation;
- length is `1..=512` UTF-8 bytes;
- NUL, C0 and C1 control characters are rejected;
- `%`, `/`, `.` and the exact strings `.` and `..` are valid logical identifier bytes when the other rules pass;
  they are never decoded as escapes or projected into a filesystem component;
- non-NFC input returns `IDENTITY_NON_CANONICAL`;
- empty input returns `IDENTITY_EMPTY`; over-limit input returns `IDENTITY_TOO_LONG`.

The 512-byte bound is product policy, not a filesystem limit. The producer must reject or map an external identifier
before it reaches this contract; the search plane does not truncate it.

### Digest framing and canonical tuples

Every domain-separated digest uses exactly this framing, without a trailing delimiter or terminating NUL:

```text
u32_be(byte_length(domain_utf8)) || domain_utf8 || payload
```

Domain text is ASCII and the length is its UTF-8 byte length. Internal digest values are exactly 32 bytes. The only
text rendering is lowercase `sha256:` followed by exactly 64 lowercase hexadecimal characters; uppercase, bare hex
and algorithm aliases fail decoding.

`RepositoryRevisionIdentityV1` uses domain `quanta-index/repository-revision/v1` and payload:

```text
u32_be(len(repo_utf8)) || repo_utf8
u32_be(len(revision_utf8)) || revision_utf8
```

`LogicalGenerationIdentityV1` does not append to already domain-framed repository-revision bytes. It uses distinct
domain `quanta-index/logical-generation/v1` and payload:

```text
u32_be(len(repo_utf8)) || repo_utf8
u32_be(len(revision_utf8)) || revision_utf8
u64_be(generation)
```

Generation is unsigned and zero is valid unless a consuming protocol explicitly forbids it.

### Exact canonical CBOR schemas

All maps below use definite lengths, keys in ascending numeric order, shortest integer encodings, text as validated
UTF-8 NFC, and byte strings at the exact stated width. Unknown, missing, duplicate, out-of-order or wrong-typed keys,
indefinite items, floats and CBOR tags are rejected. A decoder byte-compares canonical re-encoding before use. `null`
is allowed only where stated.

`ArtifactIdentityV1` is the seven-entry map:

| Key | Type | Value |
|---|---|---|
| 0 | `uint` | format version, exactly `1` |
| 1 | `tstr` | validated repository ID |
| 2 | `tstr` | validated revision ID |
| 3 | `uint` | logical generation, `u64` |
| 4 | `tstr` | exactly `repomap.compiled.v1`; this is the sole V1 artifact-domain token |
| 5 | `bstr(32)` | artifact content SHA-256 |
| 6 | `uint` | artifact byte size, `u64` |

Its commitment is SHA-256 of the general framing with domain `quanta-index/artifact-identity/v1` and payload equal
to those exact CBOR bytes.

`RepoMapCandidateEnvelopeV1` is the eleven-entry map and is itself the immutable candidate object:

| Key | Type | Value |
|---|---|---|
| 0 | `uint` | format version, exactly `1` |
| 1 | `tstr` | validated repository ID |
| 2 | `tstr` | validated revision ID |
| 3 | `uint` | logical generation, `u64` |
| 4 | `bstr(32)` | producer manifest commitment |
| 5 | `bstr(32)` | producer authority commitment |
| 6 | `bstr(32)` | compiled graph commitment |
| 7 | `bstr(32)` | schema commitment |
| 8 | `bstr(32)` | projection-profile commitment |
| 9 | `array(1)` | exactly one `ArtifactIdentityV1`, for domain `repomap.compiled.v1` |
| 10 | `bstr` | compiled RepoMap payload; empty only when the compiler contract permits an empty graph |

The artifact entry is not descriptive metadata. Its keys 1, 2 and 3 must equal envelope keys 1, 2 and 3; key 5
must equal raw SHA-256 of key 10; key 6 must equal the exact byte length of key 10. Empty, multiple, stale,
cross-identity or payload-unbound artifact inventories are non-canonical and refused before commitment calculation.

`CandidateCommitmentV1` is SHA-256 of the general framing with domain
`quanta-index/repomap-candidate-commitment/v1` and payload equal to the exact candidate-envelope bytes. The physical
`CandidateObjectDigestV1` is plain SHA-256 of those same bytes. These are different types and are never substituted.

`QuarantineReasonCodeV1` is the closed integer set below. No free-form reason, catch-all `other` value or unknown-code
success path exists.

| Code | Meaning |
|---|---|
| 1 | raw payload digest does not match the digest encoded by the object address |
| 2 | envelope decodes but is not the exact canonical byte encoding |
| 3 | envelope cannot be decoded under its declared schema |
| 4 | decoded logical identity or commitment does not match the expected catalog/address identity |
| 5 | unsafe filesystem metadata or an inode/device swap was observed |
| 6 | a symlink was encountered at the leaf or any traversed component |
| 7 | a hardlink or `nlink != 1` was observed |
| 8 | secure open or payload read was unavailable for an I/O reason not classified by codes 5, 6 or 7 |
| 9 | the source address/path grammar is non-canonical or escapes the allowed root |
| 10 | the declared persisted format/version is unsupported by live runtime |

State-root creation generates one RFC 4122 UUID and persists its 16 network-order bytes in the format-V2 root
manifest. `StateRootUuidCommitmentV1` is SHA-256 of the general framing with domain
`quanta-index/state-root-uuid/v1` and payload equal to those exact 16 bytes. Text UUID spellings are never digest
input.

`QuarantineObservationEvidenceV1` is the seven-entry canonical map used only as a stable observation commitment:

| Key | Type | Value |
|---|---|---|
| 0 | `uint` | format version, exactly `1` |
| 1 | `array<bstr>` | exact observed state-root-relative raw path-component bytes, including invalid empty/dot/NUL/slash-bearing components when reason 9 records grammar failure |
| 2 | `uint` or `null` | observed byte size, `u64`; null only when secure metadata acquisition never succeeded |
| 3 | `bstr(32)` or `null` | raw payload SHA-256; null when secure policy forbids reading or bytes are unreadable |
| 4 | `bstr(32)` or `null` | digest encoded by the original address; null only when no canonical address existed |
| 5 | `uint` | exact `QuarantineReasonCodeV1` value |
| 6 | `bstr(32)` | state-root UUID commitment |

The observation-evidence digest is SHA-256 of the general framing with domain
`quanta-index/quarantine-evidence/v1` and payload equal to those exact canonical map bytes. It deliberately excludes
catalog sequence and observation time: retries of the same catalog incident reuse one observation commitment, while
the enclosing incident binds the once-allocated sequence/time. Incident keys `3,4,5,6,7,9` MUST equal evidence keys
`1,2,3,4,5,6` respectively; an implementation cannot supply an opaque evidence digest detached from the incident
fields.

`QuarantineIncidentV1` is the ten-entry map:

| Key | Type | Value |
|---|---|---|
| 0 | `uint` | format version, exactly `1` |
| 1 | `uint` | state-root-global sequence from `catalog_sequence_v2`, exact range `1..=i64::MAX` |
| 2 | `uint` | observation time as Unix nanoseconds, captured once with the catalog event |
| 3 | `array<bstr>` | exact observed state-root-relative raw path-component bytes; evidence only, never a traversal input |
| 4 | `uint` or `null` | observed byte size, `u64`; null only when secure metadata acquisition never succeeded |
| 5 | `bstr(32)` or `null` | raw payload SHA-256; null when secure policy forbids reading or bytes are unreadable |
| 6 | `bstr(32)` or `null` | digest encoded by the original address; null only when none existed |
| 7 | `uint` | exact `QuarantineReasonCodeV1` value |
| 8 | `bstr(32)` | digest of the exact `QuarantineObservationEvidenceV1` bytes defined above |
| 9 | `bstr(32)` | state-root UUID commitment, preventing cross-root incident aliasing |

The incident address is SHA-256 of the general framing with domain `quanta-index/quarantine-incident/v1` and
payload equal to the exact incident-envelope bytes. Timestamp, sequence and envelope are allocated once in the P03
catalog transaction and reused byte-for-byte after a crash; retry never mints a new time or sequence.

Raw path evidence uses Unix `OsStrExt::as_bytes` without UTF-8 conversion and preserves the parser's exact component
segmentation. Empty, NUL/slash-bearing and exact `.`/`..` components are allowed in evidence specifically so reason
9 can record the invalid original without loss; these bytes are never joined, reopened or used for traversal.
The separate secure traversal validator accepts only a non-empty array of non-empty components and rejects NUL,
`/`, exact `.` and exact `..`. A platform that cannot capture the original and safely reopen a validated relative
path losslessly refuses before mutation with `STATE_ROOT_SECURITY_POLICY_UNSUPPORTED`. When multiple observations
fail, the reason is the first failing stage in this fixed order: raw relative-path grammar (`9`), no-follow
traversal/symlink (`6`), secure open I/O availability (`8`), hardlink count (`7`), remaining
metadata/inode/device policy (`5`), payload read I/O availability (`8`), address grammar (`9`), persisted version
(`10`), raw address digest (`1`), envelope decode (`3`), canonical re-encode (`2`), logical identity/commitment
(`4`). Later failures are not evaluated and cannot replace the recorded reason.

### Physical layout

The candidate object grammar is exact:

```text
objects/sha256/<hex[0:2]>/<hex[2:4]>/<hex[4:64]>.cbor
```

`hex` is the lowercase 64-character `CandidateObjectDigestV1`. Every component is ASCII; fanout components are
exactly two characters and the leaf is exactly 60 characters plus `.cbor`. No alternate extension, uppercase form,
human identifier, separator escaping or adjacent metadata file is accepted by live runtime.

Quarantine envelope addresses use the typed incident digest under
`quarantine/incidents/sha256/<hex[0:2]>/<hex[2:4]>/<hex[4:64]>.cbor`. Readable raw payloads use plain SHA-256 under
`quarantine/payloads/sha256/<hex[0:2]>/<hex[2:4]>/<hex[4:64]>.bin`. Incident and payload digests are not
interchangeable.

### Identity separation

- logical key: `{repo_id, revision_id, generation}`;
- candidate commitment: domain-separated digest of the exact canonical candidate envelope;
- physical object address: raw content digest of exact immutable candidate bytes;
- activation identity: `{candidate_commitment, activation_epoch}`;
- request/read identity: canonical request digest plus resolved domain evidence.

One logical generation may bind to exactly one candidate commitment. Exact same commitment is replay; a different
commitment is `CANDIDATE_COMMITMENT_CONFLICT`.

### Encoding and filesystem security

- map iteration order and source input order cannot affect a commitment;
- zero digest and empty string are not absence sentinels;
- filename/payload mismatch, hardlink, symlink or `nlink != 1` is typed corruption;
- product-created state-root directories are mode `0700`; authority, lock, key and object files are mode `0600`;
- expected owner is the effective UID captured while acquiring the state-root lease;
- open performs `lstat`, no-follow open and `fstat`, then verifies same inode/device, regular-file type, expected UID,
  exact mode and `nlink == 1` before reading bytes;
- a platform without equivalent checks refuses production open with `STATE_ROOT_SECURITY_POLICY_UNSUPPORTED` before
  mutation;
- immutable projection is create-new plus file fsync plus directory fsync; an existing address must contain exact
  same canonical bytes or is a collision refusal and is never overwritten.

The quarantine original path is the component array above, never an absolute host path. P01A owns pure
codec/address/security primitives only. P02B owns sequence allocation and the generic event ledger. P03 owns catalog
incident creation, immutable projection, crash replay and source unlink. P01A performs no live filesystem mutation.

## Compatibility

- legacy filename and `activations/` parsing exists only in the P10 offline importer;
- P03 live runtime refuses a V1 root before mutation and never deletes, renames or rewrites legacy bytes;
- product runtime has no old/new dual reader or dual writer;
- old producer/new daemon and new producer/old daemon are refused before mutation by protocol version/contract digest.

## Rejected alternatives

- adding another separator or percent-escape layer;
- lower-casing or NFC-normalizing after decode;
- using generation as content identity;
- keeping a human-readable filename as authority;
- a local, timestamp-derived or random quarantine sequence.

## Required proof

Golden byte/digest vectors fix field order, integer width, Unicode, domain tags and every nullable branch. Property
tests cover tuple injectivity. Filesystem fixtures cover case folding, path length, symlink/hardlink,
payload/address mismatch and crash retry preserving one incident sequence/envelope.
