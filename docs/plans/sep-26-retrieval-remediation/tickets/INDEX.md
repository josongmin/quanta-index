# SEP-26 Retrieval Remediation

Status: `PARTIAL` — accepted architecture and implemented code are not product qualification.

| Authority | Owner |
|---|---|
| Query policy, combined publication, symbol identity, result proof and spans | [SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md) |
| Observation, comparator, semantic/ANN proof, defaults and resources | [SEP-26-002](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md) |
| Evidence custody, experiment admission and qualification | [SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md) |
| Compact decision lookup | [SEP-26 registry](../../../adr/SEP-26-DECISION-REGISTRY.md) |
| Unfinished implementation, measurement and qualification | [Active gap register](GAP-REGISTER.md) |
| Required oracles and commands | [Test plan](TEST-PLAN.md) |

The old RBR-00..12 packets, profile draft and chronological audits were
removed from the live tree after ADR consolidation. Their exact bodies are
recoverable with `git show eff53181:<path>`; the [plan history index](../../ARCHIVE-INDEX.md)
maps the packet. Historical source-specific passes and failures are not current
qualification. The documentation consolidation changes the retrieval source
closure, so an earlier frozen checkout, including one with the same product
code, cannot be promoted to a current-main receipt without revalidating the
complete bound source and environment.

The later nFF snapshot closeout is recorded outside the repository at
`/private/tmp/qi-rbr-store-final.nFFdMH/final-closeout.json`. It binds native
contract, separate CI and independent consumer results to that snapshot only.
Concurrent main changes and the subsequent G-02 on/off diagnostic comparator
fix require a new exact-source freeze for any current-main qualification;
the [active gap register](GAP-REGISTER.md) records the remaining gates.
The post-G-06 source-bound attempt is reserved at
`/private/tmp/qi-rbr-source-final.xEv5WC/final-closeout.json`; its status is
`NOT_RUN` until terminal evidence is written. This path is not a passing flag.

The final external `PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` gates
remain `NOT_RUN` until independent corpus/gold, admitted pair/replay and
quiet-host evidence pass. Hosted CI last observed 14 jobs/0 steps and was
`BLOCKED` by billing; recheck before a current claim. Symbol text authority expansion remains
`NOT_APPLICABLE` unless product scope explicitly changes; typed refusal is
the accepted current boundary.
