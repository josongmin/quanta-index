# CS-INT-01 — Integration, contract cutover and qualification

Status: **PROPOSED**. Implementation, integrated tests and qualification: **NOT_RUN**.
Category: integration. Covers F01–F09 and G01/G02 through the nine owner RFCs.
Source baseline: [readme](../readme.md); historical proof: [evidence](../evidence.md).

The [final engine audit](../engine-audit.md) is required input for engine work.
Correctness order is domain/empty validation and count-window truth; canonical
scope/source identity; product coverage/name authority; semantic witnesses and
collector budget enforcement; only then relevance tuning. Existing exact grouped
collection, cancellation and valid pinned-snapshot cursors must be preserved.

## Authority and scope

This is a proposed integration design, not a second execution tracker.
[SEP-27-MISC](../../sep-27-misc/tickets/INDEX.md) remains the execution SSOT for
common orchestration, publication/GC, process/host capture, bounded I/O and
qualification. Reference adopted RFCs there; do not duplicate MISC ticket bodies
or replace independently verified stronger current behavior with older findings.

Do not reopen already completed retrieval changes merely because an old RCA file
names them. Recheck each finding against the selected source. Keep demonstrated
defects, intentional capability limits, experiments and missing proof separate.

## Ownership and dependency plan

| Lane | Owned work | Prerequisite and handoff |
| --- | --- | --- |
| Engine contract | ENG-01 | Independent first; hand off legal plan/decoder matrix |
| Input/oracle | BENCH-01 | Independent first; freeze corrected development and fresh holdout releases |
| Evidence | BENCH-02 | Independent first; hand off native decoder/rejection contract |
| Producer/capability | PROD-01 + ENG-02 | Parser probes independent; agree payload before changing publishers/readers |
| Relevance/presentation | ENG-03 + ENG-04 | Stable engine contract; tuning waits for BENCH-01/03 |
| Evaluation/measurement | BENCH-03 + BENCH-04 | Metric design can start early; real captures require native evidence and shared MISC prerequisites |
| Integrator | Shared contracts, SDK, fixtures, ADRs and final receipts | One serial owner after lane-local checks |

Parallel execution is permitted by this plan when implementation is authorized,
not started by this documentation change. Shared contract files have one owner.
Agree request/coverage/result payloads before producer, SDK and evaluator edits;
do not let multiple lanes invent incompatible schema fields.

## Repository and breaking-change boundary

- Retain benchmark-only Rust crates in the root Cargo workspace with one lockfile
  and toolchain. Product crates gain no normal benchmark-only dependency.
- Keep registry/benchctl/evidence and domain scorers as their accepted owners.
  No new umbrella CLI, duplicate corpus manager or second run-status manifest.
- Before changing schema, census producer, persistence reader, activation path,
  public SDK, registry, cursor, evaluator and sibling external consumers. Explicitly
  mark consumers not exercised locally; do not infer their upgrade from compilation.
- A breaking cutover is allowed when adopted: update producer/reader/SDK/test
  fixtures together; reject unknown versions. Historical replay readers, if kept,
  are explicit and cannot qualify new captures by relabeling.
- Determine whether old indexes need reindexing or an explicit migration. A mixed
  deployment must reject incompatible data/cursors rather than guess completeness.
- Update affected SEP-26 ADRs on acceptance. This proposed directory is not yet
  included in current source-closure allowlists: either bind adopted normative RFCs
  and test that closure, or consolidate accepted semantics into bound ADR/execution
  documents. Make the choice before the qualifying capture.

## Execution sequence

1. Freeze current branch/HEAD, dirty ownership and correctness-relevant input
   identities. Do not discard concurrent work or assume an old clean snapshot is
   current. Verify baseline regressions before claiming a fix.
2. Land ENG-01 and BENCH-02 independently with negative tests. Repair BENCH-01
   gold and split authority before rank tuning. Parser probes run in parallel.
3. Integrate ENG-02/PROD-01 capability/publication contract, then real-daemon
   replacement/recovery proof. Keep malformed fixtures in the admitted universe.
4. Implement ENG-03/04 behind explicit versioned experimental policies. Run the
   predeclared development ablation; freeze the selected policy and budgets.
5. Serial integration resolves shared schemas, consumers, fixtures and ADRs.
   Revalidate complete source closure; lane receipts with different inputs cannot
   be combined into one product qualification.
6. Produce fresh functional captures through registered owners. Reuse compatible
   immutable captures for multiple claims rather than recapturing per ticket.
7. On an admitted host, execute matched/native comparator profiles, incremental
   scenarios and one sealed holdout evaluation. Keep functional and performance
   admission separate. Publish only after all declared required cases are present.

## Verification ladder and command owners

The commands below exist at the documentation baseline; they are command owners,
not a claim that their current cases already cover the proposed changes. Inspect
the current recipe/selection and add targeted tests before running the relevant
rail. Proof output directories must be fresh and external.

```sh
python3 tools/prompt-manager/pm.py lint
just benchmark-policy-local
just benchmark-control-contract-local
just retrieval-contract-local
./scripts/cargow --lane test-integration-lane test -p quanta-index-lexical --all-features --locked
just retrieval-contract-proof /absolute/fresh-external-contract-proof
just retrieval-sdk-proof /absolute/fresh-external-sdk-proof
```

Registered performance profiles are selected through `benchctl.py`/registry;
existing `Justfile` freshness, open-loop and concurrency producers are extended
as needed. The current freshness rail measures explicit ingest, not a watcher.
Do not call a producer directly and omit required registry/custody admission for
a qualification claim. Focused tests and dirty-checkout local rails remain local
proof, not clean-source integration, release or deployment qualification.

| Proof tier | Required decisive checks | Exclusion |
| --- | --- | --- |
| Owner-local | Typed plan matrix, parser/oracle fixtures, grouped top-k exhaustive reference, snippet ranges, native mismatch rejection | Installed lifecycle and comparative quality |
| Integrated functional | Real SDK/daemon, source-bound publication/registry, incomplete capability, mutation/restart and wrong-source controls | Quiet-host speed and held-out improvement |
| Benchmark validity | Corpus/query/gold/binary/config binding, native replay, complete denominators and independent metrics | Product ranking advantage by itself |
| Quality admission | Frozen policy against untouched holdout with declared effect/regression criteria | Latency/resource qualification |
| Performance admission | Equivalent declared work, host controls, repeated samples, resource and timing boundaries | Deployment and unexercised platforms |

## Final DoD

- [ ] F01's six observed requests no longer return INTERNAL; accepted behavior is
  typed pre-execution rejection and valid paths remain intact.
- [ ] New engine E01–E08 cases are handled according to their actual classification:
  reproduced failures get regressions, source-bound gaps get their specified
  execution proof, and new capabilities are not mislabeled old contract violations.
- [ ] `count:1` cannot claim exact exhaustion over a larger symbol set; scope alias
  order and scope/record path mismatches cannot silently lose or retain wrong data.
- [ ] ENG-02/PROD-01 prove valid syntax coverage, intentional failure accounting,
  text admission and no stale facts across exercised update/restart sequences.
- [ ] ENG-03/04 prove result units, distinct-file collection and source-bound
  previews; any default change passes declared independent admission.
- [ ] BENCH-01 corrects definition gold/alternatives and freezes fresh evaluation.
- [ ] BENCH-02 refuses native/normalized disagreement for participating products.
- [ ] BENCH-03/04 report comparable units, capability coverage, latency/resources
  and updates without hiding failed/unsupported/unmeasured cases.
- [ ] Every required check has terminal selected/executed/pass/fail counts and
  source/input/environment identity, exact command, raw evidence path and digest.
- [ ] Execution SSOT, accepted ADRs and changed user-facing docs agree; superseded
  active instructions are consolidated without deleting historical evidence.
- [ ] Final report separates VERIFIED, FAILED, BLOCKED, NOT_RUN and NOT_APPLICABLE
  for the requested scope. No plan checkbox or old PASS is independent proof.

## Stop conditions and handoff

Stop only the affected action for conflicting shared-file ownership, changed
bound source, invalid corpus/gold, unbound native bytes, missing required input or
unqualified host. Continue independent safe work. Do not fabricate quiet-host,
legal, annotation or sibling-consumer proof to obtain a green summary.

An external manual approval/annotation is a user-owned input, not a coding task.
Core automated conformance does not wait for unrelated optional research tracks.
If held-out quality fails, report failure and preserve the old default; completing
implementation does not require claiming that Quanta beats every comparator.

Final handoff: exact source/dirty state; implementation versus proof status per
RFC; input/binary/environment identities; commands and terminal counts; immutable
evidence paths/digests; covered/excluded scope; remaining activation/CI/platform or
external-input gaps. No repository commit, merge, release or deployment is implied.
