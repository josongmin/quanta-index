# Weak-assertion audit (bugbash, Sep 22)

Scope: tests whose assertions cannot fail for the reason the test claims —
no value check after `is_ok`/`expect`/`unwrap`, `is_err` without an error
identity, substring-only output checks, and ignore-gated paths. Every finding
below was read in the cited file body; search hits alone were not counted.
Checked-and-strong counterparts are listed at the end so they are not
re-reported.

## W1 — `is_err` without refusal identity (socket access)

- File: `crates/quanta-index-ipc/src/socket_access.rs:359`
- Test: `root_is_the_owner_only_when_the_daemon_is_root`
- Symptom: `assert!(admit_peer(&SocketAccessPolicy::Private, peer(0, 0), SELF_UID).is_err())`
  verifies only that *some* error occurs. It does not check the variant is
  `PeerRefusal::NotAdmitted` nor that the refusal names the peer.
- Why it is bad (weak): any error — including a future fail-open bug that
  returns the wrong refusal kind — passes. The sibling table test ten lines
  above (`:341-347`) does check `peer == credentials` on refusal, so this
  test is strictly weaker than its neighbor.
- Fix: mirror the neighbor —
  `assert!(matches!(admit_peer(...), Err(PeerRefusal::NotAdmitted { peer }) if peer == peer(0, 0)), ...)`.

## W2 — `is_err` without error kind (embedding contract liar)

- File: `crates/quanta-index-embed/src/cache.rs:1902`
- Test: `a_provider_that_breaks_its_own_contract_poisons_nothing`
- Symptom: `assert!(provider.embed_batch(&["a"]).is_err())` does not assert
  *which* contract failed. The liar returns `vec![2.0, 0.0]` (norm violation
  at the right dimension), but a dimension error, an identity error, or any
  internal error would satisfy the same assert. Only the cache-miss half
  (`cache.get(&key).is_none()`) is pinned.
- Why it is bad (weak): the test's title promises a *contract* refusal; the
  assert proves only *a* refusal. A regression that rejects for the wrong
  reason (e.g. keying bug) stays green.
- Fix: match the error, e.g.
  `assert!(matches!(provider.embed_batch(&["a"]), Err(CoreError::Typed { code, .. }) if code == <norm-violation code>), "got {out:?}")`.
  Optionally add a second liar with the wrong dimension to pin both arms.

## W3 — normalized output vectors discarded, only counters checked

- File: `crates/quanta-index-core/tests/semantic_policy.rs:270-272,288-289`
- Test: `the_wrapper_reports_how_far_its_raw_provider_was_from_unit`
- Symptom: `let _served = provider.embed_batch(...).expect("normalized");` —
  the served (renormalized) vectors are bound to `_served` and never
  inspected. The test asserts tally counters `(3, 1, 1.0)` / `(2, 2, 0.5)`
  but never that the returned vectors are actually unit-norm.
- Why it is bad (weak): a normalizer that counts correctly yet returns
  unnormalized vectors passes. The counters test the instrumentation, not
  the data contract downstream consumers rely on.
- Fix: assert on the values, e.g.
  `for v in &_served { assert!((norm(v) - 1.0).abs() < 1e-5, ...); }`
  and `assert_eq!(_served[2], expected_unit_vector)` for the rescaled input.

## W4 — `is_err` passes whichever input fails, for any reason

- File: `crates/quanta-index-core/tests/semantic_policy.rs:278`
- Test: same as W3 (`the_wrapper_reports_how_far_its_raw_provider_was_from_unit`)
- Symptom: `assert!(refused.embed_batch(&["triple", "nan"]).is_err())` over a
  two-vector batch (`3.0` and `NaN`). The assert passes if *either* vector
  fails for *any* reason; it cannot tell which vector was refused or why.
- Why it is bad (weak/ambiguous verification): if the wrapper starts
  rejecting the `3.0` vector for a config error instead of a norm violation,
  or silently drops `NaN`, the test still passes.
- Fix: split into two single-vector batches and match each error, or assert
  the typed norm-violation code; keep the existing
  `(0, 0, 0.0)` tally assert which already proves atomicity.

## W5 — `is_err` without typed code next to a sibling that has one

- File: `crates/quanta-index-core/src/domains/observability.rs:354,357`
- Test: `the_envelope_sums_every_component_and_refuses_a_sum_over_the_ceiling`
- Symptom: `assert!(envelope(0).validate().is_err())` and
  `assert!(narrow_rss.validate().is_err())` check bare failure, while the
  over-ceiling case three lines above (`:349-352`) matches
  `CoreError::Typed { code, message }` with the exact
  `PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE` and `"2100 bytes"`.
- Why it is bad (weak): a zero-byte envelope rejected as, say, a malformed
  field rather than an exceeded/empty envelope passes; likewise an RSS
  refusal with the wrong code passes. The test proves rejection, not the
  reason operators will see.
- Fix: extend the `matches!` idiom to both lines — check the zero envelope
  yields its specific code and the narrowed RSS yields
  `PROCESS_RSS_CEILING_EXCEEDED_CODE`.

## W6 — constructor validation checks direction only, not attribution

- File: `crates/quanta-index-core/tests/ingest_resource_policy.rs:246-251`
- Test: `zero_ceilings_are_refused_at_construction`
- Symptom: three `assert!(IngestResourcePolicy::new(0/0/0-per-position).is_err())`
  plus one `is_ok`, with no check of *which* ceiling was blamed and no check
  of the stored values on the ok path.
- Why it is bad (weak): a constructor that rejects all three positions with
  the same generic error (or that clamps instead of storing on the ok path)
  passes. Misattributed errors mislead callers sizing batches.
- Fix: match the error per position (each arm names its ceiling) and, on the
  ok path, assert the admitted footprint equals the policy ceilings, e.g.
  `assert_eq!((p.max_text_bytes(), ...), (1, 1, 1))` or via a max-size batch.

## W7 — conjoined spawn assert hides which child failed; refusal untyped

- File: `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:161-162,215`
- Tests: `second_spawn_is_refused_and_rollback_is_bounded` area (`:160-162`),
  `required_child_exit_propagates_and_drains_peers` (`:215`)
- Symptom: `assert!(second.is_err(), "the second spawn is refused")` never
  checks the refusal variant; `assert!(spawned_a.is_ok() && spawned_b.is_ok())`
  conjoins two spawns with no message, so a failure does not say which child
  failed to spawn.
- Why it is bad (weak/cheesy): the refusal test passes for any spawn error,
  including a harness bug unrelated to the single-child policy; the conjoined
  assert forces the debugger to bisect by hand.
- Fix: `assert!(matches!(second, Err(... single-child-refusal ...)))` and split
  line 215 into two asserts with per-child messages.

## W8 — ID length limit tested at 512 (valid) and 16384 (invalid) with no boundary

- File: `crates/quanta-index-core/src/domains/generation.rs:581-582`
- Test: `storage_key_contains_traversal_absolute_and_long_ids_under_root`
- Symptom: `assert!(RepoId::new("r".repeat(16_384)).is_err())` (same for
  `RevisionId`) checks only a far-over-limit length, with no error identity
  and no probe of the actual boundary (valid case uses length 512).
- Why it is bad (weak): an off-by-one (or off-by-thousand) change to the max
  length passes both sides; a wrong error kind passes too.
- Fix: read the canonical max from the constructor's policy constant, assert
  `is_ok` at exactly max and `is_err` with the typed length-exceeded code at
  max + 1.

## W9 — `#[ignore]`-gated semantic e2e never runs in CI

- File: `crates/quanta-index-searchd-runtime/tests/end_to_end.rs:1759-1760`
- Test: `openai_semantic_paraphrase_outranks_unrelated_v1`
  (`#[ignore = "hits the real OpenAI API; ..."]`)
- Symptom: the only end-to-end proof that neural ranking beats lexical/hash
  signal (cat-vs-finance paraphrase) is ignore-gated on `OPENAI_API_KEY` and
  therefore silently green in every normal lane run.
- Why it is bad (failing-tolerant path): capability regressions in semantic
  ranking have no automated gate; the file's doc comment even routes readers
  to `-- --ignored`, which CI never passes.
- Fix (no new infra assumed): either add a non-ignored stub-embedding
  variant of the same cat-vs-finance corpus proving the *wiring* (rank order
  follows semantic scores, not lexical overlap), keeping the OpenAI test as
  the live-model calibration; or gate it on an env-marked nightly lane
  instead of a bare `#[ignore]`.

## Checked and NOT weak (do not re-report)

- `crates/quanta-index-searchctl/tests/cli_smoke.rs:73-137` — the outer
  `assert!(result.is_ok())` wraps `*_impl()` fns that return `Err` on every
  `stdout.contains(...)` mismatch (`:166-171`, `:200-211`, ...). Substantive
  value checks live inside the impl; the outer assert only forwards them.
- `crates/quanta-index-searchctl/src/lib.rs` parser/renderer unit tests
  (`:2805-2810`, `:3531-3536`, `:3265-3272`, ...) — each `is_ok`/`is_err` is
  followed by `exit_code`, message-content, or rendered-text `contains`
  asserts. Identity is verified.
- `crates/quanta-index-contract/tests/lexical_cursor_contract.rs:104,114,234`
  — full-value `!=` round-trip comparisons on whole pages, the strongest
  shape; the file contains no `assert!` string because it uses
  `if x != y { return Err(..) }`, which is exact.
- `crates/quanta-index-ipc/tests/wire_historical_cbor.rs:60-137` — same
  `matches!` + `return Err` idiom with exact variant + payload checks
  (e.g. `Oversized(length) if length == ...`). Strong despite zero `assert!`.
- `crates/quanta-index-lexical/tests/g0l_tantivy_snapshot_probe.rs` — probe
  tests compare digests/rankings against independent rebuilds with
  `G0L-EVIDENCE` lines; assertion-bearing, not snapshot-only.
- No `insta`/snapshot macros, no `assert!(true`-style tautologies, and no
  `#[ignore]`-gated tests other than W9 were found in `crates/`.
