from __future__ import annotations

from tools.ci.agent.validate_agent_output import AgentValidationError, validate_payload


def sample_payload(**overrides):
    payload = {
        "status": "ok",
        "summary": "all checks passed",
        "required_inputs_missing": [],
        "assumptions": [],
        "checks": [{"name": "lint", "passed": True, "evidence": "cargo clippy green"}],
        "errors": [],
        "artifacts": [{"path": "notes/report.json", "kind": "report", "deployable": False}],
    }
    payload.update(overrides)
    return payload


def test_accepts_ok_payload_without_correctness_affecting_assumption(tmp_path):
    validate_payload(sample_payload(), tmp_path, skip_rust_gates=True)


def test_rejects_ok_payload_with_missing_inputs(tmp_path):
    payload = sample_payload(required_inputs_missing=["repo_id"])

    try:
        validate_payload(payload, tmp_path, skip_rust_gates=True)
    except AgentValidationError as error:
        assert str(error) == "ok with missing inputs"
    else:  # pragma: no cover
        raise AssertionError("validator accepted missing inputs")


def test_rejects_ok_payload_with_correctness_affecting_assumption(tmp_path):
    payload = sample_payload(
        assumptions=[{"name": "repo-clean", "risk": "high", "can_affect_correctness": True}]
    )

    try:
        validate_payload(payload, tmp_path, skip_rust_gates=True)
    except AgentValidationError as error:
        assert str(error) == "ok with correctness-affecting assumption"
    else:  # pragma: no cover
        raise AssertionError("validator accepted a correctness-affecting assumption")


def test_rejects_ok_payload_with_failed_check(tmp_path):
    payload = sample_payload(checks=[{"name": "test", "passed": False, "evidence": "red"}])

    try:
        validate_payload(payload, tmp_path, skip_rust_gates=True)
    except AgentValidationError as error:
        assert str(error) == "ok with failed check"
    else:  # pragma: no cover
        raise AssertionError("validator accepted a failed check")


def test_rejects_blocked_payload_with_artifacts(tmp_path):
    payload = sample_payload(status="blocked")

    try:
        validate_payload(payload, tmp_path, skip_rust_gates=True)
    except AgentValidationError as error:
        assert str(error) == "blocked/error must not produce deployable artifacts"
    else:  # pragma: no cover
        raise AssertionError("validator accepted blocked output with artifacts")
