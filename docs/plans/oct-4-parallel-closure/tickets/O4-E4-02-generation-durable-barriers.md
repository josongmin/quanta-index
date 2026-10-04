# O4-E4-02 — generation durable publication의 그룹 barrier

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P1 / `CONDITIONAL_CODE` |
| 기준 웨이브 | [W2 — 확인된 결함 수리·선택 최적화](../waves/W2-repairs-and-selected-optimizations.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E4-01](O4-E4-01-index-phase-profile.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

실제 저장 비용이 확인된 경우 content durability와 root/manifest 공개 원자성을 유지하면서 반복 parent-directory barrier를 줄인다.

## 배경과 현재 상태

index_store.write_atomic_durable는 file sync→rename→parent sync다. coverage_pages는 새/기존/inherited hardlink page를 모은 뒤 root를 공개하고 orphan cleanup directory를 sync한다. file_authority.apply_plan은 source blobs 뒤 manifest를 공개한다. 과거182artifact syscall ABBA는 directory182→4에서 wall 감소를 보였지만 engine/crash proof가 아니다.

## 착수 입력

- 2026-10-04 current-source 정적 대조: canonical `write_atomic_durable`은 artifact별 file sync→rename→parent sync이며 coverage-page dir sync도 별도다. 현재 child clocks는 syscall 외 작업을 포함한다. 실제 release profile에서 sync가 지배한다는 결과는 `NOT_RUN`이므로 group barrier 변경을 아직 채택하지 않았다. 조건 미충족을 `NOT_APPLICABLE` 완료로 표시하지 않는다.

- E4-01 actual phase attribution 및 selected filesystem/Rust sync primitive
- write/file-sync/rename/hardlink/directory barrier/root publish/cleanup 단계별 fault injection fixture
- old/new root source commitments, independent reopen/fresh rebuild oracle

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-lexical/src/index_store.rs](../../../../crates/quanta-index-lexical/src/index_store.rs) | write_atomic_durable / write_atomic_durable_at | canonical writer 안에서 content staging/rename과 touched-directory barrier를 batch lifecycle로 표현한다. authenticated directory/no-follow semantics를 보존한다. | OWNED |
| [crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs](../../../../crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs) | write_coverage_pages | all admitted page writes·inherited hardlinks 완료→해당 dirs barrier→root write/sync/rename/barrier→old-page cleanup 순서를 지킨다. | OWNED |
| [crates/quanta-index-lexical/src/file_authority.rs](../../../../crates/quanta-index-lexical/src/file_authority.rs) | apply_plan | blob group durability 후 generation manifest를 공개한다. blob/source hash mismatch·orphan cleanup 실패를 전파한다. | OWNED |
| [crates/quanta-index-lexical/tests/sealed_manifest.rs](../../../../crates/quanta-index-lexical/tests/sealed_manifest.rs) | sealed artifact integrity | missing referenced page/blob·symlink·partial root·wrong digest refusal을 fixed fixture로 추가한다. | OWNED |
| [crates/quanta-index-lexical/tests/l2_file_mutation.rs](../../../../crates/quanta-index-lexical/tests/l2_file_mutation.rs) | lifecycle parity | fresh/delta/delete/no-op/reopen에서 independent source/coverage parity를 유지한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs) | runtime_extended_suite crash cuts | root publish 전후 process crash에서 valid old/new authority와 seal/activate 결과를 확인한다. | SHARED |

## 실행 단계

1. 실제 group당 touched directory와 old/new references를 열거하고 barrier count를 임의4회로 고정하지 않는다.
2. 최소 canonical batch primitive를 설계하고 single-file writer도 그 한 durable ordering을 사용하도록 정리한다.
3. content file full-sync와 rename, inherited hardlink dir durability를 보존한다.
4. root/manifest publish 및 post-publication barrier가 성공한 뒤에만 cleanup/seal/activate를 허용한다. old/new/live-reader/retained/rollback-required generation의 reference set을 독립 oracle로 검사해 여전히 참조된 source/page/hardlink를 지우지 않는다.
5. 각 syscall 실패·partial/missing source·symlink·root replacement·cleanup failure를 fault injection하고 reopen oracle로 판정한다.
6. matching engine release ingest A/B로 actual publish 효과를 측정한다. immutable pack은 별도 조건부 decision으로 남긴다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-lexical --test sealed_manifest --test l2_file_mutation --locked`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked e2e_crash_matrix`
- state/generation 변경 시 just rust-profile test-daemon + actual release crash-cut scenario.
- Positive: crash 후 old 또는 완전한 new root, 참조 file 존재/digest equality; Negative: barrier 실패 뒤 seal/activate, dangling hardlink/manifest, wrong inherited page 거절.

## 완료 조건

- durable ordering·error propagation·source custody가 independent fault/crash/lifecycle tests로 입증된다.
- actual engine publish 비용과 resource/output parity를 보고한다. 비용 조건이 성립하지 않으면 no-code NOT_APPLICABLE 가능.
- syscall fault와 OS-process kill/reopen은 그 failure model의 증거다. 실제 storage power-loss/flush guarantee를 측정하지 않았다면 power-loss 범위는 NOT_RUN으로 보존한다.

## 중단·거절·재개 조건

- sync 삭제·무시, tree path identity 완화, power-loss proof 없는 강한 durability claim으로 완료하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
