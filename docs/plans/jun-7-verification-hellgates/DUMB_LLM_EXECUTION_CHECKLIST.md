# Dumb LLM Execution Checklist

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


1. read `WORKER_START_HERE.md`
2. run `just rust-verify-hellgate-fast`
3. if red:
   - fix the narrowest owning seam
   - rerun only the failing fast rail first
4. when fast is green, run `just rust-verify-hellgate-broad`
5. when broad is green, run perf:
   - `just rust-bench-dsl-warm`
   - `just rust-bench-dsl-cold 20`
   - `just rust-bench-dsl-compare`
6. if cross-repo ingress changed, run:
   - `just rust-verify-hellgate-cross-repo`
7. closeout must report:
   - command
   - covered surface
   - excluded surface
   - status
   - failure class if failed
