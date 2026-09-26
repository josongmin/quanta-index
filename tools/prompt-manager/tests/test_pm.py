from __future__ import annotations

import argparse
import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

PM = Path(__file__).parent.parent / "pm.py"
REPO_ROOT = Path(__file__).parent.parent.parent.parent
TARGETS = Path(__file__).parent.parent / "targets.yaml"

SPEC = importlib.util.spec_from_file_location("prompt_manager_under_test", PM)
assert SPEC and SPEC.loader
PROMPT_MANAGER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROMPT_MANAGER
SPEC.loader.exec_module(PROMPT_MANAGER)


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(PM), *args],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )


def test_list_returns_expected_targets():
    result = run("list")
    assert result.returncode == 0
    listed = {line.split()[0] for line in result.stdout.splitlines() if "→" in line}
    for name in [
        "agents",
        "claude-entrypoint",
        "agent-core",
        "agent-playbook",
        "agent-rule-catalog",
    ]:
        assert name in listed
    for retired in [
        "codex-rules",
        "codex-start",
        "cursorrules",
        "cursor-core",
        "cursor-supplements",
        "claude",
    ]:
        assert retired not in listed


def test_status_runs():
    result = run("status")
    assert result.returncode == 0
    assert "TARGET" in result.stdout


def test_preview_agents_mentions_core_docs():
    result = run("preview", "--target", "agents")
    assert result.returncode == 0
    assert "AGENT_CORE.md" in result.stdout
    assert "AGENT_PLAYBOOK.md" in result.stdout
    assert "AGENT_RULE_CATALOG.md" in result.stdout
    assert result.stdout.endswith("\n") and not result.stdout.endswith("\n\n")


def test_preview_agent_core_mentions_prompt_manager():
    result = run("preview", "--target", "agent-core")
    assert result.returncode == 0
    assert "prompt-manager" in result.stdout
    assert "Never infer build or test state" in result.stdout
    assert "## Structured Output" in result.stdout


def test_primary_rule_targets_include_verification_contract():
    agents = run("preview", "--target", "agents")
    assert agents.returncode == 0, agents.stderr
    assert "## Verification Contract" in agents.stdout

    catalog = run("preview", "--target", "agent-rule-catalog")
    assert catalog.returncode == 0, catalog.stderr
    assert "## Verification Contract" not in catalog.stdout


def test_deprecated_and_inert_prompt_surfaces_are_absent():
    for relative in [
        ".codex/CODEX-RULES.md",
        ".codex/CODEX-START-PROMPT.md",
        ".cursorrules",
        ".cursor/rules/00-core.mdc",
        ".cursor/rules/AI-START-PROMPT.md",
        ".cursor/rules/FAIL-CLOSED-POLICY.md",
        ".cursor/rules/cursor-supplements.mdc",
        "AGENT_REFERENCE.md",
    ]:
        assert not (REPO_ROOT / relative).exists(), relative


def test_claude_entrypoint_imports_shared_contract_without_duplication():
    result = run("preview", "--target", "claude-entrypoint")
    assert result.returncode == 0, result.stderr
    body = result.stdout.split("-->\n", 1)[1]
    assert body == "@AGENTS.md\n"
    assert (REPO_ROOT / "AGENTS.md").is_file()
    assert "## Verification Contract" not in result.stdout


def test_configured_render_budgets_are_enforced():
    targets = yaml.safe_load(TARGETS.read_text())["targets"]
    for name, config in targets.items():
        if "max_bytes" not in config:
            continue
        result = run("preview", "--target", name)
        assert result.returncode == 0, result.stderr
        assert len(result.stdout.encode("utf-8")) <= config["max_bytes"]


def test_generated_outputs_are_not_gitignored():
    targets = yaml.safe_load(TARGETS.read_text())["targets"]
    for name, config in targets.items():
        result = subprocess.run(
            ["git", "check-ignore", "--no-index", "-q", config["output"]],
            cwd=str(REPO_ROOT),
        )
        assert result.returncode == 1, f"generated target is gitignored: {name}"
        tracked = subprocess.run(
            ["git", "ls-files", "--error-unmatch", config["output"]],
            cwd=str(REPO_ROOT),
            capture_output=True,
        )
        assert tracked.returncode == 0, f"generated target is not tracked: {name}"


def test_all_sources_and_templates_are_reachable_from_targets():
    targets = yaml.safe_load(TARGETS.read_text())["targets"]
    prompt_manager = TARGETS.parent

    referenced_templates = {prompt_manager / config["template"] for config in targets.values()}
    actual_templates = set((prompt_manager / "templates").glob("*"))
    assert actual_templates == referenced_templates

    referenced_sources = {
        prompt_manager / section
        for config in targets.values()
        for section in config.get("sections", [])
    }
    actual_sources = set((prompt_manager / "sources").rglob("*.md"))
    assert actual_sources == referenced_sources


def test_full_rule_catalog_includes_golden_rules():
    result = run("preview", "--target", "agent-rule-catalog")
    assert result.returncode == 0, result.stderr
    assert "## Golden Rules" in result.stdout


def test_render_budget_failure_does_not_partially_sync(tmp_path, monkeypatch):
    template = tmp_path / "template.j2"
    template.write_text("{{ target.name }}\n", encoding="utf-8")
    first_output = tmp_path / "first.md"
    second_output = tmp_path / "second.md"
    first_output.write_text("old first\n", encoding="utf-8")
    second_output.write_text("old second\n", encoding="utf-8")

    first = PROMPT_MANAGER.Target(
        name="first",
        output=first_output,
        template=template,
        description="",
        max_bytes=1024,
    )
    second = PROMPT_MANAGER.Target(
        name="second",
        output=second_output,
        template=template,
        description="",
        max_bytes=1,
    )
    monkeypatch.setattr(PROMPT_MANAGER, "load_targets", lambda _names=None: [first, second])

    result = PROMPT_MANAGER.cmd_sync(argparse.Namespace(target=None, dry_run=False))

    assert result == 1
    assert first_output.read_text(encoding="utf-8") == "old first\n"
    assert second_output.read_text(encoding="utf-8") == "old second\n"


def test_render_reports_missing_template_as_regular_error(tmp_path):
    target = PROMPT_MANAGER.Target(
        name="missing",
        output=tmp_path / "output.md",
        template=tmp_path / "missing.j2",
        description="",
        max_bytes=1024,
    )

    with pytest.raises(PROMPT_MANAGER.PromptManagerError, match="Template not found"):
        PROMPT_MANAGER.render(target)


def test_preview_agent_playbook_mentions_verify_commands():
    result = run("preview", "--target", "agent-playbook")
    assert result.returncode == 0
    assert "./scripts/cargow" in result.stdout
    assert "pm.py lint" in result.stdout


def test_dry_run_then_lint_without_mutating_generated_files():
    sync = run("sync", "--dry-run")
    assert sync.returncode == 0, sync.stderr
    lint = run("lint")
    assert lint.returncode == 0, lint.stdout + lint.stderr


def test_unknown_target_fails():
    result = run("preview", "--target", "does-not-exist")
    assert result.returncode != 0
    assert "Unknown target" in result.stderr
