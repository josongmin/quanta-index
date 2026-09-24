"""Focused contract and Linux child-process checks for linux_isolation."""

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import linux_isolation as isolation
from tools.benchmark.retrieval import run as pairrun


def _policy(tmp_path):
    allowed = tmp_path / "allowed"
    denied = tmp_path / "gold"
    output = tmp_path / "output"
    allowed.mkdir()
    output.mkdir()
    denied.write_text("hidden", encoding="utf-8")
    (allowed / "query").write_text("visible", encoding="utf-8")
    return {"readonly": [str(allowed)], "writable": [str(output)], "denied": [str(denied)]}


def test_policy_rejects_broad_grant(tmp_path):
    policy = _policy(tmp_path)
    policy["readonly"] = [str(tmp_path)]
    with pytest.raises(isolation.IsolationError, match="overlaps a denied path"):
        isolation.validate_policy(policy)


def test_policy_rejects_grant_nested_below_denied(tmp_path):
    policy = _policy(tmp_path)
    secret = tmp_path / "secret"
    secret.mkdir()
    child = secret / "child"
    child.mkdir()
    policy["denied"] = [str(secret)]
    policy["readonly"].append(str(child))
    with pytest.raises(isolation.IsolationError, match="overlaps a denied path"):
        isolation.validate_policy(policy)


def test_policy_rejects_proc_and_symlink(tmp_path):
    policy = _policy(tmp_path)
    if Path("/proc").exists():
        policy["readonly"].append("/proc")
        with pytest.raises(isolation.IsolationError, match="/proc cannot be granted"):
            isolation.validate_policy(policy)
        policy["readonly"].pop()
    link = tmp_path / "alias"
    link.symlink_to(tmp_path / "allowed")
    policy["readonly"].append(str(link))
    with pytest.raises(isolation.IsolationError, match="noncanonical"):
        isolation.validate_policy(policy)


def test_policy_requires_exact_fields(tmp_path):
    policy = _policy(tmp_path)
    policy["allow_all"] = True
    with pytest.raises(isolation.IsolationError, match="exactly"):
        isolation.validate_policy(policy)


def test_probe_reports_unavailable_explicitly():
    state = isolation.probe()
    assert state["backend"] == isolation.BACKEND
    assert state["state"] in {"available", "unavailable", "error"}
    if sys.platform != "linux":
        assert state["state"] == "unavailable"


def test_cli_rejects_policy_before_exec(tmp_path):
    policy = _policy(tmp_path)
    policy["readonly"] = [str(tmp_path)]
    path = tmp_path / "policy.json"
    path.write_text(json.dumps(policy), encoding="utf-8")
    assert (
        isolation.main(["--policy", str(path), "--", str(Path(sys.executable).resolve())])
        == isolation.EXIT_REJECTED
    )


def test_pair_capture_refuses_unavailable_landlock(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    monkeypatch.setattr(
        isolation,
        "probe",
        lambda: {"backend": isolation.BACKEND, "state": "unavailable", "reason": "ENOSYS"},
    )
    with pytest.raises(pairrun.RunError, match="Linux Landlock unavailable"):
        pairrun.prepare_isolation({"blinding": "isolated"}, tmp_path, tmp_path / "suite")
    assert not (tmp_path / "isolation-proof.json").exists()


def test_linux_capture_context_binds_policy_bytes(tmp_path):
    policy_path = tmp_path / "policy.json"
    policy_path.write_text("{}\n", encoding="utf-8")
    module = tmp_path / "linux_isolation.py"
    module.write_text("module", encoding="utf-8")
    suite = tmp_path / "gold"
    suite.write_text("secret", encoding="utf-8")
    pack = tmp_path / "pack"
    pack.write_text("query", encoding="utf-8")
    python = Path(sys.executable).resolve()
    context = {
        "backend": isolation.BACKEND,
        "module": str(module),
        "module_sha256": pairrun.sha_file(module),
        "python": str(python),
        "python_sha256": pairrun.sha_file(python),
        "policy_path": str(policy_path),
        "suite_path": str(suite),
        "suite_sha256": pairrun.sha_file(suite),
        "pack_path": str(pack),
        "pack_sha256": pairrun.sha_file(pack),
        "policy_sha256": pairrun.sha_file(policy_path),
        "proof_sha256": "a" * 64,
    }
    spec = {"blinding": "isolated", "_isolation": context}
    wrapped, evidence = pairrun.sandbox_command(spec, ["/bin/true"])
    assert wrapped[-2:] == ["--", "/bin/true"]
    assert evidence == {
        "backend": isolation.BACKEND,
        "policy_sha256": context["policy_sha256"],
        "proof_sha256": context["proof_sha256"],
        "_child_check": {"pack_sha256": context["pack_sha256"]},
    }
    policy_path.write_text('{"tampered":true}\n', encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="policy digest drifted"):
        pairrun.sandbox_command(spec, ["/bin/true"])


def test_monitored_capture_rejects_missing_child_attestation(tmp_path):
    command = [str(Path(sys.executable).resolve()), "-c", "pass", "--"]
    with pytest.raises(pairrun.RunError, match="did not attest deny/allow"):
        pairrun.run_monitored_process(
            command,
            stdout_path=tmp_path / "stdout.log",
            stderr_path=tmp_path / "stderr.log",
            resource_path=tmp_path / "resource.json",
            timeout_secs=5,
            isolation={
                "backend": isolation.BACKEND,
                "policy_sha256": "a" * 64,
                "proof_sha256": "b" * 64,
                "_child_check": {"pack_sha256": "c" * 64},
            },
        )


def test_monitored_capture_binds_child_pipe_attestation(tmp_path, monkeypatch):
    monkeypatch.setattr(
        pairrun,
        "_process_tree_sample",
        lambda pid: [{"pid": pid, "command": "test-child", "rss_bytes": 1, "cpu_percent": 0.0}],
    )
    child = (
        "import hashlib,json,os,sys,time; "
        "a=sys.argv; fd=int(a[a.index('--attest-fd')+1]); "
        "nonce=a[a.index('--nonce')+1]; "
        "cmd=a[a.index('--')+1:]; "
        "sha=hashlib.sha256(json.dumps(cmd,separators=(',',':')).encode()).hexdigest(); "
        "m=dict(nonce=nonce,abi=5,exec_sha256=sha,suite_read_denied=True,"
        "query_pack_read_allowed=True,proc_read_denied=True); "
        "os.write(fd,(json.dumps(m)+'\\n').encode()); os.close(fd); time.sleep(0.3)"
    )
    subject = tmp_path / "record.json"
    subject.write_text("record", encoding="utf-8")
    resource = pairrun.run_monitored_process(
        [str(Path(sys.executable).resolve()), "-c", child, "--", "/bin/true"],
        stdout_path=tmp_path / "stdout.log",
        stderr_path=tmp_path / "stderr.log",
        resource_path=tmp_path / "resource.json",
        timeout_secs=5,
        subject_path=subject,
        isolation={
            "backend": isolation.BACKEND,
            "policy_sha256": "a" * 64,
            "proof_sha256": "b" * 64,
            "_child_check": {"pack_sha256": "c" * 64},
        },
    )
    assert resource["exit_code"] == 0
    assert resource["isolation"]["child_attestation"]["suite_read_denied"] is True
    assert len(resource["isolation"]["child_attestation"]["nonce"]) == 64
    assert (
        resource["isolation"]["child_attestation"]["exec_sha256"] == resource["exec_command_sha256"]
    )
    resource["storage"] = {
        "index_bytes": 1,
        "model_cache_bytes": 0,
        "parser_cache_bytes": 0,
        "embedding_cache_bytes": 0,
        "discovered_files": 1,
        "indexed_chunks": 1,
        "index_storage": "disk",
        "index_measurement": "filesystem_tree_v1",
    }
    pairrun._validate_resource_metrics(resource, "child")
    copied = json.loads(json.dumps(resource))
    copied["isolation"]["child_attestation"]["exec_sha256"] = "0" * 64
    with pytest.raises(pairrun.RunError, match="child deny/allow proof"):
        pairrun._validate_resource_metrics(copied, "copied")
    with pytest.raises(pairrun.RunError, match="reused"):
        pairrun._validate_unique_linux_attestations([resource["isolation"], copied["isolation"]])
    other = json.loads(json.dumps(resource["isolation"]))
    other["child_attestation"]["nonce"] = "d" * 64
    with pytest.raises(pairrun.RunError, match="reused"):
        pairrun._validate_unique_linux_attestations([resource["isolation"], other])
    other["child_attestation"]["exec_sha256"] = "e" * 64
    pairrun._validate_unique_linux_attestations([resource["isolation"], other])
    resource["isolation"]["child_attestation"]["suite_read_denied"] = False
    with pytest.raises(pairrun.RunError, match="child deny/allow proof"):
        pairrun._validate_resource_metrics(resource, "child")


def test_linux_policy_never_grants_stage_or_denied_source(tmp_path):
    stage = tmp_path / "capture.staging"
    stage.mkdir()
    (stage / "runner-tools").mkdir()
    corpus = stage / "runner-corpus"
    corpus.mkdir()
    source = tmp_path / "source"
    source.mkdir()
    secret = tmp_path / "secret"
    secret.mkdir()
    evaluator = stage / "evaluator-only"
    evaluator.mkdir()
    files = {}
    for name in (
        "manifest",
        "query_pack",
        "runner_binary",
        "searchd_binary",
        "semble_python",
        "semble_lockfile",
    ):
        path = tmp_path / name
        path.write_text(name, encoding="utf-8")
        files[name] = str(path)
    spec = {**files, "repo": str(corpus), "repetitions": 2}
    spec["semble_python"] = str(Path(sys.executable).resolve())
    policy = pairrun._linux_policy(spec, stage, [str(source), str(secret), str(evaluator)])
    assert str(stage) not in policy["readonly"] + policy["writable"]
    assert str(evaluator) not in policy["readonly"] + policy["writable"]
    assert str(stage / "rep-00") in policy["writable"]
    assert str(stage / "rep-01") in policy["writable"]
    isolation.validate_policy(policy)


def test_linux_prepare_writes_tagged_policy_proof(tmp_path, monkeypatch):
    stage = tmp_path / "capture.staging"
    stage.mkdir()
    evaluator = stage / "evaluator-only"
    evaluator.mkdir()
    suite = evaluator / "suite.json"
    suite.write_text("gold", encoding="utf-8")
    secret = tmp_path / "secret"
    secret.mkdir()
    original = secret / "suite.json"
    original.write_bytes(suite.read_bytes())
    source = tmp_path / "source"
    source.mkdir()
    corpus = stage / "runner-corpus"
    corpus.mkdir()
    admitted = corpus / "a.txt"
    admitted.write_text("corpus", encoding="utf-8")
    manifest = stage / "corpus-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "repository_commit": "a" * 40,
                "files": [{"path": "a.txt", "file_sha256": pairrun.sha_file(admitted)}],
            }
        ),
        encoding="utf-8",
    )
    pack = stage / "query-pack.json"
    pack.write_text("blind", encoding="utf-8")
    files = {}
    for name in ("runner_binary", "searchd_binary", "semble_lockfile"):
        path = tmp_path / name
        path.write_text(name, encoding="utf-8")
        files[name] = str(path)
    spec = {
        **files,
        "repo": str(corpus),
        "manifest": str(manifest),
        "query_pack": str(pack),
        "runner_binary": files["runner_binary"],
        "searchd_binary": files["searchd_binary"],
        "semble_python": str(Path(sys.executable).resolve()),
        "suite": str(suite),
        "output_root": str(tmp_path / "final"),
        "blinding": "isolated",
        "suite_secret_root": str(secret),
        "_source_repo": str(source),
        "_materialized_corpus": pairrun._verify_materialized_corpus(corpus, manifest),
    }
    monkeypatch.setattr(pairrun.platform, "system", lambda: "Linux")
    monkeypatch.setattr(isolation, "probe", lambda: {"state": "available", "abi": 5})
    monkeypatch.setattr(
        pairrun,
        "_probe_linux",
        lambda *_args: {
            "abi": 5,
            "probes": {
                "suite_read_denied": True,
                "query_pack_read_allowed": True,
                "proc_read_denied": True,
            },
        },
    )
    prepared = pairrun.prepare_isolation(spec, stage, original)
    proof = pairrun.read_json(stage / "isolation-proof.json")
    assert proof["schema_version"] == pairrun.ISOLATION_PROOF_VERSION
    assert proof["backend"] == isolation.BACKEND
    assert proof["landlock"]["threat_model"] == "filesystem-path-read-v1"
    assert prepared["isolation_method"] == isolation.BACKEND
    policy = pairrun.read_json(stage / "runner-input" / "landlock-policy.json")
    assert str(stage) not in policy["readonly"] + policy["writable"]
    isolation.validate_policy(policy)
    proof_path = stage / "isolation-proof.json"
    validation = dict(
        root=stage,
        suite_path=suite,
        pack_path=pack,
        proof_path=proof_path,
        source_repo=source,
        manifest_path=manifest,
    )
    verified = pairrun._validate_isolation_proof(proof, **validation)
    assert verified["backend"] == isolation.BACKEND
    assert verified["proof_sha256"] == pairrun.sha_file(proof_path)
    old = dict(proof, schema_version=1)
    with pytest.raises(pairrun.RunError, match="schema/backend mismatch"):
        pairrun._validate_isolation_proof(old, **validation)
    tampered = dict(proof, policy_sha256="0" * 64)
    with pytest.raises(pairrun.RunError, match="policy digest"):
        pairrun._validate_isolation_proof(tampered, **validation)
    final = tmp_path / "final"
    stage.rename(final)
    promoted = dict(
        root=final,
        suite_path=final / "evaluator-only" / "suite.json",
        pack_path=final / "query-pack.json",
        proof_path=final / "isolation-proof.json",
        source_repo=source,
        manifest_path=final / "corpus-manifest.json",
    )
    assert pairrun._validate_isolation_proof(proof, **promoted)["backend"] == isolation.BACKEND

    def unavailable(*_args):
        raise pairrun.RunError("Linux Landlock unavailable: ENOSYS")

    monkeypatch.setattr(pairrun, "_probe_linux", unavailable)
    with pytest.raises(pairrun.RunError, match="Landlock unavailable"):
        pairrun._validate_isolation_proof(proof, **promoted)


@pytest.mark.skipif(sys.platform != "linux", reason="Landlock needs a Linux kernel")
def test_linux_exec_child_reports_deny_allow_after_enforcement(tmp_path):
    if isolation.probe()["state"] != "available":
        pytest.skip("Landlock ABI 5 unavailable")
    raw = _policy(tmp_path)
    python = Path(sys.executable).resolve()
    runtime = [python.parent.parent, Path("/lib"), Path("/usr/lib"), Path("/dev/urandom")]
    raw["readonly"].extend(str(path.resolve()) for path in runtime if path.exists())
    raw["writable"].append("/dev/null")
    isolation.validate_policy(raw)
    policy_path = tmp_path / "policy.json"
    policy_path.write_text(json.dumps(raw), encoding="utf-8")
    pack = Path(raw["readonly"][0]) / "query"
    read_fd, write_fd = os.pipe()
    try:
        completed = subprocess.run(
            [
                str(python),
                "-m",
                "tools.benchmark.retrieval.linux_isolation",
                "--policy",
                str(policy_path),
                "--suite",
                raw["denied"][0],
                "--query-pack",
                str(pack),
                "--query-pack-sha256",
                pairrun.sha_file(pack),
                "--attest-fd",
                str(write_fd),
                "--nonce",
                "a" * 64,
                "--",
                str(python),
                "-c",
                "print('runner-executed')",
            ],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            pass_fds=(write_fd,),
            timeout=15,
        )
    finally:
        os.close(write_fd)
    try:
        attestation = json.loads(os.read(read_fd, 4096))
    finally:
        os.close(read_fd)
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout == b"runner-executed\n"
    assert attestation == {
        "nonce": "a" * 64,
        "abi": isolation.probe()["abi"],
        "exec_sha256": pairrun.digest(
            pairrun.canonical([str(python), "-c", "print('runner-executed')"])
        ),
        "suite_read_denied": True,
        "query_pack_read_allowed": True,
        "proc_read_denied": True,
    }


@pytest.mark.skipif(sys.platform != "linux", reason="Landlock needs a Linux kernel")
def test_linux_child_blocks_gold_and_proc(tmp_path):
    if isolation.probe()["state"] != "available":
        pytest.skip("Landlock ABI 5 unavailable")
    raw = _policy(tmp_path)
    policy = isolation.validate_policy(raw)
    pid = os.fork()
    if pid == 0:
        try:
            null = os.open("/dev/null", os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            os.close(null)
            os.closerange(3, 1024)
            isolation.enforce(policy)
            with open(Path(raw["readonly"][0]) / "query", encoding="utf-8") as stream:
                if stream.read() != "visible":
                    os._exit(1)
            for path in (raw["denied"][0], "/proc/self/environ"):
                try:
                    with open(path, "rb"):
                        os._exit(2)
                except PermissionError:
                    pass
            os._exit(0)
        except BaseException:
            os._exit(3)
    _, status = os.waitpid(pid, 0)
    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0


@pytest.mark.skipif(sys.platform != "linux", reason="Landlock needs a Linux kernel")
def test_linux_child_rejects_preopened_fd(tmp_path):
    if isolation.probe()["state"] != "available":
        pytest.skip("Landlock ABI 5 unavailable")
    raw = _policy(tmp_path)
    policy = isolation.validate_policy(raw)
    pid = os.fork()
    if pid == 0:
        try:
            null = os.open("/dev/null", os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            os.close(null)
            os.closerange(3, 1024)
            secret_fd = os.open(raw["denied"][0], os.O_RDONLY)
            try:
                isolation.enforce(policy)
            except isolation.IsolationError:
                os.close(secret_fd)
                os._exit(0)
            os._exit(1)
        except BaseException:
            os._exit(2)
    _, status = os.waitpid(pid, 0)
    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
