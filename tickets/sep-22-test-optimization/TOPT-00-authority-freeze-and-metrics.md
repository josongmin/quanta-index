# TOPT-00 — Authority Freeze and Measurement Contract

Status: `blocked — uncontended baseline and retrospective admission missing`

Depends on: none

## Goal

Create a reproducible admission record before implementation so source drift,
foreign dirty work, selector drift, cache state, or host contention cannot turn
an optimization claim into unverifiable prose.

## Work

1. Record exact `HEAD`, `origin/main`, branch, dirty paths, and a digest of the
   six audit documents and this packet.
2. Re-open every cited owner path and mark each finding `OPEN`, `STALE`, or
   `ALREADY_FIXED`. `STALE` stops its dependent ticket until re-audited.
3. Resolve every focused selector through `tools/ci/test-authority.toml` and the
   current Justfile. A command label is not coverage evidence.
4. Capture a zero-execution test listing/selector receipt for affected targets.
5. Define timing protocol:
   - quiet host; foreign Cargo/rustc/cargo-mutants processes are a hard stop;
   - same source, toolchain, feature set, selector, target directory policy;
   - cold build time and warm execution time reported separately;
   - at least five warm samples for sub-second/low-second focused targets;
   - median, p95, min/max, selected/executed/pass/fail counts;
   - no `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` evidence.
6. Capture baselines only for R1-R4 and TH-4. R5 is an execution-count removal,
   so its baseline is selected cases/daemon boots, not a fabricated duration.

## Outputs

- source/admission record;
- selector inventory for all ticket rails;
- timing protocol plus raw baseline locations;
- exact path leases and integration order.

## Acceptance

- all 18 findings map to one live owner and one implementation ticket;
- all commands resolve to registered targets or are explicitly owner-local
  probes through `./scripts/cargow`;
- no implementation lane starts from a different source digest;
- foreign dirty paths, including concurrent proof-registry work, are named and
  excluded from lane ownership.

## Verification

- `python3 tools/ci/lint/check-test-authority.py`
- `git diff --check`
- static crosswalk count: 18 unique inputs, 18 unique terminal mappings

No Rust green claim is produced by this ticket.
