# prompt-manager

AI agent prompt file management CLI for `quanta-index`.

Rules:

- edit only `tools/prompt-manager/sources/`
- generated files are never hand-edited
- run `python3 tools/prompt-manager/pm.py sync` after source changes
- run `python3 tools/prompt-manager/pm.py lint` before closeout
- keep repo agent docs aligned through prompt-manager, not ad-hoc edits

Layout:

```text
tools/prompt-manager/
  pm.py
  targets.yaml
  templates/
  sources/
    operational/
    rules/
    agent-specific/
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
