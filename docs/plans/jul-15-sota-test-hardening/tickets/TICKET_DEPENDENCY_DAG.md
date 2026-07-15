# QIT Dependency DAG

```mermaid
flowchart LR
  Q00["QIT-00 Authority SSOT"] --> Q01["QIT-01 Wire/version"]
  Q00 --> Q02["QIT-02 Lifecycle model"]
  Q00 --> Q05["QIT-05 Differential oracle"]
  Q02 --> Q03["QIT-03 Crash matrix"]
  Q02 --> Q04["QIT-04 Concurrency"]
  Q01 --> Q06["QIT-06 SDK black-box"]
  Q05 --> Q06
  Q01 --> Q07["QIT-07 Quantitative gates"]
  Q02 --> Q07
  Q05 --> Q07
  Q03 --> Q08["QIT-08 Quality/scale"]
  Q04 --> Q08
  Q06 --> Q08
  Q07 --> Q09["QIT-09 Promotion/receipts"]
  Q08 --> Q09
```

## Parallel lanes

After QIT-00's catalog contract is stable, QIT-01, QIT-02, and QIT-05 may run
in parallel because their primary owners do not overlap. QIT-03 and QIT-04 must
wait for the lifecycle state model: otherwise crash/concurrency tests encode
unreviewed state semantics. QIT-06 is consumer closure, not an alternative
implementation path. QIT-07 instruments already defined risk owners. QIT-08
starts only after correctness closure; QIT-09 promotes receipts, not intent.

## Stop conditions

- If an invariant has no exact owner, stop and add the ownership decision before
  a test implementation.
- If a desired behavior changes public wire/lifecycle semantics, stop for a
  contract decision; do not silently adapt the reference model.
- If a required platform, corpus, or external service is unavailable, publish a
  bounded blocked receipt. Do not replace it with an unrelated local green run.
