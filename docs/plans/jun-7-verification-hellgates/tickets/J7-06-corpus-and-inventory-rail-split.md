# J7-06 — Corpus And Inventory Rail Split

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `landed`

Goal:

- document the difference between corpus execution inventory and fast hellgates

Owner seam:

- `tools/benchmark/README.md`
- `tools/benchmark/sourcegraph_parity.py`
- `tools/ci/lint/check-dsl-capability-truth.py`

Rule:

- `e2e_full_corpus` is broad executable inventory
- parity and capability checks are machine-readable truth inventory
- neither replaces the small fast hellgates
