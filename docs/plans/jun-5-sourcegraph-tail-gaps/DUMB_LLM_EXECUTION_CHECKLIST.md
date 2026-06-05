# DUMB_LLM_EXECUTION_CHECKLIST

Before editing:

1. Read [NO-GO-RULES.md](NO-GO-RULES.md)
2. Open [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
3. Pick exactly one `SGT-*` ticket
4. Write down:
   - current code fact
   - official Sourcegraph baseline
   - owner seam
   - first red rail

During work:

1. pin current behavior first
2. do not widen multiple cells at once
3. if a claimed owner seam does not exist, stop and demote to explicit unsupported
4. if a supported claim lacks runtime/front-door/parity proof, it is not supported

Before saying done:

1. ticket doc still matches actual code truth
2. analysis doc still matches actual code truth
3. `sourcegraph_parity.py --check` is green
4. `check-dsl-capability-truth.py` is green
5. `Not Done If` conditions are all false
