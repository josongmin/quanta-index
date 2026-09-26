# RBR-04 — 소스 기반 심볼 생성과 파일 단위 combined publication

> Archive status: `Historical work packet`. Accepted decisions are owned by [SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md). Current unfinished work is tracked only in [GAP-REGISTER.md](GAP-REGISTER.md). This file is retained for implementation and evidence history.


## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

현재 owning 소스의 `extract_corpus_symbols`는 unsupported admitted path/SHA와 parse failure를 typed abort로 처리하며 source path/hash/text를 재검증한다. 현 profile은 partial capability를 선언하지 않은 strict full이다. phase metrics의 파일별 path/SHA/grammar/count coverage를 verdict가 frozen corpus와 비교하고 nonzero unsupported count를 거부한다. strict-full coverage를 다시 구현할 작업은 없다. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md)가 통합 상태 authority다.

- 구현: `symbols.rs`의 다언어 definition producer와 파일당 combined `replace_scope`가 있으며, unsupported admitted 파일은 path/SHA를 포함한 typed failure로 abort한다. 현 profile은 strict full이고 partial 지원은 선언하지 않는다.
- 검증 경계: 과거 Rust lib82/chunking25와 live SDK16/17은 당시 dirty local diagnostic이다. 현재 authority는 Python319/Rust108/SDK18이며 실제 terminal은 중앙 감사에 기록한다. observation-off/floor 단일 성공은 5언어 combined publication 증명이 아니다. 최종 frozen-source 5언어 수작업 span·replace/reopen·source/grammar mutation의 exact terminal/receipt는 별도이고 clean-source proof는 `NOT_RUN`이다.
- 잔여 코드: 현 strict-full 성공/실패 coverage는 phase metrics와 corpus replay에 연결했다. 별도 partial profile은 없으며 무음 skip은 허용하지 않는다.
- 잔여 proof: 5언어 fixture와 실 daemon 게시·교체·재개방을 같은 revision/binary에서 다시 증명하고 parser/grammar/coverage manifest를 묶는다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. producer·combined publication 코드 관측; 현 소스 storage/SDK proof 미발급. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00.
- 성격: 기존 chunk-only 벤치의 기능 확장. 제품에 심볼 계약이 없다는 주장이 아니다.

## 파일·함수

- 기존 [symbols.rs](../../../../benchmarks/retrieval/src/symbols.rs), `lib.rs`, `Cargo.toml`, workspace `Cargo.lock`의 parser/grammar identity.
- [batch.rs](../../../../benchmarks/retrieval/src/batch.rs): `assemble_batch`, `scope_digest`, `BatchAssemblyReport`.
- [SymbolRecord](../../../../crates/quanta-index-contract/src/lex/symbol.rs), SDK [replace_scope](../../../../crates/quanta-index-sdk/src/lexical.rs), lexical [ReplaceLexicalScope](../../../../crates/quanta-index-lexical/src/adapter.rs) 재사용.

## 작업

1. 벤치 producer에서 Rust/Go/Python/JavaScript/TypeScript의 definition inventory를 생성한다. 기존 Rust tree-sitter와 호환되는 pinned grammar를 사용하고 실제 lockfile로 고정한다. language별 함수/메서드/타입/중첩·anonymous 지원 범위를 manifest에 선언한다.
2. `SymbolRecord`를 canonical 출력으로 사용한다. repo/path/source SHA, parser/grammar/config, byte/line span, kind/local/qualified/container names, deterministic ID를 바인딩한다. 타입을 새로 복제하지 않는다.
3. admitted files 전체를 질의·gold와 무관하게 추출한다. unsupported grammar/parse error/누락 노드는 명시적인 coverage failure다. label regex나 질의에서 이름을 읽어 심볼을 만들지 않는다. anonymous에는 허위 공개 이름을 붙이지 않는다.
4. 각 파일의 chunks와 symbols를 **한 번의** `replace_scope`에 실어 보낸다. 같은 path로 두 replacement를 보내지 않는다. empty chunk와 symbol-only 파일의 처리도 명시한다.
5. `scope_digest`와 batch manifest digest에 symbol payload·producer identity를 포함한다. symbol 이름/span만 바뀌어도 digest가 변해야 한다. symbol/chunk ID 충돌을 타입/registry에서 거부한다.
6. 이 단계에서 semantic 입력은 현재 raw-code profile을 유지한다. 구조형 semantic card를 추가하는 것은 별도 실험이며, 심볼 게시와 동시에 임베딩 텍스트를 바꾸지 않는다.

## 테스트·합격

- 5개 언어 각각 수작업 고정 definition span fixture, methods/nested definitions/attributes/decorators/Unicode/CRLF/동명이인/overload cases.
- source SHA나 grammar identity 변경, duplicate ID, inverted/out-of-bounds span, stale output, unknown language의 typed rejection.
- 순서가 바뀌어도 동일 canonical inventory/digest; symbol-only 변경은 다른 digest.
- combined replacement 후 chunks와 symbols 모두 존재하고 다음 replacement/delete에서 stale rows가 사라진다.
- 실제 SDK publish/reopen에서 동일 identity가 유지된다. 5개 언어 모두 통과하지 않으면 다언어 완료로 표기하지 않는다.

## 완료 산출물

producer capability/coverage manifest, source-bound symbols, batch counts/digests, owning contract/storage/SDK 테스트. 공통 [TEST-PLAN](TEST-PLAN.md) 적용. 별도 검색엔진·parser framework나 레포 내부 corpus 복사는 제외한다.
