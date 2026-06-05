# WORKER_START_HERE

If you are a low-context worker or LLM, start here before opening any ticket.

## 1. Goal

Close only the exact post-`jun-4` Sourcegraph tail gaps.

Do not touch landed `jun-4` surfaces unless you are repairing a regression.

## 2. Read Order

1. [NO-GO-RULES.md](NO-GO-RULES.md)
2. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
3. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
4. [rfc.md](rfc.md)
5. one `SGT-*` ticket only

## 3. Preflight Truth

- current checkout does not contain:
  - `scripts/check-persona-target-policy.sh`
  - `scripts/cg-agent-session`
- carry preflight status as `unverified`
- do not call the packet clean-preflight complete unless those scripts actually exist and pass

## 4. First Commands

```bash
rg -n "repo:contains.file|repo:contains.path|repo:has.description|repo:has.meta\\(key\\)|repo:has.meta\\(tag:|file:has.contributor|structural direct lexical|mixed non-repo predicate sibling" docs/analysis docs/plans/jun-5-sourcegraph-tail-gaps
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## 5. Work Rules

- Work one `SGT-*` ticket at a time.
- Freeze current behavior before widening.
- Add red rail first.
- If the owning seam is absent, close the cell as explicit unsupported.
- Do not upgrade alias, regex, or structural surfaces from parser-only evidence.

## 6. Done Rule

A ticket is not done until:

1. current-cell truth is pinned
2. owner seam is explicit
3. runtime/front-door/parity proof exists for supported claims, or typed-fail proof exists for unsupported claims
4. docs and guard reflect the same verdict
5. preflight truth is still reported as `unverified` when the scripts are absent
