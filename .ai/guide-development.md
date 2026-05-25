# AGENT Guide Development

이 문서는 `.ai/` 진입점이다. SSOT 자체를 대체하지 않는다.

우선순위:
1. `AGENTS.md`
2. `CLAUDE.md`
3. `docs/ssot/**`
4. `.ai/**`

이 레포의 AI/개발 워크플로우는 아래 문서로 진입한다.

- `README.md`: 루트 사용 안내
- `AGENTS.md`: 공용 에이전트 작업 규칙
- `CLAUDE.md`: Rule Catalog SSOT + Claude-specific supplement
- `docs/ssot/README.md`: 문서 SSOT 진입점
- `docs/ssot/operating-kernel-v1.md`: 운영 커널 SSOT
- `docs/ssot/authority-graph-v2.yaml`: authority graph v2
- `docs/ssot/rca-triage-matrix-v1.yaml`: RCA triage matrix v1
- `docs/ssot/lane-governance-v1.yaml`: lane governance v1

기준을 먼저 확인한 뒤, `just` 또는 `scripts/cg-cargo`로 실제 런북을 따른다.

---

## Fast Start

작업 전 최소 체크:

1. 요구사항을 재진술한다.
2. authority-owning layer를 식별한다.
3. 기존 타입/포트/에러를 먼저 찾는다.
4. probe-first로 원인 축을 1개로 좁힌다.
5. 가장 좁은 레이어만 수정한다.
6. 같은 fixture 또는 가장 작은 검증만 먼저 돌린다.

검색 기본:

- 파일 검색: `rg --files`
- 텍스트 검색: `rg -n`
- cargo 계열: `CODEGRAPH_PERSONA=agent ./scripts/cg-cargo ...`

---

## SOTA++ Buildctl Rail

이 저장소의 로컬 Rust build/test 운영은 `buildctl` + `cg-cargo` rail을 기본으로 본다.
이 섹션은 LLM/에이전트용 운영 요약이며, SSOT는 여전히 `AGENTS.md`, `CLAUDE.md`, `scripts/README.md`다.

### 핵심 원칙

- bare `cargo ...`보다 `CODEGRAPH_PERSONA=agent ./scripts/cg-cargo ...`를 우선한다.
- multi-agent local verification의 heavy compile은 `scripts/buildd` + `scripts/buildctl` admission rail이 우선이다.
- `scripts/cg-cargo` heavy compile은 `build_sheriff` + `CODEGRAPH_BUILDCTL_ADMISSION_TOKEN`이 있을 때만 canonical path로 간주한다.
- buildctl denial이 나면 plain `cargo` fallback을 만들지 않는다. fail-closed가 맞다.
- `CARGO_TARGET_DIR`를 ad-hoc으로 계산/생성하지 않는다. target pool 또는 dedicated wrapper를 사용한다.

### target / memory 기본값

현재 SOTA++ 기본 운영값은 아래처럼 보수적으로 맞춰져 있다.

- agent target pool 기본: `pool=4`
- agent cargo jobs 기본: `jobs=1`
- heavy compile 대기 기준: free memory `16GB` 미만이면 대기
- buildctl SOTA watch 메모리 경고 기준: free memory `12GB`
- rust-analyzer RSS 경고 기준: `1GB`

값의 실제 source는 아래를 본다.

- `scripts/source-sota-env.sh`
- `scripts/claim-target-pool.sh`
- `scripts/wait-for-memory.sh`
- `scripts/buildctl-sota-watch.sh`

### IDE / rust-analyzer 운영

IDE는 정확도보다 로컬 안정성을 우선하는 저메모리 기본값으로 맞춘다.

- `checkOnSave=false`
- `allTargets=false`
- `noDefaultFeatures=true`
- `buildScripts=false`
- `procMacro=false`
- `autoreload=false`
- `CARGO_TARGET_DIR`는 workspace 바깥 dedicated target 사용

관련 파일:

- `.vscode/settings.json`
- `scripts/ensure-ide-sota-env.sh`

### buildctl SOTA++ watch 운영

현재 watch/guard/notify는 가능한 한 단일 inline loop 기준으로 경량화되어 있다.

- 기본 권장: `CODEGRAPH_BUILDCTL_SOTA_INLINE_AGENT=1 ./scripts/buildctl-sota-agent.sh`
- 기본 권장: adaptive interval 사용
- 기본 권장: `STATUS_TTL_SEC=2`
- 기본 권장: `RUST_SAMPLE_EVERY=2`
- 기본 권장: `MEM_SAMPLE_EVERY=2`
- watch state 파일은 세션별 runtime dir에 둔다. 전역 `/tmp/buildctl-watch.*` 공유는 안티패턴이다.

즉, steady 상태에서는:

- `buildctl status`를 TTL 캐시로 재사용
- rust/memory 샘플링을 모든 poll마다 하지 않음
- notify/guard는 별도 상시 프로세스보다 inline 처리 우선
- packet / fields / advice는 한 번의 judge pass에서 같이 생성
- `buildctl-sota-agent/watch/guard/notify`는 `source-sota-env.sh`를 자동 반영해 bare shell에서도 SOTA++ 기본값을 상속한다

### 권장 명령

- single-shot check:
  `CODEGRAPH_PERSONA=agent ./scripts/cg-cargo check -p quanta-runtime --lib`
- single-shot test:
  `CODEGRAPH_PERSONA=agent ./scripts/cg-cargo test -p quanta-runtime --no-run`
- pooled session:
  `source scripts/source-sota-env.sh && cg_claim_target_pool`
- buildctl watch:
  `CODEGRAPH_BUILDCTL_SOTA_INLINE_AGENT=1 CODEGRAPH_BUILDCTL_SOTA_ADAPTIVE=1 CODEGRAPH_BUILDCTL_SOTA_STATUS_TTL_SEC=2 CODEGRAPH_BUILDCTL_SOTA_RUST_SAMPLE_EVERY=2 CODEGRAPH_BUILDCTL_SOTA_MEM_SAMPLE_EVERY=2 ./scripts/buildctl-sota-agent.sh`

### 금지 / 안티패턴

- heavy compile denial 후 plain `cargo` fallback
- repo-local `target/` 또는 nested `packages/**/target` 생성
- IDE가 workspace `target/`를 직접 쓰게 두는 것
- 한 task 안에서 `main` target과 pool/dedicated target을 섞는 것
- 상시 `watch + guard + notify`를 각각 별도 루프로 중복 폴링시키는 것
- 메모리 압박 시 cleanup을 위해 full watch를 다시 띄우는 것

### 에이전트용 한 줄 기억법

`cg-cargo`로 들어가고, heavy는 `buildctl` admission을 타고, target은 pool/dedicated만 쓰고, IDE는 저메모리 기본값을 유지하고, watch는 inline/adaptive/TTL/sample 모드로 운영한다.

---

## Claude Rules Digest

`CLAUDE.md` 전체를 따라야 한다. 특히 아래는 매 작업마다 기본값으로 적용한다.

- R-SAFE-01~03: silent failure / silent fallback / open-on-error 금지
- R-SAFE-09: poisoned lock는 bare `unwrap()` 금지, recovery 패턴 사용
- R-SAFE-20: `-> Result` 함수 안 `panic!` / `unreachable!` 금지
- R-SAFE-21: 외부 입력은 저장 전 검증
- R-ERR-01~02: boundary error만 `thiserror`, 내부 오류는 경계에서 매핑
- R-TEST-03~04: fake success / tautology test 금지
- R-ARCH-03~04: adapter 직접 new 하지 말고 port/DI 유지
- R-SENT-01~04: sentinel string / fake digest / fallback key 금지
- R-OBS-01~03: degraded / failure surface는 reason과 evidence를 남긴다

---

## Panic & Explanation Discipline

이 저장소에서 새 코드는 아래 규칙을 기본으로 따른다.

### 금지

- 설명 가능한 실패를 `panic!`으로 바꾸는 bridge
- `unwrap_or_else(|...| panic!(...))`
- `None => panic!(...)`, `_ => panic!(...)`, `Ok(_) => panic!(...)`를 production path에 두는 것
- `#[non_exhaustive]` enum fallback을 panic으로 처리하는 것
- lookup/cache miss를 convenience accessor에서 panic으로 승격하는 것
- 이미 구조화된 에러 타입이 있는데 문자열 panic으로 끊는 것

### 필수

- panic 대신 기존 boundary error로 올리기
- 기존 error가 없으면 `Internal(...)`, `QueryFailed(...)`, `ArchiveIncomplete(...)`처럼 구조화된 타입 추가
- `#[non_exhaustive]` 포트 enum은
  - 명시적 degrade + reason, 또는
  - structured error
  둘 중 하나로 처리
- cache miss / registration miss / precondition miss는 accessor panic 대신 `Result`
- Python/CLI/HTTP 경계에서는 generic fail-closed 문구만 쓰지 말고 원인 문자열을 포함

### 허용 예외

- test helper에서 의도적으로 corruption을 surface하는 panic
- invariant를 깨면 test 자체가 무의미해지는 test-only helper

그 경우에도:

- `#[cfg(test)]` 또는 test module 안에만 둘 것
- 메시지에 invariant 이름을 넣을 것

---

## Structural Prevention

재발 방지는 코드 리뷰보다 정적 guard가 우선이다.

사용 가능한 guard:

- suspicious panic bridge dump:
  - `python3 tools/ci/gates/g10_production_panic_budget.py --dump-suspicious --skip-budget-check`
- suspicious panic bridge fail mode:
  - `python3 tools/ci/gates/g10_production_panic_budget.py --forbid-suspicious --skip-budget-check`

이 guard는 production-adjacent Rust에서 아래 패턴을 잡는다.

- `unwrap_or_else(... panic!(...))`
- `_ => panic!(...)`
- `None => panic!(...)`
- `Ok(_) => panic!(...)`
- `Err(_) => panic!(...)`

새 lint/guard를 추가할 때 원칙:

1. test-only 디렉터리와 `#[test]` 섹션 false positive를 먼저 제거한다.
2. fail mode와 dump mode를 둘 다 제공한다.
3. 기존 budget gate와 독립 실행 가능해야 한다.

---

## Patch Checklist

패치 전:

- 기존 타입/에러/validator가 있는지 검색했는가
- authority 밖 consumer patch인지 확인했는가
- 새 fallback/shim이면 sunset 조건이 있는가

패치 후:

- panic bridge가 Result/error surface로 내려왔는가
- explanation / reason_code / evidence 중 하나 이상이 남는가
- smallest verification을 먼저 했는가
- 관련 문서/가이드가 바뀌었으면 같이 갱신했는가

---

## 티켓 상태 관리 (필수)

티켓 기반 작업 시 반드시 4종 상태로 관리한다. SSOT: `docs/rfcs/mar-18-delta++/tickets/README.md`.

| 상태 | 의미 |
|------|------|
| **대기** | 선행 미충족 또는 아직 당김 대상 아님 |
| **블록** | 선행 티켓 실패/미완료로 진행 불가 |
| **작업중** | 현재 진행 중 |
| **작업완료** | Done, 검증 통과 |

착수/완료 시 인덱스 테이블 `상태` 컬럼과 티켓 본문 헤더를 갱신한다.
