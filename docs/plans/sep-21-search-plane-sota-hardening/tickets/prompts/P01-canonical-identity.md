# Copy/paste prompt — P01 Canonical Identity and Layout

당신은 S21-01 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P00 handoff와
`p00-authority-freeze` manifest가 현재 source에 결속될 때만 시작한다. 없으면 우회하지
말고 `BLOCKED`와 missing artifact를 보고하라.

repo instructions와 아래 문서를 전부 읽어라.

- `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/INDEX.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-01-canonical-identity-and-layout-v3.md`
- P00이 만든 identity/format ADR와 wire inventory

시작 시 exact HEAD/dirty digest/owner paths를 freeze하고 기존 dirty 파일과 충돌 여부를 먼저 보고하라.

목표: logical identity, content identity, physical storage address를 분리하고 separator-join collision과
filename authority를 제거한다.

owner files/symbols:

- `crates/quanta-index-contract-base/src/macros.rs::string_newtype`
- `crates/quanta-index-contract-base/src/ids.rs::{RepoId,RevisionId}`
- `crates/quanta-index-contract/src/ipc/error.rs::SearchPlaneErrorCodeV2`
- `crates/quanta-index-repomap/src/persistence.rs::{snapshot_file_name_for,activation_file_name,encode_component}`
- `crates/quanta-index-repomap/src/store.rs::RepoMapStoreKeyV1`
- `crates/quanta-index-repomap/src/model.rs::RepoMapSnapshot`
- `crates/quanta-index-repomap/src/persistence.rs` quarantine object layout/codec section
- integration owner에게 넘길 persisted/wire inventory와 public API baseline delta

구현 순서:

1. ADR의 raw/canonical policy를 fallible type validation과 하나의 canonical tuple encoder로 구현한다.
2. field order, integer width, Unicode policy, domain tags가 고정된 golden bytes/digests를 만든다.
3. P00이 고정한 exact path grammar와 fanout으로 candidate object address를 변경하고 human identity는 payload에만 둔다.
4. logical generation key와 physical content digest를 별도 타입으로 분리한다. catalog UNIQUE/CAS constraint는
   P03 owner에게 넘기고 이 lane에서 선점하지 않는다.
5. open에서 lstat/fstat, regular-file, uid/mode, `nlink == 1`, payload/address digest를 검증한다.
6. quarantine는 incident ID, observed-at, original path, size, payload/address digest, reason, sequence를 append-only 저장한다.
7. legacy parser는 offline importer 전용으로 격리하고 runtime dual-read/write를 제거한다.
8. closed `SearchPlaneErrorCodeV2`와 stable wire representation/exhaustive mapping을 구현한다.

금지: separator escape 보강, version prefix만 추가, silent normalization, filename을 payload identity보다 신뢰,
activation filesystem authority 재도입.

DoD/proof:

- tuple injectivity/roundtrip property 및 frozen collision fixture
- composed/decomposed Unicode, case folding, `%` `/` NUL, dot segment, max length/fanout
- symlink/hardlink/payload-address mismatch/duplicate physical address refusal
- repeated quarantine basename가 이전 evidence를 덮지 않음
- public API/wire inventory/owner tests가 같은 source를 가리킴
- unknown/stale error code와 free-form code success path 0

proof node는 `p01-canonical-identity`, canonical release command는 registry의 현재 값을 사용한다. 최종 보고에 공통
상태, source freeze, 변경 파일/타입, format bump, 실행 proof counts, NOT_RUN, P02A/P02B에 제공할 frozen identity
API와 `artifacts/sep-21/handoffs/P01.json`을 포함하라. explicit owner path만 checkpoint commit하고 push는 별도 요청 시에만 한다.
