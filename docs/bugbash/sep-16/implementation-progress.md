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
| IMPL-A | P1 (repo gate) | `just rust-clippy`(= CI `ci.yml` job `rust-clippy`, `clippy --workspace --all-targets --all-features -- -D warnings`)가 감사 HEAD에서 이미 RED. main이 CI 실패 상태 | **fixed — `just rust-clippy` exit 0** |
| IMPL-E | P2 (repo gate) | `just semgrep`(CI job `semgrep` + pre-commit)이 감사 HEAD에서 RED: 317건, 그중 315건이 test 코드의 unwrap/expect/panic | **fixed** — §0.2 IMPL-E |
| IMPL-F | P2 (supply chain) | `just rust-deny`가 RED: 2026년 advisory 7건 (event-listener, h2, rkyv×3, rustls, lru) | **fixed** — 6건 lock bump, 1건 범위 지정 ignore. §0.2 IMPL-F |
| IMPL-G | P3 (dead surface) | `quanta-index-searchd`가 사용하지 않는 `quanta-index-lexical` 의존성을 선언 (`just rust-machete` RED) | **fixed** — 의존성 제거 |
| IMPL-B | **P1 (silent data loss, 확정)** | sidecar authority가 delta보다 먼저 publish되면 delta generation이 base를 통째로 잃는다 | **fixed + regression green** |
| IMPL-C | **P1 (durability / cross-generation corruption, 확정)** | `persist_text_authority_sidecars`가 `File::create`(in-place truncate)로 sidecar를 쓴다 | **fixed + regression green** |
| IMPL-I | **P1 (sealed corpus mutated after seal, 확정)** | writer cache eviction commit이 sealed generation의 `meta.json`을 재기록. sealed manifest 도입 즉시 검출 | **fixed** — seal 시 writer retire, sealed에 index mutation 거부. §3.10 |
| IMPL-H | P3 (flaky merge gate) | `quanta-index-embed` `max_batch_knob_controls_request_splitting`이 concurrency race로 간헐 실패 | **fixed** — §3.8 IMPL-H |
| IMPL-D | **P1 (retrieval correctness, 확정)** | regex prefilter가 리터럴 *교대(alternation)* 집합을 *논리곱(AND)*으로 처리해 조용히 결과를 떨어뜨린다 | **fixed + regression green** |

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

### IMPL-D 상세 — regex prefilter의 alternation/conjunction 혼동

**증상**: 어떤 텍스트가 keyword 질의로는 찾히는데 동일 토큰을 regex로 질의하면
**0건**이 나온다. 오류도, 경고도 없다.

**근본 원인**: `regex_syntax::hir::literal::Extractor`의 `Seq`는 **교대 집합**이다 —
매치는 그중 *하나*를 포함하면 된다. 그런데 이 repo는 그것을 `required_literals`라
이름 붙이고 "the trigram prefilter should AND against"라고 문서화한 뒤, 실제로
`regex_prefilter`에서 전 리터럴의 trigram을 **교집합**했다.

두 가지 오탐 계열이 발생한다:

1. **명시적 교대.** `/(alphamarker|betamarker)/` → 리터럴 `[alphamarker, betamarker]`
   → 교집합은 **둘 다** 가진 문서만 반환한다. 한쪽만 가진 문서는 조용히 사라진다.
2. **케이스폴딩.** `(?i)` 정규화 후 추출기는 유니코드 case-fold 변형을 만든다.
   `(?i)fresh` → `["fresh", "freſh", ...]` (`ſ` = U+017F LATIN SMALL LETTER LONG S,
   `s`의 폴드 짝). ASCII 문서는 `ſ`를 가질 수 없으므로 교집합은 **항상 공집합**이다.
   즉 **`s`를 포함하는 모든 case-insensitive regex가 0건을 반환한다.**

계측 증거 (`"gamma_replacement freshsentinel"` 단일 문서):

```
src="fre"   norm="(?i)fre"   lits=["fre" x8]                    prefiltered=1  -> 1 hit
src="fres"  norm="(?i)fres"  lits=["fres","fres","freſ", ...]   prefiltered=0  -> 0 hits
src="gamma" norm="(?i)gamma" lits=["gamma" x32]                 prefiltered=1  -> 1 hit
```

`fre`가 통과한 이유는 3바이트 리터럴 하나뿐이라 폴드 변형이 없어서였다. 4바이트로
늘리는 순간 `s`가 들어가 무너진다.

**수정 범위** — 3개 crate:

| crate | 변경 |
| --- | --- |
| `quanta-index-lq-trigram` | `regex_prefilter` → `regex_prefilter_any_of`. 의미를 **교대별 교집합의 합집합**으로 교체. 중복 교대 제거. **3바이트 미만 교대가 하나라도 있으면 전체를 `RegexPrefilterUnusable`로 처리** — 그 교대로만 매치되는 문서에는 trigram 증거가 없으므로 일부만 걸러내면 그 자체가 오탐이 된다 |
| `quanta-index-lq-regex` | `extract_required_literals` → `extract_prefilter_literal_alternation`, `RegexExecutor::required_literals` → `prefilter_literal_alternation`. 이름이 거짓말한 것이 버그의 원인이므로 이름을 고쳤다 (CLAUDE.md: "Doc-comments that contradict the code are bugs") |
| `quanta-index-lexical` | `RegexPlan::required_literals` → `literal_alternation`, 호출부를 새 prefilter로 전환 |

**golden 갱신**: `golden_regex_prefilter_fn_and_handle` → `..._alternation_fn_or_handle`.
repo 자체 golden corpus가 이미 이 오탐을 담고 있었다 — `["fn ", "handle_"]`에 대해
기대값이 `[1, 2]`였는데, doc 4(`impl Handler { fn new() ...`)는 `fn `를 가지므로
`/(fn |handle_)/`의 정당한 후보다. 기대값을 `[1, 2, 4]`로 고쳤다.

**회귀**: `crates/quanta-index-lexical/tests/regex_literal_alternation.rs`
- `case_folded_literal_regex_matches_the_same_text_as_the_keyword_route`
  (keyword 결과를 oracle로 삼아 regex 결과와 대조)
- `alternation_regex_returns_documents_matching_any_branch`

**검출력 확인** (교집합 의미로 되돌린 반례 실행):

```
Error: "regex freshsentinel: expected [\"c-a\"], got []"
Error: "regex alternation: expected [\"c-both\", \"c-left\", \"c-right\"], got [\"c-both\"]"
```

**연관**: QI-BB-011(text semantics)의 실증 사례다. 단, findings가 기술한 "whitespace/
ASCII folding 의존"보다 심각하다 — 보조 경로의 근사 문제가 아니라 primary regex
route의 **무증상 false negative**다.

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

### IMPL-E 상세

- 재현: `just semgrep` → 317건. 315건이 `rust-no-unwrap` / `rust-no-panic` / `rust-no-todo` / `rust-no-dbg`이며 전부 test 코드.
- 원인: 같은 shape을 clippy(`unwrap_used`, `expect_used`, `panic` 등, workspace lint)와 semgrep이 **이중 집행**했는데 clippy는 `clippy.toml`의 `allow-*-in-tests`로 test 예외를 갖고 semgrep은 갖지 않았다. 두 authority가 다른 답을 내는 상태.
- 수정: 중복 rule 4개를 `tools/ci/semgrep/rules.yml`에서 제거하고 헤더에 근거를 남겼다. semgrep은 clippy가 lint를 갖지 않는 silent-fallback shape(`or_else(|_| Ok)`, `Err(_) => default`, `is_ok()` 분기 등)만 유지한다. production 코드의 unwrap/panic 집행력은 `just rust-clippy`가 그대로 갖는다(§IMPL-A로 exit 0 확인).
- 나머지 2건은 실제 코드 수정.

### IMPL-F 상세

- 재현: `just rust-deny` → advisory 7건.
- 수정: `event-listener 5.4.2`, `h2 0.4.19`, `rkyv 0.8.18`, `rustls 0.23.45`로 lock bump(6건 해소). `RUSTSEC-2026-0253`(lru 0.12.5 `pop()` panic-safety)은 tantivy 0.22가 `lru ^0.12`에 고정돼 bump 불가 — tantivy가 인스턴스화하는 유일한 `LruCache<usize, Block>`의 key가 `Drop`을 갖지 않아 unsound 경로에 진입할 수 없음을 근거로 **범위 지정 ignore**(`deny.toml`, tantivy bump 시 제거 조건 명시).

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
| W0 | **passed** | G0-L/G0-S/G0-C passed, G0-R baseline pinned(cooperative-only). §3 참조. timing 재측정만 `blocked: contended-host` |
| W1 | planned | |
| W2 | in_progress | QI-BB-029 preflight(§3.11) + QI-BB-026 boot inventory/quarantine(§3.13) + **QI-BB-032 idempotency catalog(§3.16, `quanta-index-catalog` SQLite adapter 신설, probe crate 삭제)** 완료. 남은 것: QI-BB-020 auxiliary authority shard/persistence(catalog 확장), quarantine control surface |
| W3 | in_progress | lexical hard-link(§3.4) + sidecar 증분(§3.4.1) + semantic hard-link(§3.4.2) + physical GC(§3.9) + lexical sealed manifest(§3.10) + semantic sealed manifest/QI-BB-017(§3.14) 완료. 남은 것: sharded sidecar 포맷(O(delta) write), ANN versioned artifact(QI-BB-027) |
| W4 | in_progress | QI-BB-004 scope cap(§3.6) + SnapshotRegistry(§3.7) + QI-BB-005 execution budget(§3.8) 완료. 남은 것: QI-BB-025 보완 #4(bounded window), QI-BB-024 regex cache, streaming projection collector |
| W5 | in_progress | QI-BB-002 phase 1(§3.12) 완료: per-connection thread + bounded dispatch slot + typed overload + cooperative `RequestBudgetV1`(deadline/cancel) + peer watch. 남은 것: cancel을 lexical collector 내부(candidate batch 사이)까지 내리기, overload/refusal 서버 metric |
| W6 | in_progress | QI-BB-028 + QI-BB-031 embedding identity/vector invariant(§3.15) 완료. 남은 것: QI-BB-007(M4: production profile 측정 후), QI-BB-018/019 hybrid |
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
| G0-C catalog | **passed** | [ADR](adr/G0-C-catalog-engine.md) — SQLite 채택, redb 기각 대안 실측 |
| G0-R runtime | **baseline pinned** | [ADR](adr/G0-R-runtime-cancellation.md) — hard cancel 불가, cooperative-only 계약 |

실행: `just rust-w0-storage-gates` (신규 recipe, G0-L/S/R/C 전부). probe target은
`tools/ci/test-authority.toml`과 `pr-workspace-nextest` rail에 등록했다.

### 3.0 게이트 결과 요약

| Gate | 결론 | 핵심 수치 |
| --- | --- | --- |
| G0-L | PASS | segment 파일 재작성 0건. 2,000-doc base(55,595B)에 1-doc delta가 **2,955B(5%)**만 신규 기록, 8개 파일은 base로의 hard link. 독립 full rebuild와 **랭킹 완전 동일** (`max_abs_score_delta=0.000000`) |
| G0-S | PASS | 파일 재작성 0건. 512-row base(24,170B)에 delta가 **2,619B**만 신규 기록. **old version in-place 분기는 거부됨** — 대안 하나가 실증으로 닫힘 |
| G0-C | PASS | SQLite(rusqlite bundled): 4개 조건(crash-consistency via child `abort()`, bit-rot 검출 전 occurrence, `VACUUM INTO` export, MSRV/deny/machete/cold-build) 통과. durable 1-key commit macOS `F_FULLFSYNC` p50 **5.0ms** vs redb 5.2ms — apples-to-apples(`fullfsync=ON`)로 비교. 기각 대안 redb는 별도 probe로 실측 |
| G0-R | BASELINE | native search 중 hard cancellation 불가. process isolation 불채택. W5는 cooperative deadline + peer-liveness checkpoint(native search 전/candidate batch 사이/encode 전)로 설계 |

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

**해석 — QI-BB-006은 절반만 닫혔다.** (아래 §3.4.1에서 두 번째 절반의 재도출 비용을 닫았다.)

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

### 3.4.1 W3 lexical — text-authority sidecar 증분 갱신

`quanta-index-lq-trigram` / `quanta-index-lq-positions`에는 `from_prior` +
`upsert_doc`/`remove_doc`가 이미 있었다 — 증분 경로가 crate에 설계돼 있는데 adapter가
배선하지 않고 매번 full scan + 재토큰화 + 재구성을 했다.

**구현** (`crates/quanta-index-lexical/src/lib.rs`):
- `TextAuthorityBuilders` — 4개 파생 인덱스 + doc table을 하나가 소유. "문서를 어디에
  넣는가/빼는가"가 full rebuild와 증분 경로에서 갈라질 수 없다.
- `plan_text_authority_delta(index, ops)` — op 적용 **전**에 batch를 분류한다.
  `ReplaceLexicalScope`/`TombstoneLexicalScope`만이면 `Incremental { retired, added }`,
  `ClearLexicalSurface`/legacy chunk op가 있으면 `Rebuild`. retired candidate는 mutation
  전 index에서 path term query로 읽는다(sidecar 스키마 변경 없음).
- 새 doc은 `max(doc_id)+1`. 이전 sidecar가 없으면(fresh replace) full rebuild. 이전
  sidecar 로드 실패는 **propagate** — full rebuild로 조용히 덮지 않는다.

**정확성 oracle (DA-06)**: `delta_generation_text_authority_matches_independent_full_rebuild`
— replace + tombstone + 신규 scope를 delta로 적용한 g2와, 같은 최종 내용을 base 없이
build한 g9를 regex/phrase/keyword 11개 probe로 대조. 전부 일치. 검출력: `retire`를
무력화하면 `regex retired: incremental=["chunk-beta"] rebuild=[]`로 실패.

**비용 실측** (1,502 scopes, sidecar 1.68 MB, contended host):

```
QI-BB-006-EVIDENCE scopes=1502 text_authority_bytes=1681370 delta_one_scope_ms=898 full_rebuild_ms=1516
```

**정직한 해석**: 1.7× 개선이지만 **write bytes는 그대로 O(N)**이다. sidecar 파일이
단일 CBOR blob이라 어떤 갱신도 전체를 다시 쓴다. 이번에 O(delta)가 된 것은 재도출
(scan + 토큰화 + posting 재구성)이고, load(`from_prior`는 O(P)) + serialize가 O(N)으로
남는다. 진짜 O(delta) write는 **sharded sidecar 포맷**(scope/segment 단위 shard +
manifest)이 필요하며 이는 W3의 포맷 버저닝 항목이다. **QI-BB-006은 여전히 `passed`가
아니다** — 색인 절반 해결, sidecar 재도출 해결, sidecar write bytes 미해결.

### 3.4.2 W3 semantic — dataset hard-link 상속 (구현 완료)

G0-S 결정 1·2 그대로: `crates/quanta-index-semantic/src/build.rs`의 `copy_dir`를 삭제하고
`inherit_dataset_tree`로 교체했다. `LanceDB`가 쓰는 모든 versioned object(data, manifest,
transaction, index, deletion)는 hard link, `_versions/latest_version_hint.json`만 복사
(generation-local; 매 commit마다 다시 쓰이므로 inode를 공유하면 한 generation의 commit이
다른 generation을 가리킬 수 있다). link 실패는 typed `Storage` 오류 — 조용한 full copy 없음.
같은 generation의 두 번째 batch(`dataset/` → `dataset.staging`)도 같은 경로를 쓴다.

**측정** (`generation_delta_reuse.rs`, 512-row base, 1-row delta, contended host):

```
QI-BB-006-SEMANTIC-EVIDENCE base_files=1040 base_bytes=859451 delta_files=1049 shared_files=1037 fresh_bytes=66462
```

fresh bytes의 대부분은 `_indices/<uuid>/index.idx`(~43 KB) + `auxiliary.idx`(~9 KB) —
**ANN index가 seal마다 전체 재구축**된다. dataset 공유는 닫혔고 index 재구축이 semantic
쪽의 남은 O(N) 항목이다(QI-BB-027 versioned ANN artifact 항목과 같은 자리).

**oracle**: base 파일 전부 inode/len/sha256 불변(hint 포함), delta의 hint는 base와 다른
inode, exact-path scope로 본 serving(delta 신규 row / 상속 row / base 격리). **ANN top-1은
oracle로 쓰지 않았다** — IVF_HNSW_SQ가 근사라서 같은 fixture에서 run마다 top-1이 달랐다.
`build.rs`의 "determinism is preserved" 주석은 실측과 맞지 않으며 QI-BB-027(W3/W6)에서
다룬다. 검출력: hint를 link하도록 변이 → `delta's version hint is a hard link` 실패.

## 3.6 QI-BB-004 — semantic lexical scope의 `scope_top_k` (구현 완료)

**진단 확정**: dispatcher `semantic()`이 scope의 `top_k`를 읽지 않고
`search_all_constrained`(num_docs 기반 full recall)로 scope 전체를 실체화한 뒤 `BTreeSet`
→ `embedding_id IN (...)`으로 넘겼다. 계약 위반이자 corpus 크기에 비례하는 allowlist.

**수정** (`query_dispatcher.rs` / `query_dispatcher/semantic_query.rs`):
- `scope.top_k`를 공용 gate(`validate_query_top_k`)로 검증 — raw IPC도 SDK와 같은 코드.
- lexical lane에 `search_constrained(query, constraints, scope_cap)`로 **정확히 cap개**의
  ranked candidate만 요청. adapter가 cap을 넘기면 truncate가 아니라 `InvalidContract`.
- `SemanticScopeV1 { requested_cap, candidate_ids }`가 explanation에
  `semantic.scope.cap=<n>`(Plan) + `semantic.scope.text_candidates=<m>`(ExecFanout)을 남긴다.
- `LexicalSearcher::search_all_constrained`를 port에서 **삭제**(유일 production caller가
  이 경로였다). `search_all`은 structural routing만 쓰며 doc에 "QI-BB-005 W4 budget 대상"을
  명시. dispatcher test stub의 죽은 recorder 필드 2개 제거.

**완료 기준 검증** (`e2e_semantic_scope_cap.rs`, daemon front door):

| 기준 | 검증 |
| --- | --- |
| lexical match 100, `scope_top_k=2`, `top_k=10` → 후보가 lexical 상위 2 밖으로 안 나감 | lexical route 자체를 oracle로(같은 query, `top_k=2`) — scoped semantic ⊆ 그 집합, ≤2 rows, explanation에 cap/count |
| `scope_top_k=0` / max / max+1 / u32::MAX | 0·10,001·MAX는 `QUERY_TOP_K_OUT_OF_RANGE`, 10,000 serve |
| constraint contradiction | 기존 `InvalidContract("scope constraints must equal outer")` 유지 |
| cap이 실제로 좁힌다 | scope 100 → outer `top_k=10` 전부 채움 |

**검출력**: full recall로 되돌리면 `escaped=[item_090, item_005, ...] allowed={item_055, item_020}`.

**정직한 한계**: tie boundary(같은 BM25 점수의 k번째/k+1번째)는 lexical route와 동일하게
adapter의 tie-break에 맡긴다 — 고정 candidate set 안의 total order는 `stabilize_ranked_candidates`가
보장하지만 경계 포함 여부는 generation 간 결정성을 약속하지 않는다(§4.12). `IN (...)` 술어는
cap ≤ 10,000이라 bounded이며 chunked query(보완 #3)는 필요해질 때 adapter 안에서 처리한다.

## 3.5 QI-BB-025 — route 공통 `top_k` 계약 (구현 완료)

**진단 확정**: findings의 증거 그대로였다. semantic/hybrid 정책은 `1..=10_000`을 선언했고
dispatcher의 continuation probe는 `top_k + 1 <= 10_000`을 요구해 **10,000을 거부**했다.
lexical/symbol/history/runtime/structural은 상한을 전혀 검사하지 않았고 `top_k=0`은 route마다
typed error / empty page / 1-row로 갈렸다. SDK type-state는 존재 여부만 보증했다.

**하나의 authority** — `crates/quanta-index-contract-base/src/query/top_k.rs`:

| 항목 | 값 / 코드 |
| --- | --- |
| public 범위 | `PUBLIC_TOP_K_MIN = 1` ..= `PUBLIC_TOP_K_MAX = 10_000` |
| public 거부 코드 | `QUERY_TOP_K_OUT_OF_RANGE` (`TopKOutOfRangeV1 { requested }`) |
| continuation fetch | `continuation_fetch_size(k) = k + 1` (saturating) |
| internal ceiling | `INTERNAL_FETCH_CEILING = 10_001` |
| internal 위반 코드 | `QUERY_INTERNAL_FETCH_OUT_OF_RANGE` → `CoreError::InvalidContract` (caller 오류가 아니라 dispatcher 결함) |

**배선** (보완 #1–#3):
- core: `validate_query_top_k` / `validate_internal_fetch_size`; `SemanticPolicy::validate_top_k`
  와 `HybridOrchestratorPolicy::validate_top_k`는 위임만 한다 (`HYB_TOP_K_INVALID` 등 route별
  코드 폐기). `SemanticPolicy::validate_fetch_size`는 adapter 경계용.
- dispatcher: `probe_top_k_v1 = validate_query_top_k + continuation_fetch_size`. `lexical`,
  `symbol`, `history`, `runtime_metadata`, `structural`, `hybrid_seed` 진입점에서 같은 validator.
- semantic adapter(`quanta-index-semantic/src/search.rs`): 받은 값은 public `top_k`가 아니라
  internal fetch size이므로 `validate_fetch_size`로 검사한다. 이 구분이 없으면 10,000이
  adapter에서 "got 10001"로 다시 거부된다 — 실제로 첫 E2E에서 그렇게 잡혔다.
- SDK: `accepted_top_k(plane, k)`가 text/semantic/scope/**hybrid seed** builder에서 round-trip
  전에 같은 코드로 거부한다. hybrid seed builder는 `build_request`를 거치지 않는 별도 조립
  경로라 따로 배선했다(`hybrid_seed_builder_refuses_out_of_range_top_k_before_any_round_trip`).

**완료 기준 검증** — `crates/quanta-index-searchd-runtime/tests/e2e_top_k_truth_table.rs`
(daemon front door, raw IPC, 8 route 전부):

| 기준 | 검증 |
| --- | --- |
| `{0, 1, 9_999, 10_000, 10_001, u32::MAX}` truth table, 8 route | `every_route_refuses_out_of_range_top_k_with_the_shared_code` + `every_route_accepts_the_public_range_including_the_maximum` — lexical/symbol/semantic/hybrid/hybrid_seed/history/runtime_metadata/structural |
| SDK와 raw IPC가 같은 코드 | SDK stub-transport test 3건 + E2E 코드 비교 (`TOP_K_OUT_OF_RANGE_CODE`) |
| 허용 최대값에서 `returned <= top_k`, `has_more/candidate_count` invariant | 8 route 모두 fixture에서 **실제 serve**(typed error 없음, ≥1 row) + wire window 독립 검사 `window_contradiction` (returned==rows, lower_bound>=returned, has_more⇒returned==top_k) |

fixture는 history/dirty/symbol/structural authority를 모두 seed해서 8 route가 전부 serve한다 —
"top_k로 거부되지 않음"을 NOT_READY 뒤에 숨기지 못하게 `fixture_gives_every_route_at_least_one_row`
가 각 route의 ≥1 row를 요구한다. harness에는 route 무관 `probe_query_route`를 추가해 8 route가
같은 코드 경로로 나가게 했다(harness의 route별 `query_*` mirror method를 더 늘리지 않음).

**검출력** (`validate_public_top_k`를 `>= PUBLIC_TOP_K_MAX`로 변이):

```
fixture is not load-bearing: lexical: did not serve: QUERY_TOP_K_OUT_OF_RANGE: ... got 10000
  (symbol, semantic, hybrid, hybrid_seed, history, runtime_metadata, structural 동일)
top_k acceptance drifted: lexical: top_k=10000 is inside the public range but was refused ...
```

**부수 관측 (QI-BB-019 증거)**: 첫 truth-table 실행에서 hybrid_seed가
`window.returned=4 but 2 rows were returned`로 잡혔다. `seed_candidates`(legacy)와
`seed_candidates_v2`의 길이가 다르고 window는 v2를 기술한다. wire 계약이 "v2가 있으면 v2가
active"라 harness도 그 규칙을 따르게 했지만, 이중 result surface 자체는 QI-BB-019 범위다.

**남은 항목**: 보완 #4(history/runtime/structural response의 bounded window/cursor 계약)는
W4 read view 항목으로 넘긴다. 현재 세 route는 `top_k` 절단은 하지만 wire window가 없다.

## 3.7 QI-BB-001 / QI-BB-017 — SnapshotRegistry: single-flight, byte-bounded 상주 handle (구현 완료)

**진단 확정**: query route 8곳 전부가 매 요청마다 `lex_opener.open(...)` / `sem_opener.open(...)`을
호출했다. lexical `open`은 sealed identity 검증 + `Index::open_in_dir` + tokenizer 등록 +
reader reload + text-authority sidecar 5개 CBOR decode + metadata snapshot 6개 load를 **요청마다**
반복했다. semantic adapter는 자체 8-entry FIFO cache(count-only, single-flight 없음)를 갖고 있었고
lexical은 cache가 없었다.

**설계** (`crates/quanta-index-search-plane/src/snapshot_registry.rs`, 구조안 §3의
"QueryReadView + SnapshotRegistry" 자리):
- search-plane이 **유일한 residency owner**. adapter는 cold `open`만 제공한다(semantic adapter의
  private OpenCache 삭제 — 이중 cache는 resident bytes를 두 배로 만들고 invalidation을 가린다).
- `SnapshotRegistry<H>`: key `(repo, revision, generation)`, `Arc<H>` handle, **single-flight**
  (같은 key 동시 miss는 하나의 flight를 기다리고 성공/typed 실패를 공유; 실패는 retain하지 않아
  다음 acquire가 재시도), **entries + bytes 이중 상한**(LRU eviction, 예산보다 큰 handle은 serve하되
  retain하지 않음), **pin 생존**(eviction/invalidate는 registry 참조만 떨어뜨림; in-flight query의
  Arc는 살아 있음), `retire(key)`가 남은 holder 수를 보고해 W3 physical GC가 삭제 전 확인한다.
- `LexicalSearcher::resident_bytes_estimate` / `SemanticSearcher::resident_bytes_estimate`를
  port 계약에 추가 — open 시 generation dir(lexical) / dataset tree(semantic)의 on-disk bytes.
  count-only cache가 corpus 크기 handle을 "1개"로 세는 문제(보완 #3)를 막는다.
- policy는 composition root(`SearchdConfig::snapshot_registry_policy`, env
  `QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES` + `..._MAX_RESIDENT_BYTES` 쌍, 기본 16 entries / 1 GiB)에서
  주입. 0은 구성 오류로 거부하고 필드는 private — 존재하는 policy는 전부 유효하다.
- **invalidation은 ingest side 소유**: `SearchPlaneIngestDispatcher`가 generation을 지명하는 7종
  batch(search corpus, commit recency, topic, description, file ownership, file contributor,
  repo meta)를 publish한 뒤 두 track의 residency를 떨어뜨린다. 현재 authority에서는 sealed
  generation 뒤에도 aux snapshot이 같은 디렉터리에 쓰이므로(불변식 §4.5 위반, W2 overlay 대상)
  이것이 stale serving을 막는 유일한 방어선이다.
- metric: `lq_snapshot_{lexical,semantic}_{hit,coalesced}_total`, `..._cold_open_ms`(pin 차원).

**완료 기준 검증**:

| 기준 | 검증 |
| --- | --- |
| 같은 generation 두 번째 query에서 index open·sidecar read 0회 | `e2e_snapshot_registry.rs::second_lexical_query_reads_no_generation_file` — 첫 query 뒤 **generation 디렉터리 전체를 삭제**하고 두 번째 query가 동일 결과. reopen이면 NotFound. counter가 아니라 fault injection oracle |
| semantic도 open-time full scan 반복 없음 (QI-BB-017 증상) | `second_semantic_query_does_not_reopen_the_generation` — sealed marker + manifest 삭제 후 동일 결과 |
| 같은 key 32개 동시 query가 한 번만 load | unit `concurrent_misses_on_one_key_open_once` (Barrier 32 threads, opener 50ms hold → opens=1, coalesced=31). front door는 아직 serial UDS(QI-BB-002)라 E2E로는 증명 불가 — 정직하게 unit으로 |
| eviction 중 in-flight query·GC 안전 | unit `eviction_and_retire_do_not_invalidate_handles_in_flight` (evicted handle 사용 가능, `retire` → `StillReferenced{holders}`) |
| mutation 뒤 stale serving 없음 | `an_auxiliary_publish_invalidates_the_resident_generation` — repo-meta publish 후 디렉터리 삭제 → 다음 query가 **실패해야** 통과 |
| cold/warm p50/p95/p99, RSS 실측 | **blocked: contended-host** (§0.1). metric 배선은 완료 |

**검출력**: ingest invalidation을 다른 generation key로 변이 → `served from a stale resident
handle: [e2e-1-src/needle.rs, e2e-3-src/other.rs]` 실패.

**남은 것 (QI-BB-017 본체)**: open-time `semantic_row_commitment_v1` full scan 자체는 그대로다 —
registry는 *반복*을 없앴고, seal-time proof / open-time root 검증 분리(보완 #1–#2, #5 streaming
membership)와 boot inventory(#6, QI-BB-026)는 W2/W3 항목이다.

## 3.8 QI-BB-005 — execution budget: 무제한 corpus collect 제거 (구현 완료, 일부 W5 이관)

**진단 확정**: `count:all`은 `effective_limit = max(top_k, num_docs)`로 **모든 match를 반환**했고,
projection / `count:N` / tie-boundary 재조회는 `collect_limit = num_docs`로 corpus 전체를
`TopDocs`에 모은 뒤 문서를 전부 fetch했다. `index:no` scan은 `AllQuery` + num_docs. 응답이
16 MiB frame을 넘으면 연산을 다 끝낸 뒤 connection이 그냥 닫혔다(typed 응답 없음).

**계약 변경 (의도적, 구조안 §7.2 "`count`는 별도 operator")**:

| 이전 | 이후 |
| --- | --- |
| `count:all` → 모든 match 행 반환, window `Exact(len)` | 행은 `top_k`로 bounded; **exact total은 count collector**로 계산해 window `Exact(total)` + `has_more` |
| `count:N` → 전체 수집 후 정렬·N 절단 | 행 `min(top_k, N)`, exact total 보고, 전체 집합은 **budget 안에서만** 수집 |
| projection → 전체 수집 후 collapse | budget 안에서 전체 수집·collapse, collapsed 수를 exact total로 보고(count 없이도) |
| tie-boundary 재조회(num_docs) | 삭제. 수집 집합 전체를 total order(score, path, line, id)로 안정화; 경계 밖 동점은 index 순서(§4.12 범위 명시) |
| `index:no` scan → corpus 전체 | corpus가 budget을 넘으면 scan 전에 typed 거부 |
| frame 초과 → connection close | `RESULT_TOO_LARGE` typed 응답(양쪽 byte 수 포함), 연결 유지 |

**새 계약 표면**:
- `LexicalSearchPageV1 { candidates, exact_total: Option<u64> }` — `LexicalSearcher::search_constrained`
  반환형. `search`는 port default(`.candidates`)로 위임. `search_all_constrained`는 이전 커밋에서 삭제.
- `LexicalExecutionBudgetV1` (core; private field, 0 거부, DEFAULT 250,000) + 
  `LEXICAL_EXAMINED_BUDGET_EXCEEDED` typed code. adapter는 `collect_bounded` **하나**로 4개
  실행 경로(text/symbol constrained, `search_all`, `search_symbols_all`)를 돌린다 —
  whole-set 실행은 `min(num_docs, budget+1)`을 수집해 초과가 관측 가능하고, count collector는
  같은 pass에서 postings만 읽는다.
- `SearchdConfig::lexical_execution_budget` (env `QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES`,
  optional, 0 거부) → `LexicalAdapter::with_state_root_and_policies`.
- IPC `ResponseEnvelope::result_too_large` + `SearchPlaneIpcError::result_too_large`,
  `ERR_RESULT_TOO_LARGE`. typed payload가 없는 envelope(테스트용)은 `None` → 기존처럼 close.

**검증**:

| 기준 | 검증 |
| --- | --- |
| projection / `count:N` / `index:no`가 budget 초과 시 typed 거부, page·`count:all`은 serve | `crates/quanta-index-lexical/tests/execution_budget.rs` (budget 4, docs 5; budget 5면 exact total과 함께 serve) |
| `count:all`이 page를 넓히지 않고 exact total 보고 | `tantivy_smoke::tantivy_count_all_keeps_the_page_and_reports_the_exact_total` (이전 test는 반대 의미를 pin하고 있었음 — 교체) |
| front door window 의미: `count:all`/`count:N`/plain/`select:path` | `e2e_exact_count_window.rs` — Exact(5)+has_more / AtLeast(3) / Exact(5) 등 fixture가 미리 아는 값 |
| dispatcher 계약 검사 | `count_options_take_an_exact_window_from_the_adapter_and_never_widen_the_page_v1` — adapter가 fetch보다 많이 주거나 total < returned면 InvalidContract |
| frame 초과 typed 응답 | `ipc::server::tests::handle_connection_sends_a_typed_refusal_for_an_oversized_response` (16 MiB+1 응답 → 같은 연결로 refusal, request_id 유지) |
| 100만 doc corpus에서 peak RSS/wall time | **blocked: contended-host** + 대형 fixture 미구축(W7 qualification) |
| `max_wall_time` | **W5 이관** — cooperative deadline은 G0-R 결론에 따라 IPC scheduling과 함께 |

**남은 것**: projection은 여전히 budget까지 in-memory aggregation(구조안 "초기 버전")이다.
streaming grouped top-k collector(보완 #3)와 keyset pagination(보완 #4)은 후속. symbol port는
count collector가 없어 `count` 옵션에서도 probe window(AtLeast)를 준다 — 문서화됨.

### IMPL-H — `quanta-index-embed` flaky test (merge gate)

`openai::tests::max_batch_knob_controls_request_splitting`이 verify-rust 전체 실행에서
간헐 실패(`[[0,1],[1,0]] != [[1,0],[0,1]]`). 원인: provider가 batch를 worker 여러 개로
동시에 보내는데 `ScriptedTransport`는 요청 내용과 무관하게 script 순서로 응답한다 —
두 worker가 두 응답을 경쟁. `with_concurrency(1)`로 test를 결정적으로 고정(5/5 green).
product defect 아님 / test 설계 결함.

## 3.9 QI-BB-003 — physical GC: retention이 실제 bytes를 회수한다 (구현 완료, byte 정책은 W2 이관)

**진단 확정**: retention은 authority CBOR record만 `remove_file`했고 sealed generation의
Tantivy/`LanceDB` 디렉터리는 영구히 남았다(변이 검출 시 `found {1,2,3,4,5}`가 그 상태).
core의 destructive port는 incomplete 전용이라 sealed 삭제 경로 자체가 없었다.

**설계**:
- core `SealedGenerationReclaimPort { reclaim_sealed_generation(retired), sealed_generations_for_pair(repo, rev) }`
  — `IncompleteGenerationDiscardPort`의 대칭. 삭제 전에 durable identity(scope + digest)를
  검증하고 불일치는 typed 거부(`GENERATION_IDENTITY_DIGEST_MISMATCH` / `..._SCOPE_MISMATCH`),
  unsealed 디렉터리는 `GENERATION_NOT_SEALED`로 거부(다른 protocol 소관). lexical/semantic
  adapter 구현: bytes 측정 → `remove_dir_all` → pair dir fsync → adapter cache 정리.
- search-plane `DirectSearchCorpusMaterializer::reclaim_retired_generations_v1` — 순서가 protocol이다:
  authority reap → ledger 재조정(더 이상 어떤 query도 그 generation을 resolve/pin 못 함) →
  **track별로 디스크의 sealed generation을 열거**(receipt의 reaped set이 아니라 filesystem;
  reap과 delete 사이의 crash orphan을 다음 pass가 찾는 이유) → retained 또는 sealing 중인
  generation 이상은 skip → `SnapshotRegistry::retire(key)` fence → holder가 남아 있으면
  **defer**(reader 밑에서 지우지 않음) → reclaim. 실패는 seal 응답으로 올라오고, 재시도는
  `finalize_only` 경로로 수렴한다.
- composition: `SearchCorpusMaterializerParts { lexical_reclaim, semantic_reclaim, snapshots }`.
  RepoMap generation은 별도 lifecycle(QI-BB-008, W6)이라 이번 범위 밖.

**완료 기준 검증**:

| 기준 | 검증 |
| --- | --- |
| 5개 sealed 후 cap=2 → backend 디렉터리도 정확히 2개 | `e2e_physical_gc::reaping_a_generation_removes_its_directories_on_both_tracks` (harness `boot_with_history_max_generations(2)`, 두 track `{4,5}`) |
| active/candidate/predecessor는 삭제되지 않음 | 같은 E2E + `retained_generations_still_serve_after_gc` (newest lexical/semantic serve) |
| pin된 generation은 삭제 안 함, crash orphan은 다음 pass에서 회수 | unit `reclaim_sweeps_orphans_and_defers_pinned_generations` — 디스크에만 남은 g1(orphan) 회수, registry가 pin한 g2는 `deferred_pinned{holders:1}`, pin 해제 후 두 번째 pass에서 회수 |
| identity가 path와 모순되는 디렉터리 | `a_directory_whose_identity_contradicts_its_path_fails_closed` — 어느 identity로도 삭제하지 않고 typed 실패(boot scanner와 같은 fail-closed; quarantine은 QI-BB-026/W2) |
| reported bytes vs `du` | reclaim은 삭제 직전 recursive bytes를 측정해 receipt에 싣는다. **retention 정책의 `max_bytes`는 여전히 authority record 길이**다 — physical bytes 기반 admission으로 바꾸는 것은 catalog(W2)에서 snapshot row에 physical/logical bytes를 두면서 한다(보완 #2, #5) |

**검출력**: reclaim 호출을 `Absent`로 변이 → `expected {4, 5} on disk, found {1, 2, 3, 4, 5}`.

## 3.10 QI-BB-030 — sealed manifest: seal이 query-openable 상태를 약속한다 (구현 완료, handle 재사용은 후속)

**진단 확정**: IMPL-C가 sidecar 쓰기를 atomic-durable로 바꿨지만 내용 commitment는 없었다.
activation/restart validator(`validate_generation_identity`)는 identity + `Index::open_in_dir`만
확인하고 query `open`이 추가로 decode하는 sidecar 5개는 보지 않았다 → publish/activation이
성공한 뒤 첫 query가 실패할 수 있었다.

**구현 중 발견한 추가 결함 (IMPL-I)**: writer cache의 eviction commit이 **sealed generation의
`meta.json`을 다시 썼다**. seal 뒤에도 writer가 cache에 남아 있다가 evict될 때 commit(변경
없음)이 실행돼 Tantivy가 meta.json을 새 opstamp로 재기록한다. 새 manifest가 이걸 즉시 잡아냈다
(`writer_cache_evicts_lru_after_threshold`가 `index commit differs from the sealed commit`으로
실패). 불변식 §4.5 "sealed corpus는 불변" 위반이 실재했던 것.

**설계** (`crates/quanta-index-lexical/src/lib.rs`):
- `search-corpus-generation-manifest.cbor` (`LexicalSealedManifestV1`, 고정 순서 CBOR array):
  format version, identity의 `manifest_digest`(상호 결속), `meta.json` sha256(Tantivy commit
  identity), `text_authority: bool`(명시적 capability — "파일 없음"이 "불필요"인지 "유실"인지
  구분), 각 sidecar `(name, bytes, sha256)`. **sidecar publish → manifest → identity** 순서로
  identity 존재가 durable manifest를 함축한다.
- `verify_lexical_sealed_manifest`: digest 결속, meta.json digest, sidecar 집합/길이/해시,
  capability 일치. `open`(cold, registry 덕에 residency당 1회)과 `validate_generation_identity`
  (activation/restart)가 **같은 함수**를 부른다 — 두 문이 같은 파일 집합을 증명한다.
- seal 시 writer를 **retire**(마지막 commit 후 cache에서 제거). sealed generation에 index
  mutating op(`op_mutates_index`)가 오면 `GENERATION_IMMUTABLE`; overlay op(repo metadata)는
  writer 없이 적용(`apply_snapshot_op`) — snapshot-only op가 writer를 만들지 않으므로 eviction
  commit 경로 자체가 닫힌다.
- delta clone이 base의 identity/manifest를 **상속하지 않는다**(`is_seal_marker_entry`). 이전에는
  base identity를 link한 뒤 지우는 방식이라 그 사이 crash면 "base identity를 가진 gN" 디렉터리가
  남아 boot scanner가 거부했다.
- manifest 없는 sealed generation → `GENERATION_MANIFEST_MISSING`(explicit migration). legacy
  fallback 없음 — 기존 dev state root는 재구축 대상.

**검증** (`crates/quanta-index-lexical/tests/sealed_manifest.rs`, 실제 파일 fault injection):

| 기준 | 검증 |
| --- | --- |
| 각 sidecar의 missing / truncation / bit flip / stale copy에서 activation·open 모두 거부 | `both_doors_refuse_a_sidecar_that_does_not_match_the_manifest` — 5 sidecar × 4 fault, 복원 후 재허용 |
| Tantivy commit 결속 | `both_doors_refuse_an_index_commit_other_than_the_sealed_one` (meta.json에 개행 추가 — Tantivy는 열지만 manifest는 거부) |
| manifest 부재 / identity-manifest 결속 | `..._without_a_manifest_...` (`GENERATION_MANIFEST_MISSING`), `an_identity_that_does_not_match_the_manifest_is_refused` (`GENERATION_IDENTITY_DIGEST_MISMATCH`) |
| seal 후 index 불변 + overlay 허용 | `a_sealed_generation_refuses_index_mutation_but_keeps_its_overlay` |
| sidecar 없는 generation의 명시적 capability | manifest `text_authority=false` + 있으면 안 되는 sidecar 등장 시 거부(`verify` 분기) |
| activation 직후 동일 검증 handle로 query (보완 #3 후반) | **후속** — validator port가 handle을 반환하지 않는다. registry가 있으니 lifecycle이 activation 시 `acquire`로 warm 하는 것은 가능하지만 "재사용"은 port 변경이 필요 |
| fsync/rename crash point 매트릭스 | 부분 — 순서(sidecar → manifest → identity)로 half-promoted 상태가 unsealed로 남는 것은 구조적으로 보장; crash 주입 fixture(§12.2)는 W7 qualification |

## 3.11 QI-BB-029 — cross-track 계약을 mutation 전에 검증한다 (구현 완료)

**진단 확정**: mode/base 쌍을 lexical은 허용하고 semantic은 거부하는 조합(`ReplaceGeneration +
base_generation`)이 있었고, 빈 digest는 build/authority까지 통과한 뒤 activation identity에서만
거부됐다. delta base는 lexical이 "디렉터리 존재"만 보고 clone·seal한 뒤 semantic이 거부할 수 있었다.

**구현**:
- contract `SearchCorpusIngestBatch::validate_v1()` + `SearchCorpusBatchShapeErrorV1` —
  mode/base 형태(Replace는 base 없음, Delta는 base 필수), `base < generation`, `manifest_digest` /
  `batch_digest`는 non-empty printable ASCII token. materializer가 **lock을 잡기 전에** 호출
  (`SEARCH_CORPUS_BATCH_SHAPE_INVALID`).
- materializer `preflight_delta_base_v1` — delta의 base를 **양 track 모두** exact sealed
  identity로 검증한다: digest는 ledger의 sealed-track 기록에서 가져오고(`Ledger::sealed_track_identity_digest`),
  없으면 `SEARCH_CORPUS_DELTA_BASE_NOT_SEALED`(디렉터리가 있어도 신뢰하지 않음), 있으면
  `validate_generation_identity`(lexical은 §3.10 manifest 검증 포함)로 물리 상태를 확인. 어느
  것도 어떤 adapter보다 먼저 실행된다.
- 보완 #3/#4(half-sealed 수렴)는 감사 HEAD 이후 이미 들어와 있던 `SealedGenerationBuildPlanV1`
  (`exact_lexical_missing_semantic_retry_builds_only_missing_track` 등)이 담당 — 이번엔 추가하지 않음.

**검증**:

| 기준 | 검증 |
| --- | --- |
| invalid mode/base, 빈 digest, unsealed base, mismatched base가 lexical·semantic·authority bytes 0개 변경 | `ingest_dispatcher::tests::malformed_or_baseless_batches_change_zero_bytes` — recording fake builder 2개 + recording authority가 전부 비어 있음을 매 거부 후 확인 |
| 계약 형태 검증 | `ipc::ingest::tests::search_corpus_batch_shape_is_validated_before_any_adapter` |
| 재시도 수렴 | 기존 3개 retry test 유지 |

**정직한 비용**: non-seal delta batch도 매번 base를 물리 검증하므로 base sidecar 해시 비용이 batch당
든다(§3.10). registry처럼 검증 결과를 (key, digest)로 기억하는 것은 후속.

## 3.12 QI-BB-002 — IPC admission + cooperative cancellation (phase 1 구현 완료)

**진단 확정**(G0-R baseline, §3.0): `UdsServer::run`이 accept thread에서 connection을 inline 처리해
한 peer의 native search 또는 stalled read가 socket 전체를 막았고, 떠난 peer의 query는 아무도 읽지
않을 결과까지 끝까지 달렸으며, dispatcher는 deadline도 cancellation도 받지 못했다.

**설계** (G0-R 결론 준수 — native call 사이의 hard cancellation은 없고, checkpoint만 있다):

| 층 | 구현 |
| --- | --- |
| ingress | connection당 thread(`uds-connection`). `ServerAdmissionPolicy::max_connections` 초과는 accept에서 **닫고 count**(`refused_connections`), queue하지 않음 |
| dispatch slot | `DispatchSlots`(mutex+condvar counting semaphore). `queue_wait` 안에 slot이 없으면 typed `SERVER_OVERLOADED`(`SearchPlaneIpcError::overloaded(waited, slots)`)를 **요청자 connection에** 회신. envelope type이 typed refusal이 없으면 close(`ConnectionCloseReason::Overloaded`) |
| budget | `core::request_budget::RequestBudgetV1 { deadline, cancelled }` + `CancelHandleV1`. `checkpoint(stage)`는 `REQUEST_CANCELLED` / `REQUEST_DEADLINE_EXCEEDED`를 **checkpoint 이름과 함께** typed로 반환. `IpcDispatcher::dispatch(&self, request, budget)` — budget 없는 dispatch 시그니처는 없음 |
| peer watch | dispatch 중 별도 thread가 `poll(POLLIN\|POLLHUP)` 50ms + `recv(PEEK)`로 hang-up을 감지해 cancel. pipelined data는 hang-up이 아님. watch를 못 세우면(`try_clone`/spawn 실패) dispatch하지 않고 connection을 닫는다(`PeerWatchFailed`) — cancel이 영원히 안 오는 dispatch를 만들지 않음 |
| policy | `ServerAdmissionPolicy::new(max_connections, dispatch_slots, queue_wait, dispatch_budget, io_timeout)` — private field, zero limit / slots>connections 거부. `DEFAULT`(64/4/2s/20s/30s) = query socket, `SERIAL_DISPATCH`(64/1/10s/120s/30s) = control·ingest(mutation 직렬화는 정책이지 튜닝 항목이 아님) |
| searchd | `QUANTA_INDEX_QUERY_MAX_CONNECTIONS` / `_DISPATCH_SLOTS` / `_QUEUE_WAIT_MS` / `_DISPATCH_BUDGET_MS` — 각각 optional, unset은 DEFAULT의 해당 field, 조합은 **하나의 policy로** 검증(boot에서 거부). `QueryServer::bind(..., policy)` |

**checkpoint 배치** (query dispatcher, 모든 route는 core inbound port `*QueryPort::…(request, budget)`로 budget을 받는다):

| route | checkpoint |
| --- | --- |
| text | `lexical:entry` → `lexical:search`(open 후, native 전) → `lexical:project` |
| symbol | `symbol:entry` → `symbol:search` |
| semantic | `semantic:entry` → `semantic:scope`(lexical scope native 전) → `semantic:embed` → `semantic:search` → `semantic:project` |
| hybrid | `hybrid:entry` → `hybrid:lexical` → `hybrid:embed` → `hybrid:semantic` → `hybrid:fuse` |
| hybrid seed | `hybrid-seed:entry` → `:lexical` → `:embed` → `:semantic` → `:dense`(corpus lane마다) → `:fuse` |
| history / runtime / structural | `<route>:entry` → `<route>:execute`; structural의 lexical leaf마다 `structural:lexical-leaf` |
| explain / repo-map / cluster-membership | `:entry` (+ `explain:probe`, `cluster-membership:read`) |
| control / ingest | `control:entry` / `ingest:entry`만 — admission 후 mutation은 dispatcher 소유로 끝까지 간다(반쯤 적용된 activation/batch가 "아무도 안 읽는 답"보다 나쁘다) |

error metric: 두 code는 `lq_typed_error_interrupted_total`(신규, closed taxonomy test에 추가). `other`로 새지 않는다.

**검증**:

| 기준 | 검증 |
| --- | --- |
| in-flight dispatch가 다른 client를 막지 않음 | `g0r_runtime_cancellation_probe::an_in_flight_dispatch_does_not_block_other_clients` — `G0R-EVIDENCE head_of_line served_while_held=ok served_in_ms=1 completions_while_held=1` (W0 baseline에서는 timeout) |
| 떠난 peer가 budget을 cancel | `…::a_disconnected_peer_cancels_its_dispatch_budget` — `cancelled_observed=1`, dispatcher checkpoint가 봄 |
| shutdown drain 유지 | `…::shutdown_drains_the_in_flight_dispatch` |
| slot 고갈 → typed overload, 요청자 connection에, holder를 기다리지 않고, slot 해제 후 재서비스 | `tests/admission.rs::a_full_dispatch_queue_is_refused_with_a_typed_overload_then_serves_again` (실 control envelope, `(1 slots busy)` 메시지) |
| connection cap은 queue가 아니라 close + count | `…::connections_past_the_cap_are_closed_at_accept_and_counted` — mutation(cap 무한)으로 FAIL 확인 후 revert |
| policy deadline이 dispatcher checkpoint까지 도달, checkpoint 이름 포함 | `…::the_dispatch_budget_deadline_reaches_the_dispatcher_checkpoint` |
| 모든 query route가 entry에서 interrupted budget을 typed로 거부, opener 미접촉, metric은 `interrupted` | `query_dispatcher::tests::every_route_refuses_an_interrupted_budget_at_entry_without_opening` (8 route, reject opener) |
| policy 검증 / slot semaphore / budget 단위 | `ipc::admission::tests` 3, `core::request_budget::tests` 4, `ipc::server::tests` 19(overload·peer-hangup 포함), searchd `query_admission_env_binding_…` |

**남은 것 (W5 phase 2)**: (a) cancel을 lexical `collect_bounded` 내부 candidate batch 사이까지 —
현재는 native search 한 덩어리가 끝나야 checkpoint. (b) `refused_connections`/overload/interrupted를
서버 metric으로 (지금은 query dispatcher의 error metric만). (c) `SlotRefusal` 대기 fairness는 condvar
`notify_one` 순서에 맡김 — 명시 FIFO는 필요가 증명되면.

**정직한 한계**: 하나의 connection이 여러 request를 pipelined로 보내면 두 번째 request는 첫 번째 dispatch
가 끝난 뒤 읽힌다(connection thread는 순차). 병렬성은 connection 단위다.

## 3.13 QI-BB-026 — boot inventory + quarantine: boot는 active set만 증명한다 (구현 완료, control surface는 후속)

**진단 확정**: lexical scanner가 모든 sealed generation을 `validate_generation_identity`(§3.10 이후엔 sidecar
sha256까지)로 검증한 뒤 boot seed가 같은 후보를 **다시** 검증했고, semantic scanner는 모든 sealed generation을
`open_generation`(row root + membership 전체 scan)했다. non-canonical directory 하나, 깨진 identity 하나, 아무도
serve하지 않는 과거 generation의 sidecar 손상 하나가 전체 scan error → socket bind 전 boot 실패였다.

**구현**:

| 층 | 변경 |
| --- | --- |
| core | `SealedGenerationScanPort::inventory_sealed_generations() -> SealedGenerationInventoryV1 { sealed, quarantined }`; `QuarantinedGenerationV1 { track, path, reason, detail }`; `GenerationQuarantineReasonV1 { NonCanonicalLayout, IdentityUnreadable, ScopeMismatch, IdentityDigestMismatch }` — inventory가 identity만 보고 판단할 수 있는 결함만. content 결함은 의도적으로 없음 |
| lexical | `inventory_sealed_generations(root)`: generation당 identity 파일 하나만 읽음. legacy layout / 비정규 `g<N>` / decode 실패 / scope mismatch → quarantine(경로+이유), 계속 진행. deep validation 없음. `PersistedLexicalGeneration`·`scan_persisted_generations`·`GENERATION_STORAGE_LEGACY_LAYOUT_UNSUPPORTED` 삭제 |
| semantic | `inventory_persisted_generations(root)`: marker + manifest 두 파일. `open_generation` 호출 제거. `SemanticAdapter: SealedGenerationScanPort`. legacy migration의 durable witness는 `validate_persisted_generation_v2(root, record)`(open 1회)로 분리 — journal이 이름 붙인 generation만 증명 |
| searchd | `app::boot_inventory::{seed_track_readiness, TrackInventoryReportV1, BootInventoryReportV1}` — 두 track이 하나의 seed 경로(mirror 제거). `SearchdRuntimeParts.semantic_generation_scanner` 추가. `SearchdRuntime.boot_inventory` 노출. `validate_rehydrated_active_generations_v1 -> usize`(증명한 active pair 수) |
| harness | `E2eRuntime::start()`(lazy start 대신 boot 거부를 error로), `boot_inventory()`, `query_once()`(readiness retry 없는 단발 query) |

boot 순서: lexical inventory → legacy semantic migration → semantic inventory → **active pair만** physical validation(track당 정확히 1회) → aux authority restore → bind. 비활성 generation의 content는 serve/activate/delta의 각 문(§3.10/§3.11 검증)이 자기 차례에 증명한다.

**검증**:

| 기준 | 검증 |
| --- | --- |
| 손상된 비활성 generation(lexical sidecar bit-flip + semantic manifest row_count 변조) + legacy dir + garbage identity가 있어도 boot 성공, active 1쌍만 증명(`active_pairs_validated=1`), active serve, 비활성 pin query는 각 문에서 typed 거부(`GENERATION_SIDECAR_CORRUPT` / row-count), quarantine pin은 `NOT_READY`, report에 경로+이유 | `e2e_boot_quarantine::damage_to_an_inactive_generation_does_not_stop_the_daemon` — mutation(inventory가 deep validate)으로 FAIL 확인 후 revert |
| 손상된 active generation은 bind 전 typed 거부(`ACTIVATION_TARGET_UNOPENABLE` + `GENERATION_SIDECAR_CORRUPT`), inventory 미노출 | `…::damage_to_the_active_generation_refuses_to_boot` |
| lexical inventory: 4종 quarantine 이유 + in-progress skip + content 미검사(손상된 sidecar를 그대로 목록화, validator가 거부) | `lexical/tests/boot_inventory.rs` 2 tests |
| semantic inventory: legacy layout quarantine; content 변조는 목록화되고 deep witness/open이 거부 | `semantic::tests::inventory_quarantines_legacy_raw_identity_directories`, `persisted_semantic::inventory_lists_a_content_corrupted_generation_and_the_deep_witness_refuses_it`, `semantic_boot::tests::seed_lists_a_content_corrupted_generation_and_open_refuses_it`, `semantic_boot_report::runtime_boot_inventories_a_corrupted_inactive_semantic_generation` |
| seed: 정렬 무관 최고 generation이 head, quarantine은 ledger 부재, cross-track inventory 거부 | `boot_inventory::tests` 2 |

**완료 기준 대비**: (1) 손상 비활성 + 정상 active → boot/query 성공 + quarantine receipt ✓ (receipt는 runtime report, 파일/IPC 아님). (2) 손상 active → bind 전 typed ✓. (3) 동기 boot 검증량 ∝ 필수 set — inventory는 generation당 파일 1–2개, deep은 active pair만 ✓. (4) generation당 deep validation boot당 최대 1회 ✓ (lexical 2회→1회, semantic scan+rehydrate 2회→1회).

**남은 것**: quarantine 조회/삭제/재검증 typed control/CLI surface(finding 보완 #4) — contract IPC variant 추가가 필요해 W2 catalog와 함께. QI-BB-017 본체는 §3.14.

## 3.14 QI-BB-017 — semantic sealed manifest: open은 row가 아니라 file을 다시 잰다 (구현 완료)

**진단 확정**: `open_generation`이 매번 main table 전체를 stream해 `semantic_row_commitment_v1`을 재계산하고,
membership table도 전체 row를 `Vec<String>` DTO로 물화해 root를 재계산했다. seal → activation
validation → first query(cold open) → registry eviction 뒤 재open마다 같은 generation을 전체 scan.
boot도 (§3.13 전) 모든 sealed generation을 open했다.

**설계** (§3.10 lexical과 대칭, 공통부는 core로 승격 — mirror 금지):

| 층 | 구현 |
| --- | --- |
| core | `SealedArtifactCommitmentV1 { name, bytes, sha256 }`, `sha256_of_file`(64 KiB buffer streaming — lexical의 whole-file read도 이걸로 교체, RSS O(1)), `commit_tree_v1(root, prefix)`(재귀, 정렬, symlink 거부), `verify_tree_commitment_v1` → `Ok(Ok(bytes))` / `Ok(Err(TreeCommitmentMismatchV1::{Missing, Extra, Length, Digest}))` — **file set 전체 비교**: versioned dataset에서는 extra file(예: `_versions/999.manifest`)이 무엇이 열리는지를 바꾸므로 extra도 결함 |
| semantic seal | `semantic-sealed-manifest.cbor` = `(format_version, manifest_digest, (len,sha) of semantic-manifest.cbor, (len,sha) of semantic-build-contract.cbor, dataset/ 전체 파일 commitment)`. dataset·contract promote → ready marker → scope manifest → **sealed manifest** → sealed marker 순. marker가 있으면 sealed manifest도 있다. row root/membership root는 seal에서 1회만 계산(기존) |
| semantic open | marker의 digest로 sealed manifest를 **scope manifest를 decode하기 전에** 검증(위조된 manifest는 해석되지 않고 `GENERATION_SIDECAR_CORRUPT`) → manifest decode/scope/digest → table open, schema, `count_rows`(cheap) → membership은 schema + count만. row stream 0회. `resident_bytes_estimate`는 commitment의 합(트리 재순회 없음). legacy format(v2 등)은 sealed manifest가 없으면 종전 경로 유지; **현재 format인데 없으면 `GENERATION_MANIFEST_MISSING`(explicit migration 요구)** |
| semantic validator | marker digest == candidate 확인 후 `open_generation`이 유일한 증명(이전엔 manifest 재decode + open 이중). activation/restart/delta-base preflight 모두 이 경로 |

**cost model (seal 이후 같은 generation)**: row scan 0회. file hash는 door마다 1회(O(bytes) 순차 read, 메모리 상한 64 KiB) — activation 1회 + cold open 1회(registry가 상주시키는 동안 0회). seal은 row root 1회 + file hash 1회.

**검증**:

| 기준 | 검증 |
| --- | --- |
| dataset의 **모든** 파일에 대해 truncate / bit-flip / remove 각각 양 문(`validate_generation_identity`, `open`+search)이 `GENERATION_SIDECAR_CORRUPT`로 거부, 복원 후 재허용; 외부 파일(`_versions/999.manifest`) 주입도 거부 | `semantic/tests/sealed_manifest.rs::both_doors_refuse_a_dataset_file_that_does_not_match_the_manifest` |
| scope manifest / build contract 위조·삭제 거부, sealed manifest 부재는 `GENERATION_MANIFEST_MISSING` | `…::both_doors_refuse_forged_or_missing_sidecars` |
| 다른 generation의 sealed manifest는 `GENERATION_IDENTITY_DIGEST_MISMATCH` | `…::the_sealed_manifest_binds_the_identity_digest` |
| open이 commitment가 알아챌 어떤 것도 쓰지 않음(3회 연속 admit, search 포함) — Lance가 open에서 hint 파일 등을 rewrite하지 않는다는 실증 | `…::a_sealed_generation_is_admitted_repeatedly` |
| seal 후 같은 row 수의 content mutation(Lance update → 새 version 파일)은 row root 재계산 없이 file commitment가 거부 | `build::tests::sealed_manifest_rejects_same_row_count_content_mutation`(구 `semantic_row_root_rejects_…`, 기대 code 변경) |
| 기존 manifest 위조 test 9개(model_id/version/normalization/scope/distance/row_count/contract 삭제/깨진 CBOR)는 모두 **더 이른** typed 거부로 수렴 | `persisted_semantic.rs` — `expect_sidecar_corrupt(err, file)` helper로 통일 |
| core helper: 자기 commitment 검증 + bytes 합, drift 4종을 path 순으로 명명, streaming digest == one-shot digest | `core::domains::generation::tree_commitment_tests` 3 |

**정직한 한계**: (a) seal-time membership commitment는 여전히 row DTO를 정렬용으로 물화한다(finding 보완 #5 streaming accumulator 미적용) — seal은 ingest가 이미 batch를 메모리에 쥔 시점이라 open과 달리 RSS 상한을 새로 넘기지 않는다. (b) legacy format(v2, uncommitted-root)은 sealed manifest 없이 열리는 기존 compat 경로가 그대로다 — 이번 finding 범위 밖, 정리 대상. (c) 현재 format으로 이미 seal된 production generation은 sealed manifest가 없어 `GENERATION_MANIFEST_MISSING` — reseal 필요(§3.10 lexical과 같은 breaking-first 결정; production state root는 건드리지 않음).

## 3.15 QI-BB-028 + QI-BB-031 — embedding identity는 revision을 요구하고, vector는 계약대로다 (구현 완료)

**진단 확정**: cache key가 `(model_id, dimension, text)`뿐이라 같은 model name의 provider revision이 바뀌어도
old hit과 new miss가 한 generation에 섞였다. OpenAI provider는 `model_version() = None`, gate는 양쪽 `None`이면 통과.
cache entry는 raw f32 배열(길이 4의 배수만 검사, checksum/dimension/finite 없음, direct overwrite). 계약은
`L2Unit`을 기록하지만 아무도 norm을 검사·정규화하지 않았다(`[1,0,2]`가 valid로 pin된 test).

**설계**:

| 층 | 구현 |
| --- | --- |
| core `TextEmbeddingProvider` | `model_version() -> Option<&str>` 삭제 → **`model_revision() -> &str` 필수** + `normalization() -> EmbeddingNormalization`(raw provider는 `None`). `SemanticSearcher::index_model_version` → `index_model_revision() -> Option<&str>`(manifest의 값; `None`은 revision 이전 seal) |
| core policy | `SemanticPolicy::validate_embedding_vector_v1(vector, dimension, normalization)` — dimension, finite, nonzero, `L2Unit`이면 `\|norm-1\| ≤ 1e-3`(`L2_UNIT_NORM_TOLERANCE`); `normalize_l2_unit_v1`(f64 accumulate, 결정적, zero/NaN typed 거부); `L2UnitEmbeddingProvider<P>` wrapper — raw output을 validate → normalize → validate, `L2Unit`을 이미 약속한 provider는 stacking 거부 |
| embed OpenAI | `OpenAiProviderConfig::new(api_key, model, **model_revision**, dimension)` — 빈/비ASCII revision 거부. `normalization() = None`(raw) |
| embed cache | `EmbeddingCacheIdentityV1 { model_id, revision, dimension, normalization }` → namespace dir(`sha256` 16 hex) + key(`sha256(domain, identity, text)`). entry format v2: `QIEC` magic + u16 format + u32 dimension + f32 LE payload + 16-byte truncated sha256; temp-write+fsync+rename+parent fsync; decode 실패 파일은 삭제. hit도 fresh output도 같은 validator; 통과 못한 hit은 evict+recompute, 통과 못한 fresh는 batch 실패(cache에 안 들어감). legacy flat layout read 삭제 |
| search-plane | hash embedder revision `fnv1a64-slots-l2unit-v1`, 정규화를 공용 `normalize_l2_unit_v1`로 교체(`L2Unit` 약속). derive는 provider가 `L2Unit`이 아니면 InvalidContract(계약을 `None`으로 적지 않음), `model_version: Some(revision)`. gate: index revision `None`이면 `SEM_MODEL_MISMATCH`("reseal") — 추정으로 통과시키지 않음 |
| semantic ingest | `validate_replace_scope`가 batch contract의 normalization으로 모든 row를 `validate_embedding_vector_v1` — `l2_unit`으로 seal된 generation은 row bytes를 정직하게 설명. fixture helper는 주어진 방향을 정규화(runtime wrapper와 동일 코드) |
| searchd | `QUANTA_INDEX_EMBED_MODEL_REVISION` — openai profile에 **필수**(default 없음, unset/blank/whitespace는 boot 거부). 구성: `Caching(L2Unit(OpenAI))` — cache는 정규화된 vector를 identity namespace에 저장 |

**검증**:

| 기준 | 검증 |
| --- | --- |
| 같은 model id/dimension/text에서 revision만 다른 provider가 old entry를 재사용하지 않음(+ model id 변경, namespace 분리) | `embed::cache::tests::a_revision_or_model_change_never_reuses_a_cached_vector` |
| cache entry truncation / payload bit-flip / digest bit-flip / wrong magic / legacy raw floats / empty → miss + 파일 삭제 | `…::file_cache_refuses_and_removes_damaged_entries` |
| decode는 되지만 계약 위반 hit(0.5 scale, NaN, dimension, zero) → evict + recompute + 교체 | `…::an_unusable_hit_is_evicted_and_recomputed` |
| 계약을 어기는 fresh output은 batch 실패, cache 오염 0 | `…::a_provider_that_breaks_its_own_contract_poisons_nothing` |
| 동시 writer 8×50 — 항상 온전한 entry | `…::concurrent_writers_leave_a_whole_entry` |
| revision 없는 openai profile은 boot 거부, whitespace/blank 거부 | `searchd::config::tests::openai_model_revision_is_required_and_must_be_a_token` |
| gate: revision drift 거부, index revision `None` 거부 | `query_dispatcher::tests` gate test 갱신 |
| norm 0 / NaN / Inf / 0.5 / 2.0 / dimension: validator가 typed 거부, wrapper가 batch 실패 또는 정규화 — corpus/query 양쪽이 같은 wrapper | `core/tests/semantic_policy.rs::vector_contract` 4 tests(1536-dim 결정성 + tolerance 포함) |
| ingest row가 `L2Unit` 계약을 어기면 typed `SEM_INVALID_VECTOR`(embedding id 명시) | `persisted_semantic` dimension mismatch 2 test 기대 갱신; 비단위 fixture(`[0.9,0.1,0]`) 1건 교정 |

**남은 것**: finding 보완 #4(activation receipt에 semantic row root/ANN contract attestation)는 QI-BB-027과 함께. 기존 `embed-cache/<shard>/` v1 entry는 읽히지 않고 남는다 — 회수는 QI-BB-009(cache retention) 항목.

## 3.16 QI-BB-032 — `batch_digest`는 하나의 immutable body를 이름하고, 한 번만 적용된다 (W2 catalog 착수, 구현 완료)

**진단 확정**: ack 유실 후 같은 batch 재전송을 duplicate로 판별하지 못해 derive/embed/build를 다시 했고, 같은
key에 다른 body가 와도 거부되지 않았다. receipt는 `manifest_digest`만 돌려주고, aux route는 그 field에
`batch_digest`를 넣어 route마다 의미가 달랐다.

**설계** (G0-C 결정 조건 준수):

| 층 | 구현 |
| --- | --- |
| core `domains::idempotency` | `IdempotencyCatalogPort { begin(key, body_sha256) -> Fresh \| Replay{receipt, seq} \| Resume, finalize(key, body, receipt) -> seq, forget_generation }`, `IdempotencyKeyV1 { kind, repo, revision, generation, batch_digest }`, `IngestOperationKindV1`(receipt를 내는 11 route), typed `BATCH_DIGEST_CONFLICT` / `CATALOG_ROW_CORRUPT` / `CATALOG_BUSY` |
| `quanta-index-catalog` (신설, rusqlite bundled) | `state_root/catalog/catalog-v1.sqlite`: `idempotency_v1(kind, repo, revision, generation, batch_digest PK; body_sha256; applied; receipt_cbor; durable_sequence; row_sha256) WITHOUT ROWID` + `catalog_sequence_v1`. open 시 `journal_mode=WAL`, **`synchronous=FULL`**, `fullfsync=ON`을 요청하고 **읽어서 확인**(WAL/FULL 아니면 open 거부). 모든 row는 자기 digest(`row_sha256`)를 갖고 read마다 검증 → 불일치는 `CATALOG_ROW_CORRUPT`. `DatabaseBusy/Locked`는 `CATALOG_BUSY`(busy_timeout = caller budget). sequence는 finalize transaction 안에서 할당(전역 단조) |
| search-plane dispatcher | receipt를 내는 11 route 전부 `publish_idempotent(kind, key, body, apply)`: canonical body hash = `sha256("quanta-index:ingest-batch-body:v1", kind, CBOR(batch))` → `begin` → Replay면 저장 receipt를 `applied=false`로 회신(adapter·embedder 호출 0) → 아니면 apply → `finalize` → `applied=true` + sequence. Resume(crash 후)은 재적용(모든 route가 op별 idempotent; seal은 `finalize_only` 수렴). materializer의 physical GC가 양 track에서 회수한 generation의 record를 `forget_generation` |
| contract `BatchPublishReceipt` | `manifest_digest: Option<String>`(aux route는 `None` — field overloading 제거), `batch_digest`, `applied`, `durable_sequence` 추가; 모든 field 필수(legacy clear-surface default 삭제). SDK는 `batch_digest` 일치도 검증 |
| composition | `SqliteIdempotencyCatalog::open(state_root, 2s)` → `SearchdRuntimeParts.idempotency` → dispatcher + materializer. ingest socket은 SERIAL_DISPATCH(§3.12)라 같은 key의 동시 publish는 직렬화되어 하나가 apply, 나머지는 Replay |
| 삭제 | `quanta-index-catalog-probe` crate, `redb` workspace dep, 관련 Justfile/authority/hexagonal 항목 (G0-C 증거는 ADR에 기록됨) |

**검증**:

| 기준 | 검증 |
| --- | --- |
| 같은 body replay → 한 번만 apply, 두 번째 receipt는 원 apply의 counts/sequence + `applied=false`; 같은 digest 1-byte 변경 body → mutation 전 `BATCH_DIGEST_CONFLICT`; 거부된 body는 serve되지 않음 | `e2e_ingest_idempotency::a_replay_is_acked_from_the_record_and_a_conflict_never_lands` (실 SQLite, ingest socket) |
| 8 concurrent duplicate publish → applied 정확히 1, 모든 receipt 같은 sequence, corpus row 1개 | `…::concurrent_duplicate_publishes_converge_on_one_apply` |
| daemon restart 후 replay도 Replay(원 sequence) | `…::a_replay_after_restart_is_still_a_replay` |
| dispatcher: route 호출 1회, conflict는 route 미도달, key당 record 1개, 새 key는 다음 sequence | `ingest_dispatcher::idempotency_tests::…` — Replay short-circuit mutation으로 FAIL 확인 후 revert |
| catalog: fresh→finalize→replay, conflict는 무기록, 이중 finalize 거부; crash-before-finalize → Resume → finalize; row digest bit-flip → `CATALOG_ROW_CORRUPT`; sequence 1..4 단조(재open 포함); 외부 writer가 lock 보유 → `CATALOG_BUSY`(60ms budget 준수); forget_generation은 해당 generation만 | `catalog/tests/idempotency.rs` 6 tests |
| receipt wire shape(모든 field 필수, legacy receipt decode 거부) | `contract` receipt round-trip + `legacy_clearless_batches_decode_as_zero_clear_and_receipts_do_not_v1`; public-api baseline 갱신 |

**정직한 한계**: (a) repo-map bundle route는 receipt가 아니라 `RepoMapMutationAck`를 내고 batch digest가 없어 catalog 밖 — 계약이 receipt 형태로 바뀔 때 편입. (b) caller의 `batch_digest`와 server body hash를 **대조**하지는 않는다(producer digest scheme이 다름) — 같은 key 아래 body 불변만 강제. (c) Resume 시 unsealed batch의 재적용은 op별 overwrite semantics에 기대며, 별도 "이미 적용됨" 판정은 없다.

## 4. Finding 상태 (QI-BB-001–032)

초기값은 findings.md 확정 상태 그대로이며 owner 배정만 기록한다.

<!-- FINDINGS-TABLE -->

## 5. 실행 command 기록

<!-- RUN-LOG -->

| 시각 (host) | HEAD | command | 결과 |
| --- | --- | --- | --- |
| 2026-09-16 | 2cdd9ec | `just rust-profile verify-rust` | RED — `end_to_end::hybrid_query_rejects_zero_top_k_with_typed_code`가 폐기된 `HYB_TOP_K_INVALID`를 pin (→ 5129bf4에서 shared code로 교체) |
| 2026-09-17 | 966cf1e | `just rust-profile verify-rust` | RED — 새 test file 2개의 lint (→ 4741872, aa8dca9) |
| 2026-09-17 | 4741872 | `just rust-profile verify-rust` | RED — `quanta-index-embed` flaky (IMPL-H) |
| 2026-09-17 | (dirty tree) | `just rust-profile verify-rust` | 무효 — GC 작업 중 tree를 컴파일함 |
| 2026-09-17 | 21e7d26 | `just rust-profile verify-rust` | **GREEN** — exit 0, 1,981 passed / 0 failed (clippy·semgrep·deny·machete·doc·policy·public-api·hexagonal·test-authority 포함) |
| 2026-09-17 | a9b0d90 | `just rust-profile verify-rust` | RED — `e2e_restart_replay_determinism`: history-only generation seal이 index를 만들지 않아 manifest가 meta.json을 못 찾음 (→ 022e80b) |
| 2026-09-17 | 2504b33 | `just rust-profile verify-rust` | **GREEN** — exit 0, 1,989 passed / 0 failed |
| 2026-09-17 | (W5 tree) | `cargow --lane test-integration-lane test -p quanta-index-ipc --test g0r_runtime_cancellation_probe` | 3/3 — `head_of_line served_while_held=ok … completions_while_held=1`, `disconnect_cancels … cancelled_observed=1` |
| 2026-09-17 | (W5 tree) | `… --test admission` | 3/3; cap mutation → 1 FAIL, revert |
| 2026-09-17 | 120ee7c | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,001 passed / 0 failed (W5 phase 1 포함) |
| 2026-09-17 | 1a2f440 | `just rust-profile verify-rust` | RED — 2,006 passed / 0 failed, `rust-doc`에서 stale intra-doc link(`scan_persisted_generations`) (→ 다음 commit) |
| 2026-09-17 | ae54693 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,013 passed / 0 failed (QI-BB-026 + QI-BB-017 포함) |
| 2026-09-17 | 880a5c3 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,021 passed / 0 failed (QI-BB-028 + QI-BB-031 포함) |
| 2026-09-17 | (W2 tree) | `just rust-fuzz-smoke 30` | 4 target × 30s, crash 0 (receipt wire DTO 변경에 대한 fail-closed 확인) |
| 2026-09-17 | cb7bbd9 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,016 passed / 0 failed (QI-BB-032 catalog 포함) |
