# CS-INT-01 — Integration, contract cutover and qualification

Status: **PARTIAL IMPLEMENTATION / QUALIFICATION OPEN**. L1–L5 repairs and
selected integrated executions are present. Current static policy and native
normalization rejection checks are **FAILED**; final combined-source execution
is **NOT_RUN**. Historical receipts do not qualify the latest merge.
Category: integration. Covers F01–F09 and G01/G02 through the nine owner RFCs.
Source baseline: [readme](../readme.md); historical proof: [evidence](../evidence.md).

The [final engine audit](../engine-audit.md) is required input for engine work.
Correctness order is domain/empty validation and count-window truth; canonical
scope/source identity; product coverage/name authority; semantic witnesses and
collector budget enforcement; only then relevance tuning. Existing exact grouped
collection, cancellation and valid pinned-snapshot cursors must be preserved.

## Current remaining-work audit — 2026-09-27

Observed HEAD: `b42a9b5d6a91e1895ad8d49367423e8c446bfed9`, with concurrent
dirty L4 tests and benchmark Python work preserved. The 1,119 Git-visible
code/config input map has SHA-256
`19cf675b4ecd054dfdd86033b90c35b9c393c33700d4a60d94d9167278ee3eb1`;
`Cargo.lock` SHA-256 is
`a3606a25bcf971667c437ec658acd2de4c787d2662dfffdb417ec15537e3ebf5`.
This is a static-audit source profile, not a build/dependency/binary receipt.
Rust is 1.92.0, aarch64-apple-darwin; the comparator probe uses Python 3.13.9.
Pre/post source inventories, dirty paths, commands, logs and the fixed native
oracle are retained at `/private/tmp/qi-remaining-work-audit-20260927-h8xzg1yg`.
The source profile excludes planning/proof documents and generated artifacts;
changed-document navigation/whitespace checks are reported separately.

During this refresh, another writer committed the shared tree as
`a6a39cb34d3d0d9ebc3b362b209814c5f1337c7d` and continued editing benchmark/
proof code. A later 1,121-file observation has source SHA-256
`209d4596fc90c8ab5a0aae40f2fc1b7751f3fd34a2ffcc301f52c8ecc835e7c1`.
Six code/config inputs differ from the initial observation (two proof drivers,
`code_search_workflow.py`, live external capture, Sourcegraph and its tests).
The scored comparator, coverage serializer, regex admission and ranked-key
dependency edges retain the same source bytes. Whole-checkout receipt transfer
is **BLOCKED by source drift**. Preserve both inventories and narrow executed
claims; neither observation is a final combined-source test receipt.

Final audit observation at the same `a6a39cb3` HEAD has source SHA-256
`968f52d3c432344a0bb9626f9fe4e94b54788ba569112a3aca28a9b770b0a69c`.
Nine code/config paths differ from the initial map as concurrent work continued;
all Rust/config inputs and the 15 specifically audited owner/checker/contract
inputs retain their recorded bytes. The cycle gate and native mutation probe
were repeated after the commit and returned the same failed invariants.
Machine-readable action/artifact/source custody:
`/private/tmp/qi-remaining-work-audit-20260927-h8xzg1yg/receipt.json`, SHA-256
`48b690ae7d3320a053a4ab3eb42a9c677900822ea2c0f9d81e7dc41476dd5210`.
Changed-document file-target check: **VERIFIED**, 14 documents and zero broken
targets. Prompt-manager lint and whitespace check exit 0; repository navigation
still fails on the four historical proof links. The existing path checker does
not validate heading anchors. No Rust build/test or release action was executed
by this remaining-work audit.

Subsequent live edits to L4 test imports, verification guidance and benchmark/
proof work appeared after that recorded observation. Preserve them; the receipt
above is an audit snapshot and does not qualify those later inputs. No new
combined build/test claim is made by this document refresh.

The merge replaces the private `tantivy-sstable` patch with committed ranked-key
tables and changes the sealed manifest from format 7 to 8. Seal/open, ranked
comparison, resident estimates and migrated fixture layouts changed. Earlier
format-7 passes remain historical successes; they do not establish execution of
the new paths. Format-7 indexes require rebuilding, as specified by
[ranked-key tables](../../../../docs/ranked-key-tables.md).

| Remaining ID / owner | Current evidence and status | Concrete completion condition |
| --- | --- | --- |
| INT-C1 / ENG-03 integration | **FAILED:** `just rust-policy` stops on `ranked_keys → ranked_page → ranked_keys`. The table module imports three column-name constants from the collector, while the collector imports table types. | Move shared constants into a lower leaf/appropriate existing field owner; retain one definition and unchanged column names. Run the cycle gate and remaining static policy, then the ranked/grouped regression slice. Do not baseline the new cycle as accepted. |
| BENCH-02 / F08 | **FAILED rejection invariant:** fixed native cs bytes identify `other.rs`; changing only normalized paths/hit makes current `product_result` score 0→1 instead of refusing. No actual production tampering is alleged. | Bind and re-decode native response bytes during evaluation through the same versioned decoder used by acquisition. Reject absent/mismatched raw, wrong request/source, malformed/partial/duplicate data before scoring. Prove the unchanged-native mutation is rejected, then exercise each admitted product format. |
| L4-R1 / ENG-04 | **BLOCKED aggregate heap claim:** source reserves a policy base plus estimated-state charge; AST/HIR, automata temporaries, retained engines and search caches lack one physical allocation admission boundary. No current runtime overrun is established. | Admit expensive temporary/retained allocations before they occur, preserve canonical regex truth/ranges and preview-refusal hit stability; prove lifetimes, multiple/repeated leaves and interruption. See the [residual](../handoffs/L4_REGEX_BUDGET_RESIDUAL.md). |
| L2-C1 / ENG-02 + BENCH-04 | **VERIFIED source cost mechanism; measurement NOT_RUN:** every changed generation serializes the complete effective coverage map. Historical one-file bytes are not a format-8 cost result. | Measure total fresh bytes/latency at increasing file counts, including coverage and touched text/ranked sidecars. Declare the supported ceiling; use committed incremental shards/inheritance if needed to meet it. Validate replacement/delete, integrity and lifecycle together. No stale-result repair is implied. |
| INT-R1 / combined L1–L5 | **NOT_RUN:** final merged-source compile/tests/Clippy, exhaustive required real-daemon SDK/crash/restart and unified proof aggregation. | Freeze one combined code/dependency/config source, select the existing registered rails, explicitly execute ignored process cases, retain terminal nonzero execution counts and raw/binary/input identities. Bind all required claims to that source; compile alone is insufficient. |
| INT-D1 / evidence navigation | **FAILED:** `lint-doc-paths.py` reports four broken links in two historical proof preimages. | Preserve existing receipt bytes. Choose an explicit archival navigation policy or reissue affected dependent receipts after a truthful path fix; rerun repository doc-path lint. Do not silently edit a hashed report/template or fabricate its receipt. |
| MISC-04 / external lexical enrollment | **VERIFIED source omission; repair NOT_RUN:** `test_live_lexical_external.py` is absent from the existing Just selector, test-authority catalog and explicit source-closure test roots. Catalog lint alone does not discover it. | Complete the existing [MISC-04 acceptance](../../sep-27-misc/tickets/INDEX.md): enroll the two tests under their producer and required rail/capture closure, guard omission/mutation, and execute both. Keep this work owned once in MISC-04. |
| BENCH-01/03/04 and rollout | Corrected independent gold/holdout, ranking/context utility, equal-work latency/resource/incremental measurement and installed/external-producer activation are **NOT_RUN** in this audit. | Establish the required input/host identities and run the declared registered profiles. Keep optional ranking experiments outside core correctness closure and retain the old default if admission fails. External issuer and format-8 reindex/activation need their own receipt. |

Current commands: `just rust-policy` exited 1 at the module-cycle gate;
`python3 tools/ci/lint/lint-doc-paths.py` exited 1 with four paths;
`python3 tools/prompt-manager/pm.py lint` exited 0. Policy commands after the
cycle gate did not execute as part of that invocation. The standalone comparator
probe exited 0 after observing the failed rejection invariant; this exit is
diagnostic completion, not a passing conformance result. Its first attempt was
refused because `/tmp` is a symlink; the successful reproduction uses the
canonical `/private/tmp` evidence root, and both logs are preserved.

No new L1/L3/L5 behavioral counterexample was reproduced in this remaining-work
audit. Their existing source fixes are retained; execution of the latest merged
source remains INT-R1. The newer L4 SDK attempt records seven executed passes on
its frozen pre-merge source, but its aggregate validation is BLOCKED and its
owner selection is NOT_RUN. It cannot close L4-R1 or INT-R1.

This table updates code-search acceptance. Shared benchmark process, custody,
bounded-I/O and execution work continues to be owned once by
[MISC execution SSOT](../../sep-27-misc/tickets/INDEX.md). Earlier selected
execution details remain in the [integration audit](../handoffs/L1_L5_INTEGRATION_AUDIT_20260927.md).

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
