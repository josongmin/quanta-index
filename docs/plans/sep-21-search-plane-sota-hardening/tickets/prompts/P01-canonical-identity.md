# Copy/paste prompt — P01 Canonical Identity and Layout

당신은 S21-01 owner다. P00/M0가 DONE이고 canonical identity ADR/proof skeleton이 현재 source에 존재할 때만 시작한다.
없으면 우회하지 말고 `BLOCKED`와 missing artifact를 보고하라.

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
- `crates/quanta-index-repomap/src/persistence.rs::{snapshot_file_name_for,activation_file_name,encode_component}`
- `crates/quanta-index-repomap/src/store.rs::RepoMapStoreKeyV1`
- `crates/quanta-index-repomap/src/model.rs::RepoMapSnapshot`
- quarantine persistence owner, persisted/wire inventory, public API baselines

구현 순서:

1. ADR의 raw/canonical policy를 fallible type validation과 하나의 canonical tuple encoder로 구현한다.
2. field order, integer width, Unicode policy, domain tags가 고정된 golden bytes/digests를 만든다.
3. candidate object address를 domain-separated digest+fanout으로 변경하고 human identity는 payload에만 둔다.
4. logical generation key와 physical content digest를 별도 타입/constraint로 분리한다.
5. open에서 lstat/fstat, regular-file, uid/mode, `nlink == 1`, payload/address digest를 검증한다.
6. quarantine는 incident ID, observed-at, original path, size, payload/address digest, reason, sequence를 append-only 저장한다.
7. legacy parser는 offline importer 전용으로 격리하고 runtime dual-read/write를 제거한다.

금지: separator escape 보강, version prefix만 추가, silent normalization, filename을 payload identity보다 신뢰,
activation filesystem authority 재도입.

DoD/proof:

- tuple injectivity/roundtrip property 및 frozen collision fixture
- composed/decomposed Unicode, case folding, `%` `/` NUL, dot segment, max length/fanout
- symlink/hardlink/payload-address mismatch/duplicate physical address refusal
- repeated quarantine basename가 이전 evidence를 덮지 않음
- public API/wire inventory/owner tests가 같은 source를 가리킴

최종 보고에 DONE/BLOCKED, source freeze, 변경 파일/타입, format bump, 실행 proof counts, NOT_RUN, P02A/P02B에
제공할 frozen identity API를 포함하라. commit/push는 요청받은 경우에만 한다.
