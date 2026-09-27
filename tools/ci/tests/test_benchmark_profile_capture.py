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


@pytest.mark.parametrize("outcome", ["nonzero", "recorded", "exception"])
def test_nested_failure_cannot_be_ignored_to_publish_a_capture(tmp_path, outcome):
    root, repo = tmp_path / "evidence", tmp_path / "repo"

    @capture.capture_entrypoint("profile")
    def child(repo, root):
        capture.capture_phase("preflight")
        if outcome == "exception":
            raise ValueError("nested refusal oracle")
        if outcome == "recorded":
            capture.capture_error(ValueError("nested refusal oracle"))
        return 7

    @capture.capture_entrypoint("profile")
    def parent(repo, root):
        if outcome == "exception":
            with pytest.raises(ValueError, match="nested refusal oracle"):
                child(repo, root)
        else:
            assert child(repo, root) == 7
        epoch = capture.current_capture()
        return capture.publish_capture(
            root, capture_id=epoch.capture_id, profile="profile",
            registry_digest=evidence.digest_bytes(b"registry"),
            expected_cases={"family": ["case"]},
            runs=[prepared_run(root, "nested-run", "case")],
            replay=lambda *_: None, verify_source=lambda: None,
        )

    with pytest.raises(ValueError, match="nested refusal oracle|child returned exit 7"):
        parent(repo, root)
    assert not (root / "profiles/profile.json").exists()
    assert not (root / "runs/nested-run").exists()
    failure = json.loads(next((root / "failures").glob("*.json")).read_text())
    assert failure["phase"] == "preflight"
    assert failure["commit_state"] == "not_started"


def test_initial_journal_write_failure_retains_reason_and_resets_context(tmp_path, monkeypatch):
    def full_disk(*_):
        raise OSError("journal disk-full oracle")

    monkeypatch.setattr(capture, "_write_atomic", full_disk)
    root = tmp_path / "evidence"
    with pytest.raises(OSError, match="journal disk-full oracle"):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed"):
            pytest.fail("journal failure admitted body")
    failure = json.loads((root / "failures/failed.json").read_text())
    assert failure["phase"] == "admission"
    assert failure["error"]["message"] == "journal disk-full oracle"
    assert failure["observations"] == {}
    with pytest.raises(evidence.EvidenceError, match="no owning epoch"):
        capture.current_capture()


@pytest.mark.parametrize("mismatch", ["repo", "root", "profile"])
def test_nested_capture_requires_the_same_authority(tmp_path, mismatch):
    @capture.capture_entrypoint()
    def child(repo, root, profile):
        pytest.fail("foreign nested capture entered")

    repo, root = tmp_path / "repo", tmp_path / "evidence"
    options = dict(repo=repo, root=root, profile="profile")
    options[mismatch] = "other" if mismatch == "profile" else tmp_path / "other"
    with pytest.raises(evidence.EvidenceError, match="differs from its capture epoch"):
        with capture.CaptureEpoch(repo, root, "profile", capture_id="failed"):
            child(**options)
    failure = json.loads((root / "failures/failed.json").read_text())
    assert failure["error"]["message"] == "nested publication differs from its capture epoch"
    assert not (tmp_path / "other").exists()


@pytest.mark.parametrize("mode", ["spawn", "timeout", "nonzero", "interrupt"])
def test_capture_retains_real_execution_failure_and_does_not_invent_terminal(tmp_path, mode):
    import os
    import subprocess

    from producer_execution import ProducerExecutionError, execute

    root = tmp_path / "evidence"
    scripts = {
        "timeout": "import time; print('started', flush=True); time.sleep(30)",
        "nonzero": "print('failed-output'); raise SystemExit(7)",
        "interrupt": f"import os, signal, time; os.kill({os.getpid()}, signal.SIGTERM); time.sleep(30)",
    }
    argv = ([str(tmp_path / "missing-producer")] if mode == "spawn" else
            [sys.executable, "-c", scripts[mode]])
    with pytest.raises((ProducerExecutionError, subprocess.TimeoutExpired, OSError)):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed") as epoch:
            epoch.execute(execute, argv, cwd=tmp_path, env=dict(os.environ), timeout=1,
                          log_dir=epoch.work / "execution")
    failure = json.loads((root / "failures/failed.json").read_text())
    assert failure["phase"] == "execution"
    observed = failure["observations"]["execution"]
    raw = evidence.RawFile.capture(Path(observed["record"]["path"]))
    assert raw.sha256 == observed["record"]["sha256"]
    assert raw.size == observed["record"]["bytes"]
    terminal = json.loads(raw.read_control())
    assert terminal["status"] == "failed"
    if mode in {"spawn", "timeout", "interrupt"}:
        assert terminal["command"] is None
        assert terminal["error_type"] == "ProducerExecutionError"
        if mode == "timeout":
            assert "timed out" in failure["error"]["message"]
        elif mode == "interrupt":
            assert "interrupted by SIGTERM" in failure["error"]["message"]
        else:
            assert "FileNotFoundError" in (Path(observed["log_dir"]) / "stderr").read_text()
    else:
        assert terminal["command"]["exit_code"] != 0
    assert not (root / "profiles").exists()


def test_failure_record_keeps_primary_and_later_cleanup_error(tmp_path):
    root = tmp_path / "evidence"
    with pytest.raises(OSError, match="cleanup oracle"):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed") as epoch:
            epoch.reject(ValueError("primary oracle"))
            raise OSError("cleanup oracle")
    error = json.loads((root / "failures/failed.json").read_text())["error"]
    assert error["message"] == "primary oracle"
    assert error["secondary"] == {"type": "OSError", "message": "cleanup oracle"}


def test_failure_cannot_overwrite_an_existing_diagnostic(tmp_path):
    root = tmp_path / "evidence"
    prior = evidence.write_raw_file(root / "failures/failed.json", [b"prior diagnostic oracle"])
    primary = ValueError("new failure oracle")
    with pytest.raises(evidence.EvidenceError, match="NOT_PERSISTED") as caught:
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed"):
            raise primary
    assert caught.value.__cause__ is primary
    assert prior.path.read_bytes() == b"prior diagnostic oracle"
    with pytest.raises(evidence.EvidenceError, match="no owning epoch"):
        capture.current_capture()


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


@pytest.mark.parametrize("boundary", ["source", "replay", "final-source"])
def test_publication_cannot_ignore_callback_recorded_refusal(tmp_path, boundary):
    add_run(tmp_path)
    prior = publish(tmp_path)
    calls = 0

    def check_source():
        nonlocal calls
        calls += 1
        if (boundary == "source" and calls == 1) or (boundary == "final-source" and calls == 3):
            capture.capture_error(ValueError("source callback refusal"))

    def replay(*_):
        if boundary == "replay":
            capture.capture_error(ValueError("replay callback refusal"))

    with pytest.raises(ValueError, match="callback refusal"):
        publish_prepared(tmp_path, verify_source=check_source, replay=replay)
    assert capture.load_capture(tmp_path, profile="profile", registry_digest=prior["registry_digest"]) == prior
    failure = json.loads((tmp_path / "failures/capture2.json").read_text())
    assert failure["commit_state"] == "not_started"


@pytest.mark.parametrize(
    "boundary", ["before-capture", "before-pointer", "after-pointer", "final-source", "after-returned-commit"]
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
    elif boundary == "after-returned-commit":
        real = capture.CaptureEpoch.committed

        def failed_response(epoch, document):
            real(epoch, document)
            raise OSError("injected after-returned-commit")

        monkeypatch.setattr(capture.CaptureEpoch, "committed", failed_response)
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
    if boundary in {"after-pointer", "after-returned-commit"}:
        assert current["capture_id"] == "capture2"
        assert {row["run_id"] for row in current["runs"]} == {"r2", "r3"}
    else:
        assert current == prior
        assert pointer.read_bytes() == previous
    failure = json.loads((tmp_path / "failures/capture2.json").read_text())
    expected_state = ("returned" if boundary == "after-returned-commit" else
                      "not_started" if boundary == "final-source" else "attempted")
    assert failure["commit_state"] == expected_state
    assert failure["status"] == "failed"
    assert failure["error"]["message"] == f"injected {boundary}"
    if boundary == "after-returned-commit":
        assert failure["observations"]["capture_digest"] == current["digest"]


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
    journal = json.loads((tmp_path / "work/capture2/capture.json").read_text())
    assert journal["status"] == "active"
    assert journal["phase"] == "commit"
    assert journal["commit_state"] == "attempted"
    assert not (tmp_path / "failures/capture2.json").exists()
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
