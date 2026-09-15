# 구조 개선 구현 ledger

> 이 문서는 진행 증거다. green test 없이 finding을 `passed`로 올리지 않는다.
> 상태 어휘: `planned | in_progress | implemented_not_run | passed | failed | blocked | not_applicable`

## 0. Frozen base

| 항목 | 값 |
| --- | --- |
| 감사 HEAD | `4914156f4191daa3e12998bdb38f2b821a057fdd` |
| 시작 시 확인한 HEAD | `4914156f4191daa3e12998bdb38f2b821a057fdd` (동일) |
| branch | `main` |
| dirty product source | 없음 |
| user-owned untracked | `docs/bugbash/` (이 캠페인의 입력 authority) |
| host | darwin 24.6.0, macOS |
| cargo target root | `~/Library/Caches/quanta-index/target` (warm, 21G) |

### 입력 문서 digest

실행 프롬프트 §2가 요구하는 입력 고정. `shasum -a 256`.

| 파일 | sha256 |
| --- | --- |
| `docs/bugbash/sep-16/findings.md` | `8baff0adf9ee0ef5a367171174c2c63aa76090f9175c1a9d8173414361137da1` |
| `docs/bugbash/sep-16/structural-remediation-plan.md` | `03286f8277b4d1d7163a1215a26250dc53489a011e23f85a140dece2d5b405f1` |
| `docs/bugbash/sep-16/test-plan.md` | `423ebb97e13a9c1ab1a20b047e55b27ea2dd1a297f5f52ff392e5088c01a2abf` |
| `docs/bugbash/sep-16/implementation-agent-prompt.md` | `10c79e927b5cdf9f890abcfa64321c55bc172c595c2ed9a6519b434c2444c295` |
| `tools/ci/test-authority.toml` | `72c9d40138666a33327421d4161b0c68059912144026b6c815b44bcbd213154c` |
| `Justfile` | `f215db9d089917c5e70e3c77569f8781f97cd60386eae28a9702bdb7adc70585` |


## 0.1 실행 환경 사실 (세션 중 관측)

이 항목들은 계획 문서에 없던 환경 사실이며 판정에 영향을 준다.

| 관측 | 근거 | 영향 |
| --- | --- | --- |
| repo에 auto-commit이 동작한다 | `git reflog`에 세션 중 `526349b "done"`(04:38), `7e50b18 "done"`(04:48). author는 user, 내가 실행하지 않음 | M2의 "복구 지점 없음" 우려는 해소. 대신 partial state도 자동 commit되므로 tree를 항상 compile 가능 상태로 유지해야 한다. 이후 landing state는 의미 있는 메시지로 직접 commit한다 |
| 다른 Claude 세션이 같은 repo에서 동시 작업 중 | `ps`에 다수 `claude` 프로세스 + `bash ./scripts/cargow test` 실행 중. 내가 만지지 않은 `README.md`, `docs/ssot/*`, `docs/plans/*`, `docs/bugbash/sep-16/findings.md`가 세션 중 수정됨 (QI-BB-012 doc sync lane) | **QI-BB-012 / doc lane은 다른 소유자**다. 나는 `README.md`, `docs/ssot/`, `docs/plans/`를 건드리지 않는다. `just lint-doc-paths`는 그쪽 작업으로 이미 green이 되었다 |
| sibling repo(`semantica-codegraph-v2`) 빌드도 동시 실행 중 | `ps`에 `quanta_build_cli.py cargo`, 다수 `cargo test -p` | **모든 latency/throughput 측정이 오염된다.** W0의 timing gate(G0-C commit p95, warm/cold, scale)는 quiet host에서 재측정해야 한다. 이번 세션은 **기능 truth**만 확정하고 timing은 `blocked: contended-host`로 남긴다 |


## 0.2 캠페인 중 발견한 사전 파손 (QI-BB-001–032 밖)

| ID | 등급 | 내용 | 상태 |
| --- | --- | --- | --- |
| IMPL-A | P1 (repo gate) | `just rust-clippy`(= CI `ci.yml` job `rust-clippy`, `clippy --workspace --all-targets --all-features -- -D warnings`)가 감사 HEAD에서 이미 RED. main이 CI 실패 상태 | fixed, 검증 중 |
| IMPL-B | **P1 (silent data loss, 확정)** | sidecar authority가 delta보다 먼저 publish되면 delta generation이 base를 통째로 잃는다 | **fixed + regression green** |
| IMPL-C | **P1 (durability / cross-generation corruption, 확정)** | `persist_text_authority_sidecars`가 `File::create`(in-place truncate)로 sidecar를 쓴다 | **fixed + regression green** |
| IMPL-D | P1 (retrieval correctness, 확정 / **미수정**) | 리터럴 regex 패턴이 keyword로는 찾히는 텍스트를 찾지 못한다. delta와 무관 | **blocked — repro 있음, 원인 미특정** |

### IMPL-C 상세

`persist_text_authority_sidecars`(`crates/quanta-index-lexical/src/lib.rs`)가 5개
text-authority sidecar를 `std::fs::File::create(path)`로 기록했다. 이는 기존 파일을
**제자리에서 truncate**하므로 inode가 유지된다. 결과:

1. **crash 중간 상태가 정상처럼 보인다.** 절반만 쓰인 sidecar가 "존재"한다. seal이
   이를 검증하지 않으므로 QI-BB-030과 같은 계열의 내구성 공백이다.
2. **generation 간 오염.** delta가 base의 변경되지 않은 파일을 공유하게 되면(W3의
   hard-link 재사용) rebuild가 **base의 바이트를 덮어쓴다**.

(2)는 내가 hard-link 재사용을 넣으면서 실제로 재현됐다:

```
g1_sidecars_before=[(3626540977,112),(3626540973,235),...]
g1_sidecars_after =[(3626540977,120),(3626540973,292),...]
g1_sidecars_unchanged=false      <-- inode 동일, 길이 변경 = base 오염
```

**수정**: 5개 sidecar 전부를 메모리로 직렬화한 뒤 기존 `write_atomic_durable`
(temp + `rename` + 부모 디렉터리 fsync)로 발행한다. rename은 새 inode를 만들므로
base가 보호되고, 부분 기록 상태가 노출되지 않는다.

**회귀**: `delta_generation_does_not_mutate_base_text_authority_sidecars` —
base sidecar의 `(inode, len, sha256)`을 delta 전후로 비교한다.

```
g1_sidecars_unchanged=true
test delta_generation_does_not_mutate_base_text_authority_sidecars ... ok
```

### IMPL-D 상세 — 미수정, 다음 owner에게 인계

**증상**: 어떤 텍스트가 keyword 질의로는 찾히는데 **동일 토큰을 리터럴 regex
패턴으로 질의하면 찾히지 않는다.**

**중요**: 이것은 delta/carry-forward 버그가 **아니다**. 독립적으로 새로 build한
generation에서도 동일하게 재현된다. 조사 중 delta 경로에서 처음 관찰했으나
independent full-rebuild oracle과 대조해 배제했다.

관측 (`quanta-index-lexical`, 동일 adapter, 동일 corpus):

| 질의 | generation | 결과 |
| --- | --- | --- |
| keyword `freshsentinel` | g2 (delta) | `["chunk-beta"]` ✅ |
| regex `freshsentinel` | g2 (delta) | `[]` ❌ |
| regex `gamma.*` | g2 (delta) | `["chunk-beta"]` ✅ |
| regex `freshsentinel` | **g9 (full rebuild, 동일 최종 내용)** | `[]` ❌ |
| regex `alpha_marker` | g9 | `["chunk-alpha"]` ✅ |
| regex `retiredsentinel` | g1 | `["chunk-beta"]` ✅ |

`retiredsentinel`(g1)은 되고 `freshsentinel`(g2/g9)은 안 된다. 두 패턴은 같은
형태(순수 리터럴)다. `gamma.*`(와일드카드)는 된다.

**재현 방법**: `crates/quanta-index-lexical/tests/generation_delta_base_carryforward.rs`의
fixture를 그대로 쓰고, `LqExpr::Leaf(LqLeaf::Regex(...))` 질의를 `LqOptions::defaults()`로
실행한다. (조사에 쓴 test는 tree에 남기지 않았다 — 원인을 특정하지 못한 채 red
test를 남기면 다른 lane 전체가 막힌다. 위 표가 재현 계약이다.)

**배제한 가설**: delta carry-forward(오라클로 배제), sidecar staleness(atomic write
수정 후에도 재현), base 오염(수정됨).

**남은 후보**: regex leaf의 리터럴 추출 / trigram prefilter 교집합 / exact
verification 경로. owner는 QI-BB-011(text semantics) 또는 W4 query 실행 계약이다.
**이 항목이 닫히기 전에는 regex route를 "검증됨"으로 표기하지 않는다.**

### IMPL-A 상세

- 재현: `./scripts/cargow --lane clippy-lane clippy --workspace --all-targets --all-features -- -D warnings`
- 최초 실패 지점: `crates/quanta-index-contract-base/src/query/constraints.rs:109` (`redundant_closure_for_method_calls`). cargo가 첫 crate 실패에서 멈추므로 나머지는 가려져 있었다.
- 전체 범위 (crate별 고유 error 수):

| crate | error 수 | 성격 |
| --- | --- | --- |
| `quanta-index-contract-base` | 3 | redundant closure/clone |
| `quanta-index-contract` | 35 | cluster-membership 신규 영역: `indexing_slicing` 14, doc backticks 7, `as_conversions` 5, 기타 |
| `quanta-index-ipc` | 12 | wire 테스트의 `arithmetic_side_effects`/`indexing_slicing`/`Result::ok` |
| `quanta-index-lexical` | 2 | `Result::ok`, redundant clone |
| `quanta-index-sdk` | 9 | doc backticks, `needless_pass_by_value`, `indexing_slicing`, `unchecked_duration_subtraction` |
| `quanta-index-search-plane` | 6 | `significant_drop_tightening` 5, `suspicious_operation_groupings` 1 |

- 원인 분류: product defect 아님(동작 결함 없음) / **repo gate 파손**. cluster-membership 기능이 clippy rail을 통과하지 않은 채 landing됨. pre-commit hook에는 clippy가 없고 CI에만 있다.
- 수정 방침: `#[allow]` 금지 규칙을 지켜 실제 수정. `#[expect(..., reason=...)]`은 근거가 있는 3곳에만 사용.
- **구조적 수확 1**: `indexing_slicing` 8건이 동일한 `windows(2)` + `pair[0]`/`pair[1]` canonical-order 검사 3벌 복제였다. CLAUDE.md "No mirror methods" 위반. `crates/quanta-index-contract/src/canonical_order.rs`에 단일 helper `first_canonical_order_break_v1`로 통합하고 6개 단위 test를 추가했다. 세 호출자(`ipc/semantic_source.rs`, `query/cluster_membership.rs`, `results/cluster_membership.rs`)가 각자의 typed error로만 매핑한다.
- **구조적 수확 2**: `MAX_CLUSTER_MEMBERSHIP_READ_V1 as usize` 5건을 `u32::try_from(len)` 상향 비교로 교체. silent narrowing 제거이자 fail-closed 형태다. const-generic 자리 1건은 `CLUSTER_MEMBERS_CAPACITY_V1` 상수 + drift 고정 test로 대체.
- **관찰**: `ingest_dispatcher.rs`의 `direct_dirty_materializer_echoes_batch_digest_in_receipt`는 `receipt.manifest_digest != batch.batch_digest`를 검사한다. clippy가 버그로 의심했으나 의도된 동작이다. 다만 이는 구조안 §11이 이미 지목한 **generation digest / batch digest 필드 혼용**의 실증이다. W1에서 제거 대상.

### IMPL-B 상세

- 경로: `crates/quanta-index-lexical/src/lib.rs:3043` `prepare_generation_for_ops`
  - `if target_path.exists() { return Ok(()); }` (3049–3051)로 시작한다.
  - base clone은 그 아래 `copy_generation_directory(base, target)` (3090)에서만 일어난다.
- 그런데 6개 sidecar ingest port가 각각 같은 generation 디렉터리를 먼저 만든다: `RepoCommitRecencyIngestPort`(3443), `RepoMetaIngestPort`(3472), `RepoTopicIngestPort`(3501), `RepoDescriptionIngestPort`(3530), `FileOwnershipIngestPort`(3559), `FileContributorIngestPort`(3588) — 모두 `std::fs::create_dir_all(&index_path)`.
- 따라서 producer가 `publish_batch(recency for g2)` → `build_batch(delta g2 base=g1)` 순서로 보내면 base clone이 건너뛰어지고, g2는 delta scope만 가진 generation이 된다. 오류 없이 seal된다.
- 재현 test: `crates/quanta-index-lexical/tests/generation_delta_base_carryforward.rs`
  - `delta_generation_inherits_unmutated_base_scopes` (control, sidecar 없음)
  - `delta_generation_inherits_base_when_a_sidecar_authority_lands_first` (hazard)
  - 두 test 모두 외부 관찰만 검증한다: searcher가 g2에서 `alpha_marker`를 돌려주는가. 물리적 carry-forward 방식(full copy / hardlink / native snapshot)에 독립이다.

**확정 (수정 전 실행 결과)**

```
test delta_generation_inherits_unmutated_base_scopes ... ok
test delta_generation_inherits_base_when_a_sidecar_authority_lands_first ... FAILED
Error: "sidecar-first delta: generation g2 query `alpha_marker` expected [\"chunk-alpha\"], got []"
```

대조군은 통과하고 hazard만 실패한다 = 원인이 sidecar 선행 publish로 특정된다.
오류 없이 seal되므로 **silent data loss**다.

**수정 내용** (`crates/quanta-index-lexical/src/lib.rs`)

1. `search-corpus-delta-base.cbor` 마커를 도입했다. delta generation이 실제로
   어떤 base를 carry-forward했는지 기록하는 **명시적 authority**다. 디렉터리 존재는
   더 이상 판단 근거가 아니다.
2. `prepare_generation_for_ops`를 다시 썼다:
   - batch가 base를 선언하지 않으면 no-op
   - 마커가 같은 base를 기록하고 있으면 idempotent no-op
   - 마커가 **다른** base를 기록하고 있으면 typed `DELTA_BASE_CONFLICT`
   - 마커는 없는데 Tantivy `meta.json`이 이미 있으면 typed `DELTA_BASE_UNRESOLVED`
     (carry-forward를 증명할 수 없으므로 fail-closed. 추정으로 진행하지 않는다)
   - 그 외에는 base를 clone하고 마커를 기록한다
3. `copy_generation_directory`를 `clone_generation_directory_preserving_existing`으로
   교체했다. **이미 존재하는 항목을 덮어쓰지 않는다** — 이 generation이 자기 몫으로
   publish한 authority가 base의 같은 이름 authority보다 우선한다. 이것이 delta의
   정의이지 heuristic이 아니다. 기존 `copy_generation_directory`는 dead가 되어
   삭제했다 (47줄).
4. 마커 기록은 clone **이후**다. clone 도중 crash하면 다음 시도가
   `DELTA_BASE_UNRESOLVED`로 fail-closed된다 — 부분 base 위에 delta를 얹는 것보다
   낫고, 기존 `discard_incomplete_generation` 복구 경로와 맞는다.

**수정 후**

```
./scripts/cargow --lane test-integration-lane test --offline --all-features \
  -p quanta-index-lexical --test generation_delta_base_carryforward
test delta_generation_inherits_unmutated_base_scopes ... ok
test delta_generation_inherits_base_when_a_sidecar_authority_lands_first ... ok
test result: ok. 2 passed; 0 failed; 0 ignored
```

**회귀 확인**: `-p quanta-index-lexical` 전체 (75+33+4+2+7+37+216+38 = 412 tests) green,
`-p quanta-index-semantic` 전체 (37+1+1+4+38+4+2 = 87 tests) green,
`-p quanta-index-search-plane` green.

## 1. 실행 프롬프트 대비 적용한 수정 4건

프롬프트 원안을 그대로 쓰지 않고 아래 4건을 반영해 실행한다. 근거는 세션 리뷰에서 제시했다.

### M1 — 완료 정의를 W0 gate 결과 조건부로 분리

원안 §9의 완료 정의는 all-or-nothing이라 §5가 허용한 gate 실패와 모순된다. G0-L/G0-S가 막히면 W3가 차단되고 QI-BB-003/006/017/027/030이 구조적으로 닫을 수 없게 된다. 아래처럼 미리 분기한다.

| Gate 결과 | 완료 정의 |
| --- | --- |
| G0-L PASS | QI-BB-006 lexical lane = native snapshot 재사용으로 close. full-copy writer 삭제 |
| G0-L BLOCK | QI-BB-006 lexical lane = `blocked`, 사유·probe 증거 기록. QI-BB-003/030은 **기존 layout 위에서** close (physical GC + seal 완결성은 layout 독립) |
| G0-S PASS | QI-BB-027 = native ANN version/coverage로 close |
| G0-S BLOCK | QI-BB-027 = ANN manifest 계약 + typed 동작만 close, layout 전환은 `blocked` |
| G0-C PASS | W2 catalog 전면 도입 |
| G0-C BLOCK | W2는 선택한 대안 engine 재probe까지 `blocked`. W1/W4/W5는 계속 |
| G0-R PASS | hard-cancel 보장 구현 |
| G0-R BLOCK | cooperative-only cancellation을 **명시적 계약**으로 고정. process isolation은 별도 probe |

gate BLOCK은 실패가 아니라 확정된 설계 사실이다. BLOCK을 우회하려고 gate 기준을 낮추지 않는다.

### M2 — local commit 허용 + W별 green landing state

원안은 commit을 금지했다. 그러나 `no-dual-write` + 단일 실행자 + 136k LOC 교체에서 commit 금지는 복구 지점을 0으로 만든다. C1↔C2 사이에서 중단되면 authority가 절반만 교체된 broken 상태로 남는다.

- **허용**: `main`에 local commit (이 repo는 feature branch를 쓰지 않는다)
- **계속 금지**: push, PR 생성, deploy, production state-root migration/삭제
- 각 commit은 **green landing state**여야 한다: 그 지점에서 `just rust-profile test-fast`가 green이고 authority가 단일하게 일관됨
- 중단은 green landing state에서만 한다

### M3 — G0-C 증거 항목 확장

원안의 G0-C는 commit p95/WAL/pragma만 요구한다. 이 repo는 `#[derive(Serialize)]`까지 금지하며 cold-build 예산을 지키므로 신규 vendor 도입 비용을 gate에 포함한다. 추가 제출물:

- cold-build 시간 delta (신규 dep 추가 전/후, 동일 lane)
- MSRV 1.92.0 호환 확인
- `just rust-deny` / `just rust-machete` 통과
- **대안 engine 기각 사유** (최소 redb). 대안 없는 ADR은 거수기다

### M4 — QI-BB-007 순서 역전

원안은 hash embedder를 dev/test로 격리하면서 lexical-only mode 신설도 금지하고, 동시에 IT-16(production relevance)은 credential 부재로 blocked가 된다. 이는 **검증 가능한 대체물이 생기기 전에 로컬 유일 동작 default를 제거**하는 순서다. findings 자체가 QI-BB-007을 `P1*`(제품 결정 의존)로 표기했다.

- hash profile은 **기능 유지**. 명시적 `dev` 라벨과 profile identity만 먼저 고정한다
- production learned profile의 judged relevance가 **실측된 뒤에** 격리 여부를 결정한다
- 이 결정은 제품 계약이므로 구현자가 단독 확정하지 않고 `blocked`로 올린다

## 2. W0–W7 / C1–C4 상태

| ID | 상태 | 근거 |
| --- | --- | --- |
| W0 | in_progress | G0-L/G0-S passed. G0-C/G0-R 미착수. §3 참조 |
| W1 | planned | |
| W2 | planned | G0-C 의존 |
| W3 | in_progress | lexical hard-link 재사용 구현·검증 완료. semantic lane과 sidecar 증분은 미착수 |
| W4 | planned | |
| W5 | planned | G0-R 의존 |
| W6 | planned | M4 적용 |
| W7 | planned | |
| C1 | planned | |
| C2 | planned | |
| C3 | planned | |
| C4 | planned | |

## 3. W0 상세

| Gate | 상태 | 증거 |
| --- | --- | --- |
| W0-0 scan fixture 복구 (QI-BB-010 전제) | **passed** | §3.1 |
| G0-L Tantivy snapshot | **passed** | [ADR](adr/G0-L-tantivy-snapshot-reuse.md), §3.2 |
| G0-S Lance snapshot | **passed** | [ADR](adr/G0-S-lance-snapshot-reuse.md), §3.3 |
| G0-C catalog | planned | 미착수 |
| G0-R runtime | planned | 미착수 |

실행: `just rust-w0-storage-gates` (신규 recipe). 두 probe target은
`tools/ci/test-authority.toml`과 `pr-workspace-nextest` rail에 등록했다.

### 3.0 게이트 결과 요약

| Gate | 결론 | 핵심 수치 |
| --- | --- | --- |
| G0-L | PASS | segment 파일 재작성 0건. 2,000-doc base(55,595B)에 1-doc delta가 **2,955B(5%)**만 신규 기록, 8개 파일은 base로의 hard link. 독립 full rebuild와 **랭킹 완전 동일** (`max_abs_score_delta=0.000000`) |
| G0-S | PASS | 파일 재작성 0건. 512-row base(24,170B)에 delta가 **2,619B**만 신규 기록. **old version in-place 분기는 거부됨** — 대안 하나가 실증으로 닫힘 |

**W3 영향**: lexical/semantic 양쪽 모두 full-copy를 hard-link 재사용으로 교체 가능.
QI-BB-006이 두 lane 모두 unblocked. M1의 "G0-L BLOCK" 분기는 불필요해졌다.

**G0-S가 닫지 못한 것**: ANN delete/refill recall은 의도적으로 제외했다. storage
capability gate에 quality threshold를 섞으면 서로를 가린다. QI-BB-027 / IT-11이
semantic owner의 exhaustive exact-cosine oracle로 별도 판정한다.

### 3.2 G0-L 상세

[ADR](adr/G0-L-tantivy-snapshot-reuse.md) 참조. 4개 test 전부 pass.
probe 작성 중 발견한 **자체 결함 2건**을 기록해 둔다 (product defect 아님):

1. 최초 probe가 `.managed.json`을 segment 데이터로 취급해 "tantivy가 파일을
   재작성했다"고 오판했다. 실제로는 Tantivy의 managed-file 목록이며 `meta.json`과
   같은 북키핑이다.
2. 최초 probe가 `nlink`를 파일 identity에 포함해, hard-link 성공 자체를 "base가
   변조됨"으로 오판했다. link count는 content 속성이 아니다.

두 결함 모두 **probe를 느슨하게 만드는 방향이 아니라** 판정 기준을 정확히 하는
방향으로 고쳤고, 수정 근거를 probe 주석에 남겼다.

**남은 공백**: `gc_deleted_files=0`. GC arm은 실행됐지만 merge policy가 garbage를
만들지 않아 **실제 파일 삭제 중 pinned reader 생존은 입증되지 않았다**. W3가 실제
segment 삭제를 강제하는 case를 추가해야 한다. 재사용 결정 자체는 immutability +
hard-link 결과에 기반하므로 영향받지 않는다.

### 3.3 G0-S 상세

[ADR](adr/G0-S-lance-snapshot-reuse.md) 참조. 4개 test 전부 pass.

가장 중요한 결과는 **기각된 대안**이다: lancedb 0.30에서 checkout된 과거 version에
대한 write는 거부된다 (`Invalid input, table cannot be modified when a specific
version is checked out`). 따라서 "하나의 dataset + generation별 Lance version"
layout은 불가능하고, **generation별 dataset 디렉터리**가 유지된다. 이 사실은
`lance_refuses_to_mutate_a_checked_out_older_version` regression으로 고정했으므로
upstream 동작이 바뀌면 조용히 흘러가지 않고 test가 깨진다.

부수 확인: `Prune` 이후 pinned version은 **fail-closed**한다 (다른 row set을 조용히
서빙하지 않는다).

### 3.1 scan fixture 복구 — passed

**재현한 실패** (`4914156`, 2026-09-16):

```
./scripts/cargow run -p quanta-index-scan-experiment --bin scan_vs_index --locked -- \
  --out-dir <tmp>/scan1 --chunks 200 --chunk-bytes 256 --needle-count 5 --samples 5
-> scan_vs_index: typed failure GENERATION_IDENTITY_INCOMPLETE:
   lexical: incomplete generation has no sealed identity at <...>/g1/search-corpus-generation-identity.cbor
```

**원인 분류**: product defect 아님 / fixture defect. 실험 binary가 legacy `LexicalChannelOp` + `LexicalIndexBuildPort::build` 경로를 쓴다. 이 경로는 seal identity를 쓰지 않는다. 현재 authority 경로는 `SearchCorpusBatchBuildPort::build_batch(&SearchCorpusIngestBatch { seal: true, .. })`이며 `crates/quanta-index-lexical/src/lib.rs:3396` 에서 seal identity를 persist한다. 컴파일은 정상이다 (실행 불가는 runtime 계약 불일치).

**수정 내용** (`crates/quanta-index-scan-experiment/src/main.rs` 전면 재작성):

1. legacy channel-op 생성 제거. 파일당 하나의 `SearchCorpusReplaceScope`를 담은 단일 `SearchCorpusIngestBatch`(`ReplaceGeneration`, `seal: true`)로 ingest한다. §11의 legacy op 소비자 하나가 사라졌다.
2. digest를 조작된 label 문자열이 아니라 contract crate와 같은 length-framed `sha256:` 형식으로 계산한다. 같은 파라미터면 재현 가능하다.
3. `--source-fingerprint`를 **필수 인자**로 추가하고 결과 JSON에 싣는다. QI-BB-010의 "checked-in 수치가 현재 HEAD를 증명하지 않는다"에 대한 직접 대응이다. 출처를 못 대는 측정은 증거가 아니므로 기본값을 주지 않는다.
4. `--index-dir`을 corpus `--out-dir`에서 분리했다. 기존에는 index가 `out_dir/.index`에 있어 `rg`/`grep`이 corpus 크기가 아니라 index 크기에 따라 디렉터리를 순회했다. scan 쪽 측정의 방법론 결함이었다.
5. `index_bytes`(실제 on-disk 바이트)를 결과에 추가했다. W0 baseline이자 G0-L의 delta 비교 기준이 된다.

**실행 결과** (2026-09-16, 오염된 host — 수치는 baseline이 아니라 기능 truth 증거):

```
./scripts/cargow run -p quanta-index-scan-experiment --bin scan_vs_index --offline -- \
  --out-dir <tmp>/scan1 --chunks 2000 --chunk-bytes 256 --needle-count 5 --samples 20 \
  --source-fingerprint '7e50b1829e2737f9613789c2e070c95fefe9e5fc+dirty:6594e761a023'

{"chunks":2000,"corpus_bytes":516100,"files":2,"index_build_ms":4592.244666,
 "index_bytes":2285939,"index_hits":5,"index_query_p50_ms":0.217292,
 "index_query_p95_ms":0.651375,"index_query_p99_ms":1.638,"index_query_samples":20,
 "needle_count":5,"source_fingerprint":"7e50b182...+dirty:6594e761a023"}
```

**기능 oracle**: `index_hits == needle_count == 5`. 심어둔 needle 수와 색인 질의 결과가 정확히 일치한다.

**남은 범위**: baseline 수치 캡처는 `blocked: contended-host`. 동일 host에서 다른 cargo 빌드가 동시 실행 중이라 `index_build_ms`/`p99`는 baseline으로 쓸 수 없다. `tools/benchmark/run_scan_vs_index.py`는 아직 `--source-fingerprint`/`--index-dir`를 넘기지 않으며 bare `cargo run`을 쓴다 — W0 후속.

## 3.4 W3 lexical — hard-link 재사용 (구현 완료)

G0-L이 승인한 방식으로 `copy_generation_directory`(전체 바이트 복사)를 제거하고
inherited entry를 hard-link한다. `meta.json` / `.managed.json`은 Tantivy가 제자리에서
다시 쓰므로 **복사**하고, `.tantivy*` lock 파일은 상속하지 않는다. link 실패는 typed
`CoreError::Storage`로 올린다 — 같은 state root 안이라 cross-device가 불가능하므로
조용히 full copy로 되돌아가면 증분 보장을 말없이 잃는다.

**측정** (402-scope base, 1-scope delta):

```
base_bytes=464958  base_text_authority_bytes=408044
delta_fresh_bytes=416013
delta_fresh_entries=text-authority-docs.cbor:109848, text-authority-trigram-folded.cbor:109512,
  text-authority-trigram.cbor:109512, text-authority-positions-folded.cbor:39651,
  text-authority-positions.cbor:39651, meta.json:4393, <segment>.term:1535,
  .managed.json:545, <segment>.idx:284, <segment>.store:272, <segment>.fieldnorm:190,
  <segment>.pos:189, <base-segment>.808.del:154, <segment>.fast:145,
  search-corpus-generation-identity.cbor:131, search-corpus-delta-base.cbor:1
```

**해석 — QI-BB-006은 절반만 닫혔다.**

| 구분 | base | delta 신규 기록 | 상태 |
| --- | ---: | ---: | --- |
| Tantivy 색인 데이터 | 56,914 B | ~7,969 B (14%) | **해결** — 변경되지 않은 segment는 상속된다 |
| text-authority sidecar | 408,044 B | 408,044 B (100%) | **미해결** — 텍스트가 바뀌면 5개 sidecar 전부를 재생성한다 |

findings의 QI-BB-006 문구("base generation 전체를 복사하고 **text 변경 시 전체 text
authority sidecar를 재생성한다**")가 정확했다. 디렉터리 복사 쪽은 닫혔고, sidecar
전체 재생성이 남은 절반이며 이 fixture에서는 그쪽이 비용의 88%다. **따라서
QI-BB-006을 `passed`로 올리지 않는다.**

회귀 test `delta_generation_does_not_rewrite_unchanged_base_bytes`는 색인 데이터 절반만
예산으로 판정하고 sidecar 바이트는 별도로 보고한다 — 하나의 숫자로 합치면 남은 절반이
가려진다. 검출력은 확인했다: `inherit_generation_entry`를 full copy로 되돌리면 실패한다.

## 4. Finding 상태 (QI-BB-001–032)

초기값은 findings.md 확정 상태 그대로이며 owner 배정만 기록한다.

<!-- FINDINGS-TABLE -->

## 5. 실행 command 기록

<!-- RUN-LOG -->
