# Agent Core Protocol

Read this file for implementation or verification work.

## Execution Defaults

- Inspect current source, revision, and dirty state before relying on plans or reports.
- Preserve unrelated dirty work; establish owned paths before editing.
- Use the narrowest decisive probe first; escalate verification by affected surface.
- Never infer build or test state. Run the exact command and report its scope.
- Treat required missing input as `blocked`, not as a value to guess or synthesize.
- Propagate errors with context. Do not replace them with defaults or quiet partial success.
- Prefer an explicit typed failure or `NotImplemented` over an unproved fallback.
- Do not promote a focused green rail to repository-wide qualification.
- Examples: owner tests passed, full suite unrun -> `NOT_RUN` for full suite; compilation passed, tests unrun -> `NOT_RUN` for tests.

## Design Defaults

- Breaking-first: prefer one canonical contract over open-ended compatibility shims.
- Keep one current internal IR per semantic boundary. Change its producers and consumers together; do not add versioned IR twins, upcasters, or legacy readers for this undeployed service.
- Distinguish internal IR from persisted/wire artifacts: retain exact identity and rejection checks where needed, but never use an artifact version stamp to justify parallel IRs.
- Authority must be typed or explicit; heuristics cannot decide correctness.
- Keep core/port types independent from vendor and adapter types.
- Fix the producing authority instead of adding consumer-side repair layers.

## Tooling Authority

- build/check/test front door: `just`, `./scripts/cargow`
- bare `cargo` is not the default verification surface; use it only after `scripts/quanta-index-env.sh` is sourced or when an external nightly tool requires it
- prompt/doc front door: `python3 tools/prompt-manager/pm.py`
- generated docs drift gate: `python3 tools/prompt-manager/pm.py lint`
- edit generated agent docs through `tools/prompt-manager/sources/`, then run `pm.py sync`

## Structured Output

- When requested, emit JSON that satisfies `tools/ci/agent/agent_output.schema.json`.
- `ok` requires no missing input, correctness-affecting assumption, required non-verified claim, or error.
- `blocked` and `error` outputs must not expose a deployable artifact.

## Required Closeout

Every verification closeout must report:

- command used
- covered surface
- excluded surface or remaining seam
- final status
- failure class if failed
