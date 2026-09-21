# Copy/paste prompt — P02I M1 Integration Gate

당신은 P02A/P02B integration owner다. repo root 기준 prompts의 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를
먼저 읽고 적용한다. P01A의 동일 result SHA에서 나온 P02A/P02B checkpoint commit, handoff, proof manifest가 모두
있을 때만 시작한다. integration branch의 base는 exact P01A result SHA다. P02A checkpoint를 먼저, P02B checkpoint를
그 다음 순서로 통합한다. 원 checkpoint ancestry를 보존하는 merge를 사용한다. cherry-pick이 불가피하면
original→applied SHA mapping을 handoff에 기록한다. semantic conflict를 임의 adapter로
병합하지 말고 `BLOCKED`로 종료한다.

이 lane의 write allowlist는 P02A/P02B에 사전 배정된 두 contract section, handoff가 열거한 contract/SDK
re-export/facade symbol, `tools/ci/lint/baselines/public-api/{quanta-index-contract,quanta-index-sdk}.txt`,
`tools/ci/inventory/wire-surface.toml`, 두 lane이 넘긴 공통 proof/test-authority/profile delta뿐이다. lane 구현 로직을
재설계하거나 allowlist 밖 충돌을 해결하지 말고 `BLOCKED`다.
허용된 CI delta는 두 proof row, `repomap-compiler-owner-v1`/`operation-journal-owner-v1` integration target,
`local_scopes.p02a-repomap-compiler`/`local_scopes.p02b-operation-journal`, 두 exact Justfile recipe section뿐이다.

## 필수 작업

1. 두 handoff schema, commit ancestry/base SHA, exact write set, proof manifest digest와 P01A error-table digest를 검증한다.
2. P01A가 live persistence/activation/quarantine를 수정하지 않았고 P02A가 durable mutation을 하지 않았는지 확인한다.
3. P02B가 closed event kind만 받고 allocator/event/domain row를 한 transaction에 묶으며 rollback/restore-max proof를
   가졌는지 확인한다.
4. P02A output이 P01A canonical identity/commitment/resource receipt를 완전하게 보존하고 raw bundle 재해석을
   downstream에 허용하지 않는지 확인한다.
5. P02A/B가 추가한 모든 refusal이 `SearchPlaneErrorCodeV2` variant인지 확인한다. production source의
   `CoreError::Typed code:String`, `SearchPlaneIpcError code:String`, dynamic error-code `format!`, `&str` pass-through,
   substring/equality classification, unknown decoder success는 0이어야 한다.
6. P02A의 `contract/src/repomap.rs` compiler DTO와 P02B의 `contract/src/ipc/ingest.rs` journal DTO가 배정 밖 symbol을
   건드리지 않았는지 확인한다. SDK facade/public re-export/public API baseline/wire inventory/generated docs delta는
   이 lane에서 한 번만 통합한다.
7. 두 checkpoint를 통합하고 shared delta를 적용한 뒤 explicit allowlist만 stage해 integration checkpoint commit을
   먼저 만든다.
8. clean committed integration HEAD에서 `p02a-repomap-compiler`, `p02b-operation-journal`, public API, wire inventory,
   static gates를 재실행하고 두 proof manifest를 새 HEAD의 immutable archive path에 발급해 `--bind-source`로 검증한다.
   P02A/P02B original handoff가 참조하는 fork-HEAD archives를 overwrite하지 않는다.
9. enum `ALL` wire uniqueness, accepted-code cardinality/table digest, compiler/journal API, global sequence event-kind
   digest, mutation coordinator 경계를 amended ADR과 교차 검토한다.
10. `artifacts/sep-21/handoffs/P02I.json`에 exact integration commit, fresh integration-HEAD archive paths/digests,
    original→applied SHA mapping,
    P03 소비 API/fixture를 남기고 schema를 검증한 뒤 non-force push/remote SHA 확인을 수행한다.

금지: branch별 과거 proof 재사용, shim/optional field conflict 봉합, 한 lane failure 상쇄, unrelated dirty 포함,
P03 live layout/quarantine/activation cutover 선행 구현.

P03은 P02I handoff와 양쪽 fresh proof가 동일 clean integration HEAD에 결속될 때만 시작한다.
