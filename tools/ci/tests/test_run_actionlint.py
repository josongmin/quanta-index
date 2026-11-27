"""The workflow lint rail must not succeed without workflows to inspect."""

from __future__ import annotations

import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]


def test_actionlint_config_selects_whole_workflow_scan() -> None:
    config = yaml.safe_load((ROOT / ".pre-commit-config.yaml").read_text(encoding="utf-8"))
    hooks = {hook["id"]: hook for repo in config["repos"] for hook in repo["hooks"]}
    validator = hooks["actionlint-config"]
    assert re.fullmatch(validator["files"], ".github/actionlint.yaml")
    assert not re.search(validator["files"], ".github/workflows/ci.yml")
    assert not re.search(hooks["actionlint"]["files"], ".github/actionlint.yaml")
    assert validator["pass_filenames"] is False
    assert validator["stages"] == ["pre-commit", "pre-push"]
    assert shlex.split(validator["entry"]) == ["bash", "scripts/run-actionlint.sh"]


@pytest.mark.parametrize("change", ["create", "rewrite", "delete"])
def test_actionlint_refuses_config_changed_during_scan(tmp_path: Path, change: str) -> None:
    workflow = Path(".github/workflows/check.yml")
    (tmp_path / workflow.parent).mkdir(parents=True)
    (tmp_path / workflow).write_text("name: lint\n", encoding="utf-8")
    settings = Path(".github/actionlint.yaml")
    if change != "create":
        (tmp_path / settings).write_text(
            "self-hosted-runner:\n  labels: [before]\n", encoding="utf-8"
        )
    action = (
        "unlink()"
        if change == "delete"
        else "write_text('self-hosted-runner:\\n  labels: [after]\\n')"
    )
    code = f"from pathlib import Path; Path({str(settings)!r}).{action}"
    config = {
        "repos": [
            {
                "repo": "local",
                "hooks": [
                    {
                        "id": "actionlint",
                        "name": "actionlint fixture",
                        "entry": shlex.join([sys.executable, "-c", code]),
                        "language": "system",
                    }
                ],
            }
        ]
    }
    (tmp_path / ".pre-commit-config.yaml").write_text(yaml.safe_dump(config), encoding="utf-8")
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/ci/run-read-only-hook.py"), "actionlint", str(workflow)],
        cwd=tmp_path,
        env={**os.environ, "PRE_COMMIT_HOME": str(tmp_path / "hook-cache"), "SKIP": ""},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1, result.stdout + result.stderr
    assert f"Lint input changed while checking actionlint — {settings}" in result.stderr


@pytest.mark.parametrize(
    ("workflow_names", "directory_names"),
    [
        ((), ()),
        ((), ("directory.yml",)),
        (("check.yml", "review.yaml"), ("directory.yaml",)),
    ],
)
def test_actionlint_uses_locked_python_for_present_workflows(
    tmp_path: Path, workflow_names: tuple[str, ...], directory_names: tuple[str, ...]
) -> None:
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    wrapper = scripts / "run-actionlint.sh"
    wrapper.write_bytes((ROOT / "scripts/run-actionlint.sh").read_bytes())
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_uv = fake_bin / "uv"
    fake_uv.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CALL_LOG"\n', encoding="utf-8")
    fake_uv.chmod(0o755)
    log = tmp_path / "calls"
    for name in directory_names:
        (tmp_path / ".github" / "workflows" / name).mkdir(parents=True)
    for name in workflow_names:
        workflow = tmp_path / ".github" / "workflows" / name
        workflow.parent.mkdir(parents=True, exist_ok=True)
        workflow.write_text("name: lint\n", encoding="utf-8")

    result = subprocess.run(
        ["bash", str(wrapper)],
        cwd=tmp_path,
        env={**os.environ, "PATH": f"{fake_bin}:{os.environ['PATH']}", "CALL_LOG": str(log)},
        capture_output=True,
        text=True,
        check=False,
    )

    if not workflow_names:
        assert result.returncode != 0
        assert "No workflow files found" in result.stderr
        assert not log.exists()
        return

    assert result.returncode == 0, result.stderr
    assert log.read_text(encoding="utf-8").splitlines() == [
        "run",
        "--frozen",
        "--extra",
        "dev",
        "python",
        "tools/ci/run-read-only-hook.py",
        "actionlint",
        *(f".github/workflows/{name}" for name in workflow_names),
    ]


@pytest.mark.parametrize(("skip", "expected"), [("", 1), ("actionlint", 0)])
def test_actionlint_reports_when_config_excludes_all_workflows(
    tmp_path: Path, skip: str, expected: int
) -> None:
    workflow = Path(".github/workflows/check.yml")
    (tmp_path / workflow.parent).mkdir(parents=True)
    (tmp_path / workflow).write_text("name: lint\n", encoding="utf-8")
    (tmp_path / ".pre-commit-config.yaml").write_text(
        "repos:\n- repo: local\n  hooks:\n  - id: actionlint\n"
        "    name: actionlint fixture\n    entry: 'true'\n    language: system\n"
        "    files: '^docs/'\n",
        encoding="utf-8",
    )
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/ci/run-read-only-hook.py"), "actionlint", str(workflow)],
        cwd=tmp_path,
        env={**os.environ, "PRE_COMMIT_HOME": str(tmp_path / "hook-cache"), "SKIP": skip},
        capture_output=True,
        text=True,
    )
    assert result.returncode == expected, result.stdout + result.stderr
    assert ("no eligible lint inputs" in result.stderr) == bool(expected)
