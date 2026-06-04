# Jun 4 DSL Capability Inventory

기준:
- live code + current exact rail 기준
- 문서 claim이 아니라 executable owner seam, runtime corpus, front-door, parity rail을 우선함
- `지원됨`:
  code path가 executable이고 현재 tree에서 exact green rail이 하나 이상 있음
- `부분 지원`:
  code/lowering seam은 열려 있지만 exact rail matrix가 덜 찼거나, Sourcegraph route semantics가 native와 다름
- `미지원`:
  typed fail-closed, parser-only, 별도 carrier, 또는 현재 stack에 executable owner가 없음

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
| lexical option | `timeout:` | 예 | 예 | regex-backed lexical query에서만 executable |
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
| lexical predicate | `file.contains(path:..., <scalar>)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `file.contains(file:..., <scalar>)` | 예 | 예 | exact runtime/front-door green |
| lexical predicate | `file.has.content(lang:..., /.../)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical predicate | `repo.has.file(path:...)` / `name:...` / `lang:...` | 예 | 예 | exact runtime/parity green |
| lexical predicate | `repo.has.file(<scalar-path>)` | 예 | 예 | scalar-path shorthand |
| lexical predicate | `repo.has.content(<keyword|phrase|raw|number>)` | 예 | 예 | exact runtime/front-door green |
| lexical alias | `repo.has.path(<scalar-path>)` | 예 | 예 | canonicalized to `repo.has.file(path:...)` |
| lexical alias | `repo.contains.content(<scalar>)` | 예 | 예 | canonicalized to `repo.has.content(...)` |
| lexical boolean | `repo.has.file(...) OR ...` / `NOT repo.has.file(...)` | 예 | 예 | repo gate boolean scope green |
| lexical boolean | `repo.has.content(...) OR ...` / `NOT repo.has.content(...)` | 예 | 예 | repo gate boolean scope green |
| structural native | root-kind / root-capture / wildcard / `where` / `inside` / `outside` / variadic / typed holes | 예 | 해당 없음 | native structural surface |
| structural mixed | lexical + structural `AND` / `OR` / `AND NOT` / pure-negative root | 예 | 부분 | native는 전체, SG는 subset |
| structural SG subset | `patterntype:structural` + structural body + lexical `Keyword` / `RawString` / proved `Predicate` sibling | 해당 없음 | 예 | current green subset |

## 부분 지원

| 표현식 / 시나리오 | Native | Sourcegraph 지원 | 현재 상태 | 근거 |
| --- | --- | --- | --- | --- |
| `file.has.content(path:..., <scalar>)` / `file.has.content(file:..., <scalar>)` | 부분 | 부분 | content-scope parser/lowering seam은 열려 있으나 exact runtime/front-door/parity row를 이번 inventory에서 못 찾음 | registry comment + shared lowering path 존재, asserting rail 부족 |
| `repo.has.file(path+name)` / `path+lang` / `name+lang` / `path+name+lang` | 부분 | 부분 | matcher combination admit은 보이지만 exact execution/parity matrix 부족 | syntax/lowering admit, singleton rails 위주 |
| `repo.contains.content(...) OR ...` / `NOT repo.contains.content(...)` | 부분 | 부분 | alias는 canonical rewrite되지만 alias boolean exact rail은 없음 | canonical boolean rails only |
| `symbol.has.name(...)` | 부분 | 부분 | planner symbol-route seam은 있으나 shared runtime corpus/front-door capability inventory에는 없음 | planner owner seam only |
| SG structural `Phrase` sibling | 예 | 부분 | semantic mismatch | SG structural route는 phrase를 structural body로 rewrite |
| SG structural `Regex` sibling | 예 | 부분 | semantic mismatch | SG structural route는 regex를 structural body로 rewrite |
| SG structural `Predicate` sibling 전체 | 예 | 부분 | proved subset보다 matrix가 넓음 | current green rail은 `repo.has.file(...)` 중심, 나머지 predicate family exact matrix 부족 |

## 미지원 DSL

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 상태 / 이유 |
| --- | --- | --- | --- | --- |
| parser-only | empty query | 아니오 | 아니오 | parser는 `LqExpr::Empty` 생성, executable route는 reject |
| lexical predicate | shipped subset 밖 predicate name | 아니오 | 아니오 | `LEX_PREDICATE_UNIMPLEMENTED` typed fail |
| lexical predicate | unsupported predicate arg shape | 아니오 | 아니오 | executable registry closed set 바깥 |
| lexical predicate | `file.contains(name:..., <scalar>)` | 예 | 아니오 | native rail은 있으나 SG는 unsupported scoped content shape로 typed fail |
| lexical predicate | scoped content predicate inside `OR` / `NOT` | 예 | 아니오 | native LQ는 되지만 SG bridge는 scoped filters under `OR/NOT` 불가 |
| lexical predicate | `file:contains(\"a\", \"b\")` 같은 multi-scalar content shape | 아니오 | 아니오 | typed fail |
| runtime catalog | `dirty:only` | 아니오 | 아니오 | explicit typed fail |
| Sourcegraph predicate family | `repo:has.commit.after(...)` | 아니오 | 아니오 | executable registry 없음 |
| Sourcegraph predicate family | `repo:has.description(...)` | 아니오 | 아니오 | executable registry 없음 |
| Sourcegraph predicate family | `repo:has.tag(...)` | 아니오 | 아니오 | executable registry 없음 |
| Sourcegraph predicate family | `repo:has.meta(...)` | 아니오 | 아니오 | executable registry 없음 |
| Sourcegraph predicate family | `file:has.owner(...)` | 아니오 | 아니오 | executable registry 없음 |
| Sourcegraph predicate family | `file:has.contributor(...)` | 아니오 | 아니오 | executable registry 없음 |
| structural | SG structural pre-shaped `StructuralBlock` leaf | 아니오 | 아니오 | `BridgeTranslateFail` |
| directives | `index:no` | 해당 없음 | 아니오 | explicit refused directive |
| directives | `boost:` | 해당 없음 | 아니오 | explicit refused directive |

## 비실행 / 별도 carrier

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 비고 |
| --- | --- | --- | --- | --- |
| bridge carrier | `into:codeql` / `scope:results` / `with:lexical` | 해당 없음 | 해당 없음 | runtime search-result surface가 아니라 bridge packet carrier |
| directives | `index:yes` / `index:only` | 해당 없음 | 해당 없음 | accepted but normalized away on this index-only stack; distinct executable capability 아님 |

## 감사상 주의점

- `tools/benchmark/sourcegraph_parity.py --check` green은 필요조건이지 충분조건이 아니다.
  - 현재 guard는 filter root keyword 중심이라 alias-shape / predicate combination matrix를 전부 증명하지 않는다.
- `frontdoor_scenarios.rs` / `dsl_scenarios.rs` 같은 shared inventory는 현재 live executable surface보다 좁다.
  - capability truth는 exact runtime/front-door/parity rail을 우선했고, shared catalog 누락 자체는 별도 audit residue다.

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
  - `crates/quanta-index-searchd-harness/src/scenarios.rs`

## 현재 결론

- lexical / history / runtime-catalog / core structural DSL은 현재 tree에서 넓게 executable이다.
- Sourcegraph text route도 `content`, `case`, `count`, `type`, `select`, `patterntype`, `timeout`, repo metadata, 주요 predicate/alias surface까지 상당수 커버한다.
- 아직 `Sourcegraph 완전 parity`는 아니다.
  - SG structural `Phrase` / `Regex` sibling mismatch
  - combinatorial predicate matrix 일부 미증명
  - `symbol.has.name(...)` 같은 owner-seam-only surface
