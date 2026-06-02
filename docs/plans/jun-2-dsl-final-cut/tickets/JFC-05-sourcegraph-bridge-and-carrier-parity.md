# JFC-05 Sourcegraph Bridge and Carrier Parity

Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Keep Sourcegraph translation and bridge-packet carriers exactly aligned with native executable semantics, without collapsing carrier boundaries or inventing fallback behavior.

## Current Source Truth

- native parser owns actual LQ execution semantics
- bridge translator lowers only the currently representable Sourcegraph subset
- `into:codeql`, `scope:results`, and `with:lexical` are intentional carrier directives, not runtime search-result rows
- bridge widening for any newly landed native surface must happen after native semantics and proof exist

## 핵심 로직

- native execution is the source of truth
- bridge translation may only expose a subset that is representable on the active LQ wire
- bridge-only carriers remain bridge-packet proof, not runtime-row proof
- bridge carrier proof must stay separated from parser/translator proof; one `golden_bridge` row is not a substitute for bridge-packet evidence
- typed rejections must stay explicit for non-representable Sourcegraph shapes

## 건드릴 파일

- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/src/candidate.rs`
- `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- this packet docs where carrier ownership is described

## 건드리지 말 것

- native query semantics before they exist
- runtime corpus by forcing bridge-only carriers into search-result rows
- lexical fallback for unsupported Sourcegraph shapes
- observability schema beyond what `JFC-06` owns

## TODO

- [x] widen translator coverage after native semantics landed (`JFC-02`–`JFC-04`)
- [x] follow `JFC-01` native predicate truth for alias/parity rows
- [x] keep explicit typed rejections for unrepresentable Sourcegraph shapes
- [x] maintain parity rails for history/runtime/structural widened surfaces (`golden_bridge`, `e2e_dual_syntax_lowering_parity`)
- [x] keep bridge directives explicitly documented as `bridge_packet` carriers

## NOT TODO

- no runtime-row promotion for `into:codeql`, `scope:results`, `with:lexical`
- no silent dropping of directives
- no translator widening ahead of native semantics
- no special-case bridge semantics that native does not own

## Test Plan

- `./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge --test bridge_directive_packet -- --nocapture`
- `./scripts/cargow test -p quanta-index-lq-bridge --test property_translator_total -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## Supported narrow subset (packet truth)

- SG structural route mixed-domain parity is **keyword + structural body only**
  (`patterntype:structural <keyword> AND|OR|AND NOT "<pattern>"` without scoped
  filters under `OR`). Native may execute broader mixed shapes (`Phrase` /
  `RawString` / `Predicate` siblings); SG lowering does not mirror those until
  explicitly widened.
- **Repo-scoped filters under mixed OR** (e.g. `repo:… patterntype:structural A
  OR "…"`) remain `BRIDGE_TRANSLATE_FAIL` by design; parity uses unscoped SG
  queries or native-only runtime rows for mixed OR execution proof.
- History/runtime filter SG/native parity: `golden_bridge` rows 9–20 (lowering)
  plus `e2e_dual_syntax_lowering_parity` `AUTHORITY_SCENARIOS` (execution).

## DoD

- bridge subset exactly matches native executable subset for widened surfaces
- bridge-only carriers remain explicitly classified and proved on their own rails
  (`bridge_directive_packet` preserves `translated.directives`; SG syntax does not
  surface those directives)
- unsupported Sourcegraph shapes typed-fail with stable diagnostics
- docs do not blur translator coverage with runtime-row coverage

## Failure Modes

- bridge accepts syntax that native cannot execute
- bridge-only carrier packets are miscounted as runtime search-result proof
- parser/translator golden tests are mistaken for bridge-packet carrier proof
- translator silently drops directives or filters
- parity rail hides native/SG divergence behind set-like result comparison
