# Copy/paste prompt — P00 Foundation Gate

당신은 quanta-index의 SEP-21 M0 foundation gate owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`와 같은 디렉터리의
`README.md`를 읽고 그대로 적용한다. 현재 Accepted ADR을 재개방하지 말고 current checkpoint를 검증한 뒤
실제 결손만 보완한다. Accepted 값을 바꿔야 하면 superseding ADR 없이는 `BLOCKED`다.

먼저 repo root의 `AGENTS.md`, `AGENT_CORE.md`, `AGENT_PLAYBOOK.md`, `AGENT_RULE_CATALOG.md`를 읽고 따른다.
다음 문서를 전부 읽어라.

- `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/INDEX.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-00-authority-freeze-and-cutover-contract.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md`

시작 시 HEAD, branch/upstream/merge-base, tracked/untracked dirty paths, tracked diff digest를 freeze하라. 기존 dirty
변경을 덮어쓰지 말고 ownership을 분리하라.

목표:

1. S21-00의 모든 blocking decision이 ADR의 명시적 값으로 고정됐는지 검증하고 남은 결손만 닫는다.
2. S21-13 phase A proof authority/schema/validator/CI skeleton을 조기에 구현한다.
3. 후속 lane이 추측 없이 사용할 type/state/error/compatibility 표와 fixture 이름을 제공한다.

필수 결정:

- identifier raw/canonical/Unicode 정책과 canonical byte encoding
- persisted `BatchPublishReceipt` version, canonical CBOR, old/new refusal/import matrix
- SQLite candidate/activation single authority와 filesystem activation pointer 삭제 정책
- terminal `Refused`, retention/replay floor, serial-ingest 또는 multi-writer fence
- state-root manifest/version, offline migration, rollback cutoff
- cursor integrity, `focus_subjects`, active selector resolution semantics
- hard-drain escalation/exit code
- tenant/source/query provider egress policy
- process readiness와 repository readiness 분리
- exact filesystem fanout/path grammar, owner UID/mode/platform refusal policy
- terminal sequence의 exact global/tuple scope

write scope:

- ADR canonical owner
- `tools/ci/inventory/wire-surface.toml`
- `tools/ci/test-authority.toml`
- new proof authority registry/schema/validator and minimum blocking workflow wiring
- generated docs는 `tools/prompt-manager/sources/` owner를 고친 뒤 sync

금지:

- product path에 임시 optional field/dual decoder/zero digest sentinel 추가
- artifact schema만 만들고 CI에서 missing artifact를 허용
- unresolved decision을 TODO로 넘기고 downstream을 시작 가능하다고 표시
- current Accepted ADR을 in-place 의미 변경

DoD:

- 모든 decision에 owner, frozen value, ADR, deadline/compatibility effect가 있다.
- proof manifest가 full 40-char SHA, dirty digest, binary SHA, features/toolchain/host, command/profile/target/filter,
  selected/executed/passed/failed/ignored, timestamps, artifact digests를 필수로 검증한다.
- missing/stale/short-SHA/wrong-binary/zero-execution/ignored-only/unregistered proof가 local validator와 blocking CI에서 실패한다.
- source digest는 staged+unstaged+scoped untracked bytes를 포함하고 proof output은 제외한다.
- `binary_binding=none`과 upstream 없는 isolated worktree가 schema/validator에서 조건부로 표현된다.
- PR gate는 P00 manifest를 `--bind-source`로 검증하고, final release gate는 `--require-all --bind-source`를
  aggregate receipt와 함께 실행한다. manifest 0개 성공은 release proof가 아니다.
- registry 기반 terminal manifest writer가 있고 자유입력 command/profile을 authority로 사용하지 않는다.
- public/persisted format별 producer/consumer/version/decoder/migration fixture가 inventory에 있다.
- S21-01~13이 참조할 frozen names/state/error table을 남긴다.

proof node는 `p00-authority-freeze`다. 검증은 변경 범위에 맞는 repo canonical command로 실행하라. 실패를
숨기거나 기존 unrelated failure를 고치지 마라.

최종 보고 형식:

- status: 공통 실행 계약의 네 상태 중 하나
- frozen source/dirty ownership
- 결정 표와 변경 파일
- proof validator negative 결과
- 후속 lane이 소비할 exact artifact/API
- 실행한 proof와 NOT_RUN proof
- handoff: `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/P00.json`; residual risks와 다음 허용 prompt: P01

explicit owner path만 checkpoint commit에 포함한다. push는 별도 요청이 있을 때만 수행한다.
