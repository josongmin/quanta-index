# Copy/paste prompt — P02I M1 Integration Gate

당신은 P02A/P02B integration owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`와 같은 디렉터리의
`README.md`를 읽고 그대로 적용한다.

P01이 고정한 동일 base SHA에서 나온 P02A/P02B checkpoint commit과 handoff가 모두 있을 때만 시작한다. 두 lane
commit을 격리 integration branch에 순서대로 통합한다. conflict가 semantic owner 결정을 요구하면 임의 병합하지
말고 `BLOCKED`로 종료한다.

필수 작업:

1. P02A/P02B handoff schema, commit ancestry, exact write set, proof manifest digest를 검증한다.
2. shared contract/public API baseline/wire inventory/generated docs는 이 lane에서만 통합한다.
3. merged clean HEAD에서 `p02a-repomap-compiler`와 `p02b-operation-journal` registered command를 모두 재실행한다.
4. 두 manifest를 current integration HEAD에 `--bind-source`로 검증한다.
5. exported compiler/journal types, refusal codes, sequence scope, mutation coordinator 경계가 Accepted ADR과 일치하는지
   정적으로 교차 검토한다.
6. `artifacts/sep-21/handoffs/P02I.json`을 만들고 exact integration commit 및 두 fresh manifest digest를 downstream에 넘긴다.

금지: branch별 과거 proof 재사용, conflict를 shim/optional field로 봉합, 한 lane failure를 다른 lane success로
상쇄, unrelated dirty 변경 포함.

P03은 `P02I.json`과 양쪽 fresh proof가 동일 integration HEAD에 결속될 때만 시작할 수 있다. explicit owned path만
stage하여 integration checkpoint commit을 만들고 push는 별도 요청이 있을 때만 한다.
