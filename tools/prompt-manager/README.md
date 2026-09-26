# prompt-manager

AI agent prompt file management CLI for `quanta-index`.

Rules:

- edit only `tools/prompt-manager/sources/`
- generated files are never hand-edited
- run `python3 tools/prompt-manager/pm.py sync` after source changes
- run `python3 tools/prompt-manager/pm.py lint` before closeout
- keep repo agent docs aligned through prompt-manager, not ad-hoc edits
- keep the verification contract in `AGENTS.md`; do not copy it into tool-specific
  bootstrap files
- use `max_bytes` in `targets.yaml` to keep bootstrap context bounded
- keep shared instructions in `AGENTS.md`; generate `CLAUDE.md` as a one-line
  `@AGENTS.md` import for older or restricted Claude Code sessions
- keep every generated target allowlisted from `.gitignore` so sync and CI
  validate the same committed prompt set
- keep every source and template reachable from `targets.yaml`; orphan prompt
  fragments are rejected by tests

`sync` renders and stages every selected target before replacing generated
outputs, so a broken source or template does not leave a partial render set.
Rendering also fails before writes when a target exceeds its configured byte
budget. Targets that require first-byte frontmatter can set
`prepend_banner: false`; their template must carry its own generated notice.

Layout:

```text
tools/prompt-manager/
  pm.py
  targets.yaml
  templates/
  sources/
    operational/
    rules/
  tests/
```

## Tool loading contract

Claude Code v2.1.277+ can read `AGENTS.md` directly, but its default selection
changes when a `CLAUDE.md` or `CLAUDE.local.md` exists in the working directory
or an ancestor. The built-in `agents-md` plugin and instruction-file settings
also affect loading. The generated import adapter preserves one shared contract
for older versions and sessions without native AGENTS support; it does not copy
the verification contract. Source: [Claude Code memory documentation](https://code.claude.com/docs/en/memory#agentsmd).

Check `claude --version` and `/context` in the actual session. Generated-file
lint proves rendering and drift only; it does not prove that an installed tool
loaded the instructions or that a model followed them.

Common commands:

```bash
python3 tools/prompt-manager/pm.py sync
python3 tools/prompt-manager/pm.py lint
python3 tools/prompt-manager/pm.py status
python3 tools/prompt-manager/pm.py preview --target agents
python3 -m pytest tools/prompt-manager/tests/test_pm.py -q
```

## Decision regression probes

`evals/cases.json` fixes independent outcomes for scope inflation, compile/test
confusion, stale source, missing producer input, timeout, instructions in logs,
generated-file editing, unrelated dirty work, and a valid focused proof.
`eval.py` supplies inputs without the answer oracle and grades exact decisions.
It rejects partial, duplicate, reordered, malformed, stale, and tampered inputs.

```bash
mkdir -p artifacts/prompt-eval
python3 tools/prompt-manager/eval.py prepare \
  --request artifacts/prompt-eval/request.json --prompt artifacts/prompt-eval/prompt.txt
# Feed prompt.txt to the chosen model with tools disabled. Archive the exact
# invocation, model/config identity, and raw response; no network call is implicit.
python3 tools/prompt-manager/eval.py grade \
  --request artifacts/prompt-eval/request.json \
  --responses artifacts/prompt-eval/responses.json --output artifacts/prompt-eval/grade.json
python3 -m pytest tools/prompt-manager/tests -q
```

Responses must match the JSON contract in the prepared prompt. Raw Claude Code
`--output-format json` results are also accepted; CLI errors are rejected and its
reported model usage is retained. The request binds HEAD, relevant dirty state,
prompt/source digests, corpus, and Python/Jinja2/PyYAML environment. Regenerate it
after any bound input changes.

A passing grade covers single-response decisions only. It does not establish
instruction loading, tool behavior, multi-turn resilience, cross-model quality,
or release qualification. Unit-test golden responses exercise the grader and
must never be reported as model evaluation. Changes to instructions or model
versions require a new real-model run before claiming behavioral improvement.
