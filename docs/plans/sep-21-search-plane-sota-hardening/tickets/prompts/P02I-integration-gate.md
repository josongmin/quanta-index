# Copy/paste prompt — P02I M1 Integration Gate

당신은 P02A/P02B integration owner다. repo root 기준 prompts의 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를
먼저 읽고 적용한다. P01A의 동일 result SHA에서 나온 P02A/P02B checkpoint commit, handoff, proof manifest가 모두
있을 때만 시작한다. 격리 integration branch에 두 commit을 순서대로 통합한다. semantic conflict를 임의 adapter로
병합하지 말고 `BLOCKED`로 종료한다.

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
7. merged clean HEAD에서 `p02a-repomap-compiler`, `p02b-operation-journal`, public API, wire inventory, static gates를
   실행하고 두 proof manifest를 `--bind-source`로 검증한다.
8. enum `ALL` wire uniqueness, accepted-code cardinality/table digest, compiler/journal API, global sequence event-kind
   digest, mutation coordinator 경계를 amended ADR과 교차 검토한다.
9. `artifacts/sep-21/handoffs/P02I.json`에 exact integration commit, fresh manifest digests, P03 소비 API/fixture를 남긴다.

금지: branch별 과거 proof 재사용, shim/optional field conflict 봉합, 한 lane failure 상쇄, unrelated dirty 포함,
P03 live layout/quarantine/activation cutover 선행 구현.

P03은 P02I handoff와 양쪽 fresh proof가 동일 clean integration HEAD에 결속될 때만 시작한다. explicit owned path만
stage해 integration checkpoint commit을 만들고 current integration branch에 non-force push한다.
