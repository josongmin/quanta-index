from __future__ import annotations

import subprocess
import sys
from pathlib import Path

PM = Path(__file__).parent.parent / "pm.py"
REPO_ROOT = Path(__file__).parent.parent.parent.parent


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
        "codex-rules",
        "codex-start",
        "cursorrules",
        "cursor-start",
        "cursor-fail-closed",
        "claude",
    ]:
        assert name in result.stdout


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
    assert "Never guess build state" in result.stdout


def test_preview_agent_playbook_mentions_verify_commands():
    result = run("preview", "--target", "agent-playbook")
    assert result.returncode == 0
    assert "cargo check --workspace" in result.stdout
    assert "pm.py lint" in result.stdout


def test_sync_then_lint():
    sync = run("sync")
    assert sync.returncode == 0, sync.stderr
    lint = run("lint")
    assert lint.returncode == 0, lint.stdout + lint.stderr


def test_unknown_target_fails():
    result = run("preview", "--target", "does-not-exist")
    assert result.returncode != 0
    assert "Unknown target" in result.stderr
