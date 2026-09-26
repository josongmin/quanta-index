# SEP-26 Retrieval Active Gap Register

Status: `ACTIVE`

Consolidated: 2026-09-27 from clean pre-documentation snapshot
`5e6addd5814ce8b71af808ff201ddd0b18fbe6c4`.

This file contains only work that can still change a current verification or qualification claim. Accepted design is
owned by the [SEP-26 ADR set](../../../adr/README.md). Historical detail is indexed by
[ARCHIVE-MANIFEST.md](ARCHIVE-MANIFEST.md).

## Pre-freeze checkpoint — 2026-09-27

Observed main before this update: `d2b79fda70d8dc4c47bf75a51408190474c1fa9c` + integrated symbol-source dirty;
this update adds active-document dirty paths. No staging, revert or commit is performed by this writer.
These two active-document updates enter the source closure; this is not a post-freeze result ledger. Historical
capsules retain their original source identities and are not qualification of this current main revision.

- The previous `c6dd70af` snapshot completed actual SDK18/Python319/Rust108, three fresh validation/replays,
  separate CI1640/15subtests and12 gates, stable identity1417/package3626/232pycs, and both consumers'8mutant
  checks: `VERIFIED` only for those bound inputs. This is not all-defect closure or current-source qualification.
- Newly confirmed P1: the old guard accepted native-inadmissible language strings and unknown/ill-typed symbol
  kinds (native contracts: open ASCII syntax; nullable12 closed kinds), plus Unicode surrogate strings not
  representable as Rust UTF-8 `String` in native DTO/raw input. Seven old language/kind variants and three
  input-only Unicode fields reached the permissive path, plus scoped logical-snippet consumer counterexamples.
  The external corrected check rejected15 language/kind plus9 surrogate variants (24 total), retained24 valid5/5
  controls, and rejected18 raw-only cases. These are local logical-incremental consumer oracles, not signed producer
  forgery or a demonstrated full-custody exploit. Root integrated the canonical
  language/kind/UTF-8 guards (`conditional_proof.py` digest prefix `702fe`) and persistent raw18 assertions into an
  existing test method without new authority IDs. Root focused owning method is `VERIFIED` local:1 passed/318
  deselected, stable selected code inputs, Ruff/diff0. [Raw](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/final-native-types-raw-table.log),
  SHA `2bfa0ff52b8378e16956fbb0fd8ae9071573c14d8f3a87d4cbaebab3e19e86bc`; conditional source SHA
  `702fe9ffbdffa23ac7d3c0288d94234e772703042c60d8567b463d8607c9d38e`, test source SHA
  `8aeadd926cc560b9718ef2eadd159355f271fb92cda417ce7ee8897e69e976b6`. The local command used user
  `PYTHONPATH=.`; it is not canonical process custody. Owner external whole328 is `VERIFIED` local:328 selected,
  executed, passed and unique JUnit identities match exactly; zero failures/errors/skips,436.42s. Independent
  receipt replay checked all20 artifact hashes and identical pre/post source/tool/environment/dependency snapshots.
  [Owner receipt](/private/tmp/qi-rbr-metadata-audit-proof/owner-receipt.json), SHA
  `d5eb65d832bbe317cf660952a574da13be49d63ea5d9782d1c46bf5dc34e68bf`; whole log SHA
  `1e742346423107ec5f873b39b9c0702213c5e359c0b57a7665ec441a2fc06490`, JUnit SHA
  `e13e65bc32b7bd51f59d3b4f11cb79025eb9c5da7be88b948260052d879b3dc6`.
  Its tested bytes were sealed into isolated clean owner commit `339a20cdc207a70dfa58e1c163937548d8c6c147`;
  it excludes root's additional18 persistent raw assertions (owner test SHA `8df51b6e...`, not main `8aeadd92...`).
  It also excludes current-main integration, new Rust/SDK/model execution, pair/quality/performance and exhaustive
  defect absence. The new final full capsule is `NOT_RUN`; focused/owner results
  must not be composed. External positive checks are not proof of the final integrated bytes.
- Hosted CI verification is separately `BLOCKED`:14 jobs/0 steps due to billing. This is not manual engineering
  procurement and is not replaced by the local12-gate result.
- Shared benchmark evidence-reader custody defect is locally repaired within G-01 (no new feature ticket):
  `read_evidence` and raw verification now consume the existing no-follow regular-file reader, so same-byte
  symlink swaps and linked evidence/ancestor paths cannot pass via a prior `is_file` check. Optional latest/baseline
  pointers use the existing no-follow entry-presence guard: a truly absent pointer remains absent, while dangling
  or linked ancestors fail closed. Before fix, the selected8 negatives/control cases had6 failed/2 passed
  ([RED](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/final-store-reader-red.log), SHA
  `0bd512ea4539f4a823f033ca1cf78e6d859627842ba7fabedc6650f73343b326`). After serial integration,
  four owning files' local suite had156 passed/0 failed and selected source inputs stable
  ([GREEN](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/final-store-reader-green.log), SHA
  `38b855b2d984fdcd9809880a1d42597bbcd1c6e0f3cbfd903d3e96e12b02f721`). Evidence reader source SHA
  `9b448675de28a688e09b29af27dd318095caee23b9b569216de99d68bf74ca6f`, conformance-test SHA
  `e4083cf75947e3792a9ab59d38a75400973c9ae041b5f5dca36689e9ba113bf2`.
  This is a bounded reader/path-custody fix, not a broad GC, host or streaming-import claim. New frozen full suite
  and process/evidence qualification remain `NOT_RUN` until terminal.
- Symbol-boundary P2 before-fix probe had five failing counterexamples plus one already-correct nested-generic
  control, not six defects. Rust generic-owner character stripping mishandled `fn() -> ()` and const `<` expressions;
  nested named functions under TypeScript class arrow/static blocks and Rust impl const closures were classified
  as methods. [Native six-case before probe](/private/tmp/qi-rbr-readiness-review/native-before.log), SHA
  `8ddbc237cdf4e6e8e1b3a523bbcb31b124d16c1cb2c2c2054d7807fb6986356e`, used pinned grammars with zero parse errors.
  Root now serially integrated the exact final-v2 tested `symbols.rs` SHA
  `3c120e572d28fc6bbc10749a1ba3fce9aca8512d5e79158887ec291985b8992c`. Rust methods require direct impl/trait
  declaration-list ownership; JS/TS function declarations remain functions and explicit methods stay methods;
  Python nearest named scope survives control flow/decorators. Associated-type const cases and same-root controls
  are included. Pinned turbofish `ParseFailure` remains a negative, not an accepted positive fixture.
  [Owner receipt](/private/tmp/qi-rbr-readiness-review/owner-receipt.json), SHA
  `218b2baab77feef40a4d1b7538a1d5966c5a8fac4fda95e4566dc7d1b10b8006`: **VERIFIED local**, actual83/83 unique lib
  identities, native11 goldens, all-target Clippy/fmt0,38 artifact hashes and source/tool/environment snapshots
  stable across1424 files. The command was `./scripts/cargow --lane test-daemon-lane test -p quanta-index-retrieval-bench --lib --locked`.
  Collection108 is83 lib+25 chunking; those25 were not executed in this proof. Full current-main/SDK/model/daemon,
  whole CI, qualified pair/performance and all-defect absence remain excluded. This is producer identity/classification,
  not proof of a ranking bug or a symbol-authority expansion.
- The previous `11f146cfc643fa06decd9a8cbdcce8fb1810ac97` run is not final: SDK18 passed and12 gates exited0 within their original scope, but root
  requested a controlled stop after the P2 discovery. Python319/Rust108/full CI were interrupted and remain
  `NOT_RUN`. [Native interruption record](/private/tmp/qi-rbr-final-native-admission.BOl8u0fy/INTERRUPTED-CHECKPOINT.md)
  preserves the untouched snapshot and incomplete publication. [CI stop receipt](/private/tmp/quanta-trust-terminal-final-yt76u851/stop-receipt.json),
  SHA `d7e2392f4f454d5170ac2596ca210cdd23b914839c0456012cad845182f4f83b`, reports1047/1640 completed, exit2,
  and no remaining original processes. Preserve these partial results without composing them with `c6dd70af` or owner/focused proofs.

- Previous capsule contract319: `FAILED`, 318 passed/1 failed, due to a stale CRC diagnostic expectation. The
  fixture now checks the exact current refusal. Its actual SDK18 passed; Rust108 was `NOT_RUN` and the partial
  results are not composed into integration success. [Failed producer receipt](/private/tmp/qi-rbr-final319-native-share.lu7R0r/failed-producer-receipt.json),
  SHA `a7cab428f689f98a79f4950a1853df136222588a2d9b63dcfad281693c70da89`; historical Python JUnit SHA
  `4a10b882a23e1145945c02e7d448e3d6b609b657f7a22b1b94e6afb38240f715`.
- Previous CI1639: `FAILED`, 1637 passed/2 failed. The module-class reload failure was traced to the canonical
  bridge import; that import and a regression test were added. The actor positive fixture now states its explicit
  boundary. Its reusable-fixture preflight additionally found two setup errors from a missing capture import;
  the explicit import is now integrated. [Preflight raw](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/capture-actor-fixture-before.log),
  SHA `92534ab3e502dde2c93e8e398de6a3a9f66ec782ebacd431f7f23723b2c367f7`.
  Fresh standalone capture32/32 passed with stable selected source inputs, [plain raw](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/capture-actor-fixture-plain.log),
  SHA `b10c725ab1be4b555840e22f877873842e10618c4a7e7f634e33f8e821ce1875`.
  That first failed CI raw root is `/private/tmp/quanta-trust-terminal-1st7p9q2`; its exact receipt digest was not
  received here. Subsequent `c6dd70af` full verification is recorded above for its own bound inputs; the final P1
  capsule remains `NOT_RUN`. Source fixes and standalone results do not replace either full verification boundary.
- Bounded development execution is already complete: SEARCH3, native5 with independent replay and 85 negative
  mutants, and ANN probes. Their original receipts remain local, source-specific evidence, not new feature work.
  SEARCH3 covers 12 blind literals/88 files (file hits10/12, occurrence27/30,27/30,28/30); it excludes semantic
  gold, holdout and quiet performance. Native5/85 excludes full conditional source/model custody, encoder,
  fault/restart and performance. ANN covers the bounded406-row/4-query fixture, not broad corpus/churn quality.
  [SEARCH summary](/private/tmp/qi-rbr06-final-matrix.WSMPZu/SUMMARY.md), SHA
  `1571d783db7d71498bef2718abd144a2db0a18e2639d24a4fd360fa9fccb1b47`;
  [native5 raw](/tmp/quanta-native5-current-v3-20260927.observed.json), SHA
  `38c0ad5e03066df1d9687dc44fb36a04de776156ca1e8f717d8ab8dc6d29154d`;
  [ANN summary](/private/tmp/qi-rbr07-real-ann.DH0pzi/SUMMARY.md), SHA
  `e2936870978ccadc746b9ab424fb010a20718ceea3ebd64667198fe524ade625`.

Final terminal results must be recorded externally under [TEST-PLAN §5](TEST-PLAN.md#5-증거-묶음과-완료).
Planned final closeout (`NOT_RUN`): `/private/tmp/qi-rbr-symbol-boundary-final.x3ddOxQY/final-closeout.json`.
This is a destination, not an existing artifact or proof. No post-freeze status edit is planned here.

| ID | Scope | Current status | Exit condition |
|---|---|---|---|
| G-01 | RBR-00/02/04/05/12 final-source integration after native P1/symbol P2 and evidence-reader custody | P1/P2 and bounded no-follow evidence reader integrated; root focused1/owner328, final-v2 symbol83/native11 and reader156 are local proofs with explicit exclusions. `11f146cf` SDK18/12gates completed separately; Python319/Rust108/full CI interrupted `NOT_RUN`. Next full capsule `NOT_RUN`; hosted CI separately `BLOCKED` billing14jobs/0steps. | Freeze final code/docs and rerun canonical Python/Rust/SDK inventories, including all25 chunking tests and persistent raw18 controls, plus separate full CI/gates/evidence replay. Bind exact selected/executed/passed counts and custody in the new external closeout. Preserve `c6dd70af`, partial11f and owner proofs only for their original inputs; no composition or all-defect closure. |
| G-02 | RBR-01/09 observation overhead and query performance | `NOT_RUN` | Run identical on/off workloads, including tight-deadline behavior, then the declared k/filter/floor matrix on a quiet host. Bind actual planner trace and preserve failures/timeouts in the result. Default floor remains 100 until the qualified decision rule passes. |
| G-03 | RBR-03/04/06/07/09/12 external qualified evaluation and declared breadth | Qualified evaluation `NOT_RUN`; bounded SEARCH3/ANN development probes locally completed | Evaluate the declared broader corpus/filter/churn scope and one admitted final pair/replay when its inputs exist. Preserve the completed bounded probes; do not rerun them as missing implementation. Admission/gold/quiet-host procurement is user-owned manual input, not a blocked engineering task. |
| G-04 | RBR-08 optional canonical symbol text authority expansion | `NOT_APPLICABLE` unless explicitly added to product scope | Retain the accepted typed refusal. No ranking defect has been established. Only an explicit new product scope opens schema/ingest/lifecycle/cursor migration; a rank change separately requires a source-bound misranking case and declared evaluation. |
| G-05 | RBR-10 ingest performance and resilience | Measurements/fault/restart `NOT_RUN`; bounded native5/85 local execution complete | Bind a current installed SDK/daemon and run fresh, replace and delete workloads with row-set invariants, activation, fault and restart checks, plus quiet-host latency. Preserve the completed native5/85 scope rather than reopening implementation. Transient timing must remain separate from durable receipts. |
| G-06 | RBR-11 supported-platform resource ownership | macOS bounded owner checks exist; Linux qualified delegated-cgroup/Landlock positive proof `NOT_RUN`. Native Windows paired capture `NOT_APPLICABLE` absent explicit new scope. | For supported macOS/Linux, bind current owner/resource proof with platform-specific custody. Linux fake-owner/diagnostic process-group tests are not qualified cgroup positives. Preserve legacy macOS `ps` PID-start-identity limits; do not invent a Windows support task. |

## Closure order

1. G-01 after the final documentation revision.
2. G-02 and G-05 on immutable current binaries.
3. G-03 qualification only when its user-owned external admission inputs exist; bounded development evidence remains complete within its exclusions.
4. G-04 is not active engineering work unless explicit product scope chooses the symbol-authority expansion.
5. G-06 per supported platform.

`PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` remain `NOT_RUN` until their own exit conditions pass. A local pass,
artifact presence or summary boolean does not close a row.
