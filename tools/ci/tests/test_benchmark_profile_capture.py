"""Complete profile publication is a custody/coverage boundary."""

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import evidence
import profile_capture as capture


@pytest.mark.parametrize("error_type", [ValueError, KeyboardInterrupt])
def test_capture_epoch_retains_early_failure_without_inventing_observations(tmp_path, error_type):
    root = tmp_path / "evidence"
    with pytest.raises(error_type, match="input oracle"):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed") as epoch:
            epoch.step("inputs")
            raise error_type("input oracle")
    failure = json.loads((root / "failures/failed.json").read_text())
    assert failure["status"] == "failed"
    assert failure["phase"] == "inputs"
    assert failure["commit_state"] == "not_started"
    assert failure["observations"] == {}
    assert failure["error"]["type"] == error_type.__name__
    assert [row["phase"] for row in failure["history"]] == ["admission", "inputs"]
    assert [row["sequence"] for row in failure["history"]] == [1, 2]
    assert not (root / "captures").exists()
    assert not (root / "profiles").exists()


def test_capture_epoch_refuses_uncommitted_success(tmp_path):
    root = tmp_path / "evidence"
    with pytest.raises(evidence.EvidenceError, match="without a complete profile commit"):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed"):
            pass
    assert json.loads((root / "failures/failed.json").read_text())["status"] == "failed"


def test_capture_marker_failure_preserves_primary_and_secondary(tmp_path, monkeypatch):
    root = tmp_path / "evidence"
    primary = ValueError("primary oracle")

    def cannot_write(*args, **kwargs):
        raise OSError("marker oracle")

    monkeypatch.setattr(capture, "write_raw_file", cannot_write)
    with pytest.raises(evidence.EvidenceError, match="primary oracle.*NOT_PERSISTED.*marker oracle") as caught:
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed"):
            raise primary
    assert caught.value.__cause__ is primary
    assert not (root / "failures/failed.json").exists()
    with pytest.raises(evidence.EvidenceError, match="no owning epoch"):
        capture.current_capture()


@pytest.mark.parametrize("unsafe", ["checkout", "symlink"])
def test_capture_epoch_does_not_write_through_unsafe_namespace(tmp_path, unsafe):
    repo = tmp_path / "repo"
    repo.mkdir()
    if unsafe == "checkout":
        root = repo / "evidence"
    else:
        real = tmp_path / "real"
        real.mkdir()
        root = tmp_path / "alias"
        root.symlink_to(real, target_is_directory=True)
    with pytest.raises(evidence.EvidenceError):
        with capture.CaptureEpoch(repo, root, "profile"):
            pytest.fail("unsafe capture admitted")
    assert not (root / "work").exists()
    assert not (root / "failures").exists()


def test_capture_cli_return_keeps_the_primary_failure(tmp_path):
    @capture.capture_entrypoint("profile")
    def command(repo, root):
        capture.capture_phase("preflight")
        capture.capture_error(ValueError("real preflight refusal"))
        return 2

    root = tmp_path / "evidence"
    assert command(tmp_path / "repo", root) == 2
    failure = json.loads(next((root / "failures").glob("*.json")).read_text())
    assert failure["phase"] == "preflight"
    assert failure["error"]["message"] == "real preflight refusal"


def add_run(root, name="r1", family="family", case="case"):
    store = evidence.RunStore(root)
    staged = store.stage(name)
    source = evidence.write_raw_file(root / "work" / name / "input.txt", [b"oracle"])
    ref = staged.write_raw("raw/input.txt", source)
    record = evidence.sample_evidence()
    record.update(run_id=name, family=family, profile="profile", case_id=case, raw=[ref])
    staged.write_evidence(evidence.seal(record))
    store.promote(staged)
    return store


def publish(root, **kwargs):
    return capture.commit_capture(
        root,
        capture_id="capture1",
        profile="profile",
        registry_digest=evidence.digest_bytes(b"registry"),
        expected_cases={"family": ["case"]},
        run_ids=["r1"],
        **kwargs,
    )


def test_publish_replay_and_gc_pin(tmp_path):
    store = add_run(tmp_path)
    published = publish(tmp_path)
    assert (
        capture.load_capture(
            tmp_path, profile="profile", registry_digest=published["registry_digest"]
        )
        == published
    )
    add_run(tmp_path, "unreferenced")
    assert store.collect([]) == ["unreferenced"]
    assert store.load("r1")["case_id"] == "case"


def test_partial_capture_does_not_publish(tmp_path):
    add_run(tmp_path)
    with pytest.raises(ValueError, match="inventory"):
        capture.commit_capture(
            tmp_path,
            capture_id="capture1",
            profile="profile",
            registry_digest=evidence.digest_bytes(b"registry"),
            expected_cases={"family": ["case", "missing"]},
            run_ids=["r1"],
        )
    assert not (tmp_path / "profiles/profile.json").exists()


def test_capture_id_is_immutable(tmp_path):
    add_run(tmp_path)
    publish(tmp_path)
    with pytest.raises(ValueError, match="already exists"):
        publish(tmp_path)


def test_registry_drift_refuses_capture(tmp_path):
    add_run(tmp_path)
    publish(tmp_path)
    with pytest.raises(ValueError, match="identity"):
        capture.load_capture(
            tmp_path, profile="profile", registry_digest=evidence.digest_bytes(b"changed")
        )


def test_corrupt_capture_refuses_gc_before_deletion(tmp_path):
    store = add_run(tmp_path)
    publish(tmp_path)
    add_run(tmp_path, "unreferenced")
    (tmp_path / "captures/capture1.json").write_text("{}")
    with pytest.raises(ValueError):
        store.collect([])
    assert store.run_dir("unreferenced").is_dir()


@pytest.mark.parametrize("control", ["profiles/profile.json", "captures/capture1.json"])
def test_capture_control_documents_have_a_bounded_reader(tmp_path, monkeypatch, control):
    store = add_run(tmp_path)
    published = publish(tmp_path)
    monkeypatch.setattr(evidence, "CONTROL_DOCUMENT_BYTES", 4096)
    (tmp_path / control).write_bytes(b" " * 4097)
    with pytest.raises(evidence.EvidenceError, match="control document.*limit"):
        capture.load_capture(
            tmp_path, profile="profile", registry_digest=published["registry_digest"]
        )
    assert store.load("r1")["case_id"] == "case"


def test_oversize_capture_refuses_before_creating_a_gc_reference(tmp_path, monkeypatch):
    store = add_run(tmp_path)
    prior = publish(tmp_path)
    cases = [f"case{index}-" + "a" * 1000 for index in range(5)]
    ids = [f"long-{index}" for index in range(5)]
    for name, case in zip(ids, cases, strict=True):
        add_run(tmp_path, name, case=case)
    monkeypatch.setattr(evidence, "CONTROL_DOCUMENT_BYTES", 4096)
    with pytest.raises(evidence.EvidenceError, match="control document.*limit"):
        capture.commit_capture(
            tmp_path,
            capture_id="oversize",
            profile="profile",
            registry_digest=prior["registry_digest"],
            expected_cases={"family": cases},
            run_ids=ids,
        )
    assert not (tmp_path / "captures/oversize.json").exists()
    assert (
        capture.load_capture(tmp_path, profile="profile", registry_digest=prior["registry_digest"])
        == prior
    )
    assert sorted(store.collect([])) == sorted(ids)


@pytest.mark.parametrize("cases", [[{}], [["case"]], ["case", "case"], [], [False]])
def test_malformed_inventory_refused_as_evidence_error(cases):
    with pytest.raises(ValueError):
        capture._check_cases({"family": ["case"]}, {"family": cases})


def test_parent_symlink_is_refused(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    link = tmp_path / "link"
    link.symlink_to(real, target_is_directory=True)
    with pytest.raises(ValueError, match="symlink"):
        capture.load_capture(
            link, profile="profile", registry_digest=evidence.digest_bytes(b"registry")
        )


def test_gc_waits_for_profile_publication_across_processes(tmp_path):
    import subprocess

    from custody import custody

    store = add_run(tmp_path)
    add_run(tmp_path, "unreferenced")
    script = (
        "import sys; from pathlib import Path; "
        f"sys.path.insert(0, {str(Path(__file__).resolve().parents[3] / 'tools/benchmark')!r}); "
        "from evidence import RunStore; print('ready', flush=True); "
        "print(RunStore(Path(sys.argv[1])).collect([]), flush=True)"
    )
    with custody(tmp_path):
        process = subprocess.Popen(
            [sys.executable, "-c", script, str(tmp_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            assert process.stdout.readline().strip() == "ready"
            assert process.poll() is None
            publish(tmp_path)
        except BaseException:
            process.kill()
            process.communicate()
            raise
    stdout, stderr = process.communicate(timeout=20)
    assert process.returncode == 0, stderr
    assert stdout.strip() == "['unreferenced']"
    assert store.load("r1")["case_id"] == "case"


def test_duplicate_capture_keys_refused(tmp_path):
    add_run(tmp_path)
    published = publish(tmp_path)
    path = tmp_path / "profiles/profile.json"
    path.write_text(
        '{"capture_id":"missing","capture_id":"capture1","digest":'
        + json.dumps(published["digest"])
        + "}"
    )
    with pytest.raises(ValueError, match="duplicate"):
        capture.load_capture(
            tmp_path, profile="profile", registry_digest=published["registry_digest"]
        )


def prepared_run(root, name, case):
    record = evidence.sample_evidence()
    return {
        **{
            key: record[key]
            for key in (
                "created_utc",
                "payload",
                "source",
                "build",
                "inputs",
                "host",
                "command",
                "boundary",
                "verdict",
            )
        },
        "run_id": name,
        "family": "family",
        "profile": "profile",
        "case_id": case,
        "raw_files": {
            "oracle.txt": evidence.write_raw_file(root / "work" / name / "oracle.txt", [b"oracle"])
        },
    }


def publish_prepared(root, **overrides):
    def replay(store, record):
        assert (store.run_dir(record["run_id"]) / "raw/oracle.txt").read_bytes() == b"oracle"

    options = dict(
        capture_id="capture2",
        profile="profile",
        registry_digest=evidence.digest_bytes(b"registry"),
        expected_cases={"family": ["a", "b"]},
        runs=overrides["runs"]
        if "runs" in overrides
        else [prepared_run(root, "r2", "a"), prepared_run(root, "r3", "b")],
        replay=replay,
        verify_source=lambda: None,
    )
    options.update(overrides)
    with capture.CaptureEpoch(root.parent / "source", root, options["profile"], capture_id=options["capture_id"]):
        return capture.publish_capture(root, **options)


@pytest.mark.parametrize(
    "mutation", ["missing", "extra", "duplicate-case", "duplicate-run", "source", "profile"]
)
def test_prepared_inventory_refuses_before_any_run_is_written(tmp_path, mutation):
    runs = [prepared_run(tmp_path, "r2", "a"), prepared_run(tmp_path, "r3", "b")]
    if mutation == "missing":
        runs.pop()
    elif mutation == "extra":
        runs.append(prepared_run(tmp_path, "r4", "extra"))
    elif mutation == "duplicate-case":
        runs[1]["case_id"] = "a"
    elif mutation == "duplicate-run":
        runs[1]["run_id"] = "r2"
    elif mutation == "source":
        runs[1]["source"]["revision"] = "b" * 40
    else:
        runs[1]["profile"] = "foreign"
    with pytest.raises(evidence.EvidenceError):
        publish_prepared(tmp_path, runs=runs)
    assert not (tmp_path / "runs").exists()
    assert not (tmp_path / "profiles/profile.json").exists()


def test_domain_replay_failure_cannot_replace_profile(tmp_path):
    store = add_run(tmp_path)
    prior = publish(tmp_path)

    def refuse_second(_store, record):
        if record["run_id"] == "r3":
            raise evidence.EvidenceError("independent domain replay failed")

    with pytest.raises(evidence.EvidenceError, match="domain replay"):
        publish_prepared(tmp_path, replay=refuse_second)
    assert (
        capture.load_capture(tmp_path, profile="profile", registry_digest=prior["registry_digest"])
        == prior
    )
    assert store.collect([]) == ["r2", "r3"]
    failure = json.loads((tmp_path / "failures/capture2.json").read_text())
    assert failure["phase"] == "domain_replay"
    assert failure["observations"]["run_id"] == "r3"
    assert failure["commit_state"] == "not_started"
    assert failure["error"]["message"] == "independent domain replay failed"


@pytest.mark.parametrize(
    "boundary", ["before-capture", "before-pointer", "after-pointer", "final-source"]
)
def test_publication_failures_preserve_or_recover_a_complete_pointer(
    tmp_path, monkeypatch, boundary
):
    add_run(tmp_path)
    prior = publish(tmp_path)
    pointer = tmp_path / "profiles/profile.json"
    previous = pointer.read_bytes()
    options = {}
    if boundary == "before-capture":

        def fail_commit(*_args, **_kwargs):
            raise OSError("injected before-capture")

        monkeypatch.setattr(capture, "commit_capture", fail_commit)
    elif boundary in {"before-pointer", "after-pointer"}:
        real = capture._write_atomic

        def fail_pointer(path, raw):
            if path.parent.name != "profiles":
                return real(path, raw)
            if boundary == "after-pointer":
                real(path, raw)
            raise OSError(f"injected {boundary}")

        monkeypatch.setattr(capture, "_write_atomic", fail_pointer)
    else:

        def source_changed():
            if (tmp_path / "runs/r3").exists():
                raise evidence.EvidenceError("injected final-source")

        options["verify_source"] = source_changed
    with pytest.raises((OSError, evidence.EvidenceError), match="injected"):
        publish_prepared(tmp_path, **options)
    current = capture.load_capture(
        tmp_path, profile="profile", registry_digest=prior["registry_digest"]
    )
    if boundary == "after-pointer":
        assert current["capture_id"] == "capture2"
        assert {row["run_id"] for row in current["runs"]} == {"r2", "r3"}
    else:
        assert current == prior
        assert pointer.read_bytes() == previous


@pytest.mark.parametrize("boundary", ["before-capture", "before-pointer", "after-pointer"])
def test_process_death_at_publication_boundary_has_only_complete_pointer(tmp_path, boundary):
    import subprocess

    add_run(tmp_path)
    prior = publish(tmp_path)
    script = f"""
import os, sys
from pathlib import Path
sys.path.insert(0, {str(Path(__file__).resolve().parents[3])!r})
from tools.ci.tests.test_benchmark_profile_capture import capture, publish_prepared
boundary = sys.argv[2]
if boundary == 'before-capture':
    def die(*args, **kwargs):
        os._exit(91)
    capture.commit_capture = die
else:
    real = capture._write_atomic
    def die(path, raw):
        if path.parent.name != 'profiles':
            return real(path, raw)
        if boundary == 'after-pointer':
            real(path, raw)
        os._exit(91)
    capture._write_atomic = die
publish_prepared(Path(sys.argv[1]))
"""
    result = subprocess.run(
        [sys.executable, "-I", "-c", script, str(tmp_path), boundary],
        capture_output=True,
        timeout=20,
    )
    assert result.returncode == 91, result.stderr.decode()
    current = capture.load_capture(
        tmp_path, profile="profile", registry_digest=prior["registry_digest"]
    )
    if boundary == "after-pointer":
        assert current["capture_id"] == "capture2"
        assert {run["run_id"] for run in current["runs"]} == {"r2", "r3"}
    else:
        assert current == prior
    # A crashed owner releases OS custody; collection must neither deadlock nor
    # remove runs referenced by the surviving complete pointer/capture record.
    _removed = evidence.RunStore(tmp_path).collect([])
    assert (
        capture.load_capture(tmp_path, profile="profile", registry_digest=prior["registry_digest"])
        == current
    )
