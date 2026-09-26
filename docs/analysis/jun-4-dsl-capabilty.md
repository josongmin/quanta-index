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
| lexical option | `select:` | 예 | 예 | `repo` / `file` / `path` / `content` / `content.match` / `symbol` / `file.owners` |
| lexical option | `patterntype:literal` / `keyword` / `standard` / `regexp` | 예 | 예 | lexical route executable subset |
| lexical option | `timeout:` | 예 | 예 | regex-backed lexical query에서 executable |
| repo metadata | `fork:` / `archived:` / `visibility:` / `context:` | 예 | 예 | repo metadata filter surface |
| history | `type:commit before:` / `after:` / `since:` / `until:` | 예 | 예 | commit route |
| history | `type:commit since.time:` / `since.commit:` | 예 | 예 | unknown ref는 typed fail |
| history | `type:commit author:` / `committer:` / `message:` / `rev:` | 예 | 예 | commit-route filter surface |
| history | `type:diff diff.added:` / `diff.removed:` / `diff.touched:` | 예 | 예 | diff route |
| runtime catalog | `dirty:yes` / `dirty:no` / `dirty:only` | 예 | 예 | `dirty:only`는 current runtime-doc surface에서 `dirty:yes`와 같은 dirty-doc subset으로 실행된다. runtime/front-door/chaos/corpus green |
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
| lexical predicate | `repo.has.file(path:... content:...)` | 예 | 예 | SGX-01: per-document path∧content correlation (`Occur::Must` on one doc), NOT a repo-level cross-product. anti-overmatch + runtime/parity/corpus/front-door green |
| lexical predicate | `repo.has.file(<scalar-path>)` | 예 | 예 | scalar-path shorthand |
| lexical predicate | `repo.has.file(path+name)` / `path+lang` / `name+lang` / `path+name+lang` | 예 | 예 | combinatorial matcher matrix exact green |
| lexical predicate | `repo.has.content(<keyword|phrase|raw|number>)` | 예 | 예 | exact runtime/front-door/parity green |
| lexical alias | `repo.has.path(<scalar-path>)` | 예 | 예 | canonicalized to `repo.has.file(path:...)` |
| lexical alias | `repo.contains.file(...)` | 예 | 예 | SGT-01: canonicalized to `repo.has.file(...)`, forwards full path/name/lang matcher surface. registry/bridge/runtime/parity green |
| lexical alias | `repo.contains.path(<scalar-path>)` | 예 | 예 | SGT-01: canonicalized to `repo.has.file(path:...)`. registry/bridge/runtime/parity green |
| lexical alias | `repo.contains.content(<scalar>)` | 예 | 예 | canonicalized to `repo.has.content(...)` |
| lexical boolean | `repo.has.file(...) OR ...` / `NOT repo.has.file(...)` | 예 | 예 | repo gate boolean scope green |
| lexical boolean | `repo.has.content(...) OR ...` / `NOT repo.has.content(...)` | 예 | 예 | repo gate boolean scope green |
| lexical boolean | `repo.contains.content(...) OR ...` / `NOT repo.contains.content(...)` | 예 | 예 | alias boolean exact rail green |
| Sourcegraph predicate family | `repo:has.commit.after(...)` | 예 | 예 | quanta-index contract/sdk/runtime/front-door/parity green. Semantica ingress owner auto-emits repo commit recency from history publish and live ingress roundtrip proof is green |
| Sourcegraph predicate family | `repo:contains.commit.after(...)` | 예 | 예 | canonical alias parity green, including live producer ingress proof |
| Sourcegraph predicate family | `repo:has.meta(key:value)` | 예 | 예 | repo-scoped metadata authority is executable on current tree. key/value exact semantics, runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `repo:has.meta(key)` / `repo:has.meta(tag:)` | 예 | 예 | SGX-03: genuine key-EXISTENCE (`contains_key`, key present with any value) — not a wildcard `key:*` nor an empty-string match. runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `repo:has.meta(/key/)` / `repo:has.meta(/key/:)` / `repo:has.meta(key:/value/)` / `repo:has.meta(/key/:value)` / `repo:has.meta(/key/:/value/)` | 예 | 예 | SGX-03: regex key-only, regex-key exact-value, exact-key regex-value, regex pair 모두 query-time `RegexExecutor`로 same-pair semantics 위에서 실행된다. malformed regex는 empty가 아니라 `LEX_REGEX_*` typed fail. runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `repo:has.description(...)` | 예 | 예 | SGX-02: distinct producer-published repo-description authority (`RepoDescriptionIngestBatch` → `repo-description.cbor` shard) is executable on current tree. one textual pattern scalar compiled as a regex over each repo's verbatim description; malformed pattern → `LEX_REGEX_*`, missing authority → `REPO_DESCRIPTION_UNAVAILABLE`. runtime/round-trip/parity/public-api green |
| Sourcegraph predicate family | `repo:has.topic(...)` | 예 | 예 | source-repo keyed repo-topic authority is executable on current tree. lowercase exact topic-set semantics, runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `file:has.owner(...)` / `file:has.owner()` | 예 | 예 | source-repo keyed file-ownership authority is executable on current tree. one textual owner arg is exact lowercase owner-identity gate; zero-arg form means any-owner. runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `file:has.contributor(...)` | 예 | 예 | source-repo keyed file-contributor authority is executable on current tree. one textual contributor arg is exact lowercase contributor-identity gate backed by file-level contributor sets. runtime/front-door/parity/corpus green |
| Sourcegraph predicate family | `file:has.contributor(/<regex>/)` | 예 | 예 | SGX-04: query/runtime surface is executable on current tree. producer contract widened to structured contributor identities (`canonical`, optional `name`, optional `email`), `/.../` arg matches normalized `name` OR `email`, and raw `canonical` regex fallback는 없다. malformed regex는 `LEX_REGEX_*` typed fail. runtime/front-door/parity/corpus green; semantica `index-sdk-ingress` live publish roundtrip is targeted green |
| Sourcegraph filter / select | `rev:at.time(...)` | 아니오 | 예 | text dispatch가 history-backed revision selection으로 pin을 재결정한다. owner-local dispatcher rail + targeted runtime rail + SDK/front-door rail green |
| symbol predicate | `symbol.has.name(<keyword>)` | 예 | 예 | keyword postings 경로. Sep-26 RBR-08: Phrase/RawString/regexp 입력은 `LEX_PLANNER_UNSUPPORTED_FILTER_COMBO` typed refusal; symbol content authority는 미지원. 아래 역사적 green 표기는 해당 당시 rail 범위다 |
| structural native | root-kind / root-capture / wildcard / `where` / `inside` / `outside` / variadic / typed holes | 예 | 해당 없음 | native structural surface |
| structural mixed | lexical + structural `AND` / `OR` / `AND NOT` / pure-negative root | 예 | Native만 전체 | SG는 아래 proved subset만 shipped |
| structural SG subset | `patterntype:structural` + structural body + lexical `Keyword` / `RawString` sibling | 해당 없음 | 예 | exact lowering/runtime/parity green |
| structural SG subset | `patterntype:structural` + structural body + repo gate predicate sibling `repo.has.file` / `repo.has.path` / `repo.has.content` / `repo.contains.content` in `AND` / `OR` / `AND NOT` | 해당 없음 | 예 | exact runtime/parity green |
| structural SG subset | `patterntype:structural` + structural body + `file.contains(path|file:...)` / `file.has.content(path|file:...)` / `symbol.has.name(...)` in `AND` / `OR` / `AND NOT` | 해당 없음 | 예 | SGX-06: exact lowering/runtime/front-door/parity green. file/content siblings run on current structural route; symbol sibling projects same-path, line-overlapping symbol hits into all matching structural chunks with deterministic union |
## 미지원 DSL

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 상태 / 이유 |
| --- | --- | --- | --- | --- |
| parser-only | empty query | 아니오 | 아니오 | parser는 `LqExpr::Empty` 생성, executable route는 reject |
| lexical predicate | shipped subset 밖 predicate name | 아니오 | 아니오 | `LEX_PREDICATE_UNIMPLEMENTED` typed fail |
| lexical predicate | unsupported predicate arg shape | 아니오 | 아니오 | executable registry closed set 바깥 |
| lexical predicate | `file:contains("a", "b")` 같은 multi-scalar content shape | 아니오 | 아니오 | typed fail |
| structural | SG structural direct lexical `Phrase` sibling | 예 | 아니오 | direct SG surface 없음. `patterntype:structural "foo bar"`의 quoted token은 lexical phrase sibling이 아니라 structural body로 해석됨 |
| structural | SG structural direct lexical `Regex` sibling | 예 | 아니오 | direct SG surface 없음. `patterntype:structural /foo.*/`의 slash token은 lexical regex sibling이 아니라 structural regex body로 해석됨 |
| structural | SG structural pre-shaped `StructuralBlock` leaf | 아니오 | 아니오 | `BridgeTranslateFail` |

## 비실행 / 별도 carrier

| 분류 | 표현식 / 시나리오 | Native | Sourcegraph 지원 | 비고 |
| --- | --- | --- | --- | --- |
| bridge carrier | `into:codeql` / `scope:results` / `with:lexical` | 해당 없음 | 해당 없음 | runtime search-result surface가 아니라 bridge packet carrier |
| directives | `index:yes` / `index:only` | 예 | 예 | canonical option carrier로 accept되지만 current index-only stack에서는 distinct executable capability를 만들지 않는다 |
| directives | `index:no` | 예 | 예 | canonical option carrier로 accept되고 active lexical rail에서 stored-doc full scan으로 실행된다. indexed route와 candidate universe parity를 유지한다 |
| directives | `boost:` | 예 | 예 | canonical option carrier(`boost_millis`)로 accept되고 active lexical rail에서 score magnitude multiplier로 실행된다. query-wide boost라 ordering은 보통 유지되고 score 크기만 달라진다 |

## 감사상 주의점

- `tools/benchmark/sourcegraph_parity.py --check`는 이제 두 층을 본다.
  - accepted `SgFilter` keyword surface
  - canonical predicate/alias/select surface id (`rev.at.time`, `repo.has.commit.after`, `repo.contains.commit.after`, `repo.has.meta`, `repo.has.meta.regex.key_only`, `repo.has.meta.regex.key_exact_value`, `repo.has.meta.regex.exact_key_value`, `repo.has.meta.regex.pair`, `repo.has.topic`, `repo.has.file`, `repo.has.path`, `repo.has.content`, `repo.contains.content`, `file.contains`, `file.contains.content`, `file.has.content`, `file.has.owner`, `file.has.contributor`, `select.file.owners`, `symbol.has.name`, `sg_structural.file_contains_predicate_sibling`, `sg_structural.file_has_content_predicate_sibling`, `sg_structural.symbol_has_name_predicate_sibling`)
  - explicit unsupported structural demotion inventory (`sg_structural.direct_phrase_lexical_sibling`, `sg_structural.direct_regex_lexical_sibling`)
- 다만 이 guard도 모든 조합 행렬을 대신하지는 않는다.
  - combinatorial matcher proof와 structural mixed boolean proof의 최종 authority는 여전히 exact runtime/front-door/parity rail이다.
- SG structural mixed non-repo predicate sibling은 이제 exact proof가 있다.
  - `file.contains(path|file:...)`
  - `file.has.content(path|file:...)`
  - `symbol.has.name(...)`
  - 근거: `crates/quanta-index-search-plane/src/lowering.rs`, `crates/quanta-index-search-plane/src/query_dispatcher.rs`, `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`, `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## 최신 검증 스냅샷

- 재실행 일시:
  - 2026-06-08 Asia/Seoul
- exact rail:
  - `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
    - status: `111 passed`
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
    - status: `13 passed`
  - `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
    - status: `4 passed`
  - `python3 tools/benchmark/sourcegraph_parity.py --check`
    - status: `OK: 38 filters and 29 required surfaces`
  - `python3 tools/ci/lint/check-dsl-capability-truth.py`
    - status: `in sync`
- verification packet rail:
  - `just rust-bench-dsl-truth`
    - status: `2 passed`
  - `just rust-verify-hellgate-fast`
    - status: `green`
  - `just rust-verify-hellgate-broad`
    - status: `green`
  - `env QUANTA_INDEX_SEARCHD_BIN=/Users/songmin/Library/Caches/quanta-index/target/daemon-lane/debug/quanta-index-searchd just rust-verify-hellgate-cross-repo`
    - status: `red in this snapshot`
    - failure class: external `semantica-codegraph-v2` boundary guard `quanta-sdk.runtime-facade-boundary.v1`
  - `just rust-bench-dsl-compare`
    - status: `green`
  - `just rust-verify-hellgate-all`
    - status: aggregate target exists, but this snapshot is recorded from the component gates above instead of one completed monolithic rerun
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
- Sourcegraph text route는 shipped predicate/alias/select surface, repo commit recency gate, repo metadata gate, repo topic gate, owner/contributor authority gate, shared symbol route까지 포함해 현재 inventory 기준으로 커버된다.
- `jun-5`와 `jun-6` 결과까지 합치면 docs baseline tail gap은 더 이상 ambiguous하지 않다.
  - 지원됨으로 종결:
    - `repo:contains.file(...)`
    - `repo:contains.path(...)`
    - `repo:has.file(path:... content:...)`
    - `repo:has.description(...)`
    - `repo:has.meta(key)`
    - `repo:has.meta(tag:)`
    - `repo:has.meta(/key/)`
    - `repo:has.meta(/key/:)`
    - `repo:has.meta(key:/value/)`
    - `repo:has.meta(/key/:value)`
    - `repo:has.meta(/key/:/value/)`
    - `file:has.contributor(<name-or-email regex>)`
- 중간 상태 셀은 없다. 현재 inventory는 `지원됨`, `미지원`, `비실행 / 별도 carrier` 세 상태만 쓴다.
- 남은 경계는 intentional unsupported뿐이다.
  - SG structural direct lexical `Phrase` / `Regex` sibling 없음
  - shipped subset 밖 Sourcegraph-style predicate family support 없음
## 남은 작업

- mandatory residue:
  - 없음
- `jun-4-sourcegraph-parity` packet residue:
  - 없음
- `jun-5-sourcegraph-tail-gaps` packet (resolved):
  - packet: [docs/plans/jun-5-sourcegraph-tail-gaps/rfc.md](../plans/jun-5-sourcegraph-tail-gaps/rfc.md)
  - 지원됨으로 종결 (SGT-01):
    - `repo:contains.file(...)` — alias → `repo.has.file`
    - `repo:contains.path(...)` — alias → `repo.has.file(path:...)`
  - 이후 `jun-6`에서 지원됨으로 승격된 셀:
    - `repo:has.file(path:... content:...)`
    - `repo:has.description(...)`
    - `repo:has.meta(key)`
    - `repo:has.meta(tag:)`
    - `repo:has.meta(/key/)`
    - `repo:has.meta(/key/:)`
    - `repo:has.meta(key:/value/)`
    - `repo:has.meta(/key/:value)`
    - `repo:has.meta(/key/:/value/)`
  - 이후 `jun-6`에서 최종 지원됨/종결된 셀:
    - `file:has.contributor(<name-or-email regex>)` (SGX-04: structured `name`/`email` authority + regex match)
    - SG structural mixed non-repo predicate sibling (SGX-06: exact support)
- `jun-6-sourcegraph-expansion` packet (landed):
  - packet: [docs/plans/jun-6-sourcegraph-expansion/rfc.md](../plans/jun-6-sourcegraph-expansion/rfc.md)
  - closed unsupported, not backlog:
    - SG structural direct lexical `Phrase` / `Regex` sibling (SGX-05)
      - reason: SG structural route에서 quoted/slash token은 direct lexical sibling slot이 아니라 structural body syntax로 소비됨
  - landed support, not backlog:
    - `file:has.contributor(<name-or-email regex>)`
    - SG structural mixed `file.contains(path|file:...)`
    - SG structural mixed `file.has.content(path|file:...)`
    - SG structural mixed `symbol.has.name(...)`
  - external proof:
    - `semantica-codegraph-v2` `index-sdk-ingress` live contributor publish + query roundtrip is green
    - `quanta-runtime --lib history_wire_batch_maps_to_file_contributor_batch_v1` broad exact rail is also green
- verification follow-on:
  - packet: [docs/plans/jun-7-verification-hellgates/rfc.md](../plans/jun-7-verification-hellgates/rfc.md)
  - packet status: landed
  - this is not feature backlog
  - it owns fast hellgates, broad daemon lifecycle gates, cross-repo ingress proof routing, and perf compare naming only
  - current gate snapshot:
    - `rust-bench-dsl-truth` green
    - `rust-verify-hellgate-fast` green
    - `rust-verify-hellgate-broad` green
    - `rust-verify-hellgate-cross-repo` red in the current snapshot due external `semantica-codegraph-v2` boundary guard failure
    - `rust-bench-dsl-compare` green
