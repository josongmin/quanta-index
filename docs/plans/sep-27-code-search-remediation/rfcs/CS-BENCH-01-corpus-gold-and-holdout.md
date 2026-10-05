# CS-BENCH-01 — Shared corpus releases, independent gold and holdout

Status: acceptance **OPEN**. Independent corrected definition gold, fresh release
and sealed holdout execution are **NOT_RUN** for this acceptance scope.
Category: benchmark inputs. Finding: F07; supports F02/F04 and G02.

Existing release/binding and symbol-coverage helpers remain implemented; this
status does not claim those owners are missing. The current benchmark overlay is
not an independently audited gold/holdout release. Remaining input/acceptance
boundary: [CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls).

## Purpose and RCA

Independent definition gold must distinguish docstring examples from actual
declarations and include valid alternatives such as Rust `const fn` and
TypeScript object methods. Text occurrence remains a separate content-search
oracle. The removed diagnostic bodies are recoverable through the
[plan archive](../../ARCHIVE-INDEX.md); their old dataset counts are not release
acceptance. Correct the oracle, then freeze fresh development/holdout inputs.

No retrieval engine's output, including Quanta's symbol producer, is sufficient
by itself to define gold for a comparison involving that engine.

Sep-28 code addition: `gold_oracle.py` derives raw UTF-8 literal occurrences
and a deliberately narrow named-function declaration cohort from release
source bytes, with independent Python/Rust/TypeScript fixed-span fixtures.
`corpus_binding.py` publishes/reconstructs an external capsule and label-free
blind pack with source/oracle/parser identities and `query_family_id`; the
capsule explicitly marks labels `mechanical_unreviewed_diagnostic` and holdout
custody `unsealed_external_custody_required`. It does not replace the existing
evaluator suite/pack format or provide a reviewed fresh release. Human labels,
valid external corpus and sealed holdout execution remain open.

## Target input release contract

Extend the existing [corpus release](../../../../tools/benchmark/corpus_release.py),
[binding](../../../../tools/benchmark/corpus_binding.py) and retrieval
[corpus set](../../../../tools/benchmark/retrieval/corpus_set.py) owners. Put reusable
recipes/oracle adapters under the existing benchmark tree; retire the external
one-off recipe as an executable authority after migration and parity review.
Do not introduce another corpus manager, CLI or manually synchronized manifest.

Repository contents: schemas, pinned acquisition/build recipes, licenses metadata
or references, oracle adapters, small fixtures and split policies. External
contents: checkout/materialized corpus, generated gold, model assets, indexes,
raw captures and immutable releases. Corpus payloads remain outside this repo.

Each release binds repository URL and commit, file inventory and hashes, language,
encoding, file selection policy, symlink treatment, binary/generated/vendor
policy, size limits and required provenance. Every comparator attests the same
admitted universe or reports its measured exclusions. "Same Git repository" or
equal file counts alone is not equivalent indexed content.

Publish a new immutable release when files, labels, recipes, oracle versions or
splits change. Historical evidence retains its original release identity; never
overwrite gold and reuse old scores as corrected evidence.

## Query and relevance contracts

Each task binds stable ID, release, intent, original query, literal/regex/native
policy, case semantics, scope, result unit and independently established labels.
The runner receives a blind query pack without labels or answerability bits.

The engine re-audit found a required semantic distinction: native Quanta matches
NFC text with its versioned token/case/regex rules, not arbitrary original-byte
identity. Bind normalization and regex-engine semantics in each task/profile.
Independent scan oracles must implement the declared common semantics, or split
raw-byte and native-normalized cohorts; original source spans remain the location
oracle in either case. Do not silently treat differently normalized literals as
equivalent cross-product requests.

| Task class | Independent oracle | Label shape |
| --- | --- | --- |
| Literal/identifier occurrence | Enumerate frozen source with explicit scan semantics | File plus matching byte spans |
| Regex | Reference matcher for admitted common semantics; small semantic fixtures | Match set or typed unsupported |
| Definition | Independent language AST/compiler/declaration census | Kind, qualified/local name, all valid declaration spans |
| File/path locator | Frozen inventory and explicit path/query semantics | Set of relevant repository/path identities |
| Context/workflow | Independent task-specific judgments/outcomes | Graded or required-block labels; separate track |

If independent extraction is unavailable for a syntax form, mark that stratum
unsupported or unjudged; do not emit an empty definitive gold set. Preserve oracle
implementation/version, source hashes, extraction rules and adjudication status.
The product producer can cross-check but cannot be its own sole expected-result
generator. Small hand-specified fixtures provide a second oracle for adapters.

Classify docstring examples as content tasks. For ambiguous names, qualify the
query or label all valid alternatives. Distinguish alternative answers from
multiple jointly required context blocks; the latter uses an all-required metric,
not definition MRR. No majority vote among products creates ground truth.

Include natural no-answer, wrong-repository and intentionally absent identifiers.
A failed parser or incomplete indexed scope is not evidence of absence. Answerable
and unanswerable strata keep explicit denominators.

## Development and fresh evaluation

Keep previously inspected tasks only as a corrected, versioned diagnostic or
regression set. A reshuffle cannot turn those tasks into a fresh holdout.

Proposed first expanded release: at least 12 repositories and 1,200 **fresh** cases
across Rust, Go, Python, TS/JS and other independently supported languages, with
small/medium/large repository strata. Include repeated names, definitions versus
references, exact case, qualified names, long files, Unicode, path filters,
regex, negative queries and malformed-source text. Include 1–2-character and long
identifiers, punctuation, chunk-boundary matches, decorators, const functions,
methods, comments/examples, test declarations and near-matching false friends.
These sizes are engineering
targets, not statistical power guarantees or published SOTA requirements.

Split before tuning by repository and query family where possible. Detect shared
files, forks, copied fixtures and near-identical query templates across splits;
declare unavoidable overlap. Seal holdout labels from the runner and tuning
process using existing custody/isolation mechanisms. Predeclare a sufficiently
sized primary cohort based on variance and minimum detectable effect (BENCH-03).

Once inspected to choose a policy, a holdout becomes development data. A later
attempt needs a new frozen challenge release. Generated examples and externally
proposed benchmark tasks are not automatically representative of real users.

## Tests and DoD

- [ ] Python strings/examples, Rust const/async/generic functions and TS object
  methods have positive/negative oracle fixtures and byte-exact alternatives.
- [ ] All existing gold is reclassified or repaired; every changed case has a
  machine-readable reason and a new release ID, not a silently changed score.
- [ ] Every admitted file/task has exactly one identity; duplicate, stale,
  wrong-source, malformed, incomplete and split-leaking manifests reject.
- [ ] Comparator inventories bind to the common release; exclusions remain visible.
- [ ] External recipes are ported into canonical owners and reproducible without
  temporary paths; corpus/gold payloads remain external.
- [ ] Development/holdout partition, independent oracle identities and blind packs
  are frozen before ENG-03 default selection.
- [ ] Expanded-case counts and stratum coverage are emitted; unavailable strata
  are explicit, never default zeros or passing skips.

Manual legal approvals or new human annotation are user-owned inputs, not coding
subtasks. They only block the dependent optional release/claim; automated exact
and supported-AST tracks can proceed with already authorized inputs.
References: [S07/S08 and R01–R04](../references.md).
