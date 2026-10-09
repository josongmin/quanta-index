# SEP-26 Retrieval Decision Registry

Status: `Accepted`

Decided: 2026-09-27

The linked ADRs own full semantics. This table is the compact implementation index.

| ID | Frozen decision | Owner ADR | Origin |
|---|---|---|---|
| R-QRY-01 | Exactly one of native, literal or natural-language input policy is selected before execution | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-02 |
| R-QRY-02 | Original, effective lexical and semantic text identities remain distinct | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-02 |
| R-PUB-01 | One file replacement publishes chunks and symbols together; scope digest binds both and producer identity | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-04 |
| R-PROOF-01 | Typed published-unit registry is result-kind, path and span authority | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-05 |
| R-SPAN-01 | Indexed, returned and scored spans are separate; overlapping byte ranges are unioned | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-06 |
| R-SYM-01 | Unsupported symbol text forms are typed refusals until a canonical authority migration is accepted | [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md) | RBR-08 |
| R-OBS-01 | Executed and contributed lanes differ; missing is not zero | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-01 |
| R-OBS-02 | Query-stage, SDK wall and sidecar costs remain separate; ingest timing is transient | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-01/10 |
| R-CMP-01 | Comparator mode and actual phase events are frozen and source-bound | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-03 |
| R-SEM-01 | Full-vector parity and production-served versus independent exact scan are separate proofs | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-07 |
| R-FETCH-01 | Hybrid floor values are 25, 50 or 100; default remains 100 until qualified evidence changes it | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-09 |
| R-RES-01 | Process-tree reachability is computed before RSS filtering | [SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md) | RBR-11 |
| R-EVID-01 | Implementation, owner proof, integration proof, product qualification and release proof are distinct layers | [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) | RBR-00/12 |
| R-EVID-02 | Receipts bind exact source, inputs, dependencies, configuration, binaries, environment, command and raw terminal | [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) | RBR-00 |
| R-EVID-03 | Evidence reads and staged writes use no-follow path custody; absent optional pointers differ from linked or tampered entries | [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) | RBR-00/12 |
| R-EVAL-01 | Development selects one combination; one independently frozen holdout evaluates it | [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) | RBR-12 |

Open execution and qualification work is not a decision. It is tracked only in the
[single residual ledger](../plans/oct-10-index-closeout/VALIDATION.md#affected-checks).
