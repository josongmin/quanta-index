"""Complete profile publication is a custody/coverage boundary."""

import copy
import json
import os
import stat
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import evidence
import host_monitor
import profile_capture as capture


def _host_transcript(root, *, change=None):
    """Independent transcript fixture; the monitor under test did not emit it."""
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    header = {
        "kind": "cooperative-host-observations",
        "schema_version": 1,
        "capture_id": "capture",
        "profile": "profile",
        "reservation_id": "reservation",
        "lock_identity": [1, 2, os.getuid(), stat.S_IFREG | 0o600, 1],
        "interval_ns": host_monitor.INTERVAL_NS,
        "max_gap_ns": host_monitor.MAX_GAP_NS,
        "clock_tolerance_ns": host_monitor.CLOCK_TOLERANCE_NS,
        "host": host,
    }
    samples = [
        {
            "sequence": sequence,
            "event": event,
            "phase": "measure",
            "capture_id": "capture",
            "reservation_id": "reservation",
            "monotonic_ns": (sequence + 1) * 1_000_000_000,
            "wall_ns": (sequence + 2) * 1_000_000_000,
            "facts": copy.deepcopy(facts),
            "status": "completed" if event == "end" else "active",
        }
        for sequence, event in enumerate(("start", "end"))
    ]
    rows = [header, *samples]
    if change is not None:
        change(rows)
    return evidence.write_raw_file(
        root / "observations.jsonl",
        [(json.dumps(row, sort_keys=True) + "\n").encode() for row in rows],
    )


def test_host_transcript_uses_independent_expected_capture_and_observed_count(tmp_path):
    raw = _host_transcript(tmp_path)
    observed = host_monitor.validate(raw, capture_id="capture", profile="profile")
    assert observed == {
        "os": "macos",
        "arch": "arm64",
        "cpu_count": 8,
        "hostname_hash": "sha256:" + "a" * 64,
        "observed_samples": 2,
        "capture_id": "capture",
        "profile": "profile",
        "digest": raw.sha256,
    }
    with pytest.raises(evidence.EvidenceError, match="identity or policy differs"):
        host_monitor.validate(raw, capture_id="other", profile="profile")


@pytest.mark.parametrize(
    "damage",
    [
        "missing_end",
        "duplicate",
        "sequence",
        "reservation",
        "gap",
        "clock",
        "bool_count",
        "bad_mode",
        "hardlink",
        "failed_end",
        "wrong_profile",
    ],
)
def test_host_transcript_refuses_corrupt_or_partial_observations(tmp_path, damage):
    def mutate(rows):
        if damage == "missing_end":
            rows.pop()
        elif damage == "duplicate":
            rows.append(copy.deepcopy(rows[-1]))
        elif damage == "sequence":
            rows[-1]["sequence"] = 2
        elif damage == "reservation":
            rows[-1]["reservation_id"] = "foreign"
        elif damage == "gap":
            rows[-1]["monotonic_ns"] += host_monitor.MAX_GAP_NS
        elif damage == "clock":
            rows[-1]["wall_ns"] += host_monitor.CLOCK_TOLERANCE_NS + 1
        elif damage == "bool_count":
            rows[-1]["facts"]["process_count"] = True
        elif damage == "bad_mode":
            rows[0]["lock_identity"][3] = stat.S_IFDIR | 0o700
        elif damage == "hardlink":
            rows[0]["lock_identity"][4] = 2
        elif damage == "failed_end":
            rows[-1]["status"] = "failed"
        elif damage == "wrong_profile":
            rows[0]["profile"] = "other"

    with pytest.raises(evidence.EvidenceError):
        host_monitor.validate(
            _host_transcript(tmp_path, change=mutate), capture_id="capture", profile="profile"
        )


def test_host_monitor_cooperative_reservation_and_complete_transcript(tmp_path, monkeypatch):
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "lock")
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    first = host_monitor.HostMonitor(tmp_path / "first.jsonl", "capture", "profile").start()
    try:
        first.phase("build")
        with pytest.raises(evidence.EvidenceError, match="already held"):
            host_monitor.HostMonitor(tmp_path / "second.jsonl", "capture2", "profile").start()
        raw = first.finish()
        assert (
            host_monitor.validate(raw, capture_id="capture", profile="profile")["observed_samples"]
            == 3
        )
        second = host_monitor.HostMonitor(tmp_path / "third.jsonl", "capture3", "profile").start()
        second.finish()
    finally:
        if first.fd is not None:
            first.finish(failed=True)


def test_host_monitor_failure_retains_partial_raw_and_refuses_success(tmp_path, monkeypatch):
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "lock")
    calls = iter([(host, facts), RuntimeError("observation oracle")])

    def observe():
        value = next(calls)
        if isinstance(value, BaseException):
            raise value
        return value

    monkeypatch.setattr(host_monitor, "observe", observe)
    monitor = host_monitor.HostMonitor(tmp_path / "partial.jsonl", "capture", "profile").start()
    with pytest.raises(RuntimeError, match="observation oracle"):
        monitor.phase("measure")
    with pytest.raises(evidence.EvidenceError, match="observation oracle"):
        monitor.finish(failed=True)
    assert (tmp_path / "partial.jsonl").is_file()
    assert monitor.fd is None


def test_host_monitor_join_timeout_retains_live_custody_until_observer_stops(tmp_path, monkeypatch):
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "lock")
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    monitor = host_monitor.HostMonitor(tmp_path / "join.jsonl", "capture", "profile").start()
    actual_thread = monitor.thread
    monitor.stop_event.set()
    actual_thread.join(timeout=5)
    assert not actual_thread.is_alive()

    class StuckObserver:
        def join(self, *, timeout):
            assert timeout == 5

        def is_alive(self):
            return True

    monitor.thread = StuckObserver()
    with pytest.raises(evidence.EvidenceError, match="retained raw"):
        monitor.finish()
    assert monitor.fd is not None
    with pytest.raises(evidence.EvidenceError, match="cannot release a live host observer"):
        monitor.close()
    monitor.thread = actual_thread
    raw = monitor.finish(failed=True)
    assert raw.path.is_file()
    assert monitor.fd is None


def test_capture_failure_preserves_primary_and_monitor_finalization_error(tmp_path, monkeypatch):
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "lock")
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    root = tmp_path / "evidence"
    with pytest.raises(ValueError, match="primary oracle"):
        with capture.CaptureEpoch(
            tmp_path / "repo", root, "profile", capture_id="failed", monitor_host=True
        ) as epoch:
            epoch.host_monitor = host_monitor.HostMonitor(
                epoch.work / "host-observations.jsonl",
                "failed",
                "profile",
            ).start()
            original_finish = epoch.host_monitor.finish

            def fail_after_close(*, failed=False):
                original_finish(failed=failed)
                raise RuntimeError("monitor finalization oracle")

            monkeypatch.setattr(epoch.host_monitor, "finish", fail_after_close)
            raise ValueError("primary oracle")
    failure = json.loads((root / "failures/failed.json").read_text())
    assert failure["error"]["message"] == "primary oracle"
    assert failure["error"]["monitor"] == {
        "type": "RuntimeError",
        "message": "monitor finalization oracle",
    }
    assert not (root / "profiles").exists()


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
    with pytest.raises(
        evidence.EvidenceError, match="primary oracle.*NOT_PERSISTED.*marker oracle"
    ) as caught:
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
            root,
            capture_id=epoch.capture_id,
            profile="profile",
            registry_digest=evidence.digest_bytes(b"registry"),
            expected_cases={"family": ["case"]},
            runs=[prepared_run(root, "nested-run", "case")],
            replay=lambda *_: None,
            verify_source=lambda: None,
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
    argv = (
        [str(tmp_path / "missing-producer")]
        if mode == "spawn"
        else [sys.executable, "-c", scripts[mode]]
    )
    with pytest.raises((ProducerExecutionError, subprocess.TimeoutExpired, OSError)):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="failed") as epoch:
            epoch.execute(
                execute,
                argv,
                cwd=tmp_path,
                env=dict(os.environ),
                timeout=1,
                log_dir=epoch.work / "execution",
            )
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


def test_monitored_publication_binds_raw_host_in_every_reader(tmp_path, monkeypatch):
    import os

    from evidence_bridge import verify_host_binding
    from producer_execution import execute

    root = tmp_path / "evidence"
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "host-lock")
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    prepared = prepared_run(root, "capture-0", "case")
    prepared["boundary"] = {
        **prepared["boundary"],
        "start_event": "criterion_sample_start",
        "end_event": "criterion_sample_end",
    }
    registry = evidence.digest_bytes(b"registry")
    with capture.CaptureEpoch(
        tmp_path / "repo", root, "profile", capture_id="capture", monitor_host=True
    ) as epoch:
        epoch.execute(
            execute,
            [sys.executable, "-c", "print('owned host observation')"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=10,
            log_dir=epoch.work / "execution" / "measure",
        )
        document = capture.publish_capture(
            root,
            capture_id="capture",
            profile="profile",
            registry_digest=registry,
            expected_cases={"family": ["case"]},
            runs=[prepared],
            replay=lambda *_: None,
            verify_source=lambda: None,
        )
    assert capture.load_capture(root, profile="profile", registry_digest=registry) == document
    store = evidence.RunStore(root)
    record = store.load("capture-0")
    assert record["host"]["lease"]["mode"] == "exclusive"
    assert record["host"]["lease"]["observed_samples"] >= 3
    assert len([ref for ref in record["raw"] if ref["path"] == "raw/host-observations.jsonl"]) == 1
    assert len([row for row in record["inputs"] if row["id"] == "benchmark-host-observations"]) == 1
    verify_host_binding(store, record, capture_id="capture")
    for mutate in (
        lambda row: row["raw"].pop(),
        lambda row: row["inputs"].pop(),
        lambda row: row["host"]["lease"].update(observed_samples=0),
        lambda row: row["verdict"].update(scope="performance"),
    ):
        damaged = copy.deepcopy(record)
        mutate(damaged)
        with pytest.raises(evidence.EvidenceError):
            verify_host_binding(store, damaged, capture_id="capture")
    with pytest.raises(evidence.EvidenceError, match="capture ID"):
        verify_host_binding(store, record, capture_id="foreign")


def test_declared_monitored_boundary_cannot_publish_without_observations(tmp_path):
    root = tmp_path / "evidence"
    prepared = prepared_run(root, "capture-0", "case")
    prepared["boundary"] = {**prepared["boundary"], "start_event": "criterion_sample_start"}
    with pytest.raises(evidence.EvidenceError, match="host observation inventory"):
        with capture.CaptureEpoch(tmp_path / "repo", root, "profile", capture_id="capture"):
            capture.publish_capture(
                root,
                capture_id="capture",
                profile="profile",
                registry_digest=evidence.digest_bytes(b"registry"),
                expected_cases={"family": ["case"]},
                runs=[prepared],
                replay=lambda *_: None,
                verify_source=lambda: None,
            )
    assert not (root / "profiles/profile.json").exists()


def test_complete_publication_and_reload_peak_rss_is_payload_independent(tmp_path, record_property):
    """Measure the complete generic capture path, not only raw copy helpers."""
    import hashlib
    import subprocess

    script = """
import hashlib,json,resource,sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import evidence,profile_capture
base=Path(sys.argv[2]); size=int(sys.argv[3]); root=base/'evidence'
source=base/'input.bin'
with source.open('wb') as output: output.truncate(size)
expected=hashlib.sha256()
for _ in range(size//65536): expected.update(bytes(65536))
digest='sha256:'+expected.hexdigest()
raw=evidence.RawFile.capture(source)
assert raw.sha256==digest and raw.size==size
sample=evidence.sample_evidence()
prepared={key:sample[key] for key in ('created_utc','payload','source','build','inputs','host','command','boundary','verdict')}
prepared.update(run_id='run',family='family',profile='profile',case_id='case',raw_files={'input.bin':raw})
registry=evidence.digest_bytes(b'registry')
def replay(store,record):
    ref=next(item for item in record['raw'] if item['path']=='raw/input.bin')
    observed=evidence.RawFile.capture(store.run_dir('run')/ref['path'])
    assert (observed.sha256,observed.size)==(digest,size)
with profile_capture.CaptureEpoch(base/'repo',root,'profile',capture_id='capture'):
    document=profile_capture.publish_capture(root,capture_id='capture',profile='profile',registry_digest=registry,expected_cases={'family':['case']},runs=[prepared],replay=replay,verify_source=lambda:None)
assert profile_capture.load_capture(root,profile='profile',registry_digest=registry)==document
replay(evidence.RunStore(root),evidence.RunStore(root).load('run'))
peak=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
if sys.platform!='darwin': peak*=1024
print(json.dumps({'digest':digest,'bytes':size,'peak_bytes':peak}))
"""
    measurements = []
    for size in (8 * 1024 * 1024, 128 * 1024 * 1024):
        base = tmp_path / str(size)
        base.mkdir()
        completed = subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                script,
                str(Path(__file__).resolve().parents[3] / "tools/benchmark"),
                str(base),
                str(size),
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=90,
        )
        result = json.loads(completed.stdout)
        expected = hashlib.sha256()
        for _ in range(size // 65536):
            expected.update(bytes(65536))
        assert result["digest"] == "sha256:" + expected.hexdigest()
        assert result["bytes"] == size
        assert result["peak_bytes"] > 0
        record_property(f"complete_capture_{size}_peak_bytes", result["peak_bytes"])
        measurements.append(result)
    # A 120 MiB input increase must not introduce a payload-sized allocation.
    assert measurements[1]["peak_bytes"] - measurements[0]["peak_bytes"] < 48 * 1024 * 1024


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
    with capture.CaptureEpoch(
        root.parent / "source", root, options["profile"], capture_id=options["capture_id"]
    ):
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
    assert (
        capture.load_capture(tmp_path, profile="profile", registry_digest=prior["registry_digest"])
        == prior
    )
    failure = json.loads((tmp_path / "failures/capture2.json").read_text())
    assert failure["commit_state"] == "not_started"


@pytest.mark.parametrize(
    "boundary",
    ["before-capture", "before-pointer", "after-pointer", "final-source", "after-returned-commit"],
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
    expected_state = (
        "returned"
        if boundary == "after-returned-commit"
        else "not_started"
        if boundary == "final-source"
        else "attempted"
    )
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
