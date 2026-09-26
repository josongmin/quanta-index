"""Complete profile publication is a custody/coverage boundary."""

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import evidence
import profile_capture as capture


def add_run(root, name="r1", family="family", case="case"):
    store = evidence.RunStore(root)
    staged = store.stage(name)
    ref = staged.write_raw("raw/input.txt", b"oracle")
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
