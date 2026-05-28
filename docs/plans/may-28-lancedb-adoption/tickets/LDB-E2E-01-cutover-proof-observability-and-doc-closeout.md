# LDB-E2E-01 — Cutover Proof, Observability, and Doc Closeout

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [LDB-04-legacy-semantic-journal-migration.md](LDB-04-legacy-semantic-journal-migration.md)

## 1. Purpose

Prove that the Lance cutover is real, fail-closed, and reflected in the repo's
operator-facing docs.

## 2. Owner files

- `crates/quanta-index-searchd-runtime/tests/...`
- `crates/quanta-index-semantic/tests/...`
- benchmark/perf harnesses as needed
- prompt-manager source docs that feed generated `README.md` / `CLAUDE.md`

## 3. Proof obligations

- cold boot proof: semantic generations open from durable state without full
  replay
- restart proof: restarted daemon serves the same semantic results for a sealed
  generation
- not-ready proof: incomplete or manifest-mismatched persisted semantic state
  fails closed
- migration proof: legacy journal import yields the same semantic results as a
  clean durable build
- doc proof: generated operator docs stop claiming a shipped backend that the
  code does not implement

## 4. Observability requirements

- boot path must expose whether semantic generations were opened directly or a
  migration step ran
- bounded metrics/log labels only; no raw vectors, snippets, or unbounded path
  text in metric labels
- emit cold-boot/open timings so the replay-removal claim is measurable

## 5. Test and bench plan

- e2e restart test over real persisted semantic state
- e2e negative rows for missing marker, manifest mismatch, and incomplete
  generation
- migration equivalence test comparing legacy-imported results vs fresh durable
  build
- benchmark comparing old replay boot cost vs persisted-open boot cost on a
  representative corpus

## 6. DoD

- the cutover has executable proof, not only source inspection
- observability can distinguish durable open, migration, and typed failure
- generated docs no longer contradict the live semantic backend

## 7. Failure modes

- claiming replay removal without measuring the boot path
- updating generated docs before proof exists
- letting observability leak unbounded semantic payloads
