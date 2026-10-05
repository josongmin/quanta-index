# AGENT Guide Development

이 문서는 `.ai/` 진입점이다. SSOT 자체를 대체하지 않는다.

우선순위:

1. `AGENTS.md`
2. `CLAUDE.md`
3. `README.md`
4. `docs/adr/**`
5. `docs/plans/**`
6. `.ai/**`

## SSOT 진입점

| 문서 | 역할 |
| --- | --- |
| [`README.md`](../README.md) | 현재 tree truth, build/test entrypoints |
| [`docs/adr/SEP-21-DECISION-REGISTRY.md`](../docs/adr/SEP-21-DECISION-REGISTRY.md) | accepted search-plane decisions |
| [`docs/reference/dsl-proof-inventory.md`](../docs/reference/dsl-proof-inventory.md) | DSL proof inventory; current execution is separate |
| [`SEP-21 decisions`](../docs/adr/SEP-21-DECISION-REGISTRY.md) and [residual ledger](../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) | Current contract and unfinished readiness work; the Sep-16 audit is historical |

Historical only:

- `git show eff53181:docs/ssot/channel-architecture.md`
- `git show eff53181:docs/ssot/producer-handoff.md`
- `git show eff53181:docs/ssot/may-23-storage-architecture-endgame-implementation.md`

## Fast Start

작업 전 최소 체크:

1. 요구사항을 재진술한다.
2. authority-owning layer를 식별한다.
3. 기존 type/port/error를 먼저 찾는다.
4. 가장 좁은 레이어만 수정한다.
5. named profile 또는 crate-local test부터 돌린다.

검색 기본:

- 파일 검색: `rg --files`
- 텍스트 검색: `rg -n`
- cargo 계열: `./scripts/cargow ...`
- repo recipes: `just rust-profile <name>`

권장 entrypoints:

- compile loop: `just rust-profile dev-fast`
- default tests: `just rust-profile test-fast`
- daemon/e2e: `just rust-profile test-daemon`
- merge gate: `just verify`

## Patch Checklist

패치 전:

- authority 밖 consumer patch인지 확인했는가
- README / plan / SSOT claim이 코드와 맞는지 확인했는가

패치 후:

- behavior change면 owner-local test 또는 scenario rail 추가/갱신
- public contract / facade / generated agent doc source면 해당 rail 실행
- docs claim을 바꿨으면 stale plan README도 같이 정리

## 티켓 상태 관리

plan packet 작업은 각 packet의 `tickets/INDEX.md`와 ticket `Status:` line을 SSOT로 본다.
`docs/plans/*/README.md`의 `Status:` field와 ticket board가 어긋나면 packet README를 코드/티켓 기준으로 먼저 맞춘다.

## Claude Rules Digest

`CLAUDE.md` 전체를 따라야 한다. 특히:

- silent failure / silent fallback 금지
- core는 vendor import 금지
- generated docs는 `tools/prompt-manager/sources/`만 수정
