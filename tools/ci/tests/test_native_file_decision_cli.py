"""Native decision CLI dispatch stays separate from Legacy qualification."""

import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import decision
from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import native_file_decision as native


def _args(tmp_path: Path) -> list[str]:
    return [
        "--native-file-cohort",
        str(tmp_path / "cohort.json"),
        "--policy",
        str(tmp_path / "policy.json"),
        "--daemon-binary",
        str(tmp_path / "daemon"),
        "--daemon-config",
        str(tmp_path / "config.json"),
        "--runner-binary",
        str(tmp_path / "runner"),
        "--out",
        str(tmp_path / "decision.json"),
    ]


def test_native_decision_cli_uses_native_replay_and_preserves_product_boundary(
    tmp_path, monkeypatch
):
    calls = []

    def replay(*paths):
        calls.append(paths)
        return {"status": "quality_admit", "product_default_decision": False}

    monkeypatch.setattr(decision, "build_native_file_decision", replay, raising=False)
    assert decision.main(_args(tmp_path)) == 0
    assert calls == [
        tuple(
            tmp_path / name
            for name in (
                "cohort.json",
                "policy.json",
                "daemon",
                "config.json",
                "runner",
            )
        )
    ]
    assert json.loads((tmp_path / "decision.json").read_text())["product_default_decision"] is False


@pytest.mark.parametrize(
    "extra",
    [
        ["--repo", "repo"],
        ["--suite", "suite"],
        ["--run-manifest", "run"],
        ["--go-trace-plan", "plan"],
    ],
)
def test_native_decision_cli_refuses_mixed_profile_before_replay(tmp_path, monkeypatch, extra):
    monkeypatch.setattr(
        decision,
        "build_native_file_decision",
        lambda *_: pytest.fail("mixed profile replayed"),
        raising=False,
    )
    assert decision.main(_args(tmp_path) + extra) == 2
    assert not (tmp_path / "decision.json").exists()


def test_native_decision_cli_never_overwrites_existing_output(tmp_path, monkeypatch):
    output = tmp_path / "decision.json"
    output.write_text("preserved")
    monkeypatch.setattr(
        decision,
        "build_native_file_decision",
        lambda *_: pytest.fail("output conflict replayed"),
        raising=False,
    )
    assert decision.main(_args(tmp_path)) == 2
    assert output.read_text() == "preserved"


def test_native_decision_cli_forwards_controlled_capture_runs(tmp_path, monkeypatch):
    observed = []

    def replay(*paths, **options):
        observed.append((paths, options))
        return {"status": "quality_admit", "product_default_decision": False}

    monkeypatch.setattr(decision, "build_native_file_decision", replay)
    runs = tmp_path / "runs.json"
    runs.write_text(json.dumps(["controlled-run-a"]))
    store = tmp_path / "store"
    assert (
        decision.main(
            _args(tmp_path)
            + [
                "--native-execution-root",
                str(store),
                "--native-execution-runs",
                str(runs),
            ]
        )
        == 0
    )
    assert observed[0][1] == {
        "execution_evidence_root": store,
        "execution_run_ids": ["controlled-run-a"],
    }


@pytest.mark.parametrize("flag", ["--native-execution-root", "--native-execution-runs"])
def test_native_decision_cli_requires_both_execution_inputs(tmp_path, monkeypatch, flag):
    monkeypatch.setattr(
        decision, "build_native_file_decision", lambda *_: pytest.fail("partial evidence delegated")
    )
    assert decision.main(_args(tmp_path) + [flag, str(tmp_path / "input")]) == 2
    assert not (tmp_path / "decision.json").exists()


@pytest.mark.parametrize("kind", ["manifest", "policy"])
def test_native_control_inputs_refuse_special_files_before_json_parsing(kind):
    with pytest.raises(ValueError, match="regular"):
        if kind == "manifest":
            ev._native_cohort_manifest(Path("/dev/null"))
        else:
            native._policy(Path("/dev/null"), "", {})


@pytest.mark.parametrize("kind", ["manifest", "policy"])
def test_native_control_inputs_enforce_existing_control_byte_limit(tmp_path, kind):
    from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES
    from tools.ci.tests.test_native_file_cohort import _manifest

    manifest_path, _ = _manifest(tmp_path)
    manifest_raw = manifest_path.read_bytes()
    manifest = json.loads(manifest_raw)
    policy = {
        "schema_version": 1,
        "profile": "native_file_decision",
        "cohort_manifest_sha256": ev.digest(manifest_raw),
        "repositories": manifest["repositories"],
        "repository_strata": manifest["repository_strata"],
        "minimum_counts": {"positive": 100, "identifier": 30, "negative": 30, "hard_negative": 30},
        "no_answer_rule": "nonempty_top10_is_false_positive",
        "resource_limits": {"max_query_p95_micros": 100000},
    }
    target = manifest_path if kind == "manifest" else tmp_path / "policy.json"
    raw = manifest_raw if kind == "manifest" else json.dumps(policy).encode()
    target.write_bytes(raw + b" " * (CONTROL_DOCUMENT_BYTES + 1 - len(raw)))
    with pytest.raises(ValueError, match="byte limit"):
        if kind == "manifest":
            ev._native_cohort_manifest(target)
        else:
            native._policy(target, ev.digest(manifest_raw), manifest)
