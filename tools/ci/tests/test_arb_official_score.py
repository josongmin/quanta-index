"""Independent fixed gold for the ARB top-100 to top-20 file projection."""

from __future__ import annotations

import copy
import hashlib
import importlib.machinery
import json
import os
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

from tools.benchmark.retrieval import arb_official_score as scorer
from tools.ci.tests.test_retrieval_benchmark import fixture_v3


def test_combined_suite_accepts_quanta_projection_and_refuses_digest_or_route_drift(tmp_path):
    repo, suite, record, *_ = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = sorted(scorer.ROUTES)
    record["route_provenance"]["semantic"] = {"capture_id": "q0"}
    record["results"].extend(
        {**copy.deepcopy(row), "route": "semantic"}
        for row in list(record["results"])
        if row["route"] == "hybrid"
    )
    _, native_pack, source = scorer.evaluator.validate_suite(repo, suite)
    record["query_pack_sha256"] = scorer.evaluator.digest(scorer.evaluator.canonical(native_pack))
    # The independent three-route suite accepts the record before combining it.
    scorer.evaluator.validate_evidence_against_suite(repo, suite, native_pack, source, record)
    combined = {**suite, "routes": sorted([*scorer.ROUTES, "semble-hybrid"])}
    _, combined_pack, source = scorer.evaluator.validate_suite(repo, combined)
    with pytest.raises(scorer.evaluator.EvidenceError, match="query pack hash mismatch"):
        scorer.evaluator.validate_evidence_against_suite(
            repo, combined, combined_pack, source, record
        )
    before = copy.deepcopy((combined, combined_pack, record))
    assert scorer.validate_quanta_record(repo, combined, combined_pack, source, record) == record
    assert (combined, combined_pack, record) == before
    wrong_digest = copy.deepcopy(record)
    wrong_digest["query_pack_sha256"] = scorer.evaluator.digest(
        scorer.evaluator.canonical(combined_pack)
    )
    with pytest.raises(scorer.evaluator.EvidenceError, match="query pack hash mismatch"):
        scorer.validate_quanta_record(repo, combined, combined_pack, source, wrong_digest)
    for routes in ({"lexical", "hybrid"}, {*scorer.ROUTES, "semble-hybrid"}):
        wrong_routes = copy.deepcopy(record)
        wrong_routes["route_provenance"] = {route: {"capture_id": "q0"} for route in routes}
        with pytest.raises(scorer.ScoringRefusal, match="route set mismatch"):
            scorer.validate_quanta_record(repo, combined, combined_pack, source, wrong_routes)


def test_official_git_source_guard_rejects_dirty_helper_and_untracked_module(tmp_path):
    arb = tmp_path / "arb"
    source = arb / "src/agent_retrieval_bench"
    source.mkdir(parents=True)
    helper = source / "io.py"
    helper.write_text("SOURCE = 1\n", encoding="utf-8")
    (arb / ".gitignore").write_text("src/agent_retrieval_bench/ignored.py\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(arb), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(arb), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=ARB",
            "-c",
            "user.email=arb@example.test",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-C",
            str(arb),
            "commit",
            "-qm",
            "fixed",
        ],
        check=True,
    )
    commit = subprocess.check_output(
        ["git", "-C", str(arb), "rev-parse", "HEAD"], text=True
    ).strip()
    scorer.assert_git_source_clean(arb, commit)
    with pytest.raises(scorer.ScoringRefusal, match="commit mismatch"):
        scorer.assert_git_source_clean(arb, "0" * 40)
    helper.write_text("SOURCE = 2\n", encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match="tracked or untracked"):
        scorer.assert_git_source_clean(arb, commit)
    helper.write_text("SOURCE = 1\n", encoding="utf-8")
    foreign = source / "injected.py"
    foreign.write_text("SOURCE = 3\n", encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match="tracked or untracked"):
        scorer.assert_git_source_clean(arb, commit)
    foreign.unlink()
    (source / "ignored.py").write_text("SOURCE = 4\n", encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match="ignored Python modules"):
        scorer.assert_git_source_clean(arb, commit)


def test_cached_foreign_arb_helper_origin_is_rejected(monkeypatch, tmp_path):
    source = (
        Path(
            os.environ.get(
                "ARB_B06_ROOT",
                "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06",
            )
        )
        / "arb/src/agent_retrieval_bench"
    )
    fake_path = tmp_path / "foreign_io.py"
    fake_path.write_text("", encoding="utf-8")
    module = ModuleType("agent_retrieval_bench.io")
    module.__file__ = str(fake_path)
    module.__spec__ = importlib.machinery.ModuleSpec(
        "agent_retrieval_bench.io", loader=None, origin=str(fake_path)
    )
    monkeypatch.setitem(sys.modules, "agent_retrieval_bench.io", module)
    with pytest.raises(scorer.ScoringRefusal, match="foreign ARB module origin"):
        scorer.assert_cached_arb_origins(source)


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


class FixedOfficial:
    """Minimal official-function surface; expected scores below are fixed constants."""

    @staticmethod
    def query_text_for_eval(sample):
        return json.dumps(sample["query"], ensure_ascii=False, sort_keys=True)

    @staticmethod
    def target_gold_files(sample):
        return sample["gold"]["files"]

    @staticmethod
    def query_has_leakage(sample, query):
        return False

    @staticmethod
    def recall_at(gold, paths, k):
        return len(gold.intersection(paths[:k])) / len(gold)

    @staticmethod
    def reciprocal_rank(gold, paths):
        return next((1 / rank for rank, path in enumerate(paths, 1) if path in gold), 0.0)


def fixture():
    sid = "fixed-1"
    original = json.dumps({"failure_excerpt": "panic"}, ensure_ascii=False, sort_keys=True)
    adapted = "panic"
    original_sha = digest(original)
    adapted_sha = digest(adapted)
    gold = ["cause.go", "test.go"]
    files = [
        {"path": path, "file_sha256": "a" * 64} for path in ("cause.go", "test.go", "noise.go")
    ]
    sample = {
        "id": sid,
        "repo": "gin-gonic/gin",
        "base_commit": "b" * 40,
        "query": {"failure_excerpt": "panic"},
        "gold": {"files": gold},
    }
    case = {
        "sample_id": sid,
        "base_commit": "b" * 40,
        "original_query_sha256": original_sha,
        "adapted_query_sha256": adapted_sha,
        "official_gold_files_sha256": digest('["cause.go","test.go"]'),
        "manifest": "/tmp/source.json",
        "suite": "/tmp/suite.json",
        "pack": "/tmp/pack.json",
    }
    task = {"task_id": "ARB-" + sid, "query": adapted, "query_sha256": adapted_sha}
    suite = {"repository_commit": "b" * 40, "file_universe": files, "tasks": [task]}
    pack = {"repository_commit": "b" * 40, "file_universe": files, "tasks": [task]}
    spec = {
        "top_k": 100,
        "routes": list(scorer.ROUTES),
        "repo": "/tmp/repo",
        "manifest": case["manifest"],
        "suite": case["suite"],
        "query_pack": case["pack"],
        "strategies": [{"name": "fixed_window_strict", "overlap_bytes": 256, "window_bytes": 4096}],
    }
    manifest = {"repository_commit": "b" * 40, "files": files}
    index = {sid: {"base_commit": "b" * 40, "query_sha256": original_sha, "arb_gold_files": gold}}
    return case, sample, suite, pack, spec, manifest, index


def test_fixed_gold_projection_deduplicates_chunks_before_top_20():
    candidates = [
        {"rank": i, "path": path}
        for i, path in enumerate(["noise.go", "noise.go", "test.go", "noise.go", "cause.go"], 1)
    ]
    top = scorer.project_files(candidates, {"noise.go", "test.go", "cause.go"}, 100)
    assert top == ["noise.go", "test.go", "cause.go"]
    # Independent expected: both of two gold files found; first at file rank 2.
    assert scorer.file_metrics(["cause.go", "test.go"], top, FixedOfficial) == {
        "Recall@20": 1.0,
        "MRR@20": 0.5,
    }


def test_fixed_gold_file_not_in_top_20_has_zero_credit():
    paths = [f"noise-{i}.go" for i in range(20)] + ["cause.go"]
    candidates = [{"rank": i, "path": path} for i, path in enumerate(paths, 1)]
    top = scorer.project_files(candidates, set(paths), 100)
    assert len(top) == 20
    assert scorer.file_metrics(["cause.go"], top, FixedOfficial) == {
        "Recall@20": 0.0,
        "MRR@20": 0.0,
    }


def test_abstain_is_completed_zero_and_errors_have_no_metric():
    gold = ["cause.go"]
    universe = {"cause.go"}
    abstain = scorer.score_route(
        {"status": "abstained", "candidates": [], "error": None},
        gold,
        universe,
        100,
        FixedOfficial,
    )
    assert abstain["status"] == "COMPLETED"
    assert abstain["top_files"] == []
    assert abstain["metrics"] == {"Recall@20": 0.0, "MRR@20": 0.0}
    assert abstain["bcy"]["status"] == "BLOCKED"
    failed = scorer.score_route(
        {"status": "timeout", "candidates": [], "error": {"code": "TIMEOUT"}},
        gold,
        universe,
        100,
        FixedOfficial,
    )
    assert failed == {"status": "FAILED", "reason": {"code": "TIMEOUT"}}
    with pytest.raises(scorer.ScoringRefusal, match="abstained result carries candidates"):
        scorer.score_route(
            {"status": "abstained", "candidates": [{"rank": 1, "path": "cause.go"}]},
            gold,
            universe,
            100,
            FixedOfficial,
        )


def test_arm_denominators_refuse_boolean_numeric_alias():
    arm = {
        "requested_denominator": 88,
        "current_v3_plan_accepted": 1,
        "current_v3_plan_refused": 87,
    }
    assert scorer.arm_denominators(arm, 1) == (88, 1, 87)
    arm["current_v3_plan_accepted"] = True
    with pytest.raises(scorer.ScoringRefusal, match="must be integers"):
        scorer.arm_denominators(arm, 1)


@pytest.mark.parametrize(
    "candidates,universe",
    [
        ([{"rank": 1, "path": "outside.go"}], {"cause.go"}),
        ([{"rank": 2, "path": "cause.go"}], {"cause.go"}),
        ([{"rank": 1, "path": "cause.go"}, {"rank": 1, "path": "cause.go"}], {"cause.go"}),
    ],
)
def test_projection_refuses_universe_and_rank_violation(candidates, universe):
    with pytest.raises(scorer.ScoringRefusal):
        scorer.project_files(candidates, universe, 100)


def test_fixed_identity_accepts_separate_original_and_adapted_queries():
    case, sample, suite, pack, spec, manifest, index = fixture()
    assert scorer.bind_identity(
        case, sample, suite, pack, spec, manifest, index, FixedOfficial
    ) == ["cause.go", "test.go"]


@pytest.mark.parametrize(
    "mutation",
    [
        lambda c, s, u, p, x, m, i: s["query"].update(failure_excerpt="changed"),
        lambda c, s, u, p, x, m, i: s.update(base_commit="c" * 40),
        lambda c, s, u, p, x, m, i: s["gold"].update(files=["noise.go"]),
        lambda c, s, u, p, x, m, i: u["tasks"][0].update(query="changed"),
        lambda c, s, u, p, x, m, i: u.update(file_universe=[{"path": "noise.go"}]),
        lambda c, s, u, p, x, m, i: x.update(top_k=10),
    ],
)
def test_fixed_identity_refuses_raw_query_source_gold_adapter_universe_and_policy(mutation):
    values = fixture()
    mutation(*values)
    with pytest.raises(scorer.ScoringRefusal):
        scorer.bind_identity(*values, FixedOfficial)


def test_read_pinned_refuses_changed_bytes(tmp_path):
    path = tmp_path / "record.json"
    path.write_text('{"x":1}', encoding="utf-8")
    expected = digest('{"x":1}')
    assert scorer.read_pinned(path, expected) == {"x": 1}
    path.write_text('{"x":2}', encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match="digest mismatch"):
        scorer.read_pinned(path, expected)


@pytest.mark.parametrize(
    "raw,reason",
    [
        ('{"x":1,"x":2}', "duplicate JSON key"),
        ('{"x":NaN}', "non-finite JSON constant"),
        ('{"x":1e999}', "non-finite"),
    ],
)
def test_read_pinned_refuses_ambiguous_json(tmp_path, raw, reason):
    path = tmp_path / "control.json"
    path.write_text(raw, encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match=reason):
        scorer.read_pinned(path, digest(raw))


def test_read_pinned_refuses_symlink(tmp_path):
    target = tmp_path / "target.json"
    target.write_text("{}", encoding="utf-8")
    link = tmp_path / "alias.json"
    link.symlink_to(target)
    with pytest.raises(scorer.ScoringRefusal, match="symlink"):
        scorer.read_pinned(link, digest("{}"))


def test_refusal_requires_same_task_and_submitted_query():
    payload = {
        "schema_version": 1,
        "kind": "quanta_retrieval_query_plan_refusal",
        "phase": "query_plan",
        "task_id": "ARB-fixed-1",
        "original_query_sha256": "a" * 64,
        "error": {"code": "RBR_QUERY_REFUSED"},
    }
    assert scorer.validate_refusal(payload, "fixed-1", "a" * 64) == "RBR_QUERY_REFUSED"
    with pytest.raises(scorer.ScoringRefusal, match="query mismatch"):
        scorer.validate_refusal(payload, "fixed-1", "b" * 64)
    with pytest.raises(scorer.ScoringRefusal, match="task ID mismatch"):
        scorer.validate_refusal(payload, "other", "a" * 64)
    payload["schema_version"] = True
    with pytest.raises(scorer.ScoringRefusal, match="schema"):
        scorer.validate_refusal(payload, "fixed-1", "a" * 64)


def test_valid_old_record_cannot_be_replayed_into_new_arm():
    profile = {"policy": "natural_language", "profile_id": "fixed", "config": {}}
    spec = {
        "runner_name": "quanta-sdk",
        "run_id": "run-new",
        "runner_binary": "/tmp/runner",
        "searchd_binary": "/tmp/searchd",
        "searchd_expected_sha256": "s" * 64,
        "blinding": "attested",
        "isolation_method": "query-pack-only",
        "access_block_log": "runner-was-not-given-gold",
        "execution_profiles": {"quanta": profile},
        "strategies": [{"name": "fixed_window_strict"}],
    }
    arm = {
        "runner_binary_sha256": "n" * 64,
        "runner_binary": "/tmp/runner",
        "searchd_binary": "/tmp/searchd",
        "searchd_binary_sha256": "s" * 64,
    }
    record = {
        "runner": {
            "name": "quanta-sdk",
            "run_id": "run-new-fixed_window_strict",
            "revision": "sha256:" + "n" * 64,
            "blinding": "attested",
            "isolation_method": "query-pack-only",
            "access_block_log": "runner-was-not-given-gold",
        },
        "captures": {"one": {"system": "quanta", "execution_profile": profile}},
    }
    scorer.bind_record_identity(record, spec, arm)
    record["runner"]["revision"] = "sha256:" + "o" * 64
    with pytest.raises(scorer.ScoringRefusal, match="binary digest mismatch"):
        scorer.bind_record_identity(record, spec, arm)
    record["runner"]["revision"] = "sha256:" + "n" * 64
    record["captures"]["one"]["execution_profile"] = {**profile, "profile_id": "changed"}
    with pytest.raises(scorer.ScoringRefusal, match="execution profile mismatch"):
        scorer.bind_record_identity(record, spec, arm)


def test_actual_native_record_uses_binary_digest_not_source_commit():
    path = Path(
        os.environ.get(
            "QUANTA_FIXED_NATIVE_RECORD",
            "/private/tmp/qs1iztd4ov/c/capture/strategy-00-fw_strict/record.json",
        )
    )
    if not path.is_file():
        pytest.skip("fixed native record fixture is unavailable")
    record = scorer.read_strict(path)[0]
    expected_binary = "e51a5e0e3355348ae7518ed29adf422b209d65105ea13a5569cb293af8b7471b"
    assert record["runner"]["revision"] == "sha256:" + expected_binary
    assert record["runner"]["run_id"] == (
        "scanner-bat338-f606-fresh-closed-diagnostic-fixed_window_strict"
    )
    first_capture = next(iter(record["captures"].values()))
    assert first_capture["execution_profile"]["profile_id"] == "quanta-code-search-typo-file-v1"
    one = {"runner": record["runner"], "captures": {"first": first_capture}}
    spec = {
        "runner_name": "quanta-sdk-runner",
        "run_id": "scanner-bat338-f606-fresh-closed-diagnostic",
        "strategies": [{"name": "fixed_window_strict"}],
        "runner_binary": "/fixed/runner",
        "searchd_binary": "/fixed/searchd",
        "searchd_expected_sha256": "s" * 64,
        "blinding": "attested",
        "isolation_method": "attested-only: same-checkout pack consumer",
        "access_block_log": (
            "attested-only: no suite path is passed to the runner; "
            "pack blindness verified by freeze"
        ),
        "execution_profiles": {"quanta": first_capture["execution_profile"]},
    }
    arm = {
        "runner_binary_sha256": expected_binary,
        "runner_binary": "/fixed/runner",
        "searchd_binary": "/fixed/searchd",
        "searchd_binary_sha256": "s" * 64,
    }
    scorer.bind_record_identity(one, spec, arm)
    changed = copy.deepcopy(one)
    changed["runner"]["revision"] = "f60609fe2d2882f1193d4ba68deb8704e8972138"
    with pytest.raises(scorer.ScoringRefusal, match="binary digest mismatch"):
        scorer.bind_record_identity(changed, spec, arm)


def test_capture_preparation_permits_only_declared_metadata_edits(tmp_path):
    frozen_path = tmp_path / "frozen.json"
    capture_path = tmp_path / "capture.json"
    arm_path = tmp_path / "arm.json"
    prep_path = tmp_path / "preparation.json"
    frozen = {
        "query_pack": "same",
        "run_id": "old",
        "output_root": "/old",
        "access_block_log": "old",
        "isolation_method": "old",
    }
    current = {
        **frozen,
        "run_id": "new",
        "output_root": "/new",
        "access_block_log": "NOT_RUN",
        "isolation_method": "diagnostic",
    }
    frozen_path.write_text(json.dumps(frozen), encoding="utf-8")
    capture_path.write_text(json.dumps(current), encoding="utf-8")
    arm_path.write_text("{}", encoding="utf-8")
    case = {
        "sample_id": "sample",
        "spec": str(frozen_path),
        "spec_sha256": scorer.sha256(frozen_path.read_bytes()),
        "manifest": "m",
        "manifest_sha256": "msha",
        "suite": "s",
        "suite_sha256": "ssha",
        "pack": "p",
        "pack_sha256": "psha",
    }
    copied = {
        **case,
        "prepared_spec": case["spec"],
        "prepared_spec_sha256": case["spec_sha256"],
        "spec": str(capture_path),
        "spec_sha256": scorer.sha256(capture_path.read_bytes()),
        "output_root": "/new",
    }
    prep = {
        "source_revision": "r" * 40,
        "arms": [
            {
                "input_manifest": str(arm_path),
                "input_manifest_sha256": scorer.sha256(arm_path.read_bytes()),
                "cases": [copied],
            }
        ],
    }
    prep_path.write_text(json.dumps(prep), encoding="utf-8")
    specs, _ = scorer.capture_specs(
        prep_path, arm_path, scorer.sha256(arm_path.read_bytes()), [case], "r" * 40
    )
    assert specs["sample"] == current

    current["query_pack"] = "changed"
    capture_path.write_text(json.dumps(current), encoding="utf-8")
    prep["arms"][0]["cases"][0]["spec_sha256"] = scorer.sha256(capture_path.read_bytes())
    prep_path.write_text(json.dumps(prep), encoding="utf-8")
    with pytest.raises(scorer.ScoringRefusal, match="undeclared semantic edits"):
        scorer.capture_specs(
            prep_path, arm_path, scorer.sha256(arm_path.read_bytes()), [case], "r" * 40
        )


def test_pinned_official_arb_on_independent_fixed_sample():
    root = Path(
        os.environ.get(
            "ARB_B06_ROOT",
            "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06",
        )
    )
    if not (root / "policy.json").is_file():
        pytest.skip("pinned ARB source fixture is unavailable")
    _, baseline, bcy_curve = scorer.pinned_arb(root)
    sample = {
        "id": "fixed",
        "task_type": "trace2code",
        "query": {"failure_excerpt": "panic"},
        "gold": {"files": ["cause.go", "test.go"]},
    }
    gold = baseline.target_gold_files(sample)
    assert gold == ["cause.go", "test.go"]
    assert scorer.file_metrics(gold, ["noise.go", "test.go", "cause.go"], baseline) == {
        "Recall@20": 1.0,
        "MRR@20": 0.5,
    }

    class FixedCorpus:
        @staticmethod
        def file_text(repo, commit, path):
            return {"noise.go": "noise", "test.go": "test", "cause.go": "cause"}[path]

    detail = {
        "sample_id": "fixed",
        "task_type": "trace2code",
        "repo": "gin-gonic/gin",
        "base_commit": "b" * 40,
        "top_files": ["noise.go", "test.go", "cause.go"],
    }
    packed = bcy_curve.evaluate_sample(detail, gold, FixedCorpus(), (1, 100))["packed"]
    assert packed[1]["bcy"] == 0.0
    assert packed[100]["bcy"] == 1.0


def test_pinned_gin_case_refuses_source_gold_query_and_universe_mismatch():
    root = Path(
        os.environ.get(
            "ARB_B06_ROOT",
            "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06",
        )
    )
    if not (root / "policy.json").is_file():
        pytest.skip("pinned ARB source fixture is unavailable")
    sid = "035c3b77d35f10df09605eff"
    commit = "688a429d19d8c804447bb889d3635e2c31a5564d"
    original_sha = "ac0f8747bbf1add80b03ee9012ecc9c53f41ef5849a6603caccaa005859feec4"
    gold_sha = "4667b95c1dd095b75fab79225d8994402172fe0c85c0c311c1e6e3f372a8844a"
    _, baseline, _ = scorer.pinned_arb(root)
    samples_path = root / "data/benchmark/v2_trace2code/samples.jsonl"
    assert scorer.sha256(samples_path.read_bytes()) == (
        "9d0ff50155fa4f65c1bbb632abc4f1f11393c50fa7b0a9249e06128b368b6266"
    )
    sample = next(
        json.loads(line)
        for line in samples_path.read_text().splitlines()
        if json.loads(line).get("id") == sid
    )
    assert sample["base_commit"] == commit
    assert baseline.target_gold_files(sample) == ["logger.go"]
    assert scorer.sha256(baseline.query_text_for_eval(sample).encode()) == original_sha

    suite_path = root / "suites" / f"{sid}.json"
    pack_path = root / "packs" / f"{sid}.json"
    manifest_path = root / "manifests" / f"{commit}.json"
    suite = json.loads(suite_path.read_bytes())
    pack = json.loads(pack_path.read_bytes())
    manifest = json.loads(manifest_path.read_bytes())
    case = {
        "sample_id": sid,
        "base_commit": commit,
        "query_sha256": original_sha,
        "official_gold_files_sha256": gold_sha,
        "suite": str(suite_path),
        "pack": str(pack_path),
        "manifest": str(manifest_path),
    }
    spec = {
        "top_k": 100,
        "routes": list(scorer.ROUTES),
        "repo": str(root / "checkouts" / commit),
        "manifest": case["manifest"],
        "suite": case["suite"],
        "query_pack": case["pack"],
        "strategies": [{"name": "fixed_window_strict", "overlap_bytes": 256, "window_bytes": 4096}],
    }
    index = {
        row["sample_id"]: row for row in json.loads((root / "samples_index.json").read_bytes())
    }
    args = (case, sample, suite, pack, spec, manifest, index)
    assert scorer.bind_identity(*args, baseline) == ["logger.go"]

    controls = (
        (
            1,
            lambda value: value["query"].update(failure_excerpt="different"),
            "raw query digest mismatch",
        ),
        (1, lambda value: value.update(base_commit="c" * 40), "raw sample base commit mismatch"),
        (
            1,
            lambda value: value["gold"].update(files=["noise.go"]),
            "sample index official gold mismatch",
        ),
        (
            2,
            lambda value: value["tasks"][0].update(query="different"),
            "suite adapted query bytes mismatch",
        ),
        (
            2,
            lambda value: value.update(file_universe=[]),
            "suite differs from ARB all_files universe",
        ),
        (
            3,
            lambda value: value.update(file_universe=[]),
            "pack differs from ARB all_files universe",
        ),
    )
    for index_to_mutate, mutate, expected_error in controls:
        changed = copy.deepcopy(args)
        mutate(changed[index_to_mutate])
        with pytest.raises(scorer.ScoringRefusal, match=expected_error):
            scorer.bind_identity(*changed, baseline)
