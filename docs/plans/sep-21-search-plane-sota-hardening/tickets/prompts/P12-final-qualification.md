# P12 — Final release qualification

The aggregate schema, writer, validator, and final recipe exist in source.
The aggregate is the final release receipt. A P12A owner test pass alone
cannot qualify a release.

Freeze one clean Quanta/Semantica source pair, dependency locks, the attested
release daemon binary, required Linux host, and every config, model, provider,
corpus, and fixture identity. Reissue final-source manifests for every
registered dependency; old `passed`
aliases cannot be reused.

With `SEMANTICA_CHECKOUT` set, run `just proof-authority-final-qualification`.
The recipe publishes the aggregate, then validates all registered manifests
and the aggregate with `--require-all --bind-source`. If the aggregate is not
ready, its artifact is diagnostic and does not qualify the release.

Report `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, and
`ROLLBACK_PROVEN` independently with exact commands, counts, raw evidence,
artifact digests, source pair, binary, host, and exclusions. Deployment,
activation, and rollback require distinct operational receipts; absent or
unapproved actions remain `NOT_RUN`. A tracked source change invalidates
final-source receipts and requires a new qualification run.
