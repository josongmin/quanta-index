# INC-02 · Native Literal의 변경 블록만 게시·검증

**생산 코드 구현.** 이전 packed root가 있는 Patch에서는 정렬된 변경 키로 `lower_bound_page`를 호출해 영향 블록을 선택한다. 같은 블록의 편집은 한 번에 병합한다. 조회한 페이지의 `has_more`가 마지막 블록을 식별한다. 새 키가 기존 최대 키를 넘으면 radix map의 가장 오른쪽 경로로 마지막 descriptor 하나를 읽고, 그 블록에 추가 키를 함께 채운다. fence가 뒤로 이동하면 이전 키 삭제를 새 키 삽입보다 먼저 출력해 저장소의 정렬 계약을 지킨다. 사용자 정의 `PersistentMapPort`로 Literal의 끝에 추가하려면 `last_entry`를 구현해야 한다. 기본 구현은 지원하지 않는 연산을 명시적으로 거절한다. 블록 첫 키도 같은 fence에 도달하는지 확인해 이전 블록과의 중첩을 검출한다. 중복 추가와 없는 posting 삭제는 기존 병합기가 거절한다. [경계 탐색](../../../../crates/quanta-index-lexical/src/native/literal_blocks.rs) · [게시와 seal](../../../../crates/quanta-index-lexical/src/native/literal_packed.rs)

봉인 시 candidate는 게시 단계에서 이미 예산을 받은 정렬 edit 배열을 소유한다. `visit_diff`를 마지막 페이지까지 읽으며 키와 이전·이후 참조를 직접 대조한다. 변경 없는 행은 예상 배열에서 건너뛴다. 변경 child는 설치 직후 다시 읽어 검증한다. 이 배열의 메모리 허가는 seal까지 candidate와 함께 이동한다. 공개 Rust 타입 `LiteralCandidate`는 `Debug`를 지원하고 `Clone`은 제거했다. 저장·전송 포맷 변경은 없다. 전체 구축, 변경분, 무변경의 세 증거는 `DirectoryProof`로 구분한다. cold 구축은 전체 디렉터리 해시를 확인한다. [저장소 diff 계약](../../../../crates/quanta-index-core/src/index/ports.rs) · [불변 서브트리 계약](../../../rfc/oct-10-native-incremental-vnext.md#84-integrity-model)

## 완료 판정

- **비용과 정확성.** 기존 `native_lifecycle`에 `literal_patch_does_not_walk_unaffected_directory_pages`를 추가했다. 같은 hot 파일 변경을 작은 디렉터리와 descriptor가 64개 넘는 디렉터리에서 적용하며, 이전 root와 실제 `apply_stream` 반환 root 양쪽의 디렉터리 조회에서 반환한 행 수를 센다. 같은 root 쌍의 `visit_diff` 호출이 있고 prepare·seal의 WorkBudget 읽기·쓰기가 정해진 범위인지 검사한다. Source 바이트에서 독립적으로 계산한 (3바이트 gram, owner) 집합을 두 세대의 실제 저장 블록 전체와 대조한다. 이 대조는 측정 구간 밖에서 수행한다. 이전·현재 pin 검색도 함께 확인한다. [Kernel 회귀](../../../../crates/quanta-index-kernel/tests/native_lifecycle.rs)
- **실패와 경계.** 2→3→2바이트 편집으로 빈 디렉터리의 첫 posting 추가와 마지막 posting 제거를 검사한다. 별도 본문은 가운데 posting을 유지하면서 양 끝에 추가하고 다시 삭제한다. 기존 seal fixture의 warm Patch에서 오류를 주입하고 게시 이전의 view와 operation 상태를 검사한다. Lexical 단위 회귀는 변경 없는 descriptor 처리와 잘못된 참조·추가·누락 거절을 확인한다. 구조적 diff의 늦은 실패는 저장소 회귀가 소유한다. [seal 회귀](../../../../crates/quanta-index-kernel/tests/native_bootstrap/literal_seal.rs) · [구조적 diff](../../../../crates/quanta-index-storage/src/map/tests/structural_diff.rs)

- **반복 추가.** 12개 owner에 같은 gram을 배치 한 번과 한 파일씩 12번 추가한다. 두 경로에서 저장된 posting 전체, 블록 하나, 실제 조회의 블록 읽기 1~2회를 검사한다. 저장소 회귀는 재개방한 4,098개 키 중 오른쪽 끝의 값을 검사하고 읽기 상한과 취소를 확인한다.
- **seal 페이지 경계.** 65개 이상 descriptor가 실제로 달라진 두 세대를 비교한다. 성공 시 마지막 키의 검색 결과를 확인하고, 두 번째 diff 페이지의 실패 시 이전 pin의 검색 결과와 게시 상태를 확인한다.

## 검증 결과 · 11월 21일

| 실행 범위 | 결과 |
| --- | --- |
| 현재 Cargo 통합 실행 | `./scripts/cargow --lane test-integration-lane nextest run`으로 Kernel·Lexical·Storage의 본문 11개를 실행했다. 실행 `547734ff-6354-4f1a-ac34-227e98aba8ec`에서 9개 통과, 2개 실패. 통과한 본문에는 12회 tail 추가와 배치 결과 대조, seek 취소와 읽기 상한, 대형 디렉터리의 한 파일 Patch, 정렬·경계·거절 회귀가 포함된다. 새 seal 테스트 2개는 초기 fixture에서 descriptor 65개를 만들지 못해 페이지 경계에 도달하기 전에 실패했다. |
| 다중 페이지 fixture 수리 | 8개 owner의 stable ID가 첫 바이트부터 다르게 입력을 고쳤다. 고정 입력에서 이전 전용 gram은 24,560개다. gram당 7개 후속 posting이 stable ID 16바이트씩을 기록하므로 삭제 대상 posting만으로도 2,750,720바이트, 32 KiB 블록 최소 84개다. 두 입력 모두 마지막 검색용 `ff ff ff`는 포함하지 않는다. 이는 인코딩과 입력에서 구한 하한이다. 수정한 두 본문의 정식 nextest는 공유 잠금에서 1,800초 후 exit 124, 실행 0건으로 끝났다(`NOT_RUN`). |
| 다중 페이지 정식 재실행 · 11월 22일 | **`VERIFIED` 2/2, exit 0.** 기존 `test-integration-lane`의 Kernel `native_bootstrap`에서 수정한 두 exact 본문을 실행했다. Nextest `1d3a5bb4-06c1-419e-9a65-f37e94ae1404`에서 마지막 변경까지 조회 성공(7.258초), 두 번째 페이지 거절 뒤 이전 뷰 유지(6.540초)를 확인했다. 실행 전후 Q HEAD `d8a490565`, Cargo.lock과 fixture·생산 입력 3파일의 SHA-256이 일치했다. |
| 다중 페이지 재확인 · 11월 23일 | Q `70d17e3cb` 작업 트리의 기존 `test-integration-lane`에서 같은 두 exact 본문을 실행했다. 마지막 변경 반영과 두 번째 페이지 거절 뒤 이전 뷰 유지를 **`VERIFIED` 2/2 · exit 0**으로 확인했다. Nextest `e09ac454-c3fa-4327-9c32-ac33d96fddc9`. |
| Kernel 통합 바이너리 | 11월 20일 Cargo가 생성한 Kernel 통합 바이너리에서 네 본문을 실행했다. 이 중 warm seal 거절·게시 방지, 2→3→2바이트 편집, 양 끝 posting 삽입·삭제가 각각 1/1 `VERIFIED`. |
| 대형 디렉터리 비용 회귀 | 네 번째 본문은 초기화에서 read lease 600초를 거절해 `FAILED`. fixture의 lease를 Core 상한인 300초로 고쳤다. 수정 후 정식 nextest는 공유 잠금을 3,600초 기다린 뒤 exit 124, 본문 0으로 끝났다. 이후 공유 잠금에 입장해 현재 `native_lifecycle.rs`를 원래 Cargo 빌드의 의존성 17개와 다시 컴파일하고 같은 exact 본문을 실행했다. **1/1 통과(3.08초)**. descriptor 64개 초과, 계측한 디렉터리 scan 반환 행과 scan 호출이 각각 8 이하, 큰 입력에서 adapter의 읽기·쓰기가 작은 입력보다 각각 64 이하로 증가한다는 단언과 이전·새 세대의 전체 posting 및 pin 검색 대조가 통과했다. 3.08초는 두 fixture의 구축·검증을 포함한다. 의존성은 11월 20일 빌드이고 이후 위의 정식 nextest에서도 이 본문이 통과했다. |
| Lexical 변경 디렉터리 검증 | 실제 Core와 동일한 `DirectoryDiff` 및 `changed_directory_witness_skips_noops_and_rejects_wrong_ref` 본문을 독립 컴파일해 1/1 통과했다. 정식 Lexical lib 컴파일은 Q DSL의 `DirtyRecord` import에서 exit 101로 끝났다. 공개 위치인 `crate::lex::DirtyRecord`로 고쳤다. 수정 후 정식 nextest가 입장한 실행에서는 contract와 Core를 컴파일하고 IPC에서 exit 101로 종료됐다(`FAILED`, libtest 본문 0). 복제한 빌드 캐시의 Ciborium 메타데이터에 `from_slice_with_definite_scalars`가 없다. 현재 vendor 코드에 구현과 공개 export가 모두 있다. 이 채팅의 격리 타깃에서 해당 의존성을 다시 빌드하려 했으나 잠금에서 300초 후 종료됐다(`BLOCKED`). 이후 위의 정식 nextest는 현행 의존성으로 Lexical 검증 본문까지 통과했다. [기존 원인 기록](../../../plans/nov-12-native-cutover/tickets/NC-H-history.md#L1557) |
| 정적·판독기 검사 | 변경한 Lexical 생산 파일의 `rustfmt --check`와 범위 내 `git diff --check` 통과. 테스트 판독기는 독립적으로 작성한 V2·V3 바이트와 잘린 입력으로 1/1 통과했다. |

최초 묶음 빌드에서 발견한 테스트의 미사용 import·반환값을 정리하고, harness 함수는 실제 호출자와 같은 `legacy-bench` 또는 테스트 구성에서 컴파일하도록 고쳤다. 비용 fixture의 WorkBudget, 제품 제한, 검증 상한은 유지했다.

**기존 저장소 경로.** 실제 `FileObjectStore::apply_stream`은 기존 root에 정렬된 변경을 적용한다. 설치할 때 이번 작업에 준비된 객체가 없는 참조에서는 순회를 끝낸다. 전수 순회 수리는 Literal의 `publish`·`seal`에서 끝낸다. [COW 적용](../../../../crates/quanta-index-storage/src/map.rs#L3009) · [승계 참조 처리](../../../../crates/quanta-index-storage/src/map.rs#L1403)

**유지할 경로.** cold Bootstrap과 Legacy의 첫 포맷 전환은 전체 구축이다. 동일 Source의 빈 Patch는 기존 artifact를 재사용하고, 본문 변경 뒤 posting 차이가 0이면 prior 블록 디렉터리를 재사용한다. posting을 제거하는 파일 삭제도 변경 블록으로 처리한다. [포맷·빈 Patch 분기](../../../../crates/quanta-index-lexical/src/native/literal.rs#L387) · [posting 차이 검사](../../../../crates/quanta-index-lexical/src/native/literal_packed.rs#L864)
