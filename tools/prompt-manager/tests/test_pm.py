from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import yaml

PM = Path(__file__).parent.parent / "pm.py"
REPO_ROOT = Path(__file__).parent.parent.parent.parent
TARGETS = Path(__file__).parent.parent / "targets.yaml"


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
    for name in [
        "agents",
        "agent-core",
        "agent-playbook",
        "agent-reference",
        "agent-rule-catalog",
    ]:
        assert name in result.stdout
    for retired in [
        "codex-rules",
        "codex-start",
        "cursorrules",
        "cursor-core",
        "cursor-supplements",
        "claude",
    ]:
        assert retired not in result.stdout


def test_status_runs():
    result = run("status")
    assert result.returncode == 0
    assert "TARGET" in result.stdout


def test_preview_agents_mentions_core_docs():
    result = run("preview", "--target", "agents")
    assert result.returncode == 0
    assert "AGENT_CORE.md" in result.stdout
    assert "AGENT_PLAYBOOK.md" in result.stdout
    assert "AGENT_REFERENCE.md" in result.stdout
    assert "AGENT_RULE_CATALOG.md" in result.stdout
    assert result.stdout.endswith("\n") and not result.stdout.endswith("\n\n")


def test_preview_agent_core_mentions_prompt_manager():
    result = run("preview", "--target", "agent-core")
    assert result.returncode == 0
    assert "prompt-manager" in result.stdout
    assert "Never infer build or test state" in result.stdout
    assert "## Structured Output" in result.stdout


def test_primary_rule_targets_include_verification_contract():
    for target in ["agents", "agent-rule-catalog"]:
        result = run("preview", "--target", target)
        assert result.returncode == 0, result.stderr
        assert "## Verification Contract" in result.stdout


def test_deprecated_and_inert_prompt_surfaces_are_absent():
    for relative in [
        ".codex/CODEX-RULES.md",
        ".codex/CODEX-START-PROMPT.md",
        ".cursorrules",
        ".cursor/rules/00-core.mdc",
        ".cursor/rules/AI-START-PROMPT.md",
        ".cursor/rules/FAIL-CLOSED-POLICY.md",
        ".cursor/rules/cursor-supplements.mdc",
        "CLAUDE.md",
    ]:
        assert not (REPO_ROOT / relative).exists(), relative


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
