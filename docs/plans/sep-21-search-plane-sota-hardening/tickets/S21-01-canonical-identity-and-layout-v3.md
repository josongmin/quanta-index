# S21-01 — Canonical Identity and Durable Layout V3

Status: historical design record; current implementation and proof state must be read from source and `tools/ci/proof-authority.toml`.

Depends on: S21-00

## Goal

문자열 join 기반 persistence identity를 제거하고 logical/content/physical identity가 분리된 collision-free
layout을 도입한다.

## Initial audit root cause

`encode(repo)--encode(revision)`이 product identity를 filesystem filename으로 직접 투영한다. separator
escape와 tuple framing이 없어 다른 tuple이 같은 주소에 수렴한다. graph/cursor/receipt도 각자 identity를
축약한다.

## Target design

- `RepositoryRevisionIdentityV1`: normalized typed repo/revision tuple
- `ArtifactIdentityV1`: domain + repository revision + logical generation + artifact-set digest
- `CandidateCommitmentV1`: manifest, authority, content, schema/profile commitments
- canonical encoding: length-delimited or canonical CBOR bytes
- filesystem address: canonical bytes의 domain-separated digest + human-readable metadata는 payload 안에만 저장
- directory entry를 decode해 payload identity와 filename digest를 exact compare
- percent/separator escaping 같은 가변 문자열 convention 금지

## Ownership split

S21-01 closes in two sequential phases; P01A does not close this ticket.

- P01A: validated IDs, exact candidate/incident codecs and addresses, security-verification primitives, and complete
  closed `SearchPlaneErrorCodeV2` migration. It does not edit live `persistence.rs`/`store.rs`, publish quarantine
  files, delete activation paths or mutate catalog/runtime state.
- P03/S21-01B: live object layout and quarantine cutover together with S21-02. It consumes the P01A pure primitives
  and P02B global event authority. It owns filesystem mutation, catalog incidents and activation-path deletion.

## Original work items (recheck current source)

1. identity types와 fallible canonical digest owner를 contract-base/core에 추가
2. P01A에서 candidate/quarantine pure codec/address를 만들고 P03에서 live layout을 한 번에 전환한다.
3. filename/payload mismatch와 duplicate physical address를 typed corruption으로 분류
4. P03가 P02B sequence와 exact incident envelope를 catalog에 먼저 commit해 반복 격리가 evidence를 덮지 않게 함
   - observed-at, original path, size, payload digest, reason, incident sequence를 보존
5. path length와 directory fanout 정책 고정
6. P03가 old layout runtime reader를 제거한다. P10은 current-format state custody와 typed legacy refusal만 소유한다.
7. wire/persisted inventory와 format version 갱신

## Negative cases

- `("a--b", "c")` vs `("a", "b--c")`
- `%`, `/`, dot sequences exact roundtrip; NUL/control/mixed normalization refusal; logical text의 path projection 0
- 대소문자 민감/비민감 filesystem
- 최대 길이 repo/revision과 digest fanout
- payload identity와 filename digest 불일치
- 동일 physical file로 연결되는 symlink/hardlink 시도

## Owner files

- `crates/quanta-index-contract-base/src/ids.rs`
- `crates/quanta-index-contract-base/src/macros.rs`
- P01A: 신규 pure `crates/quanta-index-repomap/src/layout_v3.rs`
- P03: `crates/quanta-index-repomap/src/{persistence,model,store}.rs` 및 catalog/runtime quarantine owner
- `tools/ci/inventory/wire-surface.toml`

## Tests and proof

- owner-local property test: arbitrary tuple injectivity and roundtrip
- filesystem integration: case sensitivity, long path, concurrent distinct tuple writes
- frozen collision fixture import
- malformed filename/payload mismatch quarantine
- public API, wire inventory, fuzz smoke if DTO/decoder changes

## Acceptance

- product identity를 separator string으로 조합하는 production code 0
- arbitrary valid tuple pair가 같은 storage address에 alias되지 않음
- legacy root와 ambiguous old layout은 typed refusal; current-format collision은 deterministic conflict로 거부
- runtime dual-read/dual-write 0
- all persisted addresses are payload-verifiable
- quarantine evidence is append-only and a repeated basename cannot overwrite an earlier incident

## No patch-on-patch rule

기존 encoder에 `-` 하나만 추가 escape하거나 filename 앞에 version prefix만 붙이는 수정은 금지한다.
identity tuple framing과 payload verification을 같은 변경에서 끝낸다.

## Final implementation map

| File / symbol | Change | Purpose |
|---|---|---|
| `crates/quanta-index-contract-base/src/macros.rs::string_newtype` | raw/canonical representation과 fallible validation 경계를 분리 | decode 후 묵시적 normalization 제거 |
| `crates/quanta-index-contract-base/src/ids.rs::{RepoId,RevisionId}` | 허용 byte/Unicode/length 정책과 canonical encoder를 단일 owner로 구현 | wire, digest, storage의 동일 identity 보장 |
| P01A `layout_v3.rs` | exact candidate/incident codec, typed address and security primitives only | live mutation 없이 golden vectors 고정 |
| P03 `persistence.rs::{snapshot_file_name_for,activation_file_name,encode_component}` | separator/activation paths를 삭제하고 typed content address로 live cutover | tuple collision과 file activation authority 제거 |
| P03 `store.rs::RepoMapStoreKeyV1` / `model.rs` | logical key, candidate commitment, physical digest를 분리하고 exact envelope 보존 | same generation/different content 검출 |
| P03 catalog/quarantine owner | global event와 exact envelope commit 후 immutable projection/fsync/unlink | crash retry가 동일 sequence/time/evidence 재사용 |

### DoD additions

- canonical vector는 field order, integer width, Unicode form, domain tag를 고정하고 golden bytes/digest를 가진다.
- logical generation uniqueness는 DB `UNIQUE`로, physical bytes는 content address로 각각 강제한다.
- open은 `lstat/fstat`, regular-file, owner/mode, `nlink == 1`, payload/address digest를 검증한다.
- symlink/hardlink, case-folding, decomposed Unicode, 최대 길이 fixture가 typed refusal 또는 exact roundtrip을 증명한다.
- S21-01은 P03 proof가 live layout/quarantine targets를 닫기 전 `done`이 아니다.
