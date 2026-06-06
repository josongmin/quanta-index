# J7-04 — Cross Repo Ingress Hellgate

Status: `landed`

Goal:

- keep external producer proof as an explicit separate lane

Owner seam:

- `Justfile`
- external `semantica-codegraph-v2` targeted ingress test

Delivered:

- `just rust-verify-hellgate-cross-repo` wraps the targeted live roundtrip
- it requires `QUANTA_INDEX_SEARCHD_BIN`
- targeted live roundtrip is green on the external semantica rail

DoD:

- packet closeout reports targeted external green separately from local rails
