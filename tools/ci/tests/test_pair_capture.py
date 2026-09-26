"""Paired common bridge: real native oracle, declared fixture producer only."""

from __future__ import annotations

import io
import json
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import pair_capture as bridge
from evidence import RunStore, sample_evidence
from registry import load_registry

from tools.ci.tests.test_retrieval_benchmark import _pair_stage, _stage_verdict


def registry_fixture():
    return load_registry()


def fixture(tmp_path):
    stage = _pair_stage(tmp_path)
    (stage["stage"] / "verdict.json").write_text(json.dumps(_stage_verdict(stage)))
    return stage


def test_native_owner_replay_and_separate_metric_spaces(tmp_path):
    stage = fixture(tmp_path)
    manifest, verdict = bridge.derive(stage["stage"], stage["repo"])
    assert verdict["states"]["PAIR_VALID"] == "pass"
    payloads = bridge.typed_payloads(stage["stage"], manifest)
    assert len(payloads) == 6
    assert {payload["metric_space"] for payload in payloads.values()} == {"file", "context", "span"}
    assert all(payload["universe_attested"] is False for payload in payloads.values())
    for payload in payloads.values():
        assert [row["query_id"] for row in payload["rows"]] == ["T1", "T2"]
        assert all(row["value"] is None for row in payload["rows"] if row["state"] == "unsupported")


def test_owned_temporary_alias_is_canonicalized(tmp_path, monkeypatch):
    import tempfile

    stage = fixture(tmp_path)
    alias = tmp_path / "temporary-alias"
    target = tmp_path / "temporary-real"
    target.mkdir()
    alias.symlink_to(target, target_is_directory=True)
    monkeypatch.setattr(tempfile, "tempdir", str(alias))
    _, verdict = bridge.derive(stage["stage"], stage["repo"])
    assert verdict["states"]["PAIR_VALID"] == "pass"


def test_untrusted_native_root_alias_is_not_canonicalized(tmp_path):
    stage = fixture(tmp_path)
    alias = tmp_path / "native-alias"
    alias.symlink_to(stage["stage"], target_is_directory=True)
    with pytest.raises((ValueError, OSError)):
        bridge.derive(alias, stage["repo"])


def test_live_registration_names_the_actual_manifest_schema():
    registry = load_registry()
    schema = json.loads(
        (Path(bridge.owner.__file__).parent / "run-manifest.schema.json").read_text()
    )
    version = schema["properties"]["manifest_version"]["const"]
    assert version == bridge.owner.MANIFEST_VERSION
    assert (
        registry["families"][bridge.FAMILY]["native_schema"] == f"retrieval-run-manifest:v{version}"
    )
    bridge.require_registration(registry)


def test_cli_pair_spec_is_required_and_other_controls_refuse(tmp_path, capsys):
    import benchctl

    root = tmp_path / "evidence"
    assert benchctl.main(["run", bridge.PROFILE, "--evidence-root", str(root)]) == 2
    assert "requires --pair-spec" in capsys.readouterr().err
    assert benchctl.main(["run", "micro", "--pair-spec", str(tmp_path / "spec")]) == 2
    assert "applies only to retrieval-diagnostic" in capsys.readouterr().err
    assert (
        benchctl.main(
            [
                "run",
                bridge.PROFILE,
                "--evidence-root",
                str(root),
                "--pair-spec",
                str(tmp_path / "spec"),
                "--cold-samples",
                "20",
            ]
        )
        == 2
    )
    assert "unrelated producer controls" in capsys.readouterr().err
    assert not root.exists()


def test_bundle_replay_restores_real_commit_and_executable_mode(tmp_path):
    stage = fixture(tmp_path)
    source = stage["repo"]
    (source / "a.txt").chmod(0o755)
    subprocess.run(["git", "-C", str(source), "add", "a.txt"], check=True)
    subprocess.run(["git", "-C", str(source), "commit", "-qm", "executable fixture"], check=True)
    commit = (
        subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"]).decode().strip()
    )
    clone = tmp_path / "clone"
    bridge.clone_corpus(source, clone, commit, 60)
    bundle = tmp_path / "corpus.bundle"
    bridge.execute(
        ["git", "-C", str(clone), "bundle", "create", str(bundle), "HEAD"],
        cwd=bridge.ROOT,
        env={**os.environ, **bridge.GIT_ENV},
        timeout=60,
    )
    (source / "a.txt").write_bytes(b"mutated original corpus")
    restored = tmp_path / "restored"
    bridge.restore_corpus(bundle, restored, 60)
    assert bridge.evaluator.verify_repo(restored, commit) == restored.resolve()
    assert (restored / "a.txt").stat().st_mode & 0o111
    assert (restored / "a.txt").read_bytes() != (source / "a.txt").read_bytes()


def test_native_report_tamper_refuses_before_typed_scoring(tmp_path):
    stage = fixture(tmp_path)
    ref = stage["manifest"]["artifacts"]["reports"][0]
    path = stage["stage"] / ref
    report = json.loads(path.read_text())
    report["per_query"][0]["file_recall_at_10"] = 0.5
    path.write_text(json.dumps(report))
    with pytest.raises(ValueError, match="verdict differs|PAIR_VALID"):
        bridge.derive(stage["stage"], stage["repo"])


def test_native_raw_tree_refuses_links_and_special_files(tmp_path):
    root = tmp_path / "native"
    root.mkdir()
    (root / "record").write_bytes(b"raw")
    (root / "alias").symlink_to(root / "record")
    with pytest.raises(ValueError):
        bridge.tree_bytes(root)
    (root / "alias").unlink()
    os.mkfifo(root / "fifo")
    with pytest.raises(ValueError):
        bridge.tree_bytes(root)


@pytest.mark.parametrize(
    "changes", [{"scorer": "none"}, {"validator": "retrieval-proof"}, {"native_schema": "wrong"}]
)
def test_pair_registration_drift_refuses(changes):
    registry = registry_fixture()
    registry["families"][bridge.FAMILY].update(changes)
    with pytest.raises(ValueError, match="registration differs"):
        bridge.require_registration(registry)


def test_capture_complete_profile_and_replay_with_original_corpus_changed(tmp_path, monkeypatch):
    import benchctl

    calls = {"restore": 0, "derive": 0}
    for label, name in (("restore", "restore_corpus"), ("derive", "derive")):
        original = getattr(bridge, name)

        def counted(*args, _label=label, _original=original, **kwargs):
            calls[_label] += 1
            return _original(*args, **kwargs)

        monkeypatch.setattr(bridge, name, counted)

    stage = fixture(tmp_path)
    output = tmp_path / "pair-output"
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"g0-seed-searchd")
    semble_python = tmp_path / "semble-python"
    semble_python.write_bytes(b"g0-seed-interp")
    spec = {
        **stage["spec"],
        "output_root": str(output),
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": bridge.owner.sha_file(searchd),
        "top_k": 10,
        "strategies": [{"name": "whole_file"}],
        "routes": ["lexical"],
        "semble_python": str(semble_python),
        "semble_lockfile_sha256": bridge.owner.sha_file(Path(stage["spec"]["semble_lockfile"])),
    }
    spec_path = tmp_path / "spec.json"
    spec_path.write_text(json.dumps(spec))
    source = sample_evidence()["source"]
    source["revision"] = stage["manifest"]["provenance"]["quanta"]["source_sha"]
    source["dirty"] = False
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda _: source["revision"])
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(bridge, "source_identity", lambda *_: source)
    frozen_owner_digests = bridge.owner_digests()
    monkeypatch.setattr(bridge, "owner_digests", lambda: frozen_owner_digests)
    real_execute = bridge.execute

    def fixture_producer(argv, **kwargs):
        if argv[1:4] == ["-m", bridge.MODULE, "pair"]:
            shutil.copytree(stage["stage"], output)
            command = {**sample_evidence()["command"], "argv": argv, "cwd": str(bridge.ROOT)}
            return b"declared fixture producer", b"", command
        return real_execute(argv, **kwargs)

    monkeypatch.setattr(bridge, "execute", fixture_producer)
    root = tmp_path / "evidence"
    assert (
        benchctl.main(
            ["run", bridge.PROFILE, "--evidence-root", str(root), "--pair-spec", str(spec_path)]
        )
        == 0
    )
    document = bridge.validate(bridge.ROOT, root, registry_fixture())
    assert len(document["runs"]) == 6
    (stage["repo"] / "a.txt").write_bytes(b"original source no longer authoritative")
    shutil.rmtree(output)
    shutil.rmtree(root / "work")
    assert bridge.validate(bridge.ROOT, root, registry_fixture()) == document
    assert benchctl.main(["validate", bridge.PROFILE, "--evidence-root", str(root)]) == 0
    assert benchctl.main(["summarize", bridge.PROFILE, "--evidence-root", str(root)]) == 0
    assert (
        benchctl.main(["replay", document["runs"][0]["run_id"], "--evidence-root", str(root)]) == 0
    )
    store = RunStore(root)
    evidence = store.load(document["runs"][0]["run_id"])
    evidence["payload"]["rows"][0]["value"] = 0.12345
    with pytest.raises(ValueError, match="typed metrics"):
        bridge.replay_run(store, evidence)
    # Publication and three independent validations still derive all six cases.
    # Two per-run replays restore separately; capture also derives once.
    # The summary command advertises presence without claiming validation.
    assert calls == {"restore": 6, "derive": 27}

    evidence = store.load(document["runs"][0]["run_id"])
    original_derive = bridge.derive

    def mutating_owner(native, corpus):
        result = original_derive(native, corpus)
        (corpus / "a.txt").write_bytes(b"owner mutated its supposedly read-only input")
        return result

    monkeypatch.setattr(bridge, "derive", mutating_owner)
    with pytest.raises(bridge.EvidenceError, match="workspace changed"):
        bridge.replay_run(store, evidence)


@pytest.mark.parametrize(
    "names,compressed",
    [
        (["../escape"], False),
        (["/absolute"], False),
        (["a", "a"], False),
        (["z", "a"], False),
        (["a"], True),
    ],
)
def test_native_archive_refuses_escape_duplicate_reorder_and_compression(
    tmp_path, names, compressed
):
    buffer = io.BytesIO()
    with zipfile.ZipFile(
        buffer, "w", compression=zipfile.ZIP_DEFLATED if compressed else zipfile.ZIP_STORED
    ) as archive:
        for name in names:
            archive.writestr(name, b"raw")
    with pytest.raises(ValueError):
        bridge.unpack_native(buffer.getvalue(), tmp_path / "restored")
    assert not (tmp_path / "escape").exists()


@pytest.mark.parametrize("kind", ["not_zip", "truncated", "crc", "encrypted"])
def test_native_archive_malformed_inputs_are_domain_failures(tmp_path, kind):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.writestr("record", b"native-payload")
    data = buffer.getvalue()
    if kind == "not_zip":
        data = b"not a ZIP"
    elif kind == "truncated":
        data = data[:20]
    elif kind == "crc":
        data = data.replace(b"native-payload", b"broken-payload", 1)
    else:
        damaged = bytearray(data)
        damaged[6] |= 1
        central = data.index(b"PK\x01\x02")
        damaged[central + 8] |= 1
        data = bytes(damaged)
    with pytest.raises(bridge.EvidenceError, match="archive|unsafe"):
        bridge.unpack_native(data, tmp_path / "restored")


@pytest.mark.parametrize("nested_pair", [(0, 1), (0, 2), (1, 2)])
@pytest.mark.parametrize("reverse", [False, True])
def test_capture_roots_refuse_overlap_before_mutation(tmp_path, nested_pair, reverse):
    paths = [tmp_path / "evidence", tmp_path / "output", tmp_path / "corpus"]
    parent, child = nested_pair[::-1] if reverse else nested_pair
    paths[child] = paths[parent] / "nested"
    with pytest.raises(bridge.EvidenceError, match="roots overlap"):
        bridge.require_disjoint_paths(*paths)
    assert not any(path.exists() for path in paths)


def test_capture_roots_allow_disjoint_external_roots(tmp_path):
    bridge.require_disjoint_paths(*(tmp_path / name for name in ("evidence", "output", "corpus")))
