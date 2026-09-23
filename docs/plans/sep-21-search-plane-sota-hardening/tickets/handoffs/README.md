# SEP-21 Lane Handoffs

Lane 실행 결과는 `lane-handoff.schema.json`에 맞춰 `artifacts/sep-21/handoffs/<LANE>.json`으로 남긴다. 결과
artifact는 result commit 뒤 생성하고 source dirty digest에서 제외한다. tracked source에 result commit SHA를
자기참조로 기록하지 않는다. source-bound proof가 없는 placeholder handoff는 downstream gate로 사용할 수 없다.

구현 lane 순서는 `checkpoint commit → clean result HEAD proof/manifest → handoff schema validation → non-force push →
remote SHA verification`이다. proof 뒤 source가 바뀌면 manifest와 handoff를 폐기하고 새 result HEAD에서 재발급한다.

P02A/P02B 통합 owner는 두 lane commit을 합친 clean HEAD에서 proof를 다시 실행하고 `P02I.json`을 만든다.
P02I는 `integration_commits`에 P02A/P02B original/applied SHA와 MERGE/CHERRY_PICK mode를 기록한다.
P11과 P12A/P12Q handoff는 top-level quanta SHA 외에 `paired_repositories`로 exact source pair를 기록한다.
paired list 순서는 `github:josongmin/quanta-index`, `github:josongmin/semantica-codegraph-v2`로 고정한다. schema가
표현할 수 없는 cross-field 조건은 handoff semantic validator가 강제한다: top-level base/result/dirty digest는 첫 quanta
entry와 exact match하고, `PUSHED` entry의 `remote_sha == result_sha`여야 한다. validator는 lane별 canonical
ticket/proof/status tuple, filename, `git diff --no-renames base..result` exact write set, clean current source와
exact-pair manifest의 paired source/dependency lock도 검증한다. `exported_contracts`는
`<repo-relative-path>@sha256:<result-commit-blob-digest>` 형식이며 validator가 result commit의 blob을 다시 hash한다.

handoff proof는 `source-binding-digest = SHA256(canonical
{"domain":"quanta-proof-source-binding-v1","source":source,"source_pair":source_pair-or-null})`와 manifest canonical
bytes의 SHA-256을 사용한
`artifacts/proof-authority/archive/<proof-id>/<source-binding-digest>/<manifest-digest>.json` immutable receipt만 참조한다.
manifest의 `dependency_receipts`도 dependency의 exact archive path/digest를 기록한다. 동일 proof ID의 current alias는
historical authority가 아니며 alias equality를 요구하거나 rerun으로 과거 archive/dependency edge를 overwrite하면
handoff가 invalid다. manifest가 가리키는 terminal evidence와 release daemon도 각각
`artifacts/proof-authority/{evidence,binaries}/<content-sha256>` immutable object여야 한다.

Canonical semantic validation command:

```bash
just lane-handoff-check artifacts/sep-21/handoffs/<LANE>.json
```

위 `just` command는 checkpoint closeout용 strict current-source gate다. 과거 ledger를 현재 checkout과 재결속하지 않고
archive/ancestry만 검사할 때만 `just lane-handoff-check-historical <path>`를 사용한다. JSON Schema 단독 성공은 handoff
GREEN이 아니다.

전체 제품 이력의 누락·중복·순서·P02 fork/join·P03→P11 인접 SHA 결속은
`just lane-handoff-chain-check`로 검사한다. 이 명령은 각 handoff의 historical Git/archive 검증을 먼저 실행하며,
과거 result를 현재 HEAD라고 주장하지 않는다. P12 aggregate가 이 체인을 직접 소비하기 전에는 이 명령의 성공만으로
P12A/P12 완료를 선언하지 않는다.
