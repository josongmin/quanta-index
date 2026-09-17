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
| IMPL-J | P2 (W5 phase 1 회귀, 확정) | `PeerWatch`가 peer의 **half-close**(`shutdown(Write)` 후 응답 대기)를 hang-up으로 분류해 응답을 쓰지 않고 연결을 닫음. Darwin은 half-close에도 `POLLHUP`을 보고하므로 poll만으로는 구분 불가 | **fixed** — §0.2 IMPL-J |

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

### IMPL-J 상세 — peer watch의 half-close 오분류

`just rust-profile verify-rust`(dd2246b)에서 `quanta-index-ipc`
`handle_connection_returns_peer_closed_after_successful_round_trip`이 `ipc frame truncated`로 간헐 실패.
테스트의 client는 요청을 쓴 뒤 `shutdown(Write)`하고 응답을 기다린다(정당한 one-shot 프로토콜). W5 phase 1의
`PeerWatch`는 `POLLHUP` 또는 EOF(peek 0 byte)를 hang-up으로 취급해 budget을 cancel하고 **응답을 쓰지 않은 채**
연결을 닫았다. dispatch가 watch의 첫 poll(50ms)보다 먼저 끝나면 통과, contended host에서 순서가 뒤집히면 실패 —
즉 flaky가 아니라 race로 드러난 실제 결함.

probe(`scratchpad/pollprobe`)로 Darwin 의미를 확정: peer `SHUT_WR` → 우리 쪽 `POLLIN|POLLHUP`, peer 완전 close →
`POLLHUP`. **poll로는 구분 불가.** 반면 0-byte `send`는 half-close에서 `Ok(0)`, 완전 close에서 `EPIPE` —
peer의 read side 생존 여부를 정확히 가른다(Linux도 동일).

**수정**: `peer_state`는 HUP/ERR/EOF를 보면 `peer_can_receive`(0-byte send, Linux `MSG_NOSIGNAL`, Darwin은
`SO_NOSIGPIPE` + Rust runtime의 SIGPIPE ignore)로 확인 — 받을 수 있으면 `HalfClosed`, 아니면 `HungUp`. half-close
이후에는 EOF가 계속 readable이라 poll 대신 50ms sleep + probe로 완전 close를 기다린다.

**회귀**: `server::tests::a_half_closed_peer_is_not_a_hang_up_and_still_gets_its_response` — dispatch를 gate로
잡아 watch가 half-close를 여러 번 보게 한 뒤 응답 수신·budget 미취소·`PeerClosed`를 확인(수정 전 결정적 FAIL).
기존 `a_peer_that_hangs_up_mid_dispatch_cancels_the_budget`과 G0-R probe 3/3은 그대로 green. 15회 반복 green.

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
| W2 | in_progress | QI-BB-029 preflight(§3.11) + QI-BB-026 boot inventory/quarantine(§3.13) + QI-BB-032 idempotency catalog(§3.16) + **QI-BB-020 auxiliary authority rows(§3.19, catalog 확장: per-record row, validate→persist→apply, Arc snapshot read, retention prune, legacy 일회 migration)** 완료. 남은 것: quarantine control surface, aux read epoch/visibility interval(§3.19 한계) |
| W3 | in_progress | lexical hard-link(§3.4) + sidecar 증분(§3.4.1) + semantic hard-link(§3.4.2) + physical GC(§3.9) + lexical sealed manifest(§3.10) + semantic sealed manifest/QI-BB-017(§3.14) + QI-BB-021 ingest resource envelope(§3.18) + QI-BB-027 ANN sealed contract(§3.23) + **QI-BB-016 lexical writer envelope(§3.26)** 완료. 남은 것: sharded sidecar 포맷(O(delta) write), seal마다 ANN 전체 재구축(O(N), §3.23 한계), scope 단위 streamed embed→append(§3.18 한계) |
| W4 | in_progress | QI-BB-004 scope cap(§3.6) + SnapshotRegistry(§3.7) + QI-BB-005 execution budget(§3.8) + QI-BB-024 regex cache bounds(§3.17) 완료. 남은 것: QI-BB-025 보완 #4(bounded window), streaming projection collector |
| W5 | in_progress | QI-BB-002 phase 1(§3.12) 완료: per-connection thread + bounded dispatch slot + typed overload + cooperative `RequestBudgetV1`(deadline/cancel) + peer watch. QI-BB-014 UDS/state-root private hardening(§3.27) 완료. QI-BB-015 metrics 집계 + scrape(§3.28) 완료. **phase 2(§3.29): budget이 lexical native collect/scan/regex verify/predicate scope 안에서 관측** 완료. 남은 것: shared mode(group/ACL + peer credential), semantic lane 내부 관측 |
| W6 | in_progress | QI-BB-028 + QI-BB-031 embedding identity/vector invariant(§3.15) + QI-BB-009 embedding cache retention/telemetry bound(§3.18) + QI-BB-023 history recency order + keyset cursor(§3.20) + QI-BB-019 hybrid seed 단일 canonical 응답(§3.21) + QI-BB-018 true hybrid(§3.22) + QI-BB-022 explain = exact presence + lexical score trace(§3.24) + **QI-BB-008 RepoMap bounded query + durable store(§3.25)** 완료. 남은 것: QI-BB-007(M4: production profile 측정 후), history relevance order(Tantivy history index, §3.20 한계), judged corpus recall/NDCG gate(§3.22 한계), hybrid 후보의 per-lane contribution(§3.24 한계) |
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

**남은 것**: finding 보완 #4(activation receipt에 semantic row root/ANN contract attestation)는 QI-BB-027과 함께. 기존 `embed-cache/<shard>/` v1 entry는 §3.18(QI-BB-009)에서 open 시 회수한다.

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

## 3.17 QI-BB-024 — regex match cache는 bytes와 cardinality로 bounded되고, hit은 공유한다 (구현 완료)

**진단 확정**: `RegexMatchCache`는 entry 128개만 제한, value는 후보 ID 전체 `BTreeSet<String>`, hit마다
deep clone, insert도 clone 보관 → broad regex 128개 × N doc.

**구현**:
- core `RegexMatchCachePolicy { max_entries, max_resident_bytes, max_matches_per_entry }`(private field, zero
  거부; `DEFAULT` 128 / 64 MiB / 250,000 = `LexicalExecutionBudgetV1::DEFAULT`와 같은 폭) + `RegexMatchCacheStats
  { hits, misses, entries, resident_bytes, evictions, refused_cardinality, refused_bytes }`.
- lexical `RegexMatchCache`: `Arc<BTreeSet<String>>` 값, byte 가중 LRU(id bytes + entry당 64B overhead),
  policy보다 넓은 결과는 **serve하되 cache하지 않음**(`refused_cardinality`/`refused_bytes` 카운트), insert는 entry·byte
  bound 둘 다 만족할 때까지 LRU evict, generation invalidation은 bytes를 되돌려줌. hit은 `Arc::clone` — 후보 id
  deep clone 0. `collect_matching_candidate_ids_for_regex -> Arc<BTreeSet<String>>`.
- `LexicalAdapter::with_state_root_and_policies(root, RegexPolicy, budget, **RegexMatchCachePolicy**)` +
  `regex_match_cache_stats()`. searchd env `QUANTA_INDEX_REGEX_CACHE_MAX_{ENTRIES,RESIDENT_BYTES,MATCHES_PER_ENTRY}`
  (각각 optional, unset은 DEFAULT field, zero 거부).

**검증**:

| 기준 | 검증 |
| --- | --- |
| broad regex(6/6 doc, policy 2)는 정확히 serve되지만 cache 안 됨(`refused_cardinality=1, entries=0`); narrow는 cache되고 반복은 hit, resident bytes 불변, 답 동일 | `lexical/tests/regex_cache_bounds.rs::a_broad_regex_is_served_but_not_cached_and_a_narrow_one_is_a_shared_hit` |
| entry 2개분 byte bound 아래 distinct regex 6개 → 매 query 후 `resident_bytes ≤ policy`, entries=2, evictions=4, evict된 regex는 miss로 정확히 재계산 | `…::resident_bytes_never_exceed_the_policy_under_many_distinct_regexes` |
| 단위: cardinality/bytes 거부·카운트, LRU eviction, `Arc::ptr_eq` hit 공유, invalidation이 bytes 반환 | `regex_match_cache_tests::regex_match_cache_is_byte_bounded_and_shares_hits`, `…_invalidates_only_target_generation` |
| env knob 계층화·zero 거부 | `searchd::config::tests::regex_match_cache_env_binding_layers_over_the_default_and_refuses_zero` |

**정직한 한계**: (a) 후보 restriction은 여전히 `candidate_restriction_query`가 id마다 term query를 만드는 O(matches) —
compact doc-id bitmap(보완 #2)과 streaming collector는 W4 잔여. (b) stats는 adapter accessor뿐, lq-obs metric export는
QI-BB-015와 함께. (c) 100만 doc 규모 RSS 측정은 미실행(contended host) — bound는 정책적으로 증명, 수치는 W7.

## 3.18 QI-BB-009 + QI-BB-021 — embedding resource envelope: batch가 확장될 bytes는 적용 전에 재고, cache와 telemetry는 bounded다 (구현 완료)

**진단 확정**:
- (021) IPC 16 MiB frame cap은 wire bytes만 제한한다. 짧은 record N개는 `N × dim × 4` bytes vector로 확장되고,
  `QUANTA_INDEX_EMBED_DIM`/`_CONCURRENCY`에는 상한이 없었다. 반면 `OpenAiProviderConfig`는 zero 값을 `max(1)`로 **조용히
  치환**했다(default substitution). Lance record batch는 metadata column 20개를 `Vec<String>`으로 한 번 복사한 뒤
  Arrow buffer로 다시 복사했다.
- (009) file cache는 entry/byte/namespace cap이 없고, in-memory cache는 unbounded `BTreeMap`, telemetry request sample은
  process-lifetime `Vec`. v1 shard dir(`embed-cache/<2hex>/`)은 §3.15 이후 읽히지 않는 dead bytes였다.

**구현**:
- core `ingest_resource.rs`: `IngestResourcePolicy { records, text_bytes, vector_bytes }`(zero 거부; `DEFAULT` 100,000 /
  64 MiB / 256 MiB), `MAX_EMBEDDING_DIMENSION = 8_192`, `INGEST_RESOURCE_BUDGET_EXCEEDED`,
  `admit_search_corpus_batch(batch, dimension) -> IngestBatchFootprint { carried_records, embedded_records, text_bytes,
  vector_bytes }`. footprint는 derivation과 같은 규칙: typed semantic source가 있으면 그것이 embedded set, 없으면 legacy
  chunk text. records ceiling은 둘 다(플레인이 쥐는 row 전부) 센다. dimension이 `1..=MAX` 밖이면 batch 거부가 아니라
  composition defect(`InvalidContract`).
- search-plane `DirectSearchCorpusMaterializer`: `validate_surface_mutations_v1` 직후, operation lock·preflight·어떤 track
  mutation보다 앞에서 `admit_resource_envelope` — 거부는 typed, zero bytes. `IngestResourceStats { admitted, refused,
  peak_embedded_records, peak_text_bytes, peak_vector_bytes }` + `resource_stats()`. parts에 `resource_policy` 추가.
- searchd config: `QUANTA_INDEX_INGEST_MAX_{RECORDS,TEXT_BYTES,VECTOR_BYTES}`; `QUANTA_INDEX_EMBED_DIM`은 `1..=8192`,
  `QUANTA_INDEX_EMBED_CONCURRENCY`는 `1..=MAX_CONCURRENCY(64)` 밖이면 boot 거부. embed `OpenAiEmbeddingProvider::new`는
  zero batch/token budget/concurrency와 범위 밖 dimension·concurrency를 typed로 거부 — `max(1)` 치환 제거.
- semantic `build_record_batch`: metadata 20 column을 `StringArray::from_iter_values`/`collect::<StringArray>`로 borrowed
  값에서 직접 build — scope당 `Vec<String>` 중간 복사 20개 제거. vector column만 flat copy.
- embed cache (QI-BB-009): typed `EmbeddingCacheKey([u8; 32])`(hex 64자, lowercase만 parse), `EmbeddingCacheRetentionPolicy
  { entries, resident_bytes, namespaces }`(zero 거부; `DEFAULT` 500,000 / 2 GiB / 4), 공용 `RetentionLedger<V>`(BTreeMap
  key→slot + BTreeMap tick→key, O(log n) LRU, byte·entry 이중 bound, oversize refuse). `FileEmbeddingCache::new(root,
  identity, policy)`는 open 시 (a) v1 legacy shard dir 회수, (b) `.opened` marker mtime 기준 최근 `namespaces-1`개 외 namespace
  retire(marker 없는 namespace는 epoch=가장 오래된 것으로 취급 — dir mtime fallback 없음), (c) namespace scan으로 ledger 재구성
  (recency = write mtime), (d) `.tmp-` staging 잔재 제거, (e) 좁아진 policy면 open 시 evict. `get`은 ledger가 authoritative
  (absent → disk probe 없이 miss), 읽기는 lock 밖, decode 실패는 lock 안에서 slot·file 제거 + `corrupt_misses`. `put`은
  rename 후 lock 안에서 실제 on-disk len으로 계정하고 같은 lock에서 evict. `evict`도 slot→file 순서로 lock 안. `stats()`가
  trait에 추가(`EmbeddingCacheStats` 8 field), `open_report()`가 open 시 사실 5개. `InMemoryEmbeddingCache::bounded(policy)`.
  env `QUANTA_INDEX_EMBED_CACHE_MAX_{ENTRIES,BYTES,NAMESPACES}` → `OpenAiEmbedderTuning::cache_retention`.
- telemetry: request sample은 `VecDeque` ring(`REQUEST_SAMPLE_CAPACITY = 256`) + `request_samples_dropped`,
  `max_request_texts`/`max_estimated_tokens`는 atomic `fetch_max`로 정확. harness A/B report는 sample에서 max를 유도하지
  않고 snapshot 값을 쓴다(`schema_version: 2`).

**검증**:

| 기준 | 검증 |
| --- | --- |
| footprint 규칙(chunk-only / typed source 우선 / carried=둘 다), 3 ceiling이 bound에서 admit·+1에서 typed 거부, dimension이 vector bytes를 곱함, 범위 밖 dimension은 InvalidContract, 빈 batch는 최소 policy에도 fit | `core/tests/ingest_resource_policy.rs` 7건 |
| materializer: vector bound −1에서 typed 거부 + lexical/semantic builder·authority 0 touch + `refused=1`; 정확히 맞는 policy에서 admit + peak 3종 == 직접 계산값. **mutation**: admit을 `Ok(())`로 바꾸면 FAIL(receipt applied) → revert | `search_plane::ingest_dispatcher::tests::a_batch_outside_the_resource_envelope_changes_zero_bytes` |
| e2e: 1-record 폭 envelope에서 2-record batch → `INGEST_RESOURCE_BUDGET_EXCEEDED`, `indexes/{lexical,semantic}/<pair>/` 미생성; 같은 daemon에서 1-record batch는 applied → seal → query 1건. records=1 envelope에서 harness batch(chunk+source=2 row) 거부 | `searchd-runtime/tests/e2e_ingest_resource_envelope.rs` 2건 |
| provider: zero batch/token/concurrency, `MAX_CONCURRENCY+1`, dim 0/`MAX+1` 거부, bound 값은 ok | `embed::openai::tests::out_of_range_tuning_is_refused_at_construction` |
| config: `EMBED_DIM` 0/`MAX+1` 거부·`MAX` ok; `EMBED_CONCURRENCY` `MAX+1` 거부·`MAX` ok; cache retention·ingest policy env 3-knob 각각 binding + unset=DEFAULT + zero 거부 | `searchd::config::tests::{dim_knob_parses_and_rejects_garbage, tuning_assembler_propagates_a_bad_knob_as_error, cache_retention_env_binds_each_knob_and_refuses_zero, ingest_resource_env_binds_each_knob_and_refuses_zero}` |
| file cache: entry 3개분 byte bound에서 8 put → 매 put 후 `resident_bytes ≤ policy`이고 ledger == on-disk(entries·bytes), 최종 entries 3/evictions 5/puts 8, 오래된 5개 miss·최근 3개 hit; entry-count bound 단독 | `embed::cache::tests::resident_bytes_and_entries_never_exceed_the_policy` |
| hit이 recency를 갱신(read된 first가 살아남고 second가 evict) | `…::a_hit_makes_an_entry_the_newest` |
| oversize entry는 쓰지 않고 `refused_oversize=1`, 같은 key의 이전 entry도 drop | `…::an_oversize_entry_is_refused_and_never_written` |
| restart: reopen이 ledger를 디렉터리에서 재구성(6 entries, bytes 일치), 좁힌 policy(2)로 reopen 시 mtime 오래된 4개 evict(파일로 확인) | `…::reopening_rebuilds_the_ledger_and_a_tighter_policy_evicts_the_oldest_writes` |
| open: legacy shard 회수 1, namespace 3+current에 policy 3 → 가장 오래 전 open된 1개 retire, staging 2개 제거, foreign dir 보존 | `…::opening_removes_stale_staging_legacy_shards_and_surplus_namespaces` |
| concurrency: policy 4 entries, writer 4×40 put vs reader 4 loop — served vector는 항상 key의 기대값, 종료 후 entries==4, 4개 모두 정확히 serve, ledger == disk | `…::concurrent_eviction_never_serves_a_wrong_or_torn_vector` (+ 기존 `concurrent_writers_leave_a_whole_entry`에 ledger==disk 추가) |
| damaged entry 6종 → miss + 파일 제거 + `corrupt_misses=6` + ledger==disk | `…::file_cache_refuses_and_removes_damaged_entries` |
| key hex round-trip, uppercase/짧은/긴/non-hex 거부 | `…::a_key_round_trips_through_its_hex_and_only_lowercase_parses` |
| in-memory bounded(entries/bytes/oversize) | `…::in_memory_cache_is_bounded_by_entries_and_bytes` |
| telemetry ring: CAPACITY+50 record 후 `len == 256`, dropped Δ ≥ 50, max 정확 | `embed::telemetry::tests::request_samples_are_a_bounded_window_with_exact_maxima` |
| semantic metadata column 재작성 회귀 | 기존 `scv2_02_v4_round_trip_preserves_metadata_fields` 등 semantic 91건 green |

**정직한 한계**: (a) 021 보완 #2(scope 단위 embed→validate→Lance append streaming으로 `all_vectors` 미보유)는 미구현 —
resident peak는 **정책으로 bounded**(`max_vector_bytes` + provider in-flight `concurrency × max_batch`)이지 streaming으로
줄인 것이 아니다. `SemanticIngestBatch`가 wire DTO라 lazy scope iterator를 실을 수 없어 in-process port 분리가 필요, W3
잔여로 명시. (b) peak RSS 실측(완료 기준 1)은 contended host라 미실행 — W7. (c) cache stats/ingest stats는 accessor뿐,
lq-obs export는 QI-BB-015와 함께. (d) file cache recency는 restart를 건너면 write order로 근사(hit 시 mtime touch 안 함 —
hit당 write를 피하기 위한 선택). (e) namespace retire는 open 시점에만 — 실행 중 다른 identity를 여는 일은 없다.

## 3.19 QI-BB-020 — auxiliary authority는 row로 durable하고, 읽기는 snapshot, 쓰기는 변경량에 비례한다 (구현 완료)

**진단 확정**: history/runtime/structural authority가 하나의 `Arc<RwLock<Ledger>>` 안 `BTreeMap`이었고 (a) 1-row
mutation마다 세 domain 전체 map을 clone→CBOR→3 file rewrite(`persist_from_ledger`), (b) memory에 먼저 적용된 뒤 persist가
실패하면 memory≠disk, 세 file은 하나의 transaction이 아님, (c) history/runtime query는 global read guard를 쥔 채 scan, structural
`execute`는 매 query마다 state 전체 `.cloned()`, (d) aux generation은 retention에서 영원히 제거되지 않음, (e) search-corpus batch가
넣는 structural chunk universe는 다음 aux ingest가 우연히 `persist_from_ledger`를 호출하기 전까지 durable하지 않았음(restart 후
runtime metadata query가 chunk universe 부재로 not-ready — 잠재 결함, 이번에 함께 닫힘).

**구현**:
- core `domains/auxiliary.rs`: `AuxiliaryAuthorityCatalogPort { apply(batch)→receipt, for_each_row, track_rows,
  forget_generation }`, `AuxiliaryDomainV1 {History,Runtime,Structural}`, `AuxiliaryRowFamilyV1` 13종(commit/ref/tag/diff-hunk/
  dirty-doc/changed-doc/doc-facet/snapshot/affected-docs/invalidated-by-docs/chunk/parse-tree/state-meta), row key = `(domain,
  repo, revision, generation, family, row_key bytes)`, value는 opaque bytes(search-plane가 encoding 소유), `AuxiliaryRowMutationV1
  { Upsert | Delete | ClearFamily }`, track row `(repo, revision, track)`.
- catalog crate 재구성: `SqliteCatalog`(단일 connection/파일, `connection.rs`)가 `IdempotencyCatalogPort`와
  `AuxiliaryAuthorityCatalogPort`를 모두 구현. `auxiliary_rows_v1`/`auxiliary_tracks_v1`(WITHOUT ROWID, row_sha256 = 주소+길이 prefix
  key+value 위 digest, 읽을 때마다 검증), batch는 `IMMEDIATE` transaction 하나(전부 아니면 무), `synchronous=FULL`. receipt는 engine이
  실제 쓴/지운 row 수.
- search-plane `auxiliary_authority.rs`: batch → **delta**(validate + 변경 record 명명) → rows. `history_transition`(parent unknown /
  ref→unknown commit 검사, batch 내 앞선 commit도 known), `runtime_dirty_transition`, `runtime_catalog_transition`(chunk universe·epoch
  order 검사, 전 family replace = ClearFamily+upserts), `structural_transition`(parse tree vs chunk 검증, seal_requested·structural
  track state를 delta에 계산), `structural_chunks_transition`(search-corpus batch의 chunk 변경). 각 delta의 `*_delta_rows`, 전체
  state의 `*_state_rows`(migration), `restore_row_into`/`restore_track_row_into`(boot). dirty/chunk delta도 generation의
  `state-meta` row를 항상 동반한다 — net-zero batch(upsert 후 evict, chunk 없는 seal)로 **비어 있지만 published된** generation이 restart
  후에도 materialized로 남아야 하기 때문(`e2e_restart_replay_determinism::reopen_preserves_runtime_dirty_evict_empty_state`가 잡아냄).
- `Ledger`: aux map 값이 `Arc<State>` — `*_snapshot()`은 read lock 아래 `Arc::clone`만, mutation은 `Arc::make_mut`(reader가 이전
  snapshot을 쥔 동안에만 그 generation 하나를 copy). `apply_*_delta` 5종, `forget_auxiliary_generation`,
  `auxiliary_generations_older_than`, `track_state`/`restore_track_state`. 기존 `apply_*_batch`는 transition+apply(in-memory 전용,
  tests).
- materializer 프로토콜(`AuxiliaryMaterializerParts { catalog, coordinator, ledger }`, `AuxiliaryMutationCoordinator` mutex로 aux
  mutation 직렬화): coordinator lock → read lock에서 transition(validate) → `catalog.apply`(durable) → write lock에서 apply_delta →
  receipt. **validation 실패는 catalog에 닿지 않고, catalog 실패는 memory에 닿지 않는다.** search-corpus `finalize_generation_v1`도
  같은 순서로 chunk rows를 durable하게 한 뒤 track/seal 처리, retention receipt가 retain하지 않는 **더 오래된** aux generation을
  memory·catalog에서 forget(staging 중인 더 새로운 generation은 건드리지 않음).
- query: history/runtime-metadata/structural(pure-negative universe, symbol bucket)/`LedgerStructuralProducer::execute`가 snapshot을
  clone하고 guard를 놓은 뒤 scan. `snapshot_structural_state`의 state 전체 clone 제거.
- boot: `migrate_legacy_auxiliary_snapshots`(세 legacy `state.cbor`가 있으면 whole-state rows로 한 transaction apply 후 파일 삭제 —
  crash 시 재실행은 같은 upsert라 수렴) → `restore_auxiliary_rows_into`. `BootInventoryReportV1`에 `auxiliary_migration`,
  `auxiliary_rows_restored`. `AuxiliaryAuthorityStore`는 search-corpus rollback history + legacy migration만 남음(doc 갱신).

**검증**:

| 기준 | 검증 |
| --- | --- |
| upsert/delete/clear가 순서대로 적용되고 key order로 검증돼 읽힘, receipt가 쓴/지운 수와 일치 | `catalog/tests/auxiliary.rs::a_batch_applies_in_order_and_reads_back_verified_in_key_order` |
| transaction: 쓸 수 없는 row가 있으면 같은 batch의 앞선 upsert도 남지 않음, 다음 batch는 정상 | `…::a_batch_that_cannot_be_written_writes_nothing` |
| 1,000 row generation 위 1-row upsert → receipt `rows_written=1`, 나머지 999 row byte 동일 | `…::a_one_row_mutation_over_a_large_generation_writes_one_row` |
| row/track value 1 byte 변조 → scan/track_rows가 `CATALOG_ROW_CORRUPT` | `…::a_row_that_does_not_match_its_digest_is_refused_typed` |
| forget_generation이 3 domain의 해당 generation row만 제거, track row 보존; track row replace/round-trip | `…::forgetting_a_generation_drops_exactly_its_rows_across_domains`, `…::track_rows_replace_by_key_and_round_trip` |
| catalog apply 실패 → typed 전파, ledger에 state 없음, `applies=0`; 재시도는 1회 apply 후 visible | `search_plane::ingest_dispatcher::tests::a_mutation_whose_rows_never_became_durable_is_never_visible` |
| parent unknown → `HISTORY_COMMIT_PARENT_UNKNOWN`, catalog에 닿지 않음(`applies=0, rows=0`); 같은 batch 앞의 parent는 known | `…::a_batch_that_fails_validation_never_reaches_the_catalog` |
| 1,000 dirty row 위 1-row dirty batch → catalog `rows_written` Δ=2(doc row + generation meta row), in-place 갱신·1,000 resident | `…::a_one_row_dirty_mutation_writes_one_row` |
| reader가 snapshot Arc를 쥔 채 lock 해제 → 다른 thread의 mutation이 완료(무차단), snapshot은 1 commit 그대로, ledger는 2 | `…::a_reader_holding_a_snapshot_neither_blocks_nor_sees_a_mutation` |
| g3/g4 history 존재, g5 seal(receipt retains {4,5}) → ledger g3 forget·g4 유지·g5 chunk 1, catalog generations == {4,5} | `…::retention_forgets_auxiliary_generations_the_receipt_does_not_retain` |
| 13 family + structural track이 whole-state rows → restore로 round-trip(기존 CBOR round-trip test 대체) | `readiness::tests::auxiliary_authorities_roundtrip_through_catalog_rows` |
| legacy 3 file → migration receipt(1 generation, rows>0), 파일 삭제, 2번째 open은 `None`, restore 후 history flag·dirty·chunk·track seal 복원 | `readiness::tests::legacy_auxiliary_snapshots_migrate_into_the_catalog_once` |
| e2e: history+dirty+structural ingest → seal → restart. daemon 정지 상태에서 catalog 파일을 직접 열어 3 domain row가 sealed generation에 존재, restart 후 history commit ids 동일·structural tree 1 serve | `searchd-runtime/tests/e2e_auxiliary_catalog.rs::auxiliary_rows_survive_a_restart_from_the_catalog` |
| e2e: KEEP=2로 4 generation seal(각각 history/dirty 포함) → catalog에 남은 generation 집합 == 최신 2, 각 domain row 존재 | `…::retention_forgets_the_reaped_generations_auxiliary_rows` |
| 기존 restart replay / dsl / structural / runtime e2e 전부 green(row 경로로 복원) | daemon lane 전체 |

**정직한 한계**: (a) aux query의 visible row를 "immutable version/visibility interval"(plan §5.6)로 선택하는 epoch 모델은 미구현 —
현재는 durable 즉시 다음 snapshot부터 보이며 한 query는 시작 시점 snapshot을 끝까지 본다(query 중간에 overlay가 섞이지 않음은 보장).
(b) `Arc::make_mut`는 reader가 이전 snapshot을 쥔 **동안에만** 그 generation을 copy — 완료 기준 "cloned bytes ∝ 변경량"은 mutation
자체엔 성립하고 동시 reader 존재 시 generation 1개 copy가 추가된다. (c) 100만-row history query 중 다른 repo ingest latency(완료 기준
1)는 contended host라 미측정 — W7. (d) history top-k order(QI-BB-023)와 text relevance는 별도 항목. (e) idempotency finalize와 aux
rows는 같은 파일이지만 별 transaction — crash 시 Resume이 같은 rows를 upsert해 수렴(§3.16 프로토콜).

## 3.20 QI-BB-023 — history top-k는 recency total order를 따르고, cursor로 빠짐없이 걷는다 (구현 완료)

**진단 확정**: commit authority가 `BTreeMap<CommitSha, _>`라 `type:commit fix top_k=N`은 **SHA byte가 작은 N개**를 반환하고
scan은 `top_k`가 차면 즉시 멈췄다(가장 최신 commit이 SHA 순서상 뒤면 영구히 안 보임). diff는 `(sha, path)` 순서로 같은 결함.
페이지네이션·examined/matched count 없음.

**구현**:
- contract: `HistoryCursor { committer_time_ms, sha, file_path: Option }`(query/history_cursor.rs, manual serde). 순서 계약 문서화:
  commit `(committer_time_ms DESC, sha ASC)`, diff `(commit time DESC, sha ASC, path ASC)`. `HistoryQueryRequest { text_query,
  cursor: Option<HistoryCursor> }`(cursor 없으면 wire에 부재). `SearchPlaneHistoryQueryResponse { generation, commits, diffs,
  window: QueryResultWindowV1, examined: u64, next_cursor: Option }` — decoder가 fail-closed: `window.returned == rows`, commits와
  diffs 동시 비어있지 않음 거부, `has_more == next_cursor.is_some()`. 이전 두-payload macro 삭제.
- search-plane `execute_history_query`: `HistoryRank { Reverse(time), sha, path }`의 derived `Ord`가 wire 순서 그 자체 —
  cursor는 rank이고 "after"는 `>`. 전체 scan(authority가 sha 키라 최신 match가 어디든 있을 수 있음) + `BinaryHeap` bounded
  selection(`O(log k)`/match, 상주 k+1)로 cursor 이후의 최신 k개, `matched`는 **exact** count(`CandidateCountV1::Exact`),
  `examined`는 방문 record 수, `has_more = matched > returned`, `next_cursor`는 마지막 row의 rank. commit page에 diff cursor(또는
  반대)는 typed invalid request. query/runtime 경로처럼 snapshot을 lock 밖에서 scan.
- SDK `HistoryQueryBuilder::after(cursor)`, `query_request`는 DTO 그대로. searchctl은 `cursor: None`으로 요청, pretty renderer가
  `order: recency matched: N examined: M has_more: B` + `next_cursor:` 줄 출력. harness `query_history_page(syntax, text, top_k,
  cursor)`와 `E2eHistoryResult { window, examined, next_cursor }`. public-api baseline(contract/sdk), cargo-modules baseline 갱신,
  `just rust-fuzz-smoke 30` 4 target crash 0(wire DTO 변경).

**검증**:

| 기준 | 검증 |
| --- | --- |
| commit page + continuation / diff page(final) round-trip; has_more↔cursor 불일치·양쪽 row·row 수≠window 4 case decode 거부 | `contract/tests/ipc_query_result_v2_contract.rs::search_plane_ipc_response_v2_history_variant_roundtrips`, `…_history_page_rejects_inconsistent_shapes` |
| sha 순서가 시간 순서의 역인 5 match + 비match 1: top-2 == 최신 2(sha 1,2), window `Exact(5)`/has_more, examined 6, cursor는 2번째 row | `search_plane::query_dispatcher::history_page_tests::top_k_returns_the_newest_matches_not_the_smallest_shas` |
| page size 2로 3 page → 순서 그대로 5개 partition, 중복 0, 각 page의 matched == cursor 이후 남은 수 | `…::pages_partition_the_matches_in_order_without_gaps_or_overlap` |
| 동일 시간 3 commit은 sha 오름차순, 더 오래된 것은 뒤; 완료 page는 cursor 없음 | `…::equal_times_break_ties_by_sha_ascending` |
| diff: commit recency → path 순, cursor가 (sha, path), 2번째 page가 나머지; commit cursor로 diff page / diff cursor로 commit page는 InvalidContract | `…::diff_pages_order_by_commit_recency_then_path_and_refuse_a_commit_cursor` |
| e2e: sha·시간이 함께 오르는 5 commit(= sha 순서와 recency 순서가 정반대) → top-2가 최신 2(sha 5,4), examined ≥ 5, page 2로 걷기 == 전체 역순, 중복 0, restart 후 동일 page | `searchd-runtime/tests/e2e_history_order.rs::top_k_returns_the_newest_commits_and_pages_walk_every_match_once` |
| CLI pretty renderer가 order/matched/examined/has_more/next_cursor를 출력 | `searchctl::tests::pretty_renderer_supports_history_response` |
| full-corpus runtime rail의 `runtime_sourcegraph_history_commit` expected truth를 recency 순(bob 22ms → alice 12ms)으로 갱신 — 이전 truth가 sha 순서였음이 이 항목의 결함 재현 | `e2e_full_corpus::full_corpus_runtime_fixture_executes_real_rows_only` green |

**정직한 한계**: (a) 보완 #1의 **relevance** order(commit message/diff text의 실제 indexed retrieval + score)는 미구현 — 현재
계약은 recency 단일 order이고 text match는 filter다. plan §8.3대로 Tantivy history index가 W6 후속. score 없는 "relevance"를
heuristic으로 흉내 내지 않았다. (b) 매 query가 generation의 history를 전부 scan한다(O(n), lock 밖) — time-indexed 보조 구조는
row catalog 위의 후속. (c) `DiffCandidate`에는 여전히 sha가 없다(cursor가 sha를 나름). (d) large non-match latency/RSS budget(완료
기준 3)은 contended host라 미측정 — W7.

## 3.21 QI-BB-019 — hybrid seed는 canonical seed list 하나이고, dense 검색은 lane당 정확히 한 번이다 (구현 완료)

**진단 확정**: `hybrid_seed`가 같은 query vector로 (1) lexical id 집합에 scoped된 dense 검색으로 legacy `seed_candidates`를
만들고, (2) 다시 corpus별(또는 global) dense 검색으로 `seed_candidates_v2`를 만들어 **둘 다** 직렬화했다. decoder는 v2가 있으면
v2 길이를 window와 대조했고, `lq_merge_result_count` metric은 legacy 길이를 기록했다. CLI는 legacy `candidate`를 렌더링.

**구현**(breaking-first, intentional contract baseline change):
- contract: `HybridSeedQueryResponse { generation, manifest_digest, seed_candidates: Vec<SeedCandidate>, window, explanation }`.
  legacy `HybridSeedCandidate`/`HybridSeedLane`/`lexical_score_raw…` 삭제, `seed_candidates_v2` field 삭제(decoder는 unknown
  field로 거부). `SeedCandidateV2/SeedContributionV2/SeedLaneV2/SeedFusionIdentityV2` → `SeedCandidate/SeedContribution/SeedLane/
  SeedFusionIdentity`(V2 suffix 제거 — V1 상대가 사라졌으므로).
- search-plane `hybrid_seed`: scoped dense 검색·`build_hybrid_seed_candidates_v1`·`fuse_rrf` legacy 경로 삭제. dense lane(corpus별
  또는 global 1회)만 실행 → fused seed list 하나. `lq_merge_result_count`는 `window.returned()`.
- **legacy가 가리던 fusion 결함**: lexical seed identity가 `(Chunk, id, corpus=None)`, raw-code dense hit이 `(Chunk, id,
  Some(RawCodeFallback))`라 같은 chunk의 BM25 hit과 dense hit이 **절대 합쳐지지 않았다**(v2가 부가적이던 동안 SDK front door test는
  legacy list만 봐서 드러나지 않음). lexical lane은 raw-code corpus의 lexical projection이므로 identity를
  `Some(RawCodeFallback)`로 고정 — 한 chunk는 두 lane의 contribution을 가진 seed 하나가 되고 RRF에서 단일 lane seed를 이긴다.
- SDK export 갱신(`SeedCandidate/SeedContribution/SeedLane`), searchctl은 전용 `render_hybrid_seed_payload`(entity/owner_kind/path/
  lane contributions/degraded), harness window probe 단순화. public-api baseline(contract/sdk)·cargo-modules baseline 갱신.

**검증**:

| 기준 | 검증 |
| --- | --- |
| mock semantic adapter: scoped dense 검색 **0회**, corpus lane 3개 각각 정확히 1회(같은 query vector, 같은 constraints) | `search_plane::query_dispatcher::tests::hybrid_seed_dispatch_includes_dense_only_entity_in_the_seed_set` (scoped_vectors/constraints empty 단언 추가) |
| canonical list 1개: window.returned == seed_candidates.len, `seed_candidates_v2` 포함 payload는 decode 거부, JSON/CBOR round-trip | `contract::results::query_responses::tests::{hybrid_seed_query_response_refuses_a_legacy_second_seed_list, hybrid_seed_query_response_round_trips_with_seed_candidates, hybrid_seed_query_response_without_required_window_fails_closed}` |
| 같은 chunk의 BM25 hit + raw-code dense hit → contribution `[Bm25, Dense]`인 seed 1개, rank 1(두 lane이 단일 lane을 이김), 총 3 seed | `search_plane::query_dispatcher::semantic_query::seed_fusion_tests::seed_fusion_merges_a_chunk_across_the_bm25_and_raw_code_lanes` |
| SDK front door: hybrid seed top이 `entity_id == "alpha"` (3 경로) | `searchd-runtime/tests/sdk_frontdoor.rs` 3 test |
| CLI pretty가 `entity=… owner_kind=… lanes=bm25#1,dense#1`를 출력 | `searchctl/tests/cli_smoke.rs::hybrid_seed_pretty_roundtrip` |
| wire DTO 변경 fail-closed | `just rust-fuzz-smoke 30` |

**정직한 한계**: (a) 보완 #4의 "compat 사용량/sunset gate"는 필요 없어짐 — compat 경로 자체를 제거했다. (b) 일반 `hybrid`
route는 여전히 lexical-scoped rerank(QI-BB-018 별도). (c) `SeedContribution.raw_score`는 lane별 raw score이고 fused score는
RRF rank뿐 — explain contribution(QI-BB-022)과 함께 다룬다.

## 3.22 QI-BB-018 — hybrid는 독립 lexical lane + dense lane의 RRF union이다 (구현 완료)

**진단 확정**: `execute_hybrid_fusion`이 lexical 결과 id 집합을 `search_scoped_constrained`에 넘겨 dense lane을 lexical
universe에 가뒀다 — 결과 형식은 RRF지만 candidate universe는 BM25 recall 그대로(`BM25 recall + dense rerank`). explanation도
`semantic_scoped_to_lexical=true`를 기록했고, e2e `hybrid_query_excludes_semantic_outsider_from_lexical_universe`가 이 동작을 고정.

**구현**: dense lane을 `search_constrained(query_vector, constraints, internal_top_k)`(generation 전체, 같은 constraint
push-down)로 독립 실행 → `HybridOrchestratorPolicy::fuse_rrf`가 두 lane의 **union**을 fuse(기존 core fuse가 union 의미였음).
explanation trace는 `hybrid.lanes=independent; lexical_hits=…; semantic_hits=…; fused_universe=…`, strategy는 실제 기여
lane(`rrf | lexical_only | semantic_only | empty`). scoped port(`search_scoped_constrained`)는 semantic route의 명시적
`lexical_scope`(QI-BB-004)가 계속 쓰므로 유지. 보완 #1의 별도 `lexical_scoped_rerank` surface는 만들지 않음 — 이전 동작은
이름이 틀린 결함이었고, 명시 scope가 필요한 caller는 semantic route의 `lexical_scope`를 쓴다.

**검증**:

| 기준 | 검증 |
| --- | --- |
| mock semantic adapter: hybrid가 scoped search **0회**, unscoped `search_constrained` 정확히 1회(같은 vector, request constraints push-down) | `search_plane::query_dispatcher::tests::hybrid_dispatch_embeds_semantic_query_text` (scoped_vectors empty, search_vectors == [expected], search_constraints == [constraints]) |
| explanation: 두 lane 기여 시 `rrf` + trace `hybrid.lanes=independent…fused_universe`, lexical-only/semantic-only/empty 각각 정직 | `…::build_hybrid_response_explanation_reports_honest_lane_contribution_v1` |
| **e2e**: lexical `riddle`는 beta만, dense `focus alpha`는 alpha>beta>gamma → top_k=2 결과 `[beta, alpha]`: dense-only relevant hit(alpha)이 top-k에 들어오고 gamma는 제외 (이전 test는 alpha 제외를 고정했음 → 반전) | `searchd-runtime/tests/end_to_end.rs::hybrid_query_admits_a_semantic_only_relevant_hit_beside_the_lexical_hits` |
| dsl planner trace 문자열 갱신 | `dsl_scenarios` |

**정직한 한계**: (a) 보완 #4의 judged corpus(zero-overlap paraphrase/lexical-only/dense-only/tie) recall·NDCG·latency 측정은
미실행 — harness `relevance` rail은 hash embedder라 dense lane의 실제 품질 판단은 QI-BB-007(M4) production profile 이후. (b)
소규모 corpus에서는 `internal_top_k(100)`이 corpus보다 커 dense lane이 전 문서를 반환하므로 RRF가 "양 lane 존재"를 과대 보상한다
(e2e fixture 설계에서 관측: gamma가 cosine≈0으로도 alpha를 이김). 실제 corpus에서는 lane당 100 bound가 의미를 가지지만, dense
lane에 최소 similarity threshold 또는 score-aware fusion을 두는 것은 QI-BB-022(contribution)와 함께 다룰 후속.

## 3.23 QI-BB-027 — ANN index는 sealed generation의 운영 계약이다 (구현 완료)

**진단 확정**: seal이 256 row 이상에서 `IvfHnswSqIndexBuilder::default()`+cosine만 넘겨 index를 만들고, manifest는 index의
존재·종류·파라미터·library를 기록하지 않았으며, open은 `list_indices`를 보지 않았고, query는 library default effort
(nprobes 20, ef 1.5k, refine 없음 → `_distance`가 SQ8 근사값)로 돌았다. 실측으로 확인한 추가 사실: lance 7.0.0의
IvfHnswSq default `target_partition_size = 1<<20`이라 1M row 미만은 partition 1개 — 즉 topology가 dependency default에
전적으로 의존했고, `build.rs` 주석의 "sample_rate=256이라 partition=1"은 근거가 틀렸다. delta generation은 base dataset을
hard link로 상속하므로 **base의 index도 상속**됐다: delta row 수가 floor 아래로 내려가도 상속 index가 남아 서비스 mode가 seal
정책과 달랐고, 이름이 다른 index가 base 것 옆에 쌓일 수 있었다.

**구현** (`quanta-index-semantic/src/vector_index.rs`, manifest `FORMAT_VERSION 7→8`):

- **Policy는 전부 명시**: `VECTOR_INDEX_MIN_ROWS=256`, `VECTOR_INDEX_NAME="vector_ivf_hnsw_sq"`, partitions =
  `clamp(rows / 2^20, 1, 4096)`, IVF `sample_rate=256`/`max_iterations=50`, HNSW `m=20`/`ef_construction=300`, cosine —
  builder에 전부 명시 전달(default 의존 0). query effort: `nprobes=min(20, partitions)`, `ef=max(64, 2·k·refine)`,
  `refine_factor=2`(top-2k를 원본 vector로 재정렬 → 반환 score가 **정확한 cosine**). exact lane은 `bypass_vector_index()`로 명시.
- **Seal**: 상속된 vector index를 이름 불문 전부 `drop_index` → floor 이상이면 정책 index를 `replace(false)`로 build →
  `index_stats`를 읽어 `indexed_rows == row_count && unindexed_rows == 0 && type/distance 일치`가 아니면 seal 거부 →
  `VectorIndexSealV1 { mode, library, library_version, index_min_rows, ann: Option<AnnIndexSealV1 {name, distance,
  num_partitions, sample_rate, max_iterations, hnsw_m, hnsw_ef_construction, indexed_rows, index_segments, nprobes, ef_floor,
  ef_per_candidate, refine_factor} > }`를 scope manifest에 기록. `library_version`은 `Cargo.lock`의 lancedb 버전과 unit test로 pin.
- **Open**: file commitment(§3.14) 통과 후 `list_indices`/`index_stats`를 seal과 대조. exact seal인데 index가 있으면
  `ANN_INDEX_INCOMPATIBLE`; ann seal의 이름이 없거나 stats가 없으면 `ANN_INDEX_MISSING`; type/distance/coverage/segment 수/
  index 개수가 다르면 `ANN_INDEX_INCOMPATIBLE`; 다른 library가 build한 seal은 `ANN_INDEX_INCOMPATIBLE`. 같은 library의 다른
  버전은 서비스하되 attestation을 `SealedByAnotherLibraryVersion`으로 구분(아래 결정).
- **Query**: `LoadedVectorIndexV1`이 sealed effort를 `VectorQuery`에 그대로 pin. legacy(≤v7) generation은
  `LegacyUnverified` — dataset이 보고하는 index로(있으면 policy effort, 없으면 exact) 서비스하고 아무것도 증명하지 않는다.
- **관측성**: core `DenseLaneContractV1 { index: Exact | Approximate(effort), attestation: Sealed |
  SealedByAnotherLibraryVersion | LegacyUnverified }` + `SemanticSearcher::dense_lane()`; semantic/hybrid/hybrid-seed
  explanation의 Plan trace에 `dense.index=…; dense.attestation=…; dense.partitions=…; dense.nprobes=…; dense.ef=max(a,b*candidates);
  dense.refine_factor=…`.
- **format-version 분기 → capability 모델**: 6개 legacy 상수를 5곳 이상에서 match하던 것을 `FormatCapabilitiesV1 { build_contract,
  corpus_metadata, membership_commitment, semantic_row_root, file_commitment, vector_index_seal }` predicate로 대체 —
  layout schema, contract 요구, membership 검증, sealed-manifest 요구, migration row-root 증명이 모두 이 한 곳을 본다. v7은
  `file_commitment` 유지(QI-BB-017 결정 보존), v8부터 `vector_index_seal`.

**결정 — library 버전 drift는 거부하지 않는다**: 대안 "recorded `library_version != 서비스 library` ⇒ open 거부"는
기각. lancedb patch bump마다 모든 repo의 전 generation을 강제 reseal해야 하고, 보완 #4의 dependency-upgrade A/B(같은
artifact를 새 library로 열어 비교) 자체를 불가능하게 만든다. 대신 (a) file commitment가 byte 동일성을, (b) `index_stats`
대조가 metadata 동일성을 증명하고, (c) attestation이 "recall은 다른 library로 측정됨"을 trace에 노출한다. 다른 **library**
(이름 불일치)는 거부.

**검증**:

| 기준 | 검증 |
| --- | --- |
| 255 row → `exact`/Sealed, library index 0개, index 파일 0개; 256 row → `ivf_hnsw_sq`(partitions 1, nprobes 1, ef 64/2, refine 2)/Sealed, library가 정확히 `vector_ivf_hnsw_sq` 1개, stats (256, 0, 1 segment); self-vector top-1 score = 1.0 ± 1e-5 | `semantic/tests/vector_index_contract.rs::the_seal_and_the_dataset_agree_at_the_255_256_boundary` |
| base 300(index) → delta tombstone으로 200 row: delta는 exact + 상속 index drop, base는 index 그대로(stats 300/0/1) | `…::a_delta_that_shrinks_below_the_floor_drops_the_inherited_index` |
| base 300 → delta +100: index 정확히 1개(base 것 옆에 안 쌓임), stats (400, 0, 1), delta-only row가 top-1 | `…::a_delta_that_grows_seals_one_index_covering_every_row` |
| index 파일 1byte 변조 / 전부 삭제 → validate·open·restart 모두 `GENERATION_SIDECAR_CORRUPT`, 복원 후 다시 admit | `…::losing_or_damaging_the_index_files_refuses_both_doors_and_a_restart` |
| v7 manifest(byte 단위 downgrade + sealed manifest 재commit): index 있는 generation은 Approximate/LegacyUnverified로 서비스, 없는 것은 Exact/LegacyUnverified | `…::a_generation_sealed_before_the_contract_serves_unverified_on_what_it_carries` |
| **recall gate**: 4,096 row × 64-dim random unit vector(cluster 구조 없음, 최악 case), 64 query(절반은 row 근처 paraphrase, 절반 fresh), 순수 Rust exhaustive cosine oracle 대비 recall@10 ≥ 0.95, 반환 score == exact cosine ± 1e-4 | `…::the_sealed_effort_keeps_recall_against_an_exact_oracle_and_returns_exact_scores` |
| verifier typed 동작(file commitment 뒤에 있어 통합 test로는 도달 불가): exact seal + index 존재 → INCOMPATIBLE; coverage/segment 불일치 → INCOMPATIBLE; 이름 불일치·drop 후 → MISSING; 다른 버전 → Sealed­ByAnotherLibraryVersion; 다른 library → INCOMPATIBLE; legacy 관찰 | `vector_index::tests::{the_verifier_refuses_every_disagreement_between_seal_and_dataset, a_legacy_generation_is_observed_not_verified}` |
| seal이 이름 다른 상속 index를 교체, floor 아래에서 drop | `vector_index::tests::the_seal_replaces_an_inherited_index_of_any_name` |
| `ANN_LIBRARY_VERSION == Cargo.lock` | `vector_index::tests::the_recorded_library_version_is_the_locked_one` |
| manifest: v7 decode(seal 날조 없음), v8 round-trip, seal 없는 v8 거부, capability 단조성, seal 자기모순 8종 거부 | `manifest::tests::*` |
| e2e trace `dense.index=exact; dense.attestation=sealed` (semantic scoped, hybrid) | `dsl_scenarios` |
| legacy v2 fixture를 정직하게(후행 field 제거) 재작성 — version만 바꾼 v8 body는 이제 "predates the seal it carries"로 거부 | `persisted_semantic.rs::{legacy_v2_*, scan_reports_legacy_v2_*}` |

**측정 (current HEAD, 3회, 4,096×64)**: recall@10 **0.984 / 0.989 / 0.984**, p50 8.8–12.7 ms, p95 9.6–21 ms, p99 10–25 ms
(contended host, lance plan overhead 지배), build 14.8–19 s(row append 포함), index bytes **1,154,009–1,154,265**(build은
비결정적 — §3.4.2에서 관측한 것과 일치; seal이 결정적으로 만드는 것은 recipe와 effort이지 top-k가 아니다).

**정직한 한계**: (a) seal마다 index **전체 재구축**(O(N))은 그대로 — delta에서 상속 index를 drop하고 새로 만든다(lance
`optimize` delta-index는 segment가 늘어 seal의 "segment 1" 계약과 충돌; 별도 항목). (b) recall gate는 synthetic corpus 1 tier의
floor이지 corpus tier별 recall/latency gate(보완 #3)가 아니다 — 실 corpus tier 측정은 QI-BB-007(M4) production profile
이후. (c) activation receipt에 ANN attestation 노출(QI-BB-031 보완 #4)은 미구현 — searcher `dense_lane()`과 explanation
trace까지. (d) dependency-upgrade A/B(보완 #4)는 process이며 code gate가 아니다; attestation과 recorded recipe가 그 A/B의
입력을 제공한다. (e) prefilter(allowlist/language/path)가 index row의 10% 이상을 남기면 lance는 bitset을 든 HNSW walk를
하므로 filtered recall은 unfiltered gate와 같지 않다 — scoped route의 recall은 별도 측정 항목.

## 3.24 QI-BB-022 — explain은 exact presence lookup + 원 질의 하의 lexical score trace다 (구현 완료)

**진단 확정**: explain request가 원 질의를 받지 않아 candidate snippet(또는 id)로 **새** phrase probe를 만들어 top-50에
있는지만 봤다. (a) presence가 corpus 크기에 종속 — 실제 index에 있어도 probe top-50 밖이면 "NOT present"; (b)
`contributions` 항상 비어 있고 `ranker_weights_hash` 항상 0, strategy `presence_probe`; (c) ranking explanation과 stale
membership 확인이 한 이름에 섞여 있었다.

**구현**:

- **Contract**: `SearchPlaneExplainQueryRequest { generation, candidate, text_query: Option<TextQueryRequest> }` —
  `text_query`가 없으면 presence만, 있으면 그 질의 하의 score trace. `text_query.generation`은 없거나 explain pin과 같아야
  하고 selector는 금지(generation은 한 번만 명명). `SearchPlaneExplainQueryResponse { generation, presence:
  CandidatePresenceV1 { Indexed | NotIndexed }, explanation }` — presence는 typed field, summary 문자열이 아니다.
  IPC decoder fail-closed(presence 누락/미지 token 거부) + fuzz smoke 30s×4 crash 0.
- **Core port**: `LexicalSearcher::candidate_presence(id)` (exact lookup), `LexicalSearcher::explain_candidate(query,
  constraints, id) -> NotIndexed | NotMatched { reason } | Matched(LexicalScoreTraceV1 { engine: Bm25 | UnindexedScan,
  engine_score, boost_factor, emitted_score })`.
- **Lexical adapter**: presence = `candidate_id` term × `doc_kind` term 정확 조회(live doc 2개면 corrupt index 오류);
  score = ranked search와 **동일 준비 경로**(symbol-name rewrite → validate → prepare_executable_query → preflight →
  repo filter gate → compile+constraints+doc_kind) 후 compiled plan의 **scorer를 그 한 document에 위치**시켜
  `score()` — TopDocs collector가 쓰는 것과 같은 scorer이므로 page score와 동일. unindexed scan(`index:no`)은
  `manual_text_search` loop body를 `manual_doc_matches(doc, …)`로 떼어내 scan과 explain이 같은 matcher를 쓴다(1.0 × boost).
  tantivy `Query::explain`의 문자열 오류 매칭(`"does not match"`)에 의존하지 않는다.
- **Dispatcher**: `lexical()`의 plan 준비를 `plan_lexical_text_query(request) -> PlannedLexicalTextQuery { pin, query,
  constraints, force_empty }`로 올려 search와 explain이 **같은 plan**을 lower한다(rev-at-time rebinding이 다른 generation을
  고르면 InvalidContract). explanation: `contributions = [ExplanationRow { signal_name: "lexical.bm25"|"lexical.unindexed_scan",
  signal_value: engine_score, weight: boost_factor, contribution: emitted }]` — **합성 규칙: Σ contribution = emitted score**;
  `ranker_weights_hash = sha256("quanta-index lexical ranker weights v1\nengine=…\nboost_millis=…\n")`(plan의 ranker
  입력 pin, 0이 아님); trace `explain.mode=…`, `explain.candidate_indexed=…`, `explain.candidate_matched=…`,
  `explain.score_reconciled=…`(carried score와 이 plan의 emitted score가 1e-5 상대 오차 내 일치 여부 — 다른 plan에서 온
  candidate는 정직하게 false). strategy `presence_lookup | lexical_score_trace`. 옛 `presence_probe`/`default_top_k`
  삭제.
- **SDK/CLI/harness**: `search().explain(pin, candidate)`(presence) + `explain_under_query(pin, candidate, text_query)`;
  CLI `explain --candidate-json … [--syntax … --query-text …]`(둘 다 또는 둘 다 없음), `presence:` 줄 렌더링; harness
  `explain_candidate_under_query`.

**검증**:

| 완료 기준 | 검증 |
| --- | --- |
| **golden**: ranked page의 모든 candidate가 같은 query 하에서 정확히 carried score로 설명되고(1 row, Σ=score, weight 1.0), page 순서가 explained score 순서 | `lexical/tests/explain_candidate.rs::every_ranked_candidate_explains_to_exactly_its_emitted_score_in_rank_order`; e2e `e2e_explain_score_trace.rs::every_page_candidate_explains_to_its_carried_score_in_page_order` |
| **input 변경 → contribution·rank 동시 변화**: `boost:2.5` → weight 2.5, engine_score 불변, emitted ×2.5, page score도 ×2.5, weights hash 변화; boosted candidate를 plain query로 explain하면 `score_reconciled=false` + plain contribution | `lexical::…::a_boost_in_the_plan_is_the_weight_and_scales_the_emitted_score`; e2e `…::a_boost_is_the_weight_and_moves_the_page_the_trace_and_the_weights_hash_together` |
| **합성 규칙 = emitted score**(오차 1e-5): 위 두 test의 `Σ contribution == page score` 단언 | 동일 |
| **presence는 corpus 크기 무관**: 300 short doc이 같은 term으로 1 long doc을 page(50) 밖으로 밀어도 explain은 Indexed + Matched(real score); 미ingest id는 NotIndexed | `lexical::…::presence_is_an_exact_lookup_independent_of_the_corpus_around_the_candidate` |
| indexed-but-unmatched는 NotMatched(절대 "absent" 아님) — 비매칭 doc이 match set 앞/사이/뒤, 그리고 **segment에 match가 0개**인 plan에서도 | `lexical::…::every_ranked_…`(5 case; 이 fixture가 scorer 위치 결함을 잡음, 아래) + e2e `…::presence_is_a_typed_exact_lookup_and_a_non_match_is_not_absence` |
| unindexed scan(`index:no`)도 같은 per-doc matcher로 1.0×boost = page score | `lexical::…::an_unindexed_scan_explains_through_the_same_per_document_matcher` |
| presence-only explain: typed Indexed, strategy `presence_lookup`, contributions 없음; generation mismatch는 INVALID_REQUEST | `searchd-runtime/tests/explain.rs` 2 test(갱신) |
| wire: request(text_query 유/무) round-trip, response presence round-trip, presence 누락/`"maybe"` 거부 | `contract/tests/ipc_query_result_v2_contract.rs::explain_*` |
| 기존 소비자(sdk_frontdoor summary `present`, restart determinism explanation 동일성, full-corpus `engines_touched=["lexical"]`, CLI smoke `presence: indexed`) | 해당 suite 전부 green |
| **UI rail**(harness `ui.rs`)이 finding이 지적한 "contribution 0이어도 `presence_probe` tag로 통과"를 더 이상 허용하지 않음: scored explain을 돌려 strategy `lexical_score_trace`, presence Indexed, row ≥1, Σ row == carried score(1e-5)를 gate | `searchd-harness::ui::tests::seeded_ui_rail_runs_and_anchors_every_probe` (첫 verify-rust에서 `presence_probe` 기대로 RED → rail을 새 계약으로 갱신) |

**구현 중 잡은 결함**: 첫 구현은 `scorer.seek(doc)`로 한 document에 위치시켰는데 tantivy `DocSet::seek`는 현재 doc ≤ target을
`debug_assert`한다 — 후보 doc이 plan의 첫 match보다 앞이거나 segment에 match가 없으면(초기 doc = TERMINATED) debug build에서
panic(e2e에서 connection thread panic으로 관측). `scorer.doc()`을 먼저 보고 `first >= target`이면 seek하지 않도록 수정;
lexical test fixture를 비매칭 doc 위치 5-case로 확장해 fix를 되돌리면 panic하는 것을 확인했다.

**정직한 한계**: (a) hybrid/hybrid-seed/semantic route 후보의 explain은 **lexical lane만** trace한다 — RRF-fused carried
score는 `score_reconciled=false`로 정직하게 표시되지만 dense lane·RRF rank contribution은 제공하지 않는다(per-candidate
lane provenance를 hybrid 응답에 싣는 후속; hybrid seed는 이미 `SeedContribution`을 lane별로 실음). (b) 보완 #1/#4의
provenance store(immutable execution record ID, retention/privacy budget)는 만들지 않았다 — explain은 caller가 넘긴 원
질의로 같은 plan을 재유도한다(plan은 (query, generation)의 순수 함수라 결정적). (c) 엔진 내부 세부(BM25 term별 idf/tf)는
row로 펼치지 않는다 — 1 row가 정확히 emitted score와 일치하는 것이 계약이고, tantivy Explanation tree는 vendor 형식이라
노출하지 않는다. (d) symbol route candidate(`SymbolCandidate`)는 explain surface에 없다(request가 `LexicalCandidate`).

## 3.25 QI-BB-008 — RepoMap query는 bounded selection, store는 atomic·checksum·quarantine·retention (구현 완료)

**진단 확정**: (a) `read_query_snapshot`이 read lock 아래 snapshot 전체를 clone하고 query engine이 entries를 **다시** clone
→ 전체 scan/score/sort → `included=false` row까지 전부 응답(`top_k=2`에 5 row가 test로 고정). (b) `query_match_score`가
entry마다 `search_text.to_ascii_lowercase()` **할당**. (c) persistence는 `fs::write`(temp/fsync/rename 없음), checksum 없음,
JSON 하나만 깨져도 전체 open 실패, orphan activation 하나가 전체 open 실패, retention 없음. (d) 모델 `RepoMapEntry`가
query-time 출력(`included`, `rank`)을 persisted field로 들고 있었다.

**구현** (`quanta-index-repomap`):

- **Shared + indexed snapshot**: store는 `Arc<RepoMapIndexedSnapshot { snapshot, index }>`를 보관, query는 Arc clone 하나.
  `RepoMapSnapshotIndex`는 materialize/load 시 한 번: entry별 folded `search_text`, `by_subject: identity → doc_type →
  position`, `by_owner_path: path → positions`. focus 해석은 index lookup(O(log N)), term 매칭은 folded text substring
  (per-entry 할당 0), exact-focus 판정은 `(&str, DocType)` 차용 키.
- **Bounded selection**: 후보 universe(전체 또는 focus 종속) 위에서 rank key
  `(exact_focus, owner_path_focus, query_match_score, final_score_millis, Reverse(identity))`를 크기 `prefix = top_k`의
  min-heap으로 유지(O(N log k), 할당 O(k)) → 정렬 후 budget walk. token budget이 entry를 건너뛰어 page가 덜 찼을 때만
  prefix를 2배로 넓혀 재scan(greedy walk의 결정은 앞선 entry에만 의존하므로 full-sort 결과와 동일). 응답은 **포함된 row만**
  (`rank` 1..k), 나머지는 `dropped_entries_count` + reason 집계(`token_budget_exhausted`, `top_k_exhausted`).
- **Model/Contract**: `RepoMapEntry`에서 `included`/`rank` 제거(query-time 값; legacy 파일의 해당 field는 decoder가 무시).
  `RepoMapEntryDto.included` 제거(모든 row가 included) — public-api baseline 갱신, CLI 렌더/SDK/e2e 갱신.
- **Persistence**: `write_atomic` = 같은 dir의 `.tmp-*` → `sync_all` → rename → dir `sync_all`; snapshot 파일은
  `{format_version: 2, sha256(canonical compact JSON), snapshot}` envelope; open 시 `.tmp-*` sweep, envelope 없는 legacy
  파일은 1회 읽고 envelope로 재기록(`snapshots_migrated`), decode 실패/digest 불일치/파일명-identity 불일치/format 불일치는
  **해당 파일만** `quarantine/`로 이동(사유 기록), orphan activation은 `activations_without_snapshot`에 기록하고 그 repo만
  `NOT_FOUND`(전체 open은 성공). 결과는 `RepoMapOpenReportV1`(core)로 `BootInventoryReportV1.repo_map`에 노출.
- **Retention**: activation이 durable해진 뒤 같은 (repo, revision)의 **더 오래된** generation을 파일 → 메모리 순으로 retire
  (query는 activated generation만 답하므로 dead weight); 더 새로운 미활성 generation은 유지.

**검증**:

| 완료 기준 | 검증 |
| --- | --- |
| **bounded engine ≡ full sort**: 이전 engine을 reference로 test 안에 그대로 재구현, 랜덤 snapshot(≤400 entries)×request(focus 0–2, 미해결 focus 포함, term 0–2, top_k 1–12, budget 1–600) **720 case**에서 row/rank/dropped/drop reasons/degraded reasons 완전 일치 | `repomap/tests/bounded_query.rs::the_bounded_engine_agrees_with_the_full_sort_on_every_randomized_request` |
| **snapshot 10×에도 응답 고정**: 같은 1,000 entry 위에 9,000 filler(page 밖) 추가 → page 동일, CBOR 응답 byte **동일** | `…::a_fixed_top_k_response_does_not_grow_with_the_snapshot` |
| `top_k=2` → row 2개 + dropped 3(옛 test는 5 row 반환을 고정했음 → 반전) | `bootstrap_owner_flow.rs::ingest_and_query_returns_ranked_entries`, `owner_surface.rs::query_without_focus_…` |
| **crash matrix**: 자식 프로세스가 temp sync 직후 / rename 직후 `process::exit`(semantic 크레이트와 같은 subprocess 패턴) → 부모가 open: 각각 이전 snapshot(+temp 1개 sweep) / 새 snapshot, quarantine 0, 두 번째 open에 temp 0 | `persistence::tests::a_crash_at_every_write_step_leaves_the_previous_or_the_new_snapshot` |
| digest 불일치(body 1byte 변조) / 잘린 파일 / 엉뚱한 activation 파일 → 각각 사유와 함께 quarantine, 나머지는 load, 다음 open은 clean | `…::a_damaged_file_is_quarantined_with_its_reason_and_the_rest_opens` |
| 파일명 ≠ identity → quarantine; legacy bare JSON → 1회 migration 후 envelope 검증 | `…::a_file_under_the_wrong_name_is_quarantined`, `…::a_legacy_bare_snapshot_is_migrated_once_and_verified_after` |
| activation 시 g1,g2 retire(파일+메모리), g4 유지, reopen 일치; activated 파일 유실 → report + 그 repo `NOT_FOUND`(더 새로운 미활성 generation도 서비스 안 함), 재활성화로 복구 | `repomap/tests/store_durability.rs` 2 test; `bootstrap_owner_flow.rs::persistent_store_reports_an_orphaned_activation_…`(옛 "전체 open 실패" 기대 → 반전) |
| 보완 #5 corrupt 격리가 boot inventory에 보임 | `BootInventoryReportV1.repo_map` (runtime 조립) |

**정직한 한계**: (a) query CPU는 여전히 O(N)(entry당 substring 매칭)이며 O(k)인 것은 **할당과 응답**이다 — substring
semantics를 유지하면서 inverted index를 두려면 token 매칭으로 의미가 바뀌므로 하지 않았다(이 finding의 완료 기준은
allocation/response). (b) 할당 상한은 구조로 보장하지만(heap O(k), response O(k)) 계측용 counting allocator는 `unsafe`가
필요해 두지 않았다 — 측정은 응답 byte로. (c) activation record는 digest 없이 atomic write만(3 field JSON; decode 실패는
quarantine). (d) fsync 유실(전원 단절) 자체는 시뮬레이션하지 않았다 — protocol(temp sync → rename → dir sync)과 crash
boundary 2곳의 subprocess kill이 증거다.

## 3.26 QI-BB-016 — lexical writer는 하나의 heap envelope 아래 살고, 놀면 heap을 돌려준다 (구현 완료)

**진단 확정**: `LEXICAL_WRITER_CACHE_MAX = 16`, `WRITER_MEMORY_BUDGET_BYTES = 15 MB`가 상수였고 config·관측 모두 없었다.
insert 시에만 LRU evict — producer가 generation 중간에 멈추면 그 writer는 영원히 15 MB(+mmap)를 쥔다. writer 수와 writer
heap이 따로 놀아 "process 예산"이라는 것이 존재하지 않았다.

**구현**:

- core `LexicalWriterPolicy { envelope_bytes, writer_heap_bytes, idle_after }` — **writer 수는 파생값** `envelope /
  heap`(≥1). heap은 tantivy 최소 arena(15,000,000)…최대(2^32-1-1,000,000) 범위 검증, envelope ≥ heap, idle > 0.
  DEFAULT = 16 × 15 MB / 15 MB / 60 s(이전 상수와 같은 bound). `LexicalWriterCacheStats { open_writers, max_writers,
  allocated_heap_bytes, lru_releases, idle_releases, seal_releases }`.
- adapter `WriterCache`: entry에 `last_used: Instant`; `get_or_open`은 idle sweep → LRU release(envelope 여유 확보) →
  `writer_with_num_threads(threads, heap)` — thread 수도 명시(`min(heap/15MB, cpus, 8)`, tantivy가 내부적으로 하는 파생을
  드러냄). `release(key, why)`가 commit+drop과 사유 계수를 한 곳에서. seal은 `WriterRelease::Seal`로 즉시 반환(기존 동작 유지,
  계수 추가). `build_batch` 끝마다 `release_idle_writers()` sweep; composition root도 호출 가능(pub).
- searchd config: `QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES / _HEAP_BYTES / _IDLE_SECS` → `lexical_writer_policy()` →
  runtime이 adapter에 주입. `LEXICAL_WRITER_CACHE_MAX` 상수·`writer_cache_len` 삭제(breaking).

**검증**:

| 기준 | 검증 |
| --- | --- |
| envelope 3×15MB → max 3; 5 generation build 동안 매 step `open_writers ≤ max_writers`, `allocated_heap_bytes ≤ envelope`; 끝에 open 3 / lru 2; release된 generation의 on-disk segment 유지; 재접촉 시 가장 오래된 resident(g2)만 release | `lexical/tests/writer_envelope.rs::the_envelope_bounds_open_writers_and_their_heap_and_releases_the_oldest_committed` |
| idle 400ms 후 sweep → open 0, `idle_releases` 2, allocated 0, segment 유지; build-time sweep은 idle writer만 release(open 1) | `…::an_idle_writer_is_committed_and_released_on_the_next_sweep` |
| seal 후 open 0, `seal_releases` 1, sealed generation은 port로 검색 가능 | `…::a_seal_releases_its_generations_writer_and_the_index_stays_readable` |
| env binding: unset=DEFAULT(max 16); heap 60MB → max 4; idle 5s; heap<min / envelope<heap / idle 0 → typed 거부 | `searchd::config::tests::lexical_writer_env_binding_layers_over_the_default_and_refuses_bad_envelopes` |
| 옛 `writer_cache_evicts_lru_after_threshold`(17 writer 생성)는 envelope test로 대체 | `tantivy_smoke.rs`에서 삭제 |

**정직한 한계**: (a) envelope은 **writer heap**만 센다 — tantivy segment mmap, sidecar, regex cache(§3.17), SnapshotRegistry
byte budget(§3.7), semantic cache(§3.18)는 각자 bound가 있지만 하나의 process RSS 예산으로 합산되지는 않는다(finding의
"하나의 process memory envelope"에서 남은 것). (b) RSS pressure 관측·반응(evict on pressure)은 없다 — idle/LRU/seal의
결정적 release만. (c) "commit on release"는 방어적이다: 현재 adapter는 모든 build를 즉시 commit하므로 pending doc이 생기지
않고, test는 release가 committed segment를 잃지 않음을 증명한다.

## 3.27 QI-BB-014 — socket과 state root는 private mode로 강제·검증하고, live socket은 빼앗지 않는다 (구현 완료)

**진단 확정**: `UdsServer::bind`가 parent `create_dir_all`(umask 의존) + 기존 socket 파일 무조건 `remove_file` + bind였다 —
mode/owner 검증 없음, live listener 확인 없음(다른 state root의 daemon이 같은 override path를 쓰면 앞 daemon의 socket
pathname을 빼앗음). state root는 `create_dir`(umask)로 만들고 mode/owner를 보지 않았다(lock 파일만 `0600/NOFOLLOW`).

**구현** (private mode — 이 daemon의 유일한 mode):

- **socket directory**: 없으면 `0700`으로 생성(생성 후 uid/mode 재확인). 있으면 **resolve된 대상**으로 검증(macOS `/tmp`는
  `/private/tmp`로의 symlink — 첫 구현이 symlink 자체를 거부해 harness 전부가 RED, 대상 기준으로 수정) — 디렉토리 아님 거부,
  `(uid == euid && mode & 0o022 == 0)` 또는 **sticky bit**(공유 temp dir: 남이 내 socket을 unlink/rename 못 함) 아니면
  `SOCKET_PATH_INSECURE` 거부(mode/uid/euid 명시).
- **기존 socket path**: socket 아니면(regular file/dir) 거부하고 건드리지 않음; owner ≠ euid 거부; `connect` probe → 성공이면
  **live listener** → `SOCKET_IN_USE` 거부(probe 연결은 frame 없이 drop, listener는 peer hang-up으로 처리); `ECONNREFUSED`면
  stale → probe 전후 `(dev, ino)`가 같을 때만 unlink(그 사이 bind한 listener는 보존); 다른 오류는 그대로.
- **bound socket**: `chmod 0600`.
- **state root**: 생성하는 디렉토리는 `0700`(`DirBuilder::mode`); 기존 root는 lease 획득 시 `uid == euid && mode & 0o022 == 0`
  검증, 아니면 `STATE_ROOT_INSECURE`(`chmod go-w` 안내). 남이 **읽는** root는 operator 선택으로 허용, **쓰는** root는 거부.
- `IpcError::{SocketInUse(path), SocketPathInsecure{path, reason}}` typed 추가(Display에 code 문자열).

**검증** (`quanta-index-ipc::server::tests`, `searchd::app::runtime::tests`):

| 기준 | 검증 |
| --- | --- |
| live listener 충돌: 두 번째 bind `SOCKET_IN_USE`, 첫 socket inode 유지, 첫 listener는 probe 뒤에도 accept | `a_live_listener_keeps_its_path_and_a_second_bind_is_refused` |
| stale socket(listener drop 후 남은 파일) 회수 + 새 socket `0600` | `a_stale_socket_is_reclaimed_and_the_bound_socket_is_private` |
| server가 만든 dir(2단계) `0700` | `a_directory_the_server_creates_is_private` |
| 0777 dir 거부(사유 "writable by others"), 1777(sticky) 허용, 0770(group-write) 거부 | `a_socket_directory_others_can_write_is_refused_unless_sticky` |
| symlink parent는 대상으로 판정: private 대상 허용, 0777 대상 거부 | `a_symlinked_socket_directory_is_judged_by_its_target` |
| regular file at path: 거부 + 내용 보존 | `a_regular_file_at_the_socket_path_is_refused_and_left_alone` |
| 새 state root `0700`; 0777 root → `STATE_ROOT_INSECURE`(`chmod go-w`); 0755 root 허용 | `a_created_state_root_is_private_and_a_writable_one_is_refused` |
| 기존 test: superseded server drop이 replacement socket을 unlink하지 않음(그대로 green) | `dropping_superseded_server_does_not_unlink_replacement_socket` |

**정직한 한계**: (a) "다른 uid 소유" case는 root 없이 test 불가 — code path는 uid 비교로 명시. (b) shared mode(group/ACL
분리, query/control/ingest 권한 분리, `SO_PEERCRED`/`LOCAL_PEERCRED` peer credential)는 만들지 않았다 — private mode만.
(c) socket path별 lease(finding의 대안)는 live probe로 대체: probe 성공 ⇒ 거부, ECONNREFUSED ⇒ 회수 — probe와 unlink
사이의 race는 inode 재확인으로 닫았지만 "listener가 bind 직후 아직 listen 전"인 창은 OS가 bind와 listen을 원자적으로 하지
않으므로 이론상 남는다(std `UnixListener::bind`는 bind+listen을 연속 호출). (d) 기존 socket이 `0666`처럼 느슨한 mode로
남아 있어도 owner가 나면 회수 후 `0600`으로 다시 만든다 — 기존 파일의 mode는 검사 대상이 아니다(어차피 교체).

## 3.28 QI-BB-015 — metric은 이름별로 집계되어 한 번의 scrape로 나오고, ring은 진단 tail일 뿐이다 (구현 완료)

**진단 확정**: `BoundedQueryObsStore`가 4,096 sample ring + **무한** `Vec<ObsError>`뿐이었다 — 어떤 total도 ring이
잘랐고(오래된 sample은 조용히 소실), error는 process 수명 동안 자랐고, 읽을 방법은 harness가 store를 직접 잡는 것뿐이라
운영자는 daemon에서 어떤 수치도 꺼낼 수 없었다. socket 서버의 refused/live 수, cache/registry/writer 통계는 각자 가진 채
아무 데도 모이지 않았다.

**구현**:

- **집계 store** (`search-plane::observability`, query_dispatcher에서 분리): sample은 emit 즉시 이름별 aggregate로 fold —
  counter는 정확한 합(saturating), gauge는 마지막 값, histogram은 count/sum/min/max + 14-bound cumulative bucket
  (`1,2,5,…,30000` + `+Inf`). 이름은 첫 sample의 kind에 묶이고(kind 충돌은 `OBS_INVALID_METRIC`), emit 시 이름
  `[a-z][a-z0-9_]*`·finite value·counter 증분 non-negative integer를 검증해 wire에서 거짓말이 될 sample은 aggregate에도
  tail에도 넣지 않고 error로만 기록. sample ring(4,096)과 error ring(**256**)은 `BoundedRing<T>` 하나로 — 둘 다 recorded/
  dropped를 센다(`MetricsDiagnosticsV1`).
- **route metric**: 11개 route 모두 `observed_route()` wrapper 아래로 — intake 뒤 route 본체, 그 뒤 `lq_route_<route>_latency_ms`
  (histogram) + `lq_route_<route>_{served,errors}_total`. 이름은 `QueryRoute` enum(closed)에서만 나오고 request 내용은 없다.
  `ClusterMembershipRead`도 이제 intake/route를 센다. `emit_metric(name: &str)`.
- **`MetricSourcePort`** (core `domains/observability`): `scrape() -> Result<Vec<MetricPointV1>, CoreError>`(counter u64 /
  gauge f64). 구현: `IpcServerCounters`(ipc; accepted/refused/live/decode_failures/overloaded/refused_shutting_down/dispatched/
  peer_hangups — `UdsServer::bind_observed`로 composition root가 bind 전에 만들어 등록), `SnapshotRegistries`(track별 10개),
  `DirectSearchCorpusMaterializer`(ingest envelope 5개), `LexicalAdapter`(writer envelope 6 + regex cache 7),
  `CachingEmbeddingProvider`(8), `BootInventoryReportV1`(12 gauge). 이름 prefix로 서로 겹치지 않는다.
- **`ObservabilityScrape`**: store aggregate + 모든 source를 이름순으로 합쳐 `MetricsSnapshotV1` 하나로. source 실패는 그
  오류 그대로, 잘못된/중복 이름·NaN gauge는 `METRICS_SOURCE_DEFECT` — 구멍 난 snapshot은 절대 내지 않는다.
  `SearchPlaneControlDispatcher::new`의 6번째 인자(composition root가 source 목록을 **socket bind 전에** 고정).
- **wire** (`contract::ipc::metrics`): `MetricsSnapshotRequest`(빈 struct, field 있으면 거부) → `MetricsSnapshotV1`
  {counters, gauges, histograms, diagnostics}. **wire의 모든 수는 finite** — histogram은 finite bound bucket만 싣고 `+Inf`
  bucket은 `count`가 대신한다(첫 verify에서 `--output json`이 `+Inf`를 `null`로 써서 되읽지 못함 → bucket 설계 변경; JSON/CBOR
  모두 round trip). decode가 거부: 이름 규칙, 비-finite(gauge/sum/min/max/le: NaN·±Inf), bucket 비-cumulative/비-ascending,
  bucket count > `count`, kind 간 이름 중복, unknown/missing field. 이름 규칙은 Prometheus metric name 문법과 같아
  exposition이 escape 없이 나온다. store의 histogram sum은 `f64::MAX`에서 saturate.
- **surface**: SDK `client.observability().metrics_snapshot()` / `control().metrics_snapshot()`; harness `metrics_snapshot()`;
  `searchctl metrics [--output pretty|json|prometheus]` — prometheus는 `# TYPE` + `_bucket{le="…"}`/`_sum`/`_count`,
  diagnostics는 `searchd_obs_*_total` counter로; `--output prometheus`는 `metrics` 외 subcommand에서 usage error.

**검증**:

| 기준 | 검증 |
| --- | --- |
| counter total은 ring 용량(4,096) 너머에서도 정확, ring은 4,096 유지 + dropped 계수 | `search-plane::observability::tests::counter_totals_survive_the_sample_ring_being_full` |
| gauge 마지막 값, histogram cumulative bucket(0.5/1/1.5/30001 → [2,3,3,…,4]) + `+Inf` | `gauges_keep_the_last_value_and_histograms_bucket_cumulatively` |
| bad name/NaN/∞/0.5/-1/kind 충돌 7건 전부 `OBS_INVALID_METRIC`, aggregate·tail 미반영 | `invalid_samples_are_refused_and_recorded_not_aggregated` |
| error ring 256 bound + dropped 40, 가장 오래된 보존 = 41번째 | `the_error_ring_is_bounded_and_counts_its_drops` |
| scrape merge 정렬 / 실패 source·bad name·NaN·중복 → typed | `scrape_merges_sources_into_one_sorted_snapshot`, `scrape_refuses_defective_sources_typed` |
| control route가 store+source를 합치고 두 번째 scrape가 커진 total을 본다; 결함 source는 `INTERNAL`/`METRICS_SOURCE_DEFECT` | `control_dispatcher::tests::metrics_snapshot_route_{merges_the_store_and_every_source,refuses_a_defective_source_typed}` |
| wire: CBOR + **JSON** round trip + envelope kind tag, 이름 규칙 9 negative, 11개 거짓 shape 각각 **지정된 사유**로 거부, NaN 5곳 + ±Inf 2곳 거부, 음수 gauge 허용, 빈 request | `contract/tests/ipc_control_metrics_contract.rs` (5 tests) |
| dispatcher 7개 route unit test의 sample 이름 목록이 route tail을 포함 | `query_dispatcher::tests::*_emits_closed_obs_metric*` (갱신) |
| ipc: overload 1/dispatched 2/accepted 3, cap refusal 1, deadline proof dispatched 2·hangup 0, decode failure 1 | `ipc/tests/admission.rs` (3, 갱신) + `server::tests` empty-frame |
| e2e(실 daemon, control UDS): 5 query 사이 scrape delta — served 5/errors 0/intake 5/latency count 5/`+Inf`==count/ipc query dispatched·accepted 5/control dispatched 1/registry hits 5 == `lq_snapshot_lexical_hit_total` delta/samples_recorded **30**(=5×6)/errors 0; timeout query → errors 1·plan_limit 1·latency +1·samples **+5** | `searchd-runtime/tests/e2e_metrics_scrape.rs::route_socket_registry_and_diagnostic_tallies_move_by_exactly_the_traffic_sent` |
| e2e: boot gauge 12개 == `boot_inventory()` 값, seal release 1·open writers 0·heap 0, 세 plane refused/overloaded 0, control live 1(scrape 자신) | `…::boot_gauges_match_the_boot_inventory_and_the_writer_envelope_reflects_the_seal` |
| perf-chaos 8 rail: route family는 closed set, stream 끝 2개가 같은 route의 latency+outcome이고 outcome은 pipeline tail(typed error 여부)과 일치 | `e2e_perf_chaos.rs::assert_route_tail` (hybrid rail은 공용 helper로 통합) |
| CLI: control mock 위 `metrics --output prometheus` exposition 정확 일치, `--output json` round trip; renderer 3종 golden; parse/usage | `searchctl/tests/cli_smoke.rs::metrics_*`, `searchctl::tests::render_metrics_*`, `metrics_parses_alone_and_prometheus_output_is_refused_elsewhere` |
| SDK: 스냅샷 그대로 반환·요청 kind, 잘못된 kind → `Protocol`, daemon typed 거부 → `Remote` | `sdk::tests::observability_metrics_snapshot_*` |

**정직한 한계**: (a) aggregate는 **이름별**이다 — `Dimensions`(repo/generation)는 sample tail에만 남고 scrape에는 per-repo
series가 없다(cardinality를 등록된 이름 수로 묶는 선택; per-repo는 별도 설계). (b) histogram bound는 고정 ladder 하나(ms와
count 공용) — count 계열(`lq_engine_fanout_count`)은 상위 bucket이 비어 있다. (c) scrape는 control plane의 serial dispatch를
탄다 — 긴 activation 뒤에 줄을 선다(admission policy의 queue wait 안). (d) `bind_with_policy`는 plane 이름 `unnamed`
counters를 만든다 — scrape에 등록되지 않으므로 이름은 나가지 않지만, ipc 단독 사용자는 `bind_observed`로 plane을 줘야 한다.
(e) Prometheus exposition에 `# HELP`는 없다(설명 문자열을 wire에 싣지 않음). (f) `IpcServerCountersSnapshot`은 field별
atomic이지 set 전체가 원자적이지는 않다.

## 3.29 W5 phase 2 — request budget는 lexical native scan과 candidate loop **안에서** 관측된다 (구현 완료)

**진단 확정**: QI-BB-002 phase 1의 cooperative budget은 dispatcher의 checkpoint(`lexical:entry`/`lexical:search`/
`lexical:project`)에서만 보였다 — tantivy collect, `index:no` 전량 scan, regex 후보 검증, predicate scope 전량 scan은 한번
시작하면 corpus 끝까지 돌았고, 떠난 peer나 지난 deadline은 그 다음 checkpoint에서야 관측됐다. `LexicalSearcher` port는
budget을 받지도 않았다.

**구현**:

- **port**: `LexicalSearcher::{search, search_constrained, search_symbols, search_symbols_constrained, search_symbols_all,
  search_all, explain_candidate}`가 `budget: &RequestBudgetV1`을 받는다. dispatcher는 모든 call site에서 request의 budget을
  그대로 넘긴다(구조적 lane의 `search_all`/`search_symbols_all` 포함). `RequestBudgetV1::interrupted_at(stage)`(checkpoint의
  Option 형) 추가.
- **adapter 내부 threading**: budget이 `prepare_executable_query → prepare_predicate_plan → 8종 predicate scan`, `compile_query_
  with_constraints → compile_query_from_prepared → compile_expr/compile_leaf/compile_filter → compile_regex_content_leaf → regex
  verify`, `manual_text/symbol_search → manual_doc_matches → manual_predicate/expr/leaf/filter_matches`까지 **명시적 인자**로
  간다(thread-local 등 암묵 context 없음 — 새 scan을 추가하면 signature가 budget을 요구한다).
- **`budgeted_search`** (`lexical::budgeted_search`, crate-private): `Searcher::search`를 재현하되 weight를 `BudgetedWeight`로
  감싼다. `scorer()`는 `BudgetedScorer`(advance/seek마다 tick, 관측 시 `TERMINATED`로 조기 소진 — `Count`, `for_each`,
  `count()` 경로), `for_each_pruning`은 **inner에 위임하고 callback만 가로챈다** — block-WAND 유지, 관측 후엔 threshold
  `Score::MAX`를 돌려 남은 block을 전부 prune. segment 사이에서는 budget을 직접 본다. `BudgetProbe`: 첫 tick과 이후
  1,024 tick마다 `Instant::now()` 1회 + atomic load(sticky). query path의 native search 11곳 전부 이 함수로(collect_bounded 4
  caller, manual scan 2, predicate/repo/content scope 5, authority scan 2).
- **regex verify**: `lq-regex::RegexExecutor::execute_interruptible(candidates, corpus, budget_ms, &dyn Fn() -> bool)` +
  `RegexErrorCode::Interrupted`("INTERRUPTED", 멈춘 후보 index 명시). 기존 `execute_with_budget`은 `&|| false`로 위임. adapter는
  Interrupted를 `probe.interruption_error("lexical:regex-verify")`로 typed 변환.
- **manual scan loop**: doc마다 probe tick, 관측 시 `lexical:scan` typed.
- checkpoint 이름: `lexical:collect`, `lexical:scan`, `lexical:regex-verify`, `lexical:predicate-scope`, `lexical:repo-scope`,
  `lexical:content-scope`, `lexical:authority-scan`.

**검증**:

| 기준 | 검증 |
| --- | --- |
| unbounded budget은 Count/TopDocs 결과 그대로 | `budgeted_search::tests::an_unbounded_budget_collects_everything` |
| probe는 첫 tick + 1,025번째 tick에서 본다, sticky, 메시지에 stage | `the_probe_looks_on_the_first_tick_and_every_interval` |
| cancel된 budget: 50,000 doc counting scorer가 **≤ 1,024 advance**에서 멈추고 `REQUEST_CANCELLED … checkpoint \`lexical:collect\``; 같은 query가 live budget이면 50,000 advance·count 50,000 | `a_cancelled_budget_stops_a_count_walk_within_one_interval` (scorer advance 수 oracle) |
| 지난 deadline: 실제 term union(block-WAND) top-k에서 `REQUEST_DEADLINE_EXCEEDED` at `lexical:collect`, live면 10 hit | `a_passed_deadline_interrupts_a_top_k_term_union` |
| 실 generation 3,000 doc: keyword query cancel/deadline → `lexical:collect`; regex(`token_[0-9]+`, 전 doc prefilter) cancel → `lexical:regex-verify`, 그 후 control이 serve하고 **cache된 뒤엔** `lexical:collect`에서 관측(QI-BB-024 cache와의 상호작용을 명시); `index:no` → `lexical:scan`; 각 control은 10 row(scan은 exact_total 3,000) | `lexical/tests/cancellation_inside_search.rs` (3 tests) |
| dispatcher가 넘기는 budget은 request의 것: stub searcher가 받은 budget을 cancel하고 `stub:collect` checkpoint로 답하면 응답이 그 code/message이고 테스트가 쥔 budget 자체가 cancelled | `query_dispatcher::tests::the_request_budget_reaches_the_lexical_searcher` |
| lq-regex: 3번째 check에서 true → 후보 2 앞에서 `INTERRUPTED`(index 명시), check 호출 3회; false면 결과 동일 | `executor::tests::execute_interruptible_stops_at_the_first_true_check` |
| 기존 rail: lexical 79 + search-plane 246 + core 43 + lq-regex 74 unit/integration, perf-chaos 43, text-route hellgate 8, explain trace 3, metrics scrape 2 | 전부 green |

**정직한 한계**: (a) `for_each_pruning`에서 block-WAND가 아닌 scorer(phrase, term-set/candidate restriction, range)는 관측 후
callback 호출은 멈추지만 inner loop가 남은 doc의 score 계산은 계속한다 — collector 작업만 절약(`Count`/`for_each`/segment 경계는
완전 조기 종료). (b) semantic(lancedb) 검색과 sidecar phrase/trigram lookup은 이 phase 밖 — dispatcher checkpoint만. (c) probe
간격 1,024는 상수(측정 근거 없이 "Instant::now 1회 ≪ 1,024 posting" 추정). (d) e2e(실 daemon에서 mid-collect 시점에 peer
disconnect)는 timing 의존이라 만들지 않았다 — adapter 단위(cancel된 budget으로 결정적)와 dispatcher 단위(budget 동일성)로 분해
증명. (e) `RegexErrorCode::Interrupted`는 lq-regex wire code 집합에 추가된 새 코드(`INTERRUPTED`) — lexical adapter가 core
code로 번역하므로 IPC에는 나가지 않는다.

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
| 2026-09-17 | 1346134 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,020 passed / 0 failed (QI-BB-024 regex cache bounds 포함) |
| 2026-09-17 | c21c2f8 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,043 passed / 0 failed (QI-BB-009 + QI-BB-021 resource envelope 포함) |
| 2026-09-17 | 0d3a760 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,057 passed / 0 failed (QI-BB-020 auxiliary catalog rows 포함) |
| 2026-09-17 | dd2246b | `just rust-profile verify-rust` | RED — `quanta-index-ipc::handle_connection_returns_peer_closed_after_successful_round_trip` frame truncated: W5 peer watch의 half-close 오분류(IMPL-J, → 다음 commit) |
| 2026-09-17 | 46a2159 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,064 passed / 0 failed (QI-BB-023 history order/cursor + IMPL-J 포함) |
| 2026-09-17 | 46d6675 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,066 passed / 0 failed (QI-BB-019 canonical hybrid seed 포함) |
| 2026-09-17 | 75899ef | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,066 passed / 0 failed (QI-BB-018 true hybrid 포함) |
| 2026-09-17 | a9644a3 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,082 passed / 0 failed (QI-BB-027 ANN sealed contract 포함) |
| 2026-09-17 | 1cf1b5b | `just rust-profile verify-rust` | RED — `searchd-harness::ui::tests::seeded_ui_rail_runs_and_anchors_every_probe`: UI rail이 옛 `presence_probe` strategy를 기대. rail을 scored explain 계약(row Σ == score)으로 갱신(→ 다음 commit) |
| 2026-09-17 | 80b4fda | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,091 passed / 0 failed (QI-BB-022 explain score trace + UI rail 포함) |
| 2026-09-17 | 96c4a48 | `just rust-profile verify-rust` | **INVALID** — 실행 중에 QI-BB-016 편집이 working tree에 겹쳐 doctest 단계가 중간 상태를 컴파일(자체 절차 위반: dirty tree에서 verify 금지). 결과 폐기, 다음 commit에서 96c4a48 포함 재검증 |
| 2026-09-17 | f68e254 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,103 passed / 0 failed (QI-BB-008 RepoMap + QI-BB-016 writer envelope 포함; 96c4a48의 INVALID run 대체) |
| 2026-09-17 | 0c6854d | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,110 passed / 0 failed (QI-BB-014 UDS private bind + state-root 0700 포함) |
| 2026-09-17 | (QI-BB-015 tree) | `cargow --lane test-fast-lane test -p {contract,core,search-plane,ipc,sdk,searchctl,embed,lexical,searchd}` + `searchd-runtime --test e2e_perf_chaos --test e2e_metrics_scrape --test e2e_snapshot_registry --test end_to_end` + `searchd-harness` | 전부 green (perf_chaos 43/43, metrics_scrape 2/2, cli_smoke metrics 2종 포함). 첫 회차 RED 3건은 oracle 보정: 이름 있는 counter만 delta 계산(absent=0), timeout error sample 수 4→**5**(snapshot hit이 execution 전에 emit), dispatcher unit 7건에 route tail 추가 |
| 2026-09-17 | (QI-BB-015 tree) | `just rust-fuzz-smoke` | 4 target 60s 각각 완주, crash 0 (control request/response decoder에 `MetricsSnapshot` variant 추가 후) |
| 2026-09-17 | (QI-BB-015 tree) | `just rust-hexagonal` / `just semgrep` / module-discipline / error-shape / digest / derive-allowlist / cargo-toml-hygiene / `just rust-public-api-update` / `just rust-cargo-modules-update` / `just rust-test-authority` / `just fmt-check` / clippy-lane `--workspace --all-targets` | 전부 green; public-api baseline(contract, sdk)·cargo-modules baseline(contract, core) 갱신은 additive |
| 2026-09-17 | c6b5515 | `just rust-profile verify-rust` | RED — `searchctl::tests::render_metrics_json_round_trips_the_snapshot`: serde_json이 `+Inf` bucket bound를 `null`로 써서 `--output json`이 되읽히지 않음(fail-closed decode가 잡음). wire에서 `+Inf` bucket 제거, `count`가 대신(→ 다음 commit) |
| 2026-09-17 | eabe133 | `just rust-profile verify-rust` | **GREEN** — exit 0, 2,132 passed / 0 failed (QI-BB-015 metrics 집계 + scrape, finite wire 포함) |
| 2026-09-17 | (W5 phase 2 tree) | `cargow --lane test-fast-lane test -p {lexical,search-plane,lq-regex,core}` (643/643) + `searchd-runtime --test {e2e_perf_chaos,e2e_text_route_hellgate,e2e_explain_score_trace,e2e_metrics_scrape}` (56/56) + workspace clippy + hexagonal/semgrep/module-discipline/error-shape/public-api/cargo-modules | 전부 green. 첫 회차: regex cancel test가 `lexical:collect`에서 관측 → 원인은 control run이 먼저 regex match cache를 데운 것(정당한 동작) → cancel run을 먼저 돌리고 cache 뒤 동작을 두 번째 단언으로 추가 |
