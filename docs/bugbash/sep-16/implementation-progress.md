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
