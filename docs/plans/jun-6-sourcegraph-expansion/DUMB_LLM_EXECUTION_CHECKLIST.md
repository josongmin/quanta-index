# DUMB_LLM_EXECUTION_CHECKLIST

Before editing:

1. Read [NO-GO-RULES.md](NO-GO-RULES.md)
2. Open [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
3. Pick exactly one `SGX-*` ticket
4. Record preflight truth:
   - if `scripts/check-persona-target-policy.sh` or `scripts/cg-agent-session`
     are absent, status is `unverified`
5. Write down:
   - current explicit unsupported behavior
   - official Sourcegraph baseline
   - owner seam
   - first red rail
6. Confirm the cell is still backlog on the live tree.
   - not backlog anymore:
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
    - SG structural mixed `file.contains(path|file:...)`
    - SG structural mixed `file.has.content(path|file:...)`
    - SG structural mixed `symbol.has.name(...)`
    - SG structural direct lexical `Phrase` sibling
    - SG structural direct lexical `Regex` sibling
   - packet closeout residue:
     - 없음

During work:

1. pin current behavior first
2. do not widen multiple cells at once
3. if the claimed owner seam does not exist, stop and keep the verdict explicit
   unsupported
4. if a supported claim lacks runtime/front-door/parity proof, it is not
   supported
5. if a regex or structural widening still relies on current exact-string or
   lowering-only logic, it is not done

Before saying done:

1. ticket doc still matches actual code truth
2. analysis doc still matches actual code truth
3. `sourcegraph_parity.py --check` is green
4. `check-dsl-capability-truth.py` is green
5. `Not Done If` conditions are all false
6. preflight residue is still explicit
