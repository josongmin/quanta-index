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
- use `AGENTS.md` as the single native entrypoint for Codex, Cursor, and current
  Claude Code; add a tool-specific surface only for a proven tool-specific need
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

Common commands:

```bash
python3 tools/prompt-manager/pm.py sync
python3 tools/prompt-manager/pm.py lint
python3 tools/prompt-manager/pm.py status
python3 tools/prompt-manager/pm.py preview --target agents
python3 -m pytest tools/prompt-manager/tests/test_pm.py -q
```
