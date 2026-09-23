from __future__ import annotations

import hashlib
import json

from tools.ci.agent.validate_agent_output import AgentValidationError, validate_payload


def sha256(path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sample_payload(tmp_path, **overrides):
    evidence = tmp_path / "evidence.log"
    evidence.write_text("1 passed; 0 failed\n", encoding="utf-8")
    report = tmp_path / "report.json"
    report.write_text("{}\n", encoding="utf-8")
    payload = {
        "schema_version": 2,
        "status": "ok",
        "summary": "required claim verified",
        "source": {
            "revision": "a" * 40,
            "dirty_digest": "sha256:" + "b" * 64,
        },
        "required_inputs_missing": [],
        "assumptions": [],
        "claims": [
            {
                "name": "lint",
                "required": True,
                "status": "verified",
                "detail": "canonical lint completed",
                "command": "just lint",
                "evidence": [
                    {
                        "path": "evidence.log",
                        "sha256": sha256(evidence),
                        "kind": "raw_result",
                        "validator": "just lint",
                    }
                ],
                "covered_scope": ["workspace lint"],
                "excluded_scope": ["runtime behavior"],
            }
        ],
        "errors": [],
        "artifacts": [
            {
                "path": "report.json",
                "kind": "report",
                "deployable": False,
            }
        ],
    }
    payload.update(overrides)
    return payload


def assert_rejected(payload, tmp_path, message):
    try:
        validate_payload(payload, tmp_path)
    except AgentValidationError as error:
        assert str(error) == message
    else:  # pragma: no cover
        raise AssertionError("validator accepted an invalid payload")


def test_accepts_ok_payload_with_bound_claim_evidence(tmp_path):
    validate_payload(sample_payload(tmp_path), tmp_path)


def test_rejects_ok_payload_with_missing_inputs(tmp_path):
    payload = sample_payload(tmp_path, required_inputs_missing=["repo_id"])
    assert_rejected(payload, tmp_path, "status 'ok' conflicts with derived status 'blocked'")


def test_rejects_ok_payload_with_correctness_affecting_assumption(tmp_path):
    payload = sample_payload(
        tmp_path,
        assumptions=[{"name": "repo-clean", "risk": "high", "can_affect_correctness": True}],
    )
    assert_rejected(payload, tmp_path, "status 'ok' conflicts with derived status 'blocked'")


def test_rejects_ok_payload_with_failed_required_claim(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"][0]["status"] = "failed"
    assert_rejected(payload, tmp_path, "status 'ok' conflicts with derived status 'error'")


def test_rejects_not_run_required_claim_reported_as_ok(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"][0].update(
        {
            "status": "not_run",
            "detail": "external host unavailable",
            "command": None,
            "evidence": [],
            "covered_scope": [],
        }
    )
    assert_rejected(payload, tmp_path, "status 'ok' conflicts with derived status 'blocked'")


def test_allows_optional_not_run_claim_with_ok_envelope(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"].append(
        {
            "name": "release host",
            "required": False,
            "status": "not_run",
            "detail": "outside local qualification scope",
            "command": None,
            "evidence": [],
            "covered_scope": [],
            "excluded_scope": ["release qualification"],
        }
    )
    validate_payload(payload, tmp_path)


def test_blocked_payload_allows_diagnostic_but_not_deployable_artifact(tmp_path):
    payload = sample_payload(tmp_path, status="blocked", required_inputs_missing=["token"])
    validate_payload(payload, tmp_path)

    payload["artifacts"][0]["deployable"] = True
    assert_rejected(payload, tmp_path, "blocked/error must not produce deployable artifacts")


def test_rejects_forged_evidence_digest(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"][0]["evidence"][0]["sha256"] = "0" * 64
    assert_rejected(
        payload,
        tmp_path,
        "claim 'lint' evidence digest mismatch: evidence.log",
    )


def test_rejects_duplicate_claim_names(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"].append(json.loads(json.dumps(payload["claims"][0])))
    assert_rejected(payload, tmp_path, "duplicate claim name: lint")


def test_rejects_verified_claim_without_command_or_evidence(tmp_path):
    payload = sample_payload(tmp_path)
    payload["claims"][0]["command"] = None
    assert_rejected(payload, tmp_path, "verified claim lacks command: lint")

    payload = sample_payload(tmp_path)
    payload["claims"][0]["evidence"] = []
    assert_rejected(payload, tmp_path, "verified claim lacks evidence: lint")
