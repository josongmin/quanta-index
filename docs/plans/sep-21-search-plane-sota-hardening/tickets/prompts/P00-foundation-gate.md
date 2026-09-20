# Copy/paste prompt — P00 Contract Repair and Foundation Re-freeze

당신은 quanta-index의 SEP-21 M0 contract/foundation gate owner다. repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`와 같은 디렉터리의
`README.md`를 먼저 읽고 그대로 적용한다.

현재 proof infrastructure가 GREEN이어도 semantic contract가 완전하다는 뜻은 아니다. 정적 감사에서 확인된 아래
결손을 먼저 닫고 P01A 허용 여부를 다시 판정한다. 기존 Accepted 의미를 바꾸거나 상세화할 때는 compatibility를
판정하고 superseding/amending ADR로 history를 보존한다. history 없이 Accepted 문서 의미를 덮어쓰지 않는다.

먼저 `AGENTS.md`, `AGENT_CORE.md`, `AGENT_PLAYBOOK.md`, `AGENT_RULE_CATALOG.md`와 다음 문서를 전부 읽는다.

- `docs/adr/SEP-21-001-canonical-identity-and-digest-domains.md`
- `docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md`
- `docs/adr/SEP-21-DECISION-REGISTRY.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/{FINAL-AUDIT,INDEX,ACTION-LIST}.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-00-authority-freeze-and-cutover-contract.md`
- S21-01, S21-02, S21-03, S21-04, S21-13 tickets와 P01A/P02A/P02B/P02I/P03 prompts

시작 시 full HEAD, branch/upstream/merge-base, tracked/untracked dirty paths와 deterministic dirty digest, owner path의
base blobs, 기존 `p00-authority-freeze` manifest/handoff digest를 기록한다. 기존 dirty 변경을 덮어쓰지 않는다.

## 목표

1. blocking decision을 구현자가 추측할 수 없는 exact byte/schema/ownership 수준으로 재동결한다.
2. P01A와 P03 사이의 live-layout/quarantine/activation ownership 충돌을 제거한다.
3. runtime error code authority를 closed typed set으로 만들 수 있는 완전한 migration scope를 고정한다.
4. proof registry를 실제 owner-local selector와 target에 결속한다.
5. corrected P00 proof/handoff가 current source에 결속될 때만 P01A를 허용한다.

## 반드시 동결할 canonical contract

- identifier는 UTF-8 already-NFC, case-sensitive, 1..=512 bytes다. NUL/C0/C1은 거부한다. `%`, `/`, dot sequence는
  valid logical bytes인지 거부 대상인지 명시하고 어떤 경우에도 path component로 사용하지 않는다.
- 모든 digest domain framing은 `u32_be(domain_utf8_len) || domain_utf8 || payload` 하나다.
- repository revision은 `quanta-index/repository-revision/v1`, logical generation은 별도
  `quanta-index/logical-generation/v1` domain을 사용한다.
- `ArtifactIdentityV1`, `CandidateCommitmentV1`, candidate envelope, `QuarantineIncidentV1`의 canonical CBOR integer
  key, key order, exact field type/nullability, digest input, golden bytes/digest를 표로 동결한다.
- digest 내부형은 32 bytes, 외부형은 lowercase `sha256:<64hex>`만 허용한다.
- candidate object path와 quarantine incident/payload path의 exact grammar, allowed extension, raw-byte digest
  verification 순서를 동결한다.
- incident sequence는 P02B의 state-root-global `catalog_sequence_v2` transaction에서만 공급한다. timestamp/random/local
  counter는 금지한다.

## 반드시 동결할 error-code authority

현재 `SearchPlaneIpcError.code: String`, `CoreError::Typed { code: String }`, dispatcher pass-through를 실제 source에서
재검증한다. production producer, transport, query/ingest/control converter, metrics/repair classification, SDK를 전수
inventory한다.

- runtime authority는 `SearchPlaneErrorCodeV2` closed enum/table이다. `ALL`, `as_wire_str`, exact
  `from_wire_str`, manual serde, wire-string uniqueness를 가진다.
- 기존 closed `LexicalErrorCode`는 복제하지 말고 nested variant + flat manual wire representation으로 합성한다.
- `Unknown(String)`, `Other(String)`, dynamic `format!` code, `&str` pass-through, substring/equality classification,
  unknown decoder success는 금지한다.
- generic `CoreError` → wire mapping은 단일 exhaustive owner를 가진다. SDK도 enum을 보존한다.
- stale `BAD_REQUEST` historical fixture는 V2 success compatibility가 아니라 typed unknown refusal fixture가 된다.
- mandatory-new-code 목록이 아니라 현재 reachable live code 전체와 후속 reserved code를 freeze한다. accepted-code
  cardinality와 canonical table digest를 proof artifact에 기록한다.

## lane ownership과 실행 graph

아래 graph 하나만 허용한다.

`P00 → P01A → (P02A ∥ P02B) → P02I → P03(S21-01B + S21-02) → P04 → … → P12`

- P01A: canonical IDs, candidate/quarantine codecs, closed error-code migration, pure address/security primitives.
  live `persistence.rs`/`store.rs`, activation/quarantine filesystem mutation은 금지한다.
- P02A: whole-bundle graph compiler. durable mutation 금지.
- P02B: `catalog_sequence_v2`, `catalog_sequence_event_v2`, operation journal. RepoMap filesystem 수정 금지.
- P02I: 두 lane same-HEAD integration과 shared contract/baseline/inventory single writer.
- P03: live layout/object store, quarantine projection, catalog candidate/activation/invalidation, filesystem activation
  authority 제거를 한 번에 수행한다.
- legacy `activations/` directory 변환/삭제는 P10 offline importer owner다. P03 runtime은 old root를 mutation 전에
  typed refusal하고 legacy bytes/inode/mtime를 변경하지 않는다.

## write scope

- SEP-21 ADR/decision registry
- S21-00/01/02/04/13 tickets, INDEX/ACTION-LIST/FINAL-AUDIT
- P00/P01A/P02A/P02B/P02I/P03 prompts와 common runbook
- `tools/ci/{proof-authority,test-authority}.toml`, wire inventory, proof schema/checker/writer/tests, Just recipes, CI wiring
- generated docs는 `tools/prompt-manager/sources/` owner 수정 후 sync만 허용

## proof authority DoD

- P01 proof는 dedicated recipe와 non-empty targets에 결속한다: canonical identity/codec, typed error authority,
  layout/security primitives. generic `test-fast`/compile-only는 OWNER_PROOF_GREEN이 아니다.
- P02B proof는 mixed event global uniqueness, transaction rollback, generic-ledger restore reconciliation을 선택한다.
- P03 proof는 live layout/quarantine/activation crash targets를 선택하고 S21-01B/S21-02 closure를 함께 판정한다.
- missing/stale/short-SHA/wrong-binary/zero-execution/ignored-only/unregistered proof가 validator와 blocking CI에서 실패한다.
- source digest는 staged+unstaged+scoped untracked bytes를 포함하고 proof output은 제외한다.
- registry 기반 writer만 terminal manifest를 만들며 자유입력 command/profile/target은 authority가 아니다.
- public/persisted format별 producer/consumer/version/decoder/migration fixture가 inventory에 있다.

## 금지

- unresolved decision을 TODO로 넘기고 P01A를 시작 가능하다고 표시
- P01A proof를 S21-01 live-layout closure로 표기
- local incident sequence, temporary hashed activation pointer, live dual reader/writer
- error-code compatibility string wrapper 또는 boundary-only parse
- unrelated product code 수정이나 product test 실행

이 lane은 계약/CI authority owner다. 문서/validator의 정적·Python owner test만 실행하고 Rust product test는 실행하지
않는다. proof node는 `p00-authority-freeze`다. current clean checkpoint에서 manifest와
`artifacts/sep-21/handoffs/P00.json`을 재생성·검증한다.

최종 보고에는 status, exact source/dirty ownership, amended decision 표, full error-code inventory cardinality/digest,
lane owner matrix, proof negative matrix, command/counts, NOT_RUN, handoff path/digest, P01A 허용/차단 근거를 포함한다.
explicit owner path만 checkpoint commit하고 push는 별도 요청 시에만 수행한다.
