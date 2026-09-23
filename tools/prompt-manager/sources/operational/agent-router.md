# AGENTS.md

Shared AI-agent entrypoint for quanta-index.

Load context on demand:

- implementation or verification: relevant sections of [AGENT_CORE.md](AGENT_CORE.md)
- command selection: relevant sections of [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md)
- rule IDs or CI mappings: relevant sections of [AGENT_RULE_CATALOG.md](AGENT_RULE_CATALOG.md)
- authority pointers: [AGENT_REFERENCE.md](AGENT_REFERENCE.md)

Do not preload the full playbook or catalog for routine work.

Canonical tooling:

- Rust build/verification: `Justfile` and `./scripts/cargow` (bare `cargo` only for env-sourced or tool-owned exception rails)
- Prompt/doc control plane: `tools/prompt-manager/pm.py`

Conflict rule:
`AGENTS.md` > `AGENT_CORE.md` > `AGENT_PLAYBOOK.md` > `AGENT_RULE_CATALOG.md` > `AGENT_REFERENCE.md` > chat memory.

Language:

- User-facing responses: Korean
- Code, comments, and commit-message bodies: English preferred
