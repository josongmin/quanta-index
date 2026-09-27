"""Lexical common custody proofs use fixture observations, never fake searches."""

import json
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import lexical_capture as capture
from evidence import RunStore, sample_evidence
from registry import load_registry

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.query_plan import execution_profile
from tools.ci.tests.test_lexical_file_comparison import fixture_inputs


@pytest.fixture(scope="session")
def lexical_release_seed(tmp_path_factory):
    """Share immutable corpus construction; each consumer gets independent bytes."""
    tmp_path = tmp_path_factory.mktemp("lexical-release-seed")
    import corpus_release as corpus

    checkouts = tmp_path / "checkouts"
    repository = checkouts / "fixture"
    repository.mkdir(parents=True)
    corpus.git(repository, "init", "-q")
    corpus.git(repository, "config", "user.name", "Fixture")
    corpus.git(repository, "config", "user.email", "fixture@localhost")
    (repository / "LICENSE").write_text("Fixture license, not approval\n")
    (repository / "src").mkdir()
    for number in range(20):
        (repository / "src" / f"{number}.go").write_text(f"func symbol_{number}() {{}}\n")
    corpus.git(repository, "add", ".")
    corpus.git(repository, "commit", "-qm", "fixed fixture corpus")
    commit = corpus.git(repository, "rev-parse", "HEAD").decode().strip()
    recipe = {
        "source_revision": "fixture recipe, not product qualification",
        "repositories": [
            {
                "name": "fixture",
                "language": "go",
                "url": "https://example.invalid/fixture.git",
                "revision": commit,
                "benchmark_root": "src",
                "upstream_semble_benchmark_overlap": False,
            }
        ],
    }
    recipe_path = tmp_path / "recipe.json"
    recipe_path.write_text(json.dumps(recipe))
    release_root = tmp_path / "corpus-release"
    corpus.create(recipe_path, checkouts, release_root)
    return release_root


def inputs(tmp_path, lexical_release_seed):
    _, _, suite, pack = fixture_inputs(tmp_path)
    release_root = tmp_path / "corpus-release"
    shutil.copytree(lexical_release_seed, release_root, symlinks=True)
    release = json.loads((release_root / "release.json").read_bytes())
    commit = release["repositories"][0]["recipe"]["revision"]
    view = release["repositories"][0]["views"]["code_only"]
    manifest = json.loads((release_root / view["manifest"]).read_bytes())
    for payload in (suite, pack):
        payload["repository_commit"] = commit
        payload["file_universe"] = manifest["files"]
        payload["file_universe_digest"] = view["file_universe_digest"][7:]
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    native = {
        "semble_profile": "lexical-only",
        "rerank_applied": False,
        "lane_call_counts": {"bm25": 20, "semantic": 0, "encode": 0},
        "execution_events": [{"lane_entry_counts": {"bm25": 1, "semantic": 0}} for _ in range(20)],
    }
    rows = [
        {
            "route": route,
            "task_id": task["task_id"],
            "query_latency_ms": 1.0,
            "file_recall_at_10": 1.0,
            "file_hit_at_10": True,
            "status": "success",
        }
        for task in pack["tasks"]
        for route in ("lexical", "semble-hybrid")
    ]
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "sample_count": 20,
        "rank_metrics": {
            "routes": {
                route: {
                    "sample_count": 20,
                    "chunk": {"file_recall_at_10": 1.0},
                    "mean_query_latency_ms": 1.0,
                }
                for route in ("lexical", "semble-hybrid")
            }
        },
        "per_query": rows,
    }
    content = {
        "suite": suite,
        "query_pack": pack,
        "pair_report": report,
        "pair_lock": {
            "execution_profiles": {
                "quanta": execution_profile("native"),
                "semble": {
                    "profile_id": "semble-lexical-only-v1",
                    "mode": "lexical-only",
                    "alpha": None,
                    "rerank": "not_applicable",
                },
            }
        },
        "semble_native": native,
        "pair_verdict": {"states": {"PAIR_VALID": "pass"}},
    }
    paths = {}
    for role, value in content.items():
        paths[role] = tmp_path / (role + ".json")
        paths[role].write_text(json.dumps(value))
    for product in capture.owner.PRODUCTS:
        product_rows = [
            {
                "lane": "symbol_only",
                "task_id": task["task_id"],
                "submitted_query": task["query"],
                "gold_paths": [suite["tasks"][i]["gold"][0]["path"]],
                "http_status": 200,
                "exit_code": 0,
                "field": "full",
                "error": None,
                "file_paths_top_10": [suite["tasks"][i]["gold"][0]["path"]],
                "paths": [suite["tasks"][i]["gold"][0]["path"]],
                "file_hit_at_10": True,
                "elapsed_ms": 1.0,
            }
            for i, task in enumerate(pack["tasks"])
        ]
        role = product + "_rows"
        paths[role] = tmp_path / (role + ".jsonl")
        paths[role].write_text("\n".join(json.dumps(row) for row in product_rows) + "\n")
    spec = tmp_path / "spec.json"
    spec.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "corpus": {
                    "release_path": str(release_root),
                    "release_digest": release["digest"],
                    "repository": "fixture",
                    "view": "code_only",
                },
                "inputs": {role: str(path) for role, path in paths.items()},
            }
        )
    )
    return spec, paths


@pytest.fixture
def synthetic_admission(monkeypatch):
    import benchctl

    source = sample_evidence()["source"]
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(capture, "source_identity", lambda *_: source)
    return source


@pytest.mark.parametrize(
    "relationship", ["equal", "evidence-under-corpus", "corpus-under-evidence"]
)
def test_capture_overlap_refuses_before_epoch_writes(tmp_path, monkeypatch, relationship):
    release = tmp_path / "corpus"
    root = (
        release
        if relationship == "equal"
        else release / "evidence"
        if relationship == "evidence-under-corpus"
        else tmp_path
    )
    spec = tmp_path / "spec.json"
    spec.write_text("{}")
    monkeypatch.setattr(
        capture.corpus_binding, "read_spec", lambda *_: ({"release_path": str(release)}, {})
    )
    with pytest.raises(ValueError, match="roots overlap"):
        capture.capture(
            capture.ROOT,
            root,
            load_registry(capture.ROOT / "tools/benchmark/registry.toml"),
            spec,
            1,
        )
    assert not (root / "work").exists()
    assert not (root / "failures").exists()
    assert not release.exists()


def test_complete_profile_executes_owner_and_replays_frozen_observations(
    tmp_path, lexical_release_seed, synthetic_admission, capsys
):
    import benchctl

    spec, paths = inputs(tmp_path, lexical_release_seed)
    registry = load_registry()
    root = tmp_path / "evidence"
    document = capture.capture(capture.ROOT, root, registry, spec, 60)
    assert document["expected_cases"] == {capture.FAMILY: list(capture.PRODUCTS)}
    assert len(document["runs"]) == 5
    assert capture.validate(capture.ROOT, root, registry) == document
    store = RunStore(root)
    for row in document["runs"]:
        evidence = store.load(row["run_id"])
        assert evidence["payload"]["metric_space"] == "file"
        assert evidence["payload"]["judgments"] == "mechanically_labeled"
        assert evidence["payload"]["universe_attested"] is False
        assert len(evidence["payload"]["rows"]) == 20
        assert evidence["verdict"]["scope"] == "diagnostic"
        assert "qualified" not in evidence["verdict"]
    # Mutable source recordings are no longer authority after immutable freeze.
    paths["cs_rows"].write_bytes(b"changed original")
    spec.write_bytes(b"changed original spec")
    assert capture.validate(capture.ROOT, root, registry) == document
    assert benchctl.replay_command(capture.ROOT, document["runs"][0]["run_id"], root) == 0
    assert json.loads(capsys.readouterr().out)["artifact_oracle"] == "pass"
    bad = store.load(document["runs"][0]["run_id"])
    bad["payload"]["rows"][0]["value"] = 0.5
    with pytest.raises(ValueError, match="typed rows"):
        capture.replay_run(store, bad)


def test_incomplete_input_cannot_replace_previous_profile(
    tmp_path, synthetic_admission, lexical_release_seed
):
    spec, paths = inputs(tmp_path, lexical_release_seed)
    registry = load_registry()
    root = tmp_path / "evidence"
    capture.capture(capture.ROOT, root, registry, spec, 60)
    pointer = root / "profiles" / (capture.PROFILE + ".json")
    before = pointer.read_bytes()
    paths["cs_rows"].write_bytes(b"{}\n")
    with pytest.raises(ValueError, match="producer failed"):
        capture.capture(capture.ROOT, root, registry, spec, 60)
    assert pointer.read_bytes() == before
    assert len(list((root / "captures").iterdir())) == 1


@pytest.mark.parametrize(
    "changes",
    [
        {"validator": "retrieval-pair"},
        {"scorer": "none"},
        {"native_schema": "retrieval-run-manifest:v5"},
    ],
)
def test_registration_drift_refuses(changes):
    registry = load_registry()
    registry["families"][capture.FAMILY].update(changes)
    with pytest.raises(ValueError, match="registration differs"):
        capture.require_registration(registry)


def test_cli_missing_spec_and_mixed_controls_refuse_before_producer(tmp_path, capsys):
    import benchctl

    root = tmp_path / "evidence"
    assert benchctl.main(["run", capture.PROFILE, "--evidence-root", str(root)]) == 2
    assert "requires --lexical-spec" in capsys.readouterr().err
    assert benchctl.main(["run", "micro", "--lexical-spec", str(tmp_path / "spec")]) == 2
    assert "applies only to lexical-diagnostic" in capsys.readouterr().err
    assert not root.exists()


def test_corrupt_frozen_input_is_not_a_zero_score(
    tmp_path, synthetic_admission, lexical_release_seed
):
    spec, _ = inputs(tmp_path, lexical_release_seed)
    root = tmp_path / "evidence"
    document = capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    store = RunStore(root)
    run = document["runs"][0]["run_id"]
    (store.run_dir(run) / "raw" / "input-sourcegraph_rows").write_bytes(b"truncated")
    with pytest.raises(ValueError):
        capture.validate(capture.ROOT, root, load_registry())


@pytest.mark.parametrize("artifact", ["corpus-release.zip", "corpus-binding.json"])
def test_missing_corpus_custody_artifact_cannot_replay(
    tmp_path, synthetic_admission, artifact, lexical_release_seed
):
    spec, _ = inputs(tmp_path, lexical_release_seed)
    root = tmp_path / "evidence"
    document = capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    store = RunStore(root)
    evidence = store.load(document["runs"][0]["run_id"])
    (store.run_dir(evidence["run_id"]) / "raw" / artifact).unlink()
    with pytest.raises((ValueError, OSError)):
        capture.replay_run(store, evidence)


def test_lexical_adapter_streams_observation_freeze(
    tmp_path, lexical_release_seed, synthetic_admission, monkeypatch
):
    spec, paths = inputs(tmp_path, lexical_release_seed)
    original = Path.read_bytes

    def no_observation_bytes(path):
        assert path not in {paths[role] for role in paths if role.endswith("_rows")}
        assert not (path.name.startswith("input-") and path.name.endswith("_rows"))
        return original(path)

    monkeypatch.setattr(Path, "read_bytes", no_observation_bytes)
    root = tmp_path / "evidence"
    document = capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    assert len(document["runs"]) == 5
    assert capture.validate(capture.ROOT, root, load_registry()) == document


def test_lexical_frozen_input_mutation_cannot_publish(
    tmp_path, lexical_release_seed, synthetic_admission, monkeypatch
):
    spec, _ = inputs(tmp_path, lexical_release_seed)
    execute = capture.execute

    def mutate(argv, **kwargs):
        result = execute(argv, **kwargs)
        frozen = Path(argv[argv.index("--spec") + 1]).parent / "input-cs_rows"
        frozen.write_bytes(frozen.read_bytes() + b" ")
        return result

    monkeypatch.setattr(capture, "execute", mutate)
    root = tmp_path / "evidence"
    with pytest.raises(ValueError):
        capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    assert not (root / "profiles" / f"{capture.PROFILE}.json").exists()


def test_legacy_unbound_spec_refuses_before_publication(tmp_path):
    spec = tmp_path / "legacy.json"
    spec.write_text(json.dumps({"schema_version": 1}))
    root = tmp_path / "evidence"
    with pytest.raises(ValueError, match="schema_version 2"):
        capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    assert not root.exists()


def test_evidence_release_overlap_refuses_before_mutation(tmp_path, lexical_release_seed):
    spec, _ = inputs(tmp_path, lexical_release_seed)
    root = Path(json.loads(spec.read_bytes())["corpus"]["release_path"])
    before = (root / "release.json").read_bytes()
    with pytest.raises(ValueError, match="roots overlap"):
        capture.capture(capture.ROOT, root, load_registry(), spec, 60)
    assert (root / "release.json").read_bytes() == before
    assert not (root / "profiles").exists()


@pytest.mark.parametrize("status,state", [("timeout", "timeout"), ("unavailable", "unsupported")])
def test_typed_retrieval_retains_non_scored_states_without_zero(
    tmp_path, status, state, lexical_release_seed
):
    _, paths = inputs(tmp_path, lexical_release_seed)
    summary = capture.owner.evaluate_capture(paths)
    summary["pair"]["routes"]["quanta_lexical"]["per_query"][0]["status"] = status
    summary["pair"]["routes"]["quanta_lexical"]["per_query"][0]["file_recall_at_10"] = 0.0
    result = capture.payloads(summary, capture.owner._read(paths["query_pack"]))
    row = result["quanta_lexical"]["rows"][0]
    assert row["state"] == state and row["value"] is None


@pytest.mark.parametrize(
    "change",
    [
        {"status": "error"},
        {"status": None},
        {"file_recall_at_10": True},
        {"file_recall_at_10": 1.1},
    ],
)
def test_malformed_or_unrepresentable_rows_refuse(tmp_path, change, lexical_release_seed):
    _, paths = inputs(tmp_path, lexical_release_seed)
    summary = capture.owner.evaluate_capture(paths)
    summary["pair"]["routes"]["quanta_lexical"]["per_query"][0].update(change)
    with pytest.raises(ValueError):
        capture.payloads(summary, capture.owner._read(paths["query_pack"]))
