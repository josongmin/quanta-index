# RBR-02 — 자연어·literal·native DSL 입력 계약 분리

## 현행 판정 — 2026-09-26, `af640562` + dirty overlay

- 구현: `query_plan.rs`의 native/literal/NL 분기, effective/original identity, Python 재도출 oracle 및 distractor fixture 경로 확인. 벤치 opt-in 정책이지 제품 기본값 변경이 아니다.
- 검증: 과거 dirty local contract 결과는 기록되어 있으나 현 frozen-source 정책/identity 부정 테스트와 live daemon SDK proof는 `NOT_RUN`.
- 잔여: 최종 source에서 query-plan·v4/v5 record/replay tamper fixture, 실제 daemon 문장/식별자 distractor 및 exact inventory/receipt를 함께 재실행한다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P0. 정책·identity 구현 관측; local 계약 통과, clean-source proof 미발급. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00.
- 성격: 벤치 호출 계약 불일치. 기존 native AND는 결함이 아니다.

## 파일·함수

- 신설 `benchmarks/retrieval/src/query_plan.rs`, 등록 `lib.rs`.
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
