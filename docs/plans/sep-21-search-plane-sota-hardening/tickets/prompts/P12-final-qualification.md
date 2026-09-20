# Copy/paste prompt — P12 Final SOTA Qualification

당신은 S21-13 phase B final qualification owner다. 먼저 repo root 기준
`docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/COMMON-EXECUTION-CONTRACT.md`와 같은 디렉터리의
`README.md`를 읽고 그대로 적용한다. 모든 lane handoff/proof가 current final source pair에 결속되고 legacy
live paths가 제거된 뒤에만 시작한다. 코드를 편의상 고쳐서 proof를 맞추지 말고 발견된 결함은 owning ticket으로
되돌린다.

읽을 문서:

- repo instructions
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/INDEX.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md`
- P00 proof authority와 P01~P11 handoff 전부

목표: 한 final clean source pair와 동일 attested release daemon binary에서 mandatory proof graph를 검증하고
code/deploy/activation/rollback 상태를 분리 판정한다.

preflight:

- quanta-index/Semantica exact clean HEAD, upstream/merge-base, dirty=clean
- frozen `tools/ci/proof-authority.toml` digest, proof ID/dependency DAG, S/U/A/D/P/F/Q/X 전수 재산정
- daemon binary SHA/features/toolchain/state-root format
- pinned Linux production-like host identity
- corpus/config/model/provider/fixture digests
- 모든 profile이 실제 target을 선택하는지 inventory 확인

canonical mandatory families는 proof registry의 8개 code와 모든 registered release proof ID다. 아래는 별도 family
enum이 아니라 coverage dimension이다.

- S: static architecture/public API/wire/module/policy
- U: owner-local positive/negative/recovery
- A: real SQLite/Tantivy/Lance/RepoMap adapter
- D: real SDK/UDS daemon consumer
- P: child-process signal/crash/restart/lease/readiness
- F: corruption/failpoint/concurrency/cancellation
- Q: fixed-corpus relevance, ANN recall, latency/QPS/RSS/FD/thread/disk/WAL
- X: external Semantica/provider/migration/deploy evidence

registered live/J7Q/integration targets는 해당 proof node와 `test-authority` target에 교차 결속돼야 한다.

proof rules:

- each node records full source/binary/host/config identity and selected/executed/passed/failed/ignored counts
- missing/stale/skipped/zero-selected/wrong-binary/dirty mismatch is failure
- independent quality oracle cannot derive expected output from SUT output
- same binary progresses through process/external proof; untracked rebuild는 별도 identity
- macOS proof가 Linux credential/performance proof를 대체하지 않음
- fake provider가 required real-provider proof를 대체하지 않음
- final validator는 `--require-all --bind-source`로 manifest 0/missing/stale를 실패시키고 dependency receipt도 같은
  source pair/daemon binary/required host에 결속한다.
- final qualification command는 `verify-rust` 단독이 아니라 모든 registered proof 실행/수집과 aggregate receipt
  schema validation을 포함해야 한다. canonical aggregate recipe가 없으면 `BLOCKED`다.

canonical escalation은 `Justfile`과 `./scripts/cargow`를 사용하고 current profile registry로 실제 coverage를
확인하라. raw command 성공만으로 product GREEN을 주장하지 마라.

aggregate receipt의 closed state와 validator가 다음 verdict를 각각 계산한다. 사람이 보고문으로 임의 판정하지 않는다.

- `CODE_QUALIFIED`
- `DEPLOYED`
- `ACTIVATED`
- `ROLLBACK_PROVEN`

`PRODUCTION_READY`는 M0~M4 complete, registry digest 고정, legacy live path 0, full manifest valid,
migration/rollback/cross-repo/Linux/승인된 real-provider proof pass, unresolved P0/P1/NOT_RUN/BLOCKED 0일 때만
가능하다. deploy/activate/provider 승인이 없으면 해당 verdict는 NOT_RUN이고 `PRODUCTION_READY=false`다.

proof node는 `p12-final-qualification`이다. 최종 보고에는 exact source pair, registry/plan/aggregate manifest digest,
daemon binary, 모든 proof ID/family result/counts/artifact path, failed/skipped/not-run ledger, 네 verdict와 근거,
unresolved risks와 `artifacts/sep-21/handoffs/P12.json`을 포함하라. proof execution은 code 수정과 분리한다. commit/push,
provider egress, deploy, activate, rollback은 각각 별도 명시 요청 없이는 수행하지 않는다.
