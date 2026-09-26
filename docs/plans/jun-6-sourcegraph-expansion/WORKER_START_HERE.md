# WORKER_START_HERE

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


If you are a low-context worker or LLM, start here before opening any ticket.

## 1. Goal

This packet is closed. Do not reopen landed cells unless you are repairing a
real regression.

Already landed and not backlog here:

- `repo:has.file(path:... content:...)`
- `repo:has.description(...)`
- `repo:has.meta(key)`
- `repo:has.meta(tag:)`
- `repo:has.meta(/key/)`
- `repo:has.meta(/key/:)`
- `repo:has.meta(key:/value/)`
- `repo:has.meta(/key/:value)`
- `repo:has.meta(/key/:/value/)`
- `file:has.contributor(<name-or-email regex>)`
- SG structural direct lexical `Phrase` sibling remains permanently explicit unsupported
- SG structural direct lexical `Regex` sibling remains permanently explicit unsupported

Current closeout fact:

- packet feature residue: 없음
- external semantica producer proof residue: 없음
- verification follow-on moved to
  [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)

Do not touch landed supported surfaces unless you are repairing a regression.

## 2. Read Order

1. [NO-GO-RULES.md](NO-GO-RULES.md)
2. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
3. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
4. [rfc.md](rfc.md)
5. one `SGX-*` ticket only

## 3. Preflight Truth

- current checkout does not contain:
  - `scripts/check-persona-target-policy.sh`
  - `scripts/cg-agent-session`
- carry preflight status as `unverified`
- do not call the packet clean-preflight complete unless those scripts actually
  exist and pass

## 4. First Commands

```bash
rg -n "file:has.contributor\\(<name-or-email regex>\\)|remaining expansion family|mandatory residue" docs/analysis docs/plans/jun-6-sourcegraph-expansion
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## 5. Work Rules

- Do not start a new `SGX-*` implementation here unless a regression or doc-baseline change actually reopened it.
- There is no active closeout task inside `jun-6`.
- Freeze current explicit unsupported behavior before widening.
- Add the smallest red rail first.
- If the claimed owner seam does not exist, keep the cell explicit unsupported.
- Do not upgrade regex, description, or structural surfaces from parser or
  lowering evidence alone.

## 6. Done Rule

A ticket is not done until:

1. current explicit unsupported truth is pinned
2. owner seam is explicit
3. runtime/front-door/parity proof exists for supported claims, or explicit
   unsupported proof remains if the widening is rejected
4. docs and guard reflect the same verdict
5. preflight truth is still reported as `unverified` when the scripts are absent
