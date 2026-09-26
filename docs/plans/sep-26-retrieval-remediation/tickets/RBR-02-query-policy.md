# RBR-02 — 자연어·literal·native DSL 입력 계약 분리

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md)의 공유 dirty source

- 구현 관측: `query_plan.rs`의 native/literal/NL 분기·original/effective identity, Python 독립 재도출 및 distractor/tamper fixture가 현재 owning 소스에 있다. 벤치 opt-in 정책이며 제품 native AND 기본값은 유지한다. 동일 정책 재구현은 잔여 작업이 아니다.
- 검증 경계: current authority는 Python319/Rust108/SDK18. Rust108/SDK18 exact terminal은 local 통과했고 SDK symbol positive/literal typed refusal도 확인했다. full Python288의285pass/3source-drift failures와 fresh3pass, 새process31 focused는 서로 다른 입력의 local 결과이며 full319와 합성하지 않는다. JSON huge-int 7경로는 공통 변환 전 scalar guard로 typed refusal/False 보완·집중8+2통과. 최종 current-source whole/clean receipt는 NOT_RUN.
- symbol capability 수정: literal emitter의 Phrase는 그대로 보존한다. symbol에는 chunk-only content authority가 없어 미지원 Phrase/RawString/Regex·regexp keyword·contentfilter 및 해당 symbol.has.name scalar를 중앙 domain-aware typed refusal로 거부한다. 거짓 exact-empty를 반환하거나 literal을 native로 조용히 치환하지 않는다. keyword postings와 chunk-owned repo/file predicate는 유지한다. 실제 symbol literal 지원은 RBR-08의 별도 canonical authority·format/lifecycle 후속이다.
- 잔여: 최종 source/config에서 query-plan·현행 record/replay identity/tamper fixture, 실제 daemon 문장/식별자 distractor를 owning rail로 재실행하고 exact inventory·terminal·receipt를 연결한다. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md).

- 우선순위: P0. 정책·identity 구현 관측; 최종 source-bound proof 미발급. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00.
- 성격: 벤치 호출 계약 불일치. 기존 native AND는 결함이 아니다.

## 파일·함수

- 기존 [query_plan.rs](../../../../benchmarks/retrieval/src/query_plan.rs), `lib.rs` 등록.
- [sdk.rs](../../../../benchmarks/retrieval/src/sdk.rs): `RouteQuery`, `query_route`.
- [main.rs](../../../../benchmarks/retrieval/src/main.rs): cold/warmup/measured request 구성.
- [record.rs](../../../../benchmarks/retrieval/src/record.rs), [run.py](../../../../tools/benchmark/retrieval/run.py): query/config identity, frozen protocol/replay.
- 참고만: lq-norm `parse_and_expression`, lexical `compile_expr`의 `All -> Must`.

## 고정 설계

1. 호출자가 명시하는 세 정책을 둔다: `native`는 그대로 DSL, `literal`은 전체 문자열을 안전하게 literal화, `natural_language`는 원문 semantic text와 별도의 lexical retrieval plan을 생성한다. 알 수 없는 정책은 거부한다.
2. 첫 NL baseline은 **질의만 사용하는 결정적 토큰 OR 계획**으로 제한한다. 정해진 토큰화·한도·중복 제거·escaping을 profile로 고정한다. gold에서 식별자를 얻거나 `Find the function...` 템플릿만 제거하지 않는다. 복잡한 learned/LLM query rewriting은 후속 근거 없이는 추가하지 않는다.
3. NL 원문 속 경로/이름을 권한 있는 exact filter로 승격하지 않는다. 명시적 제약은 caller의 별도 canonical constraints에서만 전달한다. 토큰이 없거나 한도를 넘으면 typed refusal이며 match-all fallback은 없다.
4. 기존 AST 또는 검증된 DSL literal emitter를 재사용한다. 토큰 문자열을 operator와 무검증 연결하지 않는다. native 모드의 operator/precedence는 그대로 유지한다.
5. original query SHA, policy/config SHA, effective lexical request SHA, semantic text SHA를 기록한다. 양쪽 제품에 동일한 원문을 전달하되 각 실행 변환은 구분한다. planning 비용의 latency 포함 여부도 protocol로 고정한다.
6. 계획은 하나의 경로에서 생성하여 cold/warmup/measured가 공유한다. 벤치의 opt-in adapter 정책이며 제품 기본값을 몰래 바꾸지 않는다. 제품 채택은 holdout 평가 뒤 별도 명시한다.

## 테스트·합격

- native `a b`의 AND 의미 보존; OR 계획은 named NL profile에서만 활성화.
- literal의 따옴표/backslash/콜론/operator/Unicode/qualified identifier를 독립 expected query로 검증.
- sentence와 bare identifier의 같은 정의를 찾는 fixture에 comment/reference distractor 추가. 검색 성공뿐 아니라 실행된 요청도 검증한다.
- 빈 토큰/초과 길이/다중 이름/없는 이름/명시적 constraints/대소문자 테스트.
- gold-bearing pack 거부 및 evaluator 파일 비접근. policy/request tampering replay 거부.
- 새 development 결과를 native 입력 결과와 구분하여 기록한다. NL OR가 품질을 보장한다고 주장하지 않는다.

공통 [TEST-PLAN](TEST-PLAN.md)의 contract + SDK rail로 검증한다.
