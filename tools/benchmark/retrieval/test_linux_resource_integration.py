"""Closed Linux resource contract and owner-launch integration tests.

These fake-owner tests do not qualify a native Linux host.
"""

import copy
import json
import os

import pytest

from tools.benchmark.retrieval import linux_process, query_plan
from tools.benchmark.retrieval import run as pairrun


def _storage():
    return {
        "index_bytes": 1,
        "model_cache_bytes": 0,
        "parser_cache_bytes": 0,
        "embedding_cache_bytes": 0,
        "discovered_files": 1,
        "indexed_chunks": 1,
        "index_storage": "disk",
        "index_measurement": "filesystem_tree_v1",
    }


def _result(backend, parent=None):
    root = linux_process.ProcessIdentity(321, 98765)
    return linux_process.ProcessResult(
        root=root,
        root_exit_code=0,
        timed_out=False,
        elapsed_ms=12.5,
        sample_interval_ms=50,
        samples=3,
        peak_tree_rss_bytes=4096,
        total_user_cpu_ns=1000,
        total_kernel_cpu_ns=2000,
        processes=(linux_process.ProcessEvidence(root, 4096, 1000, 2000),),
        escaped=(),
        sampling_complete=True,
        cleanup_complete=True,
        stdout_path=None,
        stderr_path=None,
        backend=backend,
        cgroup_path=(str(parent / "quanta-retrieval-test") if backend == "cgroup-v2" else None),
        peak_cgroup_memory_bytes=32768 if backend == "cgroup-v2" else None,
        cgroup_cpu_usage_ns=3500 if backend == "cgroup-v2" else None,
    )


def _paths(tmp_path):
    return {
        "stdout_path": tmp_path / "stdout.log",
        "stderr_path": tmp_path / "stderr.log",
        "resource_path": tmp_path / "resource.json",
    }


def _isolation():
    return {
        "backend": pairrun.LINUX_ISOLATION_BACKEND,
        "policy_sha256": "a" * 64,
        "proof_sha256": "b" * 64,
        "_child_check": {"pack_sha256": "c" * 64},
    }


def _valid_qualified_v2(parent):
    command_digest = "d" * 64
    child = {
        "nonce": "e" * 64,
        "abi": pairrun.linux_isolation.MIN_ABI,
        "exec_sha256": command_digest,
        "suite_read_denied": True,
        "query_pack_read_allowed": True,
        "proc_read_denied": True,
    }
    return {
        "schema_version": 2,
        "sampler": "linux-process-owner-v1",
        "capture_scope": "qualified",
        "owner_backend": "cgroup-v2",
        "sample_interval_ms": 50,
        "command_sha256": "f" * 64,
        "exec_command_sha256": command_digest,
        "subject_sha256": "a" * 64,
        "root_pid": 321,
        "root_start_ticks": 98765,
        "exit_code": 0,
        "timed_out": False,
        "elapsed_ms": 12.5,
        "peak_rss_bytes": 4096,
        "peak_cgroup_memory_bytes": 32768,
        "total_user_cpu_ns": 1000,
        "total_kernel_cpu_ns": 2000,
        "cgroup_cpu_usage_ns": 3500,
        "cgroup_path": str(parent / "quanta-retrieval-test"),
        "delegated_cgroup_parent": pairrun._linux_parent_identity(str(parent)),
        "processes": [
            {
                "pid": 321,
                "start_ticks": 98765,
                "peak_rss_bytes": 4096,
                "user_cpu_ns": 1000,
                "kernel_cpu_ns": 2000,
            }
        ],
        "escaped": [],
        "samples": 3,
        "sampling_complete": True,
        "cleanup_complete": True,
        "ownership_complete": True,
        "isolation": {
            "backend": pairrun.LINUX_ISOLATION_BACKEND,
            "policy_sha256": "b" * 64,
            "proof_sha256": "c" * 64,
            "child_attestation": child,
        },
        "storage": _storage(),
    }


def test_qualified_v2_closed_schema_rejects_adversarial_mutations(tmp_path):
    parent = tmp_path / "delegated"
    parent.mkdir()
    base = _valid_qualified_v2(parent)
    assert base["peak_rss_bytes"] != base["peak_cgroup_memory_bytes"]
    pairrun._validate_resource_metrics(base, "base")
    mutations = [
        (lambda p: p.update(owner_backend="wrong"), "owner_backend"),
        (lambda p: p.pop("cgroup_cpu_usage_ns"), "exact"),
        (lambda p: p.update(cgroup_cpu_usage_ns=-1), "cgroup_cpu_usage_ns"),
        (lambda p: p.update(cgroup_cpu_usage_ns=None), "cgroup_cpu_usage_ns"),
        (lambda p: p.update(peak_cgroup_memory_bytes=None), "peak_cgroup_memory_bytes"),
        (lambda p: p.update(peak_cgroup_memory_bytes=-1), "peak_cgroup_memory_bytes"),
        (lambda p: p.update(peak_rss_bytes=None), "peak_rss_bytes"),
        (lambda p: p.update(peak_cpu_percent=10.0), "exact"),
        (lambda p: p.update(escaped=[{"pid": 321, "start_ticks": 98765}]), "ownership_complete"),
        (lambda p: p.update(cleanup_complete=False), "cleanup"),
        (lambda p: p["processes"][0].update(pid=-1), "identity"),
        (lambda p: p["processes"][0].update(start_ticks=-1), "identity"),
        (lambda p: p["processes"].append(dict(p["processes"][0])), "identity"),
        (lambda p: p["isolation"]["child_attestation"].update(nonce="0" * 63), "child"),
        (lambda p: p["isolation"]["child_attestation"].update(exec_sha256="0" * 64), "child"),
        (lambda p: p.update(capture_scope="exploratory"), "qualified Linux"),
        (
            lambda p: p.update(cgroup_path=str(tmp_path / "other" / "quanta-retrieval-test")),
            "outside",
        ),
        (lambda p: p["delegated_cgroup_parent"].update(inode=0), "inode"),
    ]
    for mutate, reason in mutations:
        payload = copy.deepcopy(base)
        mutate(payload)
        with pytest.raises(pairrun.RunError, match=reason):
            pairrun._validate_resource_metrics(payload, "mutated")


def test_resource_binding_rejects_v1_linux_scope_and_parent_substitution(tmp_path):
    parent = tmp_path / "delegated"
    parent.mkdir()
    payload = _valid_qualified_v2(parent)
    identity = payload["delegated_cgroup_parent"]
    pairrun._validate_resource_capture_binding(
        payload,
        host_system="Linux",
        scope="qualified",
        manifest_parent=identity,
        protocol_parent=identity,
    )
    with pytest.raises(pairrun.RunError, match="capture host"):
        pairrun._validate_resource_capture_binding(
            {"schema_version": 1},
            host_system="Linux",
            scope="qualified",
            manifest_parent=identity,
            protocol_parent=identity,
        )
    for scope, manifest_parent, protocol_parent in (
        ("exploratory", identity, identity),
        ("qualified", identity, None),
        ("qualified", dict(identity, inode=identity["inode"] + 1), identity),
    ):
        with pytest.raises(pairrun.RunError, match="scope|binding"):
            pairrun._validate_resource_capture_binding(
                payload,
                host_system="Linux",
                scope=scope,
                manifest_parent=manifest_parent,
                protocol_parent=protocol_parent,
            )


def test_delegated_parent_identity_drift_refuses_launch(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    parent = tmp_path / "delegated"
    parent.mkdir()
    frozen = pairrun._linux_parent_identity(str(parent))
    parent.rename(tmp_path / "old-delegated")
    parent.mkdir()
    assert pairrun._linux_parent_identity(str(parent)) != frozen
    monkeypatch.setattr(
        pairrun.linux_process,
        "run",
        lambda *_a, **_kw: pytest.fail("drifted parent must not launch"),
    )
    with pytest.raises(pairrun.RunError, match="identity drifted before launch"):
        pairrun.run_monitored_process(
            ["/usr/bin/python3", "linux_isolation.py", "--", "/bin/true"],
            **_paths(tmp_path),
            timeout_secs=5,
            isolation=_isolation(),
            capture_scope="qualified",
            linux_cgroup_parent=str(parent),
            linux_cgroup_parent_identity=frozen,
        )
    assert not (tmp_path / "resource.json").exists()


def test_qualified_linux_owner_forwards_attestation_and_preserves_accounting(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    parent = tmp_path / "delegated"
    parent.mkdir()
    original = ["/usr/bin/python3", "linux_isolation.py", "--", "/bin/true"]
    passed = {}

    def fake_owner(command, **kwargs):
        passed.update(kwargs)
        write_fd = kwargs["pass_fds"][0]
        assert command[command.index("--attest-fd") + 1] == str(write_fd)
        assert os.fstat(write_fd)
        nonce = command[command.index("--nonce") + 1]
        exec_command = command[command.index("--") + 1 :]
        child = {
            "nonce": nonce,
            "abi": pairrun.linux_isolation.MIN_ABI,
            "exec_sha256": pairrun.digest(pairrun.canonical(exec_command)),
            "suite_read_denied": True,
            "query_pack_read_allowed": True,
            "proc_read_denied": True,
        }
        os.write(write_fd, (json.dumps(child) + "\n").encode())
        return _result("cgroup-v2", parent)

    monkeypatch.setattr(pairrun.linux_process, "run", fake_owner)
    subject = tmp_path / "record.json"
    subject.write_text("record", encoding="utf-8")
    paths = _paths(tmp_path)
    resource = pairrun.run_monitored_process(
        original,
        **paths,
        timeout_secs=5,
        subject_path=subject,
        isolation=_isolation(),
        capture_scope="qualified",
        linux_cgroup_parent=str(parent),
    )
    assert passed["qualified"] is True
    assert passed["cgroup_parent"] == str(parent)
    assert passed["stdout_path"] == str(paths["stdout_path"])
    assert len(passed["pass_fds"]) == 1
    with pytest.raises(OSError):
        os.fstat(passed["pass_fds"][0])
    assert resource["owner_backend"] == "cgroup-v2"
    assert resource["ownership_complete"] is True
    assert resource["peak_rss_bytes"] == 4096
    assert resource["peak_cgroup_memory_bytes"] == 32768
    assert resource["total_user_cpu_ns"] == 1000
    assert resource["cgroup_cpu_usage_ns"] == 3500
    assert resource["root_start_ticks"] == 98765
    assert resource["delegated_cgroup_parent"] == pairrun._linux_parent_identity(str(parent))
    assert resource["processes"][0]["start_ticks"] == 98765
    assert "peak_cpu_percent" not in resource
    assert (
        resource["isolation"]["child_attestation"]["exec_sha256"] == resource["exec_command_sha256"]
    )
    bound = pairrun.bind_storage_metrics(paths["resource_path"], _storage())
    assert pairrun._validate_resource_metrics(bound, "qualified") == bound
    assert json.loads(paths["resource_path"].read_text()) == bound


def test_exploratory_group_is_diagnostic_and_cannot_upgrade(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    passed = {}

    def fake_owner(_command, **kwargs):
        passed.update(kwargs)
        return _result("process-group")

    monkeypatch.setattr(pairrun.linux_process, "run", fake_owner)
    paths = _paths(tmp_path)
    subject = tmp_path / "record.json"
    subject.write_text("record", encoding="utf-8")
    resource = pairrun.run_monitored_process(
        ["/bin/true"], **paths, timeout_secs=5, subject_path=subject
    )
    assert passed["qualified"] is False
    assert passed["cgroup_parent"] is None
    assert passed["pass_fds"] == ()
    assert resource["capture_scope"] == "exploratory"
    assert resource["owner_backend"] == "process-group"
    assert resource["ownership_complete"] is False
    assert resource["peak_cgroup_memory_bytes"] is None
    bound = pairrun.bind_storage_metrics(paths["resource_path"], _storage())
    pairrun._validate_resource_metrics(bound, "diagnostic")
    upgraded = dict(bound, capture_scope="qualified")
    with pytest.raises(pairrun.RunError, match="qualified Linux requires"):
        pairrun._validate_resource_metrics(upgraded, "upgraded")
    forged = dict(bound, ownership_complete=True)
    with pytest.raises(pairrun.RunError, match="ownership_complete"):
        pairrun._validate_resource_metrics(forged, "forged")


def test_linux_schema_rejects_fabricated_or_incomplete_evidence(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    parent = tmp_path / "delegated"
    parent.mkdir()
    monkeypatch.setattr(
        pairrun.linux_process, "run", lambda *_a, **_kw: _result("cgroup-v2", parent)
    )
    paths = _paths(tmp_path)
    subject = tmp_path / "record.json"
    subject.write_text("record", encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="did not attest"):
        pairrun.run_monitored_process(
            ["/usr/bin/python3", "linux_isolation.py", "--", "/bin/true"],
            **paths,
            timeout_secs=5,
            subject_path=subject,
            isolation=_isolation(),
            capture_scope="qualified",
            linux_cgroup_parent=str(parent),
        )
    assert not paths["resource_path"].exists()


@pytest.mark.parametrize("scope", ["exploratory", "qualified"])
def test_qualified_linux_requires_explicit_delegation_before_launch(tmp_path, monkeypatch, scope):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    monkeypatch.setattr(
        pairrun.linux_process,
        "run",
        lambda *_a, **_kw: pytest.fail("owner must not launch"),
    )
    if scope == "qualified":
        with pytest.raises(pairrun.RunError, match="delegated cgroup v2 and Landlock"):
            pairrun.run_monitored_process(
                ["/bin/true"],
                **_paths(tmp_path),
                timeout_secs=5,
                capture_scope=scope,
            )
        with pytest.raises(pairrun.RunError, match="linux_cgroup_parent"):
            pairrun.run_pair({"scope": scope, "blinding": "isolated"})
    else:
        with pytest.raises(pairrun.RunError, match="must not claim a cgroup parent"):
            pairrun.run_monitored_process(
                ["/bin/true"],
                **_paths(tmp_path),
                timeout_secs=5,
                linux_cgroup_parent="/sys/fs/cgroup/delegated",
            )
    assert not (tmp_path / "resource.json").exists()


def test_spec_requires_absolute_delegated_parent(tmp_path):
    spec = {
        "spec_version": 2,
        "repo": "repo",
        "manifest": "manifest",
        "suite": "suite",
        "query_pack": "pack",
        "execution_profiles": {"quanta": query_plan.execution_profile("native")},
        "top_k": 1,
        "output_root": "out",
        "runner_binary": "runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": "searchd",
        "searchd_expected_sha256": "a" * 64,
        "scope": "qualified",
        "linux_cgroup_parent": "relative",
    }
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="absolute delegated path"):
        pairrun.load_spec(path)
