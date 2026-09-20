# Copy/paste prompt — P12 Final SOTA Qualification

당신은 S21-13 phase B final qualification owner다. 제품 구현 lane이 모두 DONE이고 legacy live paths가 제거된 뒤에만
시작한다. 코드를 편의상 고쳐서 proof를 맞추지 말고 발견된 결함은 owning ticket으로 되돌린다.

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
- full proof registry와 required family count 재산정
- daemon binary SHA/features/toolchain/state-root format
- pinned Linux production-like host identity
- corpus/config/model/provider/fixture digests
- 모든 profile이 실제 target을 선택하는지 inventory 확인

mandatory families:

1. static architecture/public API/wire/module/policy
2. owner-local positive/negative/recovery
3. real SQLite/Tantivy/Lance/RepoMap adapter
4. in-process SDK/UDS route matrix
5. real child-process signal/crash/restart/lease/readiness
6. corruption/failpoint/concurrency/cancellation
7. fixed-corpus relevance, ANN recall, latency/QPS/RSS/FD/thread/disk/WAL
8. external Semantica producer and opt-in real-provider
9. migration/backup/restore/rollback
10. registered live/J7Q/integration targets from current proof authority

proof rules:

- each node records full source/binary/host/config identity and selected/executed/passed/failed/ignored counts
- missing/stale/skipped/zero-selected/wrong-binary/dirty mismatch is failure
- independent quality oracle cannot derive expected output from SUT output
- same binary progresses through process/external proof; untracked rebuild는 별도 identity
- macOS proof가 Linux credential/performance proof를 대체하지 않음
- fake provider가 required real-provider proof를 대체하지 않음

canonical escalation은 `Justfile`과 `./scripts/cargow`를 사용하고 current profile registry로 실제 coverage를
확인하라. raw command 성공만으로 product GREEN을 주장하지 마라.

최종 verdict는 각각 별도로 계산한다.

- `CODE_QUALIFIED`
- `DEPLOYED`
- `ACTIVATED`
- `ROLLBACK_PROVEN`

`PRODUCTION_READY`는 M0~M4 complete, legacy live path 0, full manifest valid, migration/rollback/cross-repo/Linux/
real-provider proof pass, unresolved P0/P1/NOT_RUN/BLOCKED 0일 때만 가능하다.

최종 보고에는 exact source pair, plan/proof manifest digest, daemon binary, 모든 family result/counts/artifact path,
failed/skipped/not-run ledger, 네 verdict와 근거, unresolved risks를 포함하라. commit/push/deploy/activate는 별도
명시 요청 없이는 수행하지 않는다.
