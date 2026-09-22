# RFC: Search configuration and profile application

- Status: `proposed` — design only; no runtime behavior is changed by this document.
- Date: 2026-09-23
- Source snapshot: `2a78af62d2684a64a63be2c96c36f52e0c5a39d7` (local HEAD at drafting). Concurrent worktree edits under runtime/test files were not part of this review. Recheck source and ownership before implementation.
- Owners: `quanta-index-searchd` composition/config, `quanta-index-search-plane` ingest/query, lexical and semantic storage adapters; deployment owns release configuration and corpus qualification.
- Prior decisions: [SEP-21-001 identity and digest domains](../../adr/SEP-21-001-canonical-identity-and-digest-domains.md), [SEP-21-003 read view and provider policy](../../adr/SEP-21-003-read-view-continuation-and-provider-policy.md), [SEP-21-004 supervision and cutover](../../adr/SEP-21-004-process-supervision-state-cutover-and-proof.md). Accepted ADRs remain authoritative.

## 1. Decision in one paragraph

Keep environment variables as the deployment input and resolve them once into a typed, immutable `SearchdConfig`. Give the already-committed lexical/semantic index contracts and current query algorithm one explicit, versioned *logical* profile name each. Show and validate the resulting configuration before boot; record a redacted effective policy digest for diagnosis. Index-affecting changes create a new sealed generation. Runtime resource changes take effect after restart. V1 does not add a configuration server, general YAML parser, live reload, per-request tuning, arbitrary ANN parameters, or a new persisted profile field merely to duplicate existing seals.

This RFC distinguishes *which setting was requested*, *which value was resolved*, *where it was applied*, and *which sealed generation a query actually read*. A successful config parse is not a successful daemon boot, index build, or quality qualification.

## 2. Current behavior and exact seams

1. The release binary parses `serve [--state-root PATH]` or offline state commands at [runtime process entry](../../../crates/quanta-index-searchd-runtime/src/bin/quanta-index-searchd.rs) and [CLI command](../../../crates/quanta-index-searchd/src/cli/command.rs). `--state-root` overrides only root selection; all policy families still resolve through [SearchdConfig::from_lookup](../../../crates/quanta-index-searchd/src/app/config.rs). Offline migration/backup/restore/verify commands do not boot a serving config.
2. `SearchdConfig` applies `ENV_POLICY_FAMILIES`, checks required history retention and the combined memory envelope, and later [runtime assembly](../../../crates/quanta-index-searchd/src/app/runtime.rs) validates provider/grant and wires query, control and ingest servers. Query admission, resource budgets, caches, writer bounds and socket policy already have typed owners. The config tests check family membership, both root entry points and non-default wiring.
3. [Semantic derivation mode](../../../crates/quanta-index-search-plane/src/semantic_derive.rs) is a separate `QUANTA_INDEX_SEMANTIC_DERIVE_MODE` read by [DirectSearchCorpusMaterializer::new_with_search_owned_semantics_from_env](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs) during runtime assembly. The default is `semantic_with_legacy_fallback`; it is absent from the `SearchdConfig` family chain.
4. The [lexical seal](../../../crates/quanta-index-lexical/src/sealed_generation/seal.rs) commits the normalizer version and artifact set. The [semantic generation contract](../../../crates/quanta-index-semantic/src/generation_contract.rs) commits model ID/revision, dimension, normalization, distance and corpus policy. The [vector-index seal](../../../crates/quanta-index-semantic/src/manifest.rs) records ANN build parameters, actual coverage and query effort. [Vector-index open/query](../../../crates/quanta-index-semantic/src/vector_index.rs) verifies and applies that sealed effort; exact generations bypass ANN.
5. [Hybrid fusion](../../../crates/quanta-index-core/src/domains/hybrid/service.rs) fixes RRF `k=60` and its candidate policy in code. [QueryReadViewV2](../../../crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs) already pins resource handles, artifact identities, normalizer and semantic model profile for a request. The public wire contract does not need another profile field for the first implementation.
6. [SDK `ClientProfile`](../../../crates/quanta-index-sdk/src/config.rs) means transport access (`Full`/`QueryOnly`). `just rust-profile` means build/test selection. Neither is a serving-quality profile.

### Consequences of the current layout

- Parser validation and provider/network or state-root readiness are different proof levels. A future `config check` must say exactly which level it covers.
- OpenAI's operator-supplied `QUANTA_INDEX_EMBED_MODEL_REVISION` is a namespace/rotation commitment, not independent proof that the remote provider kept model weights immutable.
- The running daemon composes one query/corpus embedding identity. After a model switch, an older generation built with a different model may be preserved on disk but its semantic query cannot be promised to work with the new process. No multi-model serving is implied by this RFC.
- Existing harness `config_digest` values describe their own benchmark parameters. They are not automatically a digest of the full effective daemon configuration.

## 3. Scope and setting classes

| Class | V1 examples | Owner and source | When changed | Required effect |
|---|---|---|---|---|
| Instance/runtime policy | socket paths/access; query slots/deadlines; writer, snapshot, regex, embedding-cache and stream budgets; retention; maintenance; provider retry/cost ceilings | existing `SearchdConfig` + env | startup only | validate composition, restart daemon; no generation rewrite solely for these |
| Ingest/content policy | semantic derivation mode; producer-owned source/render policy; lexical normalizer and text-authority format; embedding model/revision/dimension/normalization | `SearchdConfig` selects mode/provider; producer contracts and adapters own content truth | before a new build | new compatible generation; never reinterpret sealed bytes |
| Physical index policy | Tantivy schema/analysis; LanceDB ANN build recipe, library capability and coverage | adapter-owned code and existing seal | build/seal | new generation; delta inheritance only when its existing compatibility test accepts it |
| Query algorithm policy | current RRF and over-fetch; ANN effort from the opened generation's seal; exact/bounded outcome rules | core/search-plane/semantic adapter | process release; ANN effort selected by generation | one named built-in query profile; preserve request read pin and trace truth |
| Secret/authorization | `OPENAI_API_KEY`, egress grant, cursor key | secret source and existing grant/key owners | deployment restart or offline key workflow | never emit secret bytes in `show`, logs, digest inputs or benchmark artifacts |
| Dev/verification selection | `hash-dev`, `QUANTA_INDEX_ALLOW_DEV_EMBEDDER`, `just rust-profile` | existing dev/test front doors | explicit test invocation | cannot qualify learned production relevance |

The names `IndexProfileV1` and `QueryProfileV1` are *typed views of these contracts*, not a new tunable registry. V1 ships one named recipe of each. `index-v1` identifies the compiled build recipe; a deployment's *resolved* index policy also includes its selected model and producer content policy. The existing lexical/semantic seals attest what a particular generation actually contains. Do not infer that an old generation used `index-v1` merely because some fields look similar, or confuse its artifact digest with a recipe digest. The query profile names the current compiled fusion/planning behavior; its ANN subpolicy is the selected generation's sealed effort. A second named recipe requires a measured corpus and an explicit compatibility/release decision.

No `IndexProfileV1` string is added to the current persisted lexical or semantic format in V1. A standalone field would change the format and all read/write/migration consumers while duplicating existing commitments. If a future profile cannot be reconstructed from committed fields, first amend the relevant accepted ADR and version the persisted schema with an offline migration decision.

## 4. Desired end-to-end application flow

```text
release env + secret injection + optional --state-root
  -> SearchdCommand (serve vs offline command)
  -> one SearchdConfig::resolve(input) [defaults, parsing, dependency checks]
  -> redacted effective view + nonsecret policy digest
  -> config check (pure preflight) OR runtime assembly
  -> provider/grant + state-root/lease + adapter capability checks
  -> query/control/ingest servers under supervisor
  -> ingest: accepted producer contract -> build -> physical seal -> activation
  -> query: resolve active generation -> acquire QueryReadViewV2
            -> verify stored model/index contract -> execute sealed effort
            -> report pinned identity and actual outcome
```

### 4.1 Resolve and preflight

- Refactor only as needed so `serve`, `config check` and `config show` call the **same pure resolver**. The current `from_env`/`from_env_with_state_root` semantics, including `--state-root` precedence, remain unchanged. `config` is a distinct command branch; offline state commands remain isolated.
- `config check [--state-root PATH]` must parse every relevant env family, require the four history-retention limits, validate the memory envelope, semantic mode, embedder selection/revision, provider budget and complete egress grant when external egress is selected. It must not create a state root, bind a socket, call a provider, migrate state or claim existing artifacts are readable. Return a nonzero exit and named field on failure; success says `CONFIG_VALID`, not `READY`.
- `config show [--state-root PATH] --output json` uses the same resolver and emits schema version, profile names, effective nonsecret values and their `default|environment|cli` origins, digest and a `secret_status` such as `set|unset` where operationally useful. No `Debug` of `SearchdConfig`, raw environment dump, API key, cursor key or secret-derived digest. Human-readable output can be a separate renderer of the same typed view.
- Preserve existing fail-closed validation for known keys. Because `QUANTA_INDEX_*` also contains build, CI and benchmark variables, do **not** globally reject every unknown prefix member. The deploy wrapper or a future explicit daemon-only namespace may enforce strict names; V1 `config check` warns for plausible misspellings only when it can identify the daemon namespace without false positives. An unknown key must never silently become an accepted override.
- `effective_policy_digest_v1` hashes an explicitly enumerated, sorted map of resolved **nonsecret policy** values, including semantic derivation mode and selected built-in profile names. Use the accepted digest framing with domain `quanta-index/effective-policy/v1` and length-prefixed UTF-8 field name plus typed canonical value for each field; missing and present values are distinct. The field inventory is reviewed whenever a family changes; changing an *included* value must change the digest. Credential bytes, cursor key, potentially sensitive tenant/endpoint values, process-local paths and runtime state are excluded. The redacted view lists the excluded field names and reports authorization fields as `configured|missing` without values. The digest is a comparison aid, not authentication, a complete process identity, or proof of provider model immutability.

### 4.2 Boot and apply

- Resolve the config once before any state-root mutation. Pass `SemanticDerivationModeV1` explicitly from `SearchdConfig` into `DirectSearchCorpusMaterializer`; remove the production path that reads it inside search-plane. Keep an explicit constructor for tests. This is an ownership move, not an automatic switch from fallback to strict mode.
- Run the pure checks first, then the existing runtime checks: state format/lease, socket access, adapter open, provider grant, boot inventory and supervisor child startup. No preflight receipt substitutes for those checks. A child or adapter failure retains the existing startup rollback/exit contract.
- Use one resolved config instance for the embedder, cache identity, provider budget, ingest/stream window, query admission and maintenance. Do not reread environment variables downstream. The boot notice and `config show` report the effective policy digest, selected profile names and whether derivation is legacy/fallback/strict; never report secrets or add one metric label per arbitrary config value.

### 4.3 Ingest, seal and activation

- The producer remains owner of source/render truth. An ingest attempt first validates the producer contract, selected derivation mode and model/content compatibility **before** provider I/O or mutation. Existing accepted batching, idempotency and lifecycle fences remain authoritative.
- Runtime budget changes affect work admission, not source identity. Content, model, lexical analysis or ANN build-policy changes must build/seal a new generation. The seal continues to record concrete fields and actual backend attestations; the logical profile name cannot make an incompatible artifact acceptable.
- A delta may reuse its base only when the existing lexical/semantic/ANN compatibility rules say so. If the profile changes incompatibly, plan a full rebuild from the producer's authoritative source. Do not relabel or patch a sealed generation.
- Activation occurs only after the new generation is durable, verified and jointly ready for its declared query routes. `config show` never activates anything.

### 4.4 Query and result truth

- A request resolves its generation and acquires the existing read view before reading search data. The query path uses the opened generation's actual ANN effort and model contract. The query profile cannot override the seal, access an ambient latest index or silently substitute hash embeddings.
- V1 keeps RRF and over-fetch fixed. Expose their built-in profile name in process diagnostics and benchmark provenance. Existing read identity, dense effort and explain traces remain the per-request proof; no wire DTO addition is required merely to print a profile name.
- If a later experiment introduces a second query profile, validate it against the selected generation, bind it once per request and make it visible in response/explain and continuation context. That future wire/cursor change requires its own versioned contract review and SDK/CLI proof. It is not implicit V1 work.

## 5. Change and rollout matrix

| Operator change | Preflight | Cutover | Old generation / rollback boundary |
|---|---|---|---|
| Capacity, cache, deadlines or retention | `config check`, capacity sizing, restart smoke | restart with same root under existing lease/supervisor contract | no semantic rebuild; retention reduction can reclaim old generations, so check pin/rollback window before applying |
| Secret rotation, same provider/model identity | complete grant and credential availability; network call is separate proof | restart and opt-in real-provider smoke | effective policy digest can remain equal; record secret *version/reference* in deployment receipt outside public config output, never secret bytes |
| `semantic_with_legacy_fallback` → `semantic_only` | producer typed-source coverage and no fallback reliance on target corpus | stop accepting incompatible batches, deploy mode, build/verify new generation as needed | unchanged sealed generations remain immutable; a producer still sending legacy-only input is refused, not guessed into a typed source |
| Model ID/revision/dimension/content policy | provider grant + pinned source corpus + cost budget + judged quality evidence | drain ingest, preserve old root/config, build a separate candidate root or offline candidate, verify pair, then controlled binary/config/root cutover | current daemon serves one embedding identity; old-model semantic generations may be unreadable by the new process. Pre-write rollback means old binary/config/root; after new writes, rollback needs an explicitly verified replay/translation path, otherwise roll forward |
| Lexical normalizer or storage format | lexical compatibility/migration audit | new build and activation or offline root migration as format requires | current lexical open refuses mismatched normalizer/format; do not promise live mixed-version reads |
| ANN build recipe/library | exact-vs-ANN and incremental reuse measurements; backend capability proof | build and seal a new generation, verify manifest and dataset agree | old generation serves only if the new binary actually opens/verifies its recorded seal; library version text alone is not a refusal or compatibility proof |
| Future query profile | held-out relevance and tail-cost evidence | versioned release; generation compatibility validation | rollback to prior binary/profile only where its wire/cursor and stored generation contracts still hold |

For model/content cutover, a second state root or offline candidate is a **procedure to build and verify**, not a new dual-running product feature. The existing process lease means two daemons do not write one root. If no staging root, producer replay, or qualified rebuild is available, mark the production cutover blocked rather than claiming a zero-downtime switch.

## 6. Blast radius: concrete owners and work order

| Area | Required V1 touch | Contract/risk and proof |
|---|---|---|
| `quanta-index-searchd/src/app/config.rs` | add typed derivation mode, shared resolver and redacted effective view/digest; keep all env families in one chain | every input/default/CLI provenance and secret redaction; bad values fail before effects |
| `quanta-index-searchd/src/cli/command.rs` | add `config check/show` dispatch without entering offline-state or serve branches | `--state-root` same precedence; malformed command fails; no accidental serve |
| `quanta-index-searchd-runtime/src/bin/quanta-index-searchd.rs` and `src/lib.rs` | render config command, preserve existing process exit behavior | preflight is read-only and deterministic; serve still runs under supervisor |
| `quanta-index-searchd/src/app/runtime.rs` | pass resolved derivation mode to materializer; emit one redacted boot identity | one config object reaches provider, ingest and query; no downstream env read |
| `quanta-index-search-plane/src/semantic_derive.rs` and `ingest_dispatcher/search_corpus.rs` | remove product env reader; keep typed mode behavior and test constructor | current fallback default stays until producer migration is proved |
| Lexical adapter; semantic build/manifest/open | **no mandatory persisted schema change**; consume existing normalizer/model/ANN commitments as logical index profile | seal/read/delta compatibility remains source of truth; incompatible policy change forces new build |
| `quanta-index-core` hybrid and semantic vector-index query | **no algorithm knob in V1**; label the existing compiled query behavior | RRF, sealed effort and explain remain exact; no quality claim from naming a profile |
| SDK/IPC/wire/cursor | no V1 DTO change | if future per-request selection/response profile is added, review accepted ADR, version wire and update SDK/cursor binding together |
| Producer `semantica-codegraph-v2` | no automatic code change; deployment inventory for typed-source coverage, source/render digest and replayability | product owner must confirm `semantic_only` and model rebuild inputs; cross-repo proof where cutover requires it |
| Harness/bench and CI authority | carry effective policy/profile identity alongside existing scenario-specific `config_digest`; add owner and E2E scenarios to `test-authority.toml` if new test targets are introduced | distinguish config parse, boot, correct build, quality and performance evidence; no default threshold without corpus/SLO |
| Docs/deployment | env table with defaults, valid ranges, source, effect class, change action and rollback; secret injection/runbook | actual release configuration and binary/root identity captured in cutover receipt |

Implementation order: (1) inventory each consumed env name, default and application owner; (2) centralize derivation mode and pure resolution; (3) add check/show with redaction and digest; (4) bind diagnostics/benchmark provenance; (5) run the relevant daemon, storage and producer cutover proofs. Each step may land separately if its config path remains single-authority at that commit.

## 7. Validation and release evidence

Owner-local tests:

- One table-driven case per env family: unset, explicit default, valid override, zero/boundary, malformed/non-Unicode, dependent-pair failure. Both root entry points and `config check/show` resolve equivalent policy values.
- Deliberately mutate each resolved nonsecret value and assert the digest changes; secret bytes never occur in JSON/stderr or digest input. Identical resolved values from explicit/default env have the same policy digest; provenance is displayed separately.
- `config check/show` in an empty temporary root creates no files/sockets and never calls a provider. Pure check reports `CONFIG_VALID`; runtime error reports retain their existing exit semantics.
- Derivation mode applies identically through serve and explicit-root paths; downstream materializer has no production env read. Hash-dev remains explicit, OpenAI revision/grant remains mandatory and `unavailable` behavior remains truthfully reported.
- The built-in recipe name and each generation's concrete lexical/semantic seals are reported separately; actual model/normalizer/ANN effort matches those seals, and a contradictory seal or unsupported model is refused. Delta reuse and fresh rebuild use the same compatibility rule.

Integration scenarios:

1. Start the daemon with a constrained query/memory/provider config; observe accepted/refused requests and boot identity, restart with a changed capacity value, and verify the new limit while the sealed generation identity is unchanged.
2. Typed-source and legacy-only producer batches under each derivation mode: accepted, explicit degraded fallback or typed refusal as specified; no provider call or state mutation for a rejected batch.
3. Build/seal/restart/query a generation, inspect lexical/semantic profile evidence and ANN effort; tamper a committed field to prove fail-closed open.
4. Change model revision with the same model name and attempt to query an older semantic generation; prove the model gate refuses. Build a fresh generation, verify cache namespace separation and query success. Do not count a hash/stub provider as production relevance proof.
5. Rebuild with an ANN recipe change; verify old/new seal ownership and exact-vs-ANN quality on a qualified corpus. Check incremental reuse remains within the policy's compatibility/budget proof.
6. Rehearse a candidate-root cutover and pre-write rollback with old binary/config/root. Test the post-write replay path only if one is actually implemented and verified; otherwise record the roll-forward boundary.

Rust implementation verification uses the repository front door: owner-scoped `./scripts/cargow test -p <crate>` or registered `just rust-profile` scopes, then `just rust-profile test-integration`, `just rust-profile test-daemon`, and final `just rust-profile verify-rust` on a stable source. CLI/public-wire changes additionally run their owning CLI/public-API/fuzz rails. Record command, HEAD/dirty-source digest, selected/executed count, result and excluded surface; a dirty or older receipt is not exact-source release proof. Real-provider and judged relevance are opt-in owner-local qualifications with model, corpus, host and cost recorded; synthetic/hash runs prove mechanics only.

Release acceptance requires one reviewed redacted effective config, source-bound profile evidence for every activated generation, a completed candidate-root cutover receipt when an index-affecting profile changes, and measured relevance/latency/resource results for any *new* quality profile. Parser/boot green alone cannot approve a search-quality claim.

## 8. Deliberate limits and follow-on trigger

- V1 has one built-in index policy and one built-in query policy. It does not offer live mutable profiles, per-tenant profiles, cluster-wide config propagation, arbitrary field-level ANN settings, or multi-model serving in one daemon.
- A second profile is justified only by a fixed corpus and SLO showing a useful tradeoff, with a complete build/query compatibility rule and rollback route. `nprobes`, `ef` and refine effort affect recall and latency; changing them requires measured evidence rather than a free env knob.
- A versioned config file becomes useful only if environment management cannot express reviewed deployments. If added, it must feed the **same** typed resolver and declare explicit precedence; a second parser or hidden overlay is rejected.
- `config show` is an operator diagnostic, not a secret inventory or readiness endpoint. A deployment receipt separately records the secret reference/version and actual binary/root/source identity under its access controls.

## 9. External design references

- [Elasticsearch index settings](https://www.elastic.co/docs/reference/elasticsearch/index-settings): separates static index-creation settings from dynamic settings. This RFC applies the distinction to sealed generation versus restart-scoped process policy; it does not copy Elasticsearch's dynamic API.
- [OpenTelemetry configuration data model](https://opentelemetry.io/docs/specs/otel/configuration/data-model/): typed/schema-described resolved configuration and explicit parsing boundary motivate a single resolver and inspectable effective view.
- [Kubernetes ConfigMap](https://kubernetes.io/docs/concepts/configuration/configmap/): env-injected configuration requires process restart to observe updates, matching the V1 startup-only deployment contract.
