# NC-H · History source custody and Native adapter

<a id="execution-update--current-user-requested-closeout"></a>
## Execution state · 2026-11-17

**Partial.** Q History components have executed bodies for typed FULL/DELTA,
readback, replay custody, source-inventory verification, SDK/UDS queries
and restart. The installed S producer → Legacy source → Native candidate route has
not completed a qualified migration of an actual existing History root. The
Q BOOT source now checks contiguous History FULL publications after the
attested Source/AST view against the complete physical Legacy generation
census. S can stream the exact original inventories in generation order under
one producer fence and append one FULL per selected generation. The
Linux installed execution is pending. Export of generations with
discarded originals requires an authenticated snapshot checkpoint.

The S production no-default class **VERIFIED** compilation on its recorded
source. The separate Required History DATA regression **VERIFIED**
persistence after a later terminal budget refusal. The newest S CLI check
**FAILED** before Rust compilation because its frozen lock lacked two
declared test-support dependencies. Current S has the lock correction and
the DFG, Runtime, Taint and PTA Map kind corrections.
The [active backlog](#current-actionable-backlog--2026-11-16-re-audit)
records the frozen runs and remaining acceptance.

The current Q Catalog verifier additionally recomputes replay genesis from the
FD6-admitted writer-state root, workspace root, contract key, and first original
body/ACK. Its focused two-case nextest run **VERIFIED** the scope and root
regressions (`5dcc1052-7770-4fdf-a88e-59bba7799250`, 2 PASS / 659 skipped /
exit0); the earlier seven-case receipt covers the previous verifier revision.
The Q exporter and S candidate can transfer a paged checkpoint for settled
Legacy rows with a real applied ACK before candidate sealing. Final BOOT still
checks the physical source and writer fence. The runtime installation has not
executed this path.
S assigns the next generation to each new manifest and binds History to that
generation and manifest digest. Q's `SourceHistoryAdvance` takes that real
successor batch and the exact prior cut under one Source and History CAS. Q
retains the exact Original, commit-recency and file-contributor inputs under the
new member. Final restore carries the verified History member and cut in the
same FD3 success terminal that S archives before restarting. S can decode the
combined canonical V9 request under Original admission, including the typed
prior History cut and its retained repository/revision identity backing.

The current Q focused nextest run **VERIFIED** all 10 executed cases
(`4e109dca-3d5a-4a81-877b-ffe46ff02bd0`, exit 0). The
`source_and_next_history_generation_publish_in_one_view_and_reopen` case
executes Q Source and successor History in one Catalog view, refuses incorrect
generation/derived inputs, and reads the committed member after reopening Q.
The same run covers the installed-terminal wire binding, admitted SDK readback,
Legacy census/ACK refusal, and the launcher authority gates. The positive
offline Legacy exporter and malformed History FD7 process cases are Linux-only
and remain **NOT_RUN** on this run.

Ordinary S Product uses the exact committed Source generation digest as its
History revision, so publishing the next S manifest changes that revision.
Q now checks same-repository generation succession under the exact prior cut
and binds the new revision from the immutable original. The Q UDS run
`06784c8b-5cb1-4a1c-8afe-6858dfd65d77` **VERIFIED** three cases after this
change. The combined case rejects a forged prior revision, preserves the
old member and reads the new revision and prior commits after restart.
The readout now returns the predecessor cut commitment from the authenticated
successor manifest. The follow-up run `0a018076-bcf3-429c-9cd3-c50a259b4f65`
**VERIFIED** the exact commitment after reopen and its admitted SDK decoding
(2 PASS, exit 0).

The existing combined Q UDS case now captures the original published Catalog
view before shutdown. After reopen it checks that view, Published operation,
both History cuts and the successor query before submitting any retry.
Submitting the original request must then return the same view and leave
Latest unchanged. This prevents replay from masking a cold persistence
failure. The Q owning feature class **VERIFIED** that cold assertion and both
Catalog inventory/quiescence cases in `459aaed5-981a-425d-a575-6becd60d7022`
(3 PASS, 737 skipped, exit 0). The first attempt reached E0432 before any body
because `--no-default-features` disabled `E2eRuntime`'s `legacy-tantivy`
dependency. The later regression now publishes actual History in two
revisions with the same ordinal and asserts correct order, exact page/ACK
readback, and refusal for either missing revision. The modified body is
**VERIFIED** in `3eccd731-5e31-4d07-915b-f18d56f9d63b` (1 PASS / 1994 skipped /
exit 0).

The existing Linux `offline_legacy_history_export_streams_real_rows_and_terminal_ack`
case now seals and activates two generations in the same Legacy revision
through the actual Q daemon. It publishes a distinct History batch against each
sealed lexical manifest and keeps both applied ACKs. The fixture installs a
manifest-backed empty restored root before publishing and uses that same
selected root for both generations. The Linux ARM64 body **VERIFIED** the
ordered inventory and each original commit, snapshot digest and terminal ACK
(1 PASS / 59 filtered / exit 0). The selection and Legacy files stayed
unchanged across export. The previous attempt failed before the exporter
because a post-publication backup refused a Lance hardlink. The independent
malformed FD7 process body passed in that attempt. These are Q component and
process results. The signed S producer and installed candidate still require
H-ACCEPT-01.

The current Q receiver checks that a successful retained terminal and
the selected BOOT agree on History presence as well as the exact installed
witness. The Darwin no-default Runtime library regression
`restore_forward_retained_terminal_binds_history_presence_and_cut`
**VERIFIED** the retry comparison in
`7f9f7eb4-4d18-4bb2-9ea3-9e697f425a58` (1 PASS / 620 skipped / exit 0).
It covers a new History cut without a prior terminal, matching successful
terminals, addition and omission across a retry, and a changed replay start.
The signed launcher and installed recovery bodies remain **NOT_RUN**.

## Current actionable backlog · 2026-11-16 re-audit

The H proof sequence covers S CLI, then the signed installed replay,
cross-revision checkpoint and successor recovery. H-INTEGRATE-01,
H-INTEGRATE-04 and the installed part of H-INTEGRATE-05 are assertions in
the existing H-ACCEPT-01 fixture and use its F guest and release inputs.

| ID | Concrete remaining work | Current evidence / completion condition | Writer boundary |
| --- | --- | --- | --- |
| H-CLOSE-01 | Verify the adopted S CLI History/RepoMap/SearchCorpus callbacks | Current S transfers the Original aggregate's Required History receipt identity. The Map DFG slot and use-kind repairs now span DFG, Runtime, Taint and PTA. Check `20261117T015405.067830Z-622e5e309813` **FAILED** before Rust compilation on a frozen lock missing two dependency edges, already corrected in S `60dc03485e8`. Owning CLI compilation is **BLOCKED** while the external snapshot and build cache are disconnected. | S CLI and resident candidate owners own callback and Linux checks. |
| H-INTEGRATE-01 | Qualify the installed authenticated checkpoint handoff when S originals are unavailable | The [ADR's pre-ingress inventory, checkpoint and FD3 handoff](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover) are supplied. The signed fixture checks incomplete inventory refusal, seal, BOOT and the exact retained terminal for a Q-only cut with no S original. To qualify a production root whose prior S originals were retired, verify its own source census, terminal ACKs and readback. | Q owns physical read and final verification. D owns launcher and S candidate preparation. S cutover journal owns the retained terminal. |
| H-INTEGRATE-04 | Verify the adopted Source/History DELTA and retry across Q commit and S terminal append | Reuse the [Source/History CAS and V9 contract](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover). Verify owning compilation and terminal persistence, then bind same-operation retry to the installed Published view, predecessor cut and immutable History input. Installed successor execution is **NOT_RUN**. | Q owns transition and CAS. S producer journal owns live/cold binding and terminal evidence. |
| H-ACCEPT-01 | Qualify installed History FULL, successor DELTA and restart | Run `native-installed-history-full-delta-process-owner` once on F's signed private retained bundle. The [History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover) owns the exact two cuts, Source-bound successor, ACK-loss and restart oracle. The installed body is **NOT_RUN**. | [D](NC-D-boot.md) owns install and launcher. [F/Root](NC-F-integration.md) owns signed Linux execution. H owns the History fixture and writer. |
| H-INTEGRATE-05 | Qualify installed cross-revision History | Q BOOT snapshot identity and installed-row rejection are **VERIFIED** in `4340d420-fc85-446b-8bfa-6a339a15739a`. The same-ordinal multi-revision inventory is **VERIFIED** in `3eccd731-5e31-4d07-915b-f18d56f9d63b`. The installed cross-revision cut is **NOT_RUN** and uses H-ACCEPT-01. | H owns History candidate targets and cross-revision proof. |

Signed installation remains **BLOCKED** pending the frozen S/Q intake and
distinct Legacy and Native Linux ELF receipts. Its fixture bodies are
**NOT_RUN**. The [F queue and Published handoff](NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)
owns current R16, Caller and supplier progress.

Two wider production routes require their own inputs and verification with
[D](NC-D-boot.md).

- Legacy roots with sibling active SearchCorpus or RepoMap heads require a
  complete physical active-head inventory and queryable revision-to-view
  mapping or a preserved serving route. BOOT currently verifies one active
  Source/RepoMap revision and rejects siblings. Q Catalog and D
  Source/RepoMap own their carry-over.
- Previous Native → fresh Native with History requires an authenticated
  old-root History inventory alongside Source replay coverage, exact installed
  candidate carry-over under the held roots, and readback of each retained
  cut after restore. The current receiver rejects a retained History terminal
  in this branch. D and Q BOOT own the selection and physical receiver. The
  History producer owns the coverage supplied to that receiver.

The BOOT regression also rejects a replay manifest pointing at a snapshot
checkpoint, a different durable sequence, a different source manifest, and a
content-addressed checkpoint page with a well-formed row outside the
authenticated cut. The admitted IPC regression checks one retained wire
allocation. The Linux
`native_uds_bound_admitted_projection_stage_crosses_single_frame_with_child_ref`
case also checks the actual peer-bound SDK request, child-bearing immutable
object identity and ingress discard. Run S CLI compilation on its existing owner rail. Reuse H-CLOSE-03's
recorded DATA body and refresh affected proof on the same S source when qualifying
that build. Take the admitted S/Q pair to the existing F signed retained guest
for the installation, ACK interruption, and restart checks.

The additional S Kernel paid-parts retry was **BLOCKED** before Cargo. Artifact
capacity was 50.1 GiB and 2.7% after automatic GC, below the required 2.75%.

Retry13 used the existing `history-source-readback-snapshot-artifact` owner lane
with paired source `1ede354134f932f60219080bd1d50e73e8013cd9+ext-34d5edae4531633270f4`.
The `quanta-runtime` integration target selected
`persisted_required_data_survives_late_private_terminal_budget_refusal_v3`
under `feature-isolation:quanta-runtime.no-default.7f44320823fb`.
The queued CLI check did not start because the same admission condition remained.

Retry14 of the isolated DATA/private-budget regression on frozen S
`4f6d7c59b9a` cleared host admission. Cargo **FAILED** before the test body
on an unused `SinkCategory` import in `quanta-contract-security-types-shared`
(`20261116T100842.758344Z-54030d4f1ff8`, exit101, 0 bodies). Live S commit
`a8944939cb1` removes that import. The pending Linux and isolated Runtime proof
must observe the repaired source; the earlier receipt retains its exact scope.
The retry on `history-source-readback` captured a predecessor source snapshot
and exited 1 without a Rust terminal result or DATA body. The later repaired
DATA run is recorded below. The signed Linux suite requires its installed Q/S
binaries, digests and signed inputs.

Two live CLI attempts on `history-source-readback` exited before Cargo when a
tracked S source changed during hashing. The frozen S/Q snapshot
`b2472b870ff07c61fa9b343205c218e3ee9e628b+ext-f326170806f8cd42a359` reached Cargo on
`history-source-readback-snapshot-artifact`. The S CLI binary check used
`feature-isolation:quanta-cli-server.no-default.69a9257c2d02` and **FAILED** before
the CLI crate (`20261116T164934.659198Z-27102e5a41ea`, exit 101).
`quanta-adapters-dfg` passed a native allocation admission where
`validate_target_scope_v1` requires a canonical semantic output admission
(`E0308`) and omitted `recursive_heap_path_v1` when rebuilding the Java direct
call state (`E0063`). The Java owners supplied both fixes in S `fd8bffaefe3`.
The second CLI run `20261116T171641.527410Z-fa7a579dd737` used the source pair
`fd8bffaefe37e6822a9a1ee2b052c06f5b1e6510+ext-52148576d32bb9e60912` and passed
the earlier DFG diagnostics. It **FAILED** (exit 101) in `quanta-taint-solver`
because the frozen commit lacked `signed_java_return_v1/original_v3.rs`.
S `29e297cd4f3` added the file. The isolated DATA feature class reaches DFG
through `retrieval-authority-python-min` and does not select the CLI's Taint
Solver feature.

The isolated DATA run on that repaired DFG pair reached
`quanta-runtime-analysis` (`20261116T172318.081792Z-fe2db3e6daed`). It **FAILED**
with exit 101 before executing a body. `recursive_conditional_v1.rs` compared
the `u32` argument index with a `usize` enumeration ordinal. The existing S02
Java recursive profile claim owns that leaf. `ternary_source_v1.rs` passed four
use-index slots into the shared five-slot row carrier. The live ternary path
uses five slots throughout while retaining its existing shape and use count.
The adjustment passed scoped `rustfmt` parsing and `git diff --check`. S02
fixed the recursive comparison in `0de47bce1157` by using typed `u32` ordinals.

The third CLI check on
`971e7b66d939ae253beb74a0095e7c4e170d241a+ext-75334b8382e5d83b0647`
reached Taint Runtime Analysis
(`20261116T181052.982392Z-0941b5da7e9f`, exit 101). It found an
`EntityId`/optional DFG entity mismatch in `python_write_binding_v1.rs`, a
typed authority file comparison in `metadata_read_v11.rs`, and a Runtime
error propagation mismatch in `python_paid_v14.rs`. The isolated DATA
selector uses its existing narrower feature class on the same source pair.
The authority-file comparison now borrows its canonical string. In the current
S source the Python paid callback retains its typed Runtime result and returns
it when the DFG lender has completed. S05 supplied the canonical `EntityId`
comparison in `13ee4e88245`. Frozen S/Q
`6c217a59de3a60ba1aa859ec555b51d9061523ac+ext-c391187367dbca17ce7a`
contained the three source fixes. That check
(`20261116T190134.784695Z-f31f33a3f3cf`) **FAILED** in
`quanta-taint-solver` with exit 101 before compiling the repaired Taint
Runtime Analysis crate or the CLI binary.
There was one root cause in `original_full_flow_v1/switch_v1.rs`: the
import retained the old scalar-shape helper name after both calls switched
to `try_same_full_scalar_shape_v1`. The Solver owner corrected the import in
`1b7a598b2cc`.

Frozen S/Q
`f07b05c35e777230370d70ee43f986f4fcedd094+ext-de14324ff024d34f84d5`
passed the Solver import and Taint Runtime Analysis diagnostics and reached
`quanta-runtime` (`20261116T191634.386771Z-8f47f2bc57db`, exit 101).
The SDK model identity, synthetic honesty digest and request sizing matches
did not handle the new `StoredValue` sink variant. S commit `334cd4899a7`
adds typed site identity and matching digest and budget inputs to all three
consumers. The follow-up `e7aa67c57c7` adds a distinct-site model identity
regression. The CLI owner rail verified compilation past the SDK consumers
on the repaired pair
`e7aa67c57c73110d4bca072ea053fba87acc9104+ext-6fe4a4cdd9e500bb4b8b`.
It then **FAILED** in `quanta-sdk-runtime-composition-owner`
(`20261116T194339.454737Z-a91c83525562`, exit 101). The current Local fragment
had an inner doc comment despite being included as items. The extracted
current-role module referenced the analysis key and pipeline one level too
low. S `2bb9f88e74d` fixes the comment and both references. The current
composition crate's 20 Rust include targets also pass a static inner-doc
scan. The repaired CLI compile remains pending.

S `782db77f479` aligns the History candidate replay and writer-fence
helpers with the retrieval bundle that provides their implementation.
The current CLI distribution and Native SDK indexing entry points both
enable the Python-min retrieval bundle. Static inspection found no
unsupported call site under the narrowed gates.

The live Q overlay changed during both ordinary and sealed snapshot
capture, so those attempts ended before Cargo. The retry isolated Q's
3,274 tracked and standard untracked files outside `docs/` and verified
their paths, executable bits and contents against both live reads.
The CLI owner lane ran
`6d624ca802678bb36fda9900ac804303f8071f8c+ext-88463302332e99f6c4c5`
as `20261117T011344.045385Z-469312752a14` and **FAILED** in Java Map DFG
(exit 101). The Port use receiver needs `u16` slots. The DFG owner
corrected the checked conversion and routed both Map uses through the
existing `AssignCall` emitter. The call context still binds the exact
method and argument. Taint and PTA now compare that kind. H reconciled with
the existing Runtime writer and adopted the four matching `AssignCall`
comparisons in the two Runtime validators. Rust parsing and scoped diff
checks passed, and all referenced Port statement variants exist in the live
enum.

The retry uses `7cd7c315b75` with the committed DFG, Taint and PTA fixes.
It tests the two Runtime comparisons as an isolated overlay and reuses
the exact Q and GQLang inputs from the previous run. All 17 referenced
`PortDfgStmtKind` variants resolve to declarations in this source.
The frozen CLI input was
`7cd7c315b75bf8d6a06ccf94e8a030d7eeb7292a+ovl-1000173d0f601b5f4326867a95a0a9ce+ext-88463302332e99f6c4c5`.
Run `20261117T015405.067830Z-622e5e309813` **FAILED** with exit 101 before
Rust compilation because `--locked` refused the outdated lock.
S `60dc03485e8` adds the declared `ed25519-dalek` and `tempfile`
dependencies to the test-support package's root lock entry and updates
the nested lock. Before the next corrected-source run, the external SSD
disconnected. Its frozen source, warm QBC lane and S virtual environment
are currently unavailable. No repaired CLI compiler result was produced.
F retains its own release intake.

<a id="s-cli-and-required-history-data-proof"></a>

The isolated DATA run on that pair reached `quanta-runtime`
(`20261116T182136.122109Z-181eac4d1000`, exit 101, zero bodies). The
Product consuming bridge wrapper was exposed when its generic method and
error type were disabled. The wrapper and imports now share their guard.
The Product taint caller's feature set already implies that guard.
The repaired pair
`2f7adcdb25dc5ae3191d23d140c96a7682b2867e+ovl-31528a4c686c78dcc7c5cefe477a97cd+ext-7f406ab277f6e498a90b`
**VERIFIED** H-CLOSE-03 in `20261116T184029.973262Z-d3d4c6ba39a3`.
The QBC integration target `history_required_terminal_budget_owner_v3` under
`feature-isolation:quanta-runtime.no-default.7f44320823fb` executed
`persisted_required_data_survives_late_private_terminal_budget_refusal_v3`
(1 PASS, 0 ignored, exit 0). Required History DATA survives the later
private-terminal budget refusal. This is isolated S Runtime proof.

The modified `legacy_history_source_requires_exact_original_ack_and_complete_inventory`
case first expired twice at shared admission (180 s and 360 s, exit 124).
The same lane then admitted and **VERIFIED** the body in
`3eccd731-5e31-4d07-915b-f18d56f9d63b`.

The new exact selector is
`installed_native_history_full_then_delta_retains_generations_v1`. It shares
fixture setup and recovery with the existing Source-only required suite. It
checks the selected FULL and the successor before reattaching S, so a cold
readback failure cannot be covered up by another publish. The fixture requires
Q Latest to resolve to the exact combined Published view both before and
after restart. The ordinary Product roster matches the 37 tests in its
feature closure. History and Host recovery share the signed test-only
retained bundle. The [F retained execution](NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)
runs both exact selectors in the same guest and requires both raw receipts.
The History fixture
checks every History slot in the selected profile and rejects
unexpected generations or activation. The existing
cutover response relay interrupts selection reconciliation. Writer recovery
uses `HistoryPublishedBeforeTerminalAck` from the combined HistoryMain branch.
The Linux resident compiled with `native-retained-recovery-process-proof`
reports its PID, Q operation and actual V9 closeout and attempt IDs over the
fixture's private socket and exits before writing the V9 terminal. The fixture opens Q cold first, checks exact
retained and successor cuts without S replay, then reattaches S and requires the
same operation and unchanged Q Latest. This selector is excluded from the
ordinary installed test inventory. Rebuild the existing retained-recovery
private recipe with the current S/Q sources and run the exact History suite
in the signed Linux environment. The required environment includes the
pinned Q runtime, launcher and source custody.

The exact registry now declares `expected_ignored_names: []`.
Resolve-only returned one witness under
`quanta-cli-server.no-default.daa6083623eb`. The fixture also opens S's
completed cutover archive before the resident restarts. It decodes the first
and completion FD3 datagrams with the Q contract decoder and compares the
operation, view, instance, member and cut with the BOOT receipt and live Q
readback. After same-request reconciliation it requires the original Applied
terminal and a Reconciled History completion with the same History binding.
This assertion is part of the existing signed selector and remains
**NOT_RUN**. The later HistoryMain restart fixture checks
the S V9 checkpoint ACK against the aggregate terminal, reopens the bounded
HistoryMain CBOR receipt by its BLAKE3 identity, and compares its typed Q
operation and Published view with the same request's authenticated binding.
The scenario compares that operation and the persisted V9 closeout and attempt
IDs with the original fault event.

The Apply runner now retains the raw CLI output. A missing JSON response still
leads to a bounded wait for the exact resident fault event. Ordinary Rename
continues to require a typed successful response. The fixture creates its
fault socket under a dedicated `0700` directory and passes the selected Q
workspace and epoch. The hook fires only for HistoryMain in that namespace.

The second Legacy History cut now carries a distinct detached Git commit
under a History-only revision at the same generation ordinal. HEAD and the
selected Legacy Source stay fixed. Bootstrap sorts by generation and
revision. The History-only cut sorts last so the successor must select the
earlier cut that matches its actual Source predecessor. Each cut has its own
expected commit set. The Git CLI confirmed distinct object identity with the
same tree and unchanged HEAD and checkout.

The archived History batch has no Source manifest. Its fixture compares the
terminal ACK with the optional manifest value of the batch it actually
published. The signed original Source binding stays at the active cut. Rust
compilation and signed Linux are **NOT_RUN**.

The signed snapshot fixture first builds a separate candidate with one of the
two physical History generations omitted. It requires the producer's exact
inventory-mismatch error and the absence of a candidate selection and BOOT
receipt before proceeding with the complete target list. This installed case
is **NOT_RUN**.

### Already complete; do not recreate

- **H-INTEGRATE-02 — Ordered FULL for retained History generations (source supplied).** Q captures intermediate publications with the selected BOOT in one Catalog transaction, checks each immutable request and predecessor profile, and streams input to the Legacy census. S compares the complete ordered selection with the producer inventory under one fence. The candidate accepts `--native-candidate-history-targets-json`, appends one FULL per verified generation and reports the final operation. It rejects History operations on the live Source stream before contacting Q, so bootstrap cannot consume the stream used by the later Source DELTA. The Q component run `e50615f9-ce4a-4ea1-86cc-5ef8c1eeb76a` **VERIFIED** four selected cases. S compiler and installed proof remain pending. H producer and candidate codec, Q Kernel/BOOT and Catalog owners.
- **H-INTEGRATE-03 — Same-candidate retry after History commits (source supplied).** Resident and standalone preparation start from the attested AST operation and pass each checked predecessor to the next step. For an existing operation, S derives the retained replay or checkpoint input reference without an ingress and verifies the exact immutable request through idempotent Submit. Pending can Resume after verification. Published must return the same terminal view, including after Exact query view retirement. Linux retry execution is **NOT_RUN**. See the [ADR retry contract](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover). D owns candidate orchestration. H supplies exact input and request verification.

- The signed History fixture publishes through standalone installed S on the
  unselected launcher root and drains Q while retaining the terminal only in
  the test. The real launcher then restarts Q on the same root, invokes S, and
  returns the same root nonce, History operation, source cut and Original FULL
  sequence. The later launcher retry separately checks retained control-file
  reuse. The signed fixture is **NOT_RUN**.
- The direct candidate in the same signed fixture now reuses its published
  Source and AST to replay the first S-owned History generation. It compares
  the final operation and queried Git commit against the S publication and
  verifies the direct BOOT receipt. It then reopens the same Legacy
  SearchCorpus root to add the second, Q-only History generation. The existing
  two-generation launcher path still uses the physical checkpoint and
  verifies the complete Legacy census during restore-forward. The Q-only
  cut never had an S original. S's V3 retention cleanup is a separate
  lifecycle and remains unexercised by this fixture. Both signed paths remain
  **NOT_RUN**.
- The six installed candidate and recovery suites require Legacy Q executable
  and SHA inputs alongside the Native pair. The shared S fixture checks both
  hashes, starts and reopens SearchCorpus and RepoMap with the Legacy image,
  and uses the Native image for the candidate, BOOT and recovery helper. It
  checks each selection document's executable and SHA. Recovery installs
  manifest-backed Legacy bootstrap roots before publishing into them and
  reuses those same roots for History census and rollback. The existing
  single-ELF Product runner supplies its verified default-feature Q in both
  roles and validates those exact values in the retained receipt. F's separate
  dual-ELF witness requires distinct hashes. F's public and retained suite
  input checks and the focused Product runner tests (5 PASS) are **VERIFIED**.
  The empty Legacy root sequence was exercised on Darwin using a retained
  Legacy-enabled Q binary with SHA256
  `c85716969d33f6e41efcc279e666ad4891a2de5fdd3bc5b30c9173296e5ecadf`.
  It rejected a manifestless serving root under `verify-state`, then passed
  backup, restore, verification, launcher install and restart. The fresh
  Legacy binary build timed out before acquiring the shared build slot. The
  signed installation remains **NOT_RUN**.
- The existing Q IPC/SDK lane **VERIFIED** both libraries compiled and the
  three selected codec bodies passed in `8e44f333-d58c-4099-803a-ddbfe552ff1a`.
  The bodies cover the admitted single-frame wire, large multi-frame round
  trip and corruption, and the retained allocation boundary. The Linux
  pinned-peer Native UDS stage and installed History paths remain separate.
- The actual Linux ARM64 peer-bound UDS path **VERIFIED**
  `native_uds_bound_admitted_projection_stage_crosses_single_frame_with_child_ref`
  with 1 PASS / 78 filtered / exit 0. The first complete compiler attempt
  caught a Linux-only ignored atomic return value in the test, now fixed.
  The body exercised a retained peer and a real child-bearing Stage request
  exceeding the single-frame limit, and compared Q's installed object
  identity. This used the frozen Q sibling of S `971e7b66d93` with the single
  test fix applied in its Docker source copy. Signed S/Q installation is
  **NOT_RUN**.
- The Q `test-workspace-lane` run `4340d420-fc85-446b-8bfa-6a339a15739a`
  **VERIFIED** four bodies (4 PASS / 1991 skipped / exit 0). The tests cover the
  canonical stage-page frame bound, BOOT rejection for invalid snapshot
  identity and rows, existing-only immutable map scanning, and actual SDK/UDS
  FULL → DELTA → restart. The UDS case checks that the old Exact view is
  unavailable after retirement, identical FULL retry returns its recorded view,
  and changed input with the same key returns `DigestConflict` before and
  after restart. This run used Darwin. The signed installed S/Q path is
  **NOT_RUN**.
- The same Q SDK/UDS FULL→DELTA→restart case and
  `source_and_next_history_generation_publish_in_one_view_and_reopen` also
  **VERIFIED** on Linux ARM64 (2 PASS / 77 filtered / exit 0). Both exercised
  real Q publication and reopening. The Linux run used
  `scripts/cargow --lane test-workspace-lane test --locked --offline -p quanta-index-searchd-runtime --test native_index_uds`
  with the two exact `read_sessions::query::history` selectors in one process.
  The signed S/Q installation remains **NOT_RUN**.
- Q admitted Catalog publication-chain capture, exact Legacy original and row
  observation, fresh Source/root binding, and applied History journal ACK lookup
  **VERIFIED** in `e50615f9-ce4a-4ea1-86cc-5ef8c1eeb76a`, 4 PASS / 1045 skipped /
  exit0, on dirty Q `ade5fadc1`. The run does not cover installed candidate
  retention policy or the S Linux CLI.
- Q Native candidate restart, predecessor retention, and the existing BOOT
  and active retention process cases **VERIFIED** in
  `78eda16f-ce1a-478b-bd2a-667e5b4b6c8e`, 3 PASS / 55 skipped / exit0, on dirty
  Q `ade5fadc1`. Candidate mode preserves the operation chain, and regular
  serving applies the same configured automatic retention policy.
- Q paged snapshot FULL → Native append → restart and same-key retry, retained
  original replay, immutable page/ACK validation, independent Catalog checkpoint
  verification, and History fence identity **VERIFIED** in
  `e4c35a20-efc3-4698-bcae-e26f57f4eb49`, 5 PASS / 1371 skipped / exit0, on dirty
  Q `ade5fadc1`. This run covers the native UDS and component paths. Installed S
  producer export, checkpoint adoption and the signed subprocess remain **NOT_RUN**.
- Q shared immutable input identity **VERIFIED** in
  `a6f706a4-1900-4865-b8cc-fbde225fad96`, 3 PASS / 219 skipped / exit0. The
  Storage tests compare prediction with durable install, retain the physical
  child-existence check, and preserve the fixed `QIOBJ001` wire bytes.
- The combined Q build exited 101 at a concurrent `DiagnosticCounterV1`
  change. The next source observed the repaired enum and found a hardcoded
  Kernel test fixture array length, now removed. The next build exited 101 on
  an unused `Write` import in the borrowed exporter encoder. The repaired
  `test-workspace-lane` run includes SDK readout, FD7 startup, the combined CAS
  with derived outputs, paid request decoder, Catalog pager, exporter CLI and
  the real stale-selection process case. The malformed-FD7 case is Linux-only.
- `python3 -m pytest -q tools/ci/tests/test_deployment_launcher.py -k
  build_native_candidate_reaps_before_boot_and_reuses_durable_full` **VERIFIED**
  two cases with two Linux-only skips on Darwin. A later isolated Linux ARM64
  run with dependencies pinned by `uv.lock` **VERIFIED** six bodies
  (6 PASS / 141 deselected / exit 0), including the long procfd socket
  configurations, rejection of a listener owned by another PID, and BOOT
  refusal after interruption during drain. The tests use fixture child
  processes. The signed installed S/Q drain/export/FULL/restart remains
  **NOT_RUN**.
- Nextest `3fb00849-5762-4343-b0ff-3482c700ad47` executed seven bodies with
  four passes and three failures. The SDK admitted reader lacked fixed-size
  backing rules for the derived object references. The successor round-trip
  omitted profile admission after restart. The Legacy switch fixture omitted
  its three published Source handoff axes. All three corrections are in live Q.
  The expanded run that added installed witness wire coverage stopped before
  bodies on the existing HistoryCut fixture's raw array `ObjectRef` argument.
  The fixture now passes a typed `Digest`.

Representative Q commands for these receipts:

```sh
NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 -p quanta-index-searchd-runtime -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(legacy_history_recovery_observes_every_intermediate_publication) | test(legacy_history_source_requires_exact_original_ack_and_complete_inventory) | test(fresh_restore_candidate_requires_exact_boot_and_root_scoped_source) | test(admitted_terminal_read_uses_original_sequence_and_refuses_retired_floor)' --status-level fail --final-status-level fail
NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 -p quanta-index-searchd-runtime --test native_product_process --no-tests fail --no-fail-fast -E 'test(native_candidate_retains_predecessors_under_installed_retention_config) | test(native_boot_selection_verification_and_selected_serve_preserve_real_publication) | test(product_daemon_applies_retention_config_preserves_current_query_and_reopens)' --status-level fail --final-status-level fail
```

- **H-CLOSE-04 — production no-default after cfg repair.** **VERIFIED** compile, exit0 (`20261116T022818.404013Z-91bce8b69094`) on `feature-isolation:quanta-runtime.no-default.73ab9f2c701b` with `original-snapshot-v3,search-plane-sdk-ingress-product-proof`. Existing S production owning rail. This receipt covers compilation at its recorded source. The CLI and DATA owning requests are tracked above.
- Q producer-scope genesis and fresh restore/root binding: **VERIFIED** in
  `5dcc1052-7770-4fdf-a88e-59bba7799250`, 2 PASS / 659 skipped / exit0,
  bodies 4.927s, dirty main base `ade5fadc1a084fbb81eba831861e996081d80867`.
  The exact selectors were
  `legacy_history_source_requires_exact_original_ack_and_complete_inventory`
  and `fresh_restore_candidate_requires_exact_boot_and_root_scoped_source`.
  They reject foreign writer-state root, workspace root, contract key and a
  recomputed chain over a forged ACK, while preserving the fresh restore
  candidate's exact boot/root Source binding. This is Q focused component proof.
  Command:

  ```sh
  NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 \
    -p quanta-index-searchd-runtime --lib --no-tests fail --no-fail-fast \
    -E 'test(legacy_history_source_requires_exact_original_ack_and_complete_inventory) | test(fresh_restore_candidate_requires_exact_boot_and_root_scoped_source)' \
    --status-level fail --final-status-level fail
  ```

- H-CLOSE-02 and the Q bootstrap components: **VERIFIED** with
  `./scripts/cargow --lane test-workspace-lane nextest run` on
  `quanta-index-searchd-runtime --lib --test runtime_extended_suite`.
  Run `b143dd0c-58e5-487f-a53e-85dabeab1e84`: 4 PASS, 763 skipped, exit0.
  The selected cases cover FULL/DELTA, query and reopen, replay memory,
  the exact Legacy generation scan, and Legacy terminal/quiescence source
  byte preservation. This is Q component coverage.
- Q producer-inventory and recovery components: **VERIFIED** seven selected
  `cargow nextest` cases in run `c404948a-ab0c-4c5e-8384-14e563c48708`, exit0.
  That receipt predates the new producer-scope genesis check. The physical
  Legacy proof rejects missing inventory and a recomputed
  replay chain over a foreign ACK. The existing Native UDS case then ran with
  two actual Legacy SDK History publications, canonical replay positions,
  Native FULL and DELTA, and a query after restart. Run
  `ebf6f64e-6125-400e-8405-6fc4112cdd4c`, 1 PASS, 74 skipped, exit0. The
  Legacy fixture runs in its own process because each runtime mode installs
  a process-global SQLite VFS registration. Installed S/Q migration is a
  separate process qualification.
- Exact-generation durable readback and legacy publication/reopen source proof2:
  VERIFIED `14353f03-daf2-4e98-b0bf-219ce466f24a`.
- Q carrier/replay/scoring/planning library7: VERIFIED
  `500f188f-be51-4f62-8463-f47426e27221`.
- C83 actual Q SDK/ordinary CLI/UDS fixed-input FULL/DELTA/pages/reopen1:
  VERIFIED `8e36bbaa-3cef-40d8-959a-4f0b79302be0`; snippet gold repair closed.
- S authenticated per-publication History loan kernel fixture1: VERIFIED in the
  recorded owning run. This is independent of the actual caller mismatch above.
- Read-only snapshot authority and Native V5 RepoMap original-getter investigations:
  complete. Native RepoMap output implementation remains its existing owner;
  it is not a new H adapter task. ANN, SDK60, Tantivy/lock/vendor removal and
  broad Root installation work are not added to H's independent coding scope.

### Executed Q Runtime2 command · VERIFIED bodies2

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-searchd-runtime --test runtime_extended_suite \
  --no-tests fail --no-fail-fast \
  -E 'test(native_history_full_delta_query_old_view_and_reopen) | test(native_history_replay_memory_retires_superseded_rows)' \
  --test-threads 1 --status-level fail --final-status-level fail
```

Original limits and default features stayed unchanged. This user-requested run
completed and released its cooperative Q resource lease. Runtime leaf SHA256:
`99d00718c65a1d1868ee16bef714dcb640d652d4fea3185de76f5164a0e1a4a5`.
This re-audit used local source and recorded receipts, not external chat status.

## Recorded Q component bodies · library7, Runtime2 and C83 VERIFIED

Root's single admitted queue returned the actual library result:

- nextest run: `500f188f-be51-4f62-8463-f47426e27221`
- **7 run / 7 PASS / 1032 skipped / exit0**
- compile:25.79s; bodies:0.092s; terminal result collected and execution slot returned
- Scope: source carrier1, source replay2, pure scoring2, public planning2.
  Owning packages: `quanta-index-contract`, `quanta-index-search-plane`, `--lib`.
  The seven exact selectors are the non-Runtime entries in the request below.
- This supersedes the previous two compile101/body0 attempts; those were not
  body REDs. The 22 recorded source identities are historical and retained in the
  [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation);
  they are not a claim that today's shared source is still frozen.

### C83 initial fixed-input SDK/CLI/UDS fixture VERIFIED

Root collected the Text single-queue terminal result:

- nextest `8e36bbaa-3cef-40d8-959a-4f0b79302be0`
- **1 PASS / 74 skipped / exit0**, bodies:3.742s
- exact selector: `native_history_sdk_full_delta_retained_pages_and_reopen`
- Current feature boundary: all three History cases in this module start from a
  real Legacy SDK publication. Run them with `legacy-tantivy` enabled on the
  existing `native_index_uds` target. The Native-only configuration omits that
  module so its Text UDS cases compile with no Legacy dependencies. This
  feature-boundary change has no current History execution receipt.
- fixture `searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs`
  SHA256 `4213971c90321b0d816286ae714d460259dd1bab1217bdbe71332aefd7c5dfc2`
- scope: fixed original batch/ACK input through actual History port, Kernel CAS,
  public SDK, ordinary unprefixed CLI and UDS; FULL/DELTA, retained pages and reopen.
- The preceding actual C83 FAIL (`ca9417bf...`, 1 FAIL/74 skipped/exit100) was the
  fixture's added-text snippet expectation conflicting with the existing common
  touched→added→removed→header projection. C repaired only the exact fixture gold
  to literal equality plus actual-row diagnostic. H producer/SDK code and the
  existing converter were unchanged. No additional snippet RCA/patch/run is needed.

**VERIFIED:** standalone Runtime immutable storage/typed build/prior/FULL→DELTA/
reopen2 in `f366f01b-b802-471c-9b08-9196df56a1e2` above.
**NOT_RUN:** broader managed GC qualification and installed source snapshot
export/bootstrap. C83's fixture passes do not prove S's installed producer
or actual whole-History migration. The complete existing-History authenticated input is not supplied;
this does not establish data absence or forbid source-authority snapshot export.
Read-only findings and the narrower boundary are in the audit below.

## Implemented History contract — reuse

The existing source carrier, typed build/read ports, row map and query semantics
are consolidated in the [Native History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#native-history-publication-and-read-contract).
Library7, Runtime2 and C83 above supersede the H-API-07 compiler-retry staging.
Do not reopen carrier/codec/scoring/planner/SDK fixture wiring as new work.
Root owns shared profile/CAS/GC and Runtime mounts; H owns row/hash/adapter
semantics; C owns public wire/SDK/CLI leaves. The actual S caller, source-owned
snapshot export/high-water association and installed migration remain in the
active backlog and design below.

### Historical compiler repairs

The first two H library attempts were compile101/body0, not behavioral REDs.
The recorded repairs imported HistorySourceReplayV1, removed unused imports,
matched CborPayloadLengthProgressErrorV1::Codec, and bound/reused the validated
top_k in the existing reader/planner. API, error priority, limits and selector
sets did not change. Changed5 and changed2 rustfmt checks passed in their
recorded source scope; the later library7 receipt supersedes retry instructions.
No source freeze or runner reservation is issued by this historical note.

## Recorded Q verification scope

Library7 and Runtime2 executed the original nine selectors below in separate
owning runs; C83 has its own SDK/CLI/UDS fixture receipt above. This combined
request is historical, not a pending rerun or execution reservation. The runs
used shared HEAD `ade5fadc1a084fbb81eba831861e996081d80867` plus their recorded
dirty source. Runtime's default `legacy-tantivy` feature was retained; these H
leaves require no extra feature flag, and the extended suite has no
required-features gate. This is not a no-default qualification.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-contract -p quanta-index-search-plane -p quanta-index-searchd-runtime --lib --test runtime_extended_suite \
  --no-tests fail --no-fail-fast \
  -E 'test(history_source_carrier_keeps_originals_and_rejects_schema_drift) | test(complete_source_replay_keeps_moves_deletes_and_independent_row_digest) | test(source_replay_refuses_missing_duplicate_foreign_body_and_incomplete_boundary) | test(native_history_boolean_bm25_uses_only_matching_branches_and_complete_population) | test(native_history_relevance_refuses_unscored_alternatives_and_preserves_phrase_positions) | test(native_history_public_plan_reuses_canonical_lowering_and_rejects_wrong_kind) | test(native_history_public_plan_retains_original_byte_and_cancel_refusal) | test(native_history_full_delta_query_old_view_and_reopen) | test(native_history_replay_memory_retires_superseded_rows)' \
  --test-threads 1 --status-level fail --final-status-level fail
```

The carrier contract test uses independent fixed wire data and rejects missing,
unknown and duplicate schema fields. Runtime adapter test now exercises the actual
Core typed port for FULL and DELTA with distinct source/artifact refs, plus wrong
mode, full-as-delta, forged actual prior context, changed original body/ACK with
unchanged next cut, and generated artifact substituted as source. Existing fixed
BM25 gold, filter-before-top-k, both record populations, old view/reopen, ref move,
tag delete, cursor refusal and DATA release remain. Memory test replays 301 unique
publications with bounded final state under unchanged 512 MiB scope.

Recorded static checks on the nine H leaves: rustfmt, test-authority,
hexagonal and derive audit passed (violations0). Whole-workspace derive check
FAILED118 on its dirty cohort; H removed its initial serde derives and left
foreign findings intact. Expanded22 rustfmt failed on concurrent readiness mount
formatting; the narrower H9 check passed. The carrier test now checks all three
Map::remove results; the superseded carrier hash is retained in the recovery
entry. These static checks do not replace the separate body receipts.

Broader managed GC, source snapshot bootstrap, S actual producer caller and
installed migration remain outside the Q fixture results. Their required proof
is retained in the backlog and source-authority design below.

## Existing body evidence and outstanding producer scope

- **VERIFIED Q source readback:** nextest `14353f03-daf2-4e98-b0bf-219ce466f24a`,
  2/2 passed, 1124 skipped, exit0. Exact-generation WAL snapshot + real two-legacy-
  publication socket/reopen readback. Command:
  `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog -p quanta-index-searchd-runtime --lib --test runtime_extended_suite --no-tests fail --no-fail-fast -E 'test(history_scan_keeps_epoch_and_rows_in_one_wal_snapshot) | test(history_source_readback_binds_two_legacy_publications_after_reopen)' --test-threads 1 --status-level fail --final-status-level fail`.
- **VERIFIED S authenticated per-publication History loan:** selector
  `index_sdk_ingress::terminal_receipt_v1::tests::funded_history_loan_checks_receipt_before_visiting_original_batch_v3`,
  1/1 body with five refusal/lifetime branches. QBC run
  `20261114T153227.680089Z-d5d4ee2c38ae`, exit0, source
  `4fc2c406d5c6209271b70a06730f98a5612cde90fe846dcb3af6a1574b9fd4af`.
  `with_native_cutover_legacy_terminal_with_original_v1` supplies authenticated
  Required body/full ACK/source identity, or authenticated NotApplicable policy.
  Aggregate validation must succeed before callback output becomes authoritative.
- **Historical FAILED S Runtime compile / body0:** exec2595,
  run `20261114T161859.819264Z-957f1ef88e3a`, exit101, source
  `ea428519079703a545c23e5d86c361da8b8bb45011ab5c7666d463ae3420dc2b`,
  snapshot `97d4837547e91bf0e80078b67de184a5ad9a2017+ext-b2c982e760815b96eda5`,
  result `/private/tmp/history-loan-runtime-test-fixed-result.json`.
  Exact selector:
  `retrieval::port_impls::index_projection_writer::prepared_artifact_readback_v3::tests::terminal_integration_v3::persisted_required_data_survives_late_private_terminal_budget_refusal_v3`.
  Earliest foreign PTA E0502:
  `queries/pta/precise_query/extraction_solve/worker_cfg_dfg_file_batch_builder/original_core_file_birth_v4.rs:1702`
  (`final_body_v4` immutable borrow overlaps `final_uses_v3` closure mutation).
  H did not edit that owner hunk. Later attempts and the current exact selector
  state are reported in the active backlog; this receipt alone has zero bodies.
- S production no-default class
  `feature-isolation:quanta-runtime.no-default.73ab9f2c701b` previously FAILED6
  E0599 in `resolution_source_context_cache.rs`: PTA-off placeholder args invoked
  a PTA-only getter. H cfg-gated only that getter in the shared Original/cache-
  bypass predicate, preserving Original bypass. Static boundary preflight exit0
  (exec79514). The later exact production class compiled successfully as
  recorded in H-CLOSE-04. Parser E0283 prerequisite was repaired using explicit
  `S::Error::from(WireErrorV3::NonCanonicalEncoding)` and compiled97d.
- **Unverified actual migration input, narrower than a data-absence claim:** an
  actual preserved-root census and settled source-snapshot/replay binding have
  not been executed. It is incorrect to infer that no complete History data
  exists, or to require every genesis publication archive for every valid
  migration route. The H07 carrier is publication-replay based. The Q snapshot preparation helper
  exists, but no installed source-authority exporter/boot join is qualified. See the read-only audit
  below. A Native target scan still cannot issue its own expected source digest.

## Read-only authority audit · snapshot-first is a valid distinct route

The prior source audit selected retained replay and a source-authorized
snapshot checkpoint as valid routes. Both contracts are supplied in the
[History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover).
The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains its original source references and checkout.

### Retained V9/V5 and original body/ACK

Retained V9/V5 and an ACK alone establish no complete original-body
inventory. The producer validates complete receipt and Source association
before lending original data. Cleanup may retire that artifact. The
[exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains the original cleanup and decoder paths. Signed
production input completeness remains H-ACCEPT-01's acceptance.

### Authoritative Legacy snapshot bootstrap

The Legacy-owned WAL snapshot binds typed rows, epoch and applied ACK
under the writer fence. The Native checkpoint is checked against that held
source before selection. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) preserves limits on historical
cursor and overwritten event recovery. H's active backlog owns installed
qualification and wider Source/RepoMap routes.

## Bootstrap exporter and installed source binding

The authority boundaries found in the earlier audit still apply. The source
path below supplies the exporter and checkpoint join that were missing then;
the preceding preservation limits remain unchanged.

### Supplied source path

- The installed S coordinator retains the Host offline handoff, SDK committed
  Source/Original authority, and the exclusive History writer fence
  (`native_offline_cutover_v1.rs`). It passes the fence as FD7. The Q recovery
  child authenticates that inherited fence and holds the existing Legacy root
  lease (`native_recovery/custody.rs`). Host custody alone does not fence the
  SDK store, and the online auxiliary mutation lease does not cover this
  offline handoff.
- The admitted Q Catalog reader observes the exact generation's epoch, full
  physical row digest and latest **applied** History journal ACK in one
  quiescent Deferred transaction
  (`native/authority/read_only.rs:598..648`). The private exporter streams
  bounded pages and completion from that observation under the selected
  Legacy root. Page grouping accounts for the single-frame CBOR stage limit.
  The exporter also exposes the physical generation inventory and lends the
  actual terminal ACK.
- Before the first History ingress, the S snapshot candidate compares its
  complete selected target list with the exported physical inventory
  (`native_candidate_prepare_v1/history_snapshot_v1.rs:51..107`). For each
  generation it stages the pages and a checkpoint whose replay position binds
  the producer namespace, actual Legacy root, generation, epoch, digest and
  observed terminal ACK (`history_snapshot_v1.rs:385..495`). Retained originals
  use the separate authenticated replay route. Its peer-bound projection stage
  can use admitted IPC fragments when the envelope exceeds a single frame.
  Both routes compare the exact immutable stage ACK and attempt to discard the
  owned ingress on pre-Submit staging failure while the request budget is live.
- Q BOOT independently checks the ordered publications against the complete
  Legacy census under one held Catalog read, rederives the checkpoint replay
  commitment from the physical observation, verifies immutable pages and
  installed rows, and returns the selected History member and cut in the FD3
  terminal (`native_boot.rs:367..538`). S retains that terminal before
  restarting its producer. The selected Native target cannot issue its own
  expected source digest.

The original owner split still matters: Q's `LegacyReadAuthorityV1` supplies
existing-only physical read custody; S Host/CE owns the offline admission
seal; S SDK owns committed pointer and Original; Q Catalog owns row/journal
truth. The 30-second online AuxiliaryMutationCoordinator lease is not an
indefinite producer fence. The implemented FD7 handoff and root lease are the
fenced installed route; no parallel archive or fabricated replay cursor is
needed.

### Acceptance still open

The supplied source path has component coverage recorded above. The signed
installed S→Q drain, BOOT, restore-forward and subsequent real Source/History
DELTA across Q restart and S terminal recovery are **NOT_RUN**. The signed
fixture has a Q-only cut without an S original. No production root with
retired S originals was inventoried. H-INTEGRATE-01 and H-ACCEPT-01 own those
installed execution claims. The isolated DATA proof and CLI scope are
recorded in the active backlog. The current workflow starts from Legacy.
The previous-Native
restore-forward path rejects a retained History terminal and verifies a
Source/AST-only previous-publication chain. Replacing an already selected
Native root with History requires an authenticated previous-Native History
inventory and candidate carry-over proof. The snapshot's historical
preservation limits are stated above.

The [History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover)
contains the current producer, exporter, BOOT and successor contract. The
[active backlog](#current-actionable-backlog--2026-11-16-re-audit) owns the
remaining verification without reopening the historical issuer audit.

<a id="independent-native-v5-repomap-source-lookup--reader-supply-still-incomplete"></a>
### Independent Native V5 RepoMap source lookup — supplied

Native aggregate/replay and FULL/DELTA/first-DELTA CLI now supply the RepoMap
loan; Q's V5 repomap carrier and selected physical verifier consume it. The
old no-field/no-Native-loan finding is retired. The durable contract is in the
[receiver ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#recovery-receiver-and-selection-custody).
D/F retain owning, physical and installed acceptance. Qualification of the
supplied History snapshot bootstrap and active successor remains in
H-INTEGRATE-01, H-INTEGRATE-04 and H-ACCEPT-01.

## Recorded H source identities

The 22 full-file identities and superseded carrier identity are retained in the
[recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation).
They identify their original proof cohort, including concurrent owners' hunks;
they neither assign exclusive ownership nor attest current shared-main bytes.

## Retired section links

These fragments recover historical source/dispatch sections; current work follows
the active owner ledger above. Exact old bodies are in the [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation).

<a id="h-api-07-r2--validated-top-k-results-reused-frozen-for-root-retry"></a><!-- H-API-07-r2 · validated top-k results reused, frozen for Root retry -->
<a id="h-api-07-r1--compiler-correction-ready-frozen-for-root-retry"></a><!-- H-API-07-r1 · compiler correction ready, frozen for Root retry -->
<a id="h-api-07--source-ready-for-roots-single-compiletest-queue"></a><!-- H-API-07 · source ready for Root's single compile/test queue -->
<a id="canonical-contract-and-exact-mount-loan"></a><!-- Canonical contract and exact mount loan -->
<a id="typed-build-and-leaf-apis"></a><!-- Typed build and leaf APIs -->
<a id="implementation-boundaries"></a><!-- Implementation boundaries -->
<a id="verification-request--root-queue-only"></a><!-- Verification request · Root queue only -->
<a id="queue-admission-confirmation--2026-11-15"></a><!-- Queue admission confirmation · 2026-11-15 -->
<a id="current-h-source-identities"></a><!-- Current H source identities -->
