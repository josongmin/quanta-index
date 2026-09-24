# P12Q — Final release qualification

The aggregate schema, writer, validator, and final recipe exist in source.
`p12-final-qualification` remains staged until the registered dependency
graph and release authority are ready. A P12A owner test pass alone cannot
issue P12.

Freeze one clean Quanta/Semantica source pair, dependency locks, the attested
release daemon binary, required Linux host, and every config, model, provider,
corpus, and fixture identity. Validate authentic historical P00-P11 product
handoffs, the P02 fork/join, and the distinct P12A infrastructure handoff.
Reissue final-source manifests for every registered dependency; old `passed`
aliases cannot be reused.

With `SEMANTICA_CHECKOUT` and a schema-valid `P12_TERMINAL_INPUT` set,
run `just proof-authority-final-qualification`. The recipe publishes the
aggregate, issues the P12 manifest, then validates all registered manifests
with `--require-all --bind-source`. If the aggregate is not ready, preserve
its diagnostic artifact and do not claim a P12 manifest.

Report `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, and
`ROLLBACK_PROVEN` independently with exact commands, counts, raw evidence,
artifact digests, source pair, binary, host, and exclusions. Deployment,
activation, and rollback require distinct operational receipts; absent or
unapproved actions remain `NOT_RUN`. A tracked source change invalidates
final-source receipts and requires a new qualification run.
