# TOPT-03 — Runtime Fixture and Process Lifecycle Ownership

Status: `planned`

Depends on: TOPT-00

Findings: D1, D2, R1

Aligned S21 owner: S21-09

## Goal

One harness owner must construct direct runtimes, allocate all sockets, apply
required daemon environment, wait for readiness, and terminate children.
Scenario files own only scenario-specific data and assertions.

## Target API

A harness-owned builder/driver should provide:

- private state root;
- query, control, and ingest socket paths allocated together;
- all three explicit socket overrides;
- retention/default test environment in one function;
- readiness with typed timeout evidence;
- child lifecycle guard with race-tolerant, result-preserving cleanup;
- optional wrapper command so umask tests can change launch mechanics without
  forking cleanup semantics.

Do not create a macro or another `#[path]` copy. Prefer a normal API in
`quanta-index-searchd-harness` or the existing runtime common owner.

## Work

1. Migrate `end_to_end` and `repo_map_end_to_end` to the three-socket builder.
2. Migrate other local direct-runtime builders in the same ownership family;
   keep special embedder/config parameters explicit.
3. Reuse shared readiness/termination/env configuration from the umask wrapper.
4. Replace the lease-holder three-second sleep with an acknowledged release
   protocol: holder reports `held`, parent proves the second refusal, parent
   closes/writes the release channel, holder exits.
5. Parent death must release the holder; no orphan may sleep indefinitely.

## Required proofs

- deliberately long macOS state-root path binds all three sockets;
- already-exited child cleanup does not mask the scenario result;
- failed child still preserves the original terminal failure;
- holder cannot release before the second process observes typed
  `STATE_ROOT_IN_USE`;
- parent abort/pipe close releases holder and leaves no lease/socket residue.

## Verification

- `just rust-profile test-runtime-supervisor-owner`
- `just rust-profile test-daemon-fast`
- `just rust-profile test-daemon`
- `just rust-profile test-daemon-all`

Timing evidence compares the exact lease test before/after on the TOPT-00 quiet
host protocol. The target is removal of the fixed three-second floor, not an
unscoped percentage claim.

## Done

No scenario-local lifecycle implementation remains in the migrated family, all
three sockets are explicit, and cleanup/release behavior is event-driven.
