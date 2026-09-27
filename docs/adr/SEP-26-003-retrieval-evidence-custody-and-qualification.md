# SEP-26-003 — Retrieval Evidence Custody and Qualification Boundaries

Status: `Accepted`

Decided: 2026-09-27

Source campaign: RBR-00 and RBR-12; applies to RBR-01 through RBR-11

## Context

The remediation campaign produced implementation checks, focused tests, clean-source receipts, actual process probes
and exploratory product pairs. Those evidence classes are not interchangeable. The remediation packet, these SEP-26
ADRs and proof tooling are part of the retrieval source closure, so changing them invalidates earlier current-source
receipts.

## Decision

### Evidence layers

1. **Implementation**: code and accepted ADR contract exist.
2. **Owner-local proof**: the owning unit, fixture or bounded process check passes on an identified source and input.
3. **Current-source integration proof**: exact selected, executed and passed inventories finish on one immutable clean
   source with bound dependencies, configuration, binaries, environment and raw terminal output.
4. **Product qualification**: admitted external corpus/model, independent gold, frozen development/holdout custody,
   actual product pair, fresh-process replay and required quality/performance gates pass.
5. **Release or deployment**: release-specific admission, deployment and activation evidence passes.

Higher layers cannot be inferred from lower layers. Compilation is not a test pass; focused tests are not repository
qualification; a diagnostic pair is not quality or speed qualification.

### Receipt validity

A receipt binds the full Git revision and dirty state, source closure manifest, selected and executed test identities,
input and dependency digests, configuration, binaries, host/runtime, exact command, exit status, raw output and artifact
digests. Any bound input change makes the receipt stale.

Validators fail closed on missing, malformed, stale, duplicate, partial, reordered, forged, wrong-source,
wrong-environment, timed-out, interrupted or tampered evidence. A `pass` boolean, count, report summary or locally
self-reported hash is not an independent oracle.

Evidence documents, referenced raw files and optional latest/baseline pointers
are read through one no-follow path-custody boundary. The reader verifies the
regular leaf and directory ancestry before and after consumption and uses
descriptor-relative no-follow opens; a prior `is_file` or `exists` check does
not authorize a later read. Only a genuinely absent optional pointer is
absence. Dangling links, linked ancestors and same-byte symlink swaps are
failures, not empty/default evidence.

Staged raw evidence and evidence-document output use directory-descriptor
traversal with no-follow opens. Raw leaves and temporary document leaves are
created exclusively; document and pointer publication uses atomic rename and
directory sync. An existing run ID or staging directory is not overwritten.
Linked output parents and duplicate raw leaves or stages are typed refusals;
an existing advisory pointer may be atomically replaced but is never followed
for writing. This local path-custody contract does not claim hostile
concurrent directory-rename or remote filesystem attestation; those require
separate proof.

Conditional same-model and incremental claims are `NOT_APPLICABLE` while disabled. When enabled, they require their
raw vector or row-set inputs, typed operations, independent replay, source/model/dependency identity and execution
terminal. They cannot be opened by a summary JSON.

### Experiment and holdout custody

Before tuning, the external experiment manifest freezes task families, split keys, corpus and query digests, candidate
matrix, metrics, guards, seed, repetition structure, failure treatment and decision rule. Development selects one final
combination. The independent holdout is opened once for that combination. Timeouts, partials and failures remain in the
verdict and cannot be removed from the latency sample.

Qualified quality keeps graded density-aware NDCG@10 as the primary metric when independent adjudicated gold exists.
Exact-span recall, MRR, Hit@1, context cost and abstention are secondary metrics. Quiet-host query and ingest latency
are distinct from functional correctness.

### Retrieval artifact and scoring contract

Consolidated from the former retrieval README; executable schemas and evaluator
remain the implementation owners. Operator commands live in that README.


Current producers emit suite/blind-pack schema 3 and runner-record schema 5
(span-accounting version 1). The evaluator retains explicit runner-record
schema 3/4 readers for immutable historical replay, not current qualification.
Unknown stamps fail before scoring. Re-capture current evidence from pinned
source; do not relabel historical bytes as the current schema.

The suite requires a `comparison_contract` with `top_k`, `tokenizer`,
`tokenizer_budget_version`, `output_unit_policy`, and the byte-span unit.
Gold spans may carry grades 1–3 only. A grade-0 (irrelevant) judgment is not
a gold span: including it would mark a task answerable and award Recall/BCY
credit despite zero NDCG gain. No-answer tasks have an empty `gold` list.
The blind pack echoes this contract without train rows, labels, grades, or
answerability bits. The runner record echoes the same contract, binds each
route to a capture, and binds captures to binary, generation, receipt,
activation, and model identities. Candidate credit uses proven byte spans;
line spans are checked projections. All-answerable external suites are valid;
an absent no-answer stratum reports `not_applicable` rather than invented
tasks. Invalid provenance, hashes, fields, or version stamps are refused.

`gold_access: false` is a runner attestation. The evaluator can verify that
the supplied pack is blind and that the record contains no gold fields; it
cannot independently prove the runner did not open the suite outside this
protocol. Use isolated runner credentials or process sandboxing when that
assurance is needed.


For 2k, 4k, 8k and 16k benchmark tokens, candidates are packed in rank order.
Packing stops at the first candidate that exceeds the remaining budget.
`BCY@budget` is the fraction of answerable eval tasks whose **every** gold
block is fully covered by packed candidates. `file_recall` and `block_recall`
are task-macro averages. `no_gold_abstention` is the fraction of no-gold tasks
with explicit abstention, or `not_applicable` without a real no-answer
stratum. `mean_context_tokens` is averaged across all eval tasks. The report
gives these metrics for each route and the selected route pair's deltas and
paired win/loss/tie counts (where success means BCY for answerable tasks and
abstention for no-gold tasks). A candidate matching a gold file but missing
its gold byte span cannot earn block credit.

The current scorer reports span-aware Recall@1/5/10/20, MRR@10, graded NDCG@10 (only when every
eval answerable gold label carries a reviewed grade, else `not_applicable`),
file-only recall as a secondary view, and both chunk-level and deterministic
same-file-collapsed rankings (best chunk per file). NDCG credits each gold span
only on its first covering candidate: overlapping chunks cannot earn the same
gain twice or inflate NDCG above 1. The report labels this scoring contract
`rb-rank-context-density-first-coverage`; reports with a different scoring identity are
not comparable on overlapping-chunk suites. Duplicate candidate byte spans are
rejected. Distinct overlapping chunks consume rank and context budget but
earn no second relevance gain. Primary metric is
`ndcg_at_10` when graded, else `recall_at_10`. Reports include per-query rows,
per-route status counts and mean latency, sample counts, and a deterministic
stratified-bootstrap 95% interval on paired deltas when the sample reaches 20, else an
explicit insufficient-sample marker. Re-scoring immutable records is
deterministic under row order; scores depend only on recorded spans and
statuses, never on runner identity strings.

W0-A relevance rubric: a grade-3 gold span is independently judged sufficient
answer evidence; grade 2 is substantial but incomplete evidence; grade 1 is
weak supporting evidence. Grade 0 is excluded from `gold`. Only full byte-span
containment earns rank gain; partial overlap earns zero. Each gold span is
credited on its first covering candidate, and a candidate uses the highest
newly covered grade. Its gain is `(2**grade - 1) * U/C`, where `U` is the union
byte length of newly covered gold spans and `C` is the candidate byte length.
The ideal ranking is one exact candidate per gold span in descending grade
order. Thus an exact span gets full credit, while a whole-file hit containing
10 relevant bytes in 1 MB gets only 10/1,000,000 of that gain. Rank ties are
resolved by the recorded unique rank; the report publishes both raw chunks
and deterministic best-chunk-per-file collapse. Token-budget BCY remains a
separate, primary context-coverage diagnostic. Byte density is an explicit
precision policy, not a claim that shorter context is always semantically
better; report language/category strata and BCY alongside NDCG.

The formula alone does not qualify a quality claim. `QUALITY_DELTA` still
requires W0-B's independently adjudicated graded gold, verified isolation,
model/source/receipt admission and paired uncertainty. Current admission state is tracked by the execution ledger.

The report is evidence only for the supplied frozen suite, pinned repository
and runner record. No arbitrary pass threshold or claim of production retrieval
quality is inferred.

### Corpus, isolation and paired decision boundaries

Use one pinned repository commit and exact canonical path/SHA universe per
pair capture; aggregate actual per-repository pairs rather than inventing one
multi-repository commit. Retain unsafe/ignored/binary/oversize exclusion reasons.
A corpus manifest does not attest every product's indexed/searchable universe.
Current suite/blind-pack schema 3 and runner schema 5 share the comparison
contract; historical runner 3/4 readers are immutable replay only.

Supported chunk strategies are `whole_file`, `fixed_window_strict`,
`fixed_window_line_aligned`, `brace_heuristic` and Semble-owned `semble_native`.
Pin each product's package/commit, dependency lock and model assets. Semble 0.6.0
is a frozen comparator revision, not a latest-release claim. Tiny oracle fixtures
prove mechanics, not real developer relevance.

Keep development/holdout query families, paraphrases, answer spans, files and
definitions separated as declared; exceptions require review and both-side
rationale. Engine/model output alone is not independent adjudicated gold.
Qualified gold requires two distinct annotation authorities plus adjudication.
Verified `isolated` execution must be unable to read gold and retain an access-block
log; `attested` runner claims cannot issue isolated quality proof.

`declared_top_k_v1` requires top_k>=10 for the @10 primary. Larger-than-captured @k
is `NOT_APPLICABLE`. Predeclare margins/guards/order/seed/repeats/failure treatment;
choose one final development candidate before holdout. The paired 95% CI lower
bound must meet the declared noninferiority margin (default zero) and the claimed
improvement rule, with every stratum guard. Use query/repository clusters and
leave-one-repository-out sensitivity; latency preserves query/root structure.
Repeated calls are not independent queries. Bundle effects need separate ablation
and holdout before attribution to individual changes; no post-hoc exclusions.

### T00-T17 blocking matrix

| ID | Positive contract | Required negative controls |
| --- | --- | --- |
| T00 | Both runners' exact tracked path+SHA universe | Dirty/wrong revision, ignored/extra/missing/binary/oversize/symlink input, divergent filter |
| T01 | Pinned label bytes/lines, safe paths, blind pack and universe digest | Wrong location/hash, absent required gold, leaked grade/gold, mismatched universe |
| T02 | Dev/holdout and query-family separation, reviewed categories/grades | Duplicate/normalized/shingle-near queries, cross-split family/span leakage, invented no-answer |
| T03 | Exactly one eligible task/route row, ordered ranks/spans, exact contract and capture pins | Missing/extra/duplicate row, unknown field, nonfinite time, null versus zero confusion, dangling capture, forged lock |
| T04 | Hand-calculated span Recall/MRR/density-NDCG/BCY and independent rubric | Wrong lines, partial span credit, duplicate boosting, budget overflow, whole-file full credit |
| T05 | Separate fresh daemon, public readiness before publish | Wrong socket/state root, stale daemon/index, no readiness |
| T06 | SDK publish/seal receipt and exact CAS activation acknowledgement | Direct-IPC substitute, partial seal, digest mismatch, conflict, query before activation |
| T07 | SDK lexical/semantic/hybrid real ranked spans at expected generation | Wrong route/generation, typed timeout/error turned into empty success, capped result called exhaustive |
| T08 | Original byte slices, line bounds and stable chunk IDs | UTF-8/CRLF/BOM/EOF errors, empty/overflow span, nondeterministic IDs |
| T09 | Supported parser boundaries and explicit fallback coverage | Unsupported grammar, parse/long-declaration failure, silent whole-file fallback, overlap inflation |
| T10 | Rebuild semantic sources per chunk strategy with pinned real model | Reused vectors from another strategy, hash-dev claimed as model quality |
| T11 | Real pinned Semble mapping to exact admitted bytes/spans | Truncated snippet as full chunk, path drift, skipped query, missing model, filter mismatch |
| T12 | Complete real pair with equal commit/files/queries/k/budget/host and rebound pins | One-sided/public-score substitute, partial samples, wrong host/model/cache, repeated few-task qualification |
| T13 | Same immutable raw gives identical aggregate/disagreement/digest after relocation | Path/order-dependent score, omitted errors, duration exceeding enclosing phase, invalid result marked qualified |
| T14 | Registered commands, external output, fresh final-path public replay | Implicit CI downloads, dirty output root, unselected test claim, stage-only replay |
| T15 | Same-model claim: pinned weights/tokenizer/normalization/precision and full raw vectors within declared tolerance | Same name/dimension alone, reordered/subset/forged vectors, wrong model/config/executable |
| T16 | Incremental claim: SDK operation, exact activation and full fresh/incremental row equality | Stale hit, mixed generation, no-op change, sentinel delete, mtime-only truth, rebuild called incremental |
| T17 | Closed qualified admission binds source/corpus/suite/license/two annotators/adjudication/assets/host/receipts | Exploratory promotion, duplicate authority, stale or tampered inputs/receipts, undeclared cache |

T15/T16 are `NOT_APPLICABLE` when same-model/incremental claims are false; do not
make them unconditional pair blockers. When enabled, rederive from raw vectors
or before/operation/fresh/incremental full rows plus actual execution/binary/
configuration/collection/source custody. T16 preserves unrelated owners,
requires meaningful typed operations and one final successful build terminal;
reject contradictory/duplicate/later terminals and bool/float scalar aliases.
Summary pass/count JSON plus a digest is not independent evidence. Fault/restart
needs actual fault execution; full-row comparison alone does not prove it.

Returned-window diagnostic is separate from scorer input. Bind its SHA and
record/pack identities, exact task/route inventory, candidate rank/path/span,
status and hybrid lane contributions. Reject extra/duplicate/nonfinite/forged
fields. Distinguish runner boot/readiness, opaque publish/activate, assembly,
corpus reverification and shutdown; do not invent embed/index/seal subphase
timings. Requested floor and actual initial-fetch trace must agree; preserve
current diagnostic/protocol versions and strict required fields.

### Qualification and sample floors

- Distinct claims: `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`,
  `QUALITY_DELTA`, `PERF_QUALIFIED`. Quality/performance require valid contract,
  SDK and pair proof on the same bound source/run. Preserve specific failures.
- `scope=qualified` requires schema-closed W0-B admission (T17), binding license,
  independent gold, source, repository, corpus/suite/pack, model/tokenizer,
  Semble lockfile, host/cache and exact contract/SDK receipts. Recheck before
  capture and from frozen artifacts. Exploratory scope cannot accept admission
  to manufacture qualification; its quality/performance are not applicable.
- Same exact query text; declare internal transformation. Explicit Semble-to-
  canonical path mapping and both-side path/SHA diff digest are mandatory.
  Same file count is insufficient. Keep warm API latency separate from process
  startup; include each product's embedding/indexing cost consistently.
- Quiet same host: CPU/power/cache configuration, no competing builds/benchmarks,
  thermal/frequency checks, capture-time observations and actual reservation.
  No load-average-only or preflight-only performance approval.
- Cold/time-to-searchable: at least five fresh state roots per repo/system,
  alternate product order, prebuild/provision outside timing. Include discovery,
  chunking, embedding, publish/seal/activation and first successful query.
- Warm: at least 20 distinct admitted tasks, exactly one eligible Quanta route,
  at least one warmup pass/root, five fresh roots and 1,000 valid observations
  per eligible route. Each floor is independent. Example: 20 x 10 x 5.
  Both products follow the same driver-issued randomized schedule.
- Raw monotonic query durations must fit declared enclosing windows/tolerance;
  preserve cold/first-query evidence. Unknown duration is null, not zero.
  Missing eligible timing blocks speed. Do not gate on underpowered p99.
- Freshness/update/rename/delete and concurrency are separate scenarios.
  Primary pair is serial warm latency; concurrency capacity needs equivalent
  client/arrival contracts and full offered/completed/error accounting.
- `verdict.json` uses the existing schema-v2 owner, rederived provenance and
  per-comparison digests. Missing and not-applicable T IDs are disjoint. Every
  verified comparison is reported; no empty comparison success. Fresh public
  replay at the promoted path must reproduce canonical identities and states.

### Independent fixture oracle

Canonical JSON, tokenizer, byte-span and split-leakage fixtures are independent
Python/Rust golden inputs. Neither implementation generates the other's expected
values. Hand-review offsets/counts and compute frozen hashes independently.

### Agent recordings

Recorded A/B/C outcomes retain complete baseline/post-test identities and event
sequences. Fail-to-pass and pass-to-pass have explicit numerators/denominators;
resolution requires every baseline failure repaired and no passing-test regression.
First-useful-evidence timing is conditional on actual useful evidence; absent
evidence is null timing plus visible coverage. Imports bind the importer, not an
authenticated original producer. Small/self-selected recordings do not establish
significance or benchmark-wide quality.

## Consequences

- The historical RBR packet is recoverable from Git history, not a live status authority.
- Current decisions live in accepted ADRs; unfinished execution work lives in the SEP-27 execution SSOT; accepted comparison/admission contracts live here.
- A normative SSOT change requires new source-bound proof before current qualification.
- Current verification and qualification status is maintained in the
  [execution SSOT](../plans/sep-27-misc/tickets/INDEX.md),
  not in this decision record.
