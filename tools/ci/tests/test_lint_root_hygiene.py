"""Exercise the root hygiene gate in an isolated Git checkout."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "lint/lint-root-hygiene.sh"


def checkout(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "repo"
    script = root / "tools/ci/lint/lint-root-hygiene.sh"
    script.parent.mkdir(parents=True)
    script.write_bytes(SCRIPT.read_bytes())
    (root / "README.md").write_text("source\n")
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    subprocess.run(["git", "-C", str(root), "add", "README.md"], check=True)
    return root, script


def run(script: Path, root: Path, *, path: str | None = None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    if path is not None:
        env["PATH"] = path + os.pathsep + env["PATH"]
    return subprocess.run(["bash", str(script)], cwd=root, env=env, text=True, capture_output=True)


def test_root_and_nested_targets_are_reported_once_each(tmp_path: Path) -> None:
    root, script = checkout(tmp_path)
    (root / "target").mkdir()
    (root / "shared/isolated/target").mkdir(parents=True)

    result = run(script, root)

    assert result.returncode == 1
    assert result.stderr.count("Forbidden root artifact present: target") == 1
    assert "Forbidden nested target directory present: target" not in result.stderr
    assert (
        result.stderr.count("Forbidden nested target directory present: shared/isolated/target")
        == 1
    )


def test_nested_target_symlinks_are_rejected_without_traversal(tmp_path: Path) -> None:
    root, script = checkout(tmp_path)
    external = tmp_path / "external"
    (external / "inner/target").mkdir(parents=True)
    linked = root / "shared/isolated/target"
    linked.parent.mkdir(parents=True)
    linked.symlink_to(external, target_is_directory=True)
    dangling = root / "crates/standalone/target"
    dangling.parent.mkdir(parents=True)
    dangling.symlink_to(tmp_path / "missing", target_is_directory=True)

    result = run(script, root)

    assert result.returncode == 1
    assert "Forbidden nested target directory present: shared/isolated/target" in result.stderr
    assert "Forbidden nested target directory present: crates/standalone/target" in result.stderr
    assert "inner/target" not in result.stderr


def test_find_failure_cannot_certify_a_clean_checkout(tmp_path: Path) -> None:
    root, script = checkout(tmp_path)
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_find = fake_bin / "find"
    fake_find.write_text("#!/bin/sh\necho 'simulated find failure' >&2\nexit 23\n")
    fake_find.chmod(0o755)

    result = run(script, root, path=str(fake_bin))

    assert result.returncode == 1
    assert "simulated find failure" in result.stderr
    assert "Root target directory scan failed." in result.stderr
    assert "Root hygiene check passed." not in result.stdout


def test_dangling_root_artifact_is_rejected(tmp_path: Path) -> None:
    root, script = checkout(tmp_path)
    (root / ".codex-tmp-target").symlink_to(root / "missing")

    result = run(script, root)

    assert result.returncode == 1
    assert "Forbidden root artifact present: .codex-tmp-target" in result.stderr


def test_tracked_artifacts_still_fail_and_clean_checkout_passes(tmp_path: Path) -> None:
    root, script = checkout(tmp_path)
    assert run(script, root).returncode == 0

    tracked = root / "state/payload"
    tracked.parent.mkdir()
    tracked.write_text("artifact\n")
    subprocess.run(["git", "-C", str(root), "add", "state/payload"], check=True)

    result = run(script, root)
    assert result.returncode == 1
    assert "Tracked local artifact paths are forbidden:" in result.stderr
    assert "  - state/payload" in result.stderr
