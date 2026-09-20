# SEP-21-001 — Canonical Identity and Digest Domains

Status: `Accepted`

Decided: 2026-09-21

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
- non-NFC input returns `IDENTITY_NON_CANONICAL`;
- empty/over-limit input returns `IDENTITY_TOO_LONG`.

The 512-byte bound is product policy, not a filesystem limit. The producer must reject or map an external identifier
before it reaches this contract; the search plane does not truncate it.

### Canonical tuple encoding

`RepositoryRevisionIdentityV1` is encoded as:

```text
domain = "quanta-index/repository-revision/v1"
u32_be(len(repo_utf8)) || repo_utf8
u32_be(len(revision_utf8)) || revision_utf8
```

`LogicalGenerationIdentityV1` appends `u64_be(generation)`. A digest is SHA-256 over the domain length+bytes and
payload above and is rendered only as lowercase `sha256:<64 hex>`. Filesystem addresses use the raw 32-byte digest
with fixed two-level fanout. Human identifiers stay inside verified payloads.

The RepoMap candidate object grammar is exact:

```text
objects/sha256/<hex[0:2]>/<hex[2:4]>/<hex[4:64]>.cbor
```

`hex` is the lowercase 64-character SHA-256 of the canonical candidate bytes. Every component is ASCII, the two
fanout components are exactly two characters, and the leaf is exactly 60 characters plus `.cbor`. No alternate
extension, uppercase form, human identifier, separator escaping or adjacent metadata file is accepted by the live
runtime. Quarantine incidents use the same digest grammar under `quarantine/incidents/sha256/`, where the digest is
over the canonical incident envelope rather than the quarantined payload.

### Identity separation

- logical key: `{repo_id, revision_id, generation}`;
- candidate commitment: digest of canonical candidate envelope containing logical key, schema/profile commitments
  and sorted artifact inventory `(typed domain, content digest, size)`;
- physical object address: content digest of immutable bytes;
- activation identity: `{candidate_commitment, activation_epoch}`;
- request/read identity: canonical request digest plus resolved domain evidence.

One logical generation may bind to exactly one candidate commitment. Exact same commitment is replay;
different commitment is `CANDIDATE_COMMITMENT_CONFLICT`.

### Encoding rules

- new wire and persisted envelopes use canonical CBOR with definite lengths, shortest integers and fixed integer keys;
- each digest has a distinct domain/version prefix;
- map iteration order and input order cannot affect a commitment;
- zero digest and empty string are not absence sentinels;
- filename/payload mismatch, hardlink, symlink or `nlink != 1` is typed corruption.

### Filesystem security policy

- state-root directories created by the product are mode `0700`; authority files and lock/key/object files are mode
  `0600`;
- expected owner is the effective UID captured while acquiring the state-root lease;
- open performs `lstat`, no-follow open and `fstat`, then verifies the same inode/device, regular-file type, expected
  UID, exact mode and `nlink == 1` before reading bytes;
- a platform without equivalent owner/mode/no-follow checks cannot open a production state root and returns
  `STATE_ROOT_SECURITY_POLICY_UNSUPPORTED` before mutation;
- quarantine publish is create-new + file fsync + directory fsync. An existing incident address must contain the exact
  same canonical envelope or is a collision refusal; it is never overwritten.

`QuarantineIncidentV1` contains a positive state-root-global sequence, observed Unix nanoseconds, original relative
path, observed size, payload digest when readable, expected address digest, stable reason code and the evidence digest.
The original path must be one normalized relative path below the state root; absolute paths, `..`, empty components and
symlink traversal are refused.

## Compatibility

- legacy filename parsing exists only in the offline importer;
- product runtime has no old/new dual reader or dual writer;
- old producer/new daemon and new producer/old daemon are refused before mutation by protocol version/contract digest.

## Rejected alternatives

- adding another separator or percent-escape layer;
- lower-casing or NFC-normalizing after decode;
- using generation as content identity;
- keeping a human-readable filename as authority.

## Required proof

Golden byte/digest vectors fix field order, integer width, Unicode and domain tags. Property tests cover tuple
injectivity; filesystem fixtures cover case-folding, path length, symlink/hardlink and payload/address mismatch.
