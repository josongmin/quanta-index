# RBR-08 — 근거 기반 exact-name 심볼 ranking

> Archive status: `Historical work packet`. Accepted decisions are owned by [SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md). Current unfinished work is tracked only in [GAP-REGISTER.md](GAP-REGISTER.md). This file is retained for implementation and evidence history.


## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현/검증: 독립 원문·정의 span·symbol ID를 선고정한 설치 daemon probe를 실행했다. native-safe 8개는 7 success/1 abstained, literal 10개는 10 abstained였다. **FAILED — literal symbol candidate admission**: 지원되지 않는 실행을 exact-empty로 응답하는 공백이 재현되었으며 ranker 변경으로 해결할 수 없다. 이것은 고정 설치 binary의 국소 관측이고 현재 dirty source 전체 qualification은 아니다.
- 현 코드 수정: 기존 `LEX_PLANNER_UNSUPPORTED_FILTER_COMBO`로 symbol Phrase/RawString/Regex·Regexp keyword·contentfilter 및 symbol.has.name 미지원 scalar를 중앙 query validator와 executor의 resolved domain에서 거부한다. paged/all·type/select·Boolean/predicate 및 predicate force-empty보다 먼저 적용하며 keyword postings·chunk-owned repo/file predicates는 유지한다. 거짓 exact exhaustion 경로의 최소 수정이며 지원 확장이 아니다. lib121·smoke37·192거부 반례·all-target Clippy/fmt/diff exit0 owning local을 회수했다. installed SDK18/18은 실제 daemon의 native symbol positive와 literal typed failure를 같은 기존 identity에서 확인했다. raw `/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/sdk-terminal.log`, SHA `f85b39cabd082a0fc2c9cf3832c2607f82b3bb2e3475ba84d10fd376f3a31545`; binary 전후 동일, source drift local이라 clean-source qualification은 아님.
- 잔여 결정: 실제 symbol phrase/raw/regex 지원은 아래 별도 authority/format/lifecycle 후속 `NOT_RUN`이다. lower-case exact-name 후보의 case-fold 동점 아래 rank2는 **정책 실험 후보**이며 현재 계약 위반으로 판정하지 않는다. exact-name ranker 효과·rank pagination/reopen·holdout guard는 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P2. bounded 진입 probe `VERIFIED`; ranking 변경/유지 최종 결정 `NOT_RUN`. literal admission은 [RBR-02](RBR-02-query-policy.md)와 먼저 분리한다. 정책을 임의 승격하지 않는다. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-05/06.
- 진입 조건: source-bound symbol route에서 정답 후보가 존재하지만 부분일치/참조/동명이인보다 낮게 배치되는 재현 사례.

## 파일·함수

- [lexical/adapter.rs](../../../../crates/quanta-index-lexical/src/adapter.rs): `UpsertSymbol`와 `ReplaceLexicalScope` 두 생산 경로.
- [schema.rs](../../../../crates/quanta-index-lexical/src/schema.rs), `lib.rs::SchemaFields`, [documents.rs](../../../../crates/quanta-index-lexical/src/documents.rs) `add_symbol_fields`.
- [searcher/port.rs](../../../../crates/quanta-index-lexical/src/searcher/port.rs): `search_symbols_constrained`; compile/restrictions.
- [searcher/paging.rs](../../../../crates/quanta-index-lexical/src/searcher/paging.rs): `collect_ranked_page`, cursor/order 계약.
- owning lexical fixtures, `tests/tantivy_smoke.rs`.

## 설치 binary bounded probe — 2026-09-26

- Raw evidence: `/private/tmp/qi-rbr08-symbol-probe.movsOd/`; 재현 명령 `COMMANDS.md`, 독립 정답 `oracle.json`, `rank-analysis.json`, `rank-analysis-native-safe.json`, runner record/diagnostics/metrics와 원문 `corpus/probe.rs`를 보존한다. fixture git HEAD `33aa4a832488c4ba9823207bb8ca3add7f086936`; 원문 SHA-256 `5a6d075bfa006a041fcce7164793949a348294a7888f835eeec699cece1c87ee`; oracle SHA-256 `80eafdd912d02eaa8bafb0f41596c67ff71e5555129540882b429dcdb28c0767`.
- 설치 binary는 root rebuild와 격리해 외부 복사본으로 고정했다. runner SHA-256 `1dfe841f04e8fb94d056fbc58df2ac5d85bd1ebf79dfc305f5d894df677f4e34`; searchd SHA-256 `22e25a847d1ea5df19d953134f382d5ce5e99a90df1cdf9e14f79305679f6080`. 이 binary의 빌드 source를 현재 HEAD로 추정하지 않는다. 현재 read-only owning source identity와 공유 dirty 목록은 별도 보존한다.
- 종료 후 원래 설치 경로의 runner/searchd SHA가 root rebuild로 모두 변경됐지만 probe 복사본 SHA는 유지됐다(`binary-identities-final.txt`). 따라서 이 결과는 **historical pinned binary**의 재현이며 새 설치 binary의 수정 검증으로 재사용하지 않는다.
- 실행 범위: Rust 파일 1개, 추출 symbol 14개, `symbol` 단독, `whole_file`, `hash-dev`, `k=10`, 활성 generation 1, fresh state, warmup 0/measurement 1, attested development hand-oracle. qualified/homonym 원문 2개는 native DSL `INVALID_FILTER_VALUE`로 전체 pack이 query-plan 단계 exit 2였고, native-safe 실행에서는 명시적으로 제외했다. 실패를 성공 또는 no-answer로 변환하지 않았다.
- 실제 terminal: literal exit 0 / 10개 모두 abstained·exact-exhausted candidate total 0; native-safe exit 0 / 8개 중 7 success·1 abstained. exact/local/container/Unicode/upper-case/definition-reference 정답 ID는 rank1, lower-case `sendpacket` 정답 ID는 rank2다. `SendPacket` rank1과 score가 모두 `2.393340587615967`로 같아 case-fold 동점 정책 실험 사례이지 기존 순위 계약 결함 증명은 아니다. partial `packet`은 native에서도 exact-exhausted 0으로 substring recall을 보장하지 않는다.
- literal과 native-safe의 같은 8개 원문 대조에서 native 정답 후보 7개가 literal에서는 모두 사라졌다. source-bound ID·span이 일치하므로 missing corpus/extraction과 순위 변경을 구분한다. 외부 diagnostic의 일반 scope는 `returned_window_only`; 이번 0개 응답은 별도 `exact_count total=0` 증거가 있다. 일반적인 미반환 후보의 전체 admission을 추정하지 않는다.
- RCA: [query_plan.rs](../../../../benchmarks/retrieval/src/query_plan.rs) `plan_query` literal은 Phrase를 만들고, [sdk.rs](../../../../benchmarks/retrieval/src/sdk.rs) `query_route`는 이를 symbol native request로 보낸다. [compile.rs](../../../../crates/quanta-index-lexical/src/searcher/compile.rs) Phrase는 [match_sets.rs](../../../../crates/quanta-index-lexical/src/searcher/match_sets.rs) `phrase_match_set`의 `PhraseField::Content` authority로 내려간다. [adapter.rs](../../../../crates/quanta-index-lexical/src/adapter.rs) `apply_op`의 symbol 양 ingest 경로에는 `text_authority_doc_id` 할당이 없고, [text_authority_plan.rs](../../../../crates/quanta-index-lexical/src/text_authority_plan.rs) `summarize_text_ops`도 symbol을 제외한다. [restrictions.rs](../../../../crates/quanta-index-lexical/src/searcher/restrictions.rs) `authority_restriction_query`는 그 ID로 제한하므로 symbol 문서를 admit하지 못한다. native Keyword는 symbol name/container snippet의 inverted field를 검색하는 다른 경로다.
- 다음 owning 구현: root가 별도 owner에 기존 unsupported 계약을 보존하는 typed refusal을 배정했다. 새 symbol authority/schema migration은 이번 최소 수정 범위가 아니다. `tantivy_smoke.rs`에 같은 symbol의 Keyword/Phrase 대조를 UpsertSymbol·ReplaceLexicalScope 양 경로로 고정하고 nested expression·page/reopen의 fail-closed 계약을 검증한다. symbol canonical phrase/raw/regex 실제 지원과 case/Unicode/qualified/container 효과는 별도 `NOT_RUN` 후속이다. text chunk authority·schema/generation·제품 기본 ranking·BM25·fusion을 유지한다.
- 제외: full corpus/qualified-name 의미 보장·반복 determinism·pagination/reopen·성능·quality holdout·clean-source closure. literal total 184266ms와 native total 231467ms는 debug binary SHA/startup을 포함한다; query 합계 31ms/41ms도 단일 noisy sample이므로 성능 비교에 쓰지 않는다. `runner-sample.txt`는 SHA startup CPU를 별도 확인했으며 search latency가 아니다.

## Typed refusal owning 검증 — 최신 수정

- 생산4파일: `symbol.rs`의 borrowed/exhaustive query-level helper, `planner.rs`의 explicit symbol type/select·predicate validation, `searcher/compile.rs`의 resolved domain validation before predicate force-empty, `searcher/query_rewrite.rs`의 동일 keyword-only argument 정책. index/schema/wire/API/default 변경 없음.
- 실제 독립 golden: 양 ingest 경로의 symbol ID `sym` keyword/content-keyword 양성 및24미지원 질의×paged/all/type/select×2producer =192 typed refusals; nested Boolean·contentfilter·빈 repo gate가 refusal을 숨기지 않는 반례 포함. 기존 chunk-owned predicate/domain 검증을 유지한다.
- `./scripts/cargow --lane test-integration-lane test -p quanta-index-lexical --locked --lib`:121passed/0failed/ignored/filtered,111.58s. raw SHA `ed568dbb3af151a52c95fc81476b5e9c284ad232b62afacf2d7b658c2d629cbe`.
- 같은 lane의 `--test tantivy_smoke -- --nocapture`:37passed/0failed/ignored/filtered,10.31s. raw SHA `3448f9841ac9ef2695ab563ce05c6bc24f64b9d323237a0353168c3ff3aca179`.
- `clippy -p quanta-index-lexical --all-targets --locked -- -D warnings`:exit0,44.69s, raw SHA `503a5eb597779b196248bf7ff9345cca20ec18e27151165c62c0bebcbf777dbc`; fmt/diff exit0. 중간 wildcard/indexing 오류는 suppression 없이 exhaustive match/first()로 수정하고 실패 원본을 보존했다.
- 정확 환경·소스·명령·binary digest는 `/private/tmp/qi-rbr10-observation.sW0yqY/SYMBOL-REFUSAL.md` 및 `symbol-refusal-artifacts.sha256`에 보존한다. shared source의 연속 compile custody는 입증하지 않았으므로 `VERIFIED` owning local 범위를 installed/clean-source/전체 repository/성능/지원 확장으로 승격하지 않는다.

## 유한 후보와 작업

### 별도 지원 확장 — canonical symbol text authority (`NOT_RUN`)

현재 typed refusal 수정은 거짓 no-answer를 막는 정확성 보완이며 phrase/raw/regex 지원 완료가 아니다. 실제 지원을 요구할 때만 다음을 하나의 format/lifecycle 변경 단위로 수행한다. keyword를 phrase의 숨은 fallback으로 사용하지 않는다.

- `adapter.rs`: `UpsertSymbol`·`ReplaceLexicalScope.symbols`가 동일한 canonical name/container text와 authority ID를 생성한다.
- `text_authority_plan.rs`·`text_docs.rs`: symbol allocation/retirement/rebuild/path deletion/clear 및 개별 upsert의 이전 authority ID retirement를 포함한다. 기존 chunk-owned repo/file content 의미는 유지한다.
- `searcher/{prepare,compile,match_sets,predicates,query_rewrite}.rs`·`phrase.rs`: symbol/text domain을 명시하고 직접·nested Boolean·predicate·contentfilter를 동일 domain의 normalizer/positions/trigram authority로 실행한다.
- `text_authority/manifest.rs`·`sealed_generation/manifest.rs`: authority coverage format 계약을 변경하고 legacy generation은 typed rebuild-required로 거부한다. cursor/continuation은 새 artifact identity에 바인딩하며 옛 token 재사용을 거부한다.
- 검증: 두 ingest 경로 × keyword/phrase/raw/regex·Unicode/case·domain leakage·replace/tombstone/clear/upsert·reopen·delta inheritance/crash recovery·pagination 및 legacy refusal. 같은 external literal fixture를 재실행한다. 구현 전 해당 capability를 shipped/VERIFIED로 표기하지 않는다.

### 조건부 ranking 후보

1. baseline과 exact local-name/qualified-name 우선 정책 한 가지를 비교한다. definition intent가 명시된 symbol route에 한정한다. generic lexical이나 모든 NL 입력에 무조건 적용하지 않는다.
2. exact-name 필드가 필요하면 canonical field를 schema에 추가하고 **두 symbol ingest 경로 모두** 같은 값으로 채운다. normalization/case/qualified separator 규칙을 명시한다.
3. 기존 on-disk index의 schema mismatch/rebuild 처리와 generation/cursor 호환성 거부를 함께 구현한다. 옛 인덱스를 새 필드가 있는 것처럼 읽지 않는다.
4. ranking은 pagination 전에 적용한다. 페이지 수집 후 재정렬이나 서로 다른 route의 score를 임의 비교하지 않는다.
5. development에서 Hit@1/MRR 효과를 측정해 한 후보를 선택하고 실험 profile에만 둔다. 기본 정책 승격은 RBR-12의 단일 최종 holdout 판정 이후다. fusion weight나 범용 BM25 parameter까지 동시에 조정하지 않는다.

## 테스트·합격

- exact vs token-partial, local vs container-only, 동일 이름 다른 namespace, qualified name, Unicode/case, definitions vs references.
- stable ties, repeated runs, 여러 페이지 연결 시 중복/누락 없음, wrong-generation/config cursor 거부.
- insert/upsert/replace/reopen이 동일 정렬을 제공하며 legacy schema는 typed refusal/rebuild.
- 독립 fixtures 전부 통과. 최종 holdout에서는 사전 선언한 rank 효과와 qualified NDCG/recall guard를 [TEST-PLAN](TEST-PLAN.md) 기준으로 함께 판정해야 기본 정책을 바꿀 수 있다.
- 진입 조건이 없으면 trace/실험 근거와 함께 유지 결정으로 종료한다. 실행하지 않은 ranker를 구현 완료로 쓰지 않는다.
