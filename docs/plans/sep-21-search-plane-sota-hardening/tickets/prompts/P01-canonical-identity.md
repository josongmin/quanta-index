# Copy/paste prompt — P01A Canonical Identity, Codec and Error Authority

당신은 S21-01 phase A owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`와 같은 디렉터리의
`README.md`를 읽고 그대로 적용한다. corrected P00 checkpoint, handoff, `p00-authority-freeze` manifest가 모두 current
source에 결속되고 P01A를 명시적으로 허용할 때만 시작한다. 하나라도 없으면 `BLOCKED`다.

repo instructions, FINAL-AUDIT, INDEX, S21-01, amended SEP-21-001/decision registry, P00 handoff를 전부 읽는다. 시작 시
exact HEAD/dirty digest/owner paths, P00 source-bound error-authority discovery inventory digest/semantic requirements와 canonical
golden-vector IDs를 freeze한다. final accepted-code table을 P00에서 가져오지 말고 이 lane에서 생성한다.
P00 artifact를 `just proof-error-authority-inventory`로 exact reproduction한 뒤 migration을 시작한다.

## 목표

logical/content/physical identity를 분리하는 pure contract/codec foundation과 closed error authority를 만든다. 이
lane은 live persistence를 바꾸지 않으며 S21-01 전체 closure가 아니다.

## owner scope

- `crates/quanta-index-contract-base/{Cargo.toml,src/macros.rs,src/ids.rs}`와 identity property tests
- `crates/quanta-index-contract/{Cargo.toml,src/ipc/error.rs,src/repomap.rs}`의 allocated canonical schema/error section
- `crates/quanta-index-core/src/{error.rs,domains/generation.rs}`
- query/ingest/control error converter, query repair/metrics classification
- `crates/quanta-index-sdk/src/{error.rs,client.rs}`
- production `CoreError::Typed` producer sites와 dynamic error-code helper. preflight에서 exact generated allowlist를
  만들고 8 producer crate의 path를 기록한다. allowlist 밖 owner 변경이 필요하면 패치 전에 `BLOCKED`다.
- 신규 pure `crates/quanta-index-repomap/src/layout_v3.rs`와 lib export
- Cargo manifests/lockfile, identity/error/layout owner tests, dedicated Just/profile/test-authority/proof registry delta
- `tools/ci/search-plane-error-code-table.schema.json` 규약에 따른 committed
  `tools/ci/inventory/search-plane-error-codes.json` producer/validator
- P01A 자체 public surface 변경으로 발생한 public API baseline/wire inventory는 이 lane이 같은 checkpoint에서
  갱신한다. P02I에는 P02A/P02B 후속 delta의 single-writer 경계만 handoff한다.

## 구현 순서

1. `RepoId`/`RevisionId`의 public unchecked construction과 serde bypass를 제거한다. 하나의 validator를
   constructor/TryFrom/FromStr/serde가 공유하고 decode 후 normalize하지 않는다.
2. `RepositoryRevisionIdentityV1`, `LogicalGenerationIdentityV1`, `ArtifactIdentityV1`를 amended ADR의 exact distinct
   domain framing으로 구현한다. 기존 `GenerationStorageKeyV1` duplicate authority는 delegate/remove한다.
3. `CandidateCommitmentV1`, `CandidateObjectDigestV1`, `CandidateObjectAddressV1`, canonical candidate envelope와 fixed
   fanout grammar를 pure type/codec/path function으로 구현한다. human identity는 path component가 아니다.
4. `QuarantineIncidentV1` codec/address 함수를 구현한다. caller-supplied positive state-root-global sequence가
   mandatory다. sequence 생성/저장과 filesystem publish는 하지 않는다.
5. lstat/no-follow/fstat, inode/device, regular-file, expected uid, exact mode, `nlink == 1`을 표현하는 immutable
   `StateRootSecurityContextV1`/verification primitive를 제공하되 live store에 wiring하지 않는다.
6. `SearchPlaneErrorCodeV2` closed enum/table을 구현한다. `ALL`, `as_wire_str`, exact `from_wire_str`, manual serde와
   wire-string uniqueness를 제공한다. 기존 `LexicalErrorCode`는 nested variant로 재사용하되 wire는 flat exact string이다.
7. `SearchPlaneIpcError.code`와 `CoreError::Typed.code`를 enum으로 바꾸고 producer → core → query/ingest/control → IPC →
   SDK를 한 번에 타입화한다. generic CoreError mapping은 단일 exhaustive owner로 모은다.
8. dynamic `format!` code, `&str` pass-through, code substring/string-equality classification을 lower-domain enum과
   exhaustive mapping으로 교체한다. boundary에서 string을 enum으로 parse하는 봉합은 금지한다.
9. stale/unknown `BAD_REQUEST` decode는 실패하도록 historical fixture를 전환한다. fuzz/public/wire fixtures를 갱신한다.

## 금지

- `persistence.rs`, `store.rs`, live `model.rs`, activation filename/codec, runtime boot/open, catalog schema 수정
- object/quarantine filesystem write, legacy reader 제거, filesystem activation 제거
- local/timestamp/random incident sequence, temporary hashed activation pointer
- separator escape 보강, version prefix만 추가, silent normalization, compatibility constructor
- `Unknown(String)`, `Other(String)`, free-form code wrapper, unknown/stale decoder success

## DoD/proof

- empty/over-limit/control/non-NFC refusal; `%`, `/`, dot sequence policy; case sensitivity; serde/constructor parity
- tuple injectivity/roundtrip, frozen separator collision, distinct repository/logical domains, exact golden bytes/digests
- fixed-length address/fanout and security primitive negative matrix
- sequence 0이면 incident encode 불가; same canonical envelope는 same incident address
- `SearchPlaneIpcError.code: String`, `CoreError::Typed.code: String`, production code `format!`, `&str` pass-through,
  substring/string-equality classification, unknown decoder success가 0
- enum `ALL` wire uniqueness, accepted-code cardinality/table digest, stale `BAD_REQUEST` negative fixture
- P00 discovery regex를 closure로 재사용하지 않는다. 이 lane이 `just proof-error-authority-closed`를 exact
  enum-source/table validator + exhaustive mapping/serde/SDK owner tests로 구현하고 성공시킨다.
- production RepoMap filesystem mutation 0; live cutover symbols는 의도적으로 P03에 남음
- `rust-public-api`, wire inventory, fuzz smoke와 owner-local tests가 same source를 가리킴

proof expected tuple은 `id=p01-canonical-identity`, `family=U`, `dependencies=[p00-authority-freeze]`다. 현재 staged row를 이 lane이 dedicated recipe와 non-empty
test-authority scope/targets로 교체하고 executable로 전환한다. generic `test-fast`나 compile-only를
OWNER_PROOF_GREEN으로 승격하지 않는다.

최종 보고에는 공통 status, exact source/dirty ownership, exact write set, identity/codec/error table, accepted-code
cardinality/digest, raw-string static search, golden vectors, command별 counts, NOT_RUN, 의도적으로 남긴 live-cutover
work, P02A의 `contract/src/repomap.rs` compiler DTO section과 P02B의 `contract/src/ipc/ingest.rs` journal DTO
section이라는 disjoint allocation, 두 lane이 소비할 frozen API와 `artifacts/sep-21/handoffs/P01.json`을 포함한다.
S21-01을 done으로 바꾸지
않는다. explicit owner path만 checkpoint commit하고 current lane branch에 non-force push한다.
