# Jun 4 DSL Capability Inventory

기준:
- live code + current exact rail 기준
- 문서 claim이 아니라 executable owner seam, runtime corpus, front-door, parity rail을 우선함
- 현재 inventory의 live end-state는 `지원됨`, `미지원`, `비실행 / 별도 carrier`뿐이다
- `지원됨`:
  code path가 executable이고 현재 tree에서 owner seam + shared runtime/front-door/parity 중 필요한 증거가 있다
- `미지원`:
  typed fail-closed, parser-only, 별도 carrier, 현재 stack에 executable owner가 없거나, SG route에 direct surface가 없다

주의:
- preflight script `./scripts/check-persona-target-policy.sh`, `./scripts/cg-agent-session` 는 현재 checkout에 없다.
- 아래 표의 `Sourcegraph 지원`은 parse-only가 아니라 실제 lowering/route 기준이다.

## 현재 가능한 DSL

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 비고 |
| --- | --- | --- | --- | --- |
| lexical core | keyword / phrase / regex | 예 | 예 | text route 기본 lexical surface |
| lexical filter | `content:` / `file:` / `path:` / `repo:` / `lang:` | 예 | 예 | 기본 filter surface |
| lexical option | `case:` | 예 | 예 | parser closed set `{yes,no}` |
| lexical option | `count:` | 예 | 예 | integer 또는 `all` |
| lexical option | `type:` | 예 | 예 | `type:file` / `repo` / `symbol` / `commit` / `diff` 등 route/projection surface |
| lexical option | `select:` | 예 | 예 | `repo` / `file` / `path` / `content` / `content.match` / `symbol` |
| lexical option | `patterntype:literal` / `keyword` / `standard` / `regexp` | 예 | 예 | lexical route executable subset |
| lexical option | `timeout:` | 예 | 예 | regex-backed lexical query에서 executable |
| repo metadata | `fork:` / `archived:` / `visibility:` / `context:` | 예 | 예 | repo metadata filter surface |
| history | `type:commit before:` / `after:` / `since:` / `until:` | 예 | 예 | commit route |
| history | `type:commit since.time:` / `since.commit:` | 예 | 예 | unknown ref는 typed fail |
| history | `type:commit author:` / `committer:` / `message:` / `rev:` | 예 | 예 | commit-route filter surface |
| history | `type:diff diff.added:` / `diff.removed:` / `diff.touched:` | 예 | 예 | diff route |
| runtime catalog | `dirty:yes` / `dirty:no` | 예 | 예 | `dirty:only`는 미지원 |
| runtime catalog | `changed:` / `stale:` / `snapshot:` | 예 | 예 | unknown snapshot은 typed fail |
| runtime catalog | `meta.owner:` / `meta.service:` / `meta.layer:` / `meta.surface:` | 예 | 예 | runtime metadata surface |
| runtime catalog | `affected:` / `invalidated_by:` | 예 | 예 | runtime catalog edge authority |
| lexical predicate | `file.contains(<keyword|phrase|raw|number>)` | 예 | 예 | numeric/content scalar exact rail green |
| lexical predicate | `file.has.content(<keyword|phrase|regex|raw|number>)` | 예 | 예 | numeric/regex exact rail green |
| lexical alias | `file.contains.content(<scalar>)` | 예 | 예 | canonicalized to `file.contains(...)` |
| lexical predicate | `file.contains(path:..., <scalar>)` / `file.contains(file:..., <scalar>)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `file.has.content(path:..., <scalar>)` / `file.has.content(file:..., <scalar>)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `file.has.content(lang:..., /.../)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `repo.has.file(path:...)` / `name:...` / `lang:...` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `repo.has.file(<scalar-path>)` | 예 | 예 | scalar-path shorthand |
| lexical predicate | `repo.has.file(path+name)` / `path+lang` / `name+lang` / `path+name+lang` | 예 | 예 | combinatorial matcher matrix exact green |
| lexical predicate | `repo.has.content(<keyword|phrase|raw|number>)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical alias | `repo.has.path(<scalar-path>)` | 예 | 예 | canonicalized to `repo.has.file(path:...)` |
| lexical alias | `repo.contains.content(<scalar>)` | 예 | 예 | canonicalized to `repo.has.content(...)` |
| lexical boolean | `repo.has.file(...) OR ...` / `NOT repo.has.file(...)` | 예 | 예 | repo gate boolean scope green |
| lexical boolean | `repo.has.content(...) OR ...` / `NOT repo.has.content(...)` | 예 | 예 | repo gate boolean scope green |
| lexical boolean | `repo.contains.content(...) OR ...` / `NOT repo.contains.content(...)` | 예 | 예 | alias boolean exact rail green |
| symbol predicate | `symbol.has.name(...)` | 예 | 예 | shared symbol route exact green |
| structural native | root-kind / root-capture / wildcard / `where` / `inside` / `outside` / variadic / typed holes | 예 | 해당 없음 | native structural surface |
| structural mixed | lexical + structural `AND` / `OR` / `AND NOT` / pure-negative root | 예 | Native만 전체 | SG는 아래 proved subset만 shipped |
| structural SG subset | `patterntype:structural` + structural body + lexical `Keyword` / `RawString` sibling | 해당 없음 | 예 | exact lowering/runtime/parity green |
| structural SG subset | `patterntype:structural` + structural body + repo gate predicate sibling `repo.has.file` / `repo.has.path` / `repo.has.content` / `repo.contains.content` in `AND` / `OR` / `AND NOT` | 해당 없음 | 예 | exact runtime/parity green |

## 미지원 DSL

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 상태 / 이유 |
| --- | --- | --- | --- | --- |
| parser-only | empty query | 아니오 | 아니오 | parser는 `LqExpr::Empty` 생성, executable route는 reject |
| lexical predicate | shipped subset 밖 predicate name | 아니오 | 아니오 | `LEX_PREDICATE_UNIMPLEMENTED` typed fail |
| lexical predicate | unsupported predicate arg shape | 아니오 | 아니오 | executable registry closed set 바깥 |
| lexical predicate | `file.contains(name:..., <scalar>)` | 예 | 아니오 | SG bridge는 이 scoped content shape를 executable subset으로 열지 않음 |
| lexical predicate | scoped content predicate inside `OR` / `NOT` | 예 | 아니오 | native LQ는 되지만 SG bridge는 scoped predicates under `OR/NOT` 불가 |
| lexical predicate | `file:contains("a", "b")` 같은 multi-scalar content shape | 아니오 | 아니오 | typed fail |
| runtime catalog | `dirty:only` | 아니오 | 아니오 | explicit typed fail |
| Sourcegraph predicate family | `repo:has.commit.after(...)` | 아니오 | 아니오 | history-backed repo-recency execution owner 없음 |
| Sourcegraph predicate family | `repo:contains.commit.after(...)` | 아니오 | 아니오 | parser/bridge admission 흔적은 있지만 history-backed repo-recency execution owner 없음 |
| Sourcegraph predicate family | `repo:has.meta(...)` | 아니오 | 아니오 | producer-side repo metadata authority 없음 |
| Sourcegraph predicate family | `repo:has.topic(...)` | 아니오 | 아니오 | producer-side repo topic authority 없음 |
| Sourcegraph predicate family | `file:has.owner(...)` | 아니오 | 아니오 | people-ownership authority 없음 |
| Sourcegraph predicate family | `file:has.contributor(...)` | 아니오 | 아니오 | file-level contributor authority 없음 |
| Sourcegraph filter / select | `rev:at.time(...)` | 아니오 | 아니오 | existing history timeref substrate는 있지만 revision-at-time resolution owner 없음 |
| Sourcegraph select surface | `select:file.owners` | 아니오 | 아니오 | people-ownership projection contract/authority 없음 |
| structural | SG structural direct lexical `Phrase` sibling | 예 | 아니오 | direct SG surface 없음. quoted phrase는 structural body로 해석됨 |
| structural | SG structural direct lexical `Regex` sibling | 예 | 아니오 | direct SG surface 없음. `/.../`는 structural regex body로 해석됨 |
| structural | SG structural mixed non-repo predicate sibling | 부분 | 아니오 | SG structural mixed predicate subset은 repo gate family만 shipped. native structural evaluator는 generic lexical leaf path를 갖지만, non-repo family 전체에 대한 shared exact inventory는 없고 symbol-route predicate는 structural candidate contract와도 분리되어 있어 native full support를 주장하지 않음 |
| structural | SG structural pre-shaped `StructuralBlock` leaf | 아니오 | 아니오 | `BridgeTranslateFail` |
| directives | `index:no` | 해당 없음 | 아니오 | explicit refused directive |
| directives | `boost:` | 해당 없음 | 아니오 | explicit refused directive |

## 비실행 / 별도 carrier

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 비고 |
| --- | --- | --- | --- | --- |
| bridge carrier | `into:codeql` / `scope:results` / `with:lexical` | 해당 없음 | 해당 없음 | runtime search-result surface가 아니라 bridge packet carrier |
| directives | `index:yes` / `index:only` | 해당 없음 | 해당 없음 | accepted but normalized away on this index-only stack; distinct executable capability 아님 |

## 감사상 주의점

- `tools/benchmark/sourcegraph_parity.py --check`는 이제 두 층을 본다.
  - accepted `SgFilter` keyword surface
  - canonical predicate/alias surface id (`repo.has.file`, `repo.has.path`, `repo.has.content`, `repo.contains.content`, `file.contains`, `file.contains.content`, `file.has.content`, `symbol.has.name`)
- 다만 이 guard도 모든 조합 행렬을 대신하지는 않는다.
  - combinatorial matcher proof와 structural mixed boolean proof의 최종 authority는 여전히 exact runtime/front-door/parity rail이다.
- native structural mixed non-repo predicate sibling은 code path만 보면 generic lexical subquery evaluator에 태워진다.
  - 근거: `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  - 하지만 shared exact inventory가 없고 `symbol.has.name(...)`처럼 result contract가 structural chunk candidate와 다른 family도 섞여 있어서, 현재 문서는 native full support를 고정하지 않는다.

## 최신 검증 스냅샷

- 재실행 일시:
  - 2026-06-04 Asia/Seoul
- exact rail:
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture`
    - status: `8 passed`
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture`
    - status: `20 passed`
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
    - status: `1 passed`
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
    - status: `4 passed`
  - `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
    - status: `95 passed`
  - `python3 tools/benchmark/sourcegraph_parity.py --check`
    - status: `green`
- preflight:
  - `./scripts/check-persona-target-policy.sh --expect-agent`
  - `./scripts/cg-agent-session`
  - 둘 다 checkout에 없어 `unverified`

## 코드 기준 핵심 소스

- predicate SSOT:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - `crates/quanta-index-lexical/src/planner.rs`
  - `crates/quanta-index-lexical/src/lib.rs`
- Sourcegraph lowering:
  - `crates/quanta-index-lq-bridge/src/translator.rs`
  - `crates/quanta-index-search-plane/src/lowering.rs`
- executable / proof inventory:
  - `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
  - `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  - `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
  - `tools/benchmark/sourcegraph_parity.py`

## 현재 결론

- lexical / history / runtime-catalog / core structural DSL은 현재 tree에서 executable이다.
- Sourcegraph text route는 shipped predicate/alias surface와 shared symbol route까지 포함해 현재 inventory 기준으로 커버된다.
- `부분 지원` 셀은 없다. 현재 inventory는 `지원됨`, `미지원`, `비실행 / 별도 carrier` 세 상태만 쓴다.
- 남은 경계는 intentional unsupported뿐이다.
  - SG structural direct lexical `Phrase` / `Regex` sibling 없음
  - SG structural mixed non-repo predicate sibling 없음
  - shipped subset 밖 predicate family 없음

## 남은 작업

- mandatory residue:
  - 없음
- optional new-scope only:
  - SG structural direct lexical `Phrase` sibling을 새 executable surface로 열기
  - SG structural direct lexical `Regex` sibling을 새 executable surface로 열기
  - SG structural mixed non-repo predicate sibling family를 새 executable surface로 열기
  - shipped subset 밖 Sourcegraph-style predicate family를 새 registry scope로 열기
- Sourcegraph parity next-step split:
  - internal substrate extension required:
    - `repo:has.commit.after(...)`
    - `repo:contains.commit.after(...)`
    - `rev:at.time(...)`
  - producer-side authority 확정 전까지 blocked:
    - `repo:has.meta(...)`
    - `repo:has.topic(...)`
    - `file:has.owner(...)`
    - `select:file.owners`
    - `file:has.contributor(...)`
