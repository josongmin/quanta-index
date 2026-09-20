# SEP-21-003 — Read View, Continuation and Provider Policy

Status: `Accepted`

Decided: 2026-09-21

Gate owner: S21-00; blocks S21-05, S21-06, S21-07 and S21-08

## Query read authority

Every declared domain has exactly one `DomainReadEvidenceV2`. Physical handles are acquired per actual resource
group and may be shared, so domain count is not handle count. `QueryReadViewV2` owns all immutable handles through
request completion; routes cannot query an ambient ledger/store after acquisition.

An active selector response carries `ResolutionProofV1 { requested_scope, resolved_read_identity,
activation_epoch, candidate_commitment }`. The SDK validates requested scope, resolution proof and response read
identity. It does not compare an unresolved selector structurally with a resolved pin.

## Query outcome

`ExecutionOutcomeV2` is closed:

- `ExactExhausted`;
- `LowerBound { continuation }`;
- `CappedUnknown { cap }`;
- `InterruptedPartial { reason }`;
- `Approximate { method, quality_contract }`.

Unavailable/not-ready is a typed error, not empty success. Empty success is `ExactExhausted(count=0)` only.
`has_more=false` requires an exhaustion proof. Partial success requires explicit `allow_partial`. Lexical, symbol,
history, runtime and structural routes are pageable; semantic, hybrid, hybrid-seed and RepoMap are bounded top-k.

## Continuation security

The only live cursor is a stateless opaque signed `CursorEnvelopeV2` containing version, route, canonical request
digest, resolved read-identity digest, typed ordering key, issued/expiry timestamps and key ID. It uses canonical
CBOR, HMAC-SHA256 and base64url without padding.

- default TTL: 15 minutes;
- maximum TTL: 1 hour;
- key: persistent random 32 bytes, mode `0600`, included in backup/migration;
- signature/version/expiry/context are checked before resource acquisition;
- unsigned live decoder is forbidden.

## RepoMap focus semantics

- empty focus means global search;
- non-empty focus requires every subject to resolve;
- any unresolved subject returns `FOCUS_SUBJECT_NOT_FOUND` before execution;
- candidate universe is the union of resolved subjects and their owner-path cohort;
- focus may boost only inside that universe;
- global fallback from non-empty focus is forbidden.

## Provider egress

Remote egress is default-deny. `QueryText` and `SourceContent` are separate data classes. Source content requires an
explicit separate grant. A provider profile binds tenant allowlist, data class, provider, exact endpoint, declared
region/retention, model+revision and request/token/concurrency/cost caps. Missing fields return
`PROVIDER_EGRESS_DENIED` before I/O.

Until an operator provides a complete profile, all remote provider calls are denied; this is the frozen safe value,
not an unresolved decision. Hash/dev providers cannot satisfy production semantic proof.

## SDK binding

Plane-specific closed expected-response enums are built before request payload move. Contract decoders validate
intrinsic shape; SDK validators bind request/read/candidate/activation/operation identities. Global sequence
monotonicity remains catalog authority.

## Rejected alternatives

- server-side cursor state for V2;
- unsigned cursors;
- hit-count-derived availability/completeness;
- unresolved focus as a ranking hint;
- one egress consent for both query and source content.
