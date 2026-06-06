# J7Q-00 — Scope Lock And Measurement Policy

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Freeze the vocabulary and acceptance boundaries for search product quality so
later tickets do not collapse correctness, relevance, scale, and latency into a
single verdict.

## Current Code Fact

- correctness verification is already split and landed
- product-quality evidence is still scattered across runtime tests, benches, and
  old planning docs
- blocking vs advisory policy is still strongest on correctness, weaker on
  relevance and tail

## Owner Seam

- quality packet docs
- benchmark readme
- aggregate just targets and policy wording

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-verification-hellgates/rfc.md`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`

## Preferred Implementation Direction

- publish one claim matrix with columns:
  - quality dimension
  - truth source
  - blocking rail
  - advisory rail
  - allowed final wording
- freeze vocabulary before any downstream ticket broadens scope

## Layer Boundary Clarification

- correctness packet truth remains owned by `jun-7-verification-hellgates`
- this ticket owns wording and claim policy only, not route behavior

## Concrete Work Items

1. Define blocking vs advisory language for:
   - correctness
   - relevance
   - scale
   - latency tail
   - operator UX
2. Freeze the meaning of:
   - “green”
   - “quality regression”
   - “tail advisory”
   - “relevance regression”
3. Ensure later tickets inherit one consistent closeout vocabulary.

## First Increment

- land the packet scaffolding and explicit terminology table

## Red Rail To Pin First

```bash
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## Worker First Commands

```bash
sed -n '1,220p' docs/plans/jun-7-search-product-quality/rfc.md
sed -n '1,220p' docs/plans/jun-7-verification-hellgates/rfc.md
sed -n '1,220p' tools/benchmark/README.md
```

## No-Go

- do not redefine correctness gates as quality gates
- do not leave blocking/advisory semantics implicit

## Reviewer Rejection Checklist

- reject if one command is described as proving correctness, relevance, and tail
  at once
- reject if blocking/advisory wording still depends on chat interpretation

## DoD

- every later ticket can use one stable measurement vocabulary

## Not Done If

- the packet still mixes correctness and product quality language
- blocking vs advisory remains implicit
