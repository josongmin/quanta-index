# SEP-21 execution evidence

## 2026-09-26 direct-exit process-group custody

Review and repair on shared dirty main
`577d60b518344163145ad2f1afe1f3e7c656762e`; another writer advanced main to
`b24d11489d250613c4871df6d65fa3d7e70daef4`. Other writers' changes were
preserved; no commit, push, deployment or activation was performed.

- RCA: the session shim returned after its direct child exited. A same-group
  background descendant that closed stdout/stderr remained alive after exit 0,
  exit 7 and SIGTERM, while the controller had already accepted completion or
  returned the direct-child error. The prior liveness pipe only covered a live
  shim, not this terminal transition.
- Extended the existing `producer_execution.py` owner. The shim publishes the
  actual child exit code through a private four-byte record and retains group
  custody. The controller drains both outputs while awaiting that record;
  it kills the group before polling/reaping the leader, then performs the
  existing bounded drain/reap and interprets the record. An unreaped child PID
  pins identity even if the reported shim is killed externally before cleanup.
  Missing/partial/oversized/out-of-range records and unproved guard termination
  cannot establish successful execution. Error paths close both control pipes
  and restore handlers; cleanup does not signal an already-reaped identity.
- One public execution API and unchanged success metadata; no receipt IR,
  second executor, consumer-local cleanup or OS waitid dependency. Parent-death
  cascade remains in the same owner. Deliberately escaped sessions are still
  outside sandbox containment, with retained pipes causing bounded explicit
  failure instead of completed execution.
- Meaningful RED through the actual owner:
  `uv run --locked --offline --extra dev python -m pytest
  tools/ci/tests/test_criterion_capture.py -q -k direct_child_exit --tb=short`:
  exit 1, 3 failed / 31 deselected, 10.41s. All three failures observed a live
  background descendant after its direct child exited. Raw `red.log`, SHA-256
  `87b344d70864497b5935be28255dfa92c7df1033300903f65dd045670bfe0e71`.
- RR tightened report/cleanup ordering before acceptance. Interim 62-owner
  pass preceded that change; the controller-owned ordering then produced
  60-pass/2-fail because missing-record paths first encountered macOS EPERM
  signalling a dead group, losing the primary diagnostic. The owner now
  validates the private child-status record before the kill and preserves that
  failure plus any incomplete-cleanup context. Assertions were not weakened.
  Both intermediate logs remain under the raw directory.
- The first complete locked owner command:
  `uv run --locked --offline --extra dev python -m pytest
  tools/ci/tests/test_criterion_capture.py tools/ci/tests/test_portable_proof.py
  -q --tb=short`: exit 0, 67 passed, 23.37s. Coverage includes normal/nonzero/
  signalled child exit, the same transitions with an externally killed reported
  guard, malformed and absent terminal records, descriptor closure, output
  larger than pipe capacity, reaped-PID rejection, timeout, SIGTERM, nested
  owner SIGKILL and held-pipe escape with both live and exited direct children.
  `owners-full-final.log`, SHA-256
  `652536311df787f9e93eba1efe2ab3909ac7a7ca00bd25c14ccb721f946e1c14`.
- The first caller command:
  `uv run --locked --offline --extra dev python -m pytest
  tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_capture.py
  tools/ci/tests/test_benchmark_profile_capture.py
  tools/ci/tests/test_retrieval_contract_proof.py -q --tb=short`:
  exit 0, 87 passed, 19.65s. `callers.log`, SHA-256
  `96f141e36800090650ccaea3796c9a89154c7dee05fed41a964bbe5148416818`.
  These are local owner/caller tests, not actual retrieval campaign or release
  command qualification.
- Locked runtime: Python 3.13.9, pytest 9.1.1, macOS 15.6 arm64;
  `uv.lock` SHA-256
  `c9064e8ead8593a6054d50c9b8e7523026d07d9ae7c95ddbf1268d273d6dad73`.
  Executor before the final SIGCHLD admission change, SHA-256
  `6d6c320689e8e576165fd99bd64926f90a832148b64c32d9d267970bd4b30c4d`;
  corresponding owner test SHA-256
  `5afa4fa0754766582c5d7f950cf78fbc3fec306e1fba04d1cd7c9f25cd16c9ed`;
  portable caller unchanged at
  `ab1f64fde89b70a62df05a935a27fd43e538e35b4d0329f0ccc3d22d6d1ce5e7`.
  Scoped Ruff check/format passed. Raw directory:
  `/tmp/quanta-terminal-custody.YUbAEr` (temporary, not a release archive).
- Final RR input-boundary counterexample: an external SIGCHLD handler could
  reap a direct child behind the owner's back; SIG_IGN can auto-reap it. Either
  destroys the unreaped-PID custody assumption. The actual owner initially
  launched work in both cases. Locked `test_criterion_capture.py -q -k
  external_sigchld --tb=short`: exit 1, 2 failed / 51 deselected, 20.52s;
  `red-sigchld.log`, SHA-256
  `4191f63ad20f7ee26f23bc1dd240015e7f6aee3b34331d76b3e275cde68d7475`.
  The executor now requires default SIGCHLD before creating pipes or children;
  neither unsupported mode is silently reset or accepted. Both negatives
  independently assert that the command's marker was never created.
- Final locked owner command is the same complete two-file command above:
  exit 0, 69 passed, 32.27s. Raw `owners-with-reaping-custody.log`, SHA-256
  `a46b8aca6289109f9d438e1d120048a3c12dcaf9454f9a7c3d4b7f6845dbd253`.
  The same complete four-file caller command re-executed after the admission
  change: exit 0, 87 passed, 20.91s. Raw
  `callers-with-reaping-custody.log`, SHA-256
  `5c4fb45e71cf50065c5ab33ae6e6ed52d2e29a3f7b59564ff3ca4896cfd94aa5`.
  Final owner SHA-256:
  `4ca9bf58c5061aa9125893b3312bc14706d5150cd24131900d7463667def922e`;
  final owner test SHA-256:
  `69290229cd6516697a007861124625d74fce5c9e6d40b06394e24593516c6867`.
- System-Python compatibility only:
  `python3 -m pytest tools/ci/tests/test_criterion_capture.py -q -k
  'direct_child_exit or terminal_wait_drains or external_sigchld' --tb=short`:
  exit 0, 9 passed / 44 deselected, 16.56s; Python 3.9.6 / pytest 6.2.5,
  unlocked. `system-python-final.log`, SHA-256
  `be8692c5c291fa8f91da318a438192b4e2398b64beaabed24f22ad23a3bf46c3`.
  Its unrelated urllib3/LibreSSL warning is retained. This is not the managed
  proof environment or repository qualification. Scoped Ruff check/format and
  final whitespace check passed; owner/caller hashes stayed fixed across final
  local proof execution.
- Residual audit: portable proof's actual toolchain/wrapper execution binding
  remains R0 OPEN_PROOF_GAP. `semantic/build.rs` still performs per-owner
  serial deletes; the 1820-owner batch multiplier is source-observed, not
  established daemon timeout RCA. Neither issue is marked repaired. The prior
  mandatory daemon gate remains FAILED; no unchanged retry or weakened oracle.
  Dirty-source, Linux/release, paired-process and operational proof obligations
  remain in the current residual table; the active goal is not complete.

## 2026-09-26 bounded producer execution and direct proof calls

Started on shared dirty main `9e443489e06167bfbed3b4503af558495e27206d`;
another writer advanced it to `577d60b518344163145ad2f1afe1f3e7c656762e`.
No commit, push, deployment or activation was performed by this repair.

- RCA: failure cleanup killed an owned group then called unlimited
  `communicate()`. An escaped session holding the pipe prevented return.
  Portable proof also used unrestricted subprocess calls for commands, SDK
  recipes, Git and executable versions.
- Extended `producer_execution.py`, the existing execution owner. It now owns
  a ten-second cleanup deadline shared by drain and reap, explicit primary
  failure plus incomplete-cleanup reporting, pipe close, and restoration of
  signal handlers. Repeated cancellation does not interrupt the bounded drain.
  Its error is a leaf `ProducerExecutionError(ValueError)`; importing the
  execution owner no longer requires benchmark envelope/schema dependencies.
- RR caught nested-session custody loss introduced by simply routing direct
  proof calls through `start_new_session`. The same owner now starts a small
  isolated session shim whose parent-liveness pipe kills the group when its
  controller dies. Nested owned sessions cascade through descriptor EOF even
  when outer cleanup/finally cannot run (SIGKILL). The shim checks private
  session/group ownership before using group kill; it is not a second executor.
- Portable proof Git/tool identities use 30-second execution deadlines;
  proof/build/SDK recipe calls use 7200 seconds per command. All use the common
  owner, with no raw subprocess alternate. Unsuccessful execution cannot append
  a successful command record. Raw argv, inherited environment and receipt
  semantics remain the existing portable-proof contract. No full-rail elapsed
  time, clean-source or external sandbox containment is claimed.
- Pre-patch meaningful RED: locked pytest of `test_criterion_capture.py` and
  `test_portable_proof.py`, `-q -k 'failure_cleanup_has or
  direct_proof_command_has' --tb=short`: 2 failed / 40 deselected, 2.55s.
  The actual callers supplied no execution/cleanup timeout. Failures are in
  terminal tool output, not an invented raw RED log.
- Intermediate deadline owners: 44 passed in 12.68s; nested custody repair:
  45 passed in 15.71s. The latter raw `guarded-owners.log`, SHA-256
  `03a0d50251ef6973de075f427ee3fe6e6c32e89892c03eaa937181887a6c45df`,
  in `/tmp/quanta-execution-deadline.G7FaGN`. Expanded real direct-command tests
  then exposed two fixture defects: non-atomic PID publication and an escaped
  child not starting before a one-second deadline on the loaded host. The
  resulting 45-pass/2-fail `final-owners.log` is preserved. PID publication is
  now atomic, and only the reproduction's execution allowance changes to five
  seconds; production deadlines are unchanged. Final locked command of both
  complete owner files, `-q --tb=short`: exit 0, 47 passed, 34.13s. Raw
  `final-owners-fixture-fixed.log`, SHA-256
  `46d258d02efb7643548ad3c3c70d205bec556f9b959ecc5ff21abd8473aa7ff0`.
- Caller regression: `uv run --locked --offline --extra dev python -m pytest
  tools/ci/tests/test_retrieval_contract_proof.py
  tools/ci/tests/test_retrieval_capture.py -q --tb=short`: exit 0, 60 passed,
  50.78s. `callers.log`, SHA-256
  `434b39d41ce4b980c30a8f5b71935a05230cd1ba4bbfaeeeb1c65b48dc93b33d`.
  The subsequently added static-import closure assertion ran through locked
  `test_retrieval_contract_proof.py -q -k shared_execution_owner --tb=short`:
  exit 0, 1 passed, 31 deselected, 2.76s. Raw `source-binding.log`, SHA-256
  `899b33bba15638b38a2fe1cce38812de3523b48a1e9816499aabdfa52375d6a7`.
  Scoped Ruff check/format and whitespace checks passed. Final owner SHA-256:
  executor `f5fcd2e980f6fb9dc524b972c496577d320b956fc5669a9dda410502a61d4b9b`,
  portable caller `ab1f64fde89b70a62df05a935a27fd43e538e35b4d0329f0ccc3d22d6d1ce5e7`.
  They remained unchanged across these local proofs; moving dirty source and
  unverified release/operational inputs still exclude qualification.
- Original mandatory daemon handle terminated FAILED, not PENDING:
  `just rust-profile test-daemon`, exit 100; 204 executed, 203 passed,
  1 failed, 1 skipped, test phase 2689.597s. Failure:
  `e2e_top_k_truth_table::the_public_maximum_reports_the_continuation_over_ten_thousand_and_one_rows`,
  1225.431s, `over-maximum fixture ingest batch 2: ipc Read timed out after
  600000 ms`. Raw `/tmp/quanta-ss-rr-four.ZR5QK4/test-daemon.log`, SHA-256
  `640f91a3fc340033195572c83d885deaebf4ff135b800163d12c13c788b03b0a`.
  This old/moving-source run cannot qualify current HEAD. No unchanged retry.
  Live source confirms batches of two 910-row files, a 600-second IPC deadline
  and a 10001-row oracle. Failure class is UNKNOWN: daemon stage evidence is
  missing, and concurrent Rust jobs do not independently prove host load caused
  the timeout. Do not widen deadlines, reduce the corpus, skip the case or
  mark flaky without RCA.

## 2026-09-26 proof JSON admission closure

Review/repair started on shared dirty main
`2c08dccff4a9c1c23a3885dc74f10a5e9f4bd5f8`. Other writers continue changing
source and advancing HEAD. No commit, push, deployment or activation was
performed by this repair. Results below are local owner evidence, not final-tip
or repository qualification.

- RCA: plain `json.loads` erased duplicate authority keys before schema
  validation. Digest custody bound the ambiguous bytes but did not make their
  interpretation unambiguous. Python also accepted NaN/Infinity and numeric
  overflow such as `1e9999`.
- `tools/ci/proof_json.py` is the pure decoding owner. It rejects duplicate
  keys at every depth, non-finite constants, finite-syntax float overflow and
  excessive nesting with explicit `ValueError`. I/O, no-follow capture,
  archive hashes, schemas, source binding and domain outcomes remain with the
  existing callers. No new receipt IR or permissive compatibility reader.
- Migrated envelope/schema ingress: manifest writer; checker CLI, payload,
  dependency cache and aggregate checks; aggregate writer; handoff leaf and
  standalone lane reader; error-authority schema; archived pytest inventory;
  verification summary. Nextest event grammar remains separately domain-owned.
- Meaningful pre-patch RED: locked pytest `test_write_proof_manifest.py -q
  -k terminal_json --tb=short`, exit 1, 7 failed / 1 passed / 32 deselected.
  All seven invalid bytes were accepted by the actual existing terminal reader.
  The terminal tool output preserves the failures; no separate RED log was
  written. Unknown keys are still schema-owned, not silently rewritten.
- Locked decoder/sibling command: `uv run --locked --offline --extra dev
  python -m pytest tools/ci/tests/test_write_proof_manifest.py
  tools/ci/tests/test_check_proof_authority.py
  tools/ci/tests/test_check_lane_handoff.py
  tools/ci/tests/test_write_proof_aggregate.py
  tools/ci/tests/test_write_verification_receipt.py -q -k 'terminal_json or
  proof_json or duplicate_status_even or handoff_json_reader or
  aggregate_json_reader or summary_parser' --tb=short`: exit 0, 26 passed,
  173 deselected, 18.20s. Includes a correctly hashed immutable dependency
  archive whose duplicate `status` is refused by the real checker.
  `/tmp/quanta-proof-json.iZ6oUa/json-owners.log`, SHA-256
  `22cff833fbfc15a2daf7bbb42f382e1c6e044ef86ccd77400053b81fa756ffda`.
- RR found the first aggregate negative fixture could fail merely because its
  schema was missing. It now supplies a schema, observes a legitimate JSON
  positive reaching downstream validation, then requires the malformed bytes
  to fail before that downstream boundary. Validation is deliberately stubbed
  only for this decoder-boundary test; it is not aggregate qualification.
  Locked same-file `-k aggregate_json_reader --tb=short`: exit 0, 3 passed,
  18 deselected, 35.88s. Raw `aggregate-byte-admission.log` in the same temp root.
- Existing reverse consumers completed before this JSON repair:
  `python3 -m pytest tools/ci/tests/test_write_proof_manifest.py
  tools/ci/tests/test_write_proof_aggregate.py
  tools/ci/tests/test_handoff_validation.py
  tools/ci/tests/test_check_lane_handoff.py
  tools/ci/tests/test_write_verification_receipt.py -q`: exit 0, 101 passed,
  842.13s. Raw `/tmp/quanta-shell-word-closure.6C7IkC/reverse-consumers.log`,
  SHA-256 `a4bc075a826e4f8ed4f90f9ab69883ce215157db8992d3c83789545c027fd2b0`.
  This older-input result is not proof of the new JSON owner.
- Locked expanded owner command: `uv run --locked --offline --extra dev
  python -m pytest tools/ci/tests/test_check_proof_authority.py
  tools/ci/tests/test_proof_execution_result.py
  tools/ci/tests/test_write_verification_receipt.py
  tools/ci/tests/test_handoff_validation.py -q --tb=short`: exit 0, 163 passed,
  113.93s. Raw `checker-owners.log`, SHA-256
  `1a23692d95beda6b7d246b6d4cc7750a0812dba1264cb26247fb19a3c8277ea0`.
- Locked manifest integration: `uv run --locked --offline --extra dev python
  -m pytest tools/ci/tests/test_write_proof_manifest.py -q
  -k writer_resolves_registry_source_and_null_binary_then_semantically_validates
  --tb=short`: exit 0, 1 passed, 39 deselected, 26.57s. Uses real writer and
  semantic validation on an isolated fixture repository, not a production
  receipt. Raw `writer-publication.log`, SHA-256
  `af1ae68deccfbf629aaaf744194c8842a58204d2a600180320cd1a00a34f3cf3`.
- Shared main advanced to `9e443489e06167bfbed3b4503af558495e27206d` during
  proof. Rechecked owner SHA-256: `proof_json.py`
  `b847685c9d788cdc5363980efd2ea46a7a7c8ede4547759e228027feb2f043f1`;
  checker `f8c622f612d2af05b6f882998844ca4671910f7e53aa27bbc0d6984b6db7277d`;
  manifest writer `439225ec9428acf785069dc6c6c5545f95dc6cd2c2cdf6a3a9acbaac69f5d816`;
  handoff leaf `062a40a5e85468d581d694ed73e8db94c1f783a4a45005631938ac2b1128be37`.
  These content identities did not change during the scoped owner proof;
  broad shared-source drift still excludes final-tip qualification.
- Aggregate byte-admission raw SHA-256:
  `7dfbc8424a45623ca41a6d87e4fbc7f70227b8b92f64f5cc54de768c9fc1a884`.
  Ruff check/format and whitespace checks passed for this scoped repair.
- `just proof-authority-lint`: exit 0, REGISTRY_ONLY, 25 registered proofs,
  zero manifests validated; execution proof was not checked. Raw
  `authority-lint.log`, SHA-256
  `dbf353022f09e1b6a4c82e6878f7fa7a70e0e764e6f478c5b789046f3738bbb9`.
  This confirms the direct CLI and registry front door, not an issued proof.
- Open P2 lifecycle work: `producer_execution.execute` performs unbounded
  `communicate()` during failure cleanup; portable proof subprocess entrypoints
  lack their own execution deadline. Close the canonical execution/cleanup
  boundary and direct proof callers before claiming this finding repaired.
- The original mandatory daemon rail remains live on the same handle. R0
  producing-run attestation, P11 typed resolver/build receipts, release process
  proof and P12 operational inputs remain separate residual obligations.

## 2026-09-26 shared outcome owner, DAG reuse and Bash-word closure

Base `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e`, shared dirty main;
the index was empty at review. These are owner-local repairs, not a clean-source
qualification receipt. No commit, push, deployment or activation was performed.

During the reverse-consumer run another writer advanced main to
`e3c87234b0b3fad94df3080b65c7e0f4b086b8b1`. The shell, JUnit and DAG owner
content digests below remain the evidence boundary; moving shared HEAD and
other dirty inputs exclude exact-tip or whole-repository qualification.
Main subsequently moved again to `2c08dccff4a9c1c23a3885dc74f10a5e9f4bd5f8`
while reverse-consumer and daemon handles remained live. This ledger does not
promote either running rail to proof of that tip.

- JUnit RCA: retrieval and archive readers used different XML interpretations.
  Both now use `tools/ci/junit_events.py` for placement, unique identities,
  counted outcomes and exact counters. Required inventories still reject skips
  and selection discrepancies. Unknown wrappers and suite-level errors cannot
  become empty success. Source closure follows the new static local import.
- DAG RCA: recursive validation repeated intrinsic interpretation along every
  dependency path. `check_manifest` now owns invocation-local reuse bound to
  payload content and the complete authority object. Every incoming archive
  edge still captures no-follow bytes, verifies digest/identity/path/ancestry,
  and every cached/final binding is rehashed. The cache is not shared across
  top-level aggregate calls and is not a producer attestation.
- Bash RCA: command names and argv were interpreted through incompatible AST,
  shlex and whitespace-rewrite rules. `_literal_shell_argv` now supplies both
  executable and complete argv decisions. Bash escaped-newline removal,
  single/double quotes and embedded `#` are literal; unknown expansion/glob
  syntax and command prefix assignments fail closed. Full registered fuzz
  argv includes the actual dictionary and time limit. Python target admission
  also rejects help/version/collection-only/last-failed/attached filters rather
  than treating a nearby target filename as execution. Only full-file selectors
  and declared presentation options are admitted.
- Behavioral RED: six Bash reinterpretation/normal-line-continuation cases
  failed against the original owner (`red.log`); four use actual `bash -e`
  in private pytest fixtures and demonstrate success without the failing rail.
  Six assignment/whitespace siblings failed during construction
  (`sibling-red.log`); six Python nonexecution/partial-selection cases failed
  before the sibling repair (`python-selector-red.log`). A removed shlex import
  caused an intermediate 157-pass/3-error regression; it was restored for the
  separate static workflow-selector reader before the final run.
- Final shell owner: `python3 -m pytest
  tools/ci/tests/test_check_test_authority.py -q`: exit 0, **166 passed**, 39.47s.
  Raw `/tmp/quanta-shell-word-closure.6C7IkC/owner-final.log`, SHA-256
  `c2f5306789aefd10adee540455445256371ee34349b4ba60635c47d358d2594a`.
  Owner SHA-256 `5447b16ad824827f05987ba267ee4fbbeb7c9d031524c9271e11129e46afe61b`;
  test SHA-256 `b2c7f5edbf713e39ef186404189469a04a25d1b14954d83a1d64da240aa457b2`.
  Ruff check/format-check, scoped whitespace check and `just rust-test-authority`
  exited 0. Python was `/usr/bin/python3` 3.9.6; managed pytest9 coverage below
  is a separate environment, not interchangeable evidence.
  This system-Python environment has pytest6.2.5, tree-sitter-language-pack0.9.1,
  jsonschema4.25.1, PyYAML6.0.3 and tomli2.0.1; it is not the uv-lock admission
  environment. The final locked sibling run uses Python3.13.9/pytest9.1.1,
  uv.lock SHA-256 `c9064e8ead8593a6054d50c9b8e7523026d07d9ae7c95ddbf1268d273d6dad73`
  and pyproject SHA-256 `381ceb2908d49ff96cf718581404097cc621227008d1767b67ecb8c1c83ba135`.
  Managed dependency identities: tree-sitter-language-pack0.9.1,
  jsonschema4.25.1, PyYAML6.0.3 and tomli2.4.1.
- Prior expanded owner run: `python3 -m pytest
  tools/ci/tests/test_check_proof_authority.py
  tools/ci/tests/test_check_test_authority.py
  tools/ci/tests/test_retrieval_contract_proof.py
  tools/ci/tests/test_proof_execution_result.py -q`: exit 0, **295 passed**, 65.74s.
  Raw `/tmp/quanta-ss-rr-final.lYfN5u/owners-final.log`, SHA-256
  `4913529470dbe7ef386d4a0938655222e9994bb89e91f3af2b5de045ee54fb0b`.
  This run predates the final shell-word repair; use the 166-case result for
  that owner. It covers shared-JUnit hidden outcomes, diamond parse counts,
  live artifact/binary/archive/symlink tamper, changed authority, wrong source,
  cycles and no reuse across invocations.
- Final JUnit sibling pass also rejects unknown XML attributes, including
  testcase `status="notrun"` and root/suite `disabled`. Three meaningful RED
  inputs were accepted before repair, then refused through the same shared
  owner. `python3 -m pytest tools/ci/tests/test_retrieval_contract_proof.py
  tools/ci/tests/test_proof_execution_result.py -q`: exit 0, **69 passed**, 11.15s.
  Raw `/tmp/quanta-shell-word-closure.6C7IkC/junit-final.log`, SHA-256
  `b3ceaf17f042e2d35fb31701766bef8e21c13337760db77c68c011a337db8c8c`.
  Final JUnit owner SHA-256
  `5f7d6f5b5d285fb967b3bf702b130d3638907d71710deb330306780e6df087f6`;
  retrieval contract test SHA-256
  `59321413c2879d7da37ead1685d66b39633438c8ae3f94ffb48f7ef992b97552`.
  The 295-case run predates this attribute repair. The actual Sourcegraph27
  XML was rechecked against this final parser and the same independent method
  identity inventory; its counters still agree exactly. Native pytest
  xunit1/xunit2 attributes are admitted; arbitrary foreign dialects are not.
- Locked-environment retry history is not green proof: the first run failed
  1/235 because a concurrent Justfile repair moved output parameters to exported
  environment variables. The test now checks the current exact recipe and
  observes actual argv for both contract/sdk entrypoints with quote/command-
  substitution characters treated as data. A fixture run failed 2/237 because
  login zsh reset its observer PATH; the fixture now isolates ZDOTDIR and the
  working directory while keeping the native shell. A subsequent run failed
  1/237 because concurrent source-closure changed its dirty refusal message.
  The oracle still requires dirty-source SystemExit and no output creation;
  only the obsolete wording prefix was removed. Final rerun uses the same
  locked three-module rail after these relevant input changes. Earlier results
  must not be promoted to a final-source pass.
- Final locked sibling command: `uv run --locked --offline --extra dev python
  -m pytest tools/ci/tests/test_check_test_authority.py
  tools/ci/tests/test_retrieval_contract_proof.py
  tools/ci/tests/test_proof_execution_result.py -q --tb=short`:
  exit 0, **237 passed**, 15.91s on dirty main
  `2c08dccff4a9c1c23a3885dc74f10a5e9f4bd5f8`.
  Raw `/tmp/quanta-shell-word-closure.6C7IkC/locked-owners-final.log`, SHA-256
  `6d6e14cf4c954e8de29bef30c305d1cb85b53f53ad1ac4431ab0d4b15b85c5ad`.
  Final contract-test SHA-256
  `e7c14775b0c82503224226e4c821f7ddc59e15b9fbb65eb7fe3bd481a233652d`;
  proof parser `87fd1374db8a09d070c3f3ca4e5d1315dc6be63496f556963a456413b686440b`;
  concurrent source-closure owner
  `0e4d8a306c8a2fba6649ca814a55e51b9079deaf6da4fe6066eb214b4d6a0aea`;
  concurrent Justfile `a8b712a8ca8fba900dab246455bcb10339607059812b4c401cfaf6ac6bc45034`.
  These results replace the intermediate three-module failures only for the
  named local owner scope; they do not qualify unrelated dirty Rust changes,
  the real retrieval pair, the reverse-consumer run or daemon execution.
- Actual producer incompatibility: current uv.lock selects pytest9.1.1 on
  Python >=3.10. Its unittest subTest passes inflate `tests` without independent
  testcase identities. The observed retrieval XML advertised 315 tests but had
  283 testcase nodes; strict refusal remains correct. Sourcegraph's 16 fixed
  mutant inputs now have independent unittest methods rather than subTest
  reports; all assertions and the unittest CLI remain. No parser counter
  relaxation, post-hoc XML rewrite or framework downgrade was introduced.
- Producer owner proof: `uv run --locked --offline --extra dev python -m pytest
  tools/benchmark/retrieval/test_sourcegraph.py -q
  --junitxml=/tmp/quanta-shell-word-closure.6C7IkC/sourcegraph-junit.xml`:
  exit 0, **27 passed**, 1.04s, pytest9.1.1. The shared parser admitted those
  same 27 identities against an independent source-AST method inventory:
  selected/executed/passed=27, failed/ignored=0. Producer SHA-256
  `747633bccc46066f39174f9874315c450e1d41ba38a45bb1bf281348a8c61beb`;
  XML SHA-256 `bf0ebc1d8b8f5c20f95b6d5404893286127971945577377d8ea04e556d8c58c7`.
  The retrieval owner owns duplicate imported-class/subclass removal, exact
  `proof-required-tests.json` refresh and its full-file terminal proof. Those
  three files are handed off/frozen here; old retrieval XML is not reusable.
- Remaining rails: reverse manifest/aggregate/handoff/receipt consumers are
  running in `/tmp/quanta-shell-word-closure.6C7IkC/reverse-consumers.log`.
  The existing `just rust-profile test-daemon` handle was confirmed live and
  executing the selected runtime tests; no terminal result is claimed.
  Full exact-source/pair/Linux release and operational qualification remain
  separately unverified or blocked in `CURRENT-RESIDUAL-2026-09-26.md`.

## 2026-09-26 follow-up manifest and execution-byte custody

Base `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e`, shared dirty main.
Unrelated concurrent changes are preserved. No commit, deployment, push, real
target-root mutation or passed qualification manifest was issued by this pass.
Current implementation/actions: `CURRENT-RESIDUAL-2026-09-26.md`.

- Local focused behavior: `python3 -m pytest
  tools/ci/tests/test_proof_execution_result.py
  tools/ci/tests/test_paired_cargo_resolution.py -q`: exit 0, 65 passed,
  66.70s. Raw `/tmp/quanta-ss-rr-four.ZR5QK4/python-custody-final.log`, SHA-256
  `68062cd5f311722193fe481eebae7c16af1476455c7090ee43ad904d21d7c657`.
  Covers malformed/known Nextest exclusions, archive digest substitution,
  no-follow symlink refusal, captured-byte parsing for Nextest and JUnit,
  and real shell-script normal/alias-drift cases with fixture build/test tools.
  The fixture shell run is not a live Semantica/daemon product proof.
- Bound tested owner inputs: `nextest_events.py`
  `faf932aa2e83da828a82896aade69b68ec6af33877aea269935843f79dd5314c`;
  `proof_execution_result.py`
  `6ee40dcf4ac484498c284db62caf23f3bebbee82bec0ddf16aa14f847138fd14`;
  `binary_custody.py`
  `2e7e3d08069dd9ae942f2e1526840a66b67f1152b93a539674f72e55a95b2d92`;
  cross-repo shell wrapper
  `d5303d069cea632bdd641c30a63b0d498d3174d082b68a1075e2fb92ac0439f9`.
  Test files: execution
  `c82a86256df22a853a70284278e474b7e08d500ffae3bda27e59976dbbaf0231`,
  paired resolution
  `7ae96798d5de99e2e28c7a0fabecf7457755b9a5dece981a9c0503fe66e1089a`.
  Shared no-follow owner `handoff_validation.py`
  `36d3f68a3f3580f2ffaf8f0bcfce22735c750b184ad14a4bd8f237d09c6031bf`.
- Environment: `/usr/bin/python3` 3.9.6; Rust 1.92.0;
  Cargo.lock SHA-256
  `ff40c97922eb0c81ec1c46ca6bcc3e40e4b9d2d51ea497ec1a288cbb2b314837`;
  canonical environment script
  `26e171db52fa9d85d8065d57ff4424d19c766e8a4af46607b8c057513662ac26`.
- Scoped Ruff check/format, shell syntax and whitespace checks exited 0.
  `python3 tools/ci/lint/check-proof-authority.py` exited 0 with
  `REGISTRY_ONLY`: 25 registered proofs, zero manifests validated. This result
  establishes registry shape only, not execution or qualification.
- Initial Rust regression command failed before executing tests with three
  `unexpected cfg(feature="proof")` errors in `quanta-index-semantic`.
  The concurrent owner subsequently restored the feature declaration; the
  changed-input retry was `./scripts/cargow test -p
  quanta-index-searchd-runtime --test state_migration_owner_v1 -- --nocapture`.
  It exited 0: 55 passed, zero failed/ignored/filtered; test phase 57.50s,
  build/lock wait 14m21s. Raw
  `/tmp/quanta-ss-rr-four.ZR5QK4/state-owner.log`, SHA-256
  `46bae422b08548f6e1a8c124509fdcf3eed7321507623021a88690c5a0173943`.
  Bound migration owner
  `6ad3721d7b8dad45bc0920e8dc698acb4f9b19dee8c67ce0abb405e253f11d5b`,
  owner tests
  `7ff0694e6b5c0f370ef0a35ced6b4664636488039b090c3bdd77cf488dce4997`.
  No Rust behavioral RED is claimed from the initial compiler failure; the
  original defect has source-path evidence and these post-patch regressions.
- Expanded Python writer/checker/direct-consumer run exited 0, 222 passed,
  316.03s:
  execution result, verification receipt, proof authority, manifest writer,
  paired resolution, retrieval contract and SDK proof test modules.
  Raw `/tmp/quanta-ss-rr-four.ZR5QK4/python-full.log`, SHA-256
  `17e833fbddaddb6848ff0eb1b0a2160b22d8d8a48cc0135abf4ce47bc640c8de`.
  This is scoped local sibling regression, not whole-repository qualification.
  The execution test module received
  one additional JUnit custody case during this run; the separate 65-case
  owner run above includes that final case.
- Required residuals: required `just rust-profile test-daemon` terminal result,
  final bounded RR pass and exact-source
  owner issuance. Live pair, Linux release and operational inputs remain
  separate staged/blocked requirements in the authoritative residual table.

The daemon gate is running in the same `test-daemon-lane`, with raw
`/tmp/quanta-ss-rr-four.ZR5QK4/test-daemon.log`. Initial output admits 23 catalog
rows in three Cargo test binaries and one Cargo process. Selection/admission
is not execution; no daemon-gate pass is claimed until terminal results.

Descriptor-only imports now defer `jsonschema` to `validate_handoff` rather
than loading schema machinery in each binary guard subprocess. This preserves
one descriptor owner and one schema validator. The shared-leaf digest changed
  to `014e6901cbf172fc098e524b15d5bba9280f506ff0fc985c0d88d7b109e9bbb8`;
the earlier 65-case result predates this import-only change. The final leaf
command `python3 -m pytest tools/ci/tests/test_proof_execution_result.py
tools/ci/tests/test_paired_cargo_resolution.py
tools/ci/tests/test_handoff_validation.py
tools/ci/tests/test_check_lane_handoff.py -q` exited 0, 87 passed, 290.52s.
Raw `/tmp/quanta-ss-rr-four.ZR5QK4/python-custody-leaf-final.log`, SHA-256
`27b6cd136f11e0f4180c18b12380591c4100d49b3b65dc7cd181f2986832de8e`.
This is final-snapshot local coverage of the new reader/pinning paths plus
the schema validator and handoff CLI, not an execution-time or repository-wide
performance qualification. No speedup factor is claimed from different scopes
or concurrently loaded host timings.

`python3 tools/ci/lint/check-test-authority.py` also exited 0 against the
current dirty checkout. The new cases live in existing admitted target modules;
there is no additional test target or duplicate test registry.

## 2026-09-26 parallel residual structural repairs

Current status/actions: [residual audit](CURRENT-RESIDUAL-2026-09-26.md).
Review began on `7cefac4a10a06ed56b6f5b9f42b3726468b1f198`; concurrent
work advanced main to `8eac12c5b45fedfa4aa7cb27da82979ecdbfbb10`. Named
repairs remain a local dirty overlay; unrelated changes were preserved.
At closeout, concurrent commit `4af3bb44ea4769205485a9ed4c7dddddb35a724f`
captured these owner/proof repairs and ticket updates together with unrelated
work; this progress entry remains uncommitted. That mixed ownership commit
does not retroactively qualify the earlier moving-overlay results.
Host: Darwin arm64, Python 3.9.6, Cargo 1.92.0. Cargo.lock SHA-256:
`ff40c97922eb0c81ec1c46ca6bcc3e40e4b9d2d51ea497ec1a288cbb2b314837`.

- Local focused `VERIFIED` behavior only:
  `python3 -m pytest tools/ci/tests/test_proof_execution_result.py tools/ci/tests/test_paired_cargo_resolution.py -q`:
  40 passed, 3.98s. Raw `/tmp/quanta-residual-audit.r8ZupY/negative-owners.log`,
  SHA-256 `c4915b12510145f1415ff02d5d40e190a5a189df1ddc4e9b2c847384cfd7fa7c`.
- Local focused `VERIFIED` supervisor behavior:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_supervisor_owner_v1`:
  21 passed, zero failed/ignored/filtered; build 1m52s, cases 0.52s. Raw
  `supervisor-owner.log` in that directory, SHA-256
  `2380a72905a3dea40689d9ee969350b125c78fcc67ae253a0b30f40ea90a4c82`.
  Bound owner bytes: supervisor SHA-256
  `6d05e85abc6aa7e188c32be8d3978eb0e4484d483793fb3e3858876809892b7e`,
  owner test `2919ef656f090ff95a98cb70a6921e21ec85cb5c6d819d4558bc75bd48c42c58`.
  This is not an actual release-daemon maintenance-loss scenario.
- Local focused `VERIFIED` exact-pair writer regression after fixture repair:
  `python3 -m pytest tools/ci/tests/test_write_proof_manifest.py::test_exact_pair_manifest_is_live_bound_through_atomic_writer -q`:
  1 passed, 5.16s; `exact-pair-regression.log` SHA-256
  `5e8305ab5fe4d0b9626b3fdd9bc14ec7c91249c3c5bcadda9ed717b6ea50aaed`.
  Earlier writer/checker run was `FAILED` (101 passed, one obsolete staged-reason
  fixture failure). The fixture now matches the typed stanza independently of
  human reason text; the focused rerun does not mean the whole scope was rerun.
- Static local checks: scoped Ruff check/format and whitespace/shell syntax
  exit 0; `python3 tools/ci/lint/check-test-authority.py` exit 0 after both
  P00 inventory/execution and P12A recipe were connected to the new target.
  `test-authority.log` SHA-256
  `e04c60a38c36c65b6b659fc8ba2ad8360ec757a515b9104d4bf76e4f26daca02`.
  Proof registry lint reports REGISTRY_ONLY, 25 proofs, zero validated manifests;
  execution proof is explicitly not checked.
- `FAILED` Clippy command:
  `./scripts/cargow clippy -p quanta-index-searchd-runtime --test runtime_supervisor_owner_v1 -- -D warnings`
  stopped in another writer's `ipc/ingest_observation.rs` at collapsible_if,
  before qualifying the supervisor. Raw `clippy-supervisor.log` SHA-256
  `7728e31122f87ffc3523eb04f4bfe977d44ff0183de290d34eeaff8c5685a4d0`.
  Earlier compilation also hit a concurrently changed stream return-type
  seam; its owner corrected it before the successful owner run. Neither
  unrelated file was edited here. Supervisor Clippy qualification: NOT_RUN.
- Complete aggregate suite: NOT_RUN to completion. An integration run during
  registry edits failed on inconsistent P12A target snapshots and was interrupted.
  A restarted final-target aggregate run passed its first six tests, then was
  interrupted after 260.96s; never count that as aggregate PASS. Read-only
  graph audit confirmed shared-ancestor revalidation cost; safe cache acceptance
  is recorded as OPEN_PERFORMANCE in the residual table.
- Full clean-source gate, actual live pair, Linux release, real provider,
  authorized retained-root cutover, deployment/activation/rollback: NOT_RUN or
  BLOCKED on the specific missing inputs listed in the residual table. No
  qualification manifest, release verdict, remote push or operational action.

Raw logs are temporary local evidence, not durable release receipts. Referenced
source/config changes invalidate their applicability; concurrent dependency
edits and the moving dirty source exclude repository qualification.

This file indexes live evidence. The former dated execution ledger is available
in Git history; its dirty-checkout observations and test counts are not current
qualification.

2026-09-24 static audit at Quanta `28c20fabfdc9d57b0d7d94794d59bcf78ea7cd14`
found proof-result/host trust, resolved cross-repo Cargo dependency-root,
P09 backend-health/diagnostic, and P11 operational-action authority gaps.
The [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) and
[action list](ACTION-LIST.md) now sequence the repairs. This document update
issued no proof manifest and did not run Rust or release qualification.

## Source of truth

- [proof-authority.toml](../../../../tools/ci/proof-authority.toml) declares
  proof IDs, dependencies, execution state, host, and artifact paths.
- [check-proof-authority.py](../../../../tools/ci/lint/check-proof-authority.py)
  validates the registry, individual receipts, and the final aggregate.
- [Justfile](../../../../Justfile) owns the commands. PR CI issues and checks a
  fresh P00 receipt; the full release gate runs only with an explicit proof
  bundle and paired repository revision.
- [state-cutover-runbook.md](../../../operator/state-cutover-runbook.md) describes
  the supported current-format backup/restore/verify workflow. Legacy
  `migrate-state` is retired.

## Read the current result

From a frozen checkout, record `git rev-parse HEAD`,
`git status --porcelain=v1`, branch/upstream, and the paired Semantica
revision before using any receipt. Run:

```sh
just proof-authority-lint
python3 tools/ci/lint/check-proof-authority.py --require-all --bind-source \
  --paired-checkout "github:josongmin/semantica-codegraph-v2=$SEMANTICA_CHECKOUT"
```

The first command is registry-only; zero manifests validated is not execution
proof. The second is the final release check and requires every current-source
manifest and the P12 aggregate. An old `passed` receipt remains historical
evidence, even when its archive is intact. A staged node is blocked until its
real authority is implemented and registered; adding a JSON file cannot
promote it.

Classify owner tests, exact-pair proof, Linux process proof, deployment,
activation, and rollback separately. A missing or stale receipt is not a failed
test. The validator's raw findings, exact command, source pair, and artifact
digests are the report; do not copy a dated count from this document into a
new closure claim.

## 2026-09-26 local quarantine-recovery check

- `FAILED` before repair: `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` reopened a catalog with a sealed-only candidate quarantine and returned `CATALOG_ROW_CORRUPT`: event 7 had no activation-invalidation domain pair.
- RCA: `quarantine_repomap_candidate` appended `RepoMapInvalidation`, while replay paired that kind only to an inactive activation row. The sealed candidate has no activation row. Commit `b4e21b50` separates `RepoMapCandidateQuarantine` and stores its exact sequence on the candidate row without overwriting the seal sequence; the new event is checked for a candidate pair at reopen. Prior event-kind schemas are refused, not silently migrated.
- `VERIFIED` narrowly: the focused E2E above passed 1/1 after repair. Catalog unit coverage passed the sealed-candidate quarantine/replay/reopen path and rejected an orphan candidate-quarantine event. The later `just verify` run passed workspace Rust tests (including runtime extended 64/64 and risk 137/137), rustdoc, policy checks and Semgrep, but is **not** a final-source qualification: `main` moved from `8083abc2` to `b4e21b50` and then `1ef1ec60` during the run.
- `FAILED` final command: `just verify` exited 1 at `python-format-check` because concurrent retrieval work left `tools/benchmark/retrieval/query_plan.py`, `tools/ci/tests/test_retrieval_benchmark.py`, and `tools/ci/tests/test_write_verification_receipt.py` unformatted. This was a moving, dirty shared checkout, not a clean-HEAD receipt. Those files were not modified by this quarantine repair.
- At `1ef1ec60236bb53176d72578830a7241547acb81`, `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` and the focused daemon E2E above passed. The predecessor-schema refusal fixture was corrected from the older 1..=9 set to the immediate predecessor 1..=10 set; catalog tests were rerun and passed again (4 unit, 6 auxiliary, 18 idempotency, 18 operation-journal). Retrieval files and this progress note were dirty, so these remain narrow local checks, not clean-source qualification.
- `NOT_RUN`: clean, frozen-HEAD full verification, release proof, deployment, activation and rollback. Rerun `just verify` only after the concurrent writer freezes and formats its own files; bind the resulting receipt to that exact clean source.

## 2026-09-26 session hardening of candidate-event integrity

- Scope: local `main` at `bf51f55f` plus this task's catalog edits. Concurrent dirty retrieval-benchmark files are outside this repair and were preserved.
- Confirmed `FAILED` before repair: a correctly self-digested `RepoMapCandidateQuarantine` event with the wrong logical identity passed catalog reopen when its sequence matched a quarantined candidate. A quarantine sequence earlier than its seal sequence also passed the candidate table. An installed candidate table lacking the new ordering constraint was accepted by `CREATE IF NOT EXISTS`. These three cases were observed RED in owner-local tests against real SQLite. The same sequence-only lookup also omitted commitment binding; the final test exercises both substitutions.
- Repair: the catalog now verifies both candidate event kinds through the canonical candidate row decoder, binding sequence, row digest, logical identity and commitment; the table enforces quarantine-after-seal ordering; open refuses an incompatible installed candidate schema before allocator seed. These checks add no second serving authority or consumer fallback.
- Narrow local proof on that dirty source: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 8 unit, 6 auxiliary, 18 idempotency and 18 journal tests; package all-target Clippy passed; `just rust-test-candidate-activation-owner` passed 30/30 with 0 skipped; the focused `e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot` daemon process test passed 1/1. The three observed RED cases pass after repair; the added seal-identity and commitment variants also pass.
- `NOT_RUN`: clean-HEAD full `just verify` and release/operational proof for this new source. These narrow checks do not supersede the earlier failed full gate or qualify the concurrent retrieval changes.

## 2026-09-26 session hardening of quarantine discard and replay

- Scope: catalog quarantine record/discard authority, RepoMap payload projection and recovery, and the daemon quarantine route. The review started at `8d9b9f37`; unrelated retrieval work advanced `main` to `c6f0405a` and left other files dirty while these changes were made. The commands below therefore prove only the named owner/boundary behavior on a local overlay, not a clean-HEAD repository qualification.
- Confirmed `FAILED` before repair: a self-digested tombstone with `discarded=1, discard_sequence=NULL` returned the record sequence as a successful discard retry. The catalog now requires the discard state/sequence relation in the SQLite schema and canonical row decoder, validates the complete existing row before mutation/replay, and refuses a predecessor quarantine schema before allocator seed. The owner test first failed on the old fallback, then passed against the repaired owner.
- Confirmed `FAILED` before repair: a catalog tombstone committed before payload unlink left a file that a retry called `Absent`; two incidents with the same content-addressed payload lost the still-live projection when the first was discarded. Real-file owner tests failed on both cases. The RepoMap store now finishes pending reclaim on retry and boot, consults catalog live references before unlink, and serializes a store's discard/reclaim boundary. A repeated already-tombstoned incident with another live reference returns `Absent`, not a fabricated new discard.
- Confirmed `FAILED` before repair: a correctly self-digested `QuarantineRecord` ledger event with an incorrect incident identity reopened because integrity pairing checked only sequence. The catalog now pairs both quarantine event kinds through the canonical self-digested incident decoder and checks event identity and payload digest. The owner test covers wrong record identity and wrong discard payload.
- Narrow local proof after these repairs: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 12 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with 0 skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` passed 1/1. These are a local dirty-overlay proof of the named paths, not clean-HEAD qualification. Relevant code/test SHA-256s at closeout: `candidate.rs=a1ae23a5f549e3c2a21f5ceff9fcea225ffd03d3c204d04a436d661fd624341f`, `sequence.rs=d7c91a93e49013ac2846f8863b6e59501848fd9b6165bba4917216ba6bd09dc8`, `store.rs=2f426c392d19e54a21f651e80b056852715224e63a7d98654279bf964c061034`, `candidate_activation_owner_v1.rs=d55988e98527e0bb2f56dc72a1db84fffa20fc8635dbaab3c3ccdbd5e6abd69e`. Any relevant source change invalidates this receipt.
- Breaking persistence boundary: this build refuses existing quarantine tables lacking the new checks; there is no silent in-place migration. Before activation on any retained state root, inventory exact schema/data, back up, and select a supported rebuild/migration path. No production root was inspected or mutated by this task.
- `NOT_RUN`: frozen clean-HEAD full `just verify`, release proof, deployment, activation, rollback and production-state migration. A focused owner or daemon pass does not close those claims.

## 2026-09-26 activation-event integrity continuation

- Scope: this catalog work began from `main` at `af640562`; unrelated retrieval work advanced it to `619292ca` without touching these owner files. Concurrent retrieval/benchmark and planning-document changes are outside this ownership. The goal remains broader than this catalog slice.
- Confirmed `FAILED` before repair: `invalidate_repomap_activation(repo, revision, generation=2)` accepted a sealed generation 2 while generation 1 was the active head, appended an invalidation event, deactivated generation 1's row and marked generation 2 invalidated. The owner test observed the wrong success. The catalog now checks the requested generation against the active head and verifies its candidate state/commitment before any event allocation or row mutation; a stale target is a typed CAS refusal with zero allocator delta.
- Confirmed `FAILED` before repair: self-digested `Activation` and `RepoMapInvalidation` ledger events with the wrong logical identity or commitment passed catalog reopen because domain pairing checked only sequence. The owner test constructs real ledger and activation/candidate rows for both event kinds and both substitutions. Reopen now uses one canonical activation-row decoder and checks event identity/commitment plus the candidate's state/commitment.
- Confirmed `FAILED` before repair: substituting the original `activation_sequence` in a structurally valid inactive activation row left its previous `row_sha256` accepted, because the digest omitted that field. The activation row preimage now includes both activation and terminal sequences under a new digest domain. SQLite and the canonical decoder enforce active/inactive reason and sequence ordering, and open refuses an installed activation schema without those checks before allocator seed.
- Focused local proof on the final dirty overlay: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 16 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_boot_quarantine::quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot --all-features --locked -- --exact` passed 1/1. Relevant source SHA-256s: `candidate.rs=7b8b9bf056a3a51c7af06326ca946fa0eba6a957b24d6270785095de637f4f4e`, `sequence.rs=f99eaf124ef724e6481a38b05e3e38f62db8a8bd5cc6bcad1772b3d557625386`. These are narrow owner/process checks, not clean-HEAD qualification.
- Breaking persistence boundary: an existing activation table lacking the new constraint is refused, and old activation-row digests do not match the new domain. No automatic migration or deployed-root conversion was implemented or claimed. Inventory and back up retained state before selecting a rebuild/migration path.
- `NOT_RUN`: clean-HEAD full `just verify`, cross-repo/release proof, production-state migration, deployment, activation and rollback. The focused passes above do not close the overall P0–P2 goal.

## 2026-09-26 active-head cardinality continuation

- Base: `main` at `9af01c75f39bd6517f66ae526df34d54db15c361`. During shared-main integration, the concurrent `8b612c8d` commit captured the catalog code alongside retrieval/benchmark work; the progress note landed in `d67daae5`. The catalog source hashes below match the committed tip, but the two commits do not have independent ownership boundaries.
- Confirmed `FAILED` before repair: two active epochs for the same repo/revision, each with a self-digested activation row, candidate row and ledger event, reopened successfully. A variant with the older epoch active and the latest epoch invalidated also has individually valid event/domain pairs but violates the one-head authority invariant. The hostile owner test first failed on the dual-active fixture.
- Repair: the catalog's existing integrity pass now rejects multiple active rows or an active nonlatest epoch per repo/revision at reopen. Targeted activation reads also reject an active row in any other epoch, so a post-open direct SQLite mutation cannot make a query silently select only the latest row. The normal writer remains serialized by the catalog transaction; no second active-head authority or separate projection was introduced. No new schema/index migration was added in this slice.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 17 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33, zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; focused reboot quarantine process test passed 1/1 (136 filtered). Source SHA-256s: `candidate.rs=bdca42c2dd86eaa4288081afdd7bc61ed12bdbf5aa427278c1187fa7299ab351`, `sequence.rs=0c31fe0c28066c6abd41507931ed437a3b96d935236a3d7ba536f04d0e769dcd`.
- Post-integration rerun on `d67daae5`: `just rust-test-candidate-activation-owner` passed 33/33, zero skipped, and the full catalog package command above passed 17+6+18+18 tests. This is a committed-tip local source check; it is not the full frozen qualification gate.
- `NOT_RUN`: clean-HEAD full `just verify`, operational migration of retained roots, release/deployment/activation/rollback. This local proof is not broad P0–P2 closure.

## 2026-09-26 ledger/domain bidirectional integrity continuation

- Scope: catalog sequence and RepoMap domain authority, starting from clean `main` at `da3e5d91`. During shared-main work, `f1d041ff` captured the candidate repair and an earlier sequence version alongside unrelated retrieval edits; the remaining sequence consolidation and this note are separate. Retrieval files are outside this ownership.
- Confirmed `FAILED` before repair: deleting a middle `Activation` event while a later event remained left the allocator maximum unchanged and allowed a self-digested active activation row to reopen. An activation row borrowing a `CandidateSeal` sequence also reopened; candidate and quarantine rows borrowing a `Rollback` sequence did likewise. Owner tests observed each wrong success against real SQLite before the corresponding repair.
- Repair: the append-only event scan now requires sequences 1..N without gaps. RepoMap activation, candidate and quarantine rows have indexed reverse kind/sequence checks; the existing forward event scan remains the one place that checks row digest, identity, payload and state. Direct activation reads additionally verify the exact self-digested activation/invalidation event references and candidate state. The forward and targeted-reference paths share one canonical event decoder instead of duplicating hash rules. A separate no-domain `Rollback` deletion fixture proves the gap check independently of reverse pairing.
- Narrow local proof on the final source overlay: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 22 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33 with zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; focused reboot quarantine process test passed 1/1 (136 filtered) before the final decoder-only/test change. Relevant source SHA-256s: `candidate.rs=3aa2ab0750e9b70e9541fc2e6f4fdc69825c8265513d0e826cad5786326da7f0`, `sequence.rs=23760a87787e52208f5516ab99b5281b26129157ad7ac1fef8a4318d18468e97`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- `NOT_RUN`: clean-HEAD full `just verify`, release proof, retained-root restore/migration, deployment, activation and rollback. The owner and focused process checks do not close the broader P0–P2 goal.

## 2026-09-26 operation-journal event-custody continuation

- Scope: `main` at `f9f4ec44175cdb6a94b9e3bd1f8e8a21b9e3be8f` plus this catalog-only overlay. Concurrent Sep-23/Sep-26 retrieval edits are outside this ownership and were preserved.
- Confirmed `FAILED` before repair: after a real committed operation, replacing only its ledger event with a correctly self-digested `Rollback` event let the terminal journal row and catalog reopen successfully. The existing forward pass skipped `Rollback`, and no journal-row-to-ledger check existed. The owner test first observed the wrong success. The repaired test also exercises separately self-digested wrong identity and payload, both after an in-process replay attempt and after reopen.
- Repair: terminal journal rows now require an exact current event kind/sequence at reopen; the canonical journal reader verifies its own row digest, receipt-bytes digest, expected event kind, key identity and state-specific payload against the self-digested ledger event before returning a replay. The ledger forward pass uses that reader for surviving terminal rows and retains explicit invalidation attribution for legitimately removed historical rows. Prepared/claimed/applying recovery and generation-GC semantics remain distinct; no second journal authority was introduced.
- Existing corruption tests now expect refusal at the earlier catalog-open boundary. The former one-byte foreign-version fixture was not a coherent persisted future-format receipt because its receipt and event digests were left unchanged; it is now named as a tamper test. A separate pure decoder test checks typed foreign-tag refusal. A fully re-digested foreign-version persisted fixture remains `NOT_RUN` and must not be inferred from these two tests.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 24 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36, zero skipped; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Source SHA-256s: `idempotency.rs=77bf300d3b8b987bbf2c38cf08de07d3e9fa69da6d9baeb8e0953df4186a8ed1`, `sequence.rs=27545c76a6b6301db1fa0803d5adabe9f77b1e4c37d681828bc10b2b7f5d07c7`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- `NOT_RUN`: clean-HEAD full `just verify`, fully coherent foreign-version persisted fixture, release proof, retained-root restore/migration, deployment, activation and rollback. This owner/process check is not overall P0–P2 closure.

## 2026-09-26 coherent foreign-receipt replay proof

- Scope: catalog-only overlay on `main` at `8308310f4a5f677c63f7b5053bfa688862cd281a`. Concurrent Sep-23/Sep-26 retrieval edits and benchmark code are outside this ownership and were not included in these checks.
- The preceding section's fully coherent foreign-version persisted fixture was `NOT_RUN` at that time. It is now `VERIFIED` for the narrow catalog replay path: a test commits a real operation, advances the receipt format tag, recomputes the receipt, journal-row and ledger-event digests with canonical algorithms, and persists the coherent foreign receipt. Catalog reopen succeeds, then replay rejects the unsupported version as `CatalogRowCorrupt` without advancing the sequence allocator. This distinguishes version refusal from incidental digest-corruption refusal. The fixture does not prove behavior of an actual future-format writer.
- Verification on the final dirty overlay: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 25 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36, zero skipped (Nextest run `06379b84-9a8a-4c87-acab-040d6deb9b08`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed. Source SHA-256s: `idempotency.rs=534560248e3e32b211e935e846749e4a65d7eb8e40b26add7f509a1a38e86c4b`, `sequence.rs=fc5aed3a9b648a21c6a6a6abe04a9ec0841b5343aa92d9a6d56619acf0701ef0`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`. Any relevant source change invalidates this narrow receipt.
- `NOT_RUN`: clean-HEAD full `just verify`, release proof, retained-root restore/migration, deployment, activation and rollback. This fixture closes only the named proof gap, not the broader P0–P2 goal.

## 2026-09-26 operation-invalidation lineage and takeover continuation

- Scope: shared `main` moved from `3742b39e` to `e7fd1f8491812c6eb32e6d267638954a5e271ca6` during this work. The concurrent `e7fd1f84` commit included the first catalog sequence test and chronology fix alongside retrieval edits; the remaining catalog changes in this section are separately owned. Retrieval benchmark edits remain outside this scope.
- Confirmed `FAILED` before repair: after a real commit and generation GC, an internally valid new operation event with the same identity but no journal row reopened because an earlier invalidation was accepted as attribution. A second fixture inserted the orphan before GC; one later invalidation still excused two terminal events of the same identity. The integrity pass now accepts a missing terminal row only when the nearest later invalidation has no intervening same-identity terminal event, and never excuses a present row with the wrong terminal state. An `(kind, identity_digest, sequence)` index bounds the attribution lookup on retained ledgers.
- Confirmed `FAILED` before repair: an expired claim overwritten by same-owner `claim_prepared` or `prepare` left an orphan `OperationAborted` event and caused catalog reopen to refuse. The two writer paths now append a supersession invalidation in the same transaction before overwriting the aborted row; all four supersession call sites use one helper. A foreign owner was also wrongly rejected after lease expiry as `CatalogBusy`; only an unexpired lease now blocks takeover, while the fresh fence still replaces the stale claim. The test covers same/foreign owner crossed with prepare/claim and checks the two allocated events plus reopen.
- Narrow dirty-overlay proof after these changes: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 27 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36 with zero skipped (Nextest run `1ac8b826-48f8-4558-a621-2be846965458`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; focused `rustfmt --check` and `git diff --check` passed. Relevant SHA-256s: `idempotency.rs=3e0a7fa7632d1cf767795133351aa8405aef6326c6a64ce4b3a7a6f7084a5d5a`, `sequence.rs=78289515c352ecb38cc6d5a7d11c8b25ddbedce34c0ee268fbe3166e5fefa53d`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`. These checks cover the named paths, not clean-source qualification.
- Residual design boundary: an invalidation still identifies historical terminal rows through identity/order, not a target event commitment. A fully self-redigested replacement of an already removed historical terminal event cannot be independently compared with its former payload. Determine whether the retained-root threat model requires a breaking, target-bound invalidation format before claiming tamper resistance; do not infer that property from the above tests.
- `NOT_RUN`: clean-HEAD full `just verify`, retained-root migration/restore, release, deployment, activation and rollback. Overall P0–P2 closure remains open.

## 2026-09-26 target-bound operation invalidation and durable GC floor

- Scope: catalog-owned overlay on `main` at `86fbbca2ca9c15ca0cae5a813e7bca93517bbd0f`; concurrent retrieval-benchmark sources and proof manifests are outside this ownership. The previous section's identity/order-only invalidation boundary is superseded for newly written roots, not proof of migration for retained roots.
- Confirmed `FAILED` before repair: after a real committed operation and generation GC removed its journal row, replacing only the historical terminal event's payload, commitment and row digest with internally consistent values still allowed catalog reopen. The invalidation carried no target commitment. A second self-digested fixture recast the GC event as retry supersession, which removed the replay floor without changing its target payload. Both fixtures first observed the wrong success.
- Repair: generic event kind 7 remains retry supersession; new kind 12 is generation-GC invalidation. Both carry the immediately preceding terminal event commitment, or an explicit no-terminal marker. The operation writer and integrity verifier use the same ledger predecessor rule; missing terminal rows require the exact later target and one-to-one ordering. GC alone also writes a self-digested `operation_gc_floor_v1` tombstone in the same transaction. The floor row and kind-12 event are checked in both directions before crash recovery and during replay-floor lookup, so deleting the row or recasting the event refuses instead of making a retry fresh. This tombstone is the retained domain record, not a second event counter or compatibility reader.
- Breaking persistence boundary: the current event table requires kind `1..=12`; the immediate predecessor `1..=11` schema and an incomplete GC-floor schema refuse open before allocator seed/recovery. No automatic migration, import, retained-root conversion or deployment is claimed. Inventory and back up retained roots before any offline conversion or rebuild decision.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 31 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36 with zero skipped (Nextest run `0d4a11d4-9058-41c2-97d2-cc68d55da234`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Focused `rustfmt --check` and `git diff --check` passed. Relevant SHA-256s: `candidate.rs=2fbb79f4425358c4361f8d5336e28e8446612c8b5fae7565395e86c313085164`, `idempotency.rs=1f00a1f75ee9f415911ca8e41a6889702f81690d0be72df0fd27c57790b37c9c`, `open.rs=af06c9253baf610951262af313c4665997d8450973a924584a4d6e0913d154a5`, `sequence.rs=5bc4acd3906122d8f79de7efede219cc55e33335f86300134c322e3995ecba97`, `operation_journal.rs=43691f079f4bcf57000f49a65d74c322cbc07dc26df28a170641022dafb1c76d`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- Further audit lead, not yet a qualified fix: `idempotency.rs::fence_token_now` truncates wall-clock nanoseconds to 32 low bits before adding a fixed high bit. A stale same-owner handle could collide with a later fence after the 2^32-ns period; evaluate with a deterministic collision fixture and replace the allocator if reachable. Coordinated rewriting of both the event and GC floor remains outside self-digest tamper detection without an external trust anchor.
- `NOT_RUN`: clean-HEAD full `just verify`, retained-root migration/restore, release qualification, deployment, activation and rollback. The focused checks do not close overall P0–P2 work.

## 2026-09-26 durable root-global fence allocator continuation

- Scope: catalog-only overlay on `main` at `56280dcd8de52b71422ed6288dafa7b1b211f147`. Concurrent retrieval-benchmark work remains outside this ownership. The prior section's low-32-bit wall-clock fence audit lead is closed by this source change, not by probabilistic collision testing.
- Confirmed source defect: `fence_token_now` used `(unix_nanos & u32::MAX) | (1 << 32)`. Its token mapping repeats at offsets of `2^32` nanoseconds (~4.29 seconds), so a later same-owner claim or lease could receive the same fence as a stale handle. Owner+fence equality alone could then accept the stale handle. Time is retained only for sampled lease-deadline decisions, not fence identity.
- Repair: one self-digested `catalog_fence_v1` allocator owns positive root-global fence tokens for idempotency claims/prepares and mutation-coordinator leases. Allocation is monotonic in each owner's existing `BEGIN IMMEDIATE` transaction; rollback does not advance it, release and crash recovery do not erase it, and `i64::MAX` exhausts explicitly before operation mutation. Open verifies its schema, digest and retained journal/lease high-water before crash recovery. A pre-existing catalog without the allocator refuses rather than silently seeding a reusable fence; no clock-derived fallback or second allocator remains. Mutation lease reads now verify the stored row digest before takeover, release, or startup cleanup; previously the digest was written but ignored.
- Negative oracles: same/foreign-owner expired-claim takeover through prepare and claim advances the fence and makes the old handle lose it; mutation leases advance across release and reopen and an old handle cannot release a later same-owner lease; a damaged lease deadline refuses takeover, release, and startup cleanup without erasing the row; missing or coherently regressed allocator refuses before crash-recovery abort; exhausted allocator inserts no operation row. The current-table inventory test includes the fence allocator and GC-floor domain.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 35 unit, 6 auxiliary, 18 idempotency and 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36 with zero skipped (Nextest run `89d6ec5f-7c20-4cef-b2b0-a4a6f6d94037`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Focused `rustfmt --check` and `git diff --check` passed. Source SHA-256s: `idempotency.rs=66c5342e0c416e664ee7d44ade1deef390b1436e3c594ffa5bae1412e10f6b26`, `open.rs=5c054bd1004d8c32de45274b685c5817aab87575ef5b1f122afd6e4e61a85eb9`, `sequence.rs=518447fce5634cd386cf5f77cc16a03819d2cee31c3c727128f381b21569ea9f`, `operation_journal.rs=f7827f8e74a74edfb1b3b55449819ff59df00ab529d9d55caa144302cb8f37ea`, `Cargo.lock=d0432493233d23f01258c8f15334daeed24135d6ef6ea2a232f1b7d0cb854401`.
- Breaking persistence boundary: retained roots without `catalog_fence_v1` require an inventory-backed offline migration or rebuild decision. The implementation does not claim recovery of fence history erased by a restored older snapshot; external root/daemon fencing remains a separate operational proof.
- `NOT_RUN`: clean-HEAD full `just verify`, retained-root migration/restore, release qualification, deployment, activation and rollback. The broader P0–P2 objective remains open.

## 2026-09-26 atomic catalog initialization continuation

- Scope: catalog-only overlay on committed `main` at `c32c19c20e3c5a633a08dfbd57413ac99f8b9029`; concurrent retrieval changes remain outside ownership. The fence allocation repair above was committed at that revision, and `just rust-test-p02b-operation-journal` passed on the commit itself (36/36, zero skipped, Nextest run `0355ca56-a4f0-4642-bb28-a0042ad907e8`).
- Confirmed availability defect in the new initialization boundary: fresh-root DDL and fence-row seed previously executed as separate autocommit statements. An interrupted first open could leave a partial current schema but no allocator row; the next open would correctly classify it as an existing root and refuse implicit re-seeding. This is not an acceptable normal startup crash outcome.
- Repair: schema creation, installed-schema validation, both allocator seeds, and GC-floor validation now share one `BEGIN IMMEDIATE` transaction. Any failure rolls back the entire initialization, including newly created tables. Crash recovery remains after that transaction and keeps its existing independent write transactions. Existing roots lacking a durable fence allocator still refuse; this change does not silently migrate them.
- Negative oracle: missing-allocator reopen now asserts that the failed open did not re-create its table. Five older incompatible-schema tests had asserted an empty allocator table left by failed open; the atomic boundary correctly leaves no such table, so their assertions now check that no allocator table was created. Their typed/storage refusal assertions remain intact. The first full package run on the new source failed those five obsolete fixture expectations; after updating them, the full package passed.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 35 unit, 6 auxiliary, 18 idempotency, 18 operation-journal tests; `just rust-test-p02b-operation-journal` passed 36/36 with zero skipped (Nextest run `36ea574f-dbb5-4eb7-87df-9741cdf80527`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Focused `rustfmt --check` and `git diff --check` passed. Source SHA-256s: `open.rs=176e14e1495cf6250486114aadb0f581be60a97d1da39a07ac6430859f2a13a0`, `idempotency.rs=3ca96ece3b6753a3570e7a01e6758e401f7216eba852c97f019488a539646a95`, `candidate.rs=687b404a5e497286e318edc98defe7af857661060c106de90b936b40573f26ad`, `sequence.rs=4f45fedfa5e98d58962ab5981cc7822be1f4f6a9601ec43500e5226261053503`.
- `NOT_RUN`: process-kill fault injection between individual SQLite DDL statements, clean-HEAD full `just verify`, retained-root inventory/migration/restore, release, deployment, activation, and rollback. Focused proof is not overall P0–P2 closure.

## 2026-09-26 atomic startup recovery and sequence reconciliation continuation

- Scope: catalog-only overlay on committed `main` at `3f832def394845df55d211ba19ce9451c95009c2`; concurrent retrieval/benchmark edits remain outside ownership. The preceding atomic schema repair was committed at that revision, and `just rust-test-p02b-operation-journal` passed there (36/36, zero skipped, Nextest run `60607790-3203-4393-8b05-1bd42dfd31ea`).
- Confirmed `FAILED` before repair: with one committed ledger event, an unfinished claim, and a coherently rewound sequence allocator, open tried to append the abort at already-used sequence 1 and failed with a SQLite `UNIQUE` violation instead of reconciling first. A separate damaged stale lease caused open to refuse after the unfinished claim had already been durably aborted and the allocator advanced. Both negative tests observed the wrong outcome against real SQLite.
- Repair: root classification now occurs after `BEGIN IMMEDIATE`, so concurrent first opens cannot both decide to seed a fresh allocator. The same startup transaction owns schema/seed, sequence reconciliation from the existing ledger, unfinished-operation abort, stale-lease cleanup, final event/domain integrity verification, and commit. Sequence reconciliation precedes abort allocation. Any later lease or ledger verification failure rolls back all startup writes, including the abort event, journal state, allocator, and fresh DDL; the checked writers retain one canonical allocator and ledger.
- Negative oracles: a behind allocator plus unfinished claim now reopens with exactly one new abort at the next unused sequence; a corrupted lease refuses with the original claim, zero events and original allocator unchanged; a damaged historical ledger event refuses with the same no-partial-recovery guarantee. The latter test exercises failure at the final integrity pass, after tentative recovery writes but before commit.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 35 unit, 6 auxiliary, 18 idempotency and 21 operation-journal tests; `just rust-test-p02b-operation-journal` passed 39/39, zero skipped (Nextest run `d6eb7759-1247-43aa-87fe-c7ca4f5a0e4c`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Focused `rustfmt --check` and `git diff --check` passed. Source SHA-256s: `open.rs=b455aee5088b283bdce503851c7d7ca4684f9c22f3f1c77706074245d3abbe53`, `idempotency.rs=77e35f9e96d48395c38c2562a49a0f5cc91d8b7f405b250e1dc7f14e28dfe45a`, `sequence.rs=73af0e0c892170006b06fe446612f5071bde1de0d0f75159cd3a3309fc1d4781`, `operation_journal.rs=6f256a0f6d74f3e636f33bc929b57bd2582f1ea96f5fcbbf8385bfee022958d7`.
- `NOT_RUN`: deterministic process-kill injection across the full startup transaction, clean-HEAD full `just verify`, retained-root inventory/migration/restore, release, deployment, activation, rollback. This does not close the broader P0–P2 objective.

## 2026-09-26 pristine-root classification continuation

- Scope: catalog open only, on committed `main` at `8baa04fbf5c6ebed801270a154e36eb8160aea3f` plus this source overlay. The preceding startup-recovery repair was committed there; `just rust-test-p02b-operation-journal` passed 39/39 on that commit (Nextest run `3b29478c-4a18-4ffd-8c75-1ca20291e0e4`). Concurrent retrieval edits are outside ownership.
- Confirmed `FAILED` before repair: an existing SQLite catalog containing only the canonical auxiliary tables but no fence allocator was classified as fresh because the current-table allowlist omitted both auxiliary tables. Catalog open seeded a new fence instead of refusing a pre-existing root whose historical fence state was unknown. The negative unit test first observed the wrong success.
- Repair: under the startup `BEGIN IMMEDIATE` lock, a root is fresh only when `sqlite_master` contains no tables at all. This removes a duplicated, incomplete current-table inventory and fails closed for auxiliary-only, quarantine-only, unknown and future-domain tables. An existing root with no allocator continues to require explicit inventory and offline migration/rebuild, not an implicit zero seed.
- Negative oracle: the auxiliary-only fixture now refuses for the missing durable fence row; its failed open rolls back journal-table creation. Focused `rustfmt --check` and `git diff --check` passed. `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 36 unit, 6 auxiliary, 18 idempotency and 21 operation-journal tests; `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed. Changed source SHA-256: `open.rs=a3a672182cf2cca64957df10f7d90d840f5ed4d92c9cf90b95eb24132247f1be`.
- `NOT_RUN`: clean-HEAD full `just verify`, retained-root inventory/migration/restore, release, deployment, activation and rollback. This narrow refusal proof is not overall P0–P2 closure.

## 2026-09-26 installed catalog schema authority continuation

- Scope: catalog-only overlay on committed `main` at `12b562aa072fac0c6568da5c60e0415e32312fae`; concurrent retrieval changes remain outside ownership. The prior pristine-root change passed the full catalog package on its committed tip (36 unit, 6 auxiliary, 18 idempotency, 21 operation-journal tests).
- Confirmed `FAILED` before repair: an installed `auxiliary_rows_v1` without its composite primary key reopened because `CREATE TABLE IF NOT EXISTS` skipped the existing table. An installed `mutation_lease_v1` without its scope primary key likewise reopened. These weaken canonical row identity and `INSERT OR REPLACE`/fenced-lease semantics. Both negative fixtures first observed wrong success against real SQLite. The auxiliary fixture also covers a weakened track table and a same-name index over the wrong column; the journal/sequence fixture covers a weakened sequence allocator and a same-name event index over the wrong column.
- Repair: one crate-private schema-object verifier compares installed `sqlite_master.sql` table/index definitions with the canonical DDL after removing only the creation-time `IF NOT EXISTS` phrase and whitespace. Candidate's existing exact table verifier now delegates to it. Auxiliary, journal, lease, fence, sequence allocator, sequence event/index, and operation-GC-floor objects are checked in the startup transaction before seed, recovery, or serving. The sequence event-kind enum-to-DDL parity check remains separate. The earlier partial fence/event/floor schema string checks were removed rather than retained as duplicate authorities. An incompatible installed definition requires explicit offline inventory and migration/rebuild; equivalent-but-differently-written SQL is not silently accepted as current.
- Negative oracles: malformed auxiliary table/index and lease/sequence table/index variants refuse during open. Existing candidate predecessor-schema and operation-GC-floor predecessor-schema tests still refuse. Fresh and current-schema reopen paths remain covered by package and process tests.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-catalog --all-features --locked` passed 38 unit, 6 auxiliary, 18 idempotency and 21 operation-journal tests; `just rust-test-candidate-activation-owner` passed 33/33, zero skipped (Nextest run `89bab8c5-ac01-44b4-8cd4-db1a06397a56`); `just rust-test-p02b-operation-journal` passed 39/39, zero skipped (Nextest run `388fe703-2af8-40f7-a104-ffd75a20250d`); `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-catalog -p quanta-index-repomap --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_ingest_idempotency::a_replay_after_restart_is_still_a_replay --all-features --locked -- --exact` passed 1/1 (62 filtered). Focused `rustfmt --check` and `git diff --check` passed. Source SHA-256s: `connection.rs=d549192b84c9f8463a3c03c88af81c2cef84bcc6f59b83fc12ce9b859e026ce2`, `auxiliary.rs=e34fac63c3149f33316d0bd0da1427a83b60a748088a3310f02c2b96de12ef6d`, `candidate.rs=80114a6e6557bdcdd72a79564a7fb04ad80ee67ea1d308e91a695ebacf0f21ce`, `idempotency.rs=f2e0b4911514c5ba16554b717686896e7b870d0eafbbe781fe967a9811d6d6aa`, `sequence.rs=db394000184d1d0787e1c26a2d6947ea9c2c23b81b14a5d2060d1a043441ea02`, `open.rs=e1ca05a63fe55e3f8d6cdde10c0539e79484d45d537411eef848a6955f4fc75b`.
- `NOT_RUN`: clean-HEAD full `just verify`, retained-root inventory/migration/restore, release, deployment, activation, rollback. This owner/process proof does not close overall P0–P2 work.

## 2026-09-26 single-flight panic and cancellation closure

- Scope: search-plane single-flight, snapshot registry and history-text epoch handles on committed `main` at `f16bad934ec6b5b902eaf2d8633d734a0c34d37c` plus this overlay. Concurrent retrieval edits remain outside ownership. The preceding installed-schema change passed the full catalog package on its committed tip (38 unit, 6 auxiliary, 18 idempotency, 21 operation-journal tests).
- Audited non-defect: a damaged auxiliary row is checked by `for_each_row`/`track_rows` during the daemon's `restore_auxiliary_rows_into` bootstrap before serving. This source path does not support a claim that catalog-open's lack of a second full auxiliary scan makes corrupted rows serving-ready. No duplicate startup scan was added.
- Confirmed `FAILED` before repair: after the single-flight outcome mutex was poisoned while a waiter slept, `wake_waiter` discarded the cancellation notification. A deterministic condvar fixture observed the missed wake. Separately, a cold-open callback panic left the snapshot registry's in-flight key and the history-text registry's opening slot reserved; later requests joined the abandoned flight instead of reopening. Both owner fixtures first failed against current source. A snapshot retirement encountering that abandoned flight could wait without a terminal outcome.
- Additional confirmed `FAILED` before repair: `retire` returned `NotResident` while a live oversize handle was held by a query; an evicted live handle was similarly invisible. Callers would then be allowed to reclaim the sealed generation's bytes under that reader. An opener that never returned also made retirement wait indefinitely. The first case was reproduced by a failing focused regression test; the resident-only accounting and unbounded wait were verified in source.
- Repair: cancellation and failed settlement take the poisoned mutex guard only to preserve lock-before-notify ordering, wake waiters, and keep poison as a failure. One shared `catch_open` boundary converts an opener panic to a flight failure for waiters; both owners remove their reservation and settle the flight, then resume the original unwind so supervision still sees the panic. Snapshot residency now keeps weak tracking for every opened/promoted handle, including evicted and uncached handles, and periodically sweeps dead weak entries. Retirement counts live incarnations of a key, fences an opening flight and returns `StillReferenced` without waiting for its opener; GC, repair and quarantine callers already defer deletion on that outcome. Repeated retirement counts the same flight only once. The misleading `OpenFenced` success variant was removed. No panic becomes successful content, and no non-returning opener can block retirement itself.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-search-plane --all-features --locked --quiet` passed 412 library tests, 38 auxiliary binary tests, 3 control-readiness owner tests and 22 provider-boundary owner tests (0 failed or skipped) on the latest owned source. The preceding focused `snapshot_registry::tests` group passed 14/14. The latest clippy attempt is `FAILED` by concurrent unowned `query_dispatcher/stage_timing.rs` lint errors (`manual_unwrap_or`/`option_if_let_else`); no lint finding pointed to the owned single-flight/registry changes. Focused rustfmt and `git diff --check` passed. Source SHA-256s: `single_flight.rs=c49dc8ad30373573ea212e1bed5b3a9a2f9c83f5fe3347e21545b1553b438e7a`, `snapshot_registry.rs=dc6f6fa8278ef9d9b24275502dba0b1b873e832d62bf2e56e14f15777f438fff`, `history_text.rs=4028185eb5cd5cd390ca118154f7b5e365a56f022f0f2d337aec3cfc5073c867`.
- `NOT_RUN`: a real daemon panic/supervisor E2E, clean-HEAD full `just verify`, retained-root migration/restore, release, deployment, activation and rollback. Concurrent explanation/timing and retrieval changes remain outside this owner's qualification. Overall P0–P2 closure remains open.

## 2026-09-26 history-text repeated-retirement closure

- Base: local `main` `8d23137cad89f2118377743824b8c841899593b1` plus three owned search-plane source files; unrelated dirty retrieval and query stage-timing edits remain outside ownership.
- Confirmed `FAILED` before repair: after a history-text epoch's first `retire_and_discard_epoch` returned `None` for a live reader, it removed the resident slot. The next identical call returned `Some(Discarded { bytes: 0 })` under that same live reader. A focused test adding only the repeated call failed exactly with that counterexample. `retire_and_discard_generation` had the same missing-live-slot path on retry.
- Repair: move weak live-handle custody into one generic `LiveHandleTracker` in `single_flight.rs`, shared by snapshot and history-text registries. The history-text state now owns slots and weak custody under one mutex. Landing an epoch records the handle before exposing the resident slot; retirement checks all live incarnations even when no slot remains; generation retirement unions current-slot and previously detached keys. Weak custody is swept periodically and does not retain native resources. No second independent lifetime-counting implementation remains.
- Narrow dirty-overlay proof: `./scripts/cargow --lane test-fast-lane test -p quanta-index-search-plane --all-features --locked --quiet` passed 413 library tests, 38 auxiliary binary tests, 3 control-readiness owner tests and 22 provider-boundary owner tests; doctests 0. Focused `rustfmt --check` and `git diff --check` passed. `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-search-plane --all-targets --all-features --locked -- -D warnings` remains `FAILED` only on four concurrent unowned `query_dispatcher/stage_timing.rs` findings (`manual_unwrap_or`, `option_if_let_else`); the owned search-plane findings from the prior attempt were repaired before this rerun. Source SHA-256s: `single_flight.rs=6cdff8ef37289a6c5ce502da581beaca2f15e69e9912999afaf711e88ba6af05`, `snapshot_registry.rs=5eada7425b5dfdc48e89e0e1d3c34dcb01fa80b40bf3284f2eaea65c87e58248`, `history_text.rs=8ea59abb2c257820bf3b51a766b765a7bbb1cf4853812dd6c7393f297ee14280`.
- Exclusions: no clean-source whole-repo qualification, daemon/supervisor E2E, retained-root migration/restore, release, deployment, activation or rollback proof. The concurrent stage-timing overlay prevents a passing package clippy claim; this does not invalidate the named test result. Overall P0–P2 closure remains open.

## 2026-09-26 retirement-consumer contract and dirty-overlay lint

- Base: committed local `main` `95d2b7cd5d4000c616811fef4634d0e89198acf0`; retrieval and query stage-timing work remain dirty and independently owned. Static caller audit found that physical search-corpus GC, damaged-generation rebuild and quarantine discard all stop before reclaim on `SnapshotRetireOutcome::StillReferenced`. The old GC comment still claimed that retirement waited for an opening flight; this section's source edit makes the nonblocking/deferred contract explicit and changes the two operator errors to name reader handles or open flights.
- Concurrent stage-timing lint unblock: two saturating numeric conversions in the untracked `query_dispatcher/stage_timing.rs` used a match rejected by the package's Clippy profile; the dirty hybrid timing test used an unchecked `usize as u64`. Only those three expressions were changed in the other owner's work, without staging or committing its files. No timing feature completion or ownership transfer is inferred.
- Dirty-overlay verification after those narrow changes: `./scripts/cargow --lane test-fast-lane clippy -p quanta-index-search-plane --all-targets --all-features --locked -- -D warnings` passed; `./scripts/cargow --lane test-fast-lane test -p quanta-index-search-plane --all-features --locked --quiet` passed 413 library, 38 auxiliary binary, 3 control-readiness owner and 22 provider-boundary owner tests, with no failures or skips; focused `rustfmt --check` and `git diff --check` passed. This is not clean-HEAD qualification because other dirty sources were compiled. Source SHA-256s for owned caller files: `search_corpus.rs=dfba236dad230b77c11792dd52f55d5f0662db2591807344c5d2f7841be7448a`, `generation_plan.rs=c7a3166349972eedb0d7e4d9759fcbda928e3f29397ca70e5d5f1b208bb26deb`, `quarantine.rs=207c6f9a07136e570a0800323267316e859805b6e57e717808704516c3da2d1e`. Dirty timing source SHA-256s at verification: `stage_timing.rs=95aabbdf74d8cf72183d65d8d26c45182c0c80c359e372f2e650ae0b96efd09f`, `tests/hybrid.rs=e55b1a30e9dca00de88696df4aa58d2bc7a7fbee4f74c1e6039aa8cd496f7f80`.
- `NOT_RUN`: clean-HEAD full `just verify`, P09 timing owner's final integration, retained-root operational migration/restore, release, deployment, activation and rollback. No overall P0–P2 completion claim.

## 2026-09-26 P09 required-backend source recheck

- Current `main` at `3ce73d7d1a619968677fbbf48d2f0ab88b827627` still has `required_backend: true` in `crates/quanta-index-searchd/src/app/readiness.rs`. The runtime constructor supplies boot-proven active identities and opened lexical/semantic adapter ports, but the open-port traits expose `open` and `open_proven`, not a bounded live-health observation. `ScrubTalliesV1` invalidates cached active-candidate proof after corruption/error; that is not a complete ongoing required-backend health source. Therefore P09 backend readiness remains `FAILED` by static source inspection, not closed by the passing search-plane package rail.
- No synthetic Boolean, startup-proof reuse or per-poll unbounded full active-inventory re-open was added. The required implementation boundary remains R2 of `FINAL-RESIDUAL-EXECUTION-PLAN.md`: one owned live signal for required backend availability and a bounded, authorized projection of the existing IPC event ring, with real daemon failure/recovery and observer-denial counterexamples. This is an implementation gap, separate from the dirty-overlay timing lint fix above.
- Root-loss RCA on pre-fix source: `quanta-index-lexical/src/inventory.rs::inventory_sealed_generations` and `quanta-index-semantic/src/lib.rs::inventory_persisted_generations` both return `Ok(empty)` for an absent track root. Their `TrackDiskUsagePort` implementations return `Ok(0)` for the same absence. The scrub scheduler discovers candidates through those inventories; if an active track root vanishes after boot, a candidate-less scrub can report idle and leave `ScrubTalliesV1`'s invalidation epoch unchanged. `RuntimeReadiness` could then reuse its cached active proof and report a healthy required backend.
- Local dirty-overlay P09 root-loss fix: `SealedGenerationIdentityProbePort` delegates each track's lightweight sealed-identity check to its adapter; the maintenance owner records an exact active `(generation, activation token)` observation at boot and each tick; readiness rejects missing, failed, stale (over three maintenance cadences), or wrong-identity observations. A successful fresh physical door proof seeds a newly activated identity immediately. Zero-active catalogs do not require track roots. Identity/marker reads are capped at 4096 bytes and probe failures are counted. This is identity/root liveness only; content bytes remain the boot door and paced scrub's proof domain. `./scripts/cargow check -p quanta-index-searchd-runtime --locked` succeeded and `./scripts/cargow test -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --locked -- --nocapture` returned 18 passed, including active lexical/semantic root rename/restore and zero-active missing-root counterexamples. The 4096-byte cap was added after that test and requires a fresh run. The test was on a dirty overlay, not clean-HEAD qualification. P09's bounded, authorized IPC event projection and deployment/activation evidence remain open.
- `51d3d34253d5d8ec0a72e1550a3c5e71ca38d8ab` commits only the readiness fix and these SEP-21 docs; another writer's staged `tools/benchmark/manifest.json` and unrelated dirty retrieval/contract files were preserved. The focused `quanta-index-searchd` backend-observation unit test returned 1 passed before the final sidecar-read cap. A later package Clippy attempt was blocked by the concurrent dirty `TextQueryResponse.explanation` contract update before it finished; a narrower lexical/semantic Clippy attempt was interrupted during its dependency rebuild under concurrent host load. Neither attempt qualifies final-HEAD lint/test proof. Re-run from a stable source/lockfile snapshot before promoting the fix beyond local implementation.
- Follow-up on committed readiness code at `e88eddbaf0ba567ea33e8d868a8b37a96b47024c`, with unrelated contract/retrieval/Cargo dirty state still active: `./scripts/cargow check -p quanta-index-searchd-runtime --locked` completed; `./scripts/cargow test -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --locked` returned 18/18 passed after the 4096-byte cap. A new test then prepared an active generation, stopped that runtime, started the Cargo-built `searchd` binary over the same state root, and observed lexical root loss and restoration over the control socket; its focused run returned 1/1 passed. These are local dirty-overlay command results, not source-frozen release evidence. The new binary-process counterexample must remain in the registered owner rail and be rerun from a stable snapshot.
- Current `main` after the concurrent `420a6452` commit also contains direct oversized lexical sealed-identity and semantic sealed-marker refusal tests (`quanta-index-lexical/src/index_store.rs`, `quanta-index-semantic/src/lib.rs`). They were not part of the 18/18 readiness run. A focused `./scripts/cargow test -p quanta-index-lexical -p quanta-index-semantic --lib oversized --locked` attempt was interrupted during a cold Lance/DataFusion dependency build; an explicit warm `test-daemon-lane` retry first waited behind another live Cargo build lock and was interrupted without running tests. Thus these two new tests remain `NOT_RUN`, not passed. Re-run on a quiet, source-frozen checkout; do not treat compilation progress or their presence in `main` as evidence.
