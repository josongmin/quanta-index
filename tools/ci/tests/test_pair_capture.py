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
from evidence import RawFile, RunStore, sample_evidence, write_raw_file
from registry import load_registry

from tools.ci.tests.test_retrieval_benchmark import _pair_stage, _stage_verdict


def registry_fixture():
    return load_registry()


def fixture(tmp_path):
    stage = _pair_stage(tmp_path)
    (stage["stage"] / "verdict.json").write_text(json.dumps(_stage_verdict(stage)))
    return stage


@pytest.mark.parametrize(
    ("mode", "route"),
    [
        ("lexical-only", "semble-lexical-only"),
        ("semantic-only", "semble-semantic-only"),
        ("hybrid-no-rerank", "semble-hybrid"),
        ("native-default", "semble-hybrid"),
    ],
)
def test_semble_route_label_is_bound_to_execution_mode(mode, route):
    spec = {
        "execution_profiles": {"semble": {"mode": mode}},
        "routes": ["lexical", "semantic", "hybrid"],
        "candidate_route": "lexical",
        "baseline_route": route,
        "semble_route": route,
    }
    bridge.owner._validate_semble_route_binding(spec)

    wrong = {**spec, "baseline_route": "semble-hybrid"}
    if route != "semble-hybrid":
        with pytest.raises(bridge.owner.RunError, match="baseline_route must match"):
            bridge.owner._validate_semble_route_binding(wrong)


def test_candidate_route_must_be_enabled_for_quanta():
    spec = {
        "execution_profiles": {"semble": {"mode": "lexical-only"}},
        "routes": ["lexical"],
        "candidate_route": "hybrid",
    }
    with pytest.raises(bridge.owner.RunError, match="configured Quanta route"):
        bridge.owner._validate_semble_route_binding(spec)


def test_pair_spec_loader_rejects_route_profile_mismatch(tmp_path):
    spec = {
        "spec_version": 2,
        "repo": "/external/repo",
        "manifest": "/external/manifest.json",
        "suite": "/external/suite.json",
        "query_pack": "/external/pack.json",
        "execution_profiles": {
            "quanta": bridge.owner.qp.execution_profile("native"),
            "semble": {
                "profile_id": "semble-lexical-only-v1",
                "mode": "lexical-only",
                "alpha": None,
                "rerank": "not_applicable",
            },
        },
        "top_k": 10,
        "output_root": "/external/pair",
        "runner_binary": "/external/runner",
        "strategies": [{"name": "fixed_window_strict"}],
        "searchd_binary": "/external/searchd",
        "searchd_expected_sha256": "a" * 64,
        "routes": ["lexical"],
        "candidate_route": "lexical",
        "baseline_route": "semble-lexical-only",
        "semble_route": "semble-lexical-only",
    }
    path = tmp_path / "pair.json"
    path.write_text(json.dumps(spec))
    assert bridge.owner.load_spec(path)["baseline_route"] == "semble-lexical-only"

    spec["baseline_route"] = "semble-hybrid"
    spec["semble_route"] = "semble-hybrid"
    path.write_text(json.dumps(spec))
    with pytest.raises(bridge.owner.RunError, match="spec.semble_route must be"):
        bridge.owner.load_spec(path)


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
        log_dir=tmp_path / "bundle-execution",
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
        bridge.tree_files(root)
    (root / "alias").unlink()
    os.mkfifo(root / "fifo")
    with pytest.raises(ValueError):
        bridge.tree_files(root)


def test_native_archive_identity_is_time_independent_and_roundtrips(tmp_path, monkeypatch):
    root = tmp_path / "native"
    (root / "nested").mkdir(parents=True)
    expected = {"empty": b"", "nested/한글": b"independent payload\x00\xff"}
    for name, data in expected.items():
        (root / name).write_bytes(data)
    refs = []
    for year in (2025, 2026):
        monkeypatch.setattr(
            zipfile.time, "localtime", lambda *args, year=year: (year, 1, 1, 0, 0, 0, 0, 1, 0)
        )
        refs.append(bridge.pack_native(root, tmp_path / f"{year}.zip"))
    assert (refs[0].sha256, refs[0].size) == (refs[1].sha256, refs[1].size)
    with zipfile.ZipFile(refs[0].path) as archive:
        assert archive.namelist() == sorted(expected)
        assert {entry.filename: archive.read(entry) for entry in archive.infolist()} == expected
        assert all(entry.date_time == (1980, 1, 1, 0, 0, 0) for entry in archive.infolist())
    bridge.unpack_native(refs[0], tmp_path / "restored")
    assert {name: (tmp_path / "restored" / name).read_bytes() for name in expected} == expected


def test_archive_limit_boundaries_and_central_directory_preflight(tmp_path, monkeypatch):
    import raw_archive

    source = write_raw_file(tmp_path / "source", [b"raw"])
    files = {"record": source}
    first = raw_archive.pack(files, tmp_path / "first.zip", limits=raw_archive.ArchiveLimits(4096))
    exact = raw_archive.ArchiveLimits(first.size, max_entries=1)
    second = raw_archive.pack(files, tmp_path / "exact.zip", limits=exact)
    assert first.sha256 == second.sha256
    raw_archive.unpack(second, tmp_path / "good", limits=exact)
    with pytest.raises(bridge.EvidenceError, match="limit"):
        raw_archive.pack(
            files, tmp_path / "oversize.zip", limits=raw_archive.ArchiveLimits(first.size - 1)
        )
    with pytest.raises(bridge.EvidenceError, match="limit"):
        raw_archive.pack({"a": source, "b": source}, tmp_path / "count.zip", limits=exact)

    # A malicious central directory must be refused before ZipFile allocates it.
    def forbidden(*args, **kwargs):
        raise AssertionError("ZipFile was constructed before metadata admission")

    monkeypatch.setattr(raw_archive.zipfile, "ZipFile", forbidden)
    with pytest.raises(bridge.EvidenceError, match="limit"):
        raw_archive.unpack(
            first, tmp_path / "bad", limits=raw_archive.ArchiveLimits(4096, max_directory_bytes=1)
        )


@pytest.mark.parametrize(
    "mutation", ["ancestor", "existing", "hardlink", "changed", "forged", "prefix"]
)
def test_streamed_archive_refuses_output_aliases_and_input_changes(tmp_path, mutation):
    import raw_archive

    original = write_raw_file(tmp_path / "source", [b"raw"])
    files = {"nested/record": original}
    archive = raw_archive.pack(files, tmp_path / "raw.zip", limits=raw_archive.ArchiveLimits(4096))
    output = tmp_path / "output"
    output.mkdir()
    sentinel = tmp_path / "sentinel"
    sentinel.write_bytes(b"must not change")
    if mutation == "ancestor":
        (output / "nested").symlink_to(tmp_path, target_is_directory=True)
    elif mutation in {"existing", "hardlink"}:
        (output / "nested").mkdir()
        if mutation == "existing":
            (output / "nested/record").write_bytes(b"existing")
        else:
            os.link(sentinel, output / "nested/record")
    elif mutation == "changed":
        archive.path.write_bytes(archive.path.read_bytes() + b"changed")
    elif mutation == "forged":
        archive = RawFile(archive.path, "sha256:" + "0" * 64, archive.size)
    else:
        with pytest.raises(bridge.EvidenceError, match="overlap"):
            raw_archive.pack(
                {"a": original, "a/b": original},
                tmp_path / "prefix.zip",
                limits=raw_archive.ArchiveLimits(4096),
            )
        return
    with pytest.raises(bridge.EvidenceError):
        raw_archive.unpack(archive, output, limits=raw_archive.ArchiveLimits(4096))
    assert sentinel.read_bytes() == b"must not change"


def test_archive_streams_payload_and_verifies_seekable_mutation(tmp_path, monkeypatch):
    import raw_archive

    original = write_raw_file(tmp_path / "source", [b"x" * 65536] * 5)
    archive = raw_archive.pack(
        {"record": original}, tmp_path / "raw.zip", limits=raw_archive.ArchiveLimits(1024**2)
    )
    actual_read = zipfile.ZipExtFile.read
    sizes = []

    def bounded(self, size=-1):
        assert 0 <= size <= 65536
        sizes.append(size)
        return actual_read(self, size)

    monkeypatch.setattr(zipfile.ZipExtFile, "read", bounded)
    raw_archive.unpack(archive, tmp_path / "output", limits=raw_archive.ArchiveLimits(1024**2))
    assert len(sizes) >= 5
    assert RawFile.capture(tmp_path / "output/record").sha256 == original.sha256

    def mutate(handle):
        with archive.path.open("ab") as writer:
            writer.write(b"changed after first full commitment")

    with pytest.raises(bridge.EvidenceError, match="commitment|changed"):
        archive.consume_seekable(mutate)


def test_archive_zip64_roundtrip_with_small_forced_threshold(tmp_path, monkeypatch):
    import raw_archive

    monkeypatch.setattr(zipfile, "ZIP64_LIMIT", 32)
    ref = write_raw_file(tmp_path / "source", [b"z" * 100])
    archive = raw_archive.pack(
        {"record": ref}, tmp_path / "zip64.zip", limits=raw_archive.ArchiveLimits(4096)
    )
    raw_archive.unpack(archive, tmp_path / "output", limits=raw_archive.ArchiveLimits(4096))
    assert (tmp_path / "output/record").read_bytes() == b"z" * 100


def test_forged_archive_count_refuses_before_zip_metadata_allocation(tmp_path, monkeypatch):
    import struct

    import raw_archive

    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("a", b"first")
        archive.writestr("b", b"second")
    data = bytearray(buffer.getvalue())
    end = data.rfind(b"PK\x05\x06")
    struct.pack_into("<HH", data, end + 8, 1, 1)
    ref = write_raw_file(tmp_path / "forged.zip", [data])

    def forbidden(*args, **kwargs):
        raise AssertionError("forged count reached metadata allocation")

    monkeypatch.setattr(raw_archive.zipfile, "ZipFile", forbidden)
    with pytest.raises(bridge.EvidenceError, match="count"):
        raw_archive.unpack(ref, tmp_path / "output", limits=raw_archive.ArchiveLimits(4096))


@pytest.mark.parametrize("mutation", ["nul", "volume", "offset", "central_size"])
def test_archive_rejects_aliased_or_forged_zip_metadata(tmp_path, mutation):
    import struct

    import raw_archive

    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("a0b", b"raw")
    data = bytearray(buffer.getvalue())
    central, end = data.index(b"PK\x01\x02"), data.rfind(b"PK\x05\x06")
    if mutation == "nul":
        data = data.replace(b"a0b", b"a\x00b")
    elif mutation == "volume":
        struct.pack_into("<H", data, central + 34, 1)
    elif mutation == "offset":
        struct.pack_into("<I", data, end + 16, 1)
    else:
        struct.pack_into("<I", data, end + 12, 0xFFFFFFFF)
    ref = write_raw_file(tmp_path / "bad.zip", [data])
    with pytest.raises(bridge.EvidenceError, match="archive"):
        raw_archive.unpack(ref, tmp_path / "output", limits=raw_archive.ArchiveLimits(4096))
    assert not (tmp_path / "output").exists()


@pytest.mark.parametrize("operation", ["pack", "unpack"])
def test_archive_payload_peak_rss_is_bounded(tmp_path, operation, record_property):
    import hashlib

    # One fresh interpreter per size; fixture allocation is outside that process.
    peaks = []
    for size in (8 * 1024**2, 128 * 1024**2):
        source = tmp_path / f"input-{size}"
        source.mkdir()
        path = source / "record"
        block, expected = b"q" * 65536, hashlib.sha256()
        with path.open("wb") as handle:
            for _ in range(size // len(block)):
                handle.write(block)
                expected.update(block)
        archive = tmp_path / f"archive-{size}.zip"
        if operation == "unpack":
            # Independent stdlib fixture, not the streaming writer under test.
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as held:
                held.write(path, arcname="record")
        script = """
import json, resource, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from evidence import RawFile
from raw_archive import ArchiveLimits, pack, unpack
source, archive, output = map(Path, sys.argv[3:6])
limits = ArchiveLimits(1024**3)
if sys.argv[2] == "pack":
    ref = pack({"record": RawFile.capture(source / "record")}, archive, limits=limits)
    digest = ref.sha256
else:
    unpack(RawFile.capture(archive), output, limits=limits)
    ref = RawFile.capture(output / "record")
    digest = ref.sha256
peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == "darwin" else 1024)
print(json.dumps({"peak": peak, "digest": digest, "size": ref.size}))
"""
        result = subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                script,
                str(Path(bridge.__file__).parent),
                operation,
                str(source),
                str(archive),
                str(tmp_path / f"output-{size}"),
            ],
            capture_output=True,
            text=True,
            timeout=60,
            check=True,
        )
        measured = json.loads(result.stdout)
        if operation == "pack":
            with zipfile.ZipFile(archive) as held, held.open("record") as entry:
                observed = hashlib.sha256()
                for chunk in iter(lambda: entry.read(65536), b""):
                    observed.update(chunk)
                assert observed.hexdigest() == expected.hexdigest()
                assert held.getinfo("record").file_size == size
        else:
            assert measured["digest"] == "sha256:" + expected.hexdigest()
            assert measured["size"] == size
        assert measured["peak"] > 0
        peaks.append(measured["peak"])
        record_property(f"archive_{operation}_{size}_peak_bytes", measured["peak"])
    assert peaks[1] - peaks[0] < 32 * 1024**2


def test_archive_many_entry_inventory_and_limit(tmp_path):
    import raw_archive

    source = write_raw_file(tmp_path / "source", [b"fixed"])
    files = {f"entry-{index:05}": source for index in range(2000)}
    limit = raw_archive.ArchiveLimits(1024**2, max_entries=len(files))
    archive = raw_archive.pack(files, tmp_path / "many.zip", limits=limit)
    raw_archive.unpack(archive, tmp_path / "output", limits=limit)
    assert {path.name for path in (tmp_path / "output").iterdir()} == set(files)
    assert all((tmp_path / "output" / name).read_bytes() == b"fixed" for name in files)
    with pytest.raises(bridge.EvidenceError, match="limit"):
        raw_archive.unpack(
            archive, tmp_path / "over", limits=raw_archive.ArchiveLimits(1024**2, max_entries=1999)
        )
    assert not (tmp_path / "over").exists()


def test_both_archive_domains_bind_the_shared_io_owner():
    from tools.ci.source_closure import _python_import_roots

    for owner in ("pair_capture.py", "corpus_binding.py"):
        closed = _python_import_roots(bridge.ROOT, [f"tools/benchmark/{owner}"])
        assert "tools/benchmark/raw_archive.py" in closed
        assert "tools/benchmark/evidence.py" in closed
        assert "tools/ci/lint/handoff_validation.py" in closed


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
            log_dir = kwargs["log_dir"]
            return (
                bridge.write_raw_file(log_dir / "stdout", [b"declared fixture producer"]),
                bridge.write_raw_file(log_dir / "stderr", [b""]),
                command,
            )
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
        bridge.unpack_native(
            write_raw_file(tmp_path / "bad.zip", [buffer.getvalue()]), tmp_path / "restored"
        )
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
        bridge.unpack_native(write_raw_file(tmp_path / "bad.zip", [data]), tmp_path / "restored")


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


@pytest.mark.parametrize("nested_pair", [(0, 1), (0, 2), (1, 2)])
@pytest.mark.parametrize("reverse", [False, True])
def test_capture_entrypoint_rejects_overlap_before_epoch_writes(tmp_path, monkeypatch, nested_pair, reverse):
    paths = [tmp_path / "evidence", tmp_path / "output", tmp_path / "corpus"]
    parent, child = nested_pair[::-1] if reverse else nested_pair
    paths[child] = paths[parent] / "nested"
    spec_path = tmp_path / "spec.json"
    spec_path.write_text("{}")
    # Spec semantics are independent of this pre-mutation path boundary.
    spec = {role: str(tmp_path / role) for role in bridge.INPUT_ROLES}
    spec.update(repo=str(paths[2]), output_root=str(paths[1]), semble_python="python")
    monkeypatch.setattr(bridge.owner, "load_spec", lambda _: spec)
    with pytest.raises(bridge.EvidenceError, match="roots overlap"):
        bridge.capture(bridge.ROOT, paths[0], registry_fixture(), spec_path, 1)
    assert not any(path.exists() for path in paths)
