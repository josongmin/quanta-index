# Dumb LLM Execution Checklist

1. read `WORKER_START_HERE.md`
2. confirm correctness baseline:
   - `python3 tools/benchmark/sourcegraph_parity.py --check`
   - `python3 tools/ci/lint/check-dsl-capability-truth.py`
3. choose exactly one `J7Q-*` ticket
4. add the narrowest quality rail first
5. change only the owning seam for that ticket
6. rerun the owning rail before any broader rerun
7. if the change touches runtime behavior, rerun the smallest matching
   `searchd-runtime` test next
8. if the change touches bench/latency tooling, rerun only the relevant bench
   rail before any aggregate compare
9. closeout must report:
   - command
   - covered surface
   - excluded surface
   - status
   - failure class if failed
   - whether the proof is correctness, relevance, scale, tail, ops, or UI
10. reject the patch if it relies on:
   - unlabeled anecdotal ranking examples
   - string-presence snippet or explain checks only
   - non-seeded scale fixtures
   - one-size-fits-all tail thresholds
   - heuristic or silent query repair
   - untyped UI payload blobs
   - semantic retrieval or hybrid fusion scope mixed into this packet
