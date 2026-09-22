# TOPT-05 — Independent Fail-Closed Oracles

Status: `planned`

Depends on: TOPT-00

Findings: WA-1, WA-2, WA-3

Aligned S21 owners: lexical contract owners and S21-11

## Goal

An operation under test failing, returning a different in-corpus row, or
publishing an incomplete backup must make the owning test fail at the causal
boundary.

## Work

### Trigram properties

- convert unexpected serialize, deserialize, and valid-intersection errors to
  `TestCaseError::fail` with generated input and error context;
- retain only documented invalid-domain rejections;
- add mutation controls proving each unexpected `Err` makes the property red.

### Matrix smoke

- assert exact candidate identity/path set for `smoke_needle_rust`;
- reject missing rows and extras;
- expected values come from the fixture contract, never from returned rows;
- retain the separate typed-error scenario.

### State migration

- after the post-cutover fault, run the production offline verifier against the
  published destination;
- compare manifest object identities, sizes/digests, and required metadata with
  the expected backup manifest;
- preserve staging-absence and no-mixed-root assertions;
- mutation deleting or corrupting one object must make the test fail.

## Verification

- `./scripts/cargow test -p quanta-index-lq-trigram --test property_cbor_roundtrip`
- `./scripts/cargow nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(/^e2e_matrix_inventory::/)'`
- `just rust-profile test-state-migration-owner`
- `just rust-profile test-daemon-fast`

## Done

Each of the three tests has a negative mutation/control that fails for the
precise regression it names. Error/empty/non-empty shortcuts cannot satisfy the
oracle.
