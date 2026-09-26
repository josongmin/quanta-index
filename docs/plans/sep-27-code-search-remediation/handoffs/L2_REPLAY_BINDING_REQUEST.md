# L2 source replay public binding cutover

Status: implementation connected; contract/SDK/dispatcher regressions executed.
Final source-stable owner receipts and composed daemon proof are tracked in
`l2-proof/` and `L2_HANDOFF.md`. Earlier OPEN/ownership/slot statements in this
file are superseded by the user's explicit concurrent-work authorization.
HEAD at implementation: `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty checkout.

A repeated event requested under another containing revision/generation must
return its original publication. Previously the dispatcher did that, but the
observation and SDK required the new target, and activation used the new target.

The current `SearchCorpusPublishOutcome` requires `publication`:

- `event: SourcePublicationEvent`: exact stream/event/expected-base/payload.
- `target: GenerationSnapshot`: the original lexical containing snapshot.
- `batch_digest: String`: the original operation journal commitment.

The dispatcher reads replay bindings from the immutable catalog record, reconciles
its original journal, and checks that the original paired generation remains
retained. Applied responses match the request exactly. Replays preserve the
original receipt and nonzero durable sequence; their observation describes the
current request with no cached stage timings. Missing/unknown/duplicate wire
fields reject. The unbound `From<BatchPublishReceipt>` conversion is removed.

The SDK checks publication identity on observed and unobserved routes and builds
both activation tracks from the returned original target plus attested semantic
roots. Explicit CAS expectations are revalidated against that target, never
rewritten to another revision. The daemon remains the original-publication
authority; a replay response is not a new activation acknowledgement.

Regression coverage includes target retargeting, wrong repo/event/base/payload,
receipt/target/digest/seal inconsistencies, zero replay sequence, first publish,
replay timings, CAS refusal without control dispatch, and reclaimed original
refusal. The public SDK unit tests use transports; the new runtime frontdoor test
uses real sockets, storage/journal/catalog and restart and remains distinct proof.

The sibling contract negative test now recomputes the unit commitment and asserts
`RecordPathMismatch(Chunk)`, so a stale hash cannot satisfy its intended oracle.

Downstream Semantica producer/fixture migration is outside this repository and
has not been verified by L2. No compatibility outcome or alternate generation
identity model was introduced.
