"""Focused contract and Linux child-process checks for linux_isolation."""

import json
import os
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import linux_isolation as isolation


def _policy(tmp_path):
    allowed = tmp_path / "allowed"
    denied = tmp_path / "gold"
    output = tmp_path / "output"
    allowed.mkdir()
    output.mkdir()
    denied.write_text("hidden", encoding="utf-8")
    (allowed / "query").write_text("visible", encoding="utf-8")
    return {"readonly": [str(allowed)], "writable": [str(output)], "denied": [str(denied)]}


def test_policy_rejects_broad_grant(tmp_path):
    policy = _policy(tmp_path)
    policy["readonly"] = [str(tmp_path)]
    with pytest.raises(isolation.IsolationError, match="covers a denied path"):
        isolation.validate_policy(policy)


def test_policy_rejects_proc_and_symlink(tmp_path):
    policy = _policy(tmp_path)
    if Path("/proc").exists():
        policy["readonly"].append("/proc")
        with pytest.raises(isolation.IsolationError, match="/proc cannot be granted"):
            isolation.validate_policy(policy)
        policy["readonly"].pop()
    link = tmp_path / "alias"
    link.symlink_to(tmp_path / "allowed")
    policy["readonly"].append(str(link))
    with pytest.raises(isolation.IsolationError, match="noncanonical"):
        isolation.validate_policy(policy)


def test_policy_requires_exact_fields(tmp_path):
    policy = _policy(tmp_path)
    policy["allow_all"] = True
    with pytest.raises(isolation.IsolationError, match="exactly"):
        isolation.validate_policy(policy)


def test_probe_reports_unavailable_explicitly():
    state = isolation.probe()
    assert state["backend"] == isolation.BACKEND
    assert state["state"] in {"available", "unavailable", "error"}
    if sys.platform != "linux":
        assert state["state"] == "unavailable"


def test_cli_rejects_policy_before_exec(tmp_path):
    policy = _policy(tmp_path)
    policy["readonly"] = [str(tmp_path)]
    path = tmp_path / "policy.json"
    path.write_text(json.dumps(policy), encoding="utf-8")
    assert (
        isolation.main(["--policy", str(path), "--", str(Path(sys.executable).resolve())])
        == isolation.EXIT_REJECTED
    )


@pytest.mark.skipif(sys.platform != "linux", reason="Landlock needs a Linux kernel")
def test_linux_child_blocks_gold_and_proc(tmp_path):
    if isolation.probe()["state"] != "available":
        pytest.skip("Landlock ABI 5 unavailable")
    raw = _policy(tmp_path)
    policy = isolation.validate_policy(raw)
    pid = os.fork()
    if pid == 0:
        try:
            null = os.open("/dev/null", os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            os.close(null)
            os.closerange(3, 1024)
            isolation.enforce(policy)
            with open(Path(raw["readonly"][0]) / "query", encoding="utf-8") as stream:
                if stream.read() != "visible":
                    os._exit(1)
            for path in (raw["denied"][0], "/proc/self/environ"):
                try:
                    with open(path, "rb"):
                        os._exit(2)
                except PermissionError:
                    pass
            os._exit(0)
        except BaseException:
            os._exit(3)
    _, status = os.waitpid(pid, 0)
    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0


@pytest.mark.skipif(sys.platform != "linux", reason="Landlock needs a Linux kernel")
def test_linux_child_rejects_preopened_fd(tmp_path):
    if isolation.probe()["state"] != "available":
        pytest.skip("Landlock ABI 5 unavailable")
    raw = _policy(tmp_path)
    policy = isolation.validate_policy(raw)
    pid = os.fork()
    if pid == 0:
        try:
            null = os.open("/dev/null", os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            os.close(null)
            os.closerange(3, 1024)
            secret_fd = os.open(raw["denied"][0], os.O_RDONLY)
            try:
                isolation.enforce(policy)
            except isolation.IsolationError:
                os.close(secret_fd)
                os._exit(0)
            os._exit(1)
        except BaseException:
            os._exit(2)
    _, status = os.waitpid(pid, 0)
    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
