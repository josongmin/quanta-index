# Architecture Decision Records

이 디렉터리는 현재 product architecture decision의 canonical owner다. 완료된 historical probe와
campaign-local 기록은 Git 이력에서 회수하며, 현재 구현을 구속하는 결정은 여기의 `Accepted` ADR만 권위로 사용한다.

상태:

- `Proposed`: 구현 시작을 허용하지 않는 초안
- `Accepted`: blocking consumer가 구현할 수 있는 frozen decision
- `Superseded`: 후속 ADR 링크가 필수인 폐기 결정

Current proposals (not accepted decisions or implementation authority):

- [Search-corpus selection and ingest pressure](OCT-04-001-search-corpus-selection-and-ingest-pressure.md)
- [Effective configuration and generation policy](OCT-04-002-configuration-and-generation-policy.md)
- [Optional source-preparation SDK](OCT-04-003-source-preparation-sdk.md)

ADR 변경은 decision을 바꾸는 breaking change다. 같은 commit에서 downstream contract, inventory, migration
class와 proof authority를 갱신한다. optional compatibility field나 dual live decoder로 decision drift를 숨기지 않는다.

## OCT-05 implemented contracts

Oct-04 handoff/implementation history is consolidated into four `Accepted` ADRs.
They preserve existing contracts; conditional optimization, open proposals and
actual operational targets remain staged. Oct-06/07 completed bounded
publication, budget, executable-custody, common operational producer and CI
contracts are consolidated in OCT-05-002/003/004. Current acceptance is owned by
the [single residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md).

Dated RFC/plan/ticket packets are retired. Their standing corpus/statistics,
native/update, cost/host, consumer/platform and installed/pair/action acceptance
is consolidated into OCT-05-001/002/004 and SEP-27-005; unfinished execution
remains once in the ledger. This consolidation admits no new result or target.

- [Review, admission and result identity](OCT-05-001-review-admission-and-result-identity.md): E1 frozen typed binding, actual judgment, name units and split/gold authority.
- [Native capture, clock and index scope](OCT-05-002-native-capture-clock-and-index-scope.md): E2 native replay, strict Lucene metadata, disk/reader boundaries and warmup policy.
- [Active query and runtime lifecycle](OCT-05-003-active-query-and-runtime-lifecycle.md): E3 retire-first refusal, one-RPC binding, metering, timeout/replay and operator authorization.
- [Cost, capacity and qualification boundaries](OCT-05-004-cost-capacity-and-qualification-boundaries.md): E4/I0 causal/scanner/scale, conditional changes and current-source/release authority.

Exact old bodies and executions remain recoverable through the
[plan history index](../ARCHIVE-INDEX.md#historical-record-recovery).
Removing detailed plans does not close the active parent or issue qualification.

## Prior accepted decisions

SEP-21 accepted set:

- [Canonical identity and digest domains](SEP-21-001-canonical-identity-and-digest-domains.md)
- [Durable authority and operation lifecycle](SEP-21-002-durable-authority-and-operation-lifecycle.md)
- [Read view, continuation and provider policy](SEP-21-003-read-view-continuation-and-provider-policy.md)
- [Process supervision, state cutover and proof](SEP-21-004-process-supervision-state-cutover-and-proof.md)
- [Decision registry](SEP-21-DECISION-REGISTRY.md)
- [Catalog recovery, supervision and proof custody](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)

SEP-26 retrieval accepted set:

- [Query, publication and result-proof contracts](SEP-26-001-retrieval-query-publication-and-result-proof.md)
- [Observation, experiment and default policy](SEP-26-002-retrieval-observation-experiment-and-default-policy.md)
- [Evidence custody and qualification boundaries](SEP-26-003-retrieval-evidence-custody-and-qualification.md)
- [Decision registry](SEP-26-DECISION-REGISTRY.md)

Documentation governance:

- [Documentation authority and historical record custody](SEP-27-001-documentation-authority-and-historical-record-custody.md)

Benchmark control plane:

- [Single benchmark orchestrator and typed evidence](SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md)
- [Capture, process and resource custody](SEP-27-004-benchmark-capture-and-resource-custody.md)

Code search:

- [Query plans, source coverage, exact identity and previews](SEP-27-003-code-search-source-and-preview-contract.md)

May–Jun 2026 accepted set:

- [SDK ingress and public surface boundary](MAY-27-002-sdk-ingress-and-public-surface-boundary.md)
- [Search DSL authority and runtime contract](JUN-02-001-search-dsl-authority-and-runtime-contract.md)
- [Sourcegraph compatibility boundary](JUN-06-001-sourcegraph-compatibility-boundary.md)
- [LanceDB semantic generation authority](MAY-31-001-lancedb-semantic-generation-authority.md)
- [Verification, quality gates and benchmark separation](JUN-08-001-verification-hellgate-and-benchmark-separation.md)
- [Decision registry](MAY-JUN-2026-DECISION-REGISTRY.md)

Historical implementation packets and superseded drafts are indexed in
[the plan history index](../ARCHIVE-INDEX.md#historical-record-recovery). Historical audits,
bugbash records, old SSOTs, and receipts outside plan packets are indexed in
[the documentation archive](../ARCHIVE-INDEX.md#historical-record-recovery).
