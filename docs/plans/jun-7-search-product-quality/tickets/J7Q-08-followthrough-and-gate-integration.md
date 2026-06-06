# J7Q-08 — Followthrough And Gate Integration

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Integrate the new quality rails into stable commands and docs without mixing
them back into correctness-only packets.

## Current Code Fact

- correctness verification lanes are already split
- quality work currently lacks one stable aggregate story

## Owner Seam

- just targets
- benchmark docs
- packet docs

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/Justfile`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-verification-hellgates/rfc.md`

## Preferred Implementation Direction

- stable commands should map one-to-one to a quality dimension where possible
- aggregate commands may orchestrate, but each sub-rail must keep its own claim
  type and blocking/advisory meaning
- docs should encode command ownership and wording, not leave it to chat

## Layer Boundary Clarification

- this ticket owns command and doc integration
- it does not retroactively redefine what earlier packets proved

## Concrete Work Items

1. Add stable commands for relevance, scale, tail, and operator-quality rails.
2. Document blocking vs advisory status per command.
3. Keep verification and product-quality packets distinct.
4. Keep final closeout wording precise.

## First Increment

- define command names and ownership before wiring all underlying rails

## Red Rail To Pin First

```bash
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## Worker First Commands

```bash
sed -n '1,260p' Justfile
sed -n '1,260p' tools/benchmark/README.md
sed -n '1,220p' docs/plans/jun-7-verification-hellgates/rfc.md
```

## No-Go

- do not merge quality gates back into one monolithic command without separation
- do not let docs overclaim quality closure from partial rails

## Reviewer Rejection Checklist

- reject if users still cannot tell which command proves which dimension
- reject if aggregate targets erase blocking/advisory or claim-type boundaries
- reject if docs still require chat interpretation to explain command purpose

## DoD

- quality gates are callable, scoped, and documented

## Not Done If

- users still cannot tell which command proves which quality dimension
- docs or commands collapse correctness and product-quality states again
