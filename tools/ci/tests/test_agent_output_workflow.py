"""Keep the agent-output CI gate aligned with the complete event change range."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
ZERO_SHA = "0" * 40


def git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


def commit(repo: Path, message: str) -> str:
    git(repo, "add", ".")
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD")


def changed_step() -> dict:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["agent-output"]["steps"]
    return next(step for step in steps if step.get("id") == "changed")


def validation_step() -> dict:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["agent-output"]["steps"]
    return next(
        step for step in steps if step.get("name") == "Validate agent outputs (semantic gate)"
    )


def run_changed_step(
    repo: Path, tmp_path: Path, event: str, base: str
) -> tuple[subprocess.CompletedProcess[str], str, list[str]]:
    output = tmp_path / f"github-output-{event}-{base[:8]}"
    changed = tmp_path / "agent-output-changed-files"
    if output.exists():
        output.unlink()
    if changed.exists():
        changed.unlink()
    env = os.environ.copy()
    env.update(
        {
            "GITHUB_OUTPUT": str(output),
            "RUNNER_TEMP": str(tmp_path),
            "EVENT_NAME": event,
            "PR_BASE_SHA": base if event == "pull_request" else "",
            "PUSH_BEFORE_SHA": base if event == "push" else "",
            "MERGE_BASE_SHA": base if event == "merge_group" else "",
        }
    )
    result = subprocess.run(
        ["bash", "-c", changed_step()["run"]],
        cwd=repo,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    names = (
        [name.decode("utf-8") for name in changed.read_bytes().split(b"\0") if name]
        if changed.exists()
        else []
    )
    return result, output.read_text(encoding="utf-8") if output.exists() else "", names


def test_agent_output_gate_uses_full_push_range_and_skips_idle_install(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init")
    git(repo, "config", "user.name", "CI Test")
    git(repo, "config", "user.email", "ci@example.com")
    (repo / "README.md").write_text("base\n", encoding="utf-8")
    before = commit(repo, "base")
    (repo / "nested").mkdir()
    (repo / "nested/agent_output_first.json").write_text("{}\n", encoding="utf-8")
    commit(repo, "add agent output")
    (repo / "README.md").write_text("later\n", encoding="utf-8")
    commit(repo, "unrelated follow-up")

    for event in ("push", "pull_request", "merge_group"):
        result, output, names = run_changed_step(repo, tmp_path, event, before)
        assert result.returncode == 0, result.stderr
        assert output == "has_files=true\n"
        assert names == ["nested/agent_output_first.json"]

    unchanged, output, names = run_changed_step(
        repo, tmp_path, "push", git(repo, "rev-parse", "HEAD")
    )
    assert unchanged.returncode == 0, unchanged.stderr
    assert output == "has_files=false\n"
    assert names == []

    before_unusual_name = git(repo, "rev-parse", "HEAD")
    unusual_name = "agent_output_$(echo injected)\nfile.json"
    (repo / unusual_name).write_text("{}\n", encoding="utf-8")
    commit(repo, "unusual agent output filename")
    unusual, output, names = run_changed_step(repo, tmp_path, "push", before_unusual_name)
    assert unusual.returncode == 0, unusual.stderr
    assert output == "has_files=true\n"
    assert names == [unusual_name]

    install = next(
        step
        for step in yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]["agent-output"][
            "steps"
        ]
        if step.get("name") == "Install agent-output dependencies"
    )
    assert install["if"] == "steps.changed.outputs.has_files == 'true'"
    assert "--skip-rust-gates" not in validation_step()["run"]


def test_agent_output_gate_handles_new_branch_and_fails_closed(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init")
    git(repo, "config", "user.name", "CI Test")
    git(repo, "config", "user.email", "ci@example.com")
    (repo / "agent_output.json").write_text("{}\n", encoding="utf-8")
    commit(repo, "initial")

    new_branch, output, names = run_changed_step(repo, tmp_path, "push", ZERO_SHA)
    assert new_branch.returncode == 0, new_branch.stderr
    assert output == "has_files=true\n"
    assert names == ["agent_output.json"]

    missing_base, _, _ = run_changed_step(repo, tmp_path, "push", "a" * 40)
    assert missing_base.returncode != 0
    assert "invalid or unavailable base commit" in missing_base.stderr
