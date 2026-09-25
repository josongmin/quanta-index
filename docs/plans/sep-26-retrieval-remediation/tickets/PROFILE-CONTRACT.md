# RBR Canonical Profile Contract — Query/Producer/Comparator/Diagnostic provenance

- 작성 근거: [RBR-00](RBR-00-proof-contract.md) 작업 3. 상태: **계약 정의 only, 구현 `NOT_RUN`**.
- 이 문서는 RBR-01/02/03/04가 공통으로 사용하는 profile 계약의 단일 canonical 정의다. 구현 티켓은 이 정의를 변경 없이 구현하며, 변경이 필요하면 이 문서를 먼저 고치고 관련 receipt를 무효화한다.
- 이 디렉터리는 retrieval source closure에 포함되므로(2026-09-26 등록), 이 계약의 사후 변경은 기존 proof/receipt를 무효화한다.

## 1. 원칙

1. **숨겨진 기본값 금지.** 실행에 영향을 주는 모든 설정은 frozen profile에 명시된 값으로만 존재한다. validator는 missing/unknown/typo 필드를 거부하며 기본값으로 채우지 않는다.
2. **original/effective 분리.** 호출자가 전달한 원문과 각 lane에서 실제 실행된 변환의 identity를 별개 필드로 바인딩한다. 변환 결과만 남기고 원문을 버리지 않는다.
3. **계획은 하나의 경로에서.** query plan·profile dispatch는 단일 함수에서 생성되며 cold/warmup/measured가 동일 경로를 공유한다. 측정 단계별 다른 policy는 위조다.
4. **레거시 twin 금지.** 기존 internal IR에 병렬 스키마를 만들지 않는다. wire/artifact 확장은 producer·consumer·negative fixture·replay를 한 단위로 수정한다.
5. **실행 관측과 기여 분리.** "실행된 lane/engine"과 "결과에 기여한 lane/engine"는 독립 필드다. free-text trace는 raw 보존이며 임의 파싱 결과를 authority로 승격하지 않는다.

## 2. Query input policy (구현: RBR-02)

| 필드 | 값 계약 | authority | hash owner | frozen copy | validator | consumer |
| --- | --- | --- | --- | --- | --- | --- |
| `query_input_policy` | `native` \| `literal` \| `natural_language`. unknown 거부 | 이 계약 + `query_plan.rs` | runner (policy enum을 profile SHA 입력에 포함) | pair-spec/run-manifest의 query 섹션 | run.py spec 검증: unknown/missing policy 거부 | main.rs cold/warmup/measured, record.rs |
| `native` | 원문을 DSL 그대로 전달. operator/precedence 불변 | lq-norm parser 공개 계약 | — | 동일 | AST round-trip 회귀 | sdk.rs `query_route` |
| `literal` | 전체 문자열의 안전한 literal화. escaping 규칙은 검증된 DSL emitter 재사용 | `query_plan.rs` | effective lexical request SHA | 동일 | quoting/backslash/colon/operator/Unicode/qualified-name fixture | lexical lane |
| `natural_language` | 원문은 semantic text 그대로, lexical은 **결정적 토큰 OR 계획**. 토큰화·한도·중복 제거·escaping은 profile에 고정 | `query_plan.rs` + 이 계약 | policy config SHA + effective request SHA | 동일 | 빈 토큰/초과 길이는 typed refusal, match-all fallback 금지; gold/evaluator 파일 접근 부정 테스트 | lexical lane + semantic lane |
| `explicit_constraints` | NL 원문 내 경로/이름의 자동 exact-filter 승격 금지. caller의 canonical constraints만 허용 | 이 계약 | 없음(전달 금지가 계약) | spec constraints 섹션 | 제약 없는 원문에서 exact filter 생성 시 거부 | query_plan.rs |

## 3. Original/effective query identity (구현: RBR-02)

모든 query record는 다음 4개 SHA를 개별 필드로 가지며, replay validator는 전부 재계산해 대조한다.

| 필드 | 정의 | hash owner | validator |
| --- | --- | --- | --- |
| `original_query_sha256` | 호출자 원문 bytes | runner | run.py record/replay |
| `query_policy_config_sha256` | policy enum + 토큰화/한도/escaping 설정의 canonical JSON | `query_plan.rs` → runner가 기록 | 동일 |
| `effective_lexical_request_sha256` | 실제 lexical lane에 전달된 직렬화 요청 | `query_plan.rs` | 동일 |
| `semantic_text_sha256` | semantic lane에 전달된 text bytes (원문과 다를 수 있음) | runner | 동일 |

- 두 제품(Quanta/Semble)에는 **동일 원문**을 전달하고 각 변환 identity는 별도 기록한다.
- planning 비용의 latency 포함 여부는 `planning_cost_in_latency: bool`로 protocol에 고정한다. 결과를 본 뒤 바꾸지 않는다.

## 4. Producer/parser profile (구현: RBR-04/06)

| 필드 | 계약 | authority | frozen copy | validator |
| --- | --- | --- | --- | --- |
| `chunker_strategy` | `fixed_window_strict` \| `fixed_window_line_aligned` \| `syntax_rust_items` \| `whole_file` + `window_bytes`/overlap 등 파라미터 전체 | chunking/core.rs | batch manifest | chunking_contract.rs + run.py |
| `symbol_producer_identity` | producer 모듈 SHA 경로가 아니라 **grammar/파서 버전 + lockfile digest + 언어별 지원 범위 manifest** | symbols.rs (pinned grammar, lockfile 고정) | capability/coverage manifest | 5개 언어 수작업 span fixture; unsupported/parse-error는 coverage failure |
| `publish_unit_kinds` | `chunk` \| `symbol` (typed registry, ID 충돌 거부) | published_units.rs | batch manifest digest | RBR-05 forged/stale 반례 |
| `semantic_input_profile` | 현행 `raw-code` 유지. 구조형 card는 별도 profile value로만 | 이 계약 | run manifest | 심볼 게시와 동시 embedding text 변경 금지 |

## 5. Comparator settings/actual execution (구현: RBR-03)

| 필드 | 계약 | authority | validator |
| --- | --- | --- | --- |
| `semble_profile` | `native-default` \| `hybrid-no-rerank` \| `lexical-only` \| `semantic-only` | 이 계약 + semble.py | unknown mode 거부, phase별 mode 차이 거부 |
| `actual_alpha` / `actual_rerank` | 요청 값이 아니라 **실행된 값**. alpha 0/1에서도 dual-lane 실행은 점수 ablation으로 표기 | worker가 관측해 기록 | 위조 시 rejection |
| `lane_call_evidence` | pure-lane profile에서 반대 lane 호출 0임을 spy/sentinel으로 증명 | adapter 테스트 | lexical-only encode/dense 호출 0, semantic-only BM25 호출 0 |
| `reference_binding` | pinned upstream 함수/소스 hash + library version 묶음. 사용 불가 시 explicit unsupported | semble.py + 외부 venv hash | version/API mismatch 거부 |

## 6. Diagnostic schema (구현: RBR-01)

| 필드 그룹 | 계약 | validator |
| --- | --- | --- |
| 보존 응답 정보 | request/generation id, explanation, window completeness, early stop, **executed engines** (contributed와 별개) | executed-but-empty fixture; 미관측 값은 missing으로 남김 |
| dense lane identity | exact/approximate, internal fetch, admitted/examined/fused counts | SDK 응답↔sidecar 대조 |
| stage timings | query 단계별 elapsed/call counts, publish 내부 embedding/delete/append/seal/activate. 서버 경계에서 수집, 식별자로 연결, diagnostic on/off overhead 측정 | runner wall time 분배 금지 |
| bounded stage trace | opt-in only. request/generation/config 바인딩 + 상한 + truncation 표시 필수 | 잘린 trace 거부/명시 |

## 7. Frozen copy와 provenance 연쇄

각 profile 인스턴스는 다음 순서로만 유효하다:

```text
raw producer output (repo 외부)
  → frozen manifest (spec/protocol/profile SHA 전체 포함, run 시작 전 고정)
  → 실행 record/sidecar (original/effective identity 전부 기록)
  → replay validator (전 필드 재계산·대조, mismatch/missing/stale 거부)
```

- validator는 schema 위반이 아니라 **값 위조**(실제 실행과 기록 불일치)도 거부해야 한다.
- profile/schema/config/policy 변경은 옛 증거 재사용을 무효화한다(TEST-PLAN §5).
- admission 없이 profile을 qualified evidence로 승격하지 않는다.

## 8. 이 계약의 검증 시점

- RBR-02 구현 시: §2/§3 전 필드의 실제 schema 파일 + 부정 테스트가 동일 변경에 포함된다.
- RBR-03 구현 시: §5의 profile dispatch가 cold/warmup/measured 공유임을 증명한다.
- RBR-01 구현 시: §6 필드가 SDK 응답에서 보존됨을 daemon roundtrip으로 증명한다.
- RBR-04 구현 시: §4 producer identity가 batch manifest digest에 바인딩됨을 증명한다.
