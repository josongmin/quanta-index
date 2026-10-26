> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-02-pending-recovery.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-02 — pending 저장/claim·startup Original 수용

상태·담당·다음 완료 조건은 [SSOT](README.md#w2-02)만 갱신한다. 외부 S 저장소의 실제 미연결 caller를 대조하는 계약이다. 과거 compile 실패와 채팅 실행 상태를 현재 source의 판정으로 복제하지 않는다.

## Contract

- Native 최초 carrier는 private borrowed field view로 plan·join·scope·handoff·locator를 빌리고 기존 admitted frozen owner·JSON encoder를 소비한다. ordinary DTO 생성·deep clone으로 Native carrier를 조립하지 않는다.
- SQL row는 `get_ref()`로 빌려 callback 안에서 Native root/JSON/manifest DATA·canonical 비교·열 축 검증을 완료한다. 기존 immutable insert/readback·ledger·Immediate transaction·CAS를 ordinary/Native가 공유한다.
- Native grant는 non-Clone admitted row와 Original backing을 함께 보유한다. Source 종료 뒤 순수 이동하며 bare record 분리를 거절한다. 최초 claim·restart takeover·takeover 직후 재진입에서도 원 carrier와 현재 custody revision을 보존한다.
- Native FULL ACK는 opaque `NativeInitialFullAckV3`로 admitted V9 row와 backing을 보유한다. receipt 검증은 scoped borrow로 수행한다. oracle 내부 reread도 admitted 경로여야 한다.
- 전수 비교는 새로 관측한 canonical SQL JSON 길이와 SQL identity 길이로 Original work를 먼저 청구한다. V9 carrier가 mutate/map되면 이 fresh-row 상한을 무효화한다. Native terminal oracle은 보존된 실제 artifact wire와 공유 admitted validator를 사용한다. Native 첫 FULL visibility 전·후 manifest 비교는 각각 fresh V9 row의 상한을 같은 Original callback 안에서 청구·실행한다. reader의 기존 pin/gate 엔진은 공통 comparator seam을 공유하며, 비교 Source 수용 뒤에만 pin을 visibility로 이동한다. cold ACK에 새 expected terminal receipt가 없어도 두 비교는 생략하지 않는다. 전환 전 Original 거절은 visibility를 변경하지 않는다. 전환 후 비교·마지막 checkpoint 거절은 기존 사전 할당 uncertainty 슬롯으로 원 typed cause와 publication-uncertain 분류를 보존한다. issuer receipt는 private 생성자에서 같은 불변 authority로 만들어졌다는 타입 불변식으로 반복 비교를 제거한다.
- startup은 discovery만 수행한다. schema22의 immutable `replay_native_admission_v3`는 새 ordinary=0·Native=1이며 기존 행은 NULL로 남긴다. Native·분류 불명 행은 JSON/artifact를 ordinary-load하거나 신규 Original을 발급하지 않고 실제 producer의 동일 operation/target 재진입까지 보존한다. 이전 ordinary 행도 분류를 추정하지 않으므로 자동 bootstrap 재개 대상에서 빠지며, exact producer 재진입이 필요하다.
- 기존 funded SDK handle→실제 RPC 연결은 기존 writer가 유지한다. Q receiver·issuer·serializer·ANN/물리/launcher 작업을 복제하지 않는다.

## Reuse boundary

Contract nested job/delta·taxonomy·semantic seed, ordinary/Native scalar validation/formatter·canonical wire, intent/checkpoint/terminal/ACK·reader pin과 admitted marker removal은 기존 구현을 소비한다. 현재 실제 연결 여부는 원 producer→consumer에서 재확인한다. 과거 FIELD/DTO 검증이 root JSON/row backing 수용을 대신하지 않는다.

- Native pending 출력은 `admitted_wire_v3`와 기존 `NativeProducerBirthV3::render_json_under_control_v3`를 사용한다. encoder 출력 backing은 같은 Original birth가 보유하며 ordinary 경로와 같은 immutable insert/fresh readback/ledger/Immediate transaction을 소비한다. Native encoder 거절 뒤 ordinary encoder로 fallback하지 않는다.
- Native manifest 검증은 `PendingNativeManifestDataV3`를 기존 검증 위치에서 지연 호출한다. 기존 charged request copier→admitted ordinal sort→fixed digest→같은 birth의 digest String→공유 Contract converter/coverage 순서를 사용한다. caller는 각 ctor/wire/store 검증마다 별도 DATA를 Source 밖에 두며, 완료 후 `finish_v3` 또는 retained failure를 회수한다. Original 실패가 우선이고, 부분 복사의 rollback은 값이 먼저 drop되고 기존 group의 charge는 마지막 group drop까지 유지된다.
- pending readback/claim의 canonical 비교는 기존 V9 `BorrowedByteMatchV1` encoder를 공유한다. 검증된 불변 field view를 비교 callback에 빌려주고 추가 전체 JSON String과 중복 outer validation을 만들지 않는다. semantic decode 오류와 canonical 불일치·row-axis 불일치의 순서를 유지한다.
- `PendingNativeReplayDecodeDataV3`는 기존 10필드와 두 locator의 외부 수신 DATA다. Contract plan-ID receiver와 기존 nested receiver를 소비한다. 같은 Original JSON Source helper를 V9와 공유하고 Source-active 검증→finisher→순수 이동, 실패 candidate/첫 원인 보존을 따른다. owner-support 회귀와 생산 SQL row/claim의 외부 DATA가 이 receiver를 공유한다. 동작 수용은 SSOT와 VALIDATION의 실제 실행 결과로만 판정한다.
- 첫 Native FULL 재시도는 기존 writer V9 root·Original/target·exact admitted marker reader와 공통 coordinator를 소비한다. previsibility phase와 Committed/Terminal, bound Dispatching만 재개한다. unbound Dispatching은 외부 stage 성공·binding CAS 전 중단 가능성이 있어 기존 Pending을 유지한다. query bootstrap에서 Original을 발급하지 않는다.

## Source and acceptance boundaries

S root: /Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates.

- quanta-runtime/src/indexing_machine_v2/store/pending_aggregate_replay_v2.rs: 최초 serialize·persist·claim/read/decode와 backing lifetime.
- quanta-runtime/src/indexing_machine_v2/store/{connection,native_replay_connection_v3}.rs 및 native_ast_work_v1/sql_transport_v1.rs: 기존 closed replay SQL transport와 signed child transaction/row 수신. Native branch의 `with_replay_sql_v3`를 ordinary writable reopen으로 대체하지 않는다.
- quanta-sqlite-native-reader/src/{protocol_v1,reader_v1,worker_v1}.rs 및 기존 patched SQLite Unix VFS: pending/V9/AST statement·물리 IO·mandatory cleanup 비용과 commit uncertainty. AST 전용 19개 protocol이라는 과거 설명으로 재개발하지 않는다.
- quanta-runtime/src/retrieval/assembly_data/source_bound_aggregate_coordinator_v2.rs: plan/handoff/frozen root 및 marker reconciliation.
- quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2.rs: inventory와 canonical persistence·공통 marker cleanup.
- quanta-runtime-retrieval-kernel/src/index_sdk_ingress의 facade/native_connect_v3/query/repomap/publish/control: funded handle과 실제 RPC consumer.

<a id="current-implementation-boundary"></a>
## Receiving custody boundary

아래 field는 caller/backing 인계 확인 경계다. 구현 존재와 생산 SQL row/claim의 동작 수용을 구분한다. `normalized_manifest_v1()`는 ordinary 분기에서만 사용한다. Native 분기는 위 lazy DATA와 기존 copier/정렬/digest/coverage owner를 소비하며 writer helper의 duplicate 선검사를 앞에 추가하지 않는다. Contract의 공유 converter는 후보 manifest를 외부 slot에 먼저 넣고 coverage를 검증하므로 semantic 실패에도 후보가 Source 종료까지 남는다. 최초 carrier는 borrowed field view·admitted frozen guard를, 첫 FULL Acked 반환은 opaque admitted custody를 소비한다. 재시작·oracle의 모든 내부 호출도 같은 계약을 따라야 한다. 이 표 전체가 미구현이라는 뜻은 아니다. 이전 CP/SQL/compile 결과는 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md#native-required-readback-code-checkpoint-2026-10-15)의 당시 source에만 적용한다.

## Native SQL mutation authority

Reuse the existing signed Native writer and shared pending/V9/AST transport. Their current implementation and owning results are recorded only in [SSOT](README.md#w2-02) and [VALIDATION](../../oct-10-index-closeout/VALIDATION.md). A Native carrier/decoder owner executed against ordinary `open_at_path_v1` is still component proof; it cannot qualify installed-worker writes or independent-process recovery.

- Reuse the canonical SQLite Unix pager/locking/transaction implementation and existing descriptor binding. The reader retains its read-only capability. A writable capability must authenticate the existing DB/directory and DB/WAL/journal/SHM identities; reopening an ordinary connection or removing query-only is not a substitute.
- Preserve distinct read-only and writable descriptor capabilities. The same Original must admit actual page writes, truncation, sync/lock/SHM/sidecar operations and physical resource bounds. Acquire/release costs include mandatory one-shot teardown; a refused Original cannot be made to authorize a new rollback command. First refusal, full SQL phase errors and uncertain commit/readback custody must survive the enclosing Source. Source presence alone is not syscall-fault acceptance; consume the owning fault result and its exact scope from VALIDATION.
- Use the existing closed worker transactions for `BEGIN IMMEDIATE`, predicates/CAS, fresh readback, commit/rollback and restart reconciliation. Do not create another persistence store, statement engine or arbitrary SQL/executable authority. Existing pending/V9 statements and facade calls are reuse inputs, not a new AST-to-pending rewrite task.
- Pending persist/claim/takeover/V9 and AST enqueue/claim/bind/read-bound/finish/refuse share this lower authority. Cut the transport and those actual consumers together; the admitted JSON/row/ACK DATA already implemented above remains the owner of payload/error/backing lifetime.
- Acceptance must open the actual Native connection and observe durable rows/CAS after an independent-process interruption. Keep ordinary SQL plus Native-cost owner tests as component proof only. Test the same operation before commit, after commit but before reply, and on rollback/refusal without rekeying or resetting the Original.
- The worker's selected SQLite synchronization policy defines the durability claim. Do not promote `synchronous=NORMAL`, a reader owner pass or local C compilation to power-loss durability or installed-release recovery.
- Direct C fault tests must prove their assertions are enabled after SQLite headers. Require an intentional failing-assertion control under the same flags. A run whose assertions were disabled by `NDEBUG` supplies no behavioral proof. Keep corrected harness acceptance separate from reader, signed-worker and actual SDK recovery scopes.

## Pending root receiving map

아래는 기존 root 필드의 확인 경계다. 현재 미구현 여부나 nested 수용 완료를 뜻하지 않는다. `ReplayNative*DecodeDataV3`는 기존 Contract의 외부 DATA receiver이며 별도 DTO/codec이 아니다.

| Canonical field | Reuse | Caller boundary to verify |
| --- | --- | --- |
| `schema_version` | inline u32·공유 validator·bounded Native 거절 | Native outer visitor의 동일 version/duplicate/missing 검사 |
| `pending_plan_v1` | `ReplayNativeIndexDeltaPlanDecodeDataV3`, 기존 job/delta nested seed | borrowed plan view·outer receiving |
| `pending_attempt_v2` | Runtime의 공유 scalar validator/serializer | 실제 문자열·private plan ID backing·partial DATA receiving |
| `pending_owner_fence_digest_v2` | caller의 admitted canonical material/hash·paid String | persist/claim의 새 backing과 그 수명 연결 |
| `source_join_v1` | `ReplayNativeAnalysisDeltaRefDecodeDataV3` | borrowed analysis-delta view·outer receiving |
| `product_scope_v1` | `ReplayNativeSearchProductScopeDecodeDataV3` | borrowed scope view·outer receiving |
| `prepared_handoff_set_v1` | `ReplayNativePreparedHandoffSetDecodeDataV3` | borrowed handoff view·outer receiving |
| `prepared_artifact_ref_v3` | `ReplayNativePreparedArtifactRefDecodeDataV3` | retained ref borrow·outer receiving |
| `frozen_request_v2` | `ReplayNativeFrozenRequestDecodeDataV3`, 기존 FULL의 admitted frozen producer | change-event pending의 admitted frozen guard 연결 |
| `replay_locator_v9` | Runtime의 공유 scalar validator/serializer | 실제 세 문자열 backing·partial DATA receiving |

JSON encode·row read·claim은 이 root 밖의 별도 backing 경계다. decoder/검증 오류와 실패 candidate도 최고 Source 밖 DATA에 남기고, 성공 DTO 이동 뒤에도 같은 Original group을 최종 drop까지 유지한다. manifest 정규화의 clone을 제거할 때도 기존 정렬 순서·digest domain·coverage 검증을 공유해야 한다.


## Source-loan partial retention

- Pending JSON output writes into exterior byte/allocator/Serde-cause slots. A serializer/count/fill refusal preserves the actual cause and partial backing until the highest Source finishes.
- Pending grant owner plan/workspace strings use the existing chunked String copier into exterior slots. The ordinary constructor and Native caller share one borrowed-slot validator; failed validation consumes neither string. Successful movement happens only after prepaid validation.
- Fence hashing reuses the same canonical encoder/hash through `CanonicalMemberEncodeDataV3`. Completed bytes, retired/partial encode storage and the derived hex String stay in caller DATA through Source completion; each payload drops before its Original lease. Neither wire nor digest domain changes.
- The installed Budget third option is wall-clock ms. Repository Admission work is unbounded by policy; this chosen rail cannot prove finite work-limit rejection. Refusal probes exercise genuine same-Root arithmetic exhaustion without changing policy, resetting the Root or increasing caps.

## Acceptance

- 기존 canonical bytes/digest·schema/duplicate/missing/foreign·오류 순서·Immediate SQL transaction을 보존한다.
- 요청/ref/frozen/target·claim fence와 원 typed cause를 끝까지 유지한다. admission 거절 전 allocation/write0와 최종 drop/refund를 독립 관측한다.
- genuine Original의 read/decode/work/lifetime으로 startup inactive-loader0·손상/foreign 격리·다음 정상 row 진행을 확인한다.
- 동일 operation의 persist/retry·ACK-loss·crash/restart takeover에서 재수집·budget reset·새 키 재송신으로 성공을 만들지 않는다.
- Production store facade가 transaction 거절 후 accepted readback의 record/grant 승격을 막고 원 typed cause를 보존하는지 확인한다. 실제 custody 행 수·revision이 그대로인지 SQL scalar로 확인한다. Fence serializer 부분 거절은 canonical DATA가 enclosing Source 종료와 producer/keeper 해제 이후에도 backing을 보유하며 마지막 DATA drop에서만 실제 ledger가 환급되는지 검사한다.
- opaque Native ACK의 scoped borrow·거절/cancel/unwind·최종 drop 환급을 실제 admitted custody로 검사한다. 기존 ordinary recovery E2E는 Native 독립-process 복구 수용을 대신하지 않는다. persist 전후·claim 후 V9 전·V9 후 ACK 유실·dispatch 후 binding 전 중단을 같은 operation/target으로 검사한다.
- 기존 replay integration fixture/owning QBC를 사용한다. 공용 Contract/COORD는 한 writer가 결합하고 W2-19는 원 결과를 소비한다.

## Process-proof routing

The existing [IFX150 integration target](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/tests/ifx150_restart_recovery_product_owner.rs) owns these five selectors; do not create another recovery target.

| Durable cut | Existing selector | Actual production owner | Required recovery oracle |
| --- | --- | --- | --- |
| Pending carrier persisted, before claim | `native_pending_carrier_recovers_after_persist_before_claim_v3` | `native_pending_transfer_v3` change-event branch | Same immutable carrier and exact latest custody; no new carrier or key |
| Initial claim returned, before V9 consume | `native_pending_carrier_recovers_after_initial_claim_before_v9_v3` | `native_pending_transfer_v3` change-event branch | Producer cut marker + same carrier/fence, revision0 and absent V9; retry takeover/CAS and stale-owner rejection are separate assertions |
| Q source stage sealed, before binding | `native_initial_full_keeps_unbound_dispatch_pending_after_stage_crash_v3` | Native dispatch pipeline | Unbound Dispatching remains Pending; no re-stage or fabricated ACK |
| Q Published, before terminal ACK | `native_initial_full_recovers_q_publication_before_terminal_ack_v3` | Native submit after Published/Exact View/Original checkpoint | Same operation/view/source; exact bound V9 resume |
| V9 Acked, before SDK return | `native_initial_full_recovers_acked_before_sdk_return_v3` | Native Acked CAS and admitted readback | Same admitted ACK, without another publication |

The same target also contains the Linux `native_initial_full_replays_same_target_after_sdk_process_exit_v3` baseline, which calls the helper with `None`. It is a separate restart baseline, not a sixth durable cut and not coverage for either Facts pending-transfer cut. Selector presence or Linux-only compilation is not an executed test result.

- The first-FULL inspector explicitly rejects change-event binding. Its process fixture cannot establish the first two pending-transfer cuts; the actual producer binding is the incremental change-event rail. The SQL owner fixture establishes store behavior only.
- The pending cuts belong to the existing Facts Native `apply_changes` → runtime-owned mutation handoff → incremental rail binding. The Python first-FULL fixture cannot prove them. The process fixture now shares the existing Java Facts bootstrap with owner tests, records the actual producer identity/fence at the existing fault hook, and uses a Native read-only kind7 inspector with the same canonical pending decoder. Initial claim revision zero is process-local: after writer termination the oracle checks the observed cut marker, exact carrier/fence and absent V9; it must not invent a durable claimed flag. Restart takeover revisions remain separate. This connection is code, not executed recovery acceptance.
- Fault callbacks are installed explicitly inside fresh test processes under the existing test-support feature. Production methods do not read environment variables or terminate processes. Hook calls follow the durable readback and highest Source completion.
- Cold SDK retry uses the existing combined API and authenticated durable exact-source baseline. Direct publish requires an in-memory RepoState. The fixture compares committed generation/snapshot/pointer/manifest and exact Q operation/view/source. It must fail when the baseline is absent. This avoids ambient source recollection; it does not prove zero Product reconstruction.
- Use the existing `quanta-sdk-runtime-executor` integration target `ifx150_restart_recovery_product_owner` and reviewed feature-isolation class `quanta-sdk-runtime-executor.default.2b760a3bf098` for process proof. The five Native cut selectors are Linux-only; macOS compilation or zero selected cases cannot satisfy them. Preserve the existing fixture build/binary binding and exact one-case child oracle.
- When building from a casectl frozen source inside Docker, host `tools/casectl/snapshot_runner.py lease-run --execution-root <root> -- <docker argv>` protects the source topology and external siblings for the child lifetime. Lease protection is separate from source-digest validation and QBC build authority. Keep original Git object and external source paths available; a container-only lease is insufficient to establish host-GC protection.
- Linux execution is available through the existing Docker substrate, including the cached amd64 proof image. Locate/reuse an existing worker and its manifest first; if absent, the existing QBC `quanta-sql-rulepack-worker.release.sql-rulepack-worker-build` recipe owns worker construction. Its default target is host-native, so execute it in Linux for the Native reader. The installed four-role bundle remains owned by `quanta-cli-product.release-full.linux-installed-build`; a locally calculated image hash is not installed-release qualification. Do not add another worker, build recipe or discovery authority.
- The located Linux substrate is Docker `linux/arm64` with the cached `linux/amd64` image `semantica-ht2-local-linux-proof:rust-1.92-python-3.12`, image ID `sha256:d7ee9db9dbd598e6bd7d2a5d7112d6cec0e8c4cb002ce7b759847bf0b1ec6aec`. Its non-login shell resolves `/usr/local/cargo/bin/cargo`, Rust 1.92.0 and Python 3.12.15. The existing AG5 installer selects `<install_root>/releases/<release_manifest_sha256>/` as the bundle. The canonical installed worker location is `<bundle>/libexec/quanta/quanta-sql-rulepack-worker`; its release manifest is `<bundle>/share/quanta/release-manifest.json`. Discover actual installed provenance, or build the existing worker recipe for a separately scoped image diagnostic; never invent a signed expected digest from local hashing.
- Native execution diagnostics use Docker image `semantica-qbc-native-linux:20261019-pidfd`, ID `sha256:f97e08dda3509288eb25be08124b62cc2e37b5747d7023d696589d80fac340e0` (`linux/arm64`, Rust 1.92.0, official CPython 3.12.15). Actual `os.pidfd_open` succeeds and direct sealed/CLOEXEC `memfd` + `execveat(AT_EMPTY_PATH)` exits 0 there; the prior standalone CPython lacked `pidfd_open` and could not run QBC process custody; the amd64/Rosetta environment returns `ENOENT`. The arm64 image establishes an execution substrate only, not the x86_64 installed-release contract. Existing QBC worker construction uses the exact cached QMP/taskmesh source and the real task ID for admission; no signed worker digest has been discovered.
- Cold readback borrows the provider's actual installed worker configuration. Native backend selection occurs before the Source; query failures cannot switch to an ordinary decoder. Checkpoint row, partial backing and typed failure remain in exterior DATA until the highest Source finishes.
- Bound nonterminal resume opens an existing current-schema writable store only. The canonical coordinator admits and compares the complete exact row under the same Original before its existing Immediate/CAS recovery; reopening must not create a directory/database, migrate schema or issue an epoch.
- Before recursive post-resume inspection, release the old row and reader DATA, including the old signed worker ceiling. Reinspection uses the same Original and unchanged policy, rather than overlapping two workers or resetting the budget.
- An exited process cannot retain its in-memory Original keeper. A retry must obtain the normal authenticated Original from the same committed source under the unchanged budget policy. A new operation, replacement source, raised cap or retry-local budget reset cannot be used to force acceptance.
- Native process admission remains conditional on the actual SDK Original initial persistence/inventory/epoch import projection owners. Their explicit unfunded/ordinary-access rejection is a real upstream boundary, not permission to substitute ordinary data.

<a id="terminal-publication-join"></a>
## Implementation packet

아래는 고정 실행 계약이며 상태는 [SSOT W2-02](README.md#w2-02) 한 곳에서만 갱신한다. [W2-05 C](W2-05-producer-full-delta.md#native-publication-caller)의 실제 게시 artifact가 입력이다. 기존 SQL51·signed Store·borrowed JSON/row receiver·claim/takeover·opaque ACK와 SDK 소비2곳을 재구현하지 않는다.

1. **Runtime artifact→pending/V9:** Root가 기존 `native_delta_v3/consumer_v3`의 실제 완료 artifact·same operation/target/Original·retained source를 기존 pending/coordinator에 전달한다. `indexing_machine_v2/store/pending_aggregate_replay_v2.rs`·`retrieval/assembly_data/source_bound_aggregate_coordinator_v2.rs`의 lower serializer/SQL/CAS는 I 인계를 재사용한다. 성공/거절/취소/abort/uncertain 모두 whole owner와 원 typed cause를 이동하고 staged/selected/prepared를 published로 승격하지 않는다.
2. **ACK→SDK terminal:** PUB는 기존 `native_initial_full_replay_v1.rs::NativeInitialFullAckV3`의 scoped receipt borrow와 `indexing_api.rs`의 두 `into_published_v3` 소비자를 실제 성공 artifact에 결합한다. Root가 제공하는 오류/결과와 같은 public hunk에서 인수를 맞춘다. bare record 분리·receipt deep clone·funded handle의 generic Deref 복구·별도 decoder/RPC 엔진은 추가하지 않는다.
3. **동일 operation 재개:** 최초 claim·restart takeover·stale owner/epoch/revision 거절·bound exact resume를 같은 Store/target/policy로 검증한다. unbound Dispatching은 Pending을 보존해 중복 stage/새 ACK를 만들지 않는다. discovery는 새 per-operation Original 발급이나 Native artifact의 Ordinary load를 하지 않는다.
4. **owner 회귀:** canonical wire/digest·역순 manifest·duplicate/missing/unknown/손상/foreign/trailing JSON, allocation/work 거절 후 write/RPC0, 마지막 payload/DATA→backing drop 환급을 기존 oracle에 결합한다. initial claim과 durable takeover revision은 다른 assertion이다. 모듈/반환형 compile만으로 본문 실행을 인정하지 않는다.
5. **제품 복구:** 기존 [Process-proof routing](#process-proof-routing)의 기존 IFX150 Linux target·five cut(first-FULL3 + Facts2)을 그대로 소비한다. 동일 operation/view/source·old/new SourceCut·actual terminal receipt와 원 typed cause를 검사한다. 재수집·새 키·예산 reset·가짜 Acked SQL로 성공시키지 않는다. 설치 signed worker 기대 digest/provenance는 기존 설치 manifest/제품 recipe에서 얻고 로컬 hash를 authority로 발급하지 않는다. 누락은 해당 process gate만 BLOCKED로 남기며 다른 code/owner 검증을 막지 않는다.

완료 반환물은 실제 caller trace·변경한 exact hunk·owner compile/본문 결과와 제품 cut 결과다. 기록은 기존 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md), 집계는 [W2-19](W2-19-integration-acceptance.md#combined-checkpoints)가 소유한다. 새로운 ticket/runner/결과 원장을 만들지 않는다.

## Historical section links

아래는 이전 deep link의 호환 anchor다. 원문은 [보존 이력](../../../oct-17-index-closeout/tickets/W2-02-pending-recovery.md)에서만 확인하며 현재 할 일을 추가하지 않는다.

<a id="w2-02--pending-저장claimstartup-inventory의-native-소유권"></a>
