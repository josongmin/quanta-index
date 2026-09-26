## Verification Contract

Classify every requested claim as `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN`, or
`NOT_APPLICABLE`.

`FAILED`: executed check failed. `BLOCKED`: required input/evidence absent or
invalid, including stale source. `NOT_RUN`: required action not executed.
Classify the requested scope.

`VERIFIED` requires all of the following:

- exact source revision and dirty state
- correctness-relevant input, dependency, config, binary, and environment identity
- exact command or action plus raw or machine-readable result
- artifact path and digest when applicable
- explicit covered and excluded scope

Revalidate those inputs in the current task. Reports, docs, manifests, `PASS`
flags, booleans, counts, and summaries are not independent evidence. Expected
results must come from an independent oracle: a fixed golden, public contract,
reference implementation, invariant, or externally observable behavior.

Evidence logic must reject relevant false inputs: missing, malformed, stale,
duplicate, partial, reordered, forged, wrong-source, wrong-environment, timed-out,
interrupted, or tampered. Never turn missing or unknown evidence into zero, empty,
default, skipped-pass, or success. A fallback is valid only when activation,
reduced guarantees, observability, and tests are explicit.

Evidence becomes stale when any bound input changes. Compilation is not test
success; focused tests are not repository qualification; local proof is not E2E;
benchmark completion is not benchmark validity; qualification is not merge,
deployment, or activation.

Run the cheapest decisive checks first. Report source state, outcome, command,
evidence path/digest, scope, exclusions, and remaining gaps. Never label partial,
stale, failed, blocked, or unrun evidence as `DONE`, `PASS`, `GREEN`, or `QUALIFIED`.
