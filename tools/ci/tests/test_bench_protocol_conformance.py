"""Cross-language conformance and refusal tests for `BenchmarkEvidenceV1`.

The Rust crate `benchmarks/bench-protocol` is the normative definition of the
canonical form. These tests prove the Python writer/reader produces the same
bytes and refuses the same malformed documents, using the committed fixtures in
`benchmarks/bench-protocol/fixtures/` as the shared oracle.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
EVIDENCE_PATH = REPO_ROOT / "tools" / "benchmark" / "evidence.py"
FIXTURE_DIR = REPO_ROOT / "benchmarks" / "bench-protocol" / "fixtures"


def _load_evidence_module():
    spec = importlib.util.spec_from_file_location("benchmark_evidence", EVIDENCE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _sealed_text(module) -> str:
    return module.to_canonical_json(module.seal(module.sample_evidence()))


# --------------------------------------------------------------------------
# Canonical form
# --------------------------------------------------------------------------


def test_canonical_vectors_match_the_rust_oracle() -> None:
    module = _load_evidence_module()
    fixture = json.loads((FIXTURE_DIR / "canonical-json-vectors.json").read_text(encoding="utf-8"))
    assert fixture["schema_version"] == 1
    assert fixture["vectors"], "vector fixture is empty"
    for vector in fixture["vectors"]:
        assert module.canonical_json(vector["json"]) == vector["canonical"], vector["name"]


def test_sample_evidence_matches_the_rust_golden() -> None:
    module = _load_evidence_module()
    golden = (FIXTURE_DIR / "sample-evidence.json").read_text(encoding="utf-8").strip()
    sealed = module.seal(module.sample_evidence())
    assert module.to_canonical_json(sealed) == golden
    assert module.open_evidence(golden)["digest"] == sealed["digest"]


def test_seal_open_round_trips() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    reopened = module.open_evidence(text)
    assert module.to_canonical_json(reopened) == text


def test_float_canonical_form_never_uses_exponents() -> None:
    module = _load_evidence_module()
    assert module.canonical_json({"v": 1.0}) == '{"v":1.0}'
    assert module.canonical_json({"v": 1e-5}) == '{"v":0.00001}'
    assert module.canonical_json({"v": 1e16}) == '{"v":10000000000000000.0}'
    with pytest.raises(module.EvidenceError):
        module.canonical_json({"v": float("inf")})


@pytest.mark.parametrize("value", [float("nan"), float("inf"), -float("inf"), 10**1000])
def test_payload_validation_refuses_non_finite_numeric_facts(value) -> None:
    module = _load_evidence_module()
    payload = module.sample_evidence()["payload"]
    payload["rows"][0]["p50"] = value
    with pytest.raises(module.EvidenceError, match="finite"):
        module.validate_payload(payload)


def test_python_counters_match_the_rust_u64_boundary() -> None:
    module = _load_evidence_module()
    payload = module.sample_evidence()["payload"]
    payload["errors"] = (1 << 64) - 1
    module.validate_payload(payload)
    payload["errors"] += 1
    with pytest.raises(module.EvidenceError, match="u64 range"):
        module.validate_payload(payload)


# --------------------------------------------------------------------------
# Document-level refusals
# --------------------------------------------------------------------------


def test_duplicate_json_key_is_refused() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    mutated = text.replace("{", '{"family":"duplicated",', 1)
    with pytest.raises(module.EvidenceError, match="duplicate JSON key"):
        module.open_evidence(mutated)


def test_unknown_field_is_refused() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    mutated = text.replace("{", '{"bogus_field":1,', 1)
    with pytest.raises(module.EvidenceError):
        module.open_evidence(mutated)


def test_malformed_and_tampered_digests_are_refused() -> None:
    module = _load_evidence_module()
    sealed = module.seal(module.sample_evidence())
    declared = sealed["digest"]
    text = module.to_canonical_json(sealed)
    with pytest.raises(module.EvidenceError, match="invalid digest"):
        module.open_evidence(text.replace(declared, "sha256:zz"))
    retimed = text.replace('"wall_ms":12345', '"wall_ms":12346')
    assert retimed != text
    with pytest.raises(module.EvidenceError, match="digest mismatch"):
        module.open_evidence(retimed)


def test_unknown_protocol_and_version_are_refused() -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    evidence["protocol_version"] = 2
    with pytest.raises(module.EvidenceError, match="protocol_version"):
        module.seal(evidence)
    evidence = module.sample_evidence()
    evidence["protocol"] = "SomethingElse"
    with pytest.raises(module.EvidenceError, match="unsupported protocol"):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Identity and verdict refusals
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("mutate", "match"),
    [
        (lambda e: e["source"].update({"dirty": True}), "dirty_paths_digest"),
        (lambda e: e["host"].update({"policy": "canonical-linux"}), "canonical-linux"),
        (lambda e: e["verdict"].update({"scope": "performance"}), "exclusive host lease"),
        (
            lambda e: e["command"].update({"status": "timeout", "exit_code": None}),
            "cannot carry verdict",
        ),
        (lambda e: e["verdict"].update({"status": "not_run"}), "must state a reason"),
        (lambda e: e.update({"run_id": "latest"}), "invalid run id"),
        (lambda e: e["raw"][0].update({"path": "../escape.json"}), "escapes the run root"),
    ],
)
def test_identity_and_verdict_mutations_are_refused(mutate, match: str) -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    mutate(evidence)
    with pytest.raises(module.EvidenceError, match=match):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Payload confusion refusals
# --------------------------------------------------------------------------


def _retrieval_payload(**overrides):
    payload = {
        "kind": "retrieval",
        "lane": "native_default",
        "metric_space": "file",
        "judgments": "pooled",
        "unjudged": 0,
        "rows": [
            {
                "query_id": "q1",
                "metric": "recall@20",
                "unit": "ratio",
                "value": 0.5,
                "state": "judged",
            }
        ],
        "universe_attested": False,
        "corpus_digest": "sha256:" + "ab" * 32,
        "query_pack_digest": "sha256:" + "cd" * 32,
    }
    payload.update(overrides)
    return payload


@pytest.mark.parametrize(
    ("payload", "match"),
    [
        (
            {
                "kind": "micro",
                "bench_id": "lq-norm/pipeline",
                "metric": "instructions",
                "unit": "ms",
                "instrumentation": "instructions",
                "statistic": "mean",
                "value": 12.0,
                "iterations": 1,
                "samples": 1,
            },
            "contradicts instrumentation",
        ),
        (_retrieval_payload(metric_space="span", judgments="mechanically_labeled"), "span"),
        (
            _retrieval_payload(
                unjudged=1,
                rows=[
                    {
                        "query_id": "q1",
                        "metric": "recall@20",
                        "unit": "ratio",
                        "value": 0.0,
                        "state": "unjudged",
                    }
                ],
            ),
            "must not carry a score",
        ),
        (
            {
                "kind": "load",
                "arrival": "closed_loop",
                "generator_saturated": False,
                "points": [
                    {
                        "label": "clients-8",
                        "offered_rate": 200.0,
                        "completed_rate": 180.0,
                        "dropped": 0,
                        "timeouts": 0,
                    }
                ],
                "errors": 0,
            },
            "claims an offered_rate",
        ),
        (
            {
                "kind": "agent_outcome",
                "task_count": 1,
                "pair_count": 1,
                "arms": ["A", "B"],
                "excluded_pairs": 0,
                "unknown_pairs": 0,
                "metrics": [],
                "capture": "recorded_unauthenticated",
                "input_digest": "sha256:" + "ef" * 32,
            },
            "exactly \\[A, B, C\\]",
        ),
        (
            {
                "kind": "recorded_experiment",
                "experiment_id": "scan-vs-index",
                "diagnostic_only": False,
                "points": [{"label": "2000", "metric": "scan_ms", "unit": "ms", "value": 1.0}],
                "source_digest": "sha256:" + "ef" * 32,
            },
            "diagnostic_only",
        ),
    ],
)
def test_typed_payload_confusion_is_refused(payload, match: str) -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    evidence["payload"] = payload
    with pytest.raises(module.EvidenceError, match=match):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Immutable run store
# --------------------------------------------------------------------------


def _promote(module, store, evidence) -> dict:
    staged = store.stage(evidence["run_id"])
    staged.write_raw("raw/warm-matrix.json", module.SAMPLE_RAW)
    staged.write_evidence(evidence)
    return store.promote(staged)


def test_run_store_promotes_replays_and_retains_baselines(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    promotion = _promote(module, store, sealed)
    assert promotion["digest"] == sealed["digest"]
    assert store.load(sealed["run_id"]) == sealed
    assert store.read_latest()["run_id"] == sealed["run_id"]
    assert store.admit_baseline("dsl-warm", sealed["run_id"], 10_000, "blocked-paired")["run_id"]
    assert store.collect([]) == []
    assert store.load(sealed["run_id"])["digest"] == sealed["digest"]


def test_run_store_refuses_a_repeated_run_id(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    with pytest.raises(module.EvidenceError, match="already exists"):
        store.stage(sealed["run_id"])


def test_crash_before_promotion_leaves_no_admissible_run(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", module.SAMPLE_RAW)
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="missing run directory"):
        store.load(sealed["run_id"])
    assert store.read_latest() is None


def test_missing_extra_and_tampered_raw_files_are_refused(tmp_path: Path) -> None:
    module = _load_evidence_module()
    sealed = module.seal(module.sample_evidence())

    store = module.RunStore(tmp_path / "missing")
    staged = store.stage(sealed["run_id"])
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="is missing"):
        store.promote(staged)

    store = module.RunStore(tmp_path / "extra")
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", module.SAMPLE_RAW)
    staged.write_raw("raw/undeclared.json", b"surprise")
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="undeclared raw file"):
        store.promote(staged)

    store = module.RunStore(tmp_path / "tampered")
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", module.SAMPLE_RAW.replace(b"0.42", b"0.43"))
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="digest mismatch"):
        store.promote(staged)


def test_symlinked_raw_reference_is_refused(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    staged = store.stage(sealed["run_id"])
    (staged.path / "raw").mkdir(parents=True)
    target = tmp_path / "outside.json"
    target.write_bytes(module.SAMPLE_RAW)
    (staged.path / "raw" / "warm-matrix.json").symlink_to(target)
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.promote(staged)


def test_collect_removes_only_unreferenced_runs(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    store.admit_baseline("dsl-warm", sealed["run_id"], 10_000, "blocked-paired")

    other = module.sample_evidence()
    other["run_id"] = "run-20260926T130000Z-deadbeef"
    other = module.seal(other)
    _promote(module, store, other)

    assert store.collect([]) == ["run-20260926T130000Z-deadbeef"]
    assert store.load(sealed["run_id"])["digest"] == sealed["digest"]


def test_published_wire_schema_accepts_the_golden_and_refuses_malformed_documents() -> None:
    """The canonical wire schema must agree with the sealed golden document."""
    import jsonschema

    schema = json.loads(
        (REPO_ROOT / "tools" / "benchmark" / "evidence.schema.json").read_text(encoding="utf-8")
    )
    jsonschema.Draft202012Validator.check_schema(schema)
    validator = jsonschema.Draft202012Validator(schema)
    golden = json.loads((FIXTURE_DIR / "sample-evidence.json").read_text(encoding="utf-8"))
    validator.validate(golden)

    # The schema is a projection: the Rust/Python validators remain authoritative
    # for cross-field rules. Here it must reject structural corruption.
    broken = json.loads(json.dumps(golden))
    broken["protocol_version"] = 2
    assert list(validator.iter_errors(broken)), "schema accepted an unknown protocol version"

    broken = json.loads(json.dumps(golden))
    broken["payload"] = {"kind": "latency", "rows": [], "errors": 0, "timeouts": 0, "drops": 0}
    assert list(validator.iter_errors(broken)), "schema accepted an empty latency payload"

    broken = json.loads(json.dumps(golden))
    broken["payload"] = {
        "kind": "micro",
        "bench_id": "x",
        "metric": "mean",
        "unit": "bogus",
        "instrumentation": "wall",
        "statistic": "mean",
        "value": 1.0,
        "iterations": 1,
        "samples": 1,
    }
    assert list(validator.iter_errors(broken)), "schema accepted an unregistered unit"


def test_proof_counts_are_not_relevance_or_performance():
    module = _load_evidence_module()
    record = module.sample_evidence()
    record["verdict"]["scope"] = "contract"
    record["payload"] = {
        "kind": "proof",
        "rail": "retrieval-contract",
        "selected": 8,
        "executed": 8,
        "passed": 8,
        "failed": 0,
        "source_digest": record["source"]["closure_digest"],
        "execution_context_digest": record["source"]["closure_digest"],
    }
    sealed = module.seal(record)
    assert module.open_evidence(module.to_canonical_json(sealed)) == sealed
    record["verdict"]["scope"] = "quality"
    with pytest.raises(ValueError):
        module.seal(record)
    record["verdict"]["scope"] = "contract"
    record["payload"]["executed"] = 7
    with pytest.raises(ValueError):
        module.seal(record)
    record["payload"].update(executed=8, passed=7, failed=1)
    with pytest.raises(ValueError):
        module.seal(record)
    record["verdict"].update(status="fail", reason="one failed test")
    assert module.seal(record)
    record["payload"].update(passed=2**64 - 1, failed=1)
    with pytest.raises(ValueError):
        module.seal(record)
