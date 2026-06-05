# SGT-07 Guard And Capability Followthrough

Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Keep guard, capability inventory, and backlog packet aligned with the final tail-gap verdict set.

## Current Code Fact

- `tools/benchmark/sourcegraph_parity.py --check` already tracks supported canonical surfaces and explicit unsupported structural inventory
- current analysis doc still needs a follow-on packet link for exact tail gaps

## Official Sourcegraph Baseline

- the comparison source remains Sourcegraph official docs, not older internal packet prose

## Owner Seam

- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`
- `docs/plans/jun-5-sourcegraph-tail-gaps/`

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/SOURCEGRAPH_PARITY.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`

## First Increment

Add the new packet as the single tail-gap backlog reference and keep every new verdict machine-checkable.

## Red Rail To Pin First

- `python3 tools/benchmark/sourcegraph_parity.py --check`

## Worker First Commands

```bash
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
rg -n "jun-5-sourcegraph-tail-gaps|repo:contains.file|repo:has.description|repo:has.meta\\(key\\)|file:has.contributor|structural direct lexical" docs/analysis docs/plans
```

## No-Go

- do not add a supported surface to the guard without proof rails
- do not let docs describe a cell the guard cannot classify

## DoD

- supported cells stay in required-surface inventory only with proof
- explicit unsupported cells stay machine-checked
- analysis doc and packet docs list the same tail-gap set

## Not Done If

- docs list a tail-gap cell that the guard cannot classify
- a new supported or unsupported verdict lands without inventory followthrough
