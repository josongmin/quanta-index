# Copy/paste prompt — P12A Final Proof Infrastructure

당신은 S21-13 phase B infrastructure owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`, 같은 디렉터리의
`README.md`, `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/README.md`를 읽고 그대로 적용한다. immediate P11 source-pair checkpoint/handoff가
start HEAD와 exact match하고 `p11-cross-repo-cutover` exact-pair proof가 pass할 때만 시작한다. deployment/activation/
rollback proof는 승인 부재 시 `NOT_RUN`이어도 infrastructure 구현을 진행할 수 있지만 P12Q closure에서는 mandatory다.
이 lane은 aggregate producer를 구현하는 lane이며 final qualification을
실행하거나 `PRODUCTION_READY`를 판정하지 않는다.

읽을 문서:

- repo instructions
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/{FINAL-AUDIT,INDEX,ACTION-LIST}.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md`
- P11 handoff와 current `tools/ci/proof-authority.toml`

## 목표

`p12-final-qualification`이 proof dependency만 검사하는 얇은 wrapper가 되지 않게 한다. aggregate schema/writer/
validator/final recipe가 P00부터 P11까지의 handoff chain, P02A/P02B fork와 P02I join, exact source pair, attested release
binary, mandatory proof DAG와 네 verdict를 실제 입력으로 검증하고 하나의 atomic aggregate artifact를 발급하게 한다.

## owner scope

- `tools/ci/proof-aggregate.schema.json`
- `tools/ci/write-proof-aggregate.py`
- aggregate validator/checker와 해당 owner tests
- `tools/ci/write-proof-manifest.py`, `tools/ci/lint/check-proof-authority.py` 중 P12 issuance/aggregate binding section
- `tools/ci/proof-authority.toml`의 신규 `p12a-proof-infrastructure` row와 `p12-final-qualification` row만
- `tools/ci/test-authority.toml`의 `[[integration_targets]] id="p12a-proof-infrastructure-v1"`와
  `[local_scopes.p12a-proof-infrastructure]`
- `Justfile`의 final aggregate/qualification recipe section만
- `Justfile`의 dedicated `rust-proof-p12a-proof-infrastructure` recipe section
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json`
- P12 terminal-input schema/template와 S21-13 closeout docs

edit 전에 common contract의 exact path/symbol/base-blob owner-freeze table을 만든다. product crates, P01~P11 owner logic,
기존 proof 결과 artifact는 수정하지 않는다.

## 구현 요구

1. aggregate `product_handoffs`는 exact list `P00,P01,P02A,P02B,P02I,P03,P04,P05,P06,P07,P08,P09,P10,P11`을 요구한다.
   P12/P12A output을 자기 사전 입력으로 넣지 않는다.
   aggregate schema/writer는 별도 mandatory `infrastructure_handoff` field도 구현한다. P12A 자기 owner-proof invocation에는
   이 field를 넣지 않고, P12Q final invocation에서 P12A handoff 하나를 exact 입력으로 요구한다.
2. 각 handoff의 schema, base/result ancestry, exact write set, exported contract digest, proof manifest digest를 검증한다.
   handoff가 기록한 historical archive receipt는 domain-versioned canonical `{source,source_pair}`의
   `source-binding-digest`, manifest canonical bytes의 `manifest-digest`, transitive `dependency_receipts` exact archive
   path/digest에 결속해 검증한다. current alias와 비교하지 않으며 alias 갱신 뒤에도 historical DAG는 유효해야 한다.
   final-source rerun receipt는 별도 final ledger에서 검증한다.
3. P02A/P02B가 동일 P01 base에서 갈라지고 P02I가 두 result를 포함한 뒤 P03 base가 P02I result와 일치하는지 검증한다.
4. 순차 lane P03→P11의 immediate predecessor result/base가 정확히 연결되는지 검증한다.
5. P11 `paired_repositories`와 final quanta/Semantica clean HEAD, dependency-root digest, branch/upstream, approval/push
   state가 일치하는지 검증한다.
   top-level base/result/dirty digest는 quanta entry와 exact match하고 모든 `PUSHED` entry의
   `remote_sha == result_sha`여야 한다.
6. registry의 mandatory proof IDs, family, dependency DAG, source/binary/host/config identity, selected/executed/passed/
   failed/ignored count와 artifact digest를 다시 검증한다.
7. aggregate closed state가 `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`을 각각 계산한다. 누락,
   `NOT_RUN`, stale, wrong binary/source/host, zero-selected, ignored-only는 성공으로 계산하지 않는다.
8. verdict가 `NOT_READY`이면 diagnostic aggregate는 남길 수 있지만 `p12-final-qualification` manifest는 발급하지 않는다.
9. writer는 atomic publish하고 validator는 aggregate artifact와 이후 P12 manifest의 digest/source-pair/binary binding을
   재검증한다.
10. canonical final recipe는 `SEMANTICA_CHECKOUT`과 `P12_TERMINAL_INPUT`을 필수 입력으로 fail-closed 검사하며 로컬
    절대경로 default나 stale terminal input을 사용하지 않는다.

## DoD

- missing/reordered/duplicated handoff, broken serial edge, wrong P02 fork/join, stale manifest, wrong source pair/binary/host,
  unapproved external verdict가 모두 negative fixture에서 실패
- aggregate schema에 handoff ledger와 paired repository identity가 mandatory
- aggregate schema는 historical handoff receipt ledger와 final-source rerun receipt ledger를 분리하고 같은 proof ID의
  서로 다른 source binding/retry manifest archives를 허용한다. archive overwrite/alias-only handoff, primary source가
  같지만 source pair가 다른 receipt의 key 충돌, transitive dependency archive 변조는 거부한다.
- historical handoff status/`not_run`은 각 checkpoint 시점의 사실로 보존한다. final four-verdict와 unresolved
  `NOT_RUN` 계산은 final-current receipt ledger만 사용한다. historical `RELEASE_PROOF_PENDING`을 final failure로
  재해석하거나 handoff를 사후 갱신하지 않는다.
- writer/validator/recipe가 동일 canonical handoff order와 registry digest를 사용
- dependency validation만 성공하고 aggregate publication/P12 issuance가 빠지는 path 0
- product source 변경 0; final qualification 실행 0; deploy/activate/rollback/provider egress 0
- `p12a-proof-infrastructure`는 `family=S`, `required_host=any`, `dependencies=[p11-cross-repo-cutover]`로 schema/writer/
  validator/recipe negative fixtures를 실행하고 source-bound manifest를 발급
- lane handoff semantic validator가 `selected == executed == passed`, proof ID uniqueness,
  `not_run[]`와 `proofs[status=NOT_RUN]` exact consistency를 강제

checkpoint commit 후 clean result HEAD에서 infrastructure owner checks를 재실행한다. 이 lane은
`artifacts/sep-21/handoffs/P12A.json`을 만들며 `lane=P12A`, `ticket=S21-13`, `paired_repositories`를 기록한다.
`p12-final-qualification`은 `NOT_RUN`으로 기록하고 이유를 `qualification belongs to P12Q`로 둔다. handoff status는
`OWNER_PROOF_GREEN`이며 P12Q만 terminal manifest와 verdict를 발급한다. explicit owner path만 commit하고 non-force
push한 뒤 remote SHA를 검증한다.
