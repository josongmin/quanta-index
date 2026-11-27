"""Pin the candidate execution gate to the active Python CI rail."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

from tools.ci.verify_parser_projection_ci import validate_junit

ROOT = Path(__file__).resolve().parents[3]


def report(go: str = "", ts: str = "") -> bytes:
    return (
        "<testsuite>"
        f'<testcase classname="tools.ci.tests.test_go_projection" name="go">{go}</testcase>'
        f'<testcase classname="tools.ci.tests.test_ts_projection" name="ts">{ts}</testcase>'
        "</testsuite>"
    ).encode()


def test_both_projection_modules_execute_without_skips() -> None:
    assert validate_junit(report()) == {"test_go_projection": 1, "test_ts_projection": 1}


@pytest.mark.parametrize("module", ["go", "ts"])
def test_skipped_projection_is_rejected(module: str) -> None:
    kwargs = {module: '<skipped message="missing toolchain" />'}
    with pytest.raises(ValueError, match="was skipped"):
        validate_junit(report(**kwargs))


def test_missing_module_and_malformed_xml_are_rejected() -> None:
    with pytest.raises(ValueError, match="bodies missing"):
        validate_junit(
            b'<testsuite><testcase classname="tools.ci.tests.test_go_projection" /></testsuite>'
        )
    with pytest.raises(ValueError, match="malformed"):
        validate_junit(b"<testsuite>")


def test_active_pinned_python_rail_checks_the_same_junit_report() -> None:
    config = yaml.safe_load((ROOT / ".circleci/config.yml").read_text(encoding="utf-8"))
    steps = config["jobs"]["verify-python"]["steps"]
    installs = [
        step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and isinstance(step.get("run"), dict)
        and step["run"].get("name") == "Install pinned parser validation tools"
    ]
    policies = [
        step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and isinstance(step.get("run"), dict)
        and step["run"].get("name") == "Python policy and tooling tests"
    ]
    assert len(installs) == len(policies) == 1
    assert "go1.25.4.linux-amd64.tar.gz" in installs[0]
    assert "sudo apt-get install -y nodejs npm" in installs[0]
    assert "python -m pytest tools -q" in policies[0]
    assert '--junitxml="$projection_report"' in policies[0]
    assert 'python tools/ci/verify_parser_projection_ci.py "$projection_report"' in policies[0]
